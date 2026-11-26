//! Bounded audio previews and gain-matched listening pairs from render jobs.
use crate::jobs::JobReport;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, RandomState};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const FADE_SECONDS: f64 = 0.012;
const TARGET_PEAK_DBTP: f64 = -2.1;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Meter {
    lufs: f64,
    true_peak_dbtp: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuditionPair {
    id: String,
    question: String,
    offset_seconds: f64,
    duration_seconds: f64,
    pub audio_a: PathBuf,
    pub audio_b: PathBuf,
    a_measured: Meter,
    b_measured: Meter,
}

#[derive(Serialize, Deserialize)]
struct Source {
    job_id: String,
    project_hash: String,
    gain_db: f64,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    pair: AuditionPair,
    a: Source,
    b: Source,
}

#[derive(Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Preference {
    A,
    B,
    Tie,
    Neither,
}

fn render_wav(report: &JobReport) -> Result<&Path> {
    ensure!(
        report.state == "completed",
        "render {} is not complete",
        report.id
    );
    let wav = report
        .artifacts
        .iter()
        .find(|path| path.file_name().is_some_and(|name| name == "audio.wav"))
        .context("render has no audio artifact")?;
    ensure!(
        fs::symlink_metadata(wav)?.is_file(),
        "render audio must be a regular file"
    );
    Ok(wav)
}

fn excerpt(path: &Path, offset: f64, duration: f64, limit: f64) -> Result<(Vec<f32>, u32)> {
    ensure!(
        offset.is_finite() && offset >= 0.0,
        "offset must be finite and nonnegative"
    );
    ensure!(
        (0.25..=limit).contains(&duration),
        "excerpt duration must be 0.25–{limit} seconds"
    );
    let mut wav = hound::WavReader::open(path).context("open render WAV")?;
    let spec = wav.spec();
    ensure!(
        spec.channels == 2
            && spec.sample_format == hound::SampleFormat::Float
            && spec.bits_per_sample == 32
            && spec.sample_rate == 44_100,
        "expected daw-ai's 44.1 kHz stereo float render"
    );
    let start = (offset * f64::from(spec.sample_rate)).round();
    let frames = (duration * f64::from(spec.sample_rate)).round() as u32;
    ensure!(
        start + f64::from(frames) <= f64::from(wav.duration()),
        "excerpt extends past the render"
    );
    wav.seek(start as u32)?;
    let samples: Vec<f32> = wav
        .samples::<f32>()
        .take(frames as usize * 2)
        .collect::<std::result::Result<_, _>>()?;
    ensure!(
        samples.len() == frames as usize * 2 && samples.iter().all(|s| s.is_finite()),
        "incomplete or invalid WAV samples"
    );
    Ok((samples, spec.sample_rate))
}

/// Small PCM audio block for clients that can receive audio from MCP.
pub fn preview(report: &JobReport, offset: f64, duration: f64) -> Result<Vec<u8>> {
    let (samples, rate) = excerpt(render_wav(report)?, offset, duration, 6.0)?;
    encode_preview(&samples, rate, false)
}

pub(crate) fn encode_preview(samples: &[f32], rate: u32, stereo: bool) -> Result<Vec<u8>> {
    let mut bytes = Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels: if stereo { 2 } else { 1 },
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::new(&mut bytes, spec)?;
    let (frames, _) = samples.as_chunks::<2>();
    let pcm = |sample: f32| (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
    for &[left, right] in frames {
        if stereo {
            writer.write_sample(pcm(left))?;
            writer.write_sample(pcm(right))?;
        } else {
            writer.write_sample(pcm(left * 0.5 + right * 0.5))?;
        }
    }
    writer.finalize()?;
    Ok(bytes.into_inner())
}

fn measure(path: &Path) -> Result<Meter> {
    let output = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-nostats",
            "-filter_threads",
            "1",
            "-i",
        ])
        .arg(path)
        .args(["-af", "loudnorm=print_format=json", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output()
        .context("install FFmpeg with the loudnorm filter to prepare audition pairs")?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    ensure!(
        output.status.success(),
        "FFmpeg loudness measurement failed: {}",
        stderr.lines().last().unwrap_or("no diagnostic")
    );
    let (Some(start), Some(end)) = (stderr.rfind('{'), stderr.rfind('}')) else {
        anyhow::bail!("FFmpeg returned no loudness measurements");
    };
    let values: serde_json::Value = serde_json::from_str(
        stderr
            .get(start..=end)
            .context("invalid FFmpeg loudness report")?,
    )?;
    let value = |field: &str| -> Result<f64> {
        let number: f64 = values[field]
            .as_str()
            .with_context(|| format!("missing loudness field {field}"))?
            .parse()?;
        ensure!(
            number.is_finite(),
            "excerpt has no measurable loudness; choose a passage with audio"
        );
        Ok(number)
    };
    Ok(Meter {
        lufs: value("input_i")?,
        true_peak_dbtp: value("input_tp")?,
    })
}

fn fade(samples: &mut [f32], rate: u32) {
    let (stereo_frames, _) = samples.as_chunks_mut::<2>();
    let frames = stereo_frames.len();
    let fade_frames = (f64::from(rate) * FADE_SECONDS).round() as usize;
    for (frame, [left, right]) in stereo_frames.iter_mut().enumerate() {
        let gain = (frame.min(frames - 1 - frame) as f32 / fade_frames as f32).min(1.0);
        *left *= gain;
        *right *= gain;
    }
}

fn audition_root(base: &Path) -> Result<PathBuf> {
    let root = base.canonicalize()?.join("auditions");
    match fs::create_dir(&root) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err.into()),
    }
    ensure!(
        fs::symlink_metadata(&root)?.is_dir(),
        "auditions must be a local directory"
    );
    Ok(root)
}

struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn prepare(
    base: &Path,
    mut a: JobReport,
    mut b: JobReport,
    offset: f64,
    duration: f64,
    question: String,
) -> Result<AuditionPair> {
    ensure!(a.id != b.id, "choose two different renders");
    ensure!(
        a.range == b.range && a.sample_rate == b.sample_rate && a.normalized == b.normalized,
        "render both candidates over the same bar range with matching settings"
    );
    ensure!(
        (5.0..=30.0).contains(&duration),
        "audition pairs need 5–30 seconds"
    );
    ensure!(
        !question.trim().is_empty() && question.len() <= 1000,
        "provide a listening question of up to 1000 bytes"
    );
    let id = daw_io::session::operation_id();
    if RandomState::new().hash_one(&id) & 1 != 0 {
        std::mem::swap(&mut a, &mut b);
    }
    let (mut a_audio, rate) = excerpt(render_wav(&a)?, offset, duration, 30.0)?;
    let (mut b_audio, _) = excerpt(render_wav(&b)?, offset, duration, 30.0)?;
    fade(&mut a_audio, rate);
    fade(&mut b_audio, rate);

    let root = audition_root(base)?;
    let dir = root.join(&id);
    let temp = root.join(format!(".{id}"));
    fs::create_dir(&temp)?;
    let staging = Staging(temp);
    let a_path = staging.0.join("a.wav");
    let b_path = staging.0.join("b.wav");
    let write =
        |path: &Path, audio: &[f32]| daw_core::audio::engine::write_wav(path, audio, rate, 2);
    write(&a_path, &a_audio)?;
    write(&b_path, &b_audio)?;
    let a_input = measure(&a_path)?;
    let b_input = measure(&b_path)?;
    let safe_level = |m: &Meter| m.lufs + (TARGET_PEAK_DBTP - m.true_peak_dbtp).min(0.0);
    let target = safe_level(&a_input).min(safe_level(&b_input));
    let attenuate = |audio: &mut [f32], meter: &Meter| {
        let gain_db = (target - meter.lufs).min(0.0);
        let gain = 10f32.powf(gain_db as f32 / 20.0);
        for sample in audio {
            *sample *= gain;
        }
        gain_db
    };
    let a_gain = attenuate(&mut a_audio, &a_input);
    let b_gain = attenuate(&mut b_audio, &b_input);
    write(&a_path, &a_audio)?;
    write(&b_path, &b_audio)?;
    let a_measured = measure(&a_path)?;
    let b_measured = measure(&b_path)?;
    ensure!(
        (a_measured.lufs - b_measured.lufs).abs() <= 0.5,
        "could not match loudness within 0.5 LU using gain alone; choose another passage"
    );
    ensure!(
        [&a_measured, &b_measured]
            .iter()
            .all(|m| m.true_peak_dbtp <= -2.0),
        "audition exceeds the −2 dBTP ceiling"
    );

    let manifest = Manifest {
        pair: AuditionPair {
            id,
            question,
            offset_seconds: offset,
            duration_seconds: duration,
            audio_a: dir.join("a.wav"),
            audio_b: dir.join("b.wav"),
            a_measured,
            b_measured,
        },
        a: Source {
            job_id: a.id,
            project_hash: a.project_hash,
            gain_db: a_gain,
        },
        b: Source {
            job_id: b.id,
            project_hash: b.project_hash,
            gain_db: b_gain,
        },
    };
    fs::write(
        staging.0.join("mapping.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    fs::rename(&staging.0, &dir)?;
    Ok(manifest.pair)
}

pub fn record_choice(
    base: &Path,
    id: &str,
    preference: Preference,
    reason: &str,
) -> Result<serde_json::Value> {
    ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid audition id"
    );
    ensure!(
        reason.len() <= 4000,
        "listener comment can contain up to 4000 bytes"
    );
    let dir = audition_root(base)?.join(id);
    ensure!(
        fs::symlink_metadata(&dir)?.is_dir(),
        "audition must be a local directory"
    );
    let manifest_path = dir.join("mapping.json");
    ensure!(
        fs::symlink_metadata(&manifest_path)?.is_file(),
        "invalid audition manifest"
    );
    let manifest: Manifest = serde_json::from_reader(File::open(manifest_path)?.take(64 * 1024))?;
    ensure!(
        manifest.pair.id == id,
        "audition manifest id does not match"
    );
    let result = serde_json::json!({"audition_id": id, "question": manifest.pair.question,
        "listener_preference": preference, "comment": reason, "a": manifest.a, "b": manifest.b});
    let path = dir.join(format!("choice-{}.json", daw_io::session::operation_id()));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    output.write_all(&serde_json::to_vec_pretty(&result)?)?;
    output.sync_all()?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loudness_pair_keeps_sources_and_records_a_listener_choice() {
        let temp = Staging(std::env::temp_dir().join(format!(
            "daw-audition-test-{}",
            daw_io::session::operation_id()
        )));
        fs::create_dir(&temp.0).unwrap();
        let make_report = |id: &str, amplitude| {
            let folder = temp.0.join(id);
            fs::create_dir(&folder).unwrap();
            let wav = folder.join("audio.wav");
            let mut samples = Vec::new();
            for frame in 0..(44_100 * 5) {
                let sample =
                    amplitude * (std::f32::consts::TAU * 440.0 * frame as f32 / 44_100.0).sin();
                samples.extend([sample, sample]);
            }
            daw_core::audio::engine::write_wav(&wav, &samples, 44_100, 2).unwrap();
            JobReport {
                id: id.into(),
                state: "completed".into(),
                progress: 100,
                project_hash: format!("hash-{id}"),
                session_revision: None,
                engine_version: "test".into(),
                sample_rate: 44_100,
                normalized: false,
                dry: false,
                warmup: "test".into(),
                range: Some((0, 3)),
                artifacts: vec![wav],
                assets: Default::default(),
                analysis: None,
                error: None,
            }
        };
        let first = make_report("first", 0.1);
        let second = make_report("second", 0.4);
        let input = fs::read(&second.artifacts[0]).unwrap();
        let excerpt = preview(&first, 1.0, 3.0).unwrap();
        let wav = hound::WavReader::new(Cursor::new(excerpt)).unwrap();
        assert_eq!(wav.spec().channels, 1);
        assert_eq!(wav.duration(), 3 * 44_100);
        assert!(preview(&first, 4.0, 2.0).is_err());

        let pair = prepare(
            &temp.0,
            first,
            second.clone(),
            0.0,
            5.0,
            "Which take works?".into(),
        )
        .unwrap();
        assert!(pair.audio_a.is_file() && pair.audio_b.is_file());
        assert_eq!(fs::read(&second.artifacts[0]).unwrap(), input);
        assert!((pair.a_measured.lufs - pair.b_measured.lufs).abs() <= 0.5);
        let vote = record_choice(&temp.0, &pair.id, Preference::B, "Test fixture comment").unwrap();
        assert_eq!(vote["listener_preference"], "b");
        assert_eq!(vote["comment"], "Test fixture comment");
        let louder = if vote["a"]["job_id"] == "second" {
            "a"
        } else {
            "b"
        };
        assert!(vote[louder]["gain_db"].as_f64().unwrap() < -11.0);
        assert!(record_choice(&temp.0, "../escape", Preference::A, "").is_err());
    }
}

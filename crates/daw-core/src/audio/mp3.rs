//! MP3 encoding runs outside the audio thread. Only a completed file is published.
use crate::render_control::RenderControl;
use anyhow::{ensure, Context, Result};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const BITRATE_KBPS: u32 = 320;
// Leave room for lossy encoding's reconstructed peaks. This is a PCM peak
// ceiling, not a guarantee of decoded true peak for arbitrary material.
const PEAK_CEILING: f32 = 0.7943282; // -2 dBFS
const ENCODER_HELP: &str =
    "MP3 export needs FFmpeg with the libmp3lame encoder; install FFmpeg and restart the app";

pub fn check_encoder() -> Result<()> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .output()
        .context(ENCODER_HELP)?;
    ensure!(output.status.success(), "{ENCODER_HELP}");
    ensure!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.split_whitespace().nth(1) == Some("libmp3lame")),
        "{ENCODER_HELP}"
    );
    Ok(())
}

struct Scratch(PathBuf);
impl Scratch {
    fn beside(output: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let directory = parent.join(format!(
            ".daw-mp3-{}-{id}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory)?;
        Ok(Self(directory))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Encoder(Child);
impl Drop for Encoder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Encode interleaved stereo PCM as 320 kbps MP3. Quiet material keeps its gain;
/// peaks above -2 dBFS are attenuated uniformly for encoding headroom.
/// A failed/cancelled encode leaves an existing destination untouched.
pub fn write_mp3(
    output: &Path,
    samples: &[f32],
    sample_rate: u32,
    control: &RenderControl,
) -> Result<()> {
    write_with_encoder(Path::new("ffmpeg"), output, samples, sample_rate, control)
}

fn write_with_encoder(
    encoder: &Path,
    output: &Path,
    samples: &[f32],
    sample_rate: u32,
    control: &RenderControl,
) -> Result<()> {
    control.check()?;
    ensure!(
        !samples.is_empty() && samples.len().is_multiple_of(2),
        "MP3 export requires complete stereo frames"
    );
    ensure!(
        matches!(sample_rate, 32000 | 44100 | 48000),
        "320 kbps MP3 requires 32, 44.1 or 48 kHz audio"
    );
    ensure!(
        samples.iter().all(|s| s.is_finite()),
        "MP3 export contains non-finite audio"
    );
    let scratch = Scratch::beside(output)?;
    let wav = scratch.0.join("input.wav");
    let mp3 = scratch.0.join("output.mp3");
    let errors = scratch.0.join("encoder.log");
    let peak = samples.iter().fold(0f32, |p, s| p.max(s.abs()));
    let gain = if peak > PEAK_CEILING {
        PEAK_CEILING / peak
    } else {
        1.
    };
    let mut writer = hound::WavWriter::create(
        &wav,
        hound::WavSpec {
            channels: 2,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )?;
    for block in samples.chunks(8192) {
        control.check()?;
        for &sample in block {
            writer.write_sample(sample * gain)?;
        }
    }
    writer.finalize()?;
    control.check()?;
    let child = Command::new(encoder)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&wav)
        .args([
            "-vn",
            "-c:a",
            "libmp3lame",
            "-b:a",
            &format!("{BITRATE_KBPS}k"),
            "-ac",
            "2",
            "-id3v2_version",
            "3",
        ])
        .arg(&mp3)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(&errors)?)
        .spawn()
        .context(ENCODER_HELP)?;
    let mut child = Encoder(child);
    let status = loop {
        control.check()?;
        if let Some(status) = child.0.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        let mut diagnostic = String::new();
        File::open(&errors)?
            .take(8192)
            .read_to_string(&mut diagnostic)?;
        anyhow::bail!("MP3 encoding failed ({status}): {}", diagnostic.trim());
    }
    ensure!(
        fs::metadata(&mp3)?.len() > 0,
        "MP3 encoder produced an empty file"
    );
    control.check()?;
    fs::rename(&mp3, output).context("could not save the completed MP3")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory() -> Scratch {
        Scratch::beside(&std::env::temp_dir().join("daw-mp3-test.mp3")).unwrap()
    }

    #[test]
    fn encoded_mp3_decodes_to_stereo_with_the_tail_and_expected_level() {
        check_encoder().expect("MP3 integration tests require FFmpeg with libmp3lame");
        let directory = test_directory();
        let output = directory.0.join("song with spaces.mp3");
        let sr = 44100;
        // Different signals in L/R, followed by a quiet, decaying effect tail.
        let audio: Vec<f32> = (0..sr * 2)
            .flat_map(|frame| {
                let t = frame as f32 / sr as f32;
                let level = if t < 1. {
                    0.25
                } else {
                    0.04 * (-(t - 1.) * 3.).exp()
                };
                [
                    level * (t * 440. * std::f32::consts::TAU).sin(),
                    level * (t * 660. * std::f32::consts::TAU).sin(),
                ]
            })
            .collect();
        write_mp3(&output, &audio, sr, &RenderControl::default()).unwrap();
        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&output)
            .args(["-f", "f32le", "-acodec", "pcm_f32le", "-"])
            .output()
            .unwrap();
        assert!(decoded.status.success());
        let values: Vec<f32> = decoded
            .stdout
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert_eq!(
            values.len(),
            audio.len(),
            "gapless decode preserves the tail's length"
        );
        let energy = |v: &[f32]| v.iter().map(|s| (*s as f64).powi(2)).sum::<f64>();
        assert!(
            energy(&values[(sr as usize * 3)..]) > 0.1,
            "quiet effect tail survives encoding"
        );
        assert!(
            (energy(&values) / energy(&audio) - 1.).abs() < 0.1,
            "quiet source gain is preserved"
        );
        let lr_difference: f32 = values.chunks_exact(2).map(|f| (f[0] - f[1]).abs()).sum();
        assert!(lr_difference > 1000., "stereo channels remain distinct");

        // A louder mix must receive the documented encoding headroom.
        let loud: Vec<f32> = audio.iter().map(|s| s * 4.).collect();
        write_mp3(&output, &loud, sr, &RenderControl::default()).unwrap();
        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&output)
            .args(["-f", "f32le", "-acodec", "pcm_f32le", "-"])
            .output()
            .unwrap();
        assert!(decoded.status.success());
        let values: Vec<f32> = decoded
            .stdout
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let expected_energy = energy(&loud) * (PEAK_CEILING as f64).powi(2);
        assert!((energy(&values) / expected_energy - 1.).abs() < 0.1);
        assert!(
            values.iter().all(|s| s.abs() < 1.),
            "this encoded fixture must decode without clipping"
        );
    }

    #[test]
    fn cancellation_invalid_audio_and_missing_encoder_preserve_destination() {
        let directory = test_directory();
        let output = directory.0.join("keep.mp3");
        fs::write(&output, b"existing recording").unwrap();
        let control = RenderControl::default();
        control.cancel();
        assert!(write_mp3(&output, &[0., 0.], 44100, &control)
            .unwrap_err()
            .to_string()
            .contains("cancelled"));
        assert!(write_mp3(&output, &[f32::NAN, 0.], 44100, &RenderControl::default()).is_err());
        assert!(write_with_encoder(
            &directory.0.join("missing-ffmpeg"),
            &output,
            &[0., 0.],
            44100,
            &RenderControl::default()
        )
        .unwrap_err()
        .to_string()
        .contains("FFmpeg"));
        assert_eq!(fs::read(&output).unwrap(), b"existing recording");
        assert_eq!(
            fs::read_dir(&directory.0).unwrap().count(),
            1,
            "staging files are removed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn encoder_failure_reports_error_and_removes_partial_file() {
        use std::os::unix::fs::PermissionsExt;
        let directory = test_directory();
        let encoder = directory.0.join("failed-encoder");
        fs::write(&encoder, "#!/bin/sh\nfor output do :; done\nprintf partial > \"$output\"\nprintf 'encoder diagnostic' >&2\nexit 17\n").unwrap();
        fs::set_permissions(&encoder, fs::Permissions::from_mode(0o700)).unwrap();
        let output = directory.0.join("keep.mp3");
        fs::write(&output, b"existing recording").unwrap();
        let error = write_with_encoder(
            &encoder,
            &output,
            &[0., 0.],
            44100,
            &RenderControl::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("encoder diagnostic"));
        assert_eq!(fs::read(&output).unwrap(), b"existing recording");
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_during_encoding_kills_worker_and_removes_partial_output() {
        use std::os::unix::fs::PermissionsExt;
        let directory = test_directory();
        let encoder = directory.0.join("slow-encoder");
        fs::write(
            &encoder,
            "#!/bin/sh\n# Stay in this process so cancellation can reap it.\nwhile :; do :; done\n",
        )
        .unwrap();
        fs::set_permissions(&encoder, fs::Permissions::from_mode(0o700)).unwrap();
        let output = directory.0.join("keep.mp3");
        fs::write(&output, b"existing recording").unwrap();
        let control = std::sync::Arc::new(RenderControl::default());
        let cancel = control.clone();
        let task = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            cancel.cancel();
        });
        let error = write_with_encoder(&encoder, &output, &[0., 0.], 44100, &control).unwrap_err();
        task.join().unwrap();
        assert!(error.to_string().contains("cancelled"));
        assert_eq!(fs::read(&output).unwrap(), b"existing recording");
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 2);
    }
}

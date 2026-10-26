use super::engine::write_wav;
use super::load_wav_stereo;
use super::mixer::{build_strips_report, AudioEvent, Mixer, StripMeter, StripSpec};
use crate::project::{Instrument, Pattern, Project, TrackType};
use crate::sequencer::engine::{piano_note_duration, note_fire_tick, humanize_velocity, mod_base01, modulated_value, pattern_length_ticks, swung_step_tick, TICKS_PER_BAR, TICKS_PER_BEAT};
use anyhow::Result;
use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};
use std::path::Path;
use std::sync::Arc;

const EXPORT_SR: u32 = 44100;
const BLOCK: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat { Wav, Mp3 }

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self { Self::Wav => "wav", Self::Mp3 => "mp3" }
    }

    pub fn label(self) -> &'static str {
        match self { Self::Wav => "WAV", Self::Mp3 => "MP3" }
    }

    pub fn check_available(self) -> Result<()> {
        match self { Self::Wav => Ok(()), Self::Mp3 => super::mp3::check_encoder() }
    }

    pub fn write(self, output: &Path, samples: &[f32], sr: u32, control: &crate::render_control::RenderControl) -> Result<()> {
        control.check()?;
        match self {
            Self::Wav => write_wav(output, samples, sr, 2),
            Self::Mp3 => super::mp3::write_mp3(output, samples, sr, control),
        }
    }
}

/// Preserve the legacy 2 s pad for legacy spatial settings. Expanded spaces
/// receive a conservative decay budget, capped at 30 s per full-song render.
/// Explicit ranges still end at their requested sample, including warm-up.
fn spatial_tail_seconds(project: &Project) -> f64 {
    let mut tail: f64=2.;
    let r=&project.returns.reverb;
    if !r.muted && r.gain>0. && project.tracks.iter().any(|t|!t.muted && t.effects.reverb>0.)
        && (r.predelay_ms>0 || r.low_cut_hz>0. || r.high_cut_hz<20000.) {
        let feedback=0.7+0.28*r.room_size as f64;
        tail=tail.max(r.predelay_ms as f64/1000. + (0.001f64.ln()/feedback.ln())*(1617./44100.)+0.3);
    }
    let d=&project.returns.delay;
    if !d.muted && d.gain>0. && project.tracks.iter().any(|t|!t.muted && t.effects.send_delay>0.)
        && (d.sync.is_some() || d.time_ms>2000 || d.low_cut_hz>0. || d.high_cut_hz<20000. || d.modulation_ms>0. || d.width<1. || d.mode==crate::project::EchoMode::Stereo) {
        let repeats=if d.feedback>0. { (0.001f64.ln()/(d.feedback as f64).ln()).ceil()+1. } else {1.};
        tail=tail.max(d.effective_time_ms(project.project.bpm) as f64/1000.*repeats+d.modulation_ms as f64/1000.+0.1);
    }
    tail.clamp(2.,30.)
}

/// Total ticks of the full arrangement (max end-tick across all slots).
fn arrangement_total_ticks(project: &Project) -> u32 {
    project.arrangement.iter()
        .filter_map(|sl| {
            let pat = project.patterns.iter().find(|p| p.id() == sl.pattern)?;
            // Resized clips (len_bars) extend the song; the pattern loops to fill.
            let len = sl.len_bars
                .map(|b| b * TICKS_PER_BAR)
                .unwrap_or_else(|| pattern_length_ticks(pat));
            Some(sl.bar * TICKS_PER_BAR + sl.offset_ticks + len)
        })
        .max()
        .unwrap_or(TICKS_PER_BAR * 4)
}

/// Fallback for projects with no arrangement: length of the longest first-pattern.
fn loop_total_ticks(project: &Project) -> u32 {
    project.tracks.iter()
        .filter(|t| t.track_type != TrackType::Audio && !t.muted)
        .flat_map(|t| t.patterns.iter())
        .filter_map(|pid| project.patterns.iter().find(|p| p.id() == pid))
        .map(pattern_length_ticks)
        .max()
        .unwrap_or(TICKS_PER_BAR * 4)
}

/// Return (pattern, local_tick_within_pattern) for the arrangement slot that
/// covers `tick` for `track_id`. Returns None if no slot is active at `tick`.
fn arr_pat_tick<'a>(project: &'a Project, track_id: &str, tick: u32) -> Option<(&'a Pattern, u32)> {
    for sl in &project.arrangement {
        if sl.track != track_id { continue; }
        let Some(pat) = project.patterns.iter().find(|p| p.id() == sl.pattern) else { continue };
        let start = sl.bar * TICKS_PER_BAR + sl.offset_ticks;
        let natural = pattern_length_ticks(pat).max(1);
        // Resized clips loop the pattern to fill their length (same as live).
        let len = sl.len_bars.map(|b| b * TICKS_PER_BAR).unwrap_or(natural);
        if tick >= start && tick < start + len {
            return Some((pat, (tick - start) % natural));
        }
    }
    None
}

/// Return (pattern, local_tick) for loop mode (no arrangement).
fn loop_pat_tick<'a>(project: &'a Project, track_id: &str, tick: u32) -> Option<(&'a Pattern, u32)> {
    let pid = project.tracks.iter().find(|t| t.id == track_id)?.patterns.first()?;
    let pat = project.patterns.iter().find(|p| p.id() == pid)?;
    Some((pat, tick % pattern_length_ticks(pat)))
}

/// Render the full project to a stereo 32-bit float WAV at 44100 Hz, through
/// the same `Mixer` as live playback; device and scheduling behavior may differ.
/// Respects the arrangement timeline; falls back to single-loop if empty.
/// Render the whole project to interleaved stereo f32 at 44.1 kHz.
/// `normalize` applies the -1 dBFS peak normalization (exports do; the
/// auto-balance measurement must not).
pub fn render_project_samples(project: &Project, sf2_path: &Path, normalize: bool) -> Result<(Vec<f32>, u32)> {
    render_range_samples(project, sf2_path, normalize, None, &crate::render_control::RenderControl::default())
}

/// Render from time zero to preserve synth/FX history, retain only the requested
/// range, and stop after its end. Block boundaries match a full render exactly.
pub fn render_range_samples(project: &Project, sf2_path: &Path, normalize: bool, range: Option<(u32, u32)>, control: &crate::render_control::RenderControl) -> Result<(Vec<f32>, u32)> {
    project.validate()?;
    control.check()?;
    if let Some((start, end)) = range { anyhow::ensure!(start < end, "render range must have start < end"); }
    let bpm   = project.project.bpm;
    let swing = project.project.swing;
    let spt   = 60.0 / bpm / TICKS_PER_BEAT as f64; // seconds per tick
    let spk   = spt * EXPORT_SR as f64;              // samples per tick

    let has_arr   = !project.arrangement.is_empty();
    let max_ticks = if has_arr { arrangement_total_ticks(project) } else { loop_total_ticks(project) };
    // Cover arranged audio clips that extend past the pattern timeline.
    let audio_end_samp = project.audio_clips.iter()
        .map(|c| ((c.start_sec + c.len_sec) * EXPORT_SR as f64).ceil() as u64)
        .max()
        .unwrap_or(0);
    let total_samp = ((max_ticks as f64 * spk).ceil() as u64)
        .max(audio_end_samp)
        + (spatial_tail_seconds(project)*EXPORT_SR as f64).ceil() as u64;

    let start_sample = range.map(|(start, _)| crate::musical_time::bar_sample(start, bpm, EXPORT_SR)).unwrap_or(0);
    let end_sample = range.map(|(_, end)| crate::musical_time::bar_sample(end, bpm, EXPORT_SR)).unwrap_or(total_samp).min(total_samp);
    anyhow::ensure!(start_sample < end_sample, "render range is outside the song");
    let total_samp = total_samp.min(end_sample.div_ceil(BLOCK as u64) * BLOCK as u64);
    anyhow::ensure!(total_samp <= EXPORT_SR as u64 * 1800, "render exceeds the 30-minute resource limit");
    let max_ticks = max_ticks.min((total_samp as f64 / spk).ceil() as u32);

    // ── The mixer: same strips as live playback ───────────────────────────────
    let specs = StripSpec::from_project(project);
    let (strips, _meters, failures) = build_strips_report(&specs, sf2_path, EXPORT_SR);
    anyhow::ensure!(failures.is_empty(), "{}", failures.join("; "));
    let utility: Option<Synthesizer> = if sf2_path.exists() {
        let mut f = std::fs::File::open(sf2_path)?;
        let sf = Arc::new(SoundFont::new(&mut f)?);
        Synthesizer::new(&sf, &SynthesizerSettings::new(EXPORT_SR as i32)).ok()
    } else {
        None
    };
    let mut mixer = Mixer::new(
        EXPORT_SR,
        utility,
        Arc::new(StripMeter::new()),
        [Arc::new(StripMeter::new()), Arc::new(StripMeter::new())],
    );
    mixer.handle_event(AudioEvent::SetStrips { strips });
    mixer.handle_event(AudioEvent::SetTempo { bpm });
    // Return buses (reverb/delay sends); the 2 s render pad below covers tails.
    mixer.handle_event(AudioEvent::SetReturns { returns: project.returns.clone() });

    // CLAP instances, hosted inline (this thread is their main thread).
    let mut load_errors = Vec::new();
    let mut clap_handles: Vec<(String, crate::audio::clap::ClapHandle)> = Vec::new();
    for spec in crate::audio::clap::slot_specs(project) {
        match crate::audio::clap::ClapInstance::from_spec(&spec, EXPORT_SR) {
            Ok(mut inst) => {
                let p = inst.processor.take().unwrap();
                mixer.handle_event(AudioEvent::AddClapInstance {
                    uid: spec.uid.clone(),
                    processor: p,
                });
                clap_handles.push((spec.uid, inst.handle));
            }
            Err(e) => load_errors.push(format!("plugin {}: {e}", spec.uid)),
        }
    }

    // ── Sample-stamped event list ─────────────────────────────────────────────
    let mut evs: Vec<(u64, AudioEvent)> = Vec::new();

    let mut stopped = None;
    for tick in 0..max_ticks {
        if tick % 1024 == 0 { if let Err(error) = control.check() { stopped = Some(error); break; } }
        let s = (tick as f64 * spk).round() as u64;

        for (i, track) in project.tracks.iter().enumerate() {
            if track.muted { continue; }
            if track.track_type == TrackType::Audio { continue; }
            let strip = i as u8;
            let is_sampler = matches!(track.instrument, Instrument::Sampler { .. });

            let Some((pat, pat_tick)) = (if has_arr {
                arr_pat_tick(project, &track.id, tick)
            } else {
                loop_pat_tick(project, &track.id, tick)
            }) else { continue };

            let emit = |pitch: u8, vel: u8, dur_ticks: u32, evs: &mut Vec<(u64, AudioEvent)>| {
                let velocity = humanize_velocity(vel, strip, pitch, tick, project.project.humanize);
                if is_sampler {
                    evs.push((s, AudioEvent::TriggerSample { strip, pitch, gain: velocity as f32 / 127.0 }));
                } else {
                    let off = ((tick + dur_ticks) as f64 * spk).round() as u64;
                    evs.push((s, AudioEvent::NoteOn { strip, pitch, velocity }));
                    evs.push((off, AudioEvent::NoteOff { strip, pitch }));
                }
            };

            match pat {
                Pattern::Step(sp) => {
                    let step_count = sp.steps as u32;
                    let tps        = TICKS_PER_BAR / 16; // steps are 16ths
                    for step in 0..step_count {
                        let nominal = swung_step_tick(step, swing);
                        for row in &sp.rows {
                            if note_fire_tick(row.pitch, nominal, strip, project.project.timing, pattern_length_ticks(pat)) == pat_tick {
                                if let Some(vel) = row.step_velocity(step as usize) {
                                    emit(row.pitch, vel, sp.step_note_ticks(tps), &mut evs);
                                }
                            }
                        }
                    }
                }
                Pattern::PianoRoll(pr) => {
                    for note in &pr.notes {
                        if note_fire_tick(note.pitch, note.tick, strip, project.project.timing, pattern_length_ticks(pat)) == pat_tick {
                            emit(note.pitch, note.vel, piano_note_duration(pr, note), &mut evs);
                        }
                    }
                }
            }
        }
    }

    // ── Audio clips (arranged + legacy whole-file instruments) ────────────────
    let mut clip_bufs: Vec<(u32, Arc<Vec<f32>>, Arc<Vec<f32>>)> = Vec::new();
    let mut next_clip_id: u32 = 0;

    for clip in &project.audio_clips {
        let strip = project.tracks.iter().position(|t| t.id == clip.track).unwrap_or(0) as u8;
        match load_wav_stereo(Path::new(&clip.path), EXPORT_SR) {
            Ok((l, r)) => {
                let id = next_clip_id;
                next_clip_id += 1;
                clip_bufs.push((id, Arc::new(l), Arc::new(r)));
                let start = (clip.start_sec * EXPORT_SR as f64).round() as u64;
                evs.push((start, AudioEvent::TriggerAudioClip {
                    strip,
                    clip_id: id,
                    offset_sec: clip.offset_sec as f32,
                    len_sec: clip.len_sec as f32,
                    gain: clip.gain,
                }));
            }
            Err(e) => load_errors.push(format!("audio clip {}: {e}", clip.path)),
        }
    }

    // Legacy `AudioFile` track instruments: play the whole file once from t=0
    // on the track's strip. (The old exporter looped these forever — exporting
    // a song-length loop was wrong; one full pass is the intended sound.)
    for (i, track) in project.tracks.iter().enumerate() {
        if track.muted { continue; }
        if let Instrument::AudioFile { path } = &track.instrument {
            if path.is_empty() { continue; }
            match load_wav_stereo(Path::new(path), EXPORT_SR) {
                Ok((l, r)) => {
                    let id = next_clip_id;
                    next_clip_id += 1;
                    let len_sec = l.len() as f32 / EXPORT_SR as f32;
                    clip_bufs.push((id, Arc::new(l), Arc::new(r)));
                    evs.push((0, AudioEvent::TriggerAudioClip {
                        strip: i as u8,
                        clip_id: id,
                        offset_sec: 0.0,
                        len_sec,
                        gain: 1.0, // track volume applied by the strip
                    }));
                }
                Err(e) => load_errors.push(format!("audio clip {path}: {e}")),
            }
        }
    }

    if !clip_bufs.is_empty() {
        mixer.handle_event(AudioEvent::SetAudioClips { clips: clip_bufs });
    }

    evs.sort_by_key(|(s, _)| *s);

    // ── LFO modulation jobs: (strip, cc, base value 0..1, modulator) ──────────
    let mut mod_jobs: Vec<(u8, u8, f32, crate::project::Modulator)> = Vec::new();
    for (i, track) in project.tracks.iter().enumerate() {
        if track.track_type == TrackType::Audio { continue; }
        if matches!(track.instrument, Instrument::Sampler { .. }) { continue; }
        for m in &track.modulators {
            mod_jobs.push((i as u8, m.dest.cc(), mod_base01(m.dest, track), m.clone()));
        }
    }

    // ── Render loop ───────────────────────────────────────────────────────────
    let mut lb = vec![0f32; BLOCK];
    let mut rb = vec![0f32; BLOCK];
    let mut out: Vec<f32> = Vec::with_capacity(((end_sample - start_sample) * 2) as usize);
    let mut evs: Vec<(u64, Option<AudioEvent>)> = evs.into_iter().map(|(s, e)| (s, Some(e))).collect();
    let mut ei  = 0usize;
    let mut pos = 0u64;

    while pos < total_samp && stopped.is_none() && load_errors.is_empty() {
        if let Err(error) = control.check() { stopped = Some(error); break; }
        control.progress.store((pos * 100 / total_samp) as u32, std::sync::atomic::Ordering::Relaxed);
        let end    = (pos + BLOCK as u64).min(total_samp);
        let frames = (end - pos) as usize;

        // Apply LFO modulation for this block (CC sent at block start).
        if !mod_jobs.is_empty() {
            let t = pos as f32 / EXPORT_SR as f32;
            for (strip, cc, base01, m) in &mod_jobs {
                let v = (modulated_value(m, *base01, t) * 127.0).round().clamp(0.0, 127.0) as u8;
                mixer.handle_event(AudioEvent::RawCC { strip: *strip, cc: *cc, value: v });
            }
        }

        // Render up to each event before applying it. Dispatching all events at
        // block start quantizes hits early and can collapse note-on/off pairs.
        // Keep modulation on the original block grid, including range warm-up.
        let mut cursor = pos;
        while cursor < end {
            while ei < evs.len() && evs[ei].0 <= cursor {
                if let Some(event) = evs[ei].1.take() { mixer.handle_event(event); }
                ei += 1;
            }
            let next = evs.get(ei).map_or(end, |(sample, _)| (*sample).min(end));
            let a = (cursor - pos) as usize;
            let b = (next - pos) as usize;
            mixer.render_block(&mut lb[a..b], &mut rb[a..b]);
            cursor = next;
        }
        if let Some((uid, _)) = mixer.clap.iter().find(|(_, processor)| processor.process_failed) {
            stopped = Some(anyhow::anyhow!("plugin processing failed: {uid}"));
        }
        for i in 0..frames {
            let sample = pos + i as u64;
            if sample >= start_sample && sample < end_sample { out.push(lb[i]); out.push(rb[i]); }
        }
        pos = end;
    }

    // ── CLAP teardown (stop/deactivate on this, their main thread) ────────────
    for (uid, mut handle) in clap_handles {
        if let Some(p) = mixer.remove_clap(&uid) {
            handle.deactivate(p);
        }
    }

    if let Some(error) = stopped { return Err(error); }
    anyhow::ensure!(load_errors.is_empty(), "{}", load_errors.join("; "));
    control.progress.store(100, std::sync::atomic::Ordering::Relaxed);

    // ── Master peak-normalize (exports only) ─────────────────────────────────
    // The mixer's limiter already bounds the mix; scale so the loudest peak
    // sits at -1 dBFS (0.891) for a consistently loud export.
    if normalize {
        const TARGET_PEAK: f32 = 0.891;
        let peak = out.iter().fold(0f32, |m, &s| m.max(s.abs()));
        if peak > 1e-6 {
            let gain = TARGET_PEAK / peak;
            for s in &mut out {
                *s *= gain;
            }
        }
    }

    Ok((out, EXPORT_SR))
}

/// Render the full project to a stereo 32-bit float WAV at 44100 Hz, through
/// the same `Mixer` as live playback; device and scheduling behavior may differ.
pub fn export_wav(project: &Project, sf2_path: &Path, output_path: &Path) -> Result<()> {
    let (out, sr) = render_project_samples(project, sf2_path, true)?;
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    write_wav(output_path, &out, sr, 2)
}

/// Render the complete song, including shared effects and their decay, to MP3.
pub fn export_mp3(project: &Project, sf2_path: &Path, output_path: &Path) -> Result<()> {
    ExportFormat::Mp3.check_available()?;
    let control = crate::render_control::RenderControl::default();
    let (samples, sr) = render_range_samples(project, sf2_path, true, None, &control)?;
    ExportFormat::Mp3.write(output_path, &samples, sr, &control)
}

#[cfg(test)]
mod controlled_render_tests {
    use super::*;
    #[test]
    fn range_matches_full_render_and_cancellation_is_explicit() {
        let mut p = Project::default_project();
        p.tracks.clear(); p.patterns.clear(); p.arrangement.clear();
        let path = std::env::temp_dir().join(format!("daw-range-{}.wav", std::process::id()));
        let mut writer = hound::WavWriter::create(&path, hound::WavSpec { channels: 2, sample_rate: EXPORT_SR, bits_per_sample: 32, sample_format: hound::SampleFormat::Float }).unwrap();
        for i in 0..EXPORT_SR * 6 {
            let value = (i as f32 * 0.01).sin() * 0.2;
            writer.write_sample(value).unwrap(); writer.write_sample(value).unwrap();
        }
        writer.finalize().unwrap();
        let id = p.add_track(TrackType::Audio);
        p.tracks[0].effects.reverb = 0.5;
        p.tracks[0].effects.send_delay = 0.35;
        p.returns.reverb.predelay_ms=35;
        p.returns.reverb.low_cut_hz=180.; p.returns.reverb.high_cut_hz=7000.;
        p.returns.delay.sync=Some(crate::project::EchoDivision::EighthDotted);
        p.returns.delay.modulation_ms=2.5; p.returns.delay.high_cut_hz=3500.;
        p.add_audio_clip(&id, path.to_str().unwrap(), 0., 6.);
        let control = crate::render_control::RenderControl::default();
        let missing = Path::new("/nonexistent/silent-test.sf2");
        let (full, sr) = render_project_samples(&p, missing, false).unwrap();
        let (range, _) = render_range_samples(&p, missing, false, Some((1, 2)), &control).unwrap();
        let a = crate::musical_time::bar_sample(1, p.project.bpm, sr) as usize * 2;
        let b = crate::musical_time::bar_sample(2, p.project.bpm, sr) as usize * 2;
        assert!(range.iter().any(|s| s.abs() > 0.01));
        assert_eq!(range, full[a..b]);
        std::fs::remove_file(path).unwrap();
        control.cancel();
        assert!(render_range_samples(&p, missing, false, None, &control).unwrap_err().to_string().contains("cancelled"));
    }

    #[test]
    fn expanded_spatial_renders_include_long_echoes_and_bound_tails() {
        let mut p=Project::default_project();
        assert_eq!(spatial_tail_seconds(&p),2.);
        p.tracks[0].effects.send_delay=0.5;
        p.returns.delay.time_ms=12000; p.returns.delay.feedback=0.;
        assert!(spatial_tail_seconds(&p)>12.,"include the first long echo");
        p.returns.delay.feedback=0.95;
        assert_eq!(spatial_tail_seconds(&p),30.,"bound expensive tail rendering");
        p.returns.delay.muted=true;
        assert_eq!(spatial_tail_seconds(&p),2.);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{AudioClip, Project, TrackType};

    fn write_const_wav(path: &std::path::Path, frames: usize) {
        let spec = hound::WavSpec { channels: 2, sample_rate: EXPORT_SR,
            bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for _ in 0..frames { w.write_sample(0.5f32).unwrap(); w.write_sample(0.5f32).unwrap(); }
        w.finalize().unwrap();
    }

    #[test]
    fn sampler_hits_keep_their_sample_positions_inside_a_block() {
        let path = std::env::temp_dir().join(format!("daw-timing-{}.wav", std::process::id()));
        let mut writer = hound::WavWriter::create(&path, hound::WavSpec {
            channels: 1, sample_rate: EXPORT_SR, bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        }).unwrap();
        for i in 0..512 { writer.write_sample(if i == 0 { 0.5f32 } else { 0.0 }).unwrap(); }
        writer.finalize().unwrap();
        let mut p = Project::default_project();
        p.tracks.clear(); p.patterns.clear(); p.arrangement.clear();
        p.project.bpm = 120.; p.project.humanize = 0.; p.project.timing = 0.;
        p.add_track(TrackType::Drum);
        p.tracks[0].instrument = serde_json::from_value(serde_json::json!({
            "type": "sampler", "pads": [{"pitch": 36, "path": path, "gain": 1.0}]
        })).unwrap();
        p.tracks[0].effects = Default::default();
        p.tracks[0].patterns = vec!["pulse".into()];
        p.patterns.push(serde_json::from_value(serde_json::json!({
            "type": "piano_roll", "id": "pulse", "name": "Pulse", "bars": 1, "ticks_per_bar": 480,
            "notes": [
                {"pitch": 36, "tick": 1, "duration": 1, "vel": 100},
                {"pitch": 36, "tick": 2, "duration": 1, "vel": 100},
                {"pitch": 36, "tick": 7, "duration": 1, "vel": 100}
            ]
        })).unwrap());
        let (audio, _) = render_project_samples(&p, Path::new("/missing.sf2"), false).unwrap();
        let onsets: Vec<_> = audio.chunks_exact(2).enumerate()
            .filter(|(_, frame)| frame[0].abs() > 0.00001)
            .map(|(index, _)| index).collect();
        assert_eq!(onsets, vec![184, 368, 1286], "hits must not snap to 1024-frame boundaries");
        p.project.timing = 0.7;
        if let Pattern::PianoRoll(pattern) = p.patterns.last_mut().unwrap() {
            for (note, tick) in pattern.notes.iter_mut().zip([100, 200, 300]) { note.tick = tick; }
        }
        let expected: Vec<_> = [100, 200, 300].into_iter().map(|tick| {
            let fire = note_fire_tick(36, tick, 0, 0.7, 480);
            (fire as f64 * 44100.0 / 240.0).round() as usize
        }).collect();
        assert_ne!(expected, vec![18375, 36750, 55125], "fixture must exercise timing offsets");
        let (audio, _) = render_project_samples(&p, Path::new("/missing.sf2"), false).unwrap();
        let onsets: Vec<_> = audio.chunks_exact(2).enumerate()
            .filter(|(_, frame)| frame[0].abs() > 0.00001).map(|(index, _)| index).collect();
        assert_eq!(onsets, expected, "export must use live timing humanization");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn export_includes_audio_clip_energy() {
        let sf2 = std::path::Path::new("../../assets/GeneralUser.sf2");
        if !sf2.exists() { assert!(std::env::var_os("DAW_REQUIRE_TEST_ASSETS").is_none(), "required audio test assets missing"); eprintln!("skip: no soundfont"); return; }

        let dir = std::env::temp_dir();
        let src = dir.join("daw_export_clip_src.wav");
        let out = dir.join("daw_export_clip_out.wav");
        write_const_wav(&src, EXPORT_SR as usize); // 1.0s of 0.5

        // Empty project — no tracks, no patterns, no arrangement — only the clip.
        // This guarantees any energy in the output comes from the clip itself.
        let mut p = Project::default_project();
        p.tracks.clear();
        p.patterns.clear();
        p.arrangement.clear();
        let tid = p.add_track(TrackType::Audio);
        p.audio_clips.push(AudioClip {
            track: tid, path: src.to_string_lossy().into(),
            start_sec: 1.0, offset_sec: 0.0, len_sec: 1.0, gain: 1.0,
        });

        export_wav(&p, sf2, &out).unwrap();

        let mut rd = hound::WavReader::open(&out).unwrap();
        let ch = rd.spec().channels as usize;
        let samples: Vec<f32> = rd.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect();
        let at = (EXPORT_SR as usize) * ch + 100 * ch; // just after 1.0s
        let energy: f32 = samples[at..at + 200].iter().map(|s| s.abs()).sum();
        eprintln!("clip energy at 1s = {energy}");
        assert!(energy > 1.0, "expected clip energy after 1s, got {energy}");

        std::fs::remove_file(&src).ok();
        std::fs::remove_file(&out).ok();
    }

    #[test]
    fn export_honors_resized_clips() {
        let sf2 = std::path::Path::new("../../assets/GeneralUser.sf2");
        if !sf2.exists() { assert!(std::env::var_os("DAW_REQUIRE_TEST_ASSETS").is_none(), "required audio test assets missing"); eprintln!("skip: no soundfont"); return; }
        let out = std::env::temp_dir().join("daw_export_resized.wav");
        let mut p = Project::default_project();
        // Stretch the drum clip to 4 bars (1-bar pattern loops to fill).
        assert!(p.resize_clip("t1", 0, 4));
        export_wav(&p, sf2, &out).unwrap();
        let dur = crate::audio::wav_duration_sec(&out).unwrap();
        // 4 bars @ 120bpm = 8s + 2s pad.
        assert!(dur >= 9.5, "resized clip must extend the export, got {dur}s");
        // Bar 4 (samples 6-8s) must contain drum energy (pattern looped).
        let mut rd = hound::WavReader::open(&out).unwrap();
        let sr = rd.spec().sample_rate as usize;
        let samples: Vec<f32> = rd.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect();
        let ch = 2;
        let bar4 = &samples[6 * sr * ch..(8 * sr * ch).min(samples.len())];
        let energy: f32 = bar4.iter().map(|s| s.abs()).sum();
        assert!(energy > 1.0, "looped pattern must sound in bar 4, got {energy}");
        std::fs::remove_file(&out).ok();
    }

    #[test]
    fn export_legato_holds_notes_until_the_next_phrase_entry() {
        let plugin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/libdaw_clap_testplug_tone.so");
        assert!(plugin.exists(), "build workspace CLAP fixtures before running export regressions");
        let mut project = Project::default_project();
        project.tracks.clear(); project.patterns.clear(); project.arrangement.clear();
        project.project.bpm = 120.; project.project.timing = 0.;
        let track = project.add_track(TrackType::Melodic);
        project.set_track_clap_instrument(&track, plugin.to_str().unwrap(), "test.tone").unwrap();
        project.tracks[0].effects = Default::default();
        project.tracks[0].patterns = vec!["legato".into()];
        project.patterns.push(serde_json::from_value(serde_json::json!({
            "type":"piano_roll", "id":"legato", "name":"Legato", "bars":1, "ticks_per_bar":480, "legato":true,
            "notes":[{"pitch":60,"tick":0,"duration":10,"vel":100},
                     {"pitch":64,"tick":120,"duration":10,"vel":100}]
        })).unwrap());
        let (audio, _) = render_project_samples(&project, Path::new("/missing.sf2"), false).unwrap();
        assert!(audio[8000 * 2].abs() > 0.01, "legato must outlast the authored short duration");
        assert_eq!(audio[24000 * 2], 0., "last note keeps its authored release");
    }

    #[test]
    fn export_renders_clap_fx() {
        let sf2 = std::path::Path::new("../../assets/GeneralUser.sf2");
        let plug = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug/libdaw_clap_testplug_gain.so");
        if !sf2.exists() || !plug.exists() {
            assert!(std::env::var_os("DAW_REQUIRE_TEST_ASSETS").is_none(), "required audio test assets missing"); eprintln!("skip: assets missing");
            return;
        }
        let out = std::env::temp_dir().join("daw_export_clap_gain0.wav");
        let mut p = Project::default_project();
        let t0 = p.tracks[0].id.clone();
        let uid = p.add_clap_fx(&t0, plug.to_str().unwrap(), "test.gain").unwrap();
        // Gain 0.0 on track 0's chain must zero its contribution; mute the
        // rest so the whole export is silent.
        p.set_clap_param(&uid, 1, 0.0);
        for t in &mut p.tracks[1..] {
            t.muted = true;
        }
        export_wav(&p, sf2, &out).unwrap();
        let mut rd = hound::WavReader::open(&out).unwrap();
        let energy: f32 = rd.samples::<f32>().map(|s| s.unwrap_or(0.0).abs()).sum();
        assert!(energy < 0.01, "clap gain 0 must silence the export, got {energy}");
        std::fs::remove_file(&out).ok();
    }

    #[test]
    fn export_send_reverb_leaves_tail_energy() {
        let sf2 = std::path::Path::new("../../assets/GeneralUser.sf2");
        if !sf2.exists() { assert!(std::env::var_os("DAW_REQUIRE_TEST_ASSETS").is_none(), "required audio test assets missing"); eprintln!("skip: no soundfont"); return; }
        let out = std::env::temp_dir().join("daw_export_reverb_tail.wav");
        let mut p = Project::default_project();
        for t in &mut p.tracks { t.effects.reverb = 1.0; }
        export_wav(&p, sf2, &out).unwrap();
        let mut rd = hound::WavReader::open(&out).unwrap();
        let samples: Vec<f32> = rd.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect();
        // The last second of the 2 s pad should ring with reverb tail.
        let tail = &samples[samples.len().saturating_sub(EXPORT_SR as usize)..];
        let energy: f32 = tail.iter().map(|s| s.abs()).sum();
        assert!(energy > 0.05, "no reverb tail in export pad, energy {energy}");
        std::fs::remove_file(&out).ok();
    }

    #[test]
    fn export_muted_track_is_silent() {
        let sf2 = std::path::Path::new("../../assets/GeneralUser.sf2");
        if !sf2.exists() { assert!(std::env::var_os("DAW_REQUIRE_TEST_ASSETS").is_none(), "required audio test assets missing"); eprintln!("skip: no soundfont"); return; }
        let out = std::env::temp_dir().join("daw_export_muted_out.wav");
        let mut p = Project::default_project();
        for t in &mut p.tracks { t.muted = true; }
        export_wav(&p, sf2, &out).unwrap();
        let mut rd = hound::WavReader::open(&out).unwrap();
        let energy: f32 = rd.samples::<f32>().map(|s| s.unwrap_or(0.0).abs()).sum();
        assert!(energy < 0.01, "muted project must export silence, got {energy}");
        std::fs::remove_file(&out).ok();
    }
}

#[cfg(test)]
mod sampler_variation_render_tests {
    use super::*;
    #[test]
    fn alternate_takes_survive_export_range_warmup_and_missing_assets_fail() {
        let dir = std::env::temp_dir().join(format!("daw-take-render-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut paths = Vec::new();
        for (i, amplitude) in [0.1f32, 0.2, -0.3, 0.4, -0.5].into_iter().enumerate() {
            let path=dir.join(format!("take-{i}.wav"));
            let mut w=hound::WavWriter::create(&path,hound::WavSpec { channels:1,sample_rate:44100,bits_per_sample:32,sample_format:hound::SampleFormat::Float }).unwrap();
            for k in 0..512 { w.write_sample(if k==0 { amplitude } else { 0. }).unwrap(); }
            w.finalize().unwrap();paths.push(path);
        }
        let mut p=crate::project::Project::default_project();
        p.tracks.clear();p.patterns.clear();p.arrangement.clear();
        p.project.bpm=120.;p.project.humanize=0.;p.project.swing=0.;p.project.timing=0.;
        p.add_track(crate::project::TrackType::Drum);
        p.tracks[0].volume=1.;p.tracks[0].pan=0.;p.tracks[0].effects=Default::default();
        p.tracks[0].instrument=serde_json::from_value(serde_json::json!({"type":"sampler","pads":[{
            "pitch":36,"path":paths[0],"variants":[
                {"path":paths[1],"min_velocity":1,"max_velocity":63},
                {"path":paths[2],"min_velocity":1,"max_velocity":63},
                {"path":paths[3],"min_velocity":64,"max_velocity":127},
                {"path":paths[4],"min_velocity":64,"max_velocity":127}]}]})).unwrap();
        p.tracks[0].patterns=vec!["groove".into()];p.patterns.clear();
        p.patterns.push(serde_json::from_value(serde_json::json!({"type":"piano_roll","id":"groove","name":"Groove","bars":2,"ticks_per_bar":480,"notes":[
            {"pitch":36,"tick":0,"duration":1,"vel":40},
            {"pitch":36,"tick":90,"duration":1,"vel":40},
            {"pitch":36,"tick":240,"duration":1,"vel":40},
            {"pitch":36,"tick":360,"duration":1,"vel":100},
            {"pitch":36,"tick":480,"duration":1,"vel":40},
            {"pitch":36,"tick":600,"duration":1,"vel":100}]})).unwrap());
        p.validate().unwrap();
        let (full, sr)=render_project_samples(&p,Path::new("/missing.sf2"),false).unwrap();
        for (tick, expected, velocity) in [(0,0.2,40),(90,-0.3,40),(240,0.2,40),(360,0.4,100),(480,-0.3,40),(600,-0.5,100)] {
            let sample=(tick as f64 * sr as f64 / 240.).round() as usize;
            assert!((full[sample*2]-expected*velocity as f32/127.).abs()<1e-6,"tick {tick}: {}",full[sample*2]);
        }
        let (range,_)=render_range_samples(&p,Path::new("/missing.sf2"),false,Some((1,2)),&Default::default()).unwrap();
        let start=crate::musical_time::bar_sample(1,120.,sr) as usize*2;
        assert_eq!(range,&full[start..start+range.len()]);
        std::fs::remove_file(&paths[2]).unwrap();
        assert!(render_project_samples(&p,Path::new("/missing.sf2"),false).unwrap_err().to_string().contains("sampler variant"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

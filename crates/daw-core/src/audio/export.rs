use super::engine::write_wav;
use super::load_wav_stereo;
use super::mixer::{build_strips, AudioEvent, Mixer, StripMeter, StripSpec};
use crate::project::{Instrument, Pattern, Project, TrackType};
use crate::sequencer::engine::{humanize_velocity, mod_base01, modulated_value, pattern_length_ticks, swung_step_tick, TICKS_PER_BAR, TICKS_PER_BEAT};
use anyhow::Result;
use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};
use std::path::Path;
use std::sync::Arc;

const EXPORT_SR: u32 = 44100;
const BLOCK: usize = 1024;

/// Total ticks of the full arrangement (max end-tick across all slots).
fn arrangement_total_ticks(project: &Project) -> u32 {
    project.arrangement.iter()
        .filter_map(|sl| {
            let pat = project.patterns.iter().find(|p| p.id() == sl.pattern)?;
            // Resized clips (len_bars) extend the song; the pattern loops to fill.
            let len = sl.len_bars
                .map(|b| b * TICKS_PER_BAR)
                .unwrap_or_else(|| pattern_length_ticks(pat));
            Some(sl.bar * TICKS_PER_BAR + len)
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
        let start = sl.bar * TICKS_PER_BAR;
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
/// the same `Mixer` the live engine uses (export = playback by construction).
/// Respects the arrangement timeline; falls back to single-loop if empty.
/// Render the whole project to interleaved stereo f32 at 44.1 kHz.
/// `normalize` applies the -1 dBFS peak normalization (exports do; the
/// auto-balance measurement must not).
pub fn render_project_samples(project: &Project, sf2_path: &Path, normalize: bool) -> Result<(Vec<f32>, u32)> {
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
        + 2 * EXPORT_SR as u64;

    // ── The mixer: same strips as live playback ───────────────────────────────
    let specs = StripSpec::from_project(project);
    let (strips, _meters) = build_strips(&specs, sf2_path, EXPORT_SR);
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
    // Return buses (reverb/delay sends); the 2 s render pad below covers tails.
    mixer.handle_event(AudioEvent::SetReturns { returns: project.returns.clone() });

    // CLAP instances, hosted inline (this thread is their main thread).
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
            Err(e) => eprintln!("export: clap {} skipped: {e}", spec.uid),
        }
    }

    // ── Sample-stamped event list ─────────────────────────────────────────────
    let mut evs: Vec<(u64, AudioEvent)> = Vec::new();

    for tick in 0..max_ticks {
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

            let mut emit = |pitch: u8, vel: u8, dur_ticks: u32, evs: &mut Vec<(u64, AudioEvent)>| {
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
                        if swung_step_tick(step, swing) == pat_tick {
                            for row in &sp.rows {
                                if let Some(vel) = row.step_velocity(step as usize) {
                                    emit(row.pitch, vel, sp.step_note_ticks(tps), &mut evs);
                                }
                            }
                        }
                    }
                }
                Pattern::PianoRoll(pr) => {
                    for note in &pr.notes {
                        if note.tick == pat_tick {
                            emit(note.pitch, note.vel, note.duration, &mut evs);
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
            Err(e) => eprintln!("export audio clip load error {}: {e}", clip.path),
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
                Err(e) => eprintln!("export: skip clip {path}: {e}"),
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
    let mut out: Vec<f32> = Vec::with_capacity((total_samp * 2) as usize);
    let mut evs: Vec<(u64, Option<AudioEvent>)> = evs.into_iter().map(|(s, e)| (s, Some(e))).collect();
    let mut ei  = 0usize;
    let mut pos = 0u64;

    while pos < total_samp {
        let end    = (pos + BLOCK as u64).min(total_samp);
        let frames = (end - pos) as usize;

        while ei < evs.len() && evs[ei].0 < end {
            if let Some(ev) = evs[ei].1.take() {
                mixer.handle_event(ev);
            }
            ei += 1;
        }

        // Apply LFO modulation for this block (CC sent at block start).
        if !mod_jobs.is_empty() {
            let t = pos as f32 / EXPORT_SR as f32;
            for (strip, cc, base01, m) in &mod_jobs {
                let v = (modulated_value(m, *base01, t) * 127.0).round().clamp(0.0, 127.0) as u8;
                mixer.handle_event(AudioEvent::RawCC { strip: *strip, cc: *cc, value: v });
            }
        }

        mixer.render_block(&mut lb[..frames], &mut rb[..frames]);
        for i in 0..frames {
            out.push(lb[i]);
            out.push(rb[i]);
        }
        pos = end;
    }

    // ── CLAP teardown (stop/deactivate on this, their main thread) ────────────
    for (uid, mut handle) in clap_handles {
        if let Some(p) = mixer.remove_clap(&uid) {
            handle.deactivate(p);
        }
    }

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
/// the same `Mixer` the live engine uses (export = playback by construction).
pub fn export_wav(project: &Project, sf2_path: &Path, output_path: &Path) -> Result<()> {
    let (out, sr) = render_project_samples(project, sf2_path, true)?;
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    write_wav(output_path, &out, sr, 2)
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
    fn export_includes_audio_clip_energy() {
        let sf2 = std::path::Path::new("../../assets/GeneralUser.sf2");
        if !sf2.exists() { eprintln!("skip: no soundfont"); return; }

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
        if !sf2.exists() { eprintln!("skip: no soundfont"); return; }
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
    fn export_renders_clap_fx() {
        let sf2 = std::path::Path::new("../../assets/GeneralUser.sf2");
        let plug = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug/libdaw_clap_testplug_gain.so");
        if !sf2.exists() || !plug.exists() {
            eprintln!("skip: assets missing");
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
        if !sf2.exists() { eprintln!("skip: no soundfont"); return; }
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
        if !sf2.exists() { eprintln!("skip: no soundfont"); return; }
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

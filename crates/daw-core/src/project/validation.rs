//! Semantic validation shared by file, session and tool entry points.
use super::*;
use anyhow::{ensure, Result};
use std::collections::HashSet;

fn range(name: &str, value: f64, min: f64, max: f64) -> Result<()> {
    ensure!(
        value.is_finite() && value >= min && value <= max,
        "{name}: expected {min}..={max}, got {value}"
    );
    Ok(())
}

impl Project {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == "0.1",
            "unsupported project version: {}",
            self.version
        );
        ensure!(
            crate::musical_time::is_valid_time_signature(self.project.time_signature),
            "unsupported time signature {}/{} (numerator 1..=16, denominator 2|4|8|16)",
            self.project.time_signature[0],
            self.project.time_signature[1]
        );
        let tpb = self.ticks_per_bar();
        let spb_steps = self.steps_per_bar();
        range("bpm", self.project.bpm, crate::musical_time::MIN_BPM, crate::musical_time::MAX_BPM)?;
        ensure!(self.tempo_changes.len() <= MAX_TEMPO_POINTS, "too many tempo change points");
        ensure!(
            self.tempo_changes.windows(2).all(|w| w[0].tick < w[1].tick),
            "tempo changes must have unique, ascending ticks"
        );
        for pt in &self.tempo_changes {
            range("tempo change bpm", pt.bpm, crate::musical_time::MIN_BPM, crate::musical_time::MAX_BPM)?;
        }
        for (n, v) in [
            ("swing", self.project.swing),
            ("humanize", self.project.humanize),
            ("timing", self.project.timing),
        ] {
            range(n, v as f64, 0., 1.)?;
        }
        ensure!(
            self.tracks.len() < crate::audio::MAX_STRIPS,
            "too many tracks for the mixer"
        );
        let mut tracks = HashSet::new();
        let mut patterns = HashSet::new();
        let mut plugins = HashSet::new();
        for p in &self.patterns {
            ensure!(
                !p.id().trim().is_empty() && patterns.insert(p.id()),
                "empty or duplicate pattern id: {}",
                p.id()
            );
            match p {
                Pattern::Step(s) => {
                    ensure!(s.steps > 0, "pattern {} has zero steps", s.id);
                    range("step gate", s.gate as f64, 0.01, 16.)?;
                    if let Some(sw) = s.swing {
                        range("step swing", f64::from(sw), 0., 1.)?;
                    }
                    for r in &s.rows {
                        ensure!(
                            r.pitch <= 127 && r.vel <= 127 && r.steps.iter().all(|v| *v <= 127),
                            "invalid MIDI value in {}",
                            s.id
                        );
                        ensure!(
                            r.steps.len() == s.steps as usize,
                            "step row length does not match {}",
                            s.id
                        );
                    }
                }
                Pattern::PianoRoll(p) => {
                    ensure!(
                        p.bars > 0 && p.ticks_per_bar == tpb,
                        "{} needs positive bars and {tpb} ticks per bar",
                        p.id
                    );
                    if let Some(sw) = p.swing {
                        range("pattern swing", f64::from(sw), 0., 1.)?;
                    }
                    ensure!(
                        p.cc_events.len() <= MAX_CC_EVENTS,
                        "{} has too many CC events",
                        p.id
                    );
                    ensure!(
                        p.pitch_bends.len() <= MAX_PITCH_BEND_EVENTS,
                        "{} has too many pitch bend events",
                        p.id
                    );
                    ensure!(
                        p.legato_overlap_ticks <= 240,
                        "{} legato overlap {} exceeds 240 ticks",
                        p.id,
                        p.legato_overlap_ticks
                    );
                    // A shortened piano roll retains its notes so extending it
                    // again restores them. Their onsets must fit a valid bar count.
                    let max_length = u32::from(u8::MAX) * p.ticks_per_bar;
                    for c in &p.cc_events {
                        ensure!(
                            matches!(c.cc, 1 | 11 | 64 | 74)
                                && c.value <= 127
                                && c.tick < max_length,
                            "invalid CC event in {}",
                            p.id
                        );
                    }
                    for pb in &p.pitch_bends {
                        ensure!(
                            pb.cents.is_finite()
                                && (-2400.0..=2400.0).contains(&pb.cents)
                                && pb.tick < max_length,
                            "invalid pitch bend event in {}",
                            p.id
                        );
                    }
                    for n in &p.notes {
                        ensure!(
                            n.pitch <= 127
                                && n.vel <= 127
                                && n.tick < max_length
                                && n.duration > 0
                                && n.tick.checked_add(n.duration).is_some(),
                            "invalid note in {}",
                            p.id
                        );
                    }
                }
            }
        }
        for t in &self.tracks {
            ensure!(
                !t.id.trim().is_empty() && tracks.insert(t.id.as_str()),
                "empty or duplicate track id: {}",
                t.id
            );
            ensure!(
                t.patterns.iter().all(|p| patterns.contains(p.as_str())),
                "track {} references a missing pattern",
                t.id
            );
            range("volume", t.volume as f64, 0., 2.)?;
            ensure!(t.comments.len() <= MAX_TRACK_COMMENTS, "{} has too many comments", t.id);
            let mut commented_bars = HashSet::new();
            for comment in &t.comments {
                comment.validate()?;
                ensure!(
                    comment.bar.is_none_or(|bar| bar < u32::MAX / tpb),
                    "comment bar is too large"
                );
                ensure!(commented_bars.insert(comment.bar), "duplicate comment location on {}", t.id);
            }
            ensure!(t.volume_automation.len() <= MAX_VOLUME_POINTS, "{} has too many volume points", t.id);
            ensure!(
                t.volume_automation.windows(2).all(|pair| pair[0].tick < pair[1].tick),
                "{} volume points must have unique, ascending ticks",
                t.id
            );
            for point in &t.volume_automation {
                range("volume automation gain", f64::from(point.gain), 0., 2.)?;
            }
            range("pan", t.pan as f64, -1., 1.)?;
            range("tuning cents", t.tuning_cents as f64, -200., 200.)?;
            ensure!(
                (1..=24).contains(&t.pitch_bend_range_semitones),
                "{} pitch_bend_range_semitones {} outside 1..=24",
                t.id,
                t.pitch_bend_range_semitones
            );
            let fx = &t.effects;
            for v in [
                fx.reverb,
                fx.chorus,
                fx.cutoff,
                fx.resonance,
                fx.lfo_depth,
                fx.send_delay,
                fx.delay_mix,
                fx.drive,
            ] {
                range("effect amount", v as f64, 0., 1.)?;
            }
            range("delay feedback", fx.delay_feedback as f64, 0., 0.95)?;
            range("delay time", fx.delay_time_ms as f64, 0., 2000.)?;
            for v in [fx.eq_low_gain, fx.eq_mid_gain, fx.eq_high_gain] {
                range("EQ gain", v as f64, -12., 12.)?;
            }
            range("EQ frequency", fx.eq_mid_freq as f64, 200., 8000.)?;
            range("compressor threshold", fx.comp_threshold as f64, -60., 0.)?;
            range("compressor ratio", fx.comp_ratio as f64, 1., 20.)?;
            range("compressor attack", fx.comp_attack_ms as f64, 0.5, 100.)?;
            range("compressor release", fx.comp_release_ms as f64, 10., 1000.)?;
            for m in &t.modulators {
                range("LFO rate", m.rate_hz as f64, 0., 100.)?;
                range("LFO depth", m.depth as f64, 0., 1.)?;
                range("LFO phase", m.phase as f64, 0., 1.)?;
            }
            if let Instrument::Soundfont { preset, .. } = &t.instrument {
                ensure!(*preset <= 127, "invalid soundfont preset");
            }
            if let Instrument::Sfz { path } = &t.instrument {
                ensure!(!path.trim().is_empty(), "{} sfz path cannot be empty", t.id);
            }
            if let Instrument::Sampler { pads } = &t.instrument {
                let mut pitches = HashSet::new();
                for p in pads {
                    ensure!(
                        p.pitch <= 127 && pitches.insert(p.pitch),
                        "invalid or duplicate sampler pitch"
                    );
                    range("pad gain", p.gain as f64, 0., 2.)?;
                    p.validate_variants()?;
                }
            }
            let instrument = match &t.instrument {
                Instrument::Clap(c) => Some(c),
                _ => None,
            };
            for slot in instrument.into_iter().chain(t.clap_fx.iter()) {
                ensure!(
                    !slot.uid.is_empty() && plugins.insert(slot.uid.as_str()),
                    "duplicate or empty plugin uid"
                );
                ensure!(
                    !slot.id.is_empty() && !slot.path.is_empty(),
                    "plugin needs id and path"
                );
                ensure!(
                    slot.params
                        .iter()
                        .all(|(k, v)| k.parse::<u32>().is_ok() && v.is_finite()),
                    "invalid plugin parameter"
                );
            }
        }
        for s in &self.arrangement {
            ensure!(
                tracks.contains(s.track.as_str()) && patterns.contains(s.pattern.as_str()),
                "arrangement references missing track/pattern"
            );
            range("clip gain", f64::from(s.gain), 0., 2.)?;
            let span = s.len_bars.unwrap_or_else(|| {
                self.patterns
                    .iter()
                    .find(|p| p.id() == s.pattern)
                    .unwrap()
                    .bars_in(spb_steps)
            });
            ensure!(
                span > 0
                    && s.offset_ticks < tpb
                    && s.bar
                        .checked_add(span)
                        .and_then(|n| n.checked_mul(tpb))
                        .and_then(|n| n.checked_add(s.offset_ticks))
                        .is_some(),
                "invalid clip length or position"
            );
        }
        for c in &self.audio_clips {
            ensure!(
                self.tracks
                    .iter()
                    .any(|t| t.id == c.track && t.track_type == TrackType::Audio),
                "audio clip references missing/non-audio track"
            );
            range("audio start", c.start_sec, 0., 86400.)?;
            range("audio offset", c.offset_sec, 0., 86400.)?;
            range("audio length", c.len_sec, MIN_CLIP_SEC, 86400.)?;
            range("audio gain", c.gain as f64, 0., 2.)?;
        }
        let r = &self.returns;
        for v in [r.reverb.gain, r.delay.gain] {
            range("return gain", v as f64, 0., 1.25)?;
        }
        for v in [r.reverb.room_size, r.reverb.damping, r.reverb.width] {
            range("reverb", v as f64, 0., 1.)?;
        }
        range("return delay feedback", r.delay.feedback as f64, 0., 0.95)?;
        range("return delay time", r.delay.time_ms as f64, 1., 12000.)?;
        range("reverb pre-delay", r.reverb.predelay_ms as f64, 0., 250.)?;
        for (low, high) in [(r.reverb.low_cut_hz, r.reverb.high_cut_hz), (r.delay.low_cut_hz, r.delay.high_cut_hz)] {
            range("return low cut", low as f64, 0., 2000.)?;
            range("return high cut", high as f64, 500., 20000.)?;
            ensure!(low < high, "return low cut must be below high cut");
        }
        range("echo width", r.delay.width as f64, 0., 1.)?;
        range("echo modulation depth", r.delay.modulation_ms as f64, 0., 20.)?;
        range("echo modulation rate", r.delay.modulation_hz as f64, 0.05, 8.)?;
        ensure!(
            self.loop_region.start_bar < self.loop_region.end_bar
                && self.loop_region.end_bar.checked_mul(tpb).is_some(),
            "invalid loop bounds"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_semantic_errors() {
        let p = Project::default_project();
        p.validate().unwrap();
        let mut bad = p.clone();
        bad.tracks.push(bad.tracks[0].clone());
        assert!(bad.validate().is_err());
        let mut bad = p.clone();
        bad.project.bpm = f64::NAN;
        assert!(bad.validate().is_err());
        let mut bad = p.clone();
        bad.project.time_signature = [0, 4];
        assert!(bad.validate().is_err());
        let mut bad = p.clone();
        bad.project.time_signature = [4, 3];
        assert!(bad.validate().is_err());
        let mut bad = p;
        bad.tracks[0].patterns.push("missing".into());
        assert!(bad.validate().is_err());
    }

    #[test]
    fn accepts_odd_meters_tempo_changes_and_pitch_bends() {
        for (num, den) in [(3, 4), (5, 4), (7, 4), (6, 8)] {
            let mut p = Project::default_project();
            p.set_time_signature(num, den).unwrap();
            p.validate().unwrap();
        }
        let mut p = Project::default_project();
        p.set_time_signature(7, 4).unwrap();
        p.set_pattern_pitch_bends("p2", vec![
            PitchBendPoint { tick: 0, cents: -80.0 },
            PitchBendPoint { tick: 60, cents: 0.0 },
        ]).unwrap();
        p.set_tempo_changes(vec![
            TempoPoint { tick: 840, bpm: 204.0, ramp: false },
            TempoPoint { tick: 1680, bpm: 228.0, ramp: true },
        ]).unwrap();
        p.validate().unwrap();
        assert!(p.set_pattern_pitch_bends("p2", vec![PitchBendPoint { tick: 0, cents: 2500.0 }]).is_err());
        assert!(p.set_tempo_changes(vec![TempoPoint { tick: 0, bpm: 401.0, ramp: false }]).is_err());
    }

    #[test]
    fn accepts_uptempo_pattern_swing_and_wide_bend_range() {
        let mut p = Project::default_project();
        p.project.bpm = 360.0;
        p.set_tempo_changes(vec![TempoPoint { tick: 480, bpm: 400.0, ramp: true }]).unwrap();
        p.set_pattern_swing("p1", Some(0.0)).unwrap();
        p.set_pattern_swing("p2", Some(0.6)).unwrap();
        p.set_track_pitch_bend_range("t2", 12).unwrap();
        p.set_pattern_pitch_bends("p2", vec![
            PitchBendPoint { tick: 0, cents: -1200.0 },
            PitchBendPoint { tick: 120, cents: 700.0 },
        ]).unwrap();
        p.validate().unwrap();
        assert!(p.set_pattern_swing("p2", Some(1.2)).is_err());
        assert!(p.set_track_pitch_bend_range("t2", 0).is_err());
        assert!(p.set_track_pitch_bend_range("t2", 25).is_err());
    }
}

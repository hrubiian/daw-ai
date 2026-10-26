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
            self.project.time_signature == [4, 4],
            "only 4/4 is currently supported"
        );
        range("bpm", self.project.bpm, 20.0, 300.0)?;
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
                        p.bars > 0 && p.ticks_per_bar == 480,
                        "{} needs positive bars and 480 ticks per bar",
                        p.id
                    );
                    let length = p.bars as u32 * p.ticks_per_bar;
                    for n in &p.notes {
                        ensure!(
                            n.pitch <= 127
                                && n.vel <= 127
                                && n.tick < length
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
            range("pan", t.pan as f64, -1., 1.)?;
            range("tuning cents", t.tuning_cents as f64, -200., 200.)?;
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
            let span = s.len_bars.unwrap_or_else(|| {
                self.patterns
                    .iter()
                    .find(|p| p.id() == s.pattern)
                    .unwrap()
                    .bars()
            });
            ensure!(
                span > 0
                    && s.bar
                        .checked_add(span)
                        .and_then(|n| n.checked_mul(480))
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
                && self.loop_region.end_bar.checked_mul(480).is_some(),
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
        bad.project.time_signature = [3, 4];
        assert!(bad.validate().is_err());
        let mut bad = p;
        bad.tracks[0].patterns.push("missing".into());
        assert!(bad.validate().is_err());
    }
}

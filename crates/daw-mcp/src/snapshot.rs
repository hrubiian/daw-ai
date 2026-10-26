//! Music CI, slice A: snapshot regression. Capture an analyzed fingerprint of a
//! render as a baseline, then flag when a later edit regresses the mix.

use daw_core::project::{Pattern, Project};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Compact, comparable summary of a rendered project.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Fingerprint {
    pub peak_db: f32,
    pub rms_db: f32,
    pub clipped_samples: usize,
    pub stereo_correlation: f32,
    /// 7-band spectral balance (name, dB).
    pub bands: Vec<(String, f32)>,
    pub tracks: usize,
    pub notes: usize,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Level {
    Info,
    Warn,
    Fail,
}

pub struct Regression {
    pub level: Level,
    pub msg: String,
}

/// Total piano-roll notes across a project.
fn note_count(p: &Project) -> usize {
    p.patterns
        .iter()
        .map(|pat| if let Pattern::PianoRoll(pr) = pat { pr.notes.len() } else { 0 })
        .sum()
}

/// Fingerprint from a project + its rendered analysis.
pub fn fingerprint(project: &Project, r: &crate::analysis::AnalysisReport) -> Fingerprint {
    Fingerprint {
        peak_db: r.peak_db,
        rms_db: r.rms_db,
        clipped_samples: r.clipped_samples,
        stereo_correlation: r.stereo_correlation,
        bands: r.bands.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        tracks: project.tracks.len(),
        notes: note_count(project),
    }
}

/// Render the project and fingerprint the result.
pub fn render_fingerprint(project: &Project, sf2: &Path) -> anyhow::Result<Fingerprint> {
    let (samples, sr) = daw_core::audio::export::render_project_samples(project, sf2, false)?;
    let l: Vec<f32> = samples.iter().step_by(2).copied().collect();
    let r: Vec<f32> = samples.iter().skip(1).step_by(2).copied().collect();
    let rep = crate::analysis::analyze(&l, &r, sr, project.project.bpm);
    Ok(fingerprint(project, &rep))
}

/// Regressions from a baseline `old` fingerprint to a fresh `new` one.
/// Empty == PASS.
pub fn regressions(old: &Fingerprint, new: &Fingerprint) -> Vec<Regression> {
    let mut v = Vec::new();
    if old.clipped_samples == 0 && new.clipped_samples > 0 {
        v.push(Regression {
            level: Level::Fail,
            msg: format!("clipping introduced: {} samples", new.clipped_samples),
        });
    }
    if new.peak_db - old.peak_db > 3.0 {
        v.push(Regression {
            level: Level::Warn,
            msg: format!(
                "peak up {:.1} dB ({:.1} -> {:.1} dBFS)",
                new.peak_db - old.peak_db,
                old.peak_db,
                new.peak_db
            ),
        });
    }
    if (new.rms_db - old.rms_db).abs() > 3.0 {
        v.push(Regression {
            level: Level::Info,
            msg: format!(
                "loudness {:+.1} dB ({:.1} -> {:.1} dBFS)",
                new.rms_db - old.rms_db,
                old.rms_db,
                new.rms_db
            ),
        });
    }
    for (o, n) in old.bands.iter().zip(&new.bands) {
        if (n.1 - o.1).abs() > 6.0 {
            v.push(Regression {
                level: Level::Warn,
                msg: format!("{} band shifted {:+.1} dB ({:.0} -> {:.0})", o.0, n.1 - o.1, o.1, n.1),
            });
        }
    }
    if new.tracks < old.tracks {
        v.push(Regression {
            level: Level::Warn,
            msg: format!("{} track(s) removed ({} -> {})", old.tracks - new.tracks, old.tracks, new.tracks),
        });
    }
    if old.notes > 0 && new.notes * 10 < old.notes * 9 {
        v.push(Regression {
            level: Level::Warn,
            msg: format!("notes dropped {} -> {}", old.notes, new.notes),
        });
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Fingerprint {
        Fingerprint {
            peak_db: -6.0,
            rms_db: -18.0,
            clipped_samples: 0,
            stereo_correlation: 0.5,
            bands: vec![
                ("sub".into(), -30.0),
                ("bass".into(), -24.0),
                ("low-mid".into(), -20.0),
                ("mid".into(), -22.0),
                ("high-mid".into(), -26.0),
                ("presence".into(), -30.0),
                ("air".into(), -34.0),
            ],
            tracks: 4,
            notes: 100,
        }
    }

    fn has(v: &[Regression], level: Level, needle: &str) -> bool {
        v.iter().any(|r| r.level == level && r.msg.contains(needle))
    }

    #[test]
    fn identical_fingerprint_passes() {
        assert!(regressions(&base(), &base()).is_empty());
    }

    #[test]
    fn clipping_introduced_is_a_fail() {
        let old = base();
        let mut new = base();
        new.clipped_samples = 320;
        assert!(has(&regressions(&old, &new), Level::Fail, "clipping"));
    }

    #[test]
    fn hotter_peak_is_a_warn() {
        let old = base();
        let mut new = base();
        new.peak_db = 4.0; // +10 dB
        assert!(has(&regressions(&old, &new), Level::Warn, "peak"));
    }

    #[test]
    fn band_drift_is_a_warn() {
        let old = base();
        let mut new = base();
        new.bands[1].1 = -16.0; // bass +8 dB
        assert!(has(&regressions(&old, &new), Level::Warn, "bass"));
    }

    #[test]
    fn removed_track_is_a_warn() {
        let old = base();
        let mut new = base();
        new.tracks = 3;
        assert!(has(&regressions(&old, &new), Level::Warn, "track"));
    }
}

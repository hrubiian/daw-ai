//! Auto-balance: equalize per-track loudness to the median, measured from
//! unnormalized solo renders over each track's active bars. Pure planning —
//! callers apply the returned volumes via `Project::set_track_volume`.

use crate::project::Project;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct BalanceResult {
    pub track_id: String,
    pub name: String,
    /// Active-bar RMS dBFS measured at the old volume (`NEG_INFINITY` when
    /// the track produced no active bars — left unchanged).
    pub rms_db: f32,
    pub old_volume: f32,
    pub new_volume: f32,
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-7).log10()
}

/// Pure math half: (track_id, rms_db, old_volume) → results targeting the
/// median loudness, new volumes clamped 0.05..=2.0.
pub fn compute_balance(measured: &[(String, f32, f32)]) -> Vec<BalanceResult> {
    let mut sorted: Vec<f32> = measured.iter().map(|(_, r, _)| *r).collect();
    sorted.sort_by(|a, b| a.total_cmp(b));
    if sorted.is_empty() {
        return Vec::new();
    }
    let target = sorted[sorted.len() / 2];
    measured
        .iter()
        .map(|(id, rms, old)| {
            let new = (old * 10f32.powf((target - rms) / 20.0)).clamp(0.05, 2.0);
            BalanceResult {
                track_id: id.clone(),
                name: String::new(),
                rms_db: *rms,
                old_volume: *old,
                new_volume: new,
            }
        })
        .collect()
}

/// Measure every unmuted track solo (unnormalized render) and balance to
/// the median active-bar loudness.
pub fn auto_balance(project: &Project, sf2: &Path) -> anyhow::Result<Vec<BalanceResult>> {
    let bpm = project.project.bpm;
    let mut measured: Vec<(String, f32, f32)> = Vec::new();
    let mut silent: Vec<(String, String, f32)> = Vec::new();
    for t in project.tracks.iter().filter(|t| !t.muted) {
        let mut solo = project.clone();
        for o in &mut solo.tracks {
            o.muted = o.id != t.id;
        }
        let (samples, sr) = super::export::render_project_samples(&solo, sf2, false)?;
        let spb = 240.0 / bpm.max(1.0);
        let frames_per_bar = ((spb * sr as f64) as usize).max(1);
        let frames = samples.len() / 2;
        let mut active: Vec<f32> = Vec::new();
        let mut bar = 0usize;
        while bar * frames_per_bar < frames {
            let lo = bar * frames_per_bar;
            let hi = ((bar + 1) * frames_per_bar).min(frames);
            let sum: f64 = (lo..hi)
                .map(|f| {
                    let m = (samples[f * 2] + samples[f * 2 + 1]) / 2.0;
                    m as f64 * m as f64
                })
                .sum();
            let rms = ((sum / (hi - lo).max(1) as f64).sqrt()) as f32;
            if db(rms) > -60.0 {
                active.push(rms);
            }
            bar += 1;
        }
        if active.is_empty() {
            silent.push((t.id.clone(), t.name.clone(), t.volume));
        } else {
            let avg = active.iter().sum::<f32>() / active.len() as f32;
            measured.push((t.id.clone(), db(avg), t.volume));
        }
    }
    let mut out = compute_balance(&measured);
    for r in &mut out {
        if let Some(t) = project.tracks.iter().find(|t| t.id == r.track_id) {
            r.name = t.name.clone();
        }
    }
    for (id, name, vol) in silent {
        out.push(BalanceResult {
            track_id: id,
            name,
            rms_db: f32::NEG_INFINITY,
            old_volume: vol,
            new_volume: vol,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balance_math_targets_median_and_clamps() {
        let measured = vec![
            ("a".to_string(), -10.0f32, 0.8f32),
            ("b".to_string(), -20.0, 0.8),
            ("c".to_string(), -30.0, 0.8),
        ];
        let out = compute_balance(&measured);
        let v: std::collections::HashMap<_, _> =
            out.iter().map(|r| (r.track_id.clone(), r.new_volume)).collect();
        assert!(v["a"] < 0.8 && v["c"] > 0.8);
        assert!((v["b"] - 0.8).abs() < 1e-6);
        assert!((v["a"] - 0.253).abs() < 0.01, "got {}", v["a"]);
        let out = compute_balance(&[
            ("q".into(), -60.0, 1.0),
            ("l".into(), -10.0, 1.0),
            ("m".into(), -12.0, 1.0),
        ]);
        assert_eq!(out.iter().find(|r| r.track_id == "q").unwrap().new_volume, 2.0);
    }

    #[test]
    fn balance_shrinks_solo_spread_on_default_project() {
        let sf2 = std::path::Path::new("../../assets/GeneralUser.sf2");
        if !sf2.exists() {
            assert!(std::env::var_os("DAW_REQUIRE_TEST_ASSETS").is_none(), "required audio test assets missing"); eprintln!("skip: no soundfont");
            return;
        }
        let mut p = Project::default_project();
        // Skew it: drums quiet, bass loud.
        p.set_track_volume("t1", 0.2);
        p.set_track_volume("t2", 1.0);
        let plan = auto_balance(&p, sf2).unwrap();
        for r in &plan {
            assert!((0.05..=2.0).contains(&r.new_volume), "{r:?}");
            p.set_track_volume(&r.track_id, r.new_volume);
        }
        let after = auto_balance(&p, sf2).unwrap();
        let dbs: Vec<f32> = after
            .iter()
            .filter(|r| r.rms_db.is_finite())
            .map(|r| r.rms_db)
            .collect();
        let spread = dbs.iter().cloned().fold(f32::MIN, f32::max)
            - dbs.iter().cloned().fold(f32::MAX, f32::min);
        assert!(spread < 1.5, "post-balance spread {spread} dB ({dbs:?})");
    }
}

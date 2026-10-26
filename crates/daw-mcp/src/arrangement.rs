//! "AI ear" #2: arrangement shape — per-bar density + dynamic arc, so the agent
//! can tell a building, evolving piece from a flat static texture.

use daw_core::project::Project;
use std::path::Path;

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

fn spark_u32(vals: &[u32], max: u32) -> String {
    if max == 0 {
        return "·".repeat(vals.len());
    }
    vals.iter()
        .map(|&v| if v == 0 { '·' } else { SPARK[(v as usize * 7 / max as usize).min(7)] })
        .collect()
}

fn spark_db(vals: &[f32]) -> String {
    let aud: Vec<f32> = vals.iter().copied().filter(|d| *d > SILENT_DB).collect();
    let (lo, hi) = if aud.len() >= 2 {
        (aud.iter().copied().fold(f32::MAX, f32::min), aud.iter().copied().fold(f32::MIN, f32::max))
    } else {
        (SILENT_DB, 0.0)
    };
    let span = (hi - lo).max(1.0);
    vals.iter()
        .map(|&d| {
            if d <= SILENT_DB {
                '·'
            } else {
                SPARK[(((d - lo) / span * 7.0).round().clamp(0.0, 7.0)) as usize]
            }
        })
        .collect()
}

/// Whole-song arrangement report: density sparkline, dynamic-arc sparkline, and
/// structural findings. Renders the mix once for the loudness curve.
pub fn arrangement_report(project: &Project, sf2: &Path) -> anyhow::Result<String> {
    let song_bars = project.song_bars();
    // Symbolic density: each clip's (start_bar, effective_len).
    let slots: Vec<(u32, u32)> = project
        .arrangement
        .iter()
        .map(|s| {
            let len = s.len_bars.unwrap_or_else(|| {
                project.patterns.iter().find(|p| p.id() == s.pattern).map(|p| p.bars()).unwrap_or(1)
            });
            (s.bar, len.max(1))
        })
        .collect();
    let activity = bar_activity(&slots, song_bars);

    // Dynamic arc: per-bar loudness from one whole-mix render.
    let (samples, sr) = daw_core::audio::export::render_project_samples(project, sf2, false)?;
    let l: Vec<f32> = samples.iter().step_by(2).copied().collect();
    let r: Vec<f32> = samples.iter().skip(1).step_by(2).copied().collect();
    let rep = crate::analysis::analyze(&l, &r, sr, project.project.bpm);
    let mut bar_db = vec![-120.0f32; song_bars.max(1) as usize];
    for (bar, d) in &rep.bar_rms_db {
        let i = (*bar as usize).saturating_sub(1);
        if i < bar_db.len() {
            bar_db[i] = *d;
        }
    }

    let findings = arc_findings(&bar_db, &activity);
    let max_act = activity.iter().copied().max().unwrap_or(0);
    let mut s = format!(
        "arrangement: {song_bars} bars, {} tracks, {} clips\n",
        project.tracks.len(),
        project.arrangement.len()
    );
    s.push_str(&format!("density  (max {max_act}/bar): {}\n", spark_u32(&activity, max_act)));
    s.push_str(&format!("loudness (per-bar dBFS): {}\n", spark_db(&bar_db)));
    if findings.is_empty() {
        s.push_str("no structural issues: the arrangement builds/varies.\n");
    } else {
        s.push_str("\nfindings:\n");
        for f in &findings {
            s.push_str(&format!("  - {}\n", f.0));
        }
    }
    Ok(s)
}

/// A dynamics/structure observation about the whole song.
pub struct ArcFinding(pub String);

/// A song with less than this dynamic range (dB) across its non-silent bars is
/// flagged as a flat/static texture.
const FLAT_RANGE_DB: f32 = 3.0;
/// Bars quieter than this (dBFS) don't count toward the dynamic range.
const SILENT_DB: f32 = -60.0;

/// Active track count per bar `0..song_bars`, from arrangement clips given as
/// `(start_bar, len_bars)`.
pub fn bar_activity(slots: &[(u32, u32)], song_bars: u32) -> Vec<u32> {
    (0..song_bars)
        .map(|b| slots.iter().filter(|(s, len)| *s <= b && b < s + len).count() as u32)
        .collect()
}

/// Structural/dynamics findings from the per-bar loudness curve and density map.
pub fn arc_findings(bar_db: &[f32], activity: &[u32]) -> Vec<ArcFinding> {
    let mut out = Vec::new();

    // Flat dynamics: tiny loudness range across the audible bars.
    let audible: Vec<f32> = bar_db.iter().copied().filter(|d| *d > SILENT_DB).collect();
    if audible.len() >= 2 {
        let max = audible.iter().copied().fold(f32::MIN, f32::max);
        let min = audible.iter().copied().fold(f32::MAX, f32::min);
        let range = max - min;
        if range < FLAT_RANGE_DB {
            out.push(ArcFinding(format!(
                "flat: only {range:.1} dB of dynamic variation across {} bars \
                 (static texture — consider a build, breakdown, or section changes)",
                bar_db.len()
            )));
        }
    }

    // No intro build: full density already at the first bar.
    if let (Some(&first), Some(&max)) = (activity.first(), activity.iter().max()) {
        if max > 0 && first == max && activity.len() > 1 {
            out.push(ArcFinding(format!(
                "no intro build: all {max} tracks active from bar 1 \
                 (stagger entries for a build)"
            )));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(f: &[ArcFinding]) -> String {
        f.iter().map(|x| x.0.clone()).collect::<Vec<_>>().join(" | ")
    }

    #[test]
    fn activity_counts_overlapping_clips_per_bar() {
        // two 4-bar clips both from bar 0 => 2 active every bar
        assert_eq!(bar_activity(&[(0, 4), (0, 4)], 4), vec![2, 2, 2, 2]);
    }

    #[test]
    fn activity_tracks_staggered_entries() {
        // clips entering at bars 0,1,2 (each 4 long) over a 4-bar span
        assert_eq!(bar_activity(&[(0, 4), (1, 4), (2, 4)], 4), vec![1, 2, 3, 3]);
    }

    #[test]
    fn flat_loudness_is_flagged_static() {
        let bar_db = vec![-20.0, -20.5, -19.8, -20.2]; // ~0.7 dB range
        let activity = vec![3, 3, 3, 3];
        let f = arc_findings(&bar_db, &activity);
        assert!(texts(&f).contains("flat"), "{}", texts(&f));
    }

    #[test]
    fn building_loudness_is_not_flagged_static() {
        let bar_db = vec![-30.0, -25.0, -20.0, -15.0]; // 15 dB build
        let activity = vec![1, 2, 3, 4];
        let f = arc_findings(&bar_db, &activity);
        assert!(!texts(&f).contains("flat"), "{}", texts(&f));
    }

    #[test]
    fn all_tracks_from_bar_zero_flags_no_intro() {
        let bar_db = vec![-18.0, -12.0, -20.0, -14.0]; // varied, so only the intro flag
        let activity = vec![4, 4, 4, 4]; // full density from the first bar
        let f = arc_findings(&bar_db, &activity);
        assert!(texts(&f).contains("no intro"), "{}", texts(&f));
    }

    #[test]
    fn staggered_entry_has_no_intro_flag() {
        let bar_db = vec![-24.0, -18.0, -14.0, -12.0];
        let activity = vec![1, 2, 3, 4]; // builds up
        let f = arc_findings(&bar_db, &activity);
        assert!(!texts(&f).contains("no intro"), "{}", texts(&f));
    }
}

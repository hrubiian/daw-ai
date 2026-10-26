//! "AI ear" #3: groove/timing feel. Symbolic (no render) — how notes sit against
//! the 16th grid plus the project's swing/humanize/timing, so the agent can tell
//! a stiff, dead-on-grid pattern from one with human feel.

use daw_core::project::{Pattern, Project};

/// Symbolic groove report: grid alignment of all piano-roll notes plus the
/// project's swing/humanize/timing. No render required.
pub fn groove_report(project: &Project) -> String {
    let mut offsets: Vec<i32> = Vec::new();
    let mut n_pat = 0usize;
    for pat in &project.patterns {
        if let Pattern::PianoRoll(pr) = pat {
            if pr.notes.is_empty() {
                continue;
            }
            let grid = (pr.ticks_per_bar / 16).max(1);
            n_pat += 1;
            for note in &pr.notes {
                offsets.push(nearest_grid_offset(note.tick, grid));
            }
        }
    }
    let m = &project.project;
    if offsets.is_empty() {
        return format!(
            "groove: no piano-roll notes to analyze (step patterns are grid-locked; \
             swing {:.2}, humanize {:.2}, timing {:.2} apply at playback).",
            m.swing, m.humanize, m.timing
        );
    }
    let (frac, rms) = timing_stats(&offsets, 1);
    let findings = groove_findings(frac, rms, m.swing, m.humanize, m.timing);
    let mut s = format!("groove: {} notes across {n_pat} melodic pattern(s)\n", offsets.len());
    s.push_str(&format!(
        "  on-grid (16th): {:.0}%   micro-timing RMS: {rms:.1} ticks\n",
        frac * 100.0
    ));
    s.push_str(&format!(
        "  swing: {:.2}   humanize: {:.2}   timing: {:.2}\n",
        m.swing, m.humanize, m.timing
    ));
    if findings.is_empty() {
        s.push_str("feel: OK — not dead-mechanical.\n");
    } else {
        s.push_str("\nfindings:\n");
        for f in &findings {
            s.push_str(&format!("  - {f}\n"));
        }
    }
    s
}

/// Signed distance (ticks) from `tick` to its nearest `grid` line: positive =
/// behind the beat (late), negative = ahead (early).
pub fn nearest_grid_offset(tick: u32, grid: u32) -> i32 {
    if grid == 0 {
        return 0;
    }
    let (rem, g) = ((tick % grid) as i32, grid as i32);
    if rem <= g / 2 { rem } else { rem - g }
}

/// (fraction of offsets within `tol` ticks of the grid, RMS offset in ticks).
pub fn timing_stats(offsets: &[i32], tol: i32) -> (f32, f32) {
    if offsets.is_empty() {
        return (1.0, 0.0);
    }
    let on = offsets.iter().filter(|o| o.abs() <= tol).count();
    let frac = on as f32 / offsets.len() as f32;
    let rms = (offsets.iter().map(|o| (*o * *o) as f32).sum::<f32>() / offsets.len() as f32).sqrt();
    (frac, rms)
}

/// Feel findings from grid alignment + the project's groove settings.
pub fn groove_findings(on_grid_frac: f32, rms_ticks: f32, swing: f32, humanize: f32, timing: f32) -> Vec<String> {
    let mut out = Vec::new();
    let has_settings = swing >= 0.02 || humanize >= 0.02 || timing >= 0.02;
    // Dead on the grid AND no engine feel applied => robotic.
    if on_grid_frac > 0.98 && rms_ticks < 1.0 && !has_settings {
        out.push(
            "mechanical: notes are dead on the 16th grid with no swing/humanize/timing — \
             add swing, humanize, or off-grid note placement for feel"
                .into(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_is_zero_on_grid() {
        assert_eq!(nearest_grid_offset(60, 30), 0);
    }

    #[test]
    fn offset_is_positive_when_late() {
        assert_eq!(nearest_grid_offset(62, 30), 2); // 2 ticks behind the 60 line
    }

    #[test]
    fn offset_is_negative_when_early() {
        assert_eq!(nearest_grid_offset(28, 30), -2); // 2 ticks ahead of the 30 line
    }

    #[test]
    fn all_on_grid_stats() {
        let (frac, rms) = timing_stats(&[0, 0, 0, 0], 1);
        assert_eq!(frac, 1.0);
        assert_eq!(rms, 0.0);
    }

    #[test]
    fn spread_stats() {
        let (frac, rms) = timing_stats(&[0, 6, -6], 1);
        assert!((frac - 1.0 / 3.0).abs() < 1e-6, "frac {frac}");
        assert!((rms - (72.0f32 / 3.0).sqrt()).abs() < 1e-3, "rms {rms}");
    }

    #[test]
    fn dead_on_grid_no_settings_is_mechanical() {
        let f = groove_findings(1.0, 0.0, 0.0, 0.0, 0.0);
        assert!(f.iter().any(|s| s.contains("mechanical")), "{f:?}");
    }

    #[test]
    fn swing_gives_feel_not_mechanical() {
        let f = groove_findings(1.0, 0.0, 0.12, 0.0, 0.0);
        assert!(!f.iter().any(|s| s.contains("mechanical")), "{f:?}");
    }

    #[test]
    fn authored_microtiming_is_not_mechanical() {
        let f = groove_findings(0.5, 6.0, 0.0, 0.0, 0.0);
        assert!(!f.iter().any(|s| s.contains("mechanical")), "{f:?}");
    }
}

//! "AI ear" #3: groove/timing feel. Symbolic (no render) — how notes sit against
//! the 16th grid plus the project's swing/humanize/timing, so the agent can tell
//! a stiff, dead-on-grid pattern from one with human feel.

use daw_core::project::{Pattern, Project};

/// Symbolic groove report: grid alignment of active piano-roll notes plus the
/// project's swing/humanize/timing. No render required.
pub fn groove_report(project: &Project) -> String {
    let mut offsets: Vec<i32> = Vec::new();
    let mut n_pat = 0usize;
    let mut per_track: Vec<(String, usize, f32, f32)> = Vec::new();
    let m = &project.project;
    let has_settings = m.swing >= 0.02 || m.humanize >= 0.02 || m.timing >= 0.02;

    const SIXTEENTH: u32 = daw_core::musical_time::TICKS_PER_BEAT / 4; // 30 ticks

    for pat in &project.patterns {
        if let Pattern::PianoRoll(pr) = pat {
            let mut notes = pr.active_notes().peekable();
            if notes.peek().is_none() {
                continue;
            }
            n_pat += 1;
            for note in notes {
                offsets.push(nearest_grid_offset(note.tick, SIXTEENTH));
            }
        }
    }

    for tr in &project.tracks {
        if tr.muted || tr.track_type != daw_core::project::TrackType::Melodic {
            continue;
        }
        let mut t_offsets = Vec::new();
        for pid in &tr.patterns {
            if let Some(Pattern::PianoRoll(pr)) = project.patterns.iter().find(|p| p.id() == pid) {
                for note in pr.active_notes() {
                    t_offsets.push(nearest_grid_offset(note.tick, SIXTEENTH));
                }
            }
        }
        if !t_offsets.is_empty() {
            let (tfrac, trms) = timing_stats(&t_offsets, 1);
            per_track.push((tr.name.clone(), t_offsets.len(), tfrac, trms));
        }
    }

    if offsets.is_empty() {
        return format!(
            "groove: no piano-roll notes to analyze (step patterns are grid-locked; \
             swing {:.2}, humanize {:.2}, timing {:.2} apply at playback).",
            m.swing, m.humanize, m.timing
        );
    }
    let (frac, rms) = timing_stats(&offsets, 1);
    let mut findings = groove_findings(frac, rms, m.swing, m.humanize, m.timing);
    if !has_settings {
        let locked: Vec<String> = per_track
            .iter()
            .filter(|(_, count, tfrac, trms)| *count >= 8 && *tfrac > 0.98 && *trms < 1.0)
            .map(|(name, _, _, _)| name.clone())
            .collect();
        if !locked.is_empty() && findings.is_empty() {
            findings.push(format!(
                "mechanical track(s): {} is 100% grid-locked while another track supplies all timing spread",
                locked.join(", ")
            ));
        }
    }

    let mut s = format!("groove: {} notes across {n_pat} melodic pattern(s)\n", offsets.len());
    s.push_str(&format!(
        "  on-grid (16th): {:.0}%   micro-timing RMS: {rms:.1} ticks\n",
        frac * 100.0
    ));
    s.push_str(&format!(
        "  swing: {:.2}   humanize: {:.2}   timing: {:.2}\n",
        m.swing, m.humanize, m.timing
    ));
    if !per_track.is_empty() {
        s.push_str("  per-track:\n");
        for (name, count, tfrac, trms) in &per_track {
            s.push_str(&format!(
                "    {name}: {count} notes, {:.0}% on-grid, RMS {trms:.1} ticks\n",
                tfrac * 100.0
            ));
        }
    }
    if findings.is_empty() {
        s.push_str("notes depart from the grid or playback feel settings are enabled.\n");
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

    #[test]
    fn flags_mechanical_lead_track_even_when_accompaniment_rolls_chords() {
        let mut p = Project::default_project();
        let lead = p.add_track(daw_core::project::TrackType::Melodic);
        let lead_pid = p.tracks.iter().find(|t| t.id == lead).unwrap().patterns[0].clone();
        if let Some(Pattern::PianoRoll(pr)) = p.patterns.iter_mut().find(|pat| pat.id() == lead_pid) {
            pr.notes = (0..8).map(|i| daw_core::project::Note {
                pitch: 69, tick: i * 240, duration: 200, vel: 80,
            }).collect();
        }
        // Accompaniment track has rolled off-grid notes
        if let Some(Pattern::PianoRoll(pr)) = p.patterns.iter_mut().find(|pat| pat.id() == "p2") {
            pr.notes = (0..8).map(|i| daw_core::project::Note {
                pitch: 48, tick: i * 120 + 10, duration: 100, vel: 70,
            }).collect();
        }
        let rep = groove_report(&p);
        assert!(rep.contains("mechanical track(s):"), "{rep}");
    }

    #[test]
    fn seven_four_sixteenth_notes_are_recognized_as_on_grid() {
        let mut p = Project::default_project();
        p.set_time_signature(7, 4).unwrap();
        p.patterns[1] = Pattern::PianoRoll(daw_core::project::PianoRollPattern {
            id: "p2".into(),
            name: "7/4 Ostinato".into(),
            bars: 1,
            ticks_per_bar: 840,
            notes: [0, 30, 60, 90, 120, 150]
                .into_iter()
                .map(|tick| daw_core::project::Note { pitch: 48, tick, duration: 25, vel: 90 })
                .collect(),
            legato: false,
            cc_events: Vec::new(),
            pitch_bends: Vec::new(),
            legato_overlap_ticks: 0,
            legato_max_gap_ticks: None,
            swing: None,
        });
        let rep = groove_report(&p);
        assert!(rep.contains("on-grid (16th): 100%"), "{rep}");
        assert!(rep.contains("RMS: 0.0 ticks"), "{rep}");
    }
}

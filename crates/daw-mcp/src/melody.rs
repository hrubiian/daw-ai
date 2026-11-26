//! Symbolic melodic & motivic lint ("AI ear" #4): evaluates whether a
//! designated lead track actually carries a singable, rhythmically independent,
//! breathing, recurring melody rather than chord-tone wallpaper.

use daw_core::project::{Pattern, Project, TrackType};
use daw_core::sequencer::engine::{
    arranged_track_patterns, pattern_length_ticks, piano_note_duration,
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
struct MelEvent {
    tick: u32,
    duration: u32,
    pitch: u8,
}

fn collect_track_events(
    project: &Project,
    track_id: &str,
    start_bar: u32,
    end_bar: u32,
) -> (Vec<MelEvent>, f32) {
    let tpb = project.ticks_per_bar();
    let start_tick = start_bar * tpb;
    let end_tick = end_bar * tpb;
    let mut raw: Vec<MelEvent> = Vec::new();

    if !project.arrangement.is_empty() {
        for slot in arranged_track_patterns(project, track_id) {
            let Pattern::PianoRoll(pr) = slot.pattern else { continue };
            let pat_len = pattern_length_ticks(slot.pattern).max(1);
            let mut offset = 0u32;
            while slot.start + offset < slot.end {
                for n in pr.active_notes() {
                    let abs_tick = slot.start + offset + n.tick;
                    if abs_tick >= slot.end {
                        continue;
                    }
                    if abs_tick >= start_tick && abs_tick < end_tick {
                        raw.push(MelEvent {
                            tick: abs_tick,
                            duration: piano_note_duration(pr, n),
                            pitch: n.pitch,
                        });
                    }
                }
                if !slot.repeats {
                    break;
                }
                offset += pat_len;
            }
        }
    } else if let Some(track) = project.tracks.iter().find(|t| t.id == track_id) {
        if let Some(pid) = track.patterns.first() {
            if let Some(Pattern::PianoRoll(pr)) = project.patterns.iter().find(|p| p.id() == pid) {
                for n in pr.active_notes() {
                    if n.tick >= start_tick && n.tick < end_tick {
                        raw.push(MelEvent {
                            tick: n.tick,
                            duration: piano_note_duration(pr, n),
                            pitch: n.pitch,
                        });
                    }
                }
            }
        }
    }

    raw.sort_by_key(|e| (e.tick, e.pitch));
    if raw.is_empty() {
        return (raw, 0.0);
    }

    // Collapse simultaneous chord notes (within 20 ticks) to the top/skyline pitch,
    // while tracking polyphonic chord fraction so callers see if a lead is block chords.
    let mut skyline: Vec<MelEvent> = Vec::new();
    let mut chord_groups = 0usize;
    let mut total_groups = 0usize;
    let mut i = 0usize;
    while i < raw.len() {
        let anchor = raw[i].tick;
        let mut best = raw[i];
        let mut j = i + 1;
        while j < raw.len() && raw[j].tick - anchor <= 20 {
            if raw[j].pitch >= best.pitch {
                best = raw[j];
            }
            j += 1;
        }
        total_groups += 1;
        if j - i >= 2 {
            chord_groups += 1;
        }
        skyline.push(best);
        i = j;
    }
    let poly_frac = chord_groups as f32 / total_groups.max(1) as f32;
    (skyline, poly_frac)
}

fn collect_accomp_onsets(
    project: &Project,
    lead_track_id: &str,
    start_bar: u32,
    end_bar: u32,
) -> Vec<u32> {
    let mut all = Vec::new();
    for tr in &project.tracks {
        if tr.muted || tr.id == lead_track_id || tr.track_type != TrackType::Melodic {
            continue;
        }
        let (evs, _) = collect_track_events(project, &tr.id, start_bar, end_bar);
        all.extend(evs.into_iter().map(|e| e.tick));
    }
    all.sort_unstable();
    all
}

fn duration_bucket(dur: u32) -> usize {
    if dur <= 45 {
        0 // 16th
    } else if dur <= 95 {
        1 // 8th
    } else if dur <= 150 {
        2 // quarter
    } else if dur <= 210 {
        3 // dotted quarter
    } else if dur <= 300 {
        4 // half
    } else {
        5 // long / whole
    }
}

fn dur_rel(a: u32, b: u32) -> i8 {
    let fa = a.max(1) as f32;
    let fb = b.max(1) as f32;
    if fb > fa * 1.4 {
        1
    } else if fa > fb * 1.4 {
        -1
    } else {
        0
    }
}

fn contour_class(dp: i16) -> i8 {
    match dp {
        0 => 0,
        1..=2 => 1,
        -2..=-1 => -1,
        3..=127 => 2,
        _ => -2,
    }
}

/// Analyze a melodic track's skyline phrasing, rhythmic independence, motivic
/// recurrence, breath structure, and onset collision with accompaniment.
pub fn melody_report(
    project: &Project,
    track_id: &str,
    start_bar: Option<u32>,
    end_bar: Option<u32>,
) -> Result<String, String> {
    let track = project
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .ok_or_else(|| format!("no track '{track_id}'"))?;
    let sb = start_bar.unwrap_or(0);
    let eb = end_bar.unwrap_or_else(|| project.song_bars()).max(sb + 1);
    let (events, poly_frac) = collect_track_events(project, track_id, sb, eb);
    if events.is_empty() {
        return Ok(format!(
            "melody ({} \"{}\", bars {sb}..{eb}): 0 piano-roll notes.",
            track.id, track.name
        ));
    }

    let n = events.len();
    let min_p = events.iter().map(|e| e.pitch).min().unwrap_or(60);
    let max_p = events.iter().map(|e| e.pitch).max().unwrap_or(60);
    let span_semitones = max_p - min_p;

    let mut steps = 0usize;
    let mut leaps = 0usize;
    let mut big_jumps = 0usize;
    for w in events.windows(2) {
        let d = (w[1].pitch as i16 - w[0].pitch as i16).abs();
        if (1..=2).contains(&d) {
            steps += 1;
        } else if (3..=9).contains(&d) {
            leaps += 1;
        } else if d >= 10 {
            big_jumps += 1;
        }
    }
    let intervals = (n.saturating_sub(1)).max(1) as f32;

    // Rhythmic independence
    let strong_1_3 = events
        .iter()
        .filter(|e| {
            let b = e.tick % 240; // distance to beat 1 or beat 3
            b <= 15 || b >= 225
        })
        .count();
    let strong_1_3_pct = strong_1_3 as f32 / n as f32;

    let tpb = project.ticks_per_bar();
    let pickups_or_sync = events
        .iter()
        .filter(|e| {
            let beat_off = e.tick % 120;
            let off_beat = beat_off > 15 && beat_off < 105;
            let beat4_pickup = (e.tick % tpb) >= tpb.saturating_sub(135);
            off_beat || beat4_pickup
        })
        .count();
    let pickup_pct = pickups_or_sync as f32 / n as f32;

    let mut buckets = [0usize; 6];
    for e in &events {
        buckets[duration_bucket(e.duration)] += 1;
    }
    let max_bucket = *buckets.iter().max().unwrap_or(&0);
    let dominant_dur_pct = max_bucket as f32 / n as f32;

    // Phrase breathing (gaps >= 60 ticks = eighth rest)
    let mut phrase_rests = 0usize;
    for w in events.windows(2) {
        let prev_end = w[0].tick.saturating_add(w[0].duration);
        if w[1].tick >= prev_end + 60 {
            phrase_rests += 1;
        }
    }
    let phrases = phrase_rests + 1;
    let mean_notes_per_phrase = n as f32 / phrases as f32;

    // Motivic recurrence: 4-note cells with exact or contour-class interval match >= 2 bars apart
    type ExactKey = ([i16; 3], [i8; 2]);
    type ContourKey = ([i8; 3], [i8; 2]);
    let mut exact_seen: HashMap<ExactKey, Vec<u32>> = HashMap::new();
    let mut contour_seen: HashMap<ContourKey, Vec<u32>> = HashMap::new();
    for w in events.windows(4) {
        let dp = [
            (w[1].pitch as i16 - w[0].pitch as i16).clamp(-12, 12),
            (w[2].pitch as i16 - w[1].pitch as i16).clamp(-12, 12),
            (w[3].pitch as i16 - w[2].pitch as i16).clamp(-12, 12),
        ];
        let dr = [
            dur_rel(w[0].duration, w[1].duration),
            dur_rel(w[1].duration, w[2].duration),
        ];
        exact_seen.entry((dp, dr)).or_default().push(w[0].tick);
        let cp = [contour_class(dp[0]), contour_class(dp[1]), contour_class(dp[2])];
        contour_seen.entry((cp, dr)).or_default().push(w[0].tick);
    }
    let recurring_exact = exact_seen
        .values()
        .filter(|ticks| ticks.last().unwrap_or(&0) - ticks.first().unwrap_or(&0) >= 2 * tpb)
        .count();
    let recurring_contour = contour_seen
        .values()
        .filter(|ticks| ticks.last().unwrap_or(&0) - ticks.first().unwrap_or(&0) >= 2 * tpb)
        .count();

    // Onset collision against other melodic tracks
    let accomp = collect_accomp_onsets(project, track_id, sb, eb);
    let mut collided = 0usize;
    if !accomp.is_empty() {
        for e in &events {
            let lo = e.tick.saturating_sub(15);
            let hi = e.tick.saturating_add(15);
            let hits = accomp
                .iter()
                .filter(|&&t| t >= lo && t <= hi)
                .count();
            if hits >= 1 {
                collided += 1;
            }
        }
    }
    let collision_pct = collided as f32 / n as f32;

    // Findings
    let mut findings: Vec<String> = Vec::new();
    if n >= 8 && strong_1_3_pct > 0.80 {
        findings.push(format!(
            "WALLPAPER_LOCK: {:.0}% of melody notes start on beats 1 or 3 — add anacruses, dotted rhythms, or ties across barlines",
            strong_1_3_pct * 100.0
        ));
    }
    if n >= 8 && dominant_dur_pct > 0.75 {
        findings.push(format!(
            "ISORHYTHMIC: {:.0}% of notes share one duration bucket — contrast quick pickups/ornaments with held lyrical peaks",
            dominant_dur_pct * 100.0
        ));
    }
    if n >= 8 && pickup_pct < 0.10 {
        findings.push(format!(
            "NO_PICKUPS_OR_SYNCOPATION: only {:.0}% of notes enter off-beat or as beat-4 pickups",
            pickup_pct * 100.0
        ));
    }
    if n >= 6 && mean_notes_per_phrase < 2.5 {
        findings.push(format!(
            "FRAGMENTED_CHIMES: average phrase length is only {mean_notes_per_phrase:.1} notes — connect notes into 4–8 note vocal sentences"
        ));
    }
    let tick_span = events.last().map(|e| e.tick).unwrap_or(0) - events.first().map(|e| e.tick).unwrap_or(0);
    if tick_span >= 6 * tpb && phrase_rests == 0 {
        findings.push("RUN_ON_NO_BREATH: 6+ bars with zero phrase rests (>= 60 ticks)".into());
    }
    if n >= 12 && tick_span >= 6 * tpb && recurring_exact == 0 && recurring_contour == 0 {
        findings.push(
            "NO_MOTIVIC_RECURRENCE: no 4-note interval/contour cell returns >= 2 bars later — sequence or vary the opening motif".into()
        );
    }
    if !accomp.is_empty() && n >= 8 && collision_pct > 0.70 {
        findings.push(format!(
            "ONSET_MASKING: {:.0}% of melody attacks land within 15 ticks of accompaniment attacks — offset rhythm or drop chord attacks under lead entries",
            collision_pct * 100.0
        ));
    }
    if poly_frac > 0.45 && n >= 6 {
        findings.push(format!(
            "CHORDAL_LEAD: {:.0}% of attacks are multi-note chords rather than a dedicated singing line",
            poly_frac * 100.0
        ));
    }

    let mut out = format!(
        "melody ({} \"{}\", bars {sb}..{eb}): {n} skyline notes, range {span_semitones} st (MIDI {min_p}..{max_p})\n",
        track.id, track.name
    );
    out.push_str(&format!(
        "  contour: {:.0}% stepwise (1–2 st), {:.0}% expressive leaps (3–9 st), {:.0}% large jumps (>=10 st)\n",
        steps as f32 / intervals * 100.0,
        leaps as f32 / intervals * 100.0,
        big_jumps as f32 / intervals * 100.0,
    ));
    out.push_str(&format!(
        "  rhythm: {:.0}% on beats 1/3, {:.0}% pickups/syncopation, {:.0}% in single duration bucket\n",
        strong_1_3_pct * 100.0,
        pickup_pct * 100.0,
        dominant_dur_pct * 100.0,
    ));
    out.push_str(&format!(
        "  phrasing: {phrases} phrase(s) ({phrase_rests} breath rests), avg {mean_notes_per_phrase:.1} notes/phrase\n"
    ));
    out.push_str(&format!(
        "  motifs: {recurring_exact} exact / {recurring_contour} contour 4-note cell(s) recurring >= 2 bars apart\n"
    ));
    if !accomp.is_empty() {
        out.push_str(&format!(
            "  accompaniment onset collisions (±15 ticks): {:.0}%\n",
            collision_pct * 100.0
        ));
    }
    if findings.is_empty() {
        out.push_str("no melodic lint warnings.\n");
    } else {
        out.push_str("\nfindings:\n");
        for f in &findings {
            out.push_str(&format!("  - {f}\n"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_core::project::{ArrangementSlot, Note, Pattern, PianoRollPattern, TrackType};

    #[test]
    fn wallpaper_half_notes_flag_lock_isorhythm_and_missing_motif() {
        let mut p = Project::default_project();
        p.tracks.clear();
        p.patterns.clear();
        p.arrangement.clear();
        let tid = p.add_track(TrackType::Melodic);
        let pid = p.tracks[0].patterns[0].clone();
        // 8 bars of half-notes strictly on beats 1 and 3 with no recurring 4-note cell
        let pitches = [58, 62, 67, 71, 73, 74, 76, 76, 76, 76, 74, 73, 71, 67, 62, 58];
        let notes: Vec<Note> = pitches
            .iter()
            .enumerate()
            .map(|(i, &pitch)| Note { pitch, tick: i as u32 * 240, duration: 220, vel: 80 })
            .collect();
        p.patterns[0] = Pattern::PianoRoll(PianoRollPattern {
            id: pid.clone(),
            name: "Wallpaper".into(),
            bars: 8,
            ticks_per_bar: 480,
            notes,
            legato: false,
            cc_events: Vec::new(),
            pitch_bends: Vec::new(),
            legato_overlap_ticks: 0,
            legato_max_gap_ticks: None,
            swing: None,
        });
        p.arrangement.push(ArrangementSlot {
            track: tid.clone(), pattern: pid, bar: 0, offset_ticks: 0, len_bars: None, gain: 1.0,
        });
        let rep = melody_report(&p, &tid, None, None).unwrap();
        assert!(rep.contains("WALLPAPER_LOCK"), "{rep}");
        assert!(rep.contains("ISORHYTHMIC"), "{rep}");
        assert!(rep.contains("NO_PICKUPS_OR_SYNCOPATION"), "{rep}");
        assert!(rep.contains("NO_MOTIVIC_RECURRENCE"), "{rep}");
    }

    #[test]
    fn lyrical_theme_with_anacrusis_rests_and_transposed_motif_passes_cleanly() {
        let mut p = Project::default_project();
        p.tracks.clear();
        p.patterns.clear();
        p.arrangement.clear();
        let tid = p.add_track(TrackType::Melodic);
        let pid = p.tracks[0].patterns[0].clone();
        // Phrase 1 (bar 0..4): beat-4 pickup A4-D5 -> dotted-quarter F5 -> E5 -> Bb5 -> A5 -> rest
        // Phrase 2 (bar 4..8): transposed sequence C5-F5 -> dotted-quarter A5 -> G5 -> C6 -> Bb5
        let notes = vec![
            // Phrase 1
            Note { pitch: 69, tick: 360, duration: 60, vel: 68 },
            Note { pitch: 74, tick: 420, duration: 60, vel: 76 },
            Note { pitch: 77, tick: 480, duration: 180, vel: 92 },
            Note { pitch: 76, tick: 660, duration: 60, vel: 80 },
            Note { pitch: 82, tick: 720, duration: 120, vel: 96 },
            Note { pitch: 81, tick: 840, duration: 240, vel: 82 },
            // Phrase rest 1080..1800, then transposed motif at bar 3 beat 4 (1800)
            Note { pitch: 72, tick: 1800, duration: 60, vel: 72 },
            Note { pitch: 77, tick: 1860, duration: 60, vel: 80 },
            Note { pitch: 81, tick: 1920, duration: 180, vel: 98 },
            Note { pitch: 79, tick: 2100, duration: 60, vel: 84 },
            Note { pitch: 84, tick: 2160, duration: 120, vel: 102 },
            Note { pitch: 82, tick: 2280, duration: 300, vel: 86 },
        ];
        p.patterns[0] = Pattern::PianoRoll(PianoRollPattern {
            id: pid.clone(),
            name: "Lyrical".into(),
            bars: 8,
            ticks_per_bar: 480,
            notes,
            legato: false,
            cc_events: Vec::new(),
            pitch_bends: Vec::new(),
            legato_overlap_ticks: 0,
            legato_max_gap_ticks: None,
            swing: None,
        });
        p.arrangement.push(ArrangementSlot {
            track: tid.clone(), pattern: pid, bar: 0, offset_ticks: 0, len_bars: None, gain: 1.0,
        });
        let rep = melody_report(&p, &tid, None, None).unwrap();
        assert!(rep.contains("no melodic lint warnings"), "{rep}");
    }
}

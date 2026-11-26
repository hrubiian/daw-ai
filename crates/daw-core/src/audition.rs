//! Short test phrases for auditioning a sound in the song's key, and the
//! solo one-track project they render through. Pure — no I/O.

use crate::project::{Note, Pattern, PianoRollPattern, Project};
use crate::sequencer::{TICKS_PER_BAR, TICKS_PER_BEAT};
use serde::{Deserialize, Serialize};

/// One test note, timed in milliseconds from the phrase start.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditionNote {
    pub pitch: u8,
    pub velocity: u8,
    pub start_ms: u32,
    pub dur_ms: u32,
}

/// Built-in phrase shapes: a short hit, a held note, a held triad, or a
/// line of eighth notes walking up the scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhraseKind {
    Hit,
    Note,
    Chord,
    Line,
}

/// Longest phrase and most notes an audition may play.
pub const MAX_PHRASE_MS: u32 = 16_000;
pub const MAX_NOTES: usize = 64;

/// Pitch class of a key name ("C", "F#", "Bb"); unknown names are C.
fn key_pitch_class(key: &str) -> i32 {
    let mut chars = key.trim().chars();
    let base = match chars.next().map(|c| c.to_ascii_uppercase()) {
        Some('C') => 0, Some('D') => 2, Some('E') => 4, Some('F') => 5,
        Some('G') => 7, Some('A') => 9, Some('B') => 11,
        _ => return 0,
    };
    match chars.next() {
        Some('#') => base + 1,
        Some('b') => base - 1,
        _ => base,
    }
}

/// A test phrase in `key`/`scale` with its root in `octave` (3 = MIDI
/// 48–59), at `bpm`.
pub fn default_phrase(kind: PhraseKind, key: &str, scale: &str, octave: i8, bpm: f64) -> Vec<AuditionNote> {
    let root = ((octave as i32 + 1) * 12 + key_pitch_class(key)).clamp(0, 115) as u8;
    let steps = crate::generative::scale_intervals(scale);
    let note = |offset: i32, start_ms: u32, dur_ms: u32| AuditionNote {
        pitch: (root as i32 + offset).clamp(0, 127) as u8,
        velocity: 100,
        start_ms,
        dur_ms,
    };
    match kind {
        PhraseKind::Hit => vec![note(0, 0, 250)],
        PhraseKind::Note => vec![note(0, 0, 1500)],
        PhraseKind::Chord => [0, 2, 4]
            .iter()
            .map(|&i| note(steps.get(i).copied().unwrap_or(0), 0, 2000))
            .collect(),
        PhraseKind::Line => {
            let eighth = (30_000.0 / bpm.clamp(crate::musical_time::MIN_BPM, crate::musical_time::MAX_BPM)).round() as u32;
            (0..8)
                .map(|i| {
                    let offset = steps.get(i).copied().unwrap_or(12);
                    note(offset, i as u32 * eighth, (eighth * 9 / 10).max(1))
                })
                .collect()
        }
    }
}

/// Reject phrases an audition can't or shouldn't play.
pub fn validate_notes(notes: &[AuditionNote]) -> Result<(), String> {
    if notes.is_empty() {
        return Err("audition needs at least one note".into());
    }
    if notes.len() > MAX_NOTES {
        return Err(format!("audition has {} notes; at most {MAX_NOTES}", notes.len()));
    }
    for n in notes {
        if n.pitch > 127 || !(1..=127).contains(&n.velocity) || n.dur_ms == 0 {
            return Err(format!("invalid audition note {n:?} (pitch 0-127, velocity 1-127, duration > 0)"));
        }
        if n.start_ms.saturating_add(n.dur_ms) > MAX_PHRASE_MS {
            return Err(format!("audition note ends past {} s", MAX_PHRASE_MS / 1000));
        }
    }
    Ok(())
}

/// A phrase as piano-roll notes at `bpm` (ms → ticks), plus the bars it
/// spans (at least one).
pub fn phrase_notes(notes: &[AuditionNote], bpm: f64) -> (Vec<Note>, u8) {
    let ticks = |ms: u32| (ms as f64 * bpm / 60_000.0 * TICKS_PER_BEAT as f64).round() as u32;
    let notes: Vec<Note> = notes.iter().map(|n| Note {
        pitch: n.pitch,
        tick: ticks(n.start_ms),
        duration: ticks(n.dur_ms).max(1),
        vel: n.velocity,
    }).collect();
    let end = notes.iter().map(|n| n.tick + n.duration).max().unwrap_or(1);
    (notes, end.div_ceil(TICKS_PER_BAR).clamp(1, u8::MAX as u32) as u8)
}

/// `project` reduced to `track_id` alone (unmuted) playing `notes` once:
/// one piano-roll pattern, no arrangement, no audio clips. Its instrument,
/// effects and returns are kept, so the render is the sound in context.
pub fn audition_project(project: &Project, track_id: &str, notes: &[AuditionNote]) -> Result<Project, String> {
    validate_notes(notes)?;
    let mut track = project.tracks.iter().find(|t| t.id == track_id).cloned()
        .ok_or_else(|| format!("no track '{track_id}'"))?;
    let tpb = project.ticks_per_bar();
    let (notes, bars) = phrase_notes(notes, project.project.bpm);
    let end = notes.iter().map(|n| n.tick + n.duration).max().unwrap_or(1);
    let bars = bars.max(end.div_ceil(tpb).clamp(1, u8::MAX as u32) as u8);
    let pid = "audition".to_string();
    track.muted = false;
    track.patterns = vec![pid.clone()];
    let mut out = project.clone();
    out.tracks = vec![track];
    out.patterns = vec![Pattern::PianoRoll(PianoRollPattern {
        id: pid,
        name: "Audition".into(),
        bars,
        ticks_per_bar: tpb,
        notes,
        legato: false,
        cc_events: Vec::new(),
        pitch_bends: Vec::new(),
        legato_overlap_ticks: 0,
        legato_max_gap_ticks: None,
        swing: None,
    })];
    out.arrangement.clear();
    out.audio_clips.clear();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Pattern, Project};

    fn pcs(root: u8, scale: &str) -> Vec<u8> {
        crate::generative::scale_pitches(root, scale).iter().map(|p| p % 12).collect()
    }

    #[test]
    fn phrases_stay_in_key() {
        let in_key = pcs(57, "minor");
        for kind in [PhraseKind::Hit, PhraseKind::Note, PhraseKind::Chord, PhraseKind::Line] {
            let notes = default_phrase(kind, "A", "minor", 3, 120.0);
            assert!(!notes.is_empty());
            assert!(notes.iter().all(|n| in_key.contains(&(n.pitch % 12))), "{kind:?} out of key: {notes:?}");
            assert!(notes.iter().all(|n| (48..72).contains(&n.pitch)), "{kind:?} in octave 3 register: {notes:?}");
        }
        assert_eq!(default_phrase(PhraseKind::Hit, "A", "minor", 3, 120.0)[0].pitch, 57, "root of A in octave 3");
        let chord = default_phrase(PhraseKind::Chord, "A", "minor", 3, 120.0);
        assert_eq!(chord.len(), 3);
        assert!(chord.iter().all(|n| n.start_ms == 0));
        let line = default_phrase(PhraseKind::Line, "A", "minor", 3, 120.0);
        assert_eq!(line.len(), 8);
        assert!(line.windows(2).all(|w| w[1].start_ms - w[0].start_ms == 250), "eighths at 120 bpm");
        // Sharps/flats and unknown keys parse safely.
        assert_eq!(default_phrase(PhraseKind::Hit, "F#", "major", 3, 120.0)[0].pitch, 54);
        assert_eq!(default_phrase(PhraseKind::Hit, "Bb", "major", 3, 120.0)[0].pitch, 58);
        assert_eq!(default_phrase(PhraseKind::Hit, "??", "major", 3, 120.0)[0].pitch, 48, "unknown key → C");
    }

    #[test]
    fn notes_are_validated() {
        let ok = AuditionNote { pitch: 60, velocity: 100, start_ms: 0, dur_ms: 500 };
        assert!(validate_notes(&[ok]).is_ok());
        assert!(validate_notes(&default_phrase(PhraseKind::Line, "C", "major", 4, 90.0)).is_ok());
        assert!(validate_notes(&[]).is_err(), "nothing to play");
        assert!(validate_notes(&[AuditionNote { pitch: 200, ..ok }]).is_err());
        assert!(validate_notes(&[AuditionNote { velocity: 0, ..ok }]).is_err());
        assert!(validate_notes(&[AuditionNote { start_ms: 15_000, dur_ms: 1_001, ..ok }]).is_err(), "ends past 16 s");
        assert!(validate_notes(&[AuditionNote { dur_ms: 0, ..ok }]).is_err());
        assert!(validate_notes(&vec![ok; 65]).is_err(), "too many notes");
    }

    #[test]
    fn phrase_notes_convert_ms_to_ticks_and_bars() {
        let (notes, bars) = phrase_notes(&[AuditionNote { pitch: 60, velocity: 90, start_ms: 500, dur_ms: 500 }], 120.0);
        assert_eq!((notes[0].tick, notes[0].duration, notes[0].pitch, notes[0].vel), (120, 120, 60, 90));
        assert_eq!(bars, 1);
        let (_, long) = phrase_notes(&[AuditionNote { pitch: 60, velocity: 90, start_ms: 0, dur_ms: 4_500 }], 120.0);
        assert_eq!(long, 3, "4.5 s at 120 bpm spans 3 bars");
    }

    #[test]
    fn audition_project_solos_and_converts_time() {
        let p = Project::default_project();
        let notes = [AuditionNote { pitch: 60, velocity: 100, start_ms: 500, dur_ms: 500 }];
        let a = audition_project(&p, "t2", &notes).unwrap();
        assert_eq!(a.tracks.len(), 1);
        assert_eq!(a.tracks[0].id, "t2");
        assert!(!a.tracks[0].muted);
        assert!(a.arrangement.is_empty() && a.audio_clips.is_empty());
        assert_eq!(a.tracks[0].patterns.len(), 1);
        let pid = &a.tracks[0].patterns[0];
        let Some(Pattern::PianoRoll(pr)) = a.patterns.iter().find(|x| x.id() == pid) else { panic!("piano roll") };
        assert_eq!((pr.notes[0].tick, pr.notes[0].duration, pr.notes[0].pitch), (120, 120, 60));
        a.validate().unwrap();
        assert!(audition_project(&p, "nope", &notes).is_err());
        assert!(audition_project(&p, "t2", &[]).is_err());
    }
}

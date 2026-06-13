//! Real-time MIDI → piano-roll recording state.
//!
//! `MidiRecorder` is pure, I/O-free logic: it maps the live playhead tick to a
//! position within the pattern (looping), tracks held notes, and emits finished
//! `Note`s on note-off / stop. Frontends own the device input and the pattern;
//! the recorder only decides *what note, at what tick, for how long*.

use crate::project::Note;
use std::collections::HashMap;

/// Minimum duration (ticks) for a recorded note, so an instant tap is still audible.
const MIN_NOTE_TICKS: u32 = 30;

#[derive(Debug, Default)]
pub struct MidiRecorder {
    /// `playback_tick` value captured when recording started.
    seq_start: u32,
    /// Currently-held notes: pitch -> (pattern on_tick, velocity, global playhead at press).
    pending: HashMap<u8, (u32, u8, u32)>,
    /// Punch window in *global* (song) ticks: notes pressed outside are
    /// ignored; notes held across the end are truncated at it.
    pub punch_window: Option<(u32, u32)>,
}

impl MidiRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a take at `playhead` (the current sequencer tick). Clears any
    /// leftover held notes. The caller clears the target pattern (replace mode).
    pub fn start(&mut self, playhead: u32) {
        self.seq_start = playhead;
        self.pending.clear();
    }

    /// Position within the pattern for a given live `playhead`, looping at `len`.
    pub fn current_tick(&self, playhead: u32, len: u32) -> u32 {
        if len == 0 {
            return 0;
        }
        playhead.wrapping_sub(self.seq_start) % len
    }

    /// Record a key press as a held note at the current pattern tick.
    /// A re-press of an already-held pitch overwrites it (last press wins).
    pub fn note_on(&mut self, pitch: u8, vel: u8, playhead: u32, len: u32) {
        if let Some((s, e)) = self.punch_window {
            if playhead < s || playhead >= e {
                return; // outside the punch window: not recorded
            }
        }
        let tick = self.current_tick(playhead, len);
        self.pending.insert(pitch, (tick, vel, playhead));
    }

    /// Release a held note, returning the finished note (or `None` if `pitch`
    /// wasn't held). Duration wraps across the pattern boundary.
    pub fn note_off(&mut self, pitch: u8, playhead: u32, len: u32) -> Option<Note> {
        let (on_tick, vel, on_playhead) = self.pending.remove(&pitch)?;
        self.finish(pitch, on_tick, vel, on_playhead, playhead, len)
    }

    /// End the take, flushing every still-held note as if released now.
    pub fn stop(&mut self, playhead: u32, len: u32) -> Vec<Note> {
        let held: Vec<(u8, u32, u8, u32)> = self
            .pending
            .drain()
            .map(|(p, (t, v, ph))| (p, t, v, ph))
            .collect();
        held.into_iter()
            .filter_map(|(pitch, on_tick, vel, on_playhead)| {
                self.finish(pitch, on_tick, vel, on_playhead, playhead, len)
            })
            .collect()
    }

    /// Build a finished note from a held press, computing its (loop-aware)
    /// duration. `None` when the punch-out clamp leaves less than the
    /// minimum audible duration.
    fn finish(&self, pitch: u8, on_tick: u32, vel: u8, on_playhead: u32, playhead: u32, len: u32) -> Option<Note> {
        let cur = self.current_tick(playhead, len);
        let raw = if cur >= on_tick {
            cur - on_tick
        } else {
            // released after the pattern looped past the press
            len.saturating_sub(on_tick) + cur
        };
        let mut duration = raw.max(MIN_NOTE_TICKS);
        if let Some((_, e)) = self.punch_window {
            let max_dur = e.saturating_sub(on_playhead);
            if max_dur < MIN_NOTE_TICKS {
                return None; // clamp leaves nothing audible
            }
            duration = duration.min(max_dur);
        }
        Some(Note { pitch, tick: on_tick, duration, vel })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_tick_maps_from_start_and_wraps() {
        let mut r = MidiRecorder::new();
        r.start(1000);
        assert_eq!(r.current_tick(1240, 480), 240); // 240 into the pattern
        assert_eq!(r.current_tick(1500, 480), 20); // 500 % 480
    }

    #[test]
    fn note_on_then_off_yields_note_with_duration() {
        let mut r = MidiRecorder::new();
        r.start(0);
        r.note_on(60, 100, 0, 480);
        let note = r.note_off(60, 120, 480).expect("a finished note");
        assert_eq!(note, Note { pitch: 60, tick: 0, duration: 120, vel: 100 });
    }

    #[test]
    fn duration_wraps_across_pattern_boundary() {
        let mut r = MidiRecorder::new();
        r.start(0);
        r.note_on(64, 90, 460, 480); // pressed near the end
        let note = r.note_off(64, 500, 480).expect("a finished note"); // released after wrap
        // dur = len - on + cur = 480 - 460 + 20 = 40
        assert_eq!(note.tick, 460);
        assert_eq!(note.duration, 40);
    }

    #[test]
    fn note_off_without_matching_on_is_none() {
        let mut r = MidiRecorder::new();
        r.start(0);
        assert!(r.note_off(99, 100, 480).is_none());
    }

    #[test]
    fn stop_flushes_all_held_notes() {
        let mut r = MidiRecorder::new();
        r.start(0);
        r.note_on(60, 100, 0, 480);
        r.note_on(67, 80, 120, 480);
        let mut notes = r.stop(240, 480);
        notes.sort_by_key(|n| n.pitch);
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0], Note { pitch: 60, tick: 0, duration: 240, vel: 100 });
        assert_eq!(notes[1], Note { pitch: 67, tick: 120, duration: 120, vel: 80 });
        // pending cleared: a second stop yields nothing
        assert!(r.stop(300, 480).is_empty());
    }

    #[test]
    fn instant_tap_gets_minimum_duration() {
        let mut r = MidiRecorder::new();
        r.start(0);
        r.note_on(72, 100, 50, 480);
        let note = r.note_off(72, 50, 480).expect("a finished note"); // same tick
        assert_eq!(note.duration, MIN_NOTE_TICKS);
    }

    #[test]
    fn punch_window_filters_and_truncates() {
        let mut r = MidiRecorder::new();
        r.punch_window = Some((480, 960)); // bar 2 of a 2-bar pattern
        r.start(0);
        // Note pressed before the window: ignored entirely.
        r.note_on(60, 100, 100, 960);
        assert!(r.note_off(60, 200, 960).is_none(), "note before punch-in dropped");
        // Note inside the window: kept.
        r.note_on(62, 100, 500, 960);
        let n = r.note_off(62, 700, 960).expect("inside note kept");
        assert_eq!((n.tick, n.duration), (500, 200));
        // Held across punch-out: truncated at the boundary.
        r.note_on(64, 100, 900, 960);
        let n = r.note_off(64, 1100, 960).expect("held note kept");
        assert_eq!(n.tick, 900);
        assert_eq!(n.duration, 60, "clamped at punch-out (960 - 900)");
        // Pressed inside but clamp leaves less than the minimum: dropped.
        r.note_on(65, 100, 955, 960);
        assert!(r.note_off(65, 1100, 960).is_none(), "sub-minimum clamp dropped");
        // After the window: ignored.
        r.note_on(66, 100, 1000, 960);
        assert!(r.note_off(66, 1050, 960).is_none());
    }

    #[test]
    fn len_zero_does_not_panic() {
        let mut r = MidiRecorder::new();
        r.start(0);
        assert_eq!(r.current_tick(100, 0), 0);
        r.note_on(60, 100, 100, 0);
        let _ = r.note_off(60, 100, 0); // must not divide by zero
    }
}

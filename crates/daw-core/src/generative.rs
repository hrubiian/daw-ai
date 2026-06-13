//! Generative & music-theory helpers — ways to enrich patterns.
//!
//! Pure, dependency-free functions used by composition tooling, the TUI, and
//! (live) by an LLM driving the project. Nothing here touches audio or I/O.

use crate::project::Note;

// ── Euclidean rhythms ─────────────────────────────────────────────────────────

/// Distribute `pulses` hits as evenly as possible across `steps` (a Euclidean
/// rhythm), with the first hit on step 0, then rotated left by `rotation`.
/// Returns one `0`/`1` per step — ready to drop into a `StepRow.steps`.
///
/// E(4,16) → four-on-the-floor; E(5,8) → the cinquillo; E(3,8) → tresillo.
pub fn euclidean(pulses: usize, steps: usize, rotation: usize) -> Vec<u8> {
    if steps == 0 { return Vec::new(); }
    let pulses = pulses.min(steps);
    if pulses == 0 { return vec![0; steps]; }

    // Bresenham-style even spread; seed the accumulator so step 0 is a hit.
    let mut out = vec![0u8; steps];
    let mut bucket = (steps - 1) as i64;
    for slot in out.iter_mut() {
        bucket += pulses as i64;
        if bucket >= steps as i64 {
            bucket -= steps as i64;
            *slot = 1;
        }
    }

    if rotation % steps != 0 {
        out.rotate_left(rotation % steps);
    }
    out
}

// ── Scales & chords ───────────────────────────────────────────────────────────

/// Semitone intervals (from the root) for a named scale. Unknown names → major.
pub fn scale_intervals(name: &str) -> &'static [i32] {
    match name.to_ascii_lowercase().as_str() {
        "minor" | "aeolian"   => &[0, 2, 3, 5, 7, 8, 10],
        "harmonic_minor"      => &[0, 2, 3, 5, 7, 8, 11],
        "dorian"              => &[0, 2, 3, 5, 7, 9, 10],
        "phrygian"            => &[0, 1, 3, 5, 7, 8, 10],
        "lydian"              => &[0, 2, 4, 6, 7, 9, 11],
        "mixolydian"          => &[0, 2, 4, 5, 7, 9, 10],
        "minor_pentatonic"    => &[0, 3, 5, 7, 10],
        "major_pentatonic"    => &[0, 2, 4, 7, 9],
        "blues"               => &[0, 3, 5, 6, 7, 10],
        "chromatic"           => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        _ /* major/ionian */  => &[0, 2, 4, 5, 7, 9, 11],
    }
}

/// Semitone intervals for a named chord quality. Unknown names → major triad.
pub fn chord_intervals(quality: &str) -> &'static [i32] {
    match quality.to_ascii_lowercase().as_str() {
        "min" | "m" | "minor" => &[0, 3, 7],
        "maj7"                => &[0, 4, 7, 11],
        "min7" | "m7"         => &[0, 3, 7, 10],
        "dom7" | "7"          => &[0, 4, 7, 10],
        "min9" | "m9"         => &[0, 3, 7, 10, 14],
        "maj9"                => &[0, 4, 7, 11, 14],
        "add9"                => &[0, 4, 7, 14],
        "sus2"                => &[0, 2, 7],
        "sus4"                => &[0, 5, 7],
        "dim"                 => &[0, 3, 6],
        "dim7"                => &[0, 3, 6, 9],
        "m7b5" | "halfdim"    => &[0, 3, 6, 10],
        "aug"                 => &[0, 4, 8],
        "7b9"                 => &[0, 4, 7, 10, 13],
        _ /* maj */           => &[0, 4, 7],
    }
}

/// Pitches of `chord` built on `root` (MIDI), clamped to the valid MIDI range.
pub fn chord_pitches(root: u8, quality: &str) -> Vec<u8> {
    chord_intervals(quality).iter()
        .map(|i| (root as i32 + i).clamp(0, 127) as u8)
        .collect()
}

/// Pitches of one octave of `scale` from `root` (MIDI).
pub fn scale_pitches(root: u8, scale: &str) -> Vec<u8> {
    scale_intervals(scale).iter()
        .map(|i| (root as i32 + i).clamp(0, 127) as u8)
        .collect()
}

/// Snap `pitch` to the nearest tone of `scale` keyed at `root` (any octave).
pub fn snap_to_scale(pitch: u8, root: u8, scale: &str) -> u8 {
    let ivals = scale_intervals(scale);
    let rel = ((pitch as i32 - root as i32) % 12 + 12) % 12;
    let nearest = ivals.iter()
        .min_by_key(|&&i| ((i % 12) - rel).abs())
        .copied()
        .unwrap_or(0);
    (pitch as i32 + ((nearest % 12) - rel)).clamp(0, 127) as u8
}

// ── Arpeggiator ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArpMode { Up, Down, UpDown, Random }

/// Turn a chord (`pitches`) into an arpeggio of `count` notes, one every
/// `step_ticks`, each lasting `step_ticks * gate`, ranging over `octaves`.
/// `Random` is deterministic (seeded by step index) so renders are stable.
pub fn arpeggiate(
    pitches: &[u8],
    mode: ArpMode,
    count: usize,
    step_ticks: u32,
    gate: f32,
    octaves: u8,
    vel: u8,
) -> Vec<Note> {
    if pitches.is_empty() || count == 0 { return Vec::new(); }

    // Expand the chord across the requested octave span.
    let mut pool: Vec<u8> = Vec::new();
    for o in 0..octaves.max(1) {
        for &p in pitches {
            pool.push((p as u32 + o as u32 * 12).min(127) as u8);
        }
    }
    let seq: Vec<u8> = match mode {
        ArpMode::Up   => pool.clone(),
        ArpMode::Down => { let mut v = pool.clone(); v.reverse(); v }
        ArpMode::UpDown => {
            let mut v = pool.clone();
            if pool.len() > 2 { v.extend(pool.iter().rev().skip(1).take(pool.len() - 2)); }
            v
        }
        ArpMode::Random => pool.clone(), // index is randomized per-step below
    };

    let dur = ((step_ticks as f32) * gate).round().max(1.0) as u32;
    (0..count).map(|i| {
        let idx = match mode {
            ArpMode::Random => {
                // deterministic xorshift on the step index
                let mut x = (i as u32).wrapping_mul(2_654_435_761);
                x ^= x >> 15; x = x.wrapping_mul(2_246_822_519); x ^= x >> 13;
                (x as usize) % seq.len()
            }
            _ => i % seq.len(),
        };
        Note { pitch: seq[idx], tick: i as u32 * step_ticks, duration: dur, vel }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn euclid_known_patterns() {
        // four-on-the-floor
        assert_eq!(euclidean(4, 16, 0), vec![1,0,0,0, 1,0,0,0, 1,0,0,0, 1,0,0,0]);
        // correct count of pulses
        assert_eq!(euclidean(5, 8, 0).iter().filter(|&&x| x == 1).count(), 5);
        assert_eq!(euclidean(3, 8, 0).iter().filter(|&&x| x == 1).count(), 3);
        // step 0 is a hit; rotation shifts it
        assert_eq!(euclidean(4, 16, 0)[0], 1);
        assert_eq!(euclidean(4, 16, 1)[0], 0);
        // edge cases
        assert_eq!(euclidean(0, 8, 0), vec![0; 8]);
        assert_eq!(euclidean(9, 8, 0).iter().filter(|&&x| x == 1).count(), 8);
    }

    #[test]
    fn snap_keeps_in_scale() {
        // C minor: Eb(63) stays, E(64) snaps to Eb(63) or D# ... nearest scale tone
        for p in 60..=72u8 {
            let s = snap_to_scale(p, 60, "minor");
            let rel = ((s as i32 - 60) % 12 + 12) % 12;
            assert!(scale_intervals("minor").contains(&rel), "pitch {p} -> {s} not in scale");
        }
    }

    #[test]
    fn chord_and_arp() {
        assert_eq!(chord_pitches(60, "min7"), vec![60, 63, 67, 70]);
        let arp = arpeggiate(&[60, 64, 67], ArpMode::Up, 6, 120, 0.5, 1, 90);
        assert_eq!(arp.len(), 6);
        assert_eq!(arp[0].pitch, 60);
        assert_eq!(arp[1].pitch, 64);
        assert_eq!(arp[3].pitch, 60); // wraps
        assert_eq!(arp[1].tick, 120);
        assert_eq!(arp[0].duration, 60);
    }
}

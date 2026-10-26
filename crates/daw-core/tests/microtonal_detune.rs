// A per-track `tuning_cents` must bend the rendered audio off the 12-TET grid.
// Renders a sustained A4 (440 Hz) flute note at 0 / +50 / +100 cents and
// estimates the fundamental by autocorrelation. The +50-cent case is the point:
// its pitch lands strictly BETWEEN two adjacent MIDI notes — unreachable by any
// integer pitch, i.e. genuinely microtonal.
use daw_core::audio::export::render_project_samples;
use daw_core::project::Project;
use std::path::Path;

fn project_json(cents: f32) -> String {
    format!(
        r#"{{
      "version": "0.1",
      "project": {{ "name": "t", "bpm": 120.0, "time_signature": [4,4],
                    "key": "A", "scale": "minor", "swing": 0.0,
                    "metronome": false, "humanize": 0.0 }},
      "tracks": [{{ "id": "t1", "name": "v", "type": "melodic",
                    "instrument": {{ "type": "soundfont", "preset": 73, "bank": 0 }},
                    "volume": 1.0, "pan": 0.0, "muted": false,
                    "patterns": ["p1"], "tuning_cents": {cents} }}],
      "patterns": [{{ "type": "piano_roll", "id": "p1", "name": "n", "bars": 2,
                      "ticks_per_bar": 480,
                      "notes": [{{ "pitch": 69, "tick": 0, "duration": 900, "vel": 110 }}] }}],
      "arrangement": [{{ "track": "t1", "pattern": "p1", "bar": 0 }}]
    }}"#
    )
}

/// Autocorrelation pitch estimate (with parabolic interpolation) over a window
/// in the note's steady state. Lag search brackets ~315-630 Hz to avoid octave
/// errors around 440-466 Hz.
fn detect_hz(left: &[f32], sr: u32) -> f32 {
    let start = (sr as usize) / 2; // 0.5 s in — past the attack
    let win = (sr as usize); // 1 s window
    let sig = &left[start..(start + win).min(left.len())];
    let mean = sig.iter().sum::<f32>() / sig.len() as f32;
    let sig: Vec<f32> = sig.iter().map(|s| s - mean).collect();

    let (min_lag, max_lag) = (70usize, 140usize);
    let mut ac = vec![0f32; max_lag + 2];
    let (mut best, mut best_val) = (min_lag, f32::MIN);
    for lag in min_lag..=max_lag {
        let mut s = 0f32;
        for i in 0..(sig.len() - lag) {
            s += sig[i] * sig[i + lag];
        }
        ac[lag] = s;
        if s > best_val {
            best_val = s;
            best = lag;
        }
    }
    let (a, b, c) = (ac[best - 1], ac[best], ac[best + 1]);
    let denom = a - 2.0 * b + c;
    let shift = if denom.abs() > 1e-9 { 0.5 * (a - c) / denom } else { 0.0 };
    sr as f32 / (best as f32 + shift)
}

fn render_hz(cents: f32) -> f32 {
    let sf2 = Path::new("../../assets/GeneralUser.sf2");
    let proj: Project = serde_json::from_str(&project_json(cents)).expect("parse");
    let (samples, sr) = render_project_samples(&proj, sf2, false).expect("render");
    let left: Vec<f32> = samples.chunks_exact(2).map(|c| c[0]).collect();
    detect_hz(&left, sr)
}

#[test]
#[ignore] // needs assets/GeneralUser.sf2; run with --ignored
fn tuning_cents_detunes_between_the_keys() {
    let f0 = render_hz(0.0);
    let f50 = render_hz(50.0);
    let f100 = render_hz(100.0);
    eprintln!("0c={f0:.1}Hz  +50c={f50:.1}Hz  +100c={f100:.1}Hz");

    // A4 == 440 Hz; adjacent MIDI notes are 440 and 466.16 Hz (A#4).
    assert!((f0 - 440.0).abs() < 6.0, "0 cents should be ~440 Hz, got {f0}");
    // +100 cents == exactly one semitone up.
    assert!((f100 - 466.16).abs() < 7.0, "+100 cents should be ~466 Hz, got {f100}");
    // The crux: +50 cents is a quarter-tone — strictly between 440 and 466,
    // a pitch NO integer MIDI note can produce.
    assert!(
        f50 > 445.0 && f50 < 461.0,
        "+50 cents must land between the keys (445-461 Hz), got {f50}"
    );
}

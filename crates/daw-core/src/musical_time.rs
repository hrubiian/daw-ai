//! Shared timing math for meters and tempo maps.
use serde::{Deserialize, Serialize};

pub const TICKS_PER_BEAT: u32 = 120;
pub const BEATS_PER_BAR: u32 = 4;
pub const TICKS_PER_BAR: u32 = TICKS_PER_BEAT * BEATS_PER_BAR;
pub const MAX_TEMPO_POINTS: usize = 256;
pub const MIN_BPM: f64 = 20.0;
pub const MAX_BPM: f64 = 400.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TempoPoint {
    pub tick: u32,
    pub bpm: f64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ramp: bool,
}

pub fn is_valid_time_signature(ts: [u8; 2]) -> bool {
    (1..=16).contains(&ts[0]) && matches!(ts[1], 2 | 4 | 8 | 16)
}

pub fn ticks_per_unit(ts: [u8; 2]) -> u32 {
    match ts[1] {
        2 => 240,
        4 => 120,
        8 => 60,
        16 => 30,
        _ => 120,
    }
}

pub fn ticks_per_bar_for(ts: [u8; 2]) -> u32 {
    u32::from(ts[0].max(1)) * ticks_per_unit(ts)
}

pub fn steps_per_bar_for(ts: [u8; 2]) -> u32 {
    (ticks_per_bar_for(ts) / (TICKS_PER_BEAT / 4)).max(1)
}

pub fn beats_per_bar_for(ts: [u8; 2]) -> u32 {
    u32::from(ts[0].max(1))
}

pub fn seconds_per_bar(bpm: f64) -> f64 {
    60.0 * BEATS_PER_BAR as f64 / bpm
}

pub fn seconds_per_bar_for(bpm: f64, ts: [u8; 2]) -> f64 {
    (60.0 / bpm.max(1.0)) * (f64::from(ticks_per_bar_for(ts)) / f64::from(TICKS_PER_BEAT))
}

pub fn bar_sample(bar: u32, bpm: f64, sample_rate: u32) -> u64 {
    (bar as f64 * seconds_per_bar(bpm) * sample_rate as f64).round() as u64
}

pub fn bar_sample_for(bar: u32, bpm: f64, ts: [u8; 2], sample_rate: u32) -> u64 {
    (bar as f64 * seconds_per_bar_for(bpm, ts) * sample_rate as f64).round() as u64
}

fn clamp_bpm(bpm: f64) -> f64 {
    if bpm.is_finite() {
        bpm.clamp(MIN_BPM, MAX_BPM)
    } else {
        120.0
    }
}

pub fn bpm_at_tick(base_bpm: f64, points: &[TempoPoint], tick: u32) -> f64 {
    let mut cur_bpm = clamp_bpm(base_bpm);
    let mut cur_tick = 0u32;
    if points.is_empty() {
        return cur_bpm;
    }
    for pt in points {
        let pt_bpm = clamp_bpm(pt.bpm);
        if pt.tick == 0 {
            cur_bpm = pt_bpm;
            cur_tick = 0;
            continue;
        }
        if tick < pt.tick {
            if pt.ramp && pt.tick > cur_tick {
                let frac = f64::from(tick - cur_tick) / f64::from(pt.tick - cur_tick);
                return cur_bpm + (pt_bpm - cur_bpm) * frac;
            }
            return cur_bpm;
        }
        cur_bpm = pt_bpm;
        cur_tick = pt.tick;
        if tick == pt.tick {
            return cur_bpm;
        }
    }
    cur_bpm
}

fn segment_duration(du: u32, b0: f64, b1: f64, ramp: bool) -> f64 {
    if du == 0 {
        return 0.0;
    }
    let k = 60.0 / f64::from(TICKS_PER_BEAT);
    if !ramp || (b1 - b0).abs() <= 1e-9 {
        f64::from(du) * k / b0
    } else {
        k * (f64::from(du) / (b1 - b0)) * (b1 / b0).ln()
    }
}

pub fn tick_to_seconds(base_bpm: f64, points: &[TempoPoint], tick: u32) -> f64 {
    let mut cur_bpm = clamp_bpm(base_bpm);
    let k = 60.0 / f64::from(TICKS_PER_BEAT);
    if points.is_empty() {
        return f64::from(tick) * k / cur_bpm;
    }
    let mut cur_tick = 0u32;
    let mut secs = 0.0f64;
    for pt in points {
        let pt_bpm = clamp_bpm(pt.bpm);
        if pt.tick == 0 {
            cur_bpm = pt_bpm;
            continue;
        }
        if tick <= pt.tick {
            let du = tick.saturating_sub(cur_tick);
            if pt.ramp && pt.tick > cur_tick {
                let end_bpm = bpm_at_tick(base_bpm, points, tick);
                secs += segment_duration(du, cur_bpm, end_bpm, true);
            } else {
                secs += segment_duration(du, cur_bpm, cur_bpm, false);
            }
            return secs;
        }
        let du = pt.tick.saturating_sub(cur_tick);
        secs += segment_duration(du, cur_bpm, pt_bpm, pt.ramp);
        cur_tick = pt.tick;
        cur_bpm = pt_bpm;
    }
    secs + segment_duration(tick.saturating_sub(cur_tick), cur_bpm, cur_bpm, false)
}

pub fn seconds_to_tick(base_bpm: f64, points: &[TempoPoint], sec: f64) -> u32 {
    if sec <= 0.0 || !sec.is_finite() {
        return 0;
    }
    let mut cur_bpm = clamp_bpm(base_bpm);
    let k = 60.0 / f64::from(TICKS_PER_BEAT);
    if points.is_empty() {
        return (sec * cur_bpm / k).round() as u32;
    }
    let mut cur_tick = 0u32;
    let mut rem = sec;
    for pt in points {
        let pt_bpm = clamp_bpm(pt.bpm);
        if pt.tick == 0 {
            cur_bpm = pt_bpm;
            continue;
        }
        let du = pt.tick.saturating_sub(cur_tick);
        let seg_sec = segment_duration(du, cur_bpm, pt_bpm, pt.ramp);
        if rem <= seg_sec {
            let offset = if !pt.ramp || (pt_bpm - cur_bpm).abs() <= 1e-9 || du == 0 {
                (rem * cur_bpm / k).round() as u32
            } else {
                let bt = cur_bpm * (rem * (pt_bpm - cur_bpm) / (k * f64::from(du))).exp();
                (f64::from(du) * (bt - cur_bpm) / (pt_bpm - cur_bpm)).round() as u32
            };
            return cur_tick.saturating_add(offset.min(du));
        }
        rem -= seg_sec;
        cur_tick = pt.tick;
        cur_bpm = pt_bpm;
    }
    cur_tick.saturating_add((rem * cur_bpm / k).round() as u32)
}

pub fn bar_sample_map(
    bar: u32,
    base_bpm: f64,
    ts: [u8; 2],
    points: &[TempoPoint],
    sample_rate: u32,
) -> u64 {
    let tick = bar.saturating_mul(ticks_per_bar_for(ts));
    (tick_to_seconds(base_bpm, points, tick) * f64::from(sample_rate)).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_math_covers_odd_and_compound_signatures() {
        assert!(is_valid_time_signature([4, 4]));
        assert!(is_valid_time_signature([3, 4]));
        assert!(is_valid_time_signature([5, 4]));
        assert!(is_valid_time_signature([7, 4]));
        assert!(is_valid_time_signature([6, 8]));
        assert!(!is_valid_time_signature([0, 4]));
        assert!(!is_valid_time_signature([4, 3]));
        assert!(!is_valid_time_signature([17, 4]));

        assert_eq!(ticks_per_bar_for([4, 4]), 480);
        assert_eq!(ticks_per_bar_for([3, 4]), 360);
        assert_eq!(ticks_per_bar_for([5, 4]), 600);
        assert_eq!(ticks_per_bar_for([7, 4]), 840);
        assert_eq!(ticks_per_bar_for([6, 8]), 360);
        assert_eq!(steps_per_bar_for([7, 4]), 28);
        assert_eq!(beats_per_bar_for([7, 4]), 7);
        assert!((seconds_per_bar_for(120.0, [7, 4]) - 3.5).abs() < 1e-9);
        assert!((seconds_per_bar_for(120.0, [6, 8]) - 1.5).abs() < 1e-9);
    }

    #[test]
    fn tempo_map_step_and_ramp_round_trip() {
        let pts = [
            TempoPoint { tick: 480, bpm: 180.0, ramp: false },
            TempoPoint { tick: 960, bpm: 240.0, ramp: true },
        ];
        assert_eq!(bpm_at_tick(120.0, &pts, 0), 120.0);
        assert_eq!(bpm_at_tick(120.0, &pts, 479), 120.0);
        assert_eq!(bpm_at_tick(120.0, &pts, 480), 180.0);
        assert!((bpm_at_tick(120.0, &pts, 720) - 210.0).abs() < 1e-9);
        assert_eq!(bpm_at_tick(120.0, &pts, 960), 240.0);
        assert_eq!(bpm_at_tick(120.0, &pts, 1440), 240.0);

        for tick in [0u32, 120, 480, 600, 720, 960, 1200] {
            let sec = tick_to_seconds(120.0, &pts, tick);
            let back = seconds_to_tick(120.0, &pts, sec);
            assert!(
                back.abs_diff(tick) <= 1,
                "round-trip failed for tick {tick}: sec={sec}, back={back}"
            );
        }

        // Exact analytical integral check: 480 ticks at 120 BPM = 2.0s;
        // 480 ticks ramping 180->240 BPM = 0.5 * (480/60) * ln(240/180) = 4 * ln(4/3).
        let expected_at_960 = 2.0 + 4.0 * (240.0f64 / 180.0).ln();
        assert!((tick_to_seconds(120.0, &pts, 960) - expected_at_960).abs() < 1e-9);

        // Uptempo ceiling up to MAX_BPM (400.0 BPM): 4 beats at 400 BPM = 0.6s.
        let pts_fast = [TempoPoint { tick: 480, bpm: 400.0, ramp: false }];
        assert_eq!(bpm_at_tick(320.0, &pts_fast, 0), 320.0);
        assert_eq!(bpm_at_tick(320.0, &pts_fast, 480), 400.0);
        assert_eq!(bpm_at_tick(450.0, &[], 0), MAX_BPM);
        assert!((tick_to_seconds(400.0, &[], 480) - 0.6).abs() < 1e-9);
    }
}

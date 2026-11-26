use crate::musical_time::TICKS_PER_BEAT;
use serde::{Deserialize, Serialize};

pub const MAX_VOLUME_POINTS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VolumePoint {
    /// Absolute arrangement tick at 480 ticks per bar.
    pub tick: u32,
    /// Linear multiplier of the fader, from 0 (silence) to 2 (+6 dB).
    pub gain: f32,
}

const UNITY: VolumePoint = VolumePoint { tick: 0, gain: 1.0 };

impl VolumePoint {
    /// Gain at the requested tick and the endpoint of the current transition.
    pub fn segment_at(points: &[Self], tick: u32) -> (f32, Option<Self>) {
        match Self::curve_at(points, tick) {
            Some(segment) => (segment.evaluate(tick).0 as f32, Some(segment.end)),
            None => (points.last().unwrap_or(&UNITY).gain, None),
        }
    }

    /// Curve between the neighboring points. A first point after tick zero
    /// approaches from unity. After the last point the gain is held.
    pub fn curve_at(points: &[Self], tick: u32) -> Option<VolumeSegment> {
        let next = points.partition_point(|point| point.tick <= tick);
        let end = *points.get(next)?;
        let start = next.checked_sub(1).map_or(UNITY, |i| points[i]);
        let start_slope = if start.tick == 0 {
            0.0
        } else {
            let previous = next.checked_sub(2).map_or(UNITY, |i| points[i]);
            tangent(previous, start, end)
        };
        let end_slope = points
            .get(next + 1)
            .map_or(0.0, |following| tangent(start, end, *following));
        let span = f64::from(end.tick - start.tick) / 3.0;
        Some(VolumeSegment {
            start,
            end,
            controls: [
                f64::from(start.gain),
                f64::from(start.gain) + start_slope * span,
                f64::from(end.gain) - end_slope * span,
                f64::from(end.gain),
            ],
        })
    }

    /// Share the same automation sampling cadence between live and offline.
    /// Aim for 20 ms between updates so the mixer glide can bridge them.
    /// Always emit authored points, even when they fall between updates.
    pub(crate) fn scheduled_gain(points: &[Self], tick: u32, bpm: f64) -> Option<f32> {
        if tick > points.last()?.tick {
            return None;
        }
        let interval = (bpm * f64::from(TICKS_PER_BEAT) / 60.0 * 0.020)
            .round()
            .max(1.0) as u32;
        if tick.is_multiple_of(interval) {
            Some(Self::segment_at(points, tick).0)
        } else {
            points
                .binary_search_by_key(&tick, |point| point.tick)
                .ok()
                .map(|i| points[i].gain)
        }
    }
}

/// A shape-preserving cubic whose time axis is linear.
pub struct VolumeSegment {
    start: VolumePoint,
    pub end: VolumePoint,
    controls: [f64; 4],
}

impl VolumeSegment {
    fn evaluate(&self, tick: u32) -> (f64, f64) {
        let span = f64::from(self.end.tick - self.start.tick);
        let progress = f64::from(tick - self.start.tick) / span;
        let lerp = |a: f64, b: f64| a + (b - a) * progress;
        let [a, b, c, d] = self.controls;
        let ab = lerp(a, b);
        let bc = lerp(b, c);
        let cd = lerp(c, d);
        let left = lerp(ab, bc);
        let right = lerp(bc, cd);
        (lerp(left, right), 3.0 * (right - left) / span)
    }

    /// Bézier gains for the part of the segment in `start..=end`, including
    /// both endpoints. The two controls lie at one and two thirds of that span.
    pub fn bezier_gains(&self, start: u32, end: u32) -> [f32; 4] {
        debug_assert!(self.start.tick <= start && start <= end && end <= self.end.tick);
        let (a, slope_a) = self.evaluate(start);
        let (b, slope_b) = self.evaluate(end);
        let span = f64::from(end - start) / 3.0;
        [
            a as f32,
            (a + slope_a * span) as f32,
            (b - slope_b * span) as f32,
            b as f32,
        ]
    }
}

/// PCHIP tangent. A weighted harmonic mean carries the slope across points
/// on a rising or falling run. A reversal or a flat segment has zero tangent.
fn tangent(previous: VolumePoint, point: VolumePoint, next: VolumePoint) -> f64 {
    let left = f64::from(point.tick - previous.tick);
    let right = f64::from(next.tick - point.tick);
    let rise_left = f64::from(point.gain) - f64::from(previous.gain);
    let rise_right = f64::from(next.gain) - f64::from(point.gain);
    if rise_left * rise_right <= 0.0 {
        return 0.0;
    }
    let weight_left = 2.0 * right + left;
    let weight_right = right + 2.0 * left;
    (weight_left + weight_right)
        / (weight_left * left / rise_left + weight_right * right / rise_right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_from_unity_and_holds_the_last_gain() {
        let points = [VolumePoint {
            tick: 100,
            gain: 0.0,
        }];
        assert_eq!(VolumePoint::segment_at(&[], 25), (1.0, None));
        assert_eq!(VolumePoint::segment_at(&points, 0), (1.0, Some(points[0])));
        assert_eq!(VolumePoint::segment_at(&points, 25).0, 0.84375);
        assert_eq!(VolumePoint::segment_at(&points, 75).0, 0.15625);
        assert_eq!(VolumePoint::segment_at(&points, 100), (0.0, None));
        assert_eq!(VolumePoint::segment_at(&points, 300), (0.0, None));
    }

    #[test]
    fn uneven_points_keep_their_levels_and_direction() {
        let points = [
            VolumePoint { tick: 0, gain: 0.0 },
            VolumePoint { tick: 3, gain: 0.6 },
            VolumePoint {
                tick: 103,
                gain: 0.7,
            },
            VolumePoint {
                tick: 133,
                gain: 2.0,
            },
            VolumePoint {
                tick: 260,
                gain: 0.25,
            },
        ];
        for pair in points.windows(2) {
            let [start, end] = [pair[0], pair[1]];
            let mut previous = start.gain;
            for tick in start.tick + 1..=end.tick {
                let gain = VolumePoint::segment_at(&points, tick).0;
                if start.gain < end.gain {
                    assert!((previous..=end.gain).contains(&gain));
                } else {
                    assert!((end.gain..=previous).contains(&gain));
                }
                previous = gain;
            }
            assert_eq!(previous, end.gain);
        }
        let incoming = VolumePoint::curve_at(&points, 0)
            .unwrap()
            .bezier_gains(0, 3);
        let outgoing = VolumePoint::curve_at(&points, 3)
            .unwrap()
            .bezier_gains(3, 103);
        assert!(incoming[3] > incoming[2] && outgoing[1] > outgoing[0]);
    }

    #[test]
    fn clipped_curve_matches_gain_evaluation() {
        let points = [
            VolumePoint { tick: 0, gain: 0.1 },
            VolumePoint {
                tick: 100,
                gain: 1.3,
            },
        ];
        let segment = VolumePoint::curve_at(&points, 30).unwrap();
        let [a, b, c, d] = segment.bezier_gains(30, 70);
        let middle = (a + 3.0 * b + 3.0 * c + d) / 8.0;
        assert!((middle - VolumePoint::segment_at(&points, 50).0).abs() < 1e-6);
    }

    #[test]
    fn updates_include_off_grid_points_and_stop_after_the_hold() {
        let points = [VolumePoint {
            tick: 17,
            gain: 0.5,
        }];
        assert!(VolumePoint::scheduled_gain(&points, 1, 20.0).is_some());
        assert_eq!(VolumePoint::scheduled_gain(&points, 1, 300.0), None);
        assert!(VolumePoint::scheduled_gain(&points, 12, 300.0).is_some());
        assert_eq!(VolumePoint::scheduled_gain(&points, 17, 300.0), Some(0.5));
        assert_eq!(VolumePoint::scheduled_gain(&points, 18, 20.0), None);
    }
}

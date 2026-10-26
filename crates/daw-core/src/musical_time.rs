//! Shared 4/4 timing. Other meters are rejected by project validation.
pub const TICKS_PER_BEAT: u32 = 120;
pub const BEATS_PER_BAR: u32 = 4;
pub const TICKS_PER_BAR: u32 = TICKS_PER_BEAT * BEATS_PER_BAR;
pub fn seconds_per_bar(bpm: f64) -> f64 {
    60.0 * BEATS_PER_BAR as f64 / bpm
}
pub fn bar_sample(bar: u32, bpm: f64, sample_rate: u32) -> u64 {
    (bar as f64 * seconds_per_bar(bpm) * sample_rate as f64).round() as u64
}

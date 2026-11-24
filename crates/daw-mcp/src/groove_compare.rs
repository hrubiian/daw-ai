//! Compare scheduled note attacks in the arrangement without rendering audio.

use anyhow::{ensure, Context, Result};
use daw_core::project::{Pattern, Project, TrackType};
use daw_core::sequencer::engine::{
    arranged_track_patterns, note_fire_tick, pattern_length_ticks, swung_step_tick, TICKS_PER_BAR,
    TICKS_PER_BEAT,
};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;

const MAX_BARS: u32 = 32;
const MAX_DETAILS_PER_BAR: usize = 16;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema, Serialize)]
pub struct OnsetSource {
    pub track_id: String,
    /// Optional MIDI pitch, e.g. 36 for kick in a drum pattern.
    pub pitch: Option<u8>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GrooveCompareArgs {
    pub reference: OnsetSource,
    pub target: OnsetSource,
    /// First arrangement bar (zero based).
    pub start_bar: u32,
    /// End bar (exclusive). Compare at most 32 bars per request.
    pub end_bar: u32,
    /// Count a hit as aligned within this many milliseconds, 0–1000. Default 40.
    pub tolerance_ms: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct GrooveComparison {
    reference: OnsetSource,
    target: OnsetSource,
    bpm: f64,
    tolerance_ms: f64,
    search_window_ticks: u32,
    total: OnsetCounts,
    bars: Vec<BarComparison>,
}

#[derive(Debug, Default, Serialize)]
struct OnsetCounts {
    reference_onsets: usize,
    target_onsets: usize,
    exact: usize,
    within_tolerance: usize,
    no_reference_in_window: usize,
}

#[derive(Debug, Serialize)]
struct BarComparison {
    bar: u32,
    counts: OnsetCounts,
    offsets: Vec<Offset>,
    omitted_offsets: usize,
}

#[derive(Debug, Serialize)]
struct Offset {
    tick_in_bar: u32,
    nearest_delta_ticks: Option<i64>,
    nearest_delta_ms: Option<f64>,
    // Positive when the target follows the reference, negative when it leads.
    previous_delta_ms: Option<f64>,
    next_delta_ms: Option<f64>,
}

pub fn compare(project: &Project, args: GrooveCompareArgs) -> Result<GrooveComparison> {
    let GrooveCompareArgs {
        reference,
        target,
        start_bar,
        end_bar,
        tolerance_ms,
    } = args;
    ensure!(
        !project.arrangement.is_empty(),
        "place pattern clips on the arrangement before comparing groove"
    );
    ensure!(
        start_bar < end_bar && end_bar - start_bar <= MAX_BARS,
        "compare 1–{MAX_BARS} bars with start_bar < end_bar"
    );
    let end = end_bar
        .checked_mul(TICKS_PER_BAR)
        .context("bar position is too large")?;
    let start = start_bar * TICKS_PER_BAR;
    let tolerance_ms = tolerance_ms.unwrap_or(40.0);
    ensure!(
        (0.0..=1000.0).contains(&tolerance_ms),
        "tolerance_ms must be between 0 and 1000"
    );
    let bpm = project.project.bpm;
    let ms_per_tick = 60_000.0 / bpm / f64::from(TICKS_PER_BEAT);
    let window = TICKS_PER_BEAT.max((tolerance_ms / ms_per_tick).ceil() as u32);
    let anchors = arranged_onsets(
        project,
        &reference,
        start.saturating_sub(window)..end.saturating_add(window),
    )?;
    let hits = arranged_onsets(project, &target, start..end)?;

    let mut total = OnsetCounts::default();
    let mut bars = Vec::new();
    for bar in start_bar..end_bar {
        let bar_start = bar * TICKS_PER_BAR;
        let bar_end = bar_start + TICKS_PER_BAR;
        let reference_onsets = in_range(&anchors, bar_start..bar_end).len();
        let bar_hits = in_range(&hits, bar_start..bar_end);
        let mut counts = OnsetCounts {
            reference_onsets,
            target_onsets: bar_hits.len(),
            ..Default::default()
        };
        let mut offsets = Vec::new();
        for &hit in bar_hits {
            let before = anchors.partition_point(|&anchor| anchor <= hit);
            let after = anchors.partition_point(|&anchor| anchor < hit);
            let delta = |anchor: &u32| i64::from(hit) - i64::from(*anchor);
            let nearby = |delta: &i64| delta.unsigned_abs() <= u64::from(window);
            let previous = before
                .checked_sub(1)
                .map(|i| delta(&anchors[i]))
                .filter(nearby);
            let next = anchors.get(after).map(delta).filter(nearby);
            let nearest = previous.into_iter().chain(next).min_by_key(|d| d.abs());
            if let Some(ticks) = nearest {
                counts.exact += usize::from(ticks == 0);
                counts.within_tolerance +=
                    usize::from(ticks.abs() as f64 * ms_per_tick <= tolerance_ms);
            } else {
                counts.no_reference_in_window += 1;
            }
            if offsets.len() < MAX_DETAILS_PER_BAR {
                let as_ms = |ticks| (ticks as f64 * ms_per_tick * 10.0).round() / 10.0;
                offsets.push(Offset {
                    tick_in_bar: hit - bar_start,
                    nearest_delta_ticks: nearest,
                    nearest_delta_ms: nearest.map(as_ms),
                    previous_delta_ms: previous.map(as_ms),
                    next_delta_ms: next.map(as_ms),
                });
            }
        }
        total.reference_onsets += counts.reference_onsets;
        total.target_onsets += counts.target_onsets;
        total.exact += counts.exact;
        total.within_tolerance += counts.within_tolerance;
        total.no_reference_in_window += counts.no_reference_in_window;
        bars.push(BarComparison {
            bar,
            counts,
            omitted_offsets: bar_hits.len() - offsets.len(),
            offsets,
        });
    }
    Ok(GrooveComparison {
        reference,
        target,
        bpm,
        tolerance_ms,
        search_window_ticks: window,
        total,
        bars,
    })
}

fn in_range(onsets: &[u32], range: Range<u32>) -> &[u32] {
    let start = onsets.partition_point(|&tick| tick < range.start);
    let end = onsets.partition_point(|&tick| tick < range.end);
    &onsets[start..end]
}

fn arranged_onsets(project: &Project, source: &OnsetSource, range: Range<u32>) -> Result<Vec<u32>> {
    ensure!(
        source.pitch.is_none_or(|pitch| pitch <= 127),
        "MIDI pitch must be 0–127"
    );
    let (strip, track) = project
        .tracks
        .iter()
        .enumerate()
        .find(|(_, track)| track.id == source.track_id)
        .with_context(|| format!("no track '{}'", source.track_id))?;
    ensure!(
        track.track_type != TrackType::Audio,
        "track '{}' is audio; compare_groove reads note events",
        track.id
    );
    ensure!(!track.muted, "track '{}' is muted", track.id);
    let mut placements = arranged_track_patterns(project, &track.id);
    placements.retain(|clip| clip.start < range.end && clip.end > range.start);
    let mut notes_by_pattern = HashMap::new();
    for clip in &placements {
        notes_by_pattern
            .entry(clip.pattern.id())
            .or_insert_with(|| pattern_onsets(project, clip.pattern, strip as u8, source.pitch));
    }
    Ok(range
        .filter(|&tick| {
            placements
                .iter()
                .find_map(|clip| clip.local_tick_at(tick).map(|local| (clip.pattern, local)))
                .is_some_and(|(pattern, local)| notes_by_pattern[pattern.id()].contains(&local))
        })
        .collect())
}

fn pattern_onsets(
    project: &Project,
    pattern: &Pattern,
    strip: u8,
    pitch: Option<u8>,
) -> BTreeSet<u32> {
    let mut hits = BTreeSet::new();
    let length = pattern_length_ticks(pattern);
    let mut hit = |note_pitch, nominal| {
        if pitch.is_none_or(|pitch| pitch == note_pitch) {
            hits.insert(note_fire_tick(
                note_pitch,
                nominal,
                strip,
                project.project.timing,
                length,
            ));
        }
    };
    match pattern {
        Pattern::PianoRoll(pattern) => {
            for note in pattern.active_notes() {
                if note.vel > 0 {
                    hit(note.pitch, note.tick);
                }
            }
        }
        Pattern::Step(pattern) => {
            for row in &pattern.rows {
                if pitch.is_some_and(|pitch| pitch != row.pitch) {
                    continue;
                }
                for step in 0..usize::from(pattern.steps).min(row.steps.len()) {
                    if row.step_velocity(step).is_some_and(|velocity| velocity > 0) {
                        hit(
                            row.pitch,
                            swung_step_tick(step as u32, project.project.swing),
                        );
                    }
                }
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compares_offset_repeats_and_neighboring_bars_with_playback_clip_priority() {
        let project: Project = serde_json::from_value(json!({
            "version": "0.1",
            "project": {"name": "Timing fixture", "bpm": 120., "time_signature": [4, 4], "key": "C", "scale": "major"},
            "tracks": [
                {"id": "kit", "name": "Kit", "type": "drum", "instrument": {"type": "soundfont", "bank": 128, "preset": 0}, "volume": 1., "pan": 0., "muted": false, "patterns": ["kick", "shadow"]},
                {"id": "bass", "name": "Bass", "type": "melodic", "instrument": {"type": "soundfont", "bank": 0, "preset": 32}, "volume": 1., "pan": 0., "muted": false, "patterns": ["bass_pattern"]}
            ],
            "patterns": [
                {"id": "kick", "name": "Kick", "type": "step", "bars": 1, "steps": 4, "rows": [
                    {"name": "Kick", "pitch": 36, "vel": 90, "steps": [1, 0, 0, 0]},
                    {"name": "Rim", "pitch": 37, "vel": 80, "steps": [0, 0, 0, 1]}
                ]},
                {"id": "shadow", "name": "Overlapping kick", "type": "step", "bars": 1, "steps": 4, "rows": [
                    {"name": "Kick", "pitch": 36, "vel": 90, "steps": [1, 0, 0, 0]}
                ]},
                {"id": "bass_pattern", "name": "Bass", "type": "piano_roll", "bars": 2, "ticks_per_bar": 480, "notes": [
                    {"pitch": 40, "tick": 70, "duration": 20, "vel": 80},
                    {"pitch": 52, "tick": 70, "duration": 20, "vel": 80},
                    {"pitch": 40, "tick": 170, "duration": 20, "vel": 80},
                    {"pitch": 40, "tick": 480, "duration": 20, "vel": 80}
                ]}
            ],
            "arrangement": [
                {"track": "kit", "pattern": "shadow", "bar": 0, "offset_ticks": 120, "len_bars": 2},
                {"track": "kit", "pattern": "kick", "bar": 0, "offset_ticks": 60, "len_bars": 2},
                {"track": "bass", "pattern": "bass_pattern", "bar": 0}
            ]
        })).unwrap();
        project.validate().unwrap();
        let report = compare(
            &project,
            GrooveCompareArgs {
                reference: OnsetSource {
                    track_id: "kit".into(),
                    pitch: Some(36),
                },
                target: OnsetSource {
                    track_id: "bass".into(),
                    pitch: None,
                },
                start_bar: 0,
                end_bar: 2,
                tolerance_ms: Some(50.0),
            },
        )
        .unwrap();
        assert_eq!(report.total.target_onsets, 3);
        assert_eq!(report.total.reference_onsets, 8);
        assert_eq!(report.total.within_tolerance, 2);
        assert_eq!(report.bars[0].offsets[0].nearest_delta_ticks, Some(10));
        assert_eq!(report.bars[0].offsets[1].nearest_delta_ticks, Some(-10));
        assert_eq!(report.bars[1].offsets[0].previous_delta_ms, Some(250.0));
        assert_eq!(report.bars[1].offsets[0].next_delta_ms, Some(-250.0));
    }
}

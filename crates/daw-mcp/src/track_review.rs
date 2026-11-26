//! Per-track listening on an immutable snapshot of the open or saved song.

use crate::{analysis, audition, tools::DawTools};
use anyhow::{ensure, Context, Result};
use daw_core::audio::{export::render_track_samples, wav_duration_sec};
use daw_core::project::{format, Instrument, Project};
use daw_core::render_control::RenderControl;
use daw_core::sequencer::engine::arranged_track_patterns;
use serde::Serialize;
use std::{path::Path, sync::Mutex, time::Duration};

const QUIET_DBFS: f32 = -80.0;
static TRACK_RENDER: Mutex<()> = Mutex::new(());

struct Snapshot {
    project: Project,
    project_hash: String,
    session_revision: Option<u64>,
}

#[derive(Serialize)]
struct Review {
    project_hash: String,
    session_revision: Option<u64>,
    start_bar: u32,
    end_bar: u32,
    tracks: Vec<TrackReview>,
}

#[derive(Serialize)]
struct TrackReview {
    track_id: String,
    name: String,
    saved_muted: bool,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    measurements: Option<Measurements>,
}

#[derive(Serialize)]
struct Measurements {
    duration_seconds: f64,
    peak_dbfs: f32,
    rms_dbfs: f32,
    near_full_scale_frames: usize,
    stereo_correlation: f32,
    bands_db: Vec<(&'static str, f32)>,
    bars: Vec<BarReview>,
}

#[derive(Serialize)]
struct BarReview {
    bar: u32,
    has_clip: bool,
    rms_dbfs: f32,
    quiet: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    change_from_previous_db: Option<f32>,
}

impl Snapshot {
    fn load(tools: &DawTools) -> Result<Self> {
        let (project, session_revision, directory) = if tools.store.live_socket.is_some() {
            let state = tools.store.inspect_live().map_err(anyhow::Error::msg)?;
            let dir = state
                .project_directory
                .context("live session has no project directory")?;
            (state.project, Some(state.revision), dir)
        } else {
            let project = tools.store.load().map_err(anyhow::Error::msg)?;
            let dir = tools
                .store
                .path
                .parent()
                .unwrap_or(Path::new("."))
                .to_owned();
            (project, None, dir)
        };
        let project_hash = format::project_revision(&project)?;
        let project = format::resolve_assets(&project, &directory, &std::env::current_dir()?);
        Ok(Self {
            project,
            project_hash,
            session_revision,
        })
    }

    fn track_index(&self, track_id: &str) -> Result<usize> {
        self.project
            .tracks
            .iter()
            .position(|track| track.id == track_id)
            .with_context(|| {
                format!("unknown track '{track_id}'; use project_summary for track IDs")
            })
    }

    fn report(self, (start_bar, end_bar): (u32, u32), tracks: Vec<TrackReview>) -> Result<String> {
        Ok(serde_json::to_string(&Review {
            project_hash: self.project_hash,
            session_revision: self.session_revision,
            start_bar,
            end_bar,
            tracks,
        })?)
    }
}

fn clip_spans(project: &Project, index: usize) -> Vec<(f64, f64)> {
    let track = &project.tracks[index];
    let unit = f64::from(project.ticks_per_bar());
    let bar_seconds = project.seconds_per_bar();
    let mut spans: Vec<_> = arranged_track_patterns(project, &track.id)
        .iter()
        .map(|clip| (f64::from(clip.start) / unit, f64::from(clip.end) / unit))
        .collect();
    spans.extend(
        project
            .audio_clips
            .iter()
            .filter(|clip| clip.track == track.id)
            .map(|clip| {
                (
                    clip.start_sec / bar_seconds,
                    (clip.start_sec + clip.len_sec) / bar_seconds,
                )
            }),
    );
    if let Instrument::AudioFile { path } = &track.instrument {
        if let Ok(seconds) = wav_duration_sec(Path::new(path)) {
            spans.push((0.0, seconds / bar_seconds));
        }
    }
    spans
}

fn measure(
    project: &Project,
    index: usize,
    samples: &[f32],
    rate: u32,
    start_bar: u32,
) -> Result<Measurements> {
    ensure!(
        samples.iter().all(|sample| sample.is_finite()),
        "render produced non-finite audio"
    );
    let (left, right): (Vec<_>, Vec<_>) = samples
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[left, right]| (*left, *right))
        .unzip();
    let report = analysis::analyze_from_bar_with_timing(
        &left,
        &right,
        rate,
        project.project.bpm,
        project.project.time_signature,
        &project.tempo_changes,
        start_bar,
    );
    let spans = clip_spans(project, index);
    let loop_pattern = project.arrangement.is_empty() && !project.tracks[index].patterns.is_empty();
    let mut previous = None;
    let bars = report
        .bar_rms_db
        .into_iter()
        .map(|(bar_one_based, rms_dbfs)| {
            let bar = bar_one_based - 1;
            let quiet = rms_dbfs < QUIET_DBFS;
            let change_from_previous_db = previous.filter(|_| !quiet).map(|last| rms_dbfs - last);
            previous = (!quiet).then_some(rms_dbfs);
            let has_clip = loop_pattern
                || spans
                    .iter()
                    .any(|&(lo, hi)| lo < f64::from(bar + 1) && hi > f64::from(bar));
            BarReview {
                bar,
                has_clip,
                rms_dbfs,
                quiet,
                change_from_previous_db,
            }
        })
        .collect();
    Ok(Measurements {
        duration_seconds: report.duration_sec,
        peak_dbfs: report.peak_db,
        rms_dbfs: report.rms_db,
        near_full_scale_frames: report.clipped_samples,
        stereo_correlation: report.stereo_correlation,
        bands_db: report.bands,
        bars,
    })
}

impl TrackReview {
    fn new(project: &Project, index: usize) -> Self {
        let track = &project.tracks[index];
        Self {
            track_id: track.id.clone(),
            name: track.name.clone(),
            saved_muted: track.muted,
            status: "muted",
            error: None,
            measurements: None,
        }
    }

    fn measured(&mut self, measurements: Measurements) {
        self.status = if measurements.bars.iter().all(|bar| bar.quiet) {
            "quiet"
        } else {
            "audible"
        };
        self.measurements = Some(measurements);
    }
}

impl DawTools {
    pub fn check_tracks(
        &self,
        track_ids: Option<Vec<String>>,
        start_bar: Option<u32>,
        end_bar: Option<u32>,
    ) -> Result<String, String> {
        (|| -> Result<_> {
            let start = start_bar.unwrap_or(0);
            let end = end_bar.unwrap_or_else(|| start.saturating_add(8));
            ensure!(
                start < end && end - start <= 32,
                "check range must span 1–32 bars"
            );
            let snapshot = Snapshot::load(self)?;
            let selected = match track_ids {
                Some(ids) => {
                    ensure!(
                        !ids.is_empty(),
                        "provide a track ID or omit track_ids to check all tracks"
                    );
                    let mut selected = ids
                        .iter()
                        .map(|id| snapshot.track_index(id))
                        .collect::<Result<Vec<_>>>()?;
                    selected.sort_unstable();
                    selected.dedup();
                    selected
                }
                None => (0..snapshot.project.tracks.len()).collect(),
            };
            let _slot = TRACK_RENDER.try_lock().map_err(|_| {
                anyhow::anyhow!("track renderer busy; retry after the current check")
            })?;
            let control = RenderControl::new(Duration::from_secs(60));
            let mut tracks = Vec::with_capacity(selected.len());
            for index in selected {
                let mut review = TrackReview::new(&snapshot.project, index);
                if !review.saved_muted {
                    let rendered = render_track_samples(
                        &snapshot.project,
                        &self.sf2_path,
                        index,
                        (start, end),
                        &control,
                    )
                    .and_then(|(samples, rate)| {
                        measure(&snapshot.project, index, &samples, rate, start)
                    });
                    match rendered {
                        Ok(measurements) => review.measured(measurements),
                        Err(error) => {
                            review.status = "render_error";
                            review.error = Some(error.to_string());
                        }
                    }
                }
                tracks.push(review);
            }
            snapshot.report((start, end), tracks)
        })()
        .map_err(|error| error.to_string())
    }

    pub fn listen_track(
        &self,
        track_id: String,
        start_bar: Option<u32>,
        duration_seconds: Option<f64>,
    ) -> Result<(String, Vec<u8>), String> {
        (|| -> Result<_> {
            let duration = duration_seconds.unwrap_or(3.0);
            ensure!(
                (0.25..=6.0).contains(&duration),
                "duration_seconds must be 0.25–6"
            );
            let snapshot = Snapshot::load(self)?;
            let index = snapshot.track_index(&track_id)?;
            let start = start_bar.unwrap_or(0);
            let bars = (duration / snapshot.project.seconds_per_bar()).ceil() as u32;
            let end = start.checked_add(bars).context("start_bar is too large")?;
            let _slot = TRACK_RENDER.try_lock().map_err(|_| {
                anyhow::anyhow!("track renderer busy; retry after the current check")
            })?;
            let control = RenderControl::new(Duration::from_secs(60));
            let (mut samples, rate) = render_track_samples(
                &snapshot.project,
                &self.sf2_path,
                index,
                (start, end),
                &control,
            )?;
            samples.truncate((duration * f64::from(rate)).round() as usize * 2);
            let mut review = TrackReview::new(&snapshot.project, index);
            review.measured(measure(&snapshot.project, index, &samples, rate, start)?);
            let bytes = audition::encode_preview(&samples, rate, true)?;
            Ok((snapshot.report((start, end), vec![review])?, bytes))
        })()
        .map_err(|error| error.to_string())
    }
}

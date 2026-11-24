//! Versioned, atomic edits and shared history. No frontend or device ownership.
use crate::project::{
    format::project_revision, ArrangementSlot, Note, Pattern, PianoRollPattern, Project, TrackType,
    ARR_TICKS_PER_BAR,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::Path;

pub const PROTOCOL_VERSION: u32 = 1;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Edit {
    Replace {
        project: Project,
    },
    Tempo {
        bpm: f64,
    },
    Track {
        id: String,
        volume: Option<f32>,
        pan: Option<f32>,
        muted: Option<bool>,
    },
    ClapParam {
        uid: String,
        param_id: u32,
        value: f64,
    },
    RemoveTrack {
        id: String,
    },
    AddPianoRoll {
        track_id: String,
        pattern_id: String,
        name: String,
        bars: u8,
        notes: Vec<Note>,
    },
    SetNotes {
        pattern_id: String,
        notes: Vec<Note>,
    },
    /// Add a clip or replace the clip at this exact position. Other clips
    /// remain in place, including clips that happen to overlap this one.
    PlaceClip {
        track_id: String,
        pattern_id: String,
        bar: u32,
        #[serde(default)]
        offset_ticks: u32,
        len_bars: Option<u32>,
    },
    RemoveClip {
        track_id: String,
        bar: u32,
        #[serde(default)]
        offset_ticks: u32,
    },
}

fn exact_clip(
    project: &Project,
    track_id: &str,
    bar: u32,
    offset_ticks: u32,
) -> anyhow::Result<Option<usize>> {
    anyhow::ensure!(
        offset_ticks < ARR_TICKS_PER_BAR,
        "clip offset must be 0–479 ticks"
    );
    let mut found = project.arrangement.iter().enumerate().filter(|(_, clip)| {
        clip.track == track_id && clip.bar == bar && clip.offset_ticks == offset_ticks
    });
    let index = found.next().map(|(index, _)| index);
    anyhow::ensure!(
        found.next().is_none(),
        "multiple clips on {track_id} at bar {bar} + {offset_ticks} ticks"
    );
    Ok(index)
}

impl Edit {
    fn apply(&self, project: &mut Project) -> anyhow::Result<()> {
        use anyhow::{ensure, Context};
        match self {
            Self::Replace {
                project: replacement,
            } => *project = replacement.clone(),
            Self::Tempo { bpm } => project.project.bpm = *bpm,
            Self::Track {
                id,
                volume,
                pan,
                muted,
            } => {
                let track = project
                    .tracks
                    .iter_mut()
                    .find(|track| &track.id == id)
                    .with_context(|| format!("missing track {id}"))?;
                if let Some(volume) = volume {
                    track.volume = *volume;
                }
                if let Some(pan) = pan {
                    track.pan = *pan;
                }
                if let Some(muted) = muted {
                    track.muted = *muted;
                }
            }
            Self::RemoveTrack { id } => ensure!(project.remove_track(id), "missing track {id}"),
            Self::ClapParam { uid, param_id, value } => {
                ensure!(value.is_finite(), "CLAP parameter must be finite");
                ensure!(project.set_clap_param(uid, *param_id, *value), "missing CLAP slot {uid}");
            }
            Self::AddPianoRoll {
                track_id,
                pattern_id,
                name,
                bars,
                notes,
            } => {
                ensure!(
                    project.patterns.iter().all(|p| p.id() != pattern_id),
                    "pattern {pattern_id} already exists"
                );
                let length = u32::from(*bars) * ARR_TICKS_PER_BAR;
                ensure!(
                    notes.iter().all(|note| note.tick < length),
                    "note starts outside pattern {pattern_id}"
                );
                let track = project
                    .tracks
                    .iter_mut()
                    .find(|t| t.id == *track_id)
                    .with_context(|| format!("missing track {track_id}"))?;
                ensure!(
                    track.track_type != TrackType::Audio,
                    "cannot add a piano roll to audio track {track_id}"
                );
                track.patterns.push(pattern_id.clone());
                project.patterns.push(Pattern::PianoRoll(PianoRollPattern {
                    id: pattern_id.clone(),
                    name: name.clone(),
                    bars: *bars,
                    ticks_per_bar: ARR_TICKS_PER_BAR,
                    notes: notes.clone(),
                    legato: false,
                }));
            }
            Self::SetNotes { pattern_id, notes } => {
                let Some(Pattern::PianoRoll(pattern)) =
                    project.patterns.iter_mut().find(|p| p.id() == pattern_id)
                else {
                    anyhow::bail!("no piano-roll pattern '{pattern_id}'");
                };
                let length = pattern.length_ticks();
                ensure!(
                    notes.iter().all(|note| note.tick < length),
                    "note starts outside pattern {pattern_id}"
                );
                pattern.notes.clone_from(notes);
            }
            Self::PlaceClip {
                track_id,
                pattern_id,
                bar,
                offset_ticks,
                len_bars,
            } => {
                let track = project
                    .tracks
                    .iter()
                    .find(|t| t.id == *track_id)
                    .with_context(|| format!("missing track {track_id}"))?;
                ensure!(
                    track.track_type != TrackType::Audio && track.patterns.contains(pattern_id),
                    "'{pattern_id}' is not a pattern on note track '{track_id}'"
                );
                let index = exact_clip(project, track_id, *bar, *offset_ticks)?;
                let clip = ArrangementSlot {
                    track: track_id.clone(),
                    pattern: pattern_id.clone(),
                    bar: *bar,
                    offset_ticks: *offset_ticks,
                    len_bars: *len_bars,
                };
                if let Some(index) = index {
                    project.arrangement[index] = clip;
                } else {
                    project.arrangement.push(clip);
                }
            }
            Self::RemoveClip {
                track_id,
                bar,
                offset_ticks,
            } => {
                let index =
                    exact_clip(project, track_id, *bar, *offset_ticks)?.with_context(|| {
                        format!("no clip on {track_id} at bar {bar} + {offset_ticks} ticks")
                    })?;
                project.arrangement.remove(index);
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Inspect,
    /// Inspect the session and include the latest master-output waveform.
    InspectScope,
    Batch {
        label: String,
        edits: Vec<Edit>,
    },
    Preview {
        label: String,
        edits: Vec<Edit>,
    },
    Undo,
    Redo,
    Play,
    Pause,
    Stop,
    Seek {
        tick: u32,
    },
    /// Play test notes on a track in the running engine (sound design).
    /// Edits nothing; the revision stays.
    Audition {
        track_id: String,
        notes: Vec<crate::audition::AuditionNote>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub version: u32,
    pub operation_id: String,
    pub expected_revision: Option<u64>,
    pub command: Command,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionError {
    pub code: String,
    pub message: String,
}
impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for SessionError {}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub version: u32,
    pub operation_id: String,
    pub revision: u64,
    pub project_hash: String,
    pub project: Project,
    pub recent_edit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<Project>,
    pub project_directory: Option<std::path::PathBuf>,
    pub engine_status: String,
    #[serde(default)]
    pub loading_audio_resources: bool,
    #[serde(default)]
    pub audio_diagnostics: Option<crate::audio::engine::AudioDiagnostics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_scope: Option<Vec<f32>>,
    pub applied_revision: u64,
    pub playback_tick: u32,
    pub playing: bool,
    pub error: Option<SessionError>,
}
pub struct Session {
    pub project: Project,
    pub revision: u64,
    pub recent_edit: String,
    pub project_directory: Option<std::path::PathBuf>,
    history: VecDeque<(String, Project, Option<std::path::PathBuf>)>,
    future: Vec<(String, Project, Option<std::path::PathBuf>)>,
    completed: VecDeque<(String, String, Response)>,
}
impl Session {
    pub fn new(project: Project) -> anyhow::Result<Self> {
        project.validate()?;
        Ok(Self {
            project,
            revision: 0,
            recent_edit: "session opened".into(),
            project_directory: None,
            history: VecDeque::new(),
            future: Vec::new(),
            completed: VecDeque::new(),
        })
    }
    pub fn has_operation(&self, id: &str) -> bool {
        self.completed.iter().any(|(i, _, _)| i == id)
    }
    pub fn snapshot(&self, operation_id: &str) -> Response {
        Response {
            version: PROTOCOL_VERSION,
            operation_id: operation_id.into(),
            revision: self.revision,
            project_hash: project_revision(&self.project).unwrap_or_default(),
            project: self.project.clone(),
            recent_edit: self.recent_edit.clone(),
            preview: None,
            project_directory: self.project_directory.clone(),
            engine_status: "pending".into(),
            loading_audio_resources: false,
            audio_diagnostics: None,
            master_scope: None,
            applied_revision: 0,
            playback_tick: 0,
            playing: false,
            error: None,
        }
    }
    pub fn execute(&mut self, req: &Request) -> Response {
        self.execute_checked(req, |_, _| Ok(()))
    }

    /// Let the live host check a validated candidate before committing it.
    /// A `None` candidate checks whether transport may start. Pure previews
    /// never commit or invoke the host check.
    pub fn execute_checked(
        &mut self,
        req: &Request,
        check: impl FnOnce(&Project, Option<(&Project, Option<&Path>)>) -> Result<(), SessionError>,
    ) -> Response {
        let encoded = serde_json::to_string(req).unwrap_or_default();
        if let Some((_, original, response)) = self
            .completed
            .iter()
            .find(|(id, _, _)| id == &req.operation_id)
        {
            if original == &encoded {
                return response.clone();
            }
            return self.failure(
                req,
                "operation_id_reused",
                "operation id was already used for another request",
            );
        }
        if req.version != PROTOCOL_VERSION {
            return self.failure(
                req,
                "protocol_version",
                "unsupported session protocol version",
            );
        }
        if req.operation_id.is_empty() || req.operation_id.len() > 128 {
            return self.failure(
                req,
                "invalid_operation_id",
                "operation id must contain 1–128 bytes",
            );
        }
        if !matches!(req.command, Command::Inspect | Command::InspectScope)
            && req.expected_revision != Some(self.revision)
        {
            return self.failure(
                req,
                "revision_conflict",
                "refresh the project before retrying; nothing changed",
            );
        }
        let mut preview = None;
        let result: anyhow::Result<()> = (|| {
            match &req.command {
                Command::Batch { label, edits } | Command::Preview { label, edits } => {
                    anyhow::ensure!(
                        !edits.is_empty() && edits.len() <= 256,
                        "batch needs 1–256 edits"
                    );
                    let mut next = self.project.clone();
                    for edit in edits {
                        edit.apply(&mut next)?;
                    }
                    next.validate()?;
                    if matches!(req.command, Command::Preview { .. }) {
                        preview = Some(next);
                        return Ok(());
                    }
                    check(
                        &self.project,
                        Some((&next, self.project_directory.as_deref())),
                    )?;
                    // Each transaction is exactly one undo step; no time-based
                    // coalescing across independent human/agent operations.
                    self.history.push_back((
                        label.clone(),
                        self.project.clone(),
                        self.project_directory.clone(),
                    ));
                    if self.history.len() > 128 {
                        self.history.pop_front();
                    }
                    self.future.clear();
                    self.project = next;
                    self.recent_edit = label.clone();
                    self.revision += 1;
                }
                Command::Undo => {
                    let (_, next, directory) = self
                        .history
                        .back()
                        .ok_or_else(|| anyhow::anyhow!("nothing to undo"))?;
                    check(&self.project, Some((next, directory.as_deref())))?;
                    let (label, project, directory) = self
                        .history
                        .pop_back()
                        .ok_or_else(|| anyhow::anyhow!("nothing to undo"))?;
                    self.future.push((
                        label.clone(),
                        self.project.clone(),
                        self.project_directory.clone(),
                    ));
                    self.project_directory = directory;
                    self.project = project;
                    self.recent_edit = format!("undo: {label}");
                    self.revision += 1;
                }
                Command::Redo => {
                    let (_, next, directory) = self
                        .future
                        .last()
                        .ok_or_else(|| anyhow::anyhow!("nothing to redo"))?;
                    check(&self.project, Some((next, directory.as_deref())))?;
                    let (label, project, directory) = self
                        .future
                        .pop()
                        .ok_or_else(|| anyhow::anyhow!("nothing to redo"))?;
                    self.history.push_back((
                        label.clone(),
                        self.project.clone(),
                        self.project_directory.clone(),
                    ));
                    self.project_directory = directory;
                    self.project = project;
                    self.recent_edit = format!("redo: {label}");
                    self.revision += 1;
                }
                Command::Audition { track_id, notes } => {
                    anyhow::ensure!(
                        self.project.tracks.iter().any(|t| &t.id == track_id),
                        "missing track {track_id}"
                    );
                    crate::audition::validate_notes(notes).map_err(anyhow::Error::msg)?;
                }
                Command::Play => check(&self.project, None)?,
                _ => {}
            }
            Ok(())
        })();
        if let Err(e) = result {
            if let Some(error) = e.downcast_ref::<SessionError>() {
                return self.failure(req, &error.code, &error.message);
            }
            return self.failure(req, "invalid_edit", &e.to_string());
        }
        let mut response = self.snapshot(&req.operation_id);
        response.preview = preview;
        if !matches!(req.command, Command::Inspect | Command::InspectScope) {
            self.completed
                .push_back((req.operation_id.clone(), encoded, response.clone()));
            if self.completed.len() > 128 {
                self.completed.pop_front();
            }
        }
        response
    }
    fn failure(&self, req: &Request, code: &str, message: &str) -> Response {
        let mut r = self.snapshot(&req.operation_id);
        r.error = Some(SessionError {
            code: code.into(),
            message: message.into(),
        });
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(id: &str, revision: u64, command: Command) -> Request {
        Request {
            version: 1,
            operation_id: id.into(),
            expected_revision: Some(revision),
            command,
        }
    }

    #[test]
    fn eight_bar_clip_batch_previews_and_undoes_as_one_edit() {
        let original = Project::default_project();
        let original_hash = project_revision(&original).unwrap();
        let mut session = Session::new(original).unwrap();
        let edits: Vec<Edit> = serde_json::from_value(serde_json::json!([
            {"type": "add_piano_roll", "track_id": "t2", "pattern_id": "bridge", "name": "Bridge", "bars": 8, "notes": []},
            {"type": "set_notes", "pattern_id": "bridge", "notes": [
                {"pitch": 42, "tick": 0, "duration": 120, "vel": 89},
                {"pitch": 38, "tick": 3480, "duration": 210, "vel": 77}
            ]},
            {"type": "place_clip", "track_id": "t2", "pattern_id": "bridge", "bar": 2, "offset_ticks": 60}
        ])).unwrap();
        let label = "Agent: eight-bar bridge".to_owned();
        let preview = session.execute(&request(
            "preview-bridge",
            0,
            Command::Preview {
                label: label.clone(),
                edits: edits.clone(),
            },
        ));
        let proposed = preview.preview.unwrap();
        assert_eq!(preview.revision, 0);
        assert_eq!(preview.project_hash, original_hash);
        assert_eq!(proposed.arrangement.last().unwrap().start_tick(), 1020);
        assert_eq!(proposed.arrangement.len(), 3);

        let mut invalid = edits.clone();
        invalid.push(Edit::RemoveClip {
            track_id: "t2".into(),
            bar: 2,
            offset_ticks: 0,
        });
        let rejected = session.execute(&request(
            "bad-bridge",
            0,
            Command::Batch {
                label: label.clone(),
                edits: invalid,
            },
        ));
        assert!(rejected.error.is_some());
        assert_eq!(rejected.project_hash, original_hash);
        let applied = session.execute(&request("apply-bridge", 0, Command::Batch { label, edits }));
        assert_eq!(applied.revision, 1);
        assert_eq!(applied.project_hash, project_revision(&proposed).unwrap());
        let undone = session.execute(&request("undo-bridge", 1, Command::Undo));
        assert_eq!(undone.revision, 2);
        assert_eq!(undone.project_hash, original_hash);
    }

    #[test]
    fn audition_command_keeps_revision() {
        let mut session = Session::new(Project::default_project()).unwrap();
        let note = crate::audition::AuditionNote {
            pitch: 60,
            velocity: 100,
            start_ms: 0,
            dur_ms: 500,
        };
        let ok = session.execute(&request(
            "a1",
            0,
            Command::Audition {
                track_id: "t1".into(),
                notes: vec![note],
            },
        ));
        assert!(ok.error.is_none(), "{:?}", ok.error);
        assert_eq!(session.revision, 0, "auditioning edits nothing");
        let missing = session.execute(&request(
            "a2",
            0,
            Command::Audition {
                track_id: "zz".into(),
                notes: vec![note],
            },
        ));
        assert!(missing.error.is_some());
        let bad = session.execute(&request(
            "a3",
            0,
            Command::Audition {
                track_id: "t1".into(),
                notes: vec![],
            },
        ));
        assert!(bad.error.is_some(), "invalid notes are rejected");
    }
    #[test]
    fn preview_validates_without_mutation_or_history() {
        let mut s = Session::new(Project::default_project()).unwrap();
        let r = s.execute(&request(
            "preview",
            0,
            Command::Preview {
                label: "audition".into(),
                edits: vec![Edit::Tempo { bpm: 90. }],
            },
        ));
        assert_eq!(r.revision, 0);
        assert_eq!(r.project.project.bpm, 120.);
        assert_eq!(r.preview.unwrap().project.bpm, 90.);
        assert!(s
            .execute(&request("undo", 0, Command::Undo))
            .error
            .is_some());
    }
    #[test]
    fn batches_are_atomic_conflict_checked_and_undoable() {
        let mut s = Session::new(Project::default_project()).unwrap();
        let bad = request(
            "bad",
            0,
            Command::Batch {
                label: "bad".into(),
                edits: vec![
                    Edit::Tempo { bpm: 100. },
                    Edit::RemoveTrack {
                        id: "absent".into(),
                    },
                ],
            },
        );
        assert!(s.execute(&bad).error.is_some());
        assert_eq!(s.project.project.bpm, 120.);
        let edit = request(
            "edit",
            0,
            Command::Batch {
                label: "agent".into(),
                edits: vec![Edit::Tempo { bpm: 100. }],
            },
        );
        assert_eq!(s.execute(&edit).revision, 1);
        assert_eq!(s.execute(&edit).revision, 1);
        assert!(s
            .execute(&request("stale", 0, Command::Stop))
            .error
            .is_some());
        assert!(s
            .execute(&request("undo", 1, Command::Undo))
            .error
            .is_none());
        assert_eq!(s.project.project.bpm, 120.);
        s.execute(&request("redo", 2, Command::Redo));
        assert_eq!(s.project.project.bpm, 100.);
    }
}

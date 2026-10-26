//! Versioned, atomic edits and shared history. No frontend or device ownership.
use crate::project::{format::project_revision, Project};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

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
    RemoveTrack {
        id: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Inspect,
    Batch { label: String, edits: Vec<Edit> },
    Preview { label: String, edits: Vec<Edit> },
    Undo,
    Redo,
    Play,
    Pause,
    Stop,
    Seek { tick: u32 },
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
    pub audio_diagnostics: Option<crate::audio::engine::AudioDiagnostics>,
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
            audio_diagnostics: None,
            applied_revision: 0,
            playback_tick: 0,
            playing: false,
            error: None,
        }
    }
    pub fn execute(&mut self, req: &Request) -> Response {
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
        if !matches!(req.command, Command::Inspect) && req.expected_revision != Some(self.revision)
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
                        match edit {
                            Edit::Replace { project } => next = project.clone(),
                            Edit::Tempo { bpm } => next.project.bpm = *bpm,
                            Edit::Track {
                                id,
                                volume,
                                pan,
                                muted,
                            } => {
                                let t = next
                                    .tracks
                                    .iter_mut()
                                    .find(|t| &t.id == id)
                                    .ok_or_else(|| anyhow::anyhow!("missing track {id}"))?;
                                if let Some(v) = volume {
                                    t.volume = *v;
                                }
                                if let Some(v) = pan {
                                    t.pan = *v;
                                }
                                if let Some(v) = muted {
                                    t.muted = *v;
                                }
                            }
                            Edit::RemoveTrack { id } => {
                                anyhow::ensure!(next.remove_track(id), "missing track {id}");
                            }
                        }
                    }
                    next.validate()?;
                    if matches!(req.command, Command::Preview { .. }) {
                        preview = Some(next);
                        return Ok(());
                    }
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
                _ => {}
            }
            Ok(())
        })();
        if let Err(e) = result {
            return self.failure(req, "invalid_edit", &e.to_string());
        }
        let mut response = self.snapshot(&req.operation_id);
        response.preview = preview;
        if !matches!(req.command, Command::Inspect) {
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

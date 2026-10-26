//! Local live-session bridge. One request per connection, bounded framing and
//! timeouts; never falls back to editing a file when the session disappears.
use anyhow::{ensure, Result};
use daw_core::{
    project::{format, Project},
    sequencer::SequencerCommand,
    session::{Command, Edit, Request, Response, Session},
};
use std::os::unix::{
    fs::{MetadataExt, PermissionsExt},
    net::{UnixListener, UnixStream},
};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
static NEXT: AtomicU64 = AtomicU64::new(0);
const MAX_MESSAGE: u64 = 16 * 1024 * 1024;
/// These commands describe persisted edits. Frontends must publish their staged
/// project through revision checks before the engine receives the edit.
pub fn is_project_edit(command: &SequencerCommand) -> bool {
    matches!(
        command,
        SequencerCommand::UpdateLive(_)
            | SequencerCommand::SetBpm(_)
            | SequencerCommand::SetSwing(_)
            | SequencerCommand::SetTiming(_)
            | SequencerCommand::SetMetronome(_)
            | SequencerCommand::SetTrackEffects { .. }
            | SequencerCommand::SetReturns(_)
            | SequencerCommand::SetLoopRegion(_)
            | SequencerCommand::SetClapParam { .. }
            | SequencerCommand::SetTrackVolume { .. }
            | SequencerCommand::SetTrackPan { .. }
            | SequencerCommand::SetTrackMute { .. }
    )
}
pub fn operation_id() -> String {
    format!(
        "{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
pub fn runtime_dir() -> Result<PathBuf> {
    let uid = std::fs::metadata("/proc/self")?.uid();
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("daw-ai-{uid}"));
    match std::fs::create_dir(&dir) {
        Ok(()) => std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    let meta = std::fs::symlink_metadata(&dir)?;
    ensure!(
        meta.is_dir() && meta.uid() == uid && meta.permissions().mode() & 0o077 == 0,
        "unsafe session directory"
    );
    Ok(dir)
}
pub fn discover() -> Result<Vec<PathBuf>> {
    Ok(std::fs::read_dir(runtime_dir()?)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "sock") && UnixStream::connect(p).is_ok())
        .collect())
}
pub fn call(path: &Path, request: &Request) -> Result<Response> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let bytes = serde_json::to_vec(request)?;
    ensure!(
        bytes.len() < MAX_MESSAGE as usize,
        "session request too large"
    );
    stream.write_all(&bytes)?;
    stream.write_all(b"\n")?;
    let mut text = String::new();
    BufReader::new(stream)
        .take(MAX_MESSAGE)
        .read_line(&mut text)?;
    Ok(serde_json::from_str(&text)?)
}
struct Service {
    session: Mutex<Session>,
    tx: crossbeam_channel::Sender<SequencerCommand>,
    ack: Arc<AtomicU64>,
    sample_rate: Arc<AtomicU32>,
    tick: Arc<AtomicU32>,
    transport: Arc<AtomicU8>,
    error: Arc<Mutex<Option<String>>>,
    base: Mutex<PathBuf>,
    legacy: PathBuf,
}
impl Service {
    fn execute(&self, request: &Request) -> Result<Response> {
        self.execute_at(request, None)
    }
    fn execute_at(&self, request: &Request, base: Option<&Path>) -> Result<Response> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("session lock poisoned"))?;
        let previous = session.revision;
        let duplicate = session.has_operation(&request.operation_id);
        let mut response = session.execute(request);
        if response.error.is_none() && !duplicate {
            if let Some(base) = base {
                session.project_directory = Some(base.to_path_buf());
            }
            if let Some(base) = &session.project_directory {
                *self.base.lock().unwrap() = base.clone();
            }
            let command = if session.revision != previous {
                let runtime = format::resolve_assets(
                    &session.project,
                    &self.base.lock().unwrap(),
                    &self.legacy,
                );
                Some(SequencerCommand::UpdateRevision {
                    project: runtime,
                    revision: session.revision,
                    ack: self.ack.clone(),
                })
            } else {
                match request.command {
                    Command::Play => Some(SequencerCommand::Play),
                    Command::Pause => Some(SequencerCommand::Pause),
                    Command::Stop => {
                        self.tx.send(SequencerCommand::Pause)?;
                        Some(SequencerCommand::Seek(0))
                    }
                    Command::Seek { tick } => Some(SequencerCommand::Seek(tick)),
                    _ => None,
                }
            };
            if let Some(command) = command {
                if let Err(e) = self.tx.send(command) {
                    *self.error.lock().unwrap() = Some(format!("sequencer disconnected: {e}"));
                }
            }
        }
        response.audio_diagnostics = Some(daw_core::audio::engine::diagnostics());
        response.project_directory = Some(self.base.lock().unwrap().clone());
        response.applied_revision = self.ack.load(Ordering::Acquire);
        response.playback_tick = self.tick.load(Ordering::Relaxed);
        response.playing =
            self.transport.load(Ordering::Relaxed) == daw_core::audio::input::TRANSPORT_ROLLING;
        let dropped = daw_core::audio::engine::DROPPED_AUDIO_EVENTS.load(Ordering::Relaxed);
        response.engine_status = if let Some(e) = self.error.lock().unwrap().clone() {
            format!("failed: {e}")
        } else if self.sample_rate.load(Ordering::Relaxed) == 0 {
            "unavailable: audio output not initialized".into()
        } else if daw_core::audio::engine::AUDIO_STREAM_ERRORS.load(Ordering::Relaxed) > 0 {
            "failed: audio output stream error".into()
        } else if daw_core::audio::clap::CLAP_PROCESS_ERRORS.load(Ordering::Relaxed) > 0 {
            "failed: plugin processing error; inspect audio diagnostics".into()
        } else if daw_core::audio::clap::DROPPED_CLAP_EVENTS.load(Ordering::Relaxed) > 0 {
            "failed: plugin event overflow; inspect audio diagnostics".into()
        } else if daw_core::audio::mixer::SAMPLER_VOICE_STEALS.load(Ordering::Relaxed) > 0 {
            "overloaded: sampler voice limit reached; inspect audio diagnostics".into()
        } else if dropped > 0 {
            format!("failed: {dropped} audio events dropped; restart engine")
        } else if response.applied_revision < response.revision {
            "pending".into()
        } else {
            "applied".into()
        };
        Ok(response)
    }
}
#[derive(Clone)]
pub struct LiveSession {
    service: Arc<Service>,
    seen: Arc<Mutex<(u64, String)>>,
    pub socket: PathBuf,
}
impl LiveSession {
    pub fn start(
        project: Project,
        project_path: Option<&Path>,
        host: &crate::Host,
    ) -> Result<Self> {
        let hash = format::project_revision(&project)?;
        let mut session = Session::new(project)?;
        session.project_directory = Some(std::path::absolute(
            project_path
                .and_then(Path::parent)
                .unwrap_or(Path::new(".")),
        )?);
        let service = Arc::new(Service {
            session: Mutex::new(session),
            tx: host.cmd_tx.clone(),
            ack: Arc::new(AtomicU64::new(0)),
            sample_rate: host.sample_rate.clone(),
            tick: host.playback_tick.clone(),
            transport: host.transport_state.clone(),
            error: host.engine_error.clone(),
            base: Mutex::new(std::path::absolute(
                project_path
                    .and_then(Path::parent)
                    .unwrap_or(Path::new(".")),
            )?),
            legacy: std::env::current_dir()?,
        });
        Self::bind(
            service,
            hash,
            runtime_dir()?.join(format!("{}.sock", operation_id())),
        )
    }
    fn bind(service: Arc<Service>, hash: String, socket: PathBuf) -> Result<Self> {
        let listener = UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let weak = Arc::downgrade(&service);
        let cleanup = socket.clone();
        std::thread::spawn(move || {
            while let Some(service) = weak.upgrade() {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                        let result = (|| -> Result<()> {
                            let mut text = String::new();
                            BufReader::new(&stream)
                                .take(MAX_MESSAGE)
                                .read_line(&mut text)?;
                            let req: Request = serde_json::from_str(&text)?;
                            let response = service.execute(&req)?;
                            serde_json::to_writer(&mut stream, &response)?;
                            stream.write_all(b"\n")?;
                            Ok(())
                        })();
                        if let Err(e) = result {
                            eprintln!("session request: {e}");
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(20))
                    }
                    Err(_) => break,
                }
            }
            let _ = std::fs::remove_file(cleanup);
        });
        Ok(Self {
            service,
            seen: Arc::new(Mutex::new((0, hash))),
            socket,
        })
    }
    /// Small UI status read: never clone or serialize the project during paint.
    pub fn status_line(&self) -> String {
        let session = self.service.session.lock().unwrap();
        let applied = self.service.ack.load(Ordering::Acquire);
        let error = self.service.error.lock().unwrap();
        let health = if let Some(error) = error.as_ref() {
            format!("failed: {error}")
        } else if self.service.sample_rate.load(Ordering::Relaxed) == 0 {
            "audio unavailable".into()
        } else if daw_core::audio::clap::CLAP_PROCESS_ERRORS.load(Ordering::Relaxed) > 0 {
            "plugin processing error".into()
        } else if daw_core::audio::clap::DROPPED_CLAP_EVENTS.load(Ordering::Relaxed) > 0 {
            "plugin event overflow".into()
        } else if daw_core::audio::engine::DROPPED_AUDIO_EVENTS.load(Ordering::Relaxed) > 0 {
            "audio queue overflow".into()
        } else if daw_core::audio::engine::LATE_AUDIO_EVENTS.load(Ordering::Relaxed) > 0 {
            format!("late audio events: {}", daw_core::audio::engine::LATE_AUDIO_EVENTS.load(Ordering::Relaxed))
        } else if applied < session.revision {
            "pending".into()
        } else {
            "applied".into()
        };
        format!(
            "Session r{} · {} · {}",
            session.revision, health, session.recent_edit
        )
    }
    pub fn playing(&self) -> bool {
        self.service.transport.load(Ordering::Acquire) == daw_core::audio::input::TRANSPORT_ROLLING
    }
    pub fn transport(&self, command: Command) -> Result<()> {
        let revision = self.service.session.lock().unwrap().revision;
        let response = self.service.execute(&Request {
            version: 1,
            operation_id: operation_id(),
            expected_revision: Some(revision),
            command,
        })?;
        if let Some(error) = response.error {
            anyhow::bail!("{}: {}", error.code, error.message);
        }
        Ok(())
    }
    pub fn inspect(&self) -> Result<Response> {
        self.service.execute(&Request {
            version: 1,
            operation_id: operation_id(),
            expected_revision: None,
            command: Command::Inspect,
        })
    }
    pub fn project_directory(&self) -> PathBuf {
        self.service.base.lock().unwrap().clone()
    }
    pub fn runtime_project(&self, project: &Project) -> Project {
        format::resolve_assets(project, &self.project_directory(), &self.service.legacy)
    }
    pub fn set_base(&self, path: &Path) {
        let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut session = self.service.session.lock().unwrap();
        session.project_directory = Some(base.clone());
        *self.service.base.lock().unwrap() = base;
    }
    /// Local editors stage a project, then atomically publish against the last
    /// revision they observed. If both sides edited, retain local work and fail.
    pub fn submit(&self, project: &Project) -> Result<()> {
        self.submit_at(project, None)
    }
    pub fn import(&self, project: &Project, path: &Path) -> Result<()> {
        self.submit_at(project, path.parent())
    }
    fn submit_at(&self, project: &Project, base: Option<&Path>) -> Result<()> {
        let hash = format::project_revision(project)?;
        let mut seen = self.seen.lock().unwrap();
        if hash == seen.1 && base.is_none() {
            return Ok(());
        }
        let r = self.service.execute_at(
            &Request {
                version: 1,
                operation_id: operation_id(),
                expected_revision: Some(seen.0),
                command: Command::Batch {
                    label: "local edit".into(),
                    edits: vec![Edit::Replace {
                        project: project.clone(),
                    }],
                },
            },
            base,
        )?;
        if let Some(e) = r.error {
            anyhow::bail!("{}: {}", e.code, e.message);
        }
        *seen = (r.revision, r.project_hash);
        Ok(())
    }
    pub fn sync(&self, project: &mut Project) -> Result<bool> {
        let mut seen = self.seen.lock().unwrap();
        let hash = format::project_revision(project)?;
        if hash != seen.1 {
            let r = self.service.execute(&Request {
                version: 1,
                operation_id: operation_id(),
                expected_revision: Some(seen.0),
                command: Command::Batch {
                    label: "local edit".into(),
                    edits: vec![Edit::Replace {
                        project: project.clone(),
                    }],
                },
            })?;
            if let Some(e) = r.error {
                anyhow::bail!("{}: {} (local changes retained)", e.code, e.message);
            }
            *seen = (r.revision, r.project_hash);
            return Ok(false);
        }
        if self.service.session.lock().unwrap().revision == seen.0 {
            return Ok(false);
        }
        let r = self.inspect()?;
        let changed = r.revision != seen.0;
        *project = r.project;
        *seen = (r.revision, r.project_hash);
        Ok(changed)
    }
    /// Explicit reconciliation: preserve a conflicting local version before
    /// accepting the authoritative session. Never overwrite a recovery file.
    pub fn reconcile(&self, project: &mut Project) -> Result<Option<PathBuf>> {
        let mut seen = self.seen.lock().unwrap();
        let state = self.inspect()?;
        let mut backup = None;
        if format::project_revision(project)? != state.project_hash {
            let path = self
                .project_directory()
                .join(format!("local-recovery-{}.daw.json", operation_id()));
            format::save_if_unchanged(&self.runtime_project(project), &path, None)?;
            backup = Some(path);
        }
        *project = state.project;
        *seen = (state.revision, state.project_hash);
        Ok(backup)
    }
    pub fn history(&self, redo: bool, project: &mut Project) -> Result<()> {
        self.sync(project)?;
        let mut seen = self.seen.lock().unwrap();
        let r = self.service.execute(&Request {
            version: 1,
            operation_id: operation_id(),
            expected_revision: Some(seen.0),
            command: if redo { Command::Redo } else { Command::Undo },
        })?;
        if let Some(e) = r.error {
            anyhow::bail!("{}", e.message);
        }
        *project = r.project;
        *seen = (r.revision, r.project_hash);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_import_retains_directory_and_undo_restores_it() {
        let mut session = Session::new(Project::default_project()).unwrap();
        session.project_directory = Some(PathBuf::from("/old/project"));
        let (tx, _rx) = crossbeam_channel::unbounded();
        let service = Service {
            session: Mutex::new(session),
            tx,
            ack: Arc::new(AtomicU64::new(0)),
            sample_rate: Arc::new(AtomicU32::new(0)),
            tick: Arc::new(AtomicU32::new(0)),
            transport: Arc::new(AtomicU8::new(0)),
            error: Arc::new(Mutex::new(None)),
            base: Mutex::new(PathBuf::from("/old/project")),
            legacy: PathBuf::from("/"),
        };
        let mut request = Request {
            version: 1,
            operation_id: "import".into(),
            expected_revision: Some(9),
            command: Command::Batch {
                label: "import".into(),
                edits: vec![Edit::Tempo { bpm: 90. }],
            },
        };
        assert!(service
            .execute_at(&request, Some(Path::new("/new/project")))
            .unwrap()
            .error
            .is_some());
        assert_eq!(*service.base.lock().unwrap(), PathBuf::from("/old/project"));
        request.expected_revision = Some(0);
        assert!(service
            .execute_at(&request, Some(Path::new("/new/project")))
            .unwrap()
            .error
            .is_none());
        assert_eq!(*service.base.lock().unwrap(), PathBuf::from("/new/project"));
        request = Request {
            operation_id: "undo".into(),
            expected_revision: Some(1),
            command: Command::Undo,
            ..request
        };
        assert!(service.execute(&request).unwrap().error.is_none());
        assert_eq!(*service.base.lock().unwrap(), PathBuf::from("/old/project"));
    }
    #[test]
    fn socket_clients_conflict_retry_undo_and_disconnect() {
        let project = Project::default_project();
        let (tx, rx) = crossbeam_channel::unbounded();
        let service = Arc::new(Service {
            session: Mutex::new(Session::new(project.clone()).unwrap()),
            tx,
            ack: Arc::new(AtomicU64::new(0)),
            sample_rate: Arc::new(AtomicU32::new(44100)),
            tick: Arc::new(AtomicU32::new(123)),
            transport: Arc::new(AtomicU8::new(0)),
            error: Arc::new(Mutex::new(None)),
            base: Mutex::new(std::env::temp_dir()),
            legacy: std::env::temp_dir(),
        });
        let socket = std::env::temp_dir().join(format!("daw-test-{}.sock", operation_id()));
        let live = LiveSession::bind(
            service,
            format::project_revision(&project).unwrap(),
            socket.clone(),
        )
        .unwrap();
        let request = Request {
            version: 1,
            operation_id: "edit".into(),
            expected_revision: Some(0),
            command: Command::Batch {
                label: "agent batch".into(),
                edits: vec![Edit::Tempo { bpm: 99. }],
            },
        };
        let result = call(&socket, &request).unwrap();
        assert_eq!(result.revision, 1);
        assert_eq!(result.engine_status, "pending");
        assert_eq!(call(&socket, &request).unwrap().revision, 1);
        assert_eq!(rx.try_iter().count(), 1, "retry must not dispatch twice");
        let stale = Request {
            operation_id: "other-client".into(),
            ..request.clone()
        };
        assert_eq!(
            call(&socket, &stale).unwrap().error.unwrap().code,
            "revision_conflict"
        );
        let mut local = project.clone();
        assert!(live.sync(&mut local).unwrap());
        assert_eq!(local.project.bpm, 99.);
        live.history(false, &mut local).unwrap();
        assert_eq!(local.project.bpm, 120.);
        assert_eq!(
            call(
                &socket,
                &Request {
                    operation_id: "reconnect".into(),
                    expected_revision: None,
                    command: Command::Inspect,
                    ..request
                }
            )
            .unwrap()
            .revision,
            2
        );
        drop(live);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while socket.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!socket.exists());
        assert!(call(&socket, &stale).is_err());
    }
}

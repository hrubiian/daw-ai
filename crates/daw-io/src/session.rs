//! Local live-session bridge. One request per connection, bounded framing and
//! timeouts; never falls back to editing a file when the session disappears.
use anyhow::{ensure, Result};
use daw_core::{
    audio::input::TRANSPORT_STOPPED,
    project::{format, ClapSlot, Instrument, Project},
    sequencer::SequencerCommand,
    session::{Command, Edit, Request, Response, Session, SessionError},
};
use std::os::unix::{
    fs::{MetadataExt, PermissionsExt},
    net::{UnixListener, UnixStream},
};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering},
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
    play_requested: AtomicBool,
    resources_pending_at: AtomicU64,
    error: Arc<Mutex<Option<String>>>,
    master_scope: Arc<Mutex<daw_core::dsp::output::OutputHistory>>,
    base: Mutex<PathBuf>,
    legacy: PathBuf,
}

// This comparison is deliberately conservative. The sequencer/relay can load
// assets for any of these differences, even when a track is currently muted.
fn changes_playback_resources(current: &Project, next: &Project) -> bool {
    fn same_plugin(a: &ClapSlot, b: &ClapSlot) -> bool {
        a.uid == b.uid && a.path == b.path && a.id == b.id && a.state == b.state
    }

    current.tracks.len() != next.tracks.len()
        || current.tracks.iter().zip(&next.tracks).any(|(a, b)| {
            let same_instrument = match (&a.instrument, &b.instrument) {
                (Instrument::Clap(a), Instrument::Clap(b)) => same_plugin(a, b),
                (a, b) => a == b,
            };
            a.id != b.id
                || a.track_type != b.track_type
                || a.tuning_cents != b.tuning_cents
                || !same_instrument
                || a.clap_fx.len() != b.clap_fx.len()
                || a.clap_fx
                    .iter()
                    .zip(&b.clap_fx)
                    .any(|(a, b)| !same_plugin(a, b))
        })
        || current.audio_clips.len() != next.audio_clips.len()
        || current
            .audio_clips
            .iter()
            .zip(&next.audio_clips)
            .any(|(a, b)| a.path != b.path)
}

impl Service {
    fn resources_loading(&self) -> bool {
        self.ack.load(Ordering::Acquire) < self.resources_pending_at.load(Ordering::Relaxed)
    }

    fn check_audio_ready(&self) -> std::result::Result<(), SessionError> {
        if self.resources_loading() {
            return Err(SessionError {
                code: "resources_loading".into(),
                message:
                    "Wait for the audio engine to finish loading the latest edit before playing."
                        .into(),
            });
        }
        Ok(())
    }

    fn execute(&self, request: &Request) -> Result<Response> {
        self.execute_at(request, None, false)
    }
    fn execute_at(
        &self,
        request: &Request,
        base: Option<&Path>,
        external: bool,
    ) -> Result<Response> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("session lock poisoned"))?;
        let previous = session.revision;
        let duplicate = session.has_operation(&request.operation_id);
        let current_base = session.project_directory.clone();
        let mut loads_resources = false;
        let mut response = session.execute_checked(request, |current, candidate| {
            if let Some((next, next_base)) = candidate {
                loads_resources = base.or(next_base) != current_base.as_deref()
                    || changes_playback_resources(current, next);
                if external && loads_resources
                    && (self.play_requested.load(Ordering::Relaxed)
                        || self.transport.load(Ordering::Acquire) != TRANSPORT_STOPPED)
                {
                    return Err(SessionError {
                        code: "resources_require_pause".into(),
                        message: "Pause playback before applying changes that load instruments or audio assets.".into(),
                    });
                }
            } else {
                self.check_audio_ready()?;
            }
            Ok(())
        });
        if response.error.is_none() && !duplicate {
            if let Some(base) = base {
                session.project_directory = Some(base.to_path_buf());
            }
            if let Some(base) = &session.project_directory {
                *self.base.lock().unwrap() = base.clone();
            }
            let command = if session.revision != previous {
                if loads_resources {
                    self.resources_pending_at
                        .store(session.revision, Ordering::Relaxed);
                }
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
                    Command::Play => {
                        self.play_requested.store(true, Ordering::Relaxed);
                        Some(SequencerCommand::Play)
                    }
                    Command::Pause => {
                        self.play_requested.store(false, Ordering::Relaxed);
                        Some(SequencerCommand::Pause)
                    }
                    Command::Stop => {
                        self.play_requested.store(false, Ordering::Relaxed);
                        self.tx.send(SequencerCommand::Pause)?;
                        Some(SequencerCommand::Seek(0))
                    }
                    Command::Seek { tick } => Some(SequencerCommand::Seek(tick)),
                    Command::Audition {
                        ref track_id,
                        ref notes,
                    } => Some(SequencerCommand::Audition {
                        track_id: track_id.clone(),
                        notes: notes.clone(),
                    }),
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
        if matches!(request.command, Command::InspectScope) {
            if let Ok(samples) = self.master_scope.try_lock() {
                response.master_scope = Some(
                    samples.recent_mono(1024).step_by(4).collect(),
                );
            }
        }
        response.project_directory = Some(self.base.lock().unwrap().clone());
        response.applied_revision = self.ack.load(Ordering::Acquire);
        response.loading_audio_resources =
            response.applied_revision < self.resources_pending_at.load(Ordering::Relaxed);
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
            play_requested: AtomicBool::new(false),
            resources_pending_at: AtomicU64::new(0),
            error: host.engine_error.clone(),
            master_scope: host.vis_buf.clone(),
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
                            let read = BufReader::new(&stream)
                                .take(MAX_MESSAGE)
                                .read_line(&mut text)?;
                            if read == 0 {
                                return Ok(()); // Session discovery probes the socket.
                            }
                            let req: Request = serde_json::from_str(&text)?;
                            let response = service.execute_at(&req, None, true)?;
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
            format!(
                "late audio events: {}",
                daw_core::audio::engine::LATE_AUDIO_EVENTS.load(Ordering::Relaxed)
            )
        } else if applied < self.service.resources_pending_at.load(Ordering::Relaxed) {
            "loading audio resources".into()
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

    /// Arm a local take and queue its count-in as one transport action. The
    /// callback arms capture before the sequencer can roll.
    pub fn start_recording(&self, count_in_bars: u8, arm: impl FnOnce()) -> Result<()> {
        let _session = self
            .service
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("session lock poisoned"))?;
        self.service.check_audio_ready()?;
        arm();
        self.service
            .tx
            .send(SequencerCommand::RecordStart { count_in_bars })?;
        self.service.play_requested.store(true, Ordering::Relaxed);
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
            false,
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
    fn remote_instrument_load_waits_for_pause_while_drum_loop_keeps_scheduling() {
        use daw_core::audio::AudioEvent;

        let mut project = Project::default_project();
        project.project.bpm = 300.;
        project.loop_region.enabled = true;
        project.loop_region.end_bar = 1;
        let mut changed = project.clone();
        changed.tracks[1].instrument = Instrument::Soundfont {
            preset: 32,
            bank: 0,
            soundfont: Some("/slow-to-load.sf2".into()),
        };
        let (tx, commands) = crossbeam_channel::unbounded();
        let (audio_tx, audio) = crossbeam_channel::unbounded();
        let sequencer = daw_core::sequencer::Sequencer::new(commands, audio_tx);
        let service = Service {
            session: Mutex::new(Session::new(project.clone()).unwrap()),
            tx,
            ack: Arc::new(AtomicU64::new(0)),
            sample_rate: Arc::new(AtomicU32::new(44100)),
            tick: sequencer.playback_tick.clone(),
            transport: sequencer.transport_state.clone(),
            play_requested: AtomicBool::new(false),
            resources_pending_at: AtomicU64::new(0),
            error: Arc::new(Mutex::new(None)),
            master_scope: Default::default(),
            base: Mutex::new(std::env::temp_dir()),
            legacy: std::env::temp_dir(),
        };
        let send = |command, revision| {
            service
                .execute_at(
                    &Request {
                        version: 1,
                        operation_id: operation_id(),
                        expected_revision: Some(revision),
                        command,
                    },
                    None,
                    true,
                )
                .unwrap()
        };
        let worker = std::thread::spawn(|| sequencer.run());
        service
            .tx
            .send(SequencerCommand::LoadProject(project))
            .unwrap();
        assert!(send(Command::Play, 0).error.is_none());
        let edit = Command::Batch {
            label: "Agent: new bass instrument".into(),
            edits: vec![Edit::Replace { project: changed }],
        };
        let blocked = send(edit.clone(), 0);
        assert_eq!(blocked.error.unwrap().code, "resources_require_pause");
        assert_eq!(blocked.revision, 0);

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut kicks = 0;
        while kicks < 3 {
            let event = audio.recv_deadline(deadline).expect("drum loop kept going");
            match event {
                AudioEvent::NoteOn {
                    strip: 0,
                    pitch: 36,
                    ..
                } => kicks += 1,
                AudioEvent::LoadStrips { .. } if kicks > 0 => {
                    panic!("instrument load reached the rolling audio relay")
                }
                AudioEvent::Applied { revision: 1, .. } => panic!("blocked edit reached playback"),
                _ => {}
            }
        }
        assert!(send(Command::Pause, 0).error.is_none());
        while service.transport.load(Ordering::Acquire) != TRANSPORT_STOPPED {
            assert!(
                std::time::Instant::now() < deadline,
                "transport did not pause"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(send(edit, 0).error.is_none());
        // Withhold the audio callback's acknowledgement to model a slow load.
        assert_eq!(
            send(Command::Play, 1).error.unwrap().code,
            "resources_loading"
        );
        loop {
            if let AudioEvent::Applied { revision: 1, ack } = audio.recv_deadline(deadline).unwrap()
            {
                ack.store(1, Ordering::Release);
                break;
            }
        }
        assert!(send(Command::Play, 1).error.is_none());
        service.tx.send(SequencerCommand::Quit).unwrap();
        worker.join().unwrap();
    }
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
            play_requested: AtomicBool::new(false),
            resources_pending_at: AtomicU64::new(0),
            error: Arc::new(Mutex::new(None)),
            master_scope: Default::default(),
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
            .execute_at(&request, Some(Path::new("/new/project")), false)
            .unwrap()
            .error
            .is_some());
        assert_eq!(*service.base.lock().unwrap(), PathBuf::from("/old/project"));
        request.expected_revision = Some(0);
        assert!(service
            .execute_at(&request, Some(Path::new("/new/project")), false)
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
            play_requested: AtomicBool::new(false),
            resources_pending_at: AtomicU64::new(0),
            error: Arc::new(Mutex::new(None)),
            master_scope: Default::default(),
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

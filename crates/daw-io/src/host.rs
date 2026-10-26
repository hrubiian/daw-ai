use crate::config::UserConfig;
use crate::midi::{connect_midi, connect_midi_port, MidiEvent, MidiHandle};
use crate::midi_map::MidiMap;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use daw_core::audio::input::CaptureStream;
use daw_core::audio::{AudioEvent, RecordController, RecordState, StripMeter};
use daw_core::project::Project;
use daw_core::sequencer::{Sequencer, SequencerCommand};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU32, AtomicU8, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

/// Everything a frontend needs to talk to the running engine.
///
/// `Host` owns the live `midir` connection (`_midi_handle`); keep it alive for the
/// duration of the program. The channel/`Arc` fields are cheap to clone for the
/// frontend's own bookkeeping.
pub struct Host {
    pub session: Option<crate::session::LiveSession>,
    pub engine_error: Arc<Mutex<Option<String>>>,
    pub cmd_tx: Sender<SequencerCommand>,
    pub audio_ev_tx: Sender<AudioEvent>,
    pub playback_tick: Arc<AtomicU32>,
    pub vis_buf: Arc<Mutex<Vec<f32>>>,
    pub record_ctrl: RecordController,
    pub midi_ev_rx: Receiver<MidiEvent>,
    pub midi_channel: Arc<AtomicU8>,
    pub midi_device: Option<String>,
    pub midi_map: Arc<MidiMap>,
    /// Persistent MIDI sender — the receiver (`midi_ev_rx`) remains valid across
    /// reconnects because it uses the same channel.
    midi_tx: Sender<MidiEvent>,
    _midi_handle: Option<MidiHandle>,
    /// Output sample rate populated by the audio relay thread after engine init.
    pub sample_rate: Arc<AtomicU32>,
    /// Sequencer transport state (Stopped / CountIn / Rolling).
    pub transport_state: Arc<AtomicU8>,
    /// Playhead tick at the moment the transport last started rolling.
    pub roll_start_tick: Arc<AtomicU32>,
    /// Open input capture stream while an audio track is armed.
    pub capture: Option<CaptureStream>,
    /// Per-strip meters (index = track index), replaced on every strip build.
    pub strip_meters: Arc<Mutex<Vec<Arc<StripMeter>>>>,
    /// Master-bus meter (post-limiter).
    pub master_meter: Arc<StripMeter>,
    /// Return-bus meters: [0] = reverb, [1] = delay.
    pub return_meters: [Arc<StripMeter>; 2],
    /// (uid, plugin name, params) for each newly built CLAP instance.
    pub clap_meta_rx: Receiver<(String, String, Vec<daw_core::audio::clap::ClapParamInfo>)>,
    /// Monotonic take gate (odd = recording window open) — sequencer-owned.
    pub take_gate: Arc<AtomicU32>,
}

impl Host {
    /// Boots the audio engine thread, the sequencer thread, and MIDI input, then loads
    /// `project`. `sf2_path` is the soundfont to load; if it does not exist the engine
    /// thread is replaced by a drain so the rest of the app still runs silently.
    /// `enable_input` opens the legacy in-engine input recorder (TUI `Ctrl+R`);
    /// the GPUI frontend passes `false` and uses the capture subsystem instead.
    pub fn start(project: &Project, sf2_path: PathBuf, enable_input: bool) -> Self {
        let engine_error = Arc::new(Mutex::new(None));
        // Recording state — shared with the audio callback.
        let record_state = Arc::new(Mutex::new(RecordState::default()));
        let record_ctrl = RecordController::new(Arc::clone(&record_state));

        // Visualization buffer — written by the audio callback, read by the frontend.
        let vis_buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(vec![0.0; 4096]));

        // Sample rate — stored by the relay thread after engine init.
        let sample_rate = Arc::new(AtomicU32::new(0));
        let audio_clock = Arc::new(daw_core::audio::scheduling::AudioClock::default());

        // Mixer meters — strips replaced on each build; master lives forever.
        let strip_meters: Arc<Mutex<Vec<Arc<StripMeter>>>> = Arc::new(Mutex::new(Vec::new()));
        let master_meter = Arc::new(StripMeter::new());
        let return_meters = [Arc::new(StripMeter::new()), Arc::new(StripMeter::new())];

        // Audio event channel. `audio_ready` fires once after engine init (Ok or Err)
        // so device-probe noise is printed before a TUI enters alternate-screen mode.
        let (audio_ev_tx, audio_ev_rx) = unbounded::<AudioEvent>();
        let (audio_ready_tx, audio_ready_rx) = bounded::<()>(1);
        // CLAP plumbing: param metadata to the UI; retired processors back
        // from the RT thread for main-thread teardown.
        let (clap_meta_tx, clap_meta_rx) = unbounded();
        let (clap_garbage_tx, clap_garbage_rx) = unbounded();

        if sf2_path.exists() {
            let sf2_clone = sf2_path.clone();
            let vis_buf_audio = Arc::clone(&vis_buf);
            let sample_rate_clone = Arc::clone(&sample_rate);
            let strip_meters_relay = Arc::clone(&strip_meters);
            let master_meter_engine = Arc::clone(&master_meter);
            let return_meters_engine = [Arc::clone(&return_meters[0]), Arc::clone(&return_meters[1])];
            let errors = engine_error.clone();
            let clock = audio_clock.clone();
            thread::spawn(move || {
                match daw_core::audio::AudioEngine::new(&sf2_clone, record_state, Some(vis_buf_audio), enable_input, master_meter_engine, return_meters_engine, clap_garbage_tx, clock) {
                    Ok(mut engine) => {
                        let out_sr = engine.sample_rate;
                        sample_rate_clone.store(out_sr, Ordering::Relaxed);
                        let _ = audio_ready_tx.send(());
                        // CLAP instance registry (this thread is the plugins'
                        // main thread): handles + (path, id) fingerprints.
                        let mut clap_handles: std::collections::HashMap<String, daw_core::audio::clap::ClapHandle> = Default::default();
                        let mut clap_fps: std::collections::HashMap<String, (String, String)> = Default::default();
                        let mut clap_params: std::collections::HashMap<String, Vec<(u32, f64)>> = Default::default();
                        while let Ok(ev) = audio_ev_rx.recv() {
                            // Tear down any processors the RT thread retired.
                            for (uid, proc_) in clap_garbage_rx.try_iter() {
                                if let Some(mut h) = clap_handles.remove(&uid) {
                                    h.deactivate(proc_);
                                }
                            }
                            let ev = match ev {
                                AudioEvent::Scheduled(event) => {
                                    engine.schedule_at = Some((event.frame, event.generation));
                                    event.event
                                }
                                other => { engine.schedule_at = None; other }
                            };
                            match ev {
                                AudioEvent::LoadClipFromFile { path, volume } => {
                                    match daw_core::audio::load_wav_stereo(
                                        std::path::Path::new(&path),
                                        out_sr,
                                    ) {
                                        Ok((l, r)) => engine.send(AudioEvent::LoadClip {
                                            left: l,
                                            right: r,
                                            volume,
                                        }),
                                        Err(e) => *errors.lock().unwrap() = Some(format!("clip load error {path}: {e}")),
                                    }
                                }
                                AudioEvent::LoadStrips { specs } => {
                                    *errors.lock().unwrap() = None;
                                    let (strips, meters, failures) = daw_core::audio::mixer::build_strips_report(
                                        &specs, &sf2_clone, out_sr,
                                    );
                                    if !failures.is_empty() { *errors.lock().unwrap() = Some(failures.join("; ")); }
                                    if let Ok(mut m) = strip_meters_relay.lock() {
                                        *m = meters;
                                    }
                                    engine.send(AudioEvent::SetStrips { strips });
                                }
                                AudioEvent::LoadAudioClips { specs } => {
                                    let mut loaded = Vec::new();
                                    for (id, path) in specs {
                                        match daw_core::audio::load_wav_stereo(
                                            std::path::Path::new(&path),
                                            out_sr,
                                        ) {
                                            Ok((l, r)) => loaded.push((id, Arc::new(l), Arc::new(r))),
                                            Err(e) => *errors.lock().unwrap() = Some(format!("audio clip load error {path}: {e}")),
                                        }
                                    }
                                    engine.send(AudioEvent::SetAudioClips { clips: loaded });
                                }
                                AudioEvent::LoadClapInstances { specs } => {
                                    let desired: std::collections::HashMap<String, (String, String)> = specs
                                        .iter()
                                        .map(|sp| (sp.uid.clone(), (sp.path.clone(), sp.id.clone())))
                                        .collect();
                                    // Retire gone/changed instances (processor
                                    // comes back via the garbage channel).
                                    let stale: Vec<String> = clap_fps
                                        .iter()
                                        .filter(|(uid, fp)| desired.get(*uid) != Some(fp))
                                        .map(|(uid, _)| uid.clone())
                                        .collect();
                                    for uid in stale {
                                        clap_fps.remove(&uid);
                                        clap_params.remove(&uid);
                                        engine.send(AudioEvent::RemoveClapInstance { uid });
                                    }
                                    // Build the new ones.
                                    for spec in specs {
                                        if clap_fps.contains_key(&spec.uid) {
                                            let previous = clap_params.get(&spec.uid).map(Vec::as_slice).unwrap_or(&[]);
                                            if let Some(handle) = clap_handles.get(&spec.uid) {
                                                for (param_id, value) in daw_core::audio::clap::parameter_updates(previous, &spec.params, &handle.param_infos) {
                                                    engine.send(AudioEvent::SetClapParam { uid: spec.uid.clone(), param_id, value });
                                                }
                                            }
                                            clap_params.insert(spec.uid.clone(), spec.params.clone());
                                            continue;
                                        }
                                        match daw_core::audio::clap::ClapInstance::from_spec(&spec, out_sr) {
                                            Ok(mut inst) => {
                                                let processor = inst.processor.take().unwrap();
                                                let _ = clap_meta_tx.try_send((
                                                    spec.uid.clone(),
                                                    inst.handle.name.clone(),
                                                    inst.handle.param_infos.clone(),
                                                ));
                                                clap_params.insert(spec.uid.clone(), spec.params.clone());
                                                clap_fps.insert(spec.uid.clone(), (spec.path.clone(), spec.id.clone()));
                                                clap_handles.insert(spec.uid.clone(), inst.handle);
                                                engine.send(AudioEvent::AddClapInstance { uid: spec.uid, processor });
                                            }
                                            Err(e) => { *errors.lock().unwrap() = Some(format!("plugin {}: {e}", spec.uid)); },
                                        }
                                    }
                                }
                                AudioEvent::CollectClapState { reply } => {
                                    let blobs = clap_handles
                                        .iter_mut()
                                        .filter_map(|(uid, h)| h.collect_state().map(|b| (uid.clone(), b)))
                                        .collect();
                                    let _ = reply.send(blobs);
                                }
                                other => engine.send(other),
                            }
                        }
                    }
                    Err(e) => {
                        *errors.lock().unwrap() = Some(format!("audio engine init failed: {e}"));
                        eprintln!("audio engine init failed: {e}");
                        let _ = audio_ready_tx.send(());
                        while audio_ev_rx.recv().is_ok() {}
                    }
                }
            });
        } else {
            *engine_error.lock().unwrap() = Some(format!("soundfont missing: {}", sf2_path.display()));
            // No soundfont → no audio engine. This is the silent-Play trap when
            // launched from a desktop icon with the wrong working directory.
            eprintln!(
                "warning: soundfont not found at {} (cwd: {}); audio engine disabled — \
                 playback will be silent. Run from the repo root or set the launcher's Path=.",
                sf2_path.display(),
                std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_else(|_| "?".into()),
            );
            thread::spawn(move || while audio_ev_rx.recv().is_ok() {});
            let _ = audio_ready_tx.send(());
        }

        // Sequencer.
        let (cmd_tx, cmd_rx) = unbounded::<SequencerCommand>();
        let mut seq = Sequencer::new(cmd_rx, audio_ev_tx.clone());
        seq.audio_clock = Some(audio_clock);
        let playback_tick: Arc<AtomicU32> = Arc::clone(&seq.playback_tick);
        let transport_state = Arc::clone(&seq.transport_state);
        let take_gate = Arc::clone(&seq.take_gate);
        let roll_start_tick = Arc::clone(&seq.roll_start_tick);
        thread::spawn(move || seq.run());
        let _ = cmd_tx.try_send(SequencerCommand::LoadProject(project.clone()));

        // MIDI map + input. The audio sender goes straight into the midir callback for
        // zero-latency note playback.
        let midi_map = Arc::new(MidiMap::load_or_create());
        let midi_channel = Arc::new(AtomicU8::new(0));
        let (midi_ev_rx, midi_tx, midi_handle, midi_device) = connect_midi(
            audio_ev_tx.clone(),
            Arc::clone(&midi_channel),
            Arc::clone(&midi_map),
        );

        // Wait for audio init (5s safety timeout) so device-probe noise prints first.
        let _ = audio_ready_rx.recv_timeout(Duration::from_secs(5));

        Host {
            session: None,
            engine_error,
            cmd_tx,
            audio_ev_tx,
            playback_tick,
            vis_buf,
            record_ctrl,
            midi_ev_rx,
            midi_channel,
            midi_device,
            midi_map,
            midi_tx,
            _midi_handle: midi_handle,
            sample_rate,
            transport_state,
            roll_start_tick,
            capture: None,
            strip_meters,
            master_meter,
            return_meters,
            clap_meta_rx,
            take_gate,
        }
    }

    /// Round-trip state collection from live CLAP instances (relay thread).
    /// Returns whatever arrived before `timeout` (empty on timeout/no engine).
    pub fn collect_clap_state(&self, timeout: std::time::Duration) -> Vec<(String, Vec<u8>)> {
        let (tx, rx) = bounded(1);
        let _ = self.audio_ev_tx.try_send(AudioEvent::CollectClapState { reply: tx });
        rx.recv_timeout(timeout).unwrap_or_default()
    }

    /// Open the input capture stream per `cfg` (idempotent while armed).
    pub fn arm_input(&mut self, cfg: &UserConfig) -> anyhow::Result<&CaptureStream> {
        if self.capture.is_none() {
            let cs = CaptureStream::open(
                cfg.input_device.as_deref(),
                cfg.input_channels,
                Arc::clone(&self.take_gate),
                Arc::clone(&self.roll_start_tick),
            )?;
            self.capture = Some(cs);
        }
        Ok(self.capture.as_ref().expect("just set"))
    }

    /// Close the capture stream (finalizes any open take via Drop).
    pub fn disarm_input(&mut self) {
        self.capture = None;
    }

    /// Switch the live MIDI input to port `idx`. Returns `false` if it can't connect.
    pub fn set_midi_port(&mut self, idx: usize) -> bool {
        // Drop the old connection before opening a new one on the same port.
        self._midi_handle = None;
        match connect_midi_port(
            idx,
            self.audio_ev_tx.clone(),
            self.midi_tx.clone(),
            Arc::clone(&self.midi_channel),
            Arc::clone(&self.midi_map),
        ) {
            Some((handle, name)) => {
                self._midi_handle = Some(handle);
                self.midi_device = Some(name);
                true
            }
            None => false,
        }
    }
}

#[cfg(feature = "test-support")]
impl Host {
    /// Device-free host for frontend integration tests. Receivers let tests
    /// inspect commands while keeping the simulated engine connected.
    pub fn headless() -> (Self, Receiver<SequencerCommand>, Receiver<AudioEvent>) {
        let (cmd_tx, commands) = unbounded();
        let (audio_ev_tx, audio) = unbounded();
        let (midi_tx, midi_ev_rx) = unbounded();
        let (_, clap_meta_rx) = unbounded();
        (Self {
            session: None,
            engine_error: Default::default(),
            cmd_tx, audio_ev_tx,
            playback_tick: Default::default(),
            vis_buf: Arc::new(Mutex::new(vec![0.; 512])),
            record_ctrl: RecordController::new(Default::default()),
            midi_ev_rx, midi_channel: Default::default(), midi_device: None,
            midi_map: Arc::new(MidiMap::default()), midi_tx, _midi_handle: None,
            sample_rate: Arc::new(AtomicU32::new(44100)),
            transport_state: Default::default(), roll_start_tick: Default::default(),
            capture: None, strip_meters: Default::default(),
            master_meter: Default::default(), return_meters: Default::default(),
            clap_meta_rx, take_gate: Default::default(),
        }, commands, audio)
    }
}

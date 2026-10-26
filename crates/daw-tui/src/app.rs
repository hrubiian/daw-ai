use crate::midi::MidiEvent;
use crate::midi_map::{ButtonAction, FaderParam, KnobParam, MidiMap};
use crossbeam_channel::Sender;
use daw_core::audio::RecordController;
use daw_core::project::{ArrangementSlot, Instrument, Note, Pattern, Project, TrackType};
use daw_core::sequencer::SequencerCommand;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU8, AtomicU32, Ordering},
    Arc, Mutex,
};

#[derive(Debug, Clone, PartialEq)]
pub enum DialogMode { None, SaveAs, ExportWav }

#[derive(Debug, Clone, PartialEq)]
pub enum Tab {
    Tracks,
    Sequencer,
    PianoRoll,
    Mixer,
    Browser,
    Effects,
    Arrange,
    Viz,
    Code,
}

impl Tab {
    pub fn label(&self) -> &'static str {
        match self {
            Tab::Tracks    => "1 Tracks",
            Tab::Sequencer => "2 Sequencer",
            Tab::PianoRoll => "3 Piano Roll",
            Tab::Mixer     => "4 Mixer",
            Tab::Browser   => "5 Browser",
            Tab::Effects   => "6 FX",
            Tab::Arrange   => "7 Arrange",
            Tab::Viz       => "8 Viz",
            Tab::Code      => "9 Code",
        }
    }
    pub fn all() -> [Tab; 9] {
        [Tab::Tracks, Tab::Sequencer, Tab::PianoRoll, Tab::Mixer, Tab::Browser, Tab::Effects, Tab::Arrange, Tab::Viz, Tab::Code]
    }
}

pub struct App {
    pub session: Option<daw_io::session::LiveSession>,
    pub export_control: Option<std::sync::Arc<daw_core::render_control::RenderControl>>,
    export_result: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    pub active_tab:    Tab,
    pub project:       Project,
    pub project_path:  Option<std::path::PathBuf>,
    pub file_revision: Option<String>,
    pub saved_hash: String,
    pub is_playing:    bool,
    pub seq_tx:        Sender<SequencerCommand>,
    pub should_quit:   bool,
    pub active_track:  usize,
    pub seq_row:       usize,
    pub seq_col:       usize,
    pub seq_pat:       usize,   // which of the active track's patterns is being edited
    pub pr_pitch:      u8,
    pub pr_tick:       u32,
    pub mixer_track:   usize,
    pub browser_left:  usize,
    pub browser_right: usize,
    pub rename_mode:   bool,
    pub rename_buf:    String,
    pub record_ctrl:   RecordController,
    pub is_recording:  bool,
    pub effects_param: usize,
    pub mod_sel:        usize,
    pub live_play:      bool,
    pub dialog_mode:    DialogMode,
    pub dialog_buf:     String,
    pub dialog_status:  String,
    pub waveform_cache: HashMap<String, Vec<f32>>,
    pub arr_track:      usize,
    pub arr_bar:        usize,
    pub midi_ev_rx:     crossbeam_channel::Receiver<MidiEvent>,
    pub midi_channel:   Arc<AtomicU8>,
    pub midi_device:    Option<String>,
    pub midi_map:       Arc<MidiMap>,
    pub midi_last_cc:   Option<(u8, u8)>,
    // Piano-roll real-time MIDI recording
    pub playback_tick:        Arc<AtomicU32>,
    pub pr_midi_record:       bool,
    pub pr_record_seq_start:  u32,
    pub pr_record_pat_start:  u32,
    pub pr_pending_notes:     HashMap<u8, (u32, u8)>,  // pitch → (on_tick, velocity)
    // Hardware MIDI CC positions (0-127), initialised to midpoint
    pub knob_values:          HashMap<KnobParam, u8>,
    pub fader_values:         HashMap<FaderParam, u8>,
    // Visualization — shared with audio thread
    pub vis_buf:              Arc<Mutex<Vec<f32>>>,
    pub vis_snapshot:         Vec<f32>,
    // JAM mode — hot-reload the project file when it changes on disk
    // (lets Claude / an external editor re-pattern the music while it plays).
    pub jam_mode:             bool,
    pub project_mtime:        Option<std::time::SystemTime>,
    pub last_reload_check:    std::time::Instant,
    pub jam_status:           String,
    // Live-code (mini-notation) REPL pane
    pub repl_target:          usize,                    // track index the buffer targets
    pub repl_bufs:            HashMap<String, String>,  // track id → mini-notation buffer
    pub repl_status:          String,
}

/// File modification time, or `None` if it can't be read.
fn file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl App {
    pub fn new(
        project: Project,
        project_path: Option<std::path::PathBuf>,
        seq_tx: Sender<SequencerCommand>,
        record_ctrl: RecordController,
        midi_ev_rx: crossbeam_channel::Receiver<MidiEvent>,
        midi_channel: Arc<AtomicU8>,
        midi_device: Option<String>,
        midi_map: Arc<MidiMap>,
        playback_tick: Arc<AtomicU32>,
        vis_buf: Arc<Mutex<Vec<f32>>>,
    ) -> Self {
        let file_revision = project_path.as_ref().and_then(|path| daw_core::project::format::load_versioned(path).ok().map(|(disk, revision)| {
            if daw_core::project::format::project_revision(&disk).ok() == daw_core::project::format::project_revision(&project).ok() { revision } else { "stale-loaded-project".into() }
        }));
        let saved_hash = daw_core::project::format::project_revision(&project).unwrap_or_default();
        App {
            session: None,
            export_control: None,
            export_result: Default::default(),
            active_tab: Tab::Tracks,
            project,
            project_path,
            file_revision,
            saved_hash,
            is_playing: false,
            seq_tx,
            should_quit: false,
            active_track: 0,
            seq_row: 0,
            seq_col: 0,
            seq_pat: 0,
            pr_pitch: 60,
            pr_tick: 0,
            mixer_track: 0,
            browser_left: 0,
            browser_right: 0,
            rename_mode: false,
            rename_buf: String::new(),
            record_ctrl,
            is_recording: false,
            effects_param: 0,
            mod_sel: 0,
            live_play: false,
            dialog_mode: DialogMode::None,
            dialog_buf: String::new(),
            dialog_status: String::new(),
            waveform_cache: HashMap::new(),
            arr_track: 0,
            arr_bar:   0,
            midi_ev_rx,
            midi_channel,
            midi_device,
            midi_map,
            midi_last_cc: None,
            playback_tick,
            pr_midi_record:      false,
            pr_record_seq_start: 0,
            pr_record_pat_start: 0,
            pr_pending_notes:    HashMap::new(),
            knob_values:         HashMap::new(),
            fader_values:        HashMap::new(),
            vis_buf,
            vis_snapshot:        Vec::new(),
            jam_mode:            false,
            project_mtime:       None,
            last_reload_check:   std::time::Instant::now(),
            jam_status:          String::new(),
            repl_target:         0,
            repl_bufs:           HashMap::new(),
            repl_status:         String::new(),
        }
    }

    pub fn toggle_metronome(&mut self) {
        self.project.project.metronome = !self.project.project.metronome;
        self.send(SequencerCommand::SetMetronome(self.project.project.metronome));
    }

    pub fn active_track_id(&self) -> Option<&str> {
        self.project.tracks.get(self.active_track).map(|t| t.id.as_str())
    }

    pub fn send(&self, cmd: SequencerCommand) {
        if let Some(session) = &self.session {
            let command = match &cmd {
                SequencerCommand::Play => Some(daw_core::session::Command::Play),
                SequencerCommand::Pause => Some(daw_core::session::Command::Pause),
                SequencerCommand::Seek(tick) => Some(daw_core::session::Command::Seek { tick: *tick }),
                _ => None,
            };
            if let Some(command) = command {
                if let Err(e) = session.transport(command) { eprintln!("session transport: {e}"); }
                return;
            }
        }
        if let Some(session) = &self.session {
            if daw_io::session::is_project_edit(&cmd) {
                if let Err(e) = session.submit(&self.project) { eprintln!("session edit: {e}"); }
                return;
            }
        }
        let _ = self.seq_tx.try_send(cmd);
    }

    pub fn toggle_play(&mut self) {
        if self.is_playing {
            self.is_playing = false;
            self.send(SequencerCommand::Pause);
        } else {
            self.is_playing = true;
            self.send(SequencerCommand::Play);
        }
    }

    pub fn toggle_recording(&mut self) {
        let Some(track) = self.project.tracks.get(self.active_track) else { return };
        if track.track_type != TrackType::Audio {
            return; // only audio tracks can record
        }

        if self.is_recording {
            self.is_recording = false;
            let track_id = self.project.tracks.get(self.active_track)
                .map(|t| t.id.clone())
                .unwrap_or_default();
            // Sanitise track_id to prevent path traversal from crafted project files
            let safe_id: String = track_id.chars()
                .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
                .collect();
            let path = format!("recordings/{safe_id}.wav");
            if self.record_ctrl.stop(std::path::Path::new(&path)).is_ok() {
                if let Some(t) = self.project.tracks.get_mut(self.active_track) {
                    t.instrument = Instrument::AudioFile { path: path.clone() };
                }
                self.cache_waveform(&track_id, &path);
                self.send(SequencerCommand::UpdateLive(self.project.clone()));
            }
        } else if self.record_ctrl.available() {
            self.record_ctrl.start();
            self.is_recording = true;
        }
    }

    pub fn transpose(&mut self, semitones: i32) {
        for pattern in &mut self.project.patterns {
            if let daw_core::project::Pattern::PianoRoll(pr) = pattern {
                for note in &mut pr.notes {
                    note.pitch = ((note.pitch as i32 + semitones).clamp(0, 127)) as u8;
                }
            }
        }
        const KEYS: [&str; 12] = ["C","C#","D","D#","E","F","F#","G","G#","A","A#","B"];
        let idx = KEYS.iter().position(|&k| k == self.project.project.key.as_str()).unwrap_or(0);
        self.project.project.key = KEYS[((idx as i32 + semitones).rem_euclid(12)) as usize].to_string();
        self.send(SequencerCommand::UpdateLive(self.project.clone()));
    }

    pub fn live_play_note(&mut self, pitch: u8) {
        let Some(track) = self.project.tracks.get(self.active_track) else { return };
        if track.track_type == TrackType::Audio { return; }

        let track_id = track.id.clone();
        let bpm = self.project.project.bpm;
        let dur_ms = ((60000.0 / bpm / 4.0) as u64).max(80); // ~16th note

        self.send(SequencerCommand::PreviewNote { track_id: track_id.clone(), pitch, velocity: 90 });

        // Schedule NoteOff from a lightweight background thread (minimal stack)
        let tx = self.seq_tx.clone();
        let tid = track_id.clone();
        let _ = std::thread::Builder::new()
            .stack_size(32 * 1024)
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(dur_ms));
                let _ = tx.try_send(SequencerCommand::PreviewNoteOff { track_id: tid, pitch });
            });

        if self.active_tab == Tab::PianoRoll {
            self.midi_step_record(pitch, 90);
        }
    }

    pub fn cache_waveform(&mut self, track_id: &str, path: &str) {
        if path.is_empty() { return; }
        if let Ok((left, _right)) = daw_core::audio::load_wav_stereo(
            std::path::Path::new(path), 0
        ) {
            self.waveform_cache.insert(track_id.to_string(), left);
        }
    }

    pub fn refresh_all_waveforms(&mut self) {
        let runtime = self.session.as_ref().map(|s| s.runtime_project(&self.project)).unwrap_or_else(|| self.project.clone());
        let pairs: Vec<(String, String)> = runtime.tracks.iter()
            .filter_map(|t| {
                if let Instrument::AudioFile { path } = &t.instrument {
                    if !path.is_empty() { return Some((t.id.clone(), path.clone())); }
                }
                None
            })
            .collect();
        for (id, path) in pairs {
            self.cache_waveform(&id, &path);
        }
    }

    pub fn adjust_swing(&mut self, delta: f32) {
        let sw = (self.project.project.swing + delta).clamp(0.0, 1.0);
        self.project.project.swing = sw;
        self.send(SequencerCommand::SetSwing(sw));
    }

    /// Compile the target track's mini-notation buffer and apply it to that
    /// track's first pattern (same id + bar count, so the arrangement keeps
    /// working), pushed live and bar-quantized via UpdateLive.
    pub fn repl_eval(&mut self) {
        let Some(track) = self.project.tracks.get(self.repl_target) else { return };
        let track_id = track.id.clone();
        let buf = self.repl_bufs.get(&track_id).cloned().unwrap_or_default();
        if buf.trim().is_empty() { self.repl_status = "empty — type some mini-notation".into(); return; }

        let (pid, bars) = match track.patterns.first().cloned() {
            Some(pid) => {
                let bars = self.project.patterns.iter()
                    .find(|p| p.id() == pid).map(|p| p.bars() as u8).unwrap_or(1);
                (pid, bars.max(1))
            }
            None => (format!("live_{track_id}"), 1),
        };

        match daw_core::mini_notation::compile(&buf, &pid, "live", bars, 480, 100) {
            Ok(pat) => {
                let n = pat.notes.len();
                let pattern = daw_core::project::Pattern::PianoRoll(pat);
                if let Some(slot) = self.project.patterns.iter_mut().find(|p| p.id() == pid) {
                    *slot = pattern;
                } else {
                    self.project.patterns.push(pattern);
                    if let Some(t) = self.project.tracks.get_mut(self.repl_target) {
                        if !t.patterns.iter().any(|x| x == &pid) { t.patterns.push(pid.clone()); }
                    }
                }
                self.repl_status = format!("✓ {n} notes → {track_id} ({bars} bar)");
                self.send(SequencerCommand::UpdateLive(self.project.clone()));
            }
            Err(e) => self.repl_status = format!("✗ {e}"),
        }
    }

    /// Cycle velocity humanization 0 → 0.25 → 0.5 → 0.75 → 1.0 → 0.
    pub fn cycle_humanize(&mut self) {
        let next = ((self.project.project.humanize * 4.0).round() as i32 + 1) % 5;
        self.project.project.humanize = next as f32 / 4.0;
        self.send(SequencerCommand::UpdateLive(self.project.clone()));
    }

    /// Returns the mixer strip index (= track index) for the active track, or
    /// `None` for audio tracks (live MIDI has nothing to play there).
    pub fn active_midi_channel(&self) -> Option<u8> {
        let track = self.project.tracks.get(self.active_track)?;
        if track.track_type == daw_core::project::TrackType::Audio {
            return None;
        }
        Some(self.active_track as u8)
    }

    pub fn handle_midi_button(&mut self, action: ButtonAction) {
        match action {
            ButtonAction::RecordToggle => self.toggle_recording(),
            ButtonAction::PlayPause    => self.toggle_play(),
            ButtonAction::Panic        => self.send(SequencerCommand::Panic),
            ButtonAction::TrackNext    => {
                if !self.project.tracks.is_empty() {
                    self.active_track = (self.active_track + 1)
                        .min(self.project.tracks.len() - 1);
                }
            }
            ButtonAction::TrackPrev => {
                self.active_track = self.active_track.saturating_sub(1);
            }
            ButtonAction::TabBrowser => {
                self.active_tab = Tab::Browser;
            }
            ButtonAction::EncoderLeft => {
                self.active_track = self.active_track.saturating_sub(1);
            }
            ButtonAction::EncoderRight => {
                if !self.project.tracks.is_empty() {
                    self.active_track = (self.active_track + 1)
                        .min(self.project.tracks.len() - 1);
                }
            }
        }
    }

    /// Writes the active track's strip index into the shared atomic so the
    /// midir callback routes live MIDI to the selected track's strip.
    pub fn sync_midi_channel(&self) {
        if let Some(ch) = self.active_midi_channel() {
            self.midi_channel.store(ch, Ordering::Relaxed);
        }
    }

    /// Writes a note into the active piano-roll pattern at the current step
    /// position. Called for both keyboard live play and MIDI keyboard input.
    pub fn midi_step_record(&mut self, pitch: u8, velocity: u8) {
        let pattern_id = self.project.tracks.get(self.active_track)
            .and_then(|t| t.patterns.first())
            .cloned();
        if let Some(pid) = pattern_id {
            if let Some(pidx) = self.project.patterns.iter().position(|p| p.id() == pid) {
                if let Pattern::PianoRoll(pr) = &mut self.project.patterns[pidx] {
                    let tick = self.pr_tick;
                    if !pr.notes.iter().any(|n| n.pitch == pitch && n.tick == tick) {
                        pr.notes.push(Note { pitch, tick, duration: 120, vel: velocity });
                        pr.notes.sort_by_key(|n| (n.tick, n.pitch));
                    }
                    self.pr_tick += 120;
                    self.send(SequencerCommand::UpdateLive(self.project.clone()));
                }
            }
        }
    }

    // ── Real-time MIDI recording into piano roll ──────────────────────────────

    fn active_pr_pattern_len(&self) -> u32 {
        let pid = self.project.tracks.get(self.active_track)
            .and_then(|t| t.patterns.first())
            .cloned();
        let Some(pid) = pid else { return 0 };
        match self.project.patterns.iter().find(|p| p.id() == pid) {
            Some(Pattern::PianoRoll(pr)) => pr.ticks_per_bar * pr.bars as u32,
            _ => 0,
        }
    }

    fn pr_current_record_tick(&self) -> u32 {
        let seq_elapsed = self.playback_tick.load(Ordering::Relaxed)
            .wrapping_sub(self.pr_record_seq_start);
        let len = self.active_pr_pattern_len();
        if len == 0 { return self.pr_record_pat_start; }
        (self.pr_record_pat_start + seq_elapsed) % len
    }

    fn pr_commit_note(&mut self, pitch: u8, tick: u32, duration: u32, velocity: u8) {
        let pid = self.project.tracks.get(self.active_track)
            .and_then(|t| t.patterns.first())
            .cloned();
        let Some(pid) = pid else { return };
        if let Some(pidx) = self.project.patterns.iter().position(|p| p.id() == pid) {
            if let Pattern::PianoRoll(pr) = &mut self.project.patterns[pidx] {
                if !pr.notes.iter().any(|n| n.pitch == pitch && n.tick == tick) {
                    pr.notes.push(Note { pitch, tick, duration: duration.max(30), vel: velocity });
                    pr.notes.sort_by_key(|n| (n.tick, n.pitch));
                }
            }
        }
    }

    /// Toggle real-time MIDI recording. Only activates while the sequencer is playing.
    pub fn toggle_pr_midi_record(&mut self) {
        if self.pr_midi_record {
            let current = self.pr_current_record_tick();
            let len     = self.active_pr_pattern_len();
            let pending: Vec<(u8, u32, u8)> = self.pr_pending_notes
                .iter().map(|(&p, &(t, v))| (p, t, v)).collect();
            for (pitch, on_tick, velocity) in pending {
                let duration = if current >= on_tick {
                    current - on_tick
                } else {
                    len.saturating_sub(on_tick) + current
                };
                self.pr_commit_note(pitch, on_tick, duration, velocity);
            }
            self.pr_pending_notes.clear();
            self.pr_midi_record = false;
            self.send(SequencerCommand::UpdateLive(self.project.clone()));
        } else if self.is_playing && self.active_pr_pattern_len() > 0 {
            self.pr_record_seq_start = self.playback_tick.load(Ordering::Relaxed);
            self.pr_record_pat_start = 0;
            self.pr_midi_record = true;
        }
    }

    /// Called on MIDI NoteOn while real-time record is active.
    pub fn pr_note_on_record(&mut self, pitch: u8, velocity: u8) {
        let tick = self.pr_current_record_tick();
        self.pr_pending_notes.insert(pitch, (tick, velocity));
    }

    /// Called on MIDI NoteOff while real-time record is active.
    pub fn pr_note_off_record(&mut self, pitch: u8) {
        let Some((on_tick, velocity)) = self.pr_pending_notes.remove(&pitch) else { return };
        let off_tick = self.pr_current_record_tick();
        let len      = self.active_pr_pattern_len();
        let duration = if off_tick >= on_tick {
            off_tick - on_tick
        } else {
            len.saturating_sub(on_tick) + off_tick
        };
        self.pr_commit_note(pitch, on_tick, duration, velocity);
        self.send(SequencerCommand::UpdateLive(self.project.clone()));
    }

    pub fn apply_effects(&self) {
        if let Some(track) = self.project.tracks.get(self.active_track) {
            self.send(SequencerCommand::SetTrackEffects {
                track_id: track.id.clone(),
                effects:  track.effects.clone(),
            });
        }
    }

    /// Called when a hardware knob CC arrives (from MIDI input or keyboard).
    /// Updates the stored position and syncs overlapping EffectChain fields for display.
    pub fn on_knob_cc(&mut self, knob: KnobParam, value: u8) {
        self.knob_values.insert(knob, value);
        let v = value as f32 / 127.0;
        if let Some(track) = self.project.tracks.get_mut(self.active_track) {
            match knob {
                KnobParam::Cutoff    => { track.effects.cutoff    = v; }
                KnobParam::Resonance => { track.effects.resonance = v; }
                KnobParam::Chorus    => { track.effects.chorus    = v; }
                KnobParam::LfoAmt    => { track.effects.lfo_depth = v; }
                _ => {}
            }
        }
    }

    /// Called when a hardware fader CC arrives (from MIDI input or keyboard).
    /// Updates the stored position; Volume fader also updates track.volume.
    pub fn on_fader_cc(&mut self, fader: FaderParam, value: u8) {
        self.fader_values.insert(fader, value);
        if fader == FaderParam::Volume {
            let v = value as f32 / 127.0;
            if let Some(track) = self.project.tracks.get_mut(self.active_track) {
                track.volume = v;
                let (id, vol) = (track.id.clone(), track.volume);
                self.send(SequencerCommand::SetTrackVolume { track_id: id, volume: vol });
            }
        }
    }

    /// Save to current path, or open Save-As dialog if none is set.
    pub fn save_or_prompt(&mut self) {
        if self.project_path.is_some() {
            if let Err(e) = self.save() {
                self.dialog_status = format!("Save error: {e}");
            } else {
                self.dialog_status = "Saved.".into();
            }
        } else {
            self.dialog_buf = "project.daw.json".into();
            self.dialog_mode = DialogMode::SaveAs;
        }
    }

    /// Toggle JAM mode. When enabled, the app watches the project file and
    /// hot-reloads it whenever it changes on disk — so Claude (or any editor)
    /// can re-pattern the music while it keeps playing.
    pub fn toggle_jam(&mut self) {
        self.jam_mode = !self.jam_mode;
        if self.jam_mode {
            // Arm from the current file state so we only react to later edits.
            self.project_mtime = self.project_path.as_ref().and_then(|p| file_mtime(p));
            self.last_reload_check = std::time::Instant::now();
            self.jam_status = "JAM armed — edit the .daw.json to re-pattern live".into();
        } else {
            self.jam_status = "JAM off".into();
        }
    }

    /// Poll the project file (throttled) and hot-reload on external change.
    /// Called from the event loop. Uses `UpdateLive`, so the playhead keeps
    /// running and the swap lands on the next bar.
    pub fn poll_hot_reload(&mut self) {
        if let Some(message) = self.export_result.lock().ok().and_then(|mut r| r.take()) {
            self.export_control = None;
            self.dialog_status = message;
        }
        if let Some(session) = &self.session {
            if let Err(e) = session.sync(&mut self.project) { self.jam_status = e.to_string(); }
            if let Ok(state) = session.inspect() {
                self.is_playing = state.playing;
                if state.engine_status.starts_with("failed") || state.engine_status.starts_with("unavailable") { self.jam_status = state.engine_status; }
            }
        }
        if !self.jam_mode { return; }
        if self.last_reload_check.elapsed() < std::time::Duration::from_millis(300) { return; }
        self.last_reload_check = std::time::Instant::now();

        let Some(path) = self.project_path.clone() else { return; };
        let Some(mtime) = file_mtime(&path) else { return; };
        if Some(mtime) == self.project_mtime { return; }

        // File changed externally. It may be mid-write (partial/invalid JSON);
        // on parse error we leave state untouched and retry on the next poll.
        if daw_core::project::format::project_revision(&self.project).ok().as_deref() != Some(self.saved_hash.as_str()) {
            self.jam_status = "JAM conflict: local edits retained; save as a new file before loading external changes".into();
            return;
        }
        match daw_core::project::format::load_versioned(&path) {
            Ok((proj, revision)) => {
                self.file_revision = Some(revision);
                self.saved_hash = daw_core::project::format::project_revision(&proj).unwrap_or_default();
                self.project = proj;
                self.project_mtime = Some(mtime);
                // Live update keeps the playhead so the groove continues.
                self.send(SequencerCommand::UpdateLive(self.project.clone()));
                let n = self.project.tracks.len();
                self.jam_status = format!("⟳ reloaded ({n} tracks)");
            }
            Err(_) => { /* partial write — try again next tick */ }
        }
    }

    pub fn save(&mut self) -> anyhow::Result<()> {
        let path = self.project_path.clone()
            .ok_or_else(|| anyhow::anyhow!("no project path set"))?;
        let persisted = self.session.as_ref().filter(|s| Some(s.project_directory().as_path()) != path.parent()).map(|s| s.runtime_project(&self.project)).unwrap_or_else(|| self.project.clone());
        self.file_revision = Some(daw_core::project::format::save_if_unchanged(&persisted, &path, self.file_revision.as_deref())?);
        self.saved_hash = daw_core::project::format::project_revision(&self.project)?;
        // Remember our own write so JAM mode doesn't reload it as an external change.
        self.project_mtime = file_mtime(&path);
        Ok(())
    }

    pub fn confirm_save_as(&mut self) {
        let raw = self.dialog_buf.trim().to_string();
        let path = if raw.ends_with(".daw.json") || raw.ends_with(".json") {
            std::path::PathBuf::from(raw)
        } else {
            std::path::PathBuf::from(format!("{raw}.daw.json"))
        };
        let path = match daw_core::project::format::absolute(&path) {
            Ok(path) => path,
            Err(e) => { self.dialog_status = format!("Save error: {e}"); return; }
        };
        let project = self.session.as_ref().map(|s| s.runtime_project(&self.project)).unwrap_or_else(|| self.project.clone());
        match daw_core::project::format::save_if_unchanged(&project, &path, None) {
            Ok(revision)  => {
                if let Some(session) = &self.session {
                    if let Err(e) = session.import(&project, &path) {
                        self.dialog_status = format!("copy saved to {}; session conflict: {e}", path.display());
                        self.dialog_mode = DialogMode::None;
                        return;
                    }
                }
                self.project = project;
                self.file_revision = Some(revision);
                self.saved_hash = daw_core::project::format::project_revision(&self.project).unwrap_or_default();
                self.project_path  = Some(path.clone());
                self.project_mtime = file_mtime(&path);
                self.dialog_status = format!("Saved → {}", path.display());
            }
            Err(e) => { self.dialog_status = format!("Save error: {e}"); }
        }
        self.dialog_mode = DialogMode::None;
        self.dialog_buf.clear();
    }

    pub fn arr_place(&mut self) {
        if self.project.tracks.is_empty() { return; }
        let bar = self.arr_bar as u32;
        let Some(t) = self.project.tracks.get(self.arr_track) else { return };
        let track_id = t.id.clone();
        let track_patterns = t.patterns.clone();
        if track_patterns.is_empty() { return; }

        // Find what pattern is currently at this bar (if any)
        let existing_pat = self.project.arrangement.iter()
            .find(|s| s.track == track_id && s.bar == bar)
            .map(|s| s.pattern.clone());

        // Remove any slots for this track that overlap the target bar
        let patterns = &self.project.patterns;
        let track_id_ref = &track_id;
        self.project.arrangement.retain(|s| {
            if &s.track != track_id_ref { return true; }
            let pat_bars = patterns.iter().find(|p| p.id() == s.pattern)
                .map(|p| p.bars())
                .unwrap_or(1);
            !(s.bar <= bar && bar < s.bar + pat_bars)
        });

        // Cycle to the next pattern in this track's list
        let next_pat = if let Some(cur) = existing_pat {
            let idx = track_patterns.iter().position(|p| p == &cur).unwrap_or(0);
            let next_idx = (idx + 1) % track_patterns.len();
            if next_idx == 0 && track_patterns.len() == 1 {
                return; // only one pattern — second Enter removes, so just stop here
            }
            track_patterns[next_idx].clone()
        } else {
            track_patterns[0].clone()
        };

        self.project.arrangement.push(ArrangementSlot {
            track:   track_id,
            pattern: next_pat,
            bar,
            offset_ticks: 0,
            len_bars: None,
        });
        self.send(SequencerCommand::UpdateLive(self.project.clone()));
    }

    pub fn arr_remove(&mut self) {
        if self.project.tracks.is_empty() { return; }
        let bar = self.arr_bar as u32;
        let Some(t) = self.project.tracks.get(self.arr_track) else { return };
        let track_id = t.id.clone();
        let patterns = &self.project.patterns;
        let tid      = &track_id;
        self.project.arrangement.retain(|s| {
            if &s.track != tid { return true; }
            let pat_bars = patterns.iter().find(|p| p.id() == s.pattern)
                .map(|p| p.bars())
                .unwrap_or(1);
            !(s.bar <= bar && bar < s.bar + pat_bars)
        });
        self.send(SequencerCommand::UpdateLive(self.project.clone()));
    }

    pub fn confirm_export_wav(&mut self) {
        let raw = self.dialog_buf.trim().to_string();
        let format = if std::path::Path::new(&raw).extension().is_some_and(|e| e.eq_ignore_ascii_case("mp3")) {
            daw_core::audio::ExportFormat::Mp3
        } else {
            daw_core::audio::ExportFormat::Wav
        };
        let path = if format == daw_core::audio::ExportFormat::Mp3 || raw.to_ascii_lowercase().ends_with(".wav") {
            std::path::PathBuf::from(raw)
        } else {
            std::path::PathBuf::from(format!("{raw}.wav"))
        };
        let sf2 = std::path::PathBuf::from("assets/GeneralUser.sf2");
        if self.export_control.is_some() { return; }
        let project = self.session.as_ref().map(|s| s.runtime_project(&self.project)).unwrap_or_else(|| self.project.clone());
        let control = std::sync::Arc::new(daw_core::render_control::RenderControl::default());
        self.export_control = Some(control.clone());
        let result = self.export_result.clone();
        self.dialog_status = format!("Exporting → {}… (Ctrl+E cancels)", path.display());
        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> anyhow::Result<()> {
                format.check_available()?;
                let (samples, sr) = daw_core::audio::export::render_range_samples(&project, &sf2, true, None, &control)?;
                control.check()?;
                format.write(&path, &samples, sr, &control)
            }));
            let message = match outcome {
                Ok(Ok(())) => format!("Exported → {}", path.display()),
                Ok(Err(e)) => format!("Export error: {e}"),
                Err(_) => "Export worker panicked".into(),
            };
            if let Ok(mut slot) = result.lock() { *slot = Some(message); }
        });
        self.dialog_mode = DialogMode::None;
        self.dialog_buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi_map::MidiMap;
    use daw_core::{
        audio::{RecordController, RecordState},
        project::{EffectChain, Instrument, Project, Track, TrackType},
    };
    use std::sync::{Arc, Mutex, atomic::{AtomicU8, AtomicU32}};

    fn make_app(tracks: Vec<Track>) -> App {
        let (seq_tx, _) = crossbeam_channel::unbounded();
        let (_, midi_rx) = crossbeam_channel::unbounded();
        let record_state = Arc::new(Mutex::new(RecordState::default()));
        let ctrl = RecordController::new(record_state);
        let mut project = Project::default_project();
        project.tracks = tracks;
        App::new(
            project,
            None,
            seq_tx,
            ctrl,
            midi_rx,
            Arc::new(AtomicU8::new(0)),
            None,
            Arc::new(MidiMap::default()),
            Arc::new(AtomicU32::new(0)),
            Arc::new(Mutex::new(Vec::new())),
        )
    }

    fn melodic_track(id: &str) -> Track {
        Track {
            id: id.into(), name: id.into(),
            track_type: TrackType::Melodic,
            instrument: Instrument::Soundfont { preset: 0, bank: 0, soundfont: None },
            volume: 1.0, pan: 0.0, muted: false,
            patterns: vec![],
            effects: EffectChain::default(),
            modulators: Vec::new(),
            clap_fx: Vec::new(),
            tuning_cents: 0.0,
        }
    }

    fn drum_track(id: &str) -> Track {
        Track {
            id: id.into(), name: id.into(),
            track_type: TrackType::Drum,
            instrument: Instrument::Soundfont { preset: 0, bank: 0, soundfont: None },
            volume: 1.0, pan: 0.0, muted: false,
            patterns: vec![],
            effects: EffectChain::default(),
            modulators: Vec::new(),
            clap_fx: Vec::new(),
            tuning_cents: 0.0,
        }
    }

    fn audio_track(id: &str) -> Track {
        Track {
            id: id.into(), name: id.into(),
            track_type: TrackType::Audio,
            instrument: Instrument::AudioFile { path: String::new() },
            volume: 1.0, pan: 0.0, muted: false,
            patterns: vec![],
            effects: EffectChain::default(),
            modulators: Vec::new(),
            clap_fx: Vec::new(),
            tuning_cents: 0.0,
        }
    }

    #[test]
    fn first_melodic_track_is_channel_0() {
        let mut app = make_app(vec![melodic_track("t1")]);
        app.active_track = 0;
        assert_eq!(app.active_midi_channel(), Some(0));
    }

    #[test]
    fn jam_hot_reload_applies_external_edits() {
        use std::time::{Duration, Instant};
        let path = std::env::temp_dir()
            .join(format!("jam_test_{}_{:?}.daw.json", std::process::id(), std::thread::current().id()));

        let mut app = make_app(vec![melodic_track("t1")]);
        app.project_path = Some(path.clone());

        app.toggle_jam();
        assert!(app.jam_mode);

        // Simulate an external editor (e.g. Claude) re-patterning the file.
        let mut edited = Project::default_project();
        edited.project.bpm = 142.0;
        edited.tracks.clear();
        edited.arrangement.clear();
        daw_core::project::format::save(&edited, &path).unwrap();

        // Force the poll to run and to see the file as "changed".
        app.last_reload_check = Instant::now() - Duration::from_secs(1);
        app.project_mtime = None;
        app.poll_hot_reload();

        assert_eq!(app.project.project.bpm, 142.0, "bpm not hot-reloaded");
        assert_eq!(app.project.tracks.len(), 0, "tracks not hot-reloaded");
        assert!(app.jam_status.contains("reloaded"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn repl_eval_compiles_mini_notation_into_a_pattern() {
        let mut app = make_app(vec![melodic_track("t1")]);
        app.repl_target = 0;
        app.repl_bufs.insert("t1".into(), "c4 e4 g4".into());
        app.repl_eval();

        assert!(app.repl_status.starts_with('✓'), "status: {}", app.repl_status);
        let track = &app.project.tracks[0];
        assert_eq!(track.patterns.len(), 1);
        let pid = track.patterns[0].clone();
        let pat = app.project.patterns.iter().find(|p| p.id() == pid).unwrap();
        match pat {
            daw_core::project::Pattern::PianoRoll(pr) => {
                assert_eq!(pr.notes.len(), 3);
                assert_eq!(pr.notes[0].pitch, 60); // c4
                assert_eq!(pr.notes[2].pitch, 67); // g4
            }
            _ => panic!("expected a piano-roll pattern"),
        }
    }

    #[test]
    fn jam_off_does_not_reload() {
        let mut app = make_app(vec![melodic_track("t1")]);
        // jam_mode defaults off; poll should be a no-op even with a stale check time
        app.last_reload_check = std::time::Instant::now() - std::time::Duration::from_secs(1);
        app.poll_hot_reload();
        assert_eq!(app.project.tracks.len(), 1);
    }

    #[test]
    fn drum_track_routes_to_its_strip() {
        let mut app = make_app(vec![drum_track("d1")]);
        app.active_track = 0;
        assert_eq!(app.active_midi_channel(), Some(0)); // strip = track index
    }

    #[test]
    fn audio_track_returns_none() {
        let mut app = make_app(vec![audio_track("a1")]);
        app.active_track = 0;
        assert_eq!(app.active_midi_channel(), None);
    }

    #[test]
    fn strip_routing_is_track_index_audio_none() {
        let mut app = make_app(vec![
            melodic_track("t0"),
            audio_track("a0"),
            melodic_track("t1"),
            drum_track("d0"),
        ]);
        app.active_track = 0; assert_eq!(app.active_midi_channel(), Some(0));
        app.active_track = 1; assert_eq!(app.active_midi_channel(), None); // audio
        app.active_track = 2; assert_eq!(app.active_midi_channel(), Some(2));
        app.active_track = 3; assert_eq!(app.active_midi_channel(), Some(3));
    }
}

use crate::state::{DockTab, HeadlessState, RenameTarget};
use daw_core::project::Project;
use daw_core::sequencer::SequencerCommand;
use daw_io::Host;
use gpui::{
    div, prelude::*, px, rgb, size, App, Application, Bounds, Context, DragMoveEvent, FocusHandle,
    KeyDownEvent, SharedString, Window, WindowBounds, WindowOptions,
};
use std::path::PathBuf;
use std::sync::atomic::Ordering;

// Ableton-flat theme: muted neutral greys, hairline separators, one warm accent.
const BG: u32 = 0x16161a; // app background (canvas)
const PANEL: u32 = 0x212126; // surfaces (transport, docks, headers)
const PANEL_ALT: u32 = 0x2e2e35; // raised / selected / control fills
const BORDER: u32 = 0x0e0e11; // hairline separators between panels & lanes
const TEXT: u32 = 0xd6d6db;
const TEXT_DIM: u32 = 0x76767e;
const ACCENT: u32 = 0xff9d4d; // warm selection/playhead accent (= ACCENTS[0])
const ACCENTS: [u32; 3] = [0xff9d4d, 0x4dd0ff, 0x6ee87a]; // orange, cyan, green
const RECORD: u32 = 0xff4d47;
const METER_WARN: u32 = 0xe8c84a;

/// Per-track color (Ableton-style track tinting): stripe in the header + the
/// track's clip blocks. Muted, desaturated palette that reads on the dark bg.
const TRACK_PALETTE: [u32; 6] = [0xe0795a, 0x5a9bd4, 0x6cc18e, 0xd6b94a, 0xa97fd0, 0x4fb5b0];

fn track_color(track_idx: usize) -> u32 {
    TRACK_PALETTE[track_idx % TRACK_PALETTE.len()]
}

// Center-canvas geometry (track header column + arrangement lanes).
const TRACK_H: f32 = 56.0;
const STEP_W: f32 = 22.0;
// Piano Roll cell dimensions.
const PR_ROW_H: f32 = 12.0;
const PR_COL_W: f32 = 18.0;
// Wide enough for the full header row (badge + name + mute/solo + vol/pan steppers
// + instrument + type/pattern/remove buttons).
const TRACK_HEADER_W: f32 = 680.0;
const BAR_W: f32 = 64.0;
const MIN_BAR_W: f32 = 8.0;
const MAX_BAR_W: f32 = 200.0;
const RULER_H: f32 = 20.0;

/// GPUI root view: owns the headless state and the engine `Host`.
pub struct DawView {
    st: HeadlessState,
    host: Host,
    playing: bool,
    /// Set by a background export thread; drained into `st.status` each frame.
    export_result: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// Set by the background "Load" file dialog; the picked path is drained and
    /// opened each frame.
    load_result: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    /// Set by the background "+WAV" file dialog; drained each frame and added as
    /// an audio clip on the selected track at the current playhead position.
    audio_clip_result: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    /// Set by the per-pad "load sample" dialog: (track_id, pitch, wav path),
    /// drained each frame into the track's Sampler via `set_sampler_pad`.
    pad_load_result: std::sync::Arc<std::sync::Mutex<Option<(String, u8, std::path::PathBuf)>>>,
    /// Focus handle for the Code tab REPL input.
    focus: FocusHandle,
    /// Set by the background "Save As" file dialog; drained into project_path + save each frame.
    save_as_result: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    /// Soundfont path string, threaded from run() for display in Settings.
    sf2_path: String,
    /// Computed once at startup: the soundfont file is absent, so audio is
    /// disabled — shown as a banner so a fresh clone isn't silently muted.
    sf2_missing: bool,
    /// Focus handle for the instrument-browser search input.
    search_focus: FocusHandle,
    /// Real-time MIDI → piano-roll recorder (records into the open pattern).
    recorder: daw_core::sequencer::MidiRecorder,
    /// True while armed/recording MIDI into the open piano-roll pattern.
    midi_recording: bool,
    /// Audio-input record session (armed / awaiting roll / recording).
    audio_rec: daw_core::audio::input::RecordingSession,
    /// Persisted user config (input device/channel, count-in, latency trim).
    config: daw_io::config::UserConfig,
    /// Input device list for the settings panel (refreshed when it opens).
    input_devices: Vec<daw_core::audio::input::InputDeviceInfo>,
    /// Snapshot undo/redo over `st.project`.
    undo: daw_core::undo::UndoStack,
    /// Focus handle for window-level shortcuts (Ctrl+Z etc.).
    root_focus: FocusHandle,
    /// Filled once by the background CLAP discovery scan; drained in pump().
    clap_scan: std::sync::Arc<std::sync::Mutex<Option<Vec<daw_core::audio::clap::ClapPluginDesc>>>>,
    /// Focus handle for the inline rename editor.
    rename_focus: FocusHandle,
    /// Result of a background ⚖ auto-balance run; drained in pump().
    balance_result: std::sync::Arc<std::sync::Mutex<Option<Vec<daw_core::audio::balance::BalanceResult>>>>,
}

impl DawView {
    /// Current accent color (Appearance setting).
    fn accent(&self) -> u32 { ACCENTS[self.st.accent_idx % ACCENTS.len()] }

    fn toggle_play(&mut self) {
        self.playing = !self.playing;
        // Pausing while armed ends the take (a frozen playhead can't record).
        if !self.playing && self.midi_recording {
            self.end_midi_record();
        }
        // Pausing also punches out an active audio take.
        if !self.playing {
            use daw_core::audio::input::RecordPhase;
            if matches!(self.audio_rec.phase, RecordPhase::AwaitingRoll | RecordPhase::Recording) {
                self.stop_audio_take();
            }
        }
        let cmd = if self.playing {
            SequencerCommand::Play
        } else {
            SequencerCommand::Pause
        };
        let _ = self.host.cmd_tx.try_send(cmd);
    }

    fn project_name(&self) -> SharedString {
        SharedString::from(self.st.project.project.name.clone())
    }

    /// True stop: pause and return the playhead to the start.
    fn stop(&mut self) {
        if self.midi_recording {
            self.end_midi_record();
        }
        {
            use daw_core::audio::input::RecordPhase;
            match self.audio_rec.phase {
                RecordPhase::AwaitingRoll | RecordPhase::Recording => self.stop_audio_take(),
                // Stop while armed-but-idle disarms (closes the input stream).
                RecordPhase::Armed => {
                    self.audio_rec.disarm();
                    self.host.disarm_input();
                    self.st.set_status("record disarmed");
                }
                RecordPhase::Idle => {}
            }
        }
        self.playing = false;
        let _ = self.host.cmd_tx.try_send(SequencerCommand::Pause);
        let _ = self.host.cmd_tx.try_send(SequencerCommand::Seek(0));
    }

    /// The open piano-roll pattern as `(index in project.patterns, length in ticks)`,
    /// or `None` when no piano-roll pattern is open (e.g. a Step pattern is selected).
    fn open_pr_pattern(&self) -> Option<(usize, u32)> {
        let pid = self.st.selected_pattern_id()?;
        let idx = self.st.project.patterns.iter().position(|p| p.id() == pid)?;
        match &self.st.project.patterns[idx] {
            daw_core::project::Pattern::PianoRoll(pr) => Some((idx, pr.ticks_per_bar * pr.bars as u32)),
            _ => None,
        }
    }

    /// Insert a recorded note into the open pattern (dedup on pitch+tick, keep
    /// sorted), then push the live project to the engine.
    fn insert_recorded_note(&mut self, idx: usize, note: daw_core::project::Note) {
        // One label for the whole take: coalescing folds a run of notes into
        // a single undo step.
        self.undo.record("record MIDI", &self.st.project);
        if let daw_core::project::Pattern::PianoRoll(pr) = &mut self.st.project.patterns[idx] {
            if !pr.notes.iter().any(|n| n.pitch == note.pitch && n.tick == note.tick) {
                pr.notes.push(note);
                pr.notes.sort_by_key(|n| (n.tick, n.pitch));
            }
        }
        self.push_live();
    }

    /// Flush still-held notes and disarm. Safe to call when not recording.
    fn end_midi_record(&mut self) {
        if !self.midi_recording {
            return;
        }
        self.recorder.punch_window = None;
        let ph = self.st.playhead();
        if let Some((idx, len)) = self.open_pr_pattern() {
            for note in self.recorder.stop(ph, len) {
                self.insert_recorded_note(idx, note);
            }
        }
        self.midi_recording = false;
        self.st.set_status("recording stopped");
    }

    /// Toggle real-time MIDI recording into the open piano-roll pattern. Arming
    /// auto-starts the transport and clears the pattern (replace mode).
    fn toggle_midi_record(&mut self) {
        if self.midi_recording {
            self.end_midi_record();
            return;
        }
        let Some((idx, _len)) = self.open_pr_pattern() else {
            self.st.set_status("open a piano-roll pattern to record");
            return;
        };
        if !self.playing {
            self.playing = true;
            let _ = self.host.cmd_tx.try_send(SequencerCommand::Play);
        }
        self.undo.record("record MIDI", &self.st.project);
        let lr = self.st.project.loop_region.clone();
        if !lr.enabled {
            // Replace mode (legacy). With the loop region enabled we overdub:
            // each pass's notes layer into the pattern, nothing is cleared.
            if let daw_core::project::Pattern::PianoRoll(pr) = &mut self.st.project.patterns[idx] {
                pr.notes.clear();
            }
        }
        self.recorder.punch_window = if lr.punch_in || lr.punch_out {
            let (s, e) = lr.ticks();
            Some((
                if lr.punch_in { s } else { 0 },
                if lr.punch_out { e } else { u32::MAX },
            ))
        } else {
            None
        };
        self.push_live();
        let ph = self.st.playhead();
        self.recorder.start(ph);
        self.midi_recording = true;
        self.st.set_status("recording MIDI…");
    }

    /// Point live MIDI input at the selected track's mixer strip (the shared
    /// atomic carries a strip index = track index). Audio tracks keep the
    /// previous target — there's nothing for a keyboard to play there.
    fn sync_midi_strip(&self) {
        if let Some(idx) = self.st.selected_track.as_deref()
            .and_then(|id| self.st.project.tracks.iter().position(|t| t.id == id))
        {
            let is_audio = self.st.project.tracks[idx].track_type == daw_core::project::TrackType::Audio;
            if !is_audio {
                self.host.midi_channel.store(idx as u8, Ordering::Relaxed);
            }
        }
    }

    fn selected_track_is_audio(&self) -> bool {
        self.st
            .selected_track
            .as_deref()
            .and_then(|id| self.st.project.tracks.iter().find(|t| t.id == id))
            .map(|t| t.track_type == daw_core::project::TrackType::Audio)
            .unwrap_or(false)
    }

    /// Rec pill: audio track → audio take flow; otherwise the MIDI-record flow.
    fn toggle_record(&mut self) {
        use daw_core::audio::input::RecordPhase;
        if !self.selected_track_is_audio() {
            self.toggle_midi_record();
            return;
        }
        match self.audio_rec.phase {
            RecordPhase::Idle => self.arm_audio_record(),
            RecordPhase::Armed => self.start_audio_take(),
            RecordPhase::AwaitingRoll | RecordPhase::Recording => self.stop_audio_take(),
        }
    }

    /// First Rec press on an audio track: open the input stream, meter live.
    fn arm_audio_record(&mut self) {
        match self.host.arm_input(&self.config) {
            Ok(cs) => {
                cs.reset_clip_latch();
                let name = cs.device_name.clone();
                self.audio_rec.arm();
                self.st.set_status(format!("armed: {name} — Rec to start, Stop to disarm"));
            }
            Err(e) => self.st.set_status(format!("input unavailable: {e}")),
        }
    }

    /// Re-open the capture stream after a device/channel change (armed only —
    /// never mid-take).
    fn reopen_capture_if_armed(&mut self) {
        use daw_core::audio::input::RecordPhase;
        if self.host.capture.is_some() && self.audio_rec.phase == RecordPhase::Armed {
            self.host.disarm_input();
            self.arm_audio_record();
        }
    }

    /// Second Rec press: queue the take and start the count-in (none if the
    /// transport is already rolling — punch-in).
    fn start_audio_take(&mut self) {
        let Some(track_id) = self.st.selected_track.clone() else { return };
        let safe: String = track_id
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let stamp = daw_core::audio::input::utc_timestamp(now);
        // Session base: the writer appends "-p<N>.wav" per loop pass.
        let base = std::path::PathBuf::from(format!("recordings/{safe}-{stamp}"));
        if let Some(parent) = base.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Some(cs) = self.host.capture.as_ref() else { return };
        cs.begin_session(base);
        self.audio_rec.begin();
        let bars = if self.playing { 0 } else { self.config.count_in_bars };
        let _ = self.host.cmd_tx.try_send(SequencerCommand::RecordStart { count_in_bars: bars });
        self.playing = true;
        self.st.set_status(if bars > 0 {
            format!("count-in: {bars} bar(s)…")
        } else {
            "recording…".to_string()
        });
    }

    /// Punch out: finalize the take; the transport keeps rolling. The clip is
    /// placed when the TakeResult arrives in `poll_recording`.
    fn stop_audio_take(&mut self) {
        if let Some(cs) = self.host.capture.as_ref() {
            cs.end_session();
        }
        let _ = self.host.cmd_tx.try_send(SequencerCommand::RecordStop);
        self.st.set_status("finishing take…");
    }

    /// Per-frame: latch the roll tick, collect finished takes into clips.
    /// Returns true while the meter should repaint (capture stream open).
    fn poll_recording(&mut self) -> bool {
        use daw_core::audio::input::{RecordPhase, TRANSPORT_ROLLING};
        if self.audio_rec.phase == RecordPhase::AwaitingRoll
            && self.host.transport_state.load(Ordering::Relaxed) == TRANSPORT_ROLLING
        {
            let tick = self.host.roll_start_tick.load(Ordering::Relaxed);
            self.audio_rec.rolled(tick);
            self.st.set_status("recording…");
        }

        let results: Vec<daw_core::audio::input::TakeResult> = self
            .host
            .capture
            .as_ref()
            .map(|c| c.take_rx.try_iter().collect())
            .unwrap_or_default();
        for take in results {
            if take.dropped_frames > 0 {
                self.st.set_status(format!("warning: {} input frames dropped", take.dropped_frames));
            }
            let placement =
                self.audio_rec.finish(take.start_tick, self.st.bpm(), self.config.latency_trim_ms, take.duration_sec);
            let Some((start_sec, offset_sec, len_sec)) = placement else {
                let _ = std::fs::remove_file(&take.path);
                self.st.set_status("take too short — discarded");
                continue;
            };
            let Some(track_id) = self.st.selected_track.clone() else { continue };
            let path_str = take.path.to_string_lossy().into_owned();
            // Undo removes the clip from the project; the WAV stays on disk.
            self.undo.record("record take", &self.st.project);
            self.st.project.audio_clips.push(daw_core::project::AudioClip {
                track: track_id.clone(),
                path: path_str.clone(),
                start_sec,
                offset_sec,
                len_sec,
                gain: 1.0,
            });
            // Loop passes stack: silence the previous pass's clip (it stays
            // in the project for manual comping — nothing is destroyed).
            if let Some(prev) = self.audio_rec.last_clip.take() {
                self.st.project.set_audio_clip_gain(prev, 0.0);
            }
            self.audio_rec.last_clip = Some(self.st.project.audio_clips.len() - 1);
            self.audio_rec.pass += 1;
            // (pump section 0c caches the waveform peaks for the new path)
            let _ = self
                .host
                .cmd_tx
                .try_send(SequencerCommand::UpdateLive(self.st.project.clone()));
            if self.audio_rec.pass > 1 {
                self.st.set_status(format!("pass {} saved: {path_str}", self.audio_rec.pass));
            } else {
                self.st.set_status(format!("take saved: {path_str}"));
            }
        }
        self.host.capture.is_some()
    }

    /// A small input-level meter, shown while the capture stream is open.
    fn input_meter(&self) -> Option<gpui::Div> {
        let (peak, clipped) = self.host.capture.as_ref()?.meter();
        let bars = 12usize;
        let lit = (peak.min(1.0) * bars as f32).round() as usize;
        let cells = (0..bars).map(move |i| {
            let on = i < lit;
            let color = if i >= bars - 2 { RECORD } else if i >= bars - 5 { METER_WARN } else { 0x6ee87a };
            div().w(px(4.)).h(px(12.)).rounded_sm().bg(rgb(if on { color } else { PANEL_ALT }))
        });
        Some(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(div().text_color(rgb(if clipped { RECORD } else { TEXT_DIM })).child(if clipped { "CLIP" } else { "IN" }))
                .children(cells),
        )
    }

    /// Stamp a pattern into the arrange lane for `track_id` at `bar`: the
    /// currently selected pattern if it belongs to this track, otherwise the
    /// track's first pattern. Pushes the edit live.
    fn place_in_arrange(&mut self, track_id: &str, bar: u32) {
        let sel = self.st.selected_pattern_id().filter(|pid| {
            self.st.project.tracks.iter().find(|t| t.id == track_id)
                .is_some_and(|t| t.patterns.iter().any(|p| p == pid))
        });
        self.undo.record("place pattern", &self.st.project);
        match sel {
            Some(pid) => { self.st.project.place_pattern_id(track_id, bar, &pid); }
            None      => { self.st.project.place_pattern(track_id, bar); }
        }
        self.push_live();
    }

    /// Move the playhead to the start of `bar` (0-based).
    fn seek_to_bar(&mut self, bar: u32) {
        let tick = bar.saturating_mul(daw_core::sequencer::TICKS_PER_BAR);
        let _ = self.host.cmd_tx.try_send(SequencerCommand::Seek(tick));
    }

    /// Move the playhead to an absolute timeline position in seconds. Used when
    /// clicking an audio clip, which is placed by seconds rather than bars, so
    /// Play starts from exactly where the clip begins.
    fn seek_to_sec(&mut self, sec: f64) {
        let tick = daw_core::sequencer::engine::sec_to_tick(sec, self.st.project.project.bpm);
        let _ = self.host.cmd_tx.try_send(SequencerCommand::Seek(tick));
    }

    /// Nudge the tempo and push it to the engine.
    fn nudge_bpm(&mut self, delta: f64) {
        self.undo.record("BPM", &self.st.project);
        let bpm = (self.st.project.project.bpm + delta).clamp(20.0, 300.0);
        self.st.project.project.bpm = bpm;
        let _ = self.host.cmd_tx.try_send(SequencerCommand::SetBpm(bpm));
    }

    /// Flip a track's own mute flag, then push its effective mute to the engine.
    fn toggle_mute(&mut self, id: &str) {
        self.undo.record("mute", &self.st.project);
        let cur = self
            .st
            .project
            .tracks
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.muted)
            .unwrap_or(false);
        self.st.project.set_track_muted(id, !cur);
        self.push_mute(id);
    }

    /// Toggle solo, then re-push every track's effective mute (solo affects all).
    fn toggle_solo(&mut self, id: &str) {
        self.st.toggle_solo(id);
        let ids: Vec<String> = self.st.project.tracks.iter().map(|t| t.id.clone()).collect();
        for tid in ids {
            self.push_mute(&tid);
        }
    }

    /// Send the whole project to the engine as a cycle-quantized live edit.
    fn push_live(&self) {
        let _ = self
            .host
            .cmd_tx
            .try_send(SequencerCommand::UpdateLive(self.st.project.clone()));
    }

    /// The single funnel for project mutations: record an undo snapshot,
    /// apply the edit, push it to the engine. Use a stable `label` for
    /// continuous gestures (drags/steppers) so they coalesce into one step.
    fn edit(&mut self, label: &str, f: impl FnOnce(&mut daw_core::project::Project)) {
        self.undo.record(label, &self.st.project);
        f(&mut self.st.project);
        self.push_live();
    }

    fn undo(&mut self) {
        match self.undo.undo(self.st.project.clone()) {
            Some((label, snap)) => {
                self.st.project = snap;
                self.push_live();
                self.st.set_status(format!("undid: {label}"));
            }
            None => self.st.set_status("nothing to undo"),
        }
    }

    fn redo(&mut self) {
        match self.undo.redo(self.st.project.clone()) {
            Some((label, snap)) => {
                self.st.project = snap;
                self.push_live();
                self.st.set_status(format!("redid: {label}"));
            }
            None => self.st.set_status("nothing to redo"),
        }
    }

    fn transpose(&mut self, semis: i32) {
        self.edit("transpose", |p| { p.transpose(semis); });
    }

    fn cycle_humanize(&mut self) {
        self.edit("humanize", |p| { p.cycle_humanize(); });
    }

    fn adjust_swing(&mut self, d: f32) {
        self.undo.record("swing", &self.st.project);
        let sw = (self.st.project.project.swing + d).clamp(0.0, 1.0);
        self.st.project.set_swing(sw);
        let _ = self.host.cmd_tx.try_send(SequencerCommand::SetSwing(sw));
    }

    fn toggle_metronome(&mut self) {
        self.undo.record("metronome", &self.st.project);
        let on = !self.st.project.project.metronome;
        self.st.project.set_metronome(on);
        let _ = self.host.cmd_tx.try_send(SequencerCommand::SetMetronome(on));
    }

    fn panic_engine(&self) {
        let _ = self.host.cmd_tx.try_send(SequencerCommand::Panic);
    }

    fn add_track(&mut self, ty: daw_core::project::TrackType) {
        self.undo.record("add track", &self.st.project);
        let id = self.st.project.add_track(ty);
        self.st.select_track(&id);
        self.sync_midi_strip();
        self.push_live();
    }

    fn remove_track(&mut self, id: &str) {
        self.undo.record("remove track", &self.st.project);
        if self.st.project.remove_track(id) {
            // Fix selection if we removed the selected track.
            if self.st.selected_track.as_deref() == Some(id) {
                let first = self.st.project.tracks.first().map(|t| t.id.clone());
                match first {
                    Some(fid) => self.st.select_track(&fid),
                    None => self.st.selected_track = None,
                }
                self.sync_midi_strip();
            }
            self.push_live();
        }
    }

    fn cycle_track_type(&mut self, id: &str) {
        self.edit("track type", |p| { p.cycle_track_type(id); });
    }

    fn add_pattern(&mut self, id: &str) {
        self.undo.record("add pattern", &self.st.project);
        if let Some(pid) = self.st.project.add_pattern(id) {
            self.st.select_pattern(&pid);
            self.push_live();
        }
    }

    /// Push the selected track's effect chain to the engine (applied live).
    fn push_effects(&self, track_id: &str) {
        if let Some(t) = self.st.project.tracks.iter().find(|t| t.id == track_id) {
            let _ = self.host.cmd_tx.try_send(SequencerCommand::SetTrackEffects {
                track_id: track_id.to_string(),
                effects: t.effects.clone(),
            });
        }
    }

    /// Push the project's return-bus config to the engine (applied live).
    fn push_returns(&self) {
        let _ = self.host.cmd_tx.try_send(SequencerCommand::SetReturns(
            self.st.project.returns.clone(),
        ));
    }

    fn nudge_return(&mut self, param: daw_core::project::ReturnParam, delta: f32) {
        self.undo.record("return param", &self.st.project);
        self.st.project.nudge_return(param, delta);
        self.push_returns();
    }

    fn toggle_return_mute(&mut self, idx: usize) {
        self.undo.record("return mute", &self.st.project);
        self.st.project.toggle_return_mute(idx);
        self.push_returns();
    }

    fn start_rename(&mut self, target: RenameTarget, current: String) {
        self.st.rename_buf = current;
        self.st.renaming = Some(target);
    }

    fn commit_rename(&mut self) {
        let Some(target) = self.st.renaming.take() else { return };
        let name = self.st.rename_buf.trim().to_string();
        if name.is_empty() {
            return;
        }
        self.undo.record("rename", &self.st.project);
        let ok = match &target {
            RenameTarget::Track(id) => self.st.project.set_track_name(id, &name),
            RenameTarget::Pattern(pid) => self.st.project.set_pattern_name(pid, &name),
            RenameTarget::Row(pid, ri) => self.st.project.set_row_name(pid, *ri, &name),
        };
        if ok {
            self.push_live();
        }
    }

    /// The inline editor that replaces a name while it's being renamed.
    fn rename_editor(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let buf = self.st.rename_buf.clone();
        div()
            .id("rename-input")
            .key_context("RenameInput")
            .track_focus(&self.rename_focus)
            .px_1()
            .bg(rgb(BG))
            .border_1()
            .border_color(rgb(self.accent()))
            .rounded_sm()
            .text_color(rgb(TEXT))
            .whitespace_nowrap()
            .child(format!("{buf}▏"))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
                match ev.keystroke.key.as_str() {
                    "enter" => this.commit_rename(),
                    "escape" => this.st.renaming = None,
                    "backspace" => {
                        this.st.rename_buf.pop();
                    }
                    _ => {
                        if let Some(ch) = &ev.keystroke.key_char {
                            for c in ch.chars() {
                                this.st.rename_buf.push(c);
                            }
                        }
                    }
                }
                cx.notify();
            }))
    }

    fn push_loop_region(&self) {
        let _ = self.host.cmd_tx.try_send(SequencerCommand::SetLoopRegion(
            self.st.project.loop_region.clone(),
        ));
    }

    fn toggle_loop(&mut self) {
        self.undo.record("loop region", &self.st.project);
        self.st.project.toggle_loop();
        self.push_loop_region();
    }

    fn toggle_punch_in(&mut self) {
        self.undo.record("loop region", &self.st.project);
        self.st.project.toggle_punch_in();
        self.push_loop_region();
    }

    fn toggle_punch_out(&mut self) {
        self.undo.record("loop region", &self.st.project);
        self.st.project.toggle_punch_out();
        self.push_loop_region();
    }

    fn set_loop_region_bars(&mut self, start_bar: u32, end_bar: u32) {
        self.undo.record("loop region", &self.st.project); // stable label coalesces
        self.st.project.set_loop_region(start_bar, end_bar);
        self.push_loop_region();
    }

    /// Region-row click rules: left of the region moves the start, right of it
    /// extends the end, inside shrinks to that single bar.
    fn click_loop_bar(&mut self, bar: u32) {
        let lr = &self.st.project.loop_region;
        let (s, e) = (lr.start_bar, lr.end_bar);
        let (ns, ne) = if bar < s {
            (bar, e)
        } else if bar >= e {
            (s, bar + 1)
        } else {
            (bar, bar + 1)
        };
        self.set_loop_region_bars(ns, ne);
    }

    fn add_clap_fx(&mut self, desc_path: String, desc_id: String) {
        let Some(tid) = self.st.selected_track.clone() else {
            self.st.set_status("select a track first".to_string());
            return;
        };
        self.undo.record("add clap fx", &self.st.project);
        if self.st.project.add_clap_fx(&tid, &desc_path, &desc_id).is_some() {
            self.push_live();
            self.st.set_status(format!("+ {desc_id}"));
        }
    }

    fn set_clap_instrument(&mut self, desc_path: String, desc_id: String) {
        let Some(tid) = self.st.selected_track.clone() else {
            self.st.set_status("select a track first".to_string());
            return;
        };
        self.undo.record("clap instrument", &self.st.project);
        if self
            .st
            .project
            .set_track_clap_instrument(&tid, &desc_path, &desc_id)
            .is_some()
        {
            self.push_live();
            self.st.set_status(format!("{tid} ← {desc_id}"));
        }
    }

    fn remove_clap_fx(&mut self, uid: &str) {
        let Some(tid) = self.st.selected_track.clone() else { return };
        self.undo.record("remove clap fx", &self.st.project);
        if self.st.project.remove_clap_fx(&tid, uid) {
            self.st.clap_meta.remove(uid);
            self.push_live();
        }
    }

    /// Set a CLAP param: updates the project override (undoable) and sends
    /// the change straight to the live processor. No push_live — a full
    /// project reload per stepper click would be heavy, and the event
    /// reaches the running instance directly.
    fn nudge_clap_param(&mut self, uid: String, param_id: u32, value: f64) {
        self.undo.record("clap param", &self.st.project);
        self.st.project.set_clap_param(&uid, param_id, value);
        let _ = self
            .host
            .cmd_tx
            .try_send(SequencerCommand::SetClapParam { uid, param_id, value });
    }

    /// One CLAP slot section for the FX dock: header (name + remove) and
    /// generic param stepper rows from the relay-shipped metadata.
    fn clap_slot_section(
        &self,
        slot: &daw_core::project::ClapSlot,
        removable: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let uid = slot.uid.clone();
        let (name, params) = match self.st.clap_meta.get(&uid) {
            Some((n, p)) => (n.clone(), p.clone()),
            None => (format!("{} (loading…)", slot.id), Vec::new()),
        };

        let mut header = div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(22.))
            .child(div().text_color(rgb(TEXT)).child(format!("⌁ {name}")));
        if removable {
            let uid_x = uid.clone();
            header = header.child(
                cell_btn(format!("clapx-{uid}"), "✕", false, PANEL_ALT).on_click(cx.listener(
                    move |this, _e, _w, cx| {
                        this.remove_clap_fx(&uid_x);
                        cx.notify();
                    },
                )),
            );
        }

        let rows = params.into_iter().map(|info| {
            let cur = slot
                .params
                .get(&info.id.to_string())
                .copied()
                .unwrap_or(info.default);
            let step = ((info.max - info.min) / 20.0).max(1e-6);
            let (uid_d, uid_i) = (uid.clone(), uid.clone());
            let (lo, hi, id) = (info.min, info.max, info.id);
            div()
                .flex()
                .items_center()
                .gap_2()
                .h(px(24.))
                .child(div().w(px(72.)).flex_shrink_0().whitespace_nowrap().overflow_hidden().text_color(rgb(TEXT_DIM)).child(info.name.clone()))
                .child(cell_btn(format!("cp-{uid}-{id}"), "−", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| {
                    this.nudge_clap_param(uid_d.clone(), id, (cur - step).clamp(lo, hi));
                    cx.notify();
                })))
                .child(div().w(px(64.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(TEXT)).child(format!("{cur:.2}")))
                .child(cell_btn(format!("cp+{uid}-{id}"), "+", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| {
                    this.nudge_clap_param(uid_i.clone(), id, (cur + step).clamp(lo, hi));
                    cx.notify();
                })))
        });

        div().flex().flex_col().gap_1().child(header).children(rows)
    }

    /// Bus-parameter editor for the selected return (FX dock).
    fn return_editor(&self, ri: usize, cx: &mut Context<Self>) -> gpui::Div {
        use daw_core::project::ReturnParam;
        let (title, params): (&str, &[ReturnParam]) = if ri == 0 {
            ("Reverb Return", &ReturnParam::REVERB)
        } else {
            ("Delay Return", &ReturnParam::DELAY)
        };
        let muted = if ri == 0 {
            self.st.project.returns.reverb.muted
        } else {
            self.st.project.returns.delay.muted
        };
        let rows = params.iter().copied().map(|p| {
            let val = p.value(&self.st.project.returns);
            let d = if p == ReturnParam::DlyTimeMs { 10.0 } else { 0.05 };
            div()
                .flex()
                .items_center()
                .gap_2()
                .h(px(24.))
                .child(div().w(px(72.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(TEXT_DIM)).child(p.label()))
                .child(cell_btn(format!("rt-{ri}-{}", p.label()), "−", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| {
                    this.nudge_return(p, -d);
                    cx.notify();
                })))
                .child(div().w(px(64.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(TEXT)).child(format!("{val:.2}")))
                .child(cell_btn(format!("rt+{ri}-{}", p.label()), "+", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| {
                    this.nudge_return(p, d);
                    cx.notify();
                })))
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .overflow_hidden()
            .child(div().text_color(rgb(TEXT)).child(title))
            .children(rows)
            .child(cell_btn(format!("rtm{ri}"), "Mute", muted, RECORD).on_click(cx.listener(move |this, _e, _w, cx| {
                this.toggle_return_mute(ri);
                cx.notify();
            })))
    }

    /// Effect-chain editor for the selected track (type-appropriate params).
    fn fx_editor(&self, cx: &mut Context<Self>) -> gpui::Div {
        use daw_core::project::{EffectParam, TrackType};
        if let Some(ri) = self.st.selected_return {
            return self.return_editor(ri, cx);
        }
        let Some(tid) = self.st.selected_track.clone() else {
            return dock_hint("Select a track to edit its effects.");
        };
        let Some(track) = self.st.project.tracks.iter().find(|t| t.id == tid) else {
            return dock_hint("Select a track to edit its effects.");
        };
        let params: &[EffectParam] = if track.track_type == TrackType::Audio {
            &EffectParam::AUDIO
        } else {
            &EffectParam::MIDI
        };
        let step = |p: EffectParam| -> f32 {
            match p {
                EffectParam::DelayTimeMs => 10.0,
                EffectParam::EqMidFreq => 100.0,
                EffectParam::EqLowGain | EffectParam::EqMidGain | EffectParam::EqHighGain
                | EffectParam::CompThreshold | EffectParam::CompRatio
                | EffectParam::CompAttackMs | EffectParam::CompReleaseMs => 1.0,
                _ => 0.05,
            }
        };

        let rows: Vec<_> = params.iter().copied().map(|p| {
            let val = p.value(&track.effects);
            let (tdec, tinc) = (tid.clone(), tid.clone());
            let d = step(p);
            div()
                .flex()
                .items_center()
                .gap_2()
                .h(px(24.))
                .child(div().w(px(72.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(TEXT_DIM)).child(p.label()))
                .child(cell_btn(format!("fx-{}", p.label()), "−", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| {
                    this.undo.record("effects", &this.st.project);
                    this.st.project.nudge_effect(&tdec, p, -d);
                    this.push_effects(&tdec);
                    cx.notify();
                })))
                .child(div().w(px(64.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(TEXT)).child(format!("{val:.2}")))
                .child(cell_btn(format!("fx+{}", p.label()), "+", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| {
                    this.undo.record("effects", &this.st.project);
                    this.st.project.nudge_effect(&tinc, p, d);
                    this.push_effects(&tinc);
                    cx.notify();
                })))
        }).collect();

        // CLAP sections: the instrument (if any) above the insert chain.
        let mut clap_sections: Vec<gpui::Div> = Vec::new();
        if let daw_core::project::Instrument::Clap(slot) = &track.instrument {
            clap_sections.push(self.clap_slot_section(&slot.clone(), false, cx));
        }
        for slot in track.clap_fx.clone() {
            clap_sections.push(self.clap_slot_section(&slot, true, cx));
        }

        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .overflow_hidden()
            .children(rows)
            .children(clap_sections)
    }

    /// Clickable chips for the selected track's patterns; highlights the open one.
    fn pattern_picker(&self, cx: &mut Context<Self>) -> gpui::Div {
        let sel = self.st.selected_pattern_id();
        let chips = self.st.track_pattern_ids().into_iter().map(|pid| {
            let name = self
                .st
                .project
                .patterns
                .iter()
                .find(|p| p.id() == pid)
                .map(|p| p.name().to_string())
                .unwrap_or_else(|| pid.clone());
            let is_sel = sel.as_deref() == Some(pid.as_str());
            let pid_c = pid.clone();
            div()
                .id(SharedString::from(format!("pat-{pid}")))
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .whitespace_nowrap()
                .bg(rgb(if is_sel { PANEL_ALT } else { PANEL }))
                .text_color(rgb(if is_sel { TEXT } else { TEXT_DIM }))
                .child(name.clone())
                .on_click(cx.listener(move |this, e: &gpui::ClickEvent, w, cx| {
                    if e.click_count() >= 2 {
                        this.start_rename(RenameTarget::Pattern(pid_c.clone()), name.clone());
                        this.rename_focus.clone().focus(w);
                    } else {
                        this.st.select_pattern(&pid_c);
                    }
                    cx.notify();
                }))
        });
        let chips: Vec<_> = chips.collect();
        // While a chip is being renamed, show the editor in front of the chips.
        let editor = matches!(&self.st.renaming, Some(RenameTarget::Pattern(_)))
            .then(|| self.rename_editor(cx));
        div()
            .flex()
            .gap_1()
            .px_2()
            .py_1()
            .bg(rgb(BG))
            .border_b_1()
            .border_color(rgb(BORDER))
            .children(editor)
            .children(chips)
    }

    /// Push a track's *effective* mute (own flag OR solo-silenced) to the engine.
    fn push_mute(&self, id: &str) {
        let muted = self.st.effective_muted(id);
        let _ = self.host.cmd_tx.try_send(SequencerCommand::SetTrackMute {
            track_id: id.to_string(),
            muted,
        });
    }

    fn nudge_volume(&mut self, id: &str, delta: f32) {
        self.undo.record("volume", &self.st.project);
        let cur = self
            .st
            .project
            .tracks
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.volume)
            .unwrap_or(0.0);
        let v = (cur + delta).clamp(0.0, 2.0); // 1.0 = unity, up to +6 dB
        self.st.project.set_track_volume(id, v);
        let _ = self.host.cmd_tx.try_send(SequencerCommand::SetTrackVolume {
            track_id: id.to_string(),
            volume: v,
        });
    }

    fn nudge_pan(&mut self, id: &str, delta: f32) {
        self.undo.record("pan", &self.st.project);
        let cur = self
            .st
            .project
            .tracks
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.pan)
            .unwrap_or(0.0);
        let p = (cur + delta).clamp(-1.0, 1.0);
        self.st.project.set_track_pan(id, p);
        let _ = self.host.cmd_tx.try_send(SequencerCommand::SetTrackPan {
            track_id: id.to_string(),
            pan: p,
        });
    }

    /// Return the height for a single track lane/header row (px). Falls back to
    /// the global default when no per-track override has been set.
    fn track_h(&self, id: &str) -> f32 {
        self.st.track_heights.get(id).copied().unwrap_or(TRACK_H)
    }

    /// Effective per-bar pixel width at the current zoom (clamped).
    fn bar_w(&self) -> f32 {
        (BAR_W * self.st.arrange_zoom).clamp(MIN_BAR_W, MAX_BAR_W)
    }

    /// The track header column: one row per track with select / mute / solo /
    /// volume / pan steppers / instrument label.
    fn track_headers(&self, cx: &mut Context<Self>) -> gpui::Div {
        let accent = self.accent();
        let selected = self.st.selected_track.clone();
        let rows = self.st.project.tracks.iter().enumerate().map(|(ti, t)| {
            let id = t.id.clone();
            let is_sel = selected.as_deref() == Some(id.as_str());
            let muted = t.muted;
            let soloed = self.st.soloed.contains(&id);
            let vol = t.volume;
            let pan = t.pan;
            let name = t.name.clone();
            let badge = track_type_badge(t);
            let instr = instrument_label(t);
            // The plugged instrument shows in the accent color so it's an
            // obvious, always-visible confirmation of what's loaded.
            let instr_color = match &t.instrument {
                daw_core::project::Instrument::Soundfont { bank: 0, .. } => accent,
                _ => TEXT_DIM,
            };
            let tcolor = track_color(ti);
            let th = self.track_h(&id);

            div()
                .id(SharedString::from(format!("trk-{id}")))
                .relative()
                .flex()
                .items_center()
                .gap_1()
                .h(px(th))
                .bg(rgb(if is_sel { PANEL_ALT } else { PANEL }))
                .border_b_1()
                .border_color(rgb(BORDER))
                .cursor_pointer()
                .on_click(cx.listener({
                    let id = id.clone();
                    move |this, _e, _w, cx| {
                        this.st.select_track(&id);
                        this.sync_midi_strip();
                        cx.notify();
                    }
                }))
                // Ableton-style per-track color stripe at the left edge.
                .child(div().w(px(4.)).h(px(th)).flex_shrink_0().bg(rgb(tcolor)))
                .child(div().pl_1().w(px(40.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(tcolor)).child(badge))
                .child(if self.st.renaming == Some(RenameTarget::Track(id.clone())) {
                    div().flex_1().min_w(px(60.)).child(self.rename_editor(cx)).into_any_element()
                } else {
                    let (idr, namer) = (id.clone(), name.clone());
                    div()
                        .id(SharedString::from(format!("trkname-{id}")))
                        .flex_1()
                        .min_w(px(60.))
                        .truncate()
                        .text_color(rgb(TEXT))
                        .child(name.clone())
                        .on_click(cx.listener(move |this, e: &gpui::ClickEvent, w, cx| {
                            if e.click_count() >= 2 {
                                this.start_rename(RenameTarget::Track(idr.clone()), namer.clone());
                                this.rename_focus.clone().focus(w);
                                cx.notify();
                            }
                        }))
                        .into_any_element()
                })
                .child(cell_btn(format!("m-{id}"), "M", muted, RECORD).on_click(cx.listener({
                    let id = id.clone();
                    move |this, _e, _w, cx| {
                        this.toggle_mute(&id);
                        cx.notify();
                    }
                })))
                .child(cell_btn(format!("s-{id}"), "S", soloed, accent).on_click(cx.listener({
                    let id = id.clone();
                    move |this, _e, _w, cx| {
                        this.toggle_solo(&id);
                        cx.notify();
                    }
                })))
                .child(cell_btn(format!("vd-{id}"), "−", false, PANEL_ALT).on_click(cx.listener({
                    let id = id.clone();
                    move |this, _e, _w, cx| {
                        this.nudge_volume(&id, -0.05);
                        cx.notify();
                    }
                })))
                .child(div().w(px(46.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(TEXT_DIM)).child(format!("v{vol:.2}")))
                .child(cell_btn(format!("vu-{id}"), "+", false, PANEL_ALT).on_click(cx.listener({
                    let id = id.clone();
                    move |this, _e, _w, cx| {
                        this.nudge_volume(&id, 0.05);
                        cx.notify();
                    }
                })))
                .child(cell_btn(format!("pd-{id}"), "−", false, PANEL_ALT).on_click(cx.listener({
                    let id = id.clone();
                    move |this, _e, _w, cx| {
                        this.nudge_pan(&id, -0.1);
                        cx.notify();
                    }
                })))
                .child(div().w(px(46.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(TEXT_DIM)).child(format!("p{pan:+.1}")))
                .child(cell_btn(format!("pu-{id}"), "+", false, PANEL_ALT).on_click(cx.listener({
                    let id = id.clone();
                    move |this, _e, _w, cx| {
                        this.nudge_pan(&id, 0.1);
                        cx.notify();
                    }
                })))
                .child(div().w(px(120.)).flex_shrink_0().truncate().text_color(rgb(instr_color)).child(instr))
                .child(cell_btn(format!("tt-{id}"), "T", false, PANEL_ALT).on_click(cx.listener({
                    let id = id.clone();
                    move |this, _e, _w, cx| { this.cycle_track_type(&id); cx.notify(); }
                })))
                .child(cell_btn(format!("tp-{id}"), "+P", false, PANEL_ALT).on_click(cx.listener({
                    let id = id.clone();
                    move |this, _e, _w, cx| { this.add_pattern(&id); cx.notify(); }
                })))
                .child(cell_btn(format!("tx-{id}"), "✕", false, RECORD).on_click(cx.listener({
                    let id = id.clone();
                    move |this, _e, _w, cx| { this.remove_track(&id); cx.notify(); }
                })))
                // Bottom-edge resize handle: dragging it adjusts this track's height.
                .child(
                    div()
                        .id(SharedString::from(format!("rzh-{id}")))
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(px(4.))
                        .cursor_pointer()
                        .bg(rgb(BORDER))
                        .on_drag(TrackResize { track: id.clone() }, |_, _, _, cx| cx.new(|_| EmptyDrag))
                        .on_drag_move(cx.listener({
                            let id = id.clone();
                            move |this, e: &DragMoveEvent<TrackResize>, _w, cx| {
                                // Only the track actually being dragged resizes (the move
                                // event fires for every TrackResize handler).
                                let dragged = { e.drag(cx).track.clone() };
                                if dragged != id { return; }
                                // Make this track's bottom follow the cursor: new_h =
                                // cur_h + (mouse_y - handle_top_y). The handle re-renders
                                // at the new bottom each frame, so it converges.
                                let handle_y = f32::from(e.bounds.origin.y);
                                let mouse_y = f32::from(e.event.position.y);
                                let cur = this.track_h(&id);
                                let new = (cur + (mouse_y - handle_y)).clamp(28.0, 220.0);
                                this.st.track_heights.insert(id.clone(), new);
                                cx.notify();
                            }
                        })),
                )
        });

        div()
            .flex()
            .flex_col()
            .w(px(TRACK_HEADER_W))
            .flex_shrink_0()
            .overflow_hidden()
            .bg(rgb(PANEL))
            // Top spacer aligns header rows with the timeline lanes (below the ruler).
            .child(div().h(px(RULER_H)).border_b_1().border_color(rgb(BORDER)))
            .children(rows)
    }

    /// The arrangement timeline: a clickable bar ruler, one lane per track with
    /// clip blocks from the project's arrangement, and a current-bar playhead cue.
    fn timeline(&self, cx: &mut Context<Self>, vis_w: f32) -> gpui::Div {
        let accent = self.accent();
        let bars = self.st.project.song_bars();
        let cur_bar = self.st.current_bar();
        let bps = self.st.project.project.time_signature[0].max(1) as f64;
        let bar_seconds = (60.0 / self.st.project.project.bpm) * bps;
        let bw = self.bar_w();
        let pps = (bw as f64 / bar_seconds) as f32; // pixels per second
        let scroll_x = self.st.arrange_scroll_x;

        // Virtualize: only build the bars within the visible scroll window (plus a
        // small margin), so render cost is O(visible bars), not O(song length).
        // `vis_w` (window width) is a conservative over-estimate of the timeline
        // region — it renders a few extra bars rather than ever too few.
        let first_bar = ((scroll_x / bw).floor() as u32).min(bars);
        let vis_count = (vis_w / bw).ceil() as u32 + 2;
        let last_bar = (first_bar + vis_count).min(bars);

        // Loop-region row: a thin clickable lane above the ruler. Click left
        // of the region to move its start, right of it to extend the end,
        // inside it to shrink to that bar (toolbar ⟳ enables looping).
        let lr = self.st.project.loop_region.clone();
        let region_row = div()
            .flex()
            .h(px(8.))
            .bg(rgb(PANEL))
            .child(div().w(px(first_bar as f32 * bw)).flex_shrink_0())
            .children((first_bar..last_bar).map(|b| {
                let inside = b >= lr.start_bar && b < lr.end_bar;
                let bg = if inside && lr.enabled {
                    accent
                } else if inside {
                    0x666666
                } else {
                    PANEL_ALT
                };
                div()
                    .id(SharedString::from(format!("loopbar-{b}")))
                    .w(px(bw))
                    .h_full()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .border_r_1()
                    .border_color(rgb(PANEL))
                    .bg(rgb(bg))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.click_loop_bar(b);
                        cx.notify();
                    }))
            }));

        // Ruler: clickable bar-number cells that seek.
        let ruler = div()
            .flex()
            .h(px(RULER_H))
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            // leading spacer for the bars scrolled off the left
            .child(div().w(px(first_bar as f32 * bw)).flex_shrink_0())
            .children((first_bar..last_bar).map(|b| {
                div()
                    .id(SharedString::from(format!("bar-{b}")))
                    .w(px(bw))
                    .flex_shrink_0()
                    .flex()
                    .items_end()
                    .cursor_pointer()
                    .border_r_1()
                    .border_color(rgb(BORDER))
                    .child(div().px_1().text_color(rgb(if b == cur_bar { accent } else { TEXT_DIM })).child(format!("{}", b + 1)))
                    .children((0..3).map(move |_| div().w(px(1.)).h(px(5.)).bg(rgb(PANEL_ALT)).ml(px(bw / 4.0 - 1.0))))
                    .on_click(cx.listener(move |this, _e, _w, cx| { this.seek_to_bar(b); cx.notify(); }))
            }));

        // One lane per track; clip blocks span their bars, empty bars are grid cells.
        let lanes = self.st.project.tracks.iter().enumerate().map(|(ti, t)| {
            let color = clip_color(ti);
            let th = self.track_h(&t.id);
            let blocks: Vec<(u32, u32, String, String)> = self
                .st
                .project
                .arrangement
                .iter()
                .filter(|s| s.track == t.id)
                .filter_map(|s| {
                    let pat = self.st.project.patterns.iter().find(|p| p.id() == s.pattern)?;
                    let span = s.len_bars.unwrap_or_else(|| pat.bars().max(1)).max(1);
                    Some((s.bar, span, pat.name().to_string(), s.pattern.clone()))
                })
                .collect();

            let tid = t.id.clone();
            // Start at the first visible bar, but back up to the start of any clip
            // that straddles it so a clip extending into view still renders.
            let start_bar = blocks.iter()
                .find(|(s, span, _, _)| *s <= first_bar && first_bar < *s + *span)
                .map(|(s, _, _, _)| *s)
                .unwrap_or(first_bar);
            let mut cells: Vec<gpui::AnyElement> = Vec::new();
            if start_bar > 0 {
                cells.push(div().w(px(start_bar as f32 * bw)).flex_shrink_0().into_any_element());
            }
            let mut bar = start_bar;
            while bar < last_bar {
                if let Some((_start, span, name, pid)) = blocks.iter().find(|(s, _, _, _)| *s == bar).cloned() {
                    let tid_c = tid.clone();
                    let tid_x = tid.clone();
                    let tid_cyc = tid.clone();
                    let bx = bar;
                    let drag = ClipDrag { track: tid.clone(), bar, name: name.clone(), color };
                    let drop_tid = tid.clone();
                    let drop_bar = bar;
                    cells.push(
                        div()
                            .id(SharedString::from(format!("clip-{tid}-{bar}")))
                            .w(px(bw * span as f32))
                            .flex_shrink_0()
                            .h(px(th - 8.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(color))
                            .text_color(rgb(BG))
                            .px_2()
                            .overflow_hidden()
                            .relative()
                            // Mini content preview (absolute underlay; the
                            // name + buttons row paints on top).
                            .child({
                                let marks = self
                                    .st
                                    .project
                                    .patterns
                                    .iter()
                                    .find(|p| p.id() == pid)
                                    .map(|p| clip_preview_marks(p, bw * span as f32, th - 8.0, span * daw_core::sequencer::TICKS_PER_BAR))
                                    .unwrap_or_default();
                                div().absolute().inset_0().children(marks.into_iter().map(|(x, y, w, h)| {
                                    div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).rounded_sm().bg(rgb(BG))
                                }))
                            })
                            // Hover: full pattern name even on 1-bar slivers.
                            .tooltip({
                                let tip: SharedString = format!(
                                    "{name} · {pid} · {span} bar{}",
                                    if span == 1 { "" } else { "s" }
                                )
                                .into();
                                move |_w, cx| cx.new(|_| TextTooltip { text: tip.clone() }).into()
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(name),
                            )
                            // Cycle this clip to the track's next pattern, in place.
                            .child(
                                div()
                                    .id(SharedString::from(format!("clipcyc-{tid}-{bar}")))
                                    .px_1()
                                    .text_color(rgb(BG))
                                    .cursor_pointer()
                                    .child("⟳")
                                    .on_click(cx.listener(move |this, _e, _w, cx| {
                                        this.undo.record("pattern", &this.st.project);
                                        this.st.project.cycle_clip_pattern(&tid_cyc, bx);
                                        this.push_live();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("clipx-{tid}-{bar}")))
                                    .px_1()
                                    .text_color(rgb(BG))
                                    .cursor_pointer()
                                    .child("✕")
                                    .on_click(cx.listener(move |this, _e, _w, cx| {
                                        this.undo.record("remove clip", &this.st.project);
                                        this.st.project.remove_clip(&tid_x, bx);
                                        this.push_live();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("rzc-{tid}-{bar}")))
                                    .w(px(6.))
                                    .h(px(th - 8.0))
                                    .flex_shrink_0()
                                    .cursor_pointer()
                                    .bg(rgb(BG))
                                    .on_drag(ResizeClip { track: tid.clone(), bar }, |_, _, _, cx| cx.new(|_| EmptyDrag))
                                    .on_drag_move(cx.listener({
                                        let tr = tid.clone();
                                        let bw = bw;
                                        move |this, e: &DragMoveEvent<ResizeClip>, _w, cx| {
                                            // Only the clip actually being dragged should resize:
                                            // on_drag_move fires for every ResizeClip handler.
                                            let (dt, db) = { let p = e.drag(cx); (p.track.clone(), p.bar) };
                                            if dt != tr || db != bar { return; }
                                            // handle's left edge ≈ clip's right edge; how far the
                                            // cursor is past it (in bars) is the length delta. The
                                            // handle re-renders at the new edge each step, so this
                                            // converges to the cursor without feedback.
                                            let handle_x = f32::from(e.bounds.origin.x);
                                            let mouse_x = f32::from(e.event.position.x);
                                            let delta = ((mouse_x - handle_x) / bw).round() as i32;
                                            if delta != 0 {
                                                if let Some(cur) = this.st.project.clip_len_bars(&tr, bar) {
                                                    let new = (cur as i32 + delta).max(1) as u32;
                                                    this.undo.record("resize clip", &this.st.project);
                                                    if this.st.project.resize_clip(&tr, bar, new) {
                                                        this.push_live();
                                                        cx.notify();
                                                    }
                                                }
                                            }
                                        }
                                    })),
                            )
                            .on_click(cx.listener(move |this, _e, _w, cx| {
                                this.st.select_track(&tid_c);
                                this.sync_midi_strip();
                                this.st.select_pattern(&pid);
                                // Set the playback start to this clip's first bar so
                                // Play begins from here (matches DAW arrange behavior).
                                this.seek_to_bar(bx);
                                cx.notify();
                            }))
                            .on_drag(drag, |d, _off, _win, cx| {
                                cx.new(|_| ClipDragPreview { name: d.name.clone(), color: d.color })
                            })
                            .on_drop(cx.listener(move |this, d: &ClipDrag, _w, cx| {
                                this.undo.record("move clip", &this.st.project);
                                if this.st.project.move_clip(&d.track, d.bar, &drop_tid, drop_bar) {
                                    this.push_live();
                                }
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                    bar += span.max(1);
                } else {
                    let here = bar == cur_bar;
                    let tid_c = tid.clone();
                    let drop_tid = tid.clone();
                    let b = bar;
                    cells.push(
                        div()
                            .id(SharedString::from(format!("cell-{tid}-{bar}")))
                            .w(px(bw))
                            .flex_shrink_0()
                            .h(px(th))
                            .cursor_pointer()
                            .border_r_1()
                            .border_color(rgb(PANEL))
                            .bg(rgb(if here { PANEL_ALT } else { BG }))
                            .on_click(cx.listener(move |this, _e, _w, cx| {
                                this.place_in_arrange(&tid_c, b);
                                cx.notify();
                            }))
                            .on_drop(cx.listener(move |this, d: &ClipDrag, _w, cx| {
                                this.undo.record("move clip", &this.st.project);
                                if this.st.project.move_clip(&d.track, d.bar, &drop_tid, b) {
                                    this.push_live();
                                }
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                    bar += 1;
                }
            }

            let audio_blocks: Vec<gpui::AnyElement> = self.st.project.audio_clips.iter().enumerate()
                .filter(|(_, c)| c.track == t.id)
                // skip clips entirely outside the visible scroll window
                .filter(|(_, c)| {
                    let left = (c.start_sec as f32) * pps;
                    let right = left + ((c.len_sec as f32) * pps).max(2.0);
                    right >= scroll_x && left <= scroll_x + vis_w
                })
                .map(|(idx, c)| {
                    let left  = (c.start_sec as f32) * pps;
                    let width = ((c.len_sec as f32) * pps).max(2.0);
                    let h = th - 8.0;
                    let dur = self.st.peaks_dur_cache.get(&c.path).copied()
                        .unwrap_or(c.offset_sec + c.len_sec);
                    let start_frac = if dur > 0.0 { c.offset_sec / dur } else { 0.0 };
                    let end_frac   = if dur > 0.0 { (c.offset_sec + c.len_sec) / dur } else { 1.0 };
                    let cols = ((width / 2.0) as usize).clamp(1, 400);
                    let mags = match self.st.peaks_cache.get(&c.path) {
                        Some(pk) => waveform_columns(pk, start_frac, end_frac, cols),
                        None => Vec::new(),
                    };
                    let name = std::path::Path::new(&c.path).file_name()
                        .map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    let start_sec = c.start_sec;
                    div()
                        .id(SharedString::from(format!("aclip-{idx}")))
                        .absolute()
                        .left(px(left))
                        .top(px(4.))
                        .w(px(width))
                        .h(px(h))
                        .rounded_md()
                        .bg(rgb(PANEL_ALT))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .overflow_hidden()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            // Start playback from exactly where this audio clip begins.
                            this.seek_to_sec(start_sec);
                            cx.notify();
                        }))
                        .on_drag(AudioMove { idx }, |_, _, _, cx| cx.new(|_| EmptyDrag))
                        .on_drag_move(cx.listener({
                            move |this, e: &DragMoveEvent<AudioMove>, _w, cx| {
                                // Only the dragged clip moves (fires for every AudioMove handler).
                                if e.drag(cx).idx != idx { return; }
                                // Block's current left (px) tracks start_sec; move it so its
                                // left follows the cursor. Converges as the block re-renders.
                                let block_x = f32::from(e.bounds.origin.x);
                                let mouse_x = f32::from(e.event.position.x);
                                let delta_sec = (mouse_x - block_x) / pps;
                                if delta_sec.abs() > 0.001 {
                                    let cur = this.st.project.audio_clips.get(idx).map(|c| c.start_sec).unwrap_or(0.0);
                                    this.undo.record("move audio clip", &this.st.project);
                                    this.st.project.move_audio_clip(idx, cur + delta_sec as f64);
                                    this.push_live();
                                    cx.notify();
                                }
                            }
                        }))
                        .child(
                            div().absolute().inset_0().flex().items_center()
                                .children(mags.into_iter().map(|m|
                                    div().w(px(1.)).h(px((m * h).max(1.0))).mr(px(1.)).bg(rgb(accent))
                                ))
                        )
                        .child(div().absolute().top_0().left_0().px_1().text_color(rgb(TEXT)).child(name))
                        .child(
                            div().id(SharedString::from(format!("aclipx-{idx}")))
                                .absolute().top_0().right_0().px_1().cursor_pointer()
                                .text_color(rgb(TEXT)).child("✕")
                                .on_click(cx.listener(move |this, _e, _w, cx| {
                                    this.undo.record("remove audio clip", &this.st.project);
                                    this.st.project.remove_audio_clip(idx);
                                    this.push_live();
                                    cx.notify();
                                }))
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("acl-{idx}")))
                                .absolute().top_0().left_0().bottom_0()
                                .w(px(6.))
                                .cursor_pointer()
                                .bg(rgb(BORDER))
                                .on_drag(AudioCropL { idx }, |_, _, _, cx| cx.new(|_| EmptyDrag))
                                .on_drag_move(cx.listener(move |this, e: &DragMoveEvent<AudioCropL>, _w, cx| {
                                    if e.drag(cx).idx != idx { return; }
                                    // handle's left ≈ clip's left edge; cursor delta (px→sec) trims the front.
                                    let handle_x = f32::from(e.bounds.origin.x);
                                    let mouse_x = f32::from(e.event.position.x);
                                    let delta_sec = (mouse_x - handle_x) / pps;
                                    if delta_sec.abs() > 0.001 {
                                        this.undo.record("crop clip", &this.st.project);
                                        this.st.project.crop_audio_clip_start(idx, delta_sec as f64);
                                        this.push_live();
                                        cx.notify();
                                    }
                                }))
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("acr-{idx}")))
                                .absolute().top_0().right_0().bottom_0()
                                .w(px(6.))
                                .cursor_pointer()
                                .bg(rgb(BORDER))
                                .on_drag(AudioCropR { idx }, |_, _, _, cx| cx.new(|_| EmptyDrag))
                                .on_drag_move(cx.listener(move |this, e: &DragMoveEvent<AudioCropR>, _w, cx| {
                                    if e.drag(cx).idx != idx { return; }
                                    // handle sits at clip's right edge; (mouse - handle) is the length delta.
                                    // Read len_sec into a local first to avoid borrow conflict.
                                    let handle_x = f32::from(e.bounds.origin.x);
                                    let mouse_x = f32::from(e.event.position.x);
                                    let delta_sec = (mouse_x - handle_x) / pps;
                                    let cur_len = this.st.project.audio_clips.get(idx).map(|c| c.len_sec).unwrap_or(0.01);
                                    let new_len = (cur_len + delta_sec as f64).max(0.01);
                                    if delta_sec.abs() > 0.001 {
                                        this.undo.record("crop clip", &this.st.project);
                                        this.st.project.crop_audio_clip_end(idx, new_len);
                                        this.push_live();
                                        cx.notify();
                                    }
                                }))
                        )
                        .into_any_element()
                }).collect();

            div()
                .relative()
                .flex()
                .items_center()
                .h(px(th))
                .border_b_1()
                .border_color(rgb(BG))
                .children(cells)
                .children(audio_blocks)
        });

        // Manual horizontal scroll: we shift the content left by `scroll_x` (computed
        // above) inside an overflow-hidden viewport. We do NOT use gpui's
        // overflow_x_scroll/track_scroll here because a plain mouse wheel emits only
        // vertical deltas (which an x-only scroll ignores) and because the scroll
        // element overwrites a handle's offset from its own input state each frame —
        // so set_offset had no lasting effect.
        div()
            .flex_1()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .min_w(px(0.))
            .child(
                div()
                    .id("arrange-scroll")
                    .flex_1()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(|this, e: &gpui::ScrollWheelEvent, _w, cx| {
                        // Ctrl+wheel = zoom; any other wheel = horizontal scroll.
                        let dy = match e.delta {
                            gpui::ScrollDelta::Lines(p)  => p.y,
                            gpui::ScrollDelta::Pixels(p) => f32::from(p.y),
                        };
                        if dy == 0.0 { return; }
                        let content_w = this.st.project.song_bars() as f32 * this.bar_w();

                        if e.control {
                            let old_bw = this.bar_w();
                            let factor = if dy > 0.0 { 1.1 } else { 1.0 / 1.1 };
                            this.st.arrange_zoom = (this.st.arrange_zoom * factor)
                                .clamp(MIN_BAR_W / BAR_W, MAX_BAR_W / BAR_W);
                            let new_bw = this.bar_w();
                            // Keep the left-edge bar roughly fixed as zoom changes.
                            this.st.arrange_scroll_x =
                                (this.st.arrange_scroll_x * new_bw / old_bw).max(0.0);
                            cx.notify();
                            return;
                        }

                        // Wheel down (dy < 0) advances toward later bars. Lines→px per notch.
                        let step = match e.delta {
                            gpui::ScrollDelta::Lines(p)  => p.y * 48.0,
                            gpui::ScrollDelta::Pixels(p) => f32::from(p.y),
                        };
                        let max_off = (content_w - 100.0).max(0.0);
                        this.st.arrange_scroll_x =
                            (this.st.arrange_scroll_x - step).clamp(0.0, max_off);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .relative()
                            .left(px(-scroll_x))
                            .flex()
                            .flex_col()
                            .child(region_row)
                            .child(ruler)
                            .child(div().flex().flex_col().children(lanes)),
                    ),
            )
    }

    /// Left dock: search field + (when search empty) GM category list + presets,
    /// or (when non-empty) a flat filtered list of matching instruments.
    fn browser(&self, cx: &mut Context<Self>) -> gpui::Div {
        let q = self.st.browser_search.clone();
        let accent = self.accent();
        // The preset currently plugged into the selected track — highlighted in
        // the lists below so you can see (and confirm) what's loaded.
        let active_preset: Option<u8> = self.st.selected_track.as_ref()
            .and_then(|tid| self.st.project.tracks.iter().find(|t| &t.id == tid))
            .and_then(|t| match &t.instrument {
                // Only melodic-bank presets correspond to the browser list.
                daw_core::project::Instrument::Soundfont { preset, bank: 0, .. } => Some(*preset),
                _ => None,
            });

        // Search input — mirrors the REPL pattern.
        let search_field = div()
            .id("browser-search")
            .key_context("BrowserSearch")
            .track_focus(&self.search_focus)
            .px_2()
            .py_1()
            .m_1()
            .rounded_md()
            .bg(rgb(PANEL_ALT))
            .text_color(rgb(TEXT))
            .cursor_pointer()
            .child(if q.is_empty() {
                SharedString::from("search instruments…")
            } else {
                SharedString::from(format!("{}▎", q))
            })
            .on_click(cx.listener(|this, _e, window, _cx| {
                this.search_focus.focus(window);
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
                let key = ev.keystroke.key.as_str();
                match key {
                    "backspace" => { this.st.browser_search.pop(); }
                    "escape"    => { this.st.browser_search.clear(); }
                    _ => {
                        // Prefer key_char (shifted chars/punctuation), but fall back
                        // to the key name — with IME disabled (the XWayland launch),
                        // key_char is None for plain letters, so without this the
                        // field captured nothing. Mirrors the Code REPL input.
                        if let Some(s) = &ev.keystroke.key_char {
                            this.st.browser_search.push_str(s);
                        } else if key.chars().count() == 1 {
                            this.st.browser_search.push_str(key);
                        }
                    }
                }
                cx.notify();
            }));

        let cat = self.st.browser_category.min(GM_CATEGORIES.len() - 1);

        let mut panel = div()
            .w(px(self.st.browser_w))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(section_title("Browser"))
            .child(search_field);

        if q.is_empty() {
            // Normal view: category list + preset list.
            let cats = GM_CATEGORIES.iter().enumerate().map(|(i, (name, _))| {
                let is_sel = i == cat;
                div()
                    .id(SharedString::from(format!("cat-{i}")))
                    .px_2().py_1().rounded_md().cursor_pointer().whitespace_nowrap()
                    .bg(rgb(if is_sel { PANEL_ALT } else { PANEL }))
                    .text_color(rgb(if is_sel { TEXT } else { TEXT_DIM }))
                    .child(*name)
                    .on_click(cx.listener(move |this, _e, _w, cx| { this.st.set_browser_category(i); cx.notify(); }))
            });

            let presets = GM_CATEGORIES[cat].1.iter().map(|(name, prog)| {
                let prog = *prog;
                let is_active = active_preset == Some(prog);
                div()
                    .id(SharedString::from(format!("preset-{prog}")))
                    .px_2().py_1().rounded_md().cursor_pointer().whitespace_nowrap()
                    .bg(rgb(if is_active { accent } else { PANEL }))
                    .text_color(rgb(if is_active { BG } else { TEXT }))
                    .child(format!("{name} ({prog})"))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        if let Some(tid) = this.st.selected_track.clone() {
                            this.undo.record("preset", &this.st.project);
                            if this.st.project.set_track_preset(&tid, prog) {
                                this.push_live();
                            }
                        }
                        cx.notify();
                    }))
            });

            panel = panel
                .child(div().flex().flex_col().gap_1().p_1().overflow_hidden().children(cats))
                .child(section_title("Presets"))
                .child(div().flex().flex_col().gap_1().p_1().overflow_hidden().children(presets));
        } else {
            // Filtered view: flat list of matching presets across all categories.
            let needle = q.to_lowercase();
            let matches = GM_CATEGORIES.iter().flat_map(|(cat_name, presets)| {
                presets.iter().map(move |(name, prog)| (*cat_name, *name, *prog))
            })
            // Match the preset name OR its category, so a category word like
            // "bass"/"piano"/"guitar" finds its presets (which are named
            // "Acoustic"/"Finger"/… and don't contain the category word).
            .filter(|(cat_name, name, _)| {
                name.to_lowercase().contains(&needle) || cat_name.to_lowercase().contains(&needle)
            })
            .map(|(cat_name, name, prog)| {
                let is_active = active_preset == Some(prog);
                div()
                    .id(SharedString::from(format!("hit-{prog}-{name}")))
                    .px_2().py_1().rounded_md().cursor_pointer()
                    .bg(rgb(if is_active { accent } else { PANEL }))
                    .text_color(rgb(if is_active { BG } else { TEXT }))
                    .child(format!("{name}  ·  {cat_name}"))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        if let Some(tid) = this.st.selected_track.clone() {
                            this.undo.record("preset", &this.st.project);
                            if this.st.project.set_track_preset(&tid, prog) { this.push_live(); }
                        }
                        cx.notify();
                    }))
            });

            panel = panel
                .child(section_title("Matches"))
                .child(div().flex().flex_col().gap_1().p_1().overflow_hidden().children(matches));
        }

        // CLAP plugins (background discovery scan). Instruments retarget the
        // selected track; effects append to its insert chain.
        if !self.st.clap_plugins.is_empty() {
            let rows = self.st.clap_plugins.iter().cloned().map(|d| {
                let tag = if d.is_instrument { "[I]" } else { "[FX]" };
                let label = format!("{tag} {}", d.name);
                let key = format!("clap-{}-{}", d.path, d.id);
                div()
                    .id(SharedString::from(key))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(PANEL))
                    .text_color(rgb(TEXT))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(label)
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        if d.is_instrument {
                            this.set_clap_instrument(d.path.clone(), d.id.clone());
                        } else {
                            this.add_clap_fx(d.path.clone(), d.id.clone());
                        }
                        cx.notify();
                    }))
            });
            panel = panel
                .child(section_title("CLAP"))
                .child(div().flex().flex_col().gap_1().p_1().overflow_hidden().children(rows));
        }

        panel
    }

    /// Bottom dock with tabs; the editors behind them arrive in later phases.
    fn editor_dock(&self, cx: &mut Context<Self>) -> gpui::Div {
        let active = self.st.active_dock;
        let tabs = div()
            .flex()
            .gap_1()
            .px_2()
            .py_1()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .children(DockTab::ALL.into_iter().map(|tab| {
                let selected = tab == active;
                div()
                    .id(SharedString::from(tab.label()))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(if selected { PANEL_ALT } else { PANEL }))
                    .text_color(rgb(if selected { TEXT } else { TEXT_DIM }))
                    .child(tab.label())
                    .on_click(cx.listener(move |this, _ev, _w, cx| {
                        this.st.active_dock = tab;
                        cx.notify();
                    }))
            }));

        let picker = self.pattern_picker(cx);
        let body = match active {
            DockTab::Sequencer => self.sequencer_editor(cx),
            DockTab::PianoRoll => self.piano_roll_editor(cx),
            DockTab::Fx => self.fx_editor(cx),
            DockTab::Code => self.code_editor(cx),
        };

        div()
            .h(px(self.st.dock_h))
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .border_t_1()
            .border_color(rgb(BORDER))
            .child(tabs)
            .child(picker)
            .child(div().flex_1().overflow_hidden().child(body))
    }

    /// Step-grid editor for the selected pattern (when it's a step pattern).
    fn sequencer_editor(&self, cx: &mut Context<Self>) -> gpui::Div {
        let accent = self.accent();
        let Some(pid) = self.st.selected_pattern_id() else {
            return dock_hint("Select a track with a step pattern.");
        };
        let pat = self.st.project.patterns.iter().find(|p| p.id() == pid);
        let Some(daw_core::project::Pattern::Step(sp)) = pat else {
            return dock_hint("This pattern isn't a step pattern — use the Piano Roll tab.");
        };

        let steps = sp.steps as usize;
        let gate = sp.gate;
        // Steps are 16th notes (30 ticks); the play column wraps over the
        // whole pattern length (steps × 30).
        let ticks_per_step = daw_core::sequencer::TICKS_PER_BAR / 16;
        let pat_len = (steps as u32 * ticks_per_step).max(1);
        let play_step = ((self.st.playhead() % pat_len) / ticks_per_step) as usize;

        // Collect per-row data into owned values to avoid holding a borrow on sp
        // while building the children (which need cx).
        let row_data: Vec<(String, u8, Vec<bool>)> = sp.rows.iter().map(|row| {
            let name = row.name.clone();
            let pitch = row.pitch;
            let step_flags: Vec<bool> = (0..steps).map(|si| row.step_velocity(si).is_some()).collect();
            (name, pitch, step_flags)
        }).collect();

        // Build toolbar — captures clones of pid, not sp.
        let pid_t = pid.clone();
        let toolbar = {
            let (p1, p2, p3, p4, p5) = (pid.clone(), pid.clone(), pid.clone(), pid.clone(), pid.clone());
            div().flex().items_center().gap_2().pb_1().text_color(rgb(TEXT_DIM))
                .child(div().child(format!("{} steps", steps)))
                .child(cell_btn("seq-steps-down", "−4", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.undo.record("steps", &this.st.project); this.st.project.resize_steps(&p1, -4); this.push_live(); cx.notify(); })))
                .child(cell_btn("seq-steps-up", "+4", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.undo.record("steps", &this.st.project); this.st.project.resize_steps(&p2, 4); this.push_live(); cx.notify(); })))
                .child(cell_btn("seq-addrow", "+ Row", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.undo.record("add row", &this.st.project); this.st.project.add_step_row(&p3); this.push_live(); cx.notify(); })))
                .child(div().child(format!("gate {:.2}", gate)))
                .child(cell_btn("seq-gate-down", "−", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| {
                    let g = (this.cur_gate(&p4) - 0.05).max(0.1); this.undo.record("gate", &this.st.project); this.st.project.set_gate(&p4, g); this.push_live(); cx.notify();
                })))
                .child(cell_btn("seq-gate-up", "+", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| {
                    let g = (this.cur_gate(&p5) + 0.05).min(2.0); this.undo.record("gate", &this.st.project); this.st.project.set_gate(&p5, g); this.push_live(); cx.notify();
                })))
        };
        let _ = pid_t;

        // When the selected track is a Sampler, expose a per-row "load sample"
        // button (pitch -> wav path) so a kit can be built entirely in the GUI.
        let (sampler_tid, pad_paths): (Option<String>, std::collections::HashMap<u8, String>) = {
            let track = self.st.selected_track.as_ref()
                .and_then(|tid| self.st.project.tracks.iter().find(|t| &t.id == tid));
            match track.map(|t| (t.id.clone(), &t.instrument)) {
                Some((id, daw_core::project::Instrument::Sampler { pads })) =>
                    (Some(id), pads.iter().map(|p| (p.pitch, p.path.clone())).collect()),
                _ => (None, std::collections::HashMap::new()),
            }
        };

        let rows = row_data.into_iter().enumerate().map(|(ri, (row_name, row_pitch, step_flags))| {
            let pitch_tag = format!("p{row_pitch}");
            let label = if row_name == pitch_tag {
                pitch_tag.clone()
            } else {
                format!("{} {}", trunc(&row_name, 8), pitch_tag)
            };
            // Sampler-pad load button for this row's pitch (sampler tracks only).
            let pad_btn = sampler_tid.clone().map(|tid| {
                let loaded = pad_paths.get(&row_pitch).cloned();
                let lbl = loaded.as_deref()
                    .and_then(|p| std::path::Path::new(p).file_name())
                    .map(|s| trunc(&s.to_string_lossy(), 12))
                    .unwrap_or_else(|| "+ wav".to_string());
                cell_btn(format!("pad-{ri}"), lbl, loaded.is_some(), accent)
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.load_pad_dialog(tid.clone(), row_pitch);
                        cx.notify();
                    }))
            });
            let cells: Vec<_> = step_flags.iter().enumerate().map(|(si, &on)| {
                let is_play_col = si == play_step;
                let beat = si % 4 == 0;
                // Off cells are clearly visible; beat starts read brighter.
                let bg = if on { accent }
                         else if is_play_col { 0x4a4a4a }
                         else if beat { 0x383838 }
                         else { PANEL_ALT };
                let pid_c = pid.clone();
                div()
                    .id(SharedString::from(format!("step-{pid}-{ri}-{si}")))
                    .w(px(STEP_W))
                    .h(px(STEP_W))
                    .flex_shrink_0()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(bg))
                    .border_1()
                    .border_color(rgb(BORDER))
                    // Drum-machine grouping: extra gap before each 4-step block.
                    .when(beat && si > 0, |d| d.ml(px(6.)))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.undo.record("toggle step", &this.st.project);
                        this.st.project.toggle_step(&pid_c, ri, si);
                        this.push_live();
                        cx.notify();
                    }))
            }).collect();
            div()
                .flex()
                .items_center()
                .gap_1()
                .h(px(STEP_W + 4.0))
                .child(if self.st.renaming == Some(RenameTarget::Row(pid.clone(), ri)) {
                    div().w(px(96.)).flex_shrink_0().child(self.rename_editor(cx)).into_any_element()
                } else {
                    let (pid_r, name_r) = (pid.clone(), row_name.clone());
                    div()
                        .id(SharedString::from(format!("rowlbl-{ri}")))
                        .w(px(96.))
                        .flex_shrink_0()
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .text_color(rgb(TEXT_DIM))
                        .child(label)
                        .on_click(cx.listener(move |this, e: &gpui::ClickEvent, w, cx| {
                            if e.click_count() >= 2 {
                                this.start_rename(RenameTarget::Row(pid_r.clone(), ri), name_r.clone());
                                this.rename_focus.clone().focus(w);
                                cx.notify();
                            }
                        }))
                        .into_any_element()
                })
                .children(pad_btn)
                .child(cell_btn(format!("rp-down-{ri}"), "−", false, PANEL_ALT).on_click(cx.listener({
                    let pid = pid.clone();
                    move |this, _e, _w, cx| { this.undo.record("row pitch", &this.st.project); this.st.project.nudge_row_pitch(&pid, ri, -1); this.push_live(); cx.notify(); }
                })))
                .child(cell_btn(format!("rp-up-{ri}"), "+", false, PANEL_ALT).on_click(cx.listener({
                    let pid = pid.clone();
                    move |this, _e, _w, cx| { this.undo.record("row pitch", &this.st.project); this.st.project.nudge_row_pitch(&pid, ri, 1); this.push_live(); cx.notify(); }
                })))
                .children(cells)
        });

        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .overflow_hidden()
            .child(toolbar)
            .children(rows)
    }

    /// Read the current gate value for a step pattern by id (avoids stale captures).
    fn cur_gate(&self, pid: &str) -> f32 {
        self.st.project.patterns.iter().find_map(|p| match p {
            daw_core::project::Pattern::Step(sp) if sp.id == pid => Some(sp.gate),
            _ => None,
        }).unwrap_or(0.5)
    }

    /// Pitch×time grid editor for the selected pattern (when it's piano-roll).
    fn piano_roll_editor(&self, cx: &mut Context<Self>) -> gpui::Div {
        let accent = self.accent();
        const TICKS_PER_COL: u32 = 120; // 16th-note columns (480/bar)
        let Some(pid) = self.st.selected_pattern_id() else {
            return dock_hint("Select a track with a piano-roll pattern.");
        };
        let pat = self.st.project.patterns.iter().find(|p| p.id() == pid);
        let Some(daw_core::project::Pattern::PianoRoll(pr)) = pat else {
            return dock_hint("This pattern isn't a piano-roll pattern — use the Sequencer tab.");
        };

        let pat_len = pr.ticks_per_bar * pr.bars.max(1) as u32;
        let cols = (pat_len / TICKS_PER_COL).max(1) as usize;

        // Pitch window: span the pattern's notes with generous padding so notes
        // can be added above/below; the grid scrolls vertically through it.
        // Default ~C3..C6 when empty; always at least ~3 octaves tall.
        let (mut lo, mut hi) = (48u8, 84u8);
        if let (Some(min), Some(max)) =
            (pr.notes.iter().map(|n| n.pitch).min(), pr.notes.iter().map(|n| n.pitch).max())
        {
            lo = min.saturating_sub(8);
            hi = (max as u16 + 8).min(127) as u8;
        }
        if hi.saturating_sub(lo) < 36 {
            hi = lo.saturating_add(36).min(127);
            lo = hi.saturating_sub(36);
        }
        let play_col = ((self.st.playhead() % pat_len.max(1)) / TICKS_PER_COL) as usize;

        // Slim toolbar: pattern length in bars (notes auto-grow it too).
        let pr_bars = pr.bars;
        let (pb1, pb2) = (pid.clone(), pid.clone());
        let toolbar = div().flex().items_center().gap_2().pb_1().px_2().text_color(rgb(TEXT_DIM))
            .child(div().child(format!("{pr_bars} bars")))
            .child(cell_btn("pr-bars-down", "−Bar", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| {
                this.undo.record("pr bars", &this.st.project);
                this.st.project.resize_pr_bars(&pb1, -1);
                this.push_live();
                cx.notify();
            })))
            .child(cell_btn("pr-bars-up", "+Bar", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| {
                this.undo.record("pr bars", &this.st.project);
                this.st.project.resize_pr_bars(&pb2, 1);
                this.push_live();
                cx.notify();
            })));

        // High pitch at top down to low.
        let rows = (lo..=hi).rev().map(|pitch| {
            let is_black = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
            let label = note_name(pitch);
            let cells: Vec<_> = (0..cols).map(|c| {
                let tick = c as u32 * TICKS_PER_COL;
                // Span rendering: starts are full accent, continuations dimmer.
                let (is_start, is_cont) = pr.notes.iter().fold((false, false), |(st, ct), n| {
                    if n.pitch != pitch {
                        return (st, ct);
                    }
                    (st || n.tick == tick, ct || (n.tick < tick && tick < n.tick + n.duration))
                });
                let is_play_col = c == play_col;
                let bg = if is_start { accent }
                    else if is_cont { 0x9a6a2a } // dimmed accent: held note body
                    else if is_play_col { PANEL_ALT }
                    else if is_black { BG }
                    else { PANEL };
                let pid_c = pid.clone();
                let cell = div()
                    .id(SharedString::from(format!("nt-{pid}-{pitch}-{c}")))
                    .w(px(PR_COL_W))
                    .h(px(PR_ROW_H))
                    .flex_shrink_0()
                    .cursor_pointer()
                    .bg(rgb(bg))
                    .border_1()
                    .border_color(rgb(BG))
                    // Drop target: stretch a dragged note (same row) to end here.
                    .on_drop(cx.listener({
                        let pid_d = pid.clone();
                        move |this, d: &NoteStretch, _w, cx| {
                            if d.pid != pid_d || d.pitch != pitch || tick < d.tick {
                                return;
                            }
                            let dur = tick + TICKS_PER_COL - d.tick;
                            this.undo.record("stretch note", &this.st.project);
                            this.st.project.set_note_duration(&d.pid, d.pitch, d.tick, dur);
                            this.push_live();
                            cx.notify();
                        }
                    }));
                if is_start {
                    // Start cells: drag to stretch; click still toggles off.
                    cell.on_drag(
                        NoteStretch { pid: pid.clone(), pitch, tick },
                        |_, _, _, cx| cx.new(|_| EmptyDrag),
                    )
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.undo.record("toggle note", &this.st.project);
                        this.st.project.toggle_note(&pid_c, pitch, tick, TICKS_PER_COL);
                        this.push_live();
                        cx.notify();
                    }))
                    .into_any_element()
                } else if is_cont {
                    // Continuation cells are drop targets only.
                    cell.into_any_element()
                } else {
                    cell.on_click(cx.listener(move |this, _e, _w, cx| {
                        this.undo.record("toggle note", &this.st.project);
                        this.st.project.toggle_note(&pid_c, pitch, tick, TICKS_PER_COL);
                        this.push_live();
                        cx.notify();
                    }))
                    .into_any_element()
                }
            }).collect();
            div()
                .flex()
                .items_center()
                .gap_1()
                .h(px(PR_ROW_H + 1.0))
                .child(div().w(px(40.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(TEXT_DIM)).child(label))
                .children(cells)
        });

        // Fill the dock body and scroll the (taller) pitch grid vertically; long
        // patterns also scroll horizontally. The `.id` persists the scroll offset.
        // Wrapped in a plain div so the return type stays `gpui::Div` (a `.id()`
        // chain is `Stateful<Div>`) — matching the other editor_dock match arms.
        div().size_full().flex().flex_col().child(toolbar).child(
            div()
                .id("pr-scroll")
                .size_full()
                .overflow_scroll()
                .flex()
                .flex_col()
                .p_2()
                .children(rows),
        )
    }

    /// Right-dock mixer: one vertical channel strip per track.
    fn mixer(&self, cx: &mut Context<Self>) -> gpui::Div {
        let accent = self.accent();
        let sel = self.st.selected_track.clone();
        // Live per-strip peaks from the engine mixer (index = track index).
        let peaks: Vec<(f32, bool)> = self
            .host
            .strip_meters
            .lock()
            .map(|v| v.iter().map(|m| m.read()).collect())
            .unwrap_or_default();
        let strips = self.st.project.tracks.iter().enumerate().map(|(ti, t)| {
            let (peak, clipped) = peaks.get(ti).copied().unwrap_or((0.0, false));
            let id = t.id.clone();
            let is_sel = sel.as_deref() == Some(id.as_str());
            let vol = t.volume;
            let pan = t.pan;
            let muted = t.muted;
            let soloed = self.st.soloed.contains(&id);
            let name = trunc(&t.name, 6);

            let fader = div()
                .w(px(14.))
                .h(px(96.))
                .relative()
                .flex()
                .flex_col()
                .justify_end()
                .bg(rgb(PANEL_ALT))
                .rounded_md()
                .overflow_hidden()
                .child(div().w(px(14.)).h(px((96.0 * vol / 2.0).clamp(0.0, 96.0))).bg(rgb(accent)))
                // Unity notch: vol = 1.0 sits at half height.
                .child(div().absolute().left_0().bottom(px(48.)).w(px(14.)).h(px(1.)).bg(rgb(TEXT_DIM)));

            // Post-fader peak meter beside the fader; red latch when clipped.
            let meter_color = if clipped { RECORD } else if peak > 0.7 { METER_WARN } else { 0x6ee87a };
            let meter = div()
                .w(px(6.))
                .h(px(96.))
                .flex()
                .flex_col()
                .justify_end()
                .bg(rgb(PANEL_ALT))
                .rounded_sm()
                .overflow_hidden()
                .child(div().w(px(6.)).h(px((96.0 * peak.min(1.0)).max(0.0))).bg(rgb(meter_color)));
            let fader = div().flex().gap_1().items_end().child(fader).child(meter);

            let (idu, idd, idm, ids, idpl, idpr) =
                (id.clone(), id.clone(), id.clone(), id.clone(), id.clone(), id.clone());

            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .px_1()
                .py_2()
                .w(px(64.))
                .flex_shrink_0()
                .bg(rgb(if is_sel { PANEL_ALT } else { PANEL }))
                .child(div().whitespace_nowrap().overflow_hidden().text_color(rgb(TEXT)).child(name))
                .child(cell_btn(format!("mv+{id}"), "+", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.nudge_volume(&idu, 0.05); cx.notify(); })))
                .child(fader)
                .child(cell_btn(format!("mv-{id}"), "−", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.nudge_volume(&idd, -0.05); cx.notify(); })))
                .child(div().text_color(rgb(TEXT_DIM)).child(format!("{:.0}", vol * 100.0)))
                .child(
                    div().flex().gap_1()
                        .child(cell_btn(format!("mpl{id}"), "◄", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.nudge_pan(&idpl, -0.1); cx.notify(); })))
                        .child(cell_btn(format!("mpr{id}"), "►", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.nudge_pan(&idpr, 0.1); cx.notify(); }))),
                )
                .child(div().text_color(rgb(TEXT_DIM)).child(format!("p{pan:+.1}")))
                .child(
                    div().flex().gap_1()
                        .child(cell_btn(format!("mm{id}"), "M", muted, RECORD).on_click(cx.listener(move |this, _e, _w, cx| { this.toggle_mute(&idm); cx.notify(); })))
                        .child(cell_btn(format!("ms{id}"), "S", soloed, accent).on_click(cx.listener(move |this, _e, _w, cx| { this.toggle_solo(&ids); cx.notify(); }))),
                )
        });

        // Return strips (RVB / DLY): fader = bus gain, live meter, mute,
        // click → bus params in the FX dock.
        let ret_peaks: [(f32, bool); 2] = [
            self.host.return_meters[0].read(),
            self.host.return_meters[1].read(),
        ];
        let returns = (0..2usize).map(|i| {
            let (label, gain, muted) = match i {
                0 => ("RVB", self.st.project.returns.reverb.gain, self.st.project.returns.reverb.muted),
                _ => ("DLY", self.st.project.returns.delay.gain, self.st.project.returns.delay.muted),
            };
            let (peak, clipped) = ret_peaks[i];
            let is_sel = self.st.selected_return == Some(i);
            let gain_param = if i == 0 {
                daw_core::project::ReturnParam::RevGain
            } else {
                daw_core::project::ReturnParam::DlyGain
            };
            let meter_color = if clipped { RECORD } else if peak > 0.7 { METER_WARN } else { 0x6ee87a };

            let fader = div()
                .w(px(14.))
                .h(px(96.))
                .flex()
                .flex_col()
                .justify_end()
                .bg(rgb(PANEL_ALT))
                .rounded_md()
                .overflow_hidden()
                .child(div().w(px(14.)).h(px((96.0 * gain / 1.25).clamp(0.0, 96.0))).bg(rgb(accent)));
            let meter = div()
                .w(px(6.))
                .h(px(96.))
                .flex()
                .flex_col()
                .justify_end()
                .bg(rgb(PANEL_ALT))
                .rounded_sm()
                .overflow_hidden()
                .child(div().w(px(6.)).h(px((96.0 * peak.min(1.0)).max(0.0))).bg(rgb(meter_color)));

            div()
                .id(SharedString::from(format!("return-{i}")))
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .px_1()
                .py_2()
                .w(px(64.))
                .flex_shrink_0()
                .cursor_pointer()
                .bg(rgb(if is_sel { PANEL_ALT } else { BG }))
                .border_t_2()
                .border_color(rgb(accent))
                .on_click(cx.listener(move |this, _e, _w, cx| {
                    this.st.selected_return = Some(i);
                    this.st.active_dock = DockTab::Fx;
                    cx.notify();
                }))
                .child(div().whitespace_nowrap().text_color(rgb(accent)).child(label))
                .child(cell_btn(format!("rg+{i}"), "+", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.nudge_return(gain_param, 0.05); cx.notify(); })))
                .child(div().flex().gap_1().items_end().child(fader).child(meter))
                .child(cell_btn(format!("rg-{i}"), "−", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.nudge_return(gain_param, -0.05); cx.notify(); })))
                .child(div().text_color(rgb(TEXT_DIM)).child(format!("{:.0}", gain * 100.0)))
                .child(cell_btn(format!("rm{i}"), "M", muted, RECORD).on_click(cx.listener(move |this, _e, _w, cx| { this.toggle_return_mute(i); cx.notify(); })))
        });

        div()
            .w(px(self.st.mixer_w))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_l_1()
            .border_color(rgb(BORDER))
            .child(
                div().flex().items_center().justify_between().pr_1()
                    .child(section_title("Mixer"))
                    .child(cell_btn("balance", "⚖", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| {
                        let project = this.st.project.clone();
                        let sf2 = std::path::PathBuf::from(this.sf2_path.clone());
                        let slot = std::sync::Arc::clone(&this.balance_result);
                        this.st.set_status("balancing… (solo-rendering each track)");
                        std::thread::spawn(move || {
                            match daw_core::audio::balance::auto_balance(&project, &sf2) {
                                Ok(plan) => {
                                    if let Ok(mut s) = slot.lock() {
                                        *s = Some(plan);
                                    }
                                }
                                Err(e) => eprintln!("balance failed: {e}"),
                            }
                        });
                        cx.notify();
                    })))
            )
            .child(div().flex().gap_1().p_1().overflow_hidden().children(strips).children(returns))
    }

    /// Single-line mini-notation REPL for the selected track.
    fn code_editor(&self, cx: &mut Context<Self>) -> gpui::Div {
        let buf = self.st.repl_buf.clone();
        let status = self.st.status.clone();

        // The inner stateful div carries the id (required for click events),
        // focus tracking, key_context, and key/click handlers.
        let inner = div()
            .id("repl")
            .key_context("ReplInput")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .on_click(cx.listener(|this, _e: &gpui::ClickEvent, window, _cx| {
                this.focus.focus(window);
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
                let key = ev.keystroke.key.as_str();
                match key {
                    "enter" => {
                        if let Some(tid) = this.st.selected_track.clone() {
                            let src = this.st.repl_buf.clone();
                            this.undo.record("REPL eval", &this.st.project);
                            match this.st.project.apply_mini_notation(&tid, &src) {
                                Ok(n) => {
                                    this.push_live();
                                    this.st.set_status(format!("✓ {n} notes → {tid}"));
                                }
                                Err(e) => this.st.set_status(format!("✗ {e}")),
                            }
                        }
                    }
                    "backspace" => this.st.repl_backspace(),
                    "escape" => this.st.repl_clear(),
                    _ => {
                        // Prefer key_char: includes shifted characters and punctuation.
                        if let Some(ch_str) = &ev.keystroke.key_char {
                            for c in ch_str.chars() {
                                this.st.repl_push(c);
                            }
                        } else if key.chars().count() == 1 {
                            if let Some(c) = key.chars().next() {
                                this.st.repl_push(c);
                            }
                        }
                    }
                }
                cx.notify();
            }))
            .child(
                div()
                    .text_color(rgb(TEXT_DIM))
                    .child("type mini-notation (e.g. `c4 e4 g4`), Enter to eval"),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(PANEL))
                    .text_color(rgb(TEXT))
                    .min_h(px(24.))
                    .child(if buf.is_empty() {
                        SharedString::from("▎")
                    } else {
                        SharedString::from(format!("{buf}▎"))
                    }),
            )
            .child(
                div()
                    .text_color(rgb(TEXT_DIM))
                    .child(SharedString::from(status)),
            );

        // Outer plain Div (no .id()) — required for the return type to be `gpui::Div`.
        div().flex_1().child(inner)
    }

    fn save_project(&mut self) {
        // Capture live plugin state into the project first (blob per slot).
        if !self.st.clap_meta.is_empty() {
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD;
            for (uid, blob) in self
                .host
                .collect_clap_state(std::time::Duration::from_millis(100))
            {
                self.st.project.set_clap_state(&uid, b64.encode(blob));
            }
        }
        let path = self.st.project_path.clone()
            .unwrap_or_else(|| std::path::PathBuf::from("untitled.daw.json"));
        match daw_core::project::format::save(&self.st.project, &path) {
            Ok(()) => {
                self.st.set_status(format!("saved {}", path.display()));
                // Record our own write's mtime so JAM doesn't reload what we just saved.
                self.st.project_mtime = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok());
                self.st.project_path = Some(path);
            }
            Err(e) => self.st.set_status(format!("save failed: {e}")),
        }
    }

    fn export_wav(&mut self) {
        // Don't start a second render while one is still pending/undrained.
        if self.export_result.lock().map(|s| s.is_some()).unwrap_or(false) {
            return;
        }
        let project = self.st.project.clone();
        let sf2 = std::path::PathBuf::from("assets/GeneralUser.sf2");
        let stem = self.st.project.project.name.replace(' ', "_");
        let out = std::path::PathBuf::from(format!("{stem}.wav"));
        let result = std::sync::Arc::clone(&self.export_result);
        self.st.set_status(format!("exporting {}…", out.display()));
        std::thread::spawn(move || {
            let msg = match daw_core::audio::export::export_wav(&project, &sf2, &out) {
                Ok(()) => format!("exported {}", out.display()),
                Err(e) => format!("export failed: {e}"),
            };
            if let Ok(mut slot) = result.lock() { *slot = Some(msg); }
        });
    }

    /// Open a native "Save As" file dialog; the chosen path is drained in pump(),
    /// which sets project_path and calls save_project().
    fn save_as_dialog(&mut self) {
        let slot = self.save_as_result.clone();
        std::thread::spawn(move || {
            if let Some(f) = rfd::FileDialog::new()
                .add_filter("daw project", &["json"])
                .set_file_name("song.daw.json")
                .save_file()
            {
                *slot.lock().unwrap() = Some(f);
            }
        });
    }

    /// Open a native WAV picker for Import Sample (works regardless of track
    /// selection — pump() creates an Audio track if none is selected).
    fn import_sample_dialog(&mut self) {
        let slot = self.audio_clip_result.clone();
        std::thread::spawn(move || {
            if let Some(f) = rfd::FileDialog::new()
                .add_filter("WAV", &["wav"])
                .pick_file()
            {
                *slot.lock().unwrap() = Some(f);
            }
        });
    }

    /// Open a WAV picker (background thread) for the Sampler pad at `pitch` on
    /// `track_id`. The pick is drained in `pump()` and applied via `set_sampler_pad`.
    fn load_pad_dialog(&self, track_id: String, pitch: u8) {
        let slot = self.pad_load_result.clone();
        std::thread::spawn(move || {
            if let Some(f) = rfd::FileDialog::new()
                .add_filter("WAV", &["wav"])
                .pick_file()
            {
                *slot.lock().unwrap() = Some((track_id, pitch, f));
            }
        });
    }

    /// Open a native file dialog (on a background thread) to pick a `.daw.json`
    /// project; the chosen path is drained and loaded in `pump`.
    fn open_dialog(&mut self) {
        if self.load_result.lock().map(|s| s.is_some()).unwrap_or(false) {
            return;
        }
        let result = std::sync::Arc::clone(&self.load_result);
        let dir = std::env::current_dir()
            .map(|d| d.join("projects"))
            .unwrap_or_else(|_| std::path::PathBuf::from("."));
        self.st.set_status("opening…");
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("daw project", &["json"])
                .set_directory(dir)
                .pick_file()
            {
                if let Ok(mut slot) = result.lock() {
                    *slot = Some(path);
                }
            }
        });
    }

    /// Replace the current project with one loaded from `path` and hand it to the
    /// engine (resets the playhead). Used by the Load dialog.
    fn load_project(&mut self, path: std::path::PathBuf) {
        match daw_core::project::format::load(&path) {
            Ok(project) => {
                // Undoable like JAM reload: Ctrl+Z returns to the prior
                // in-memory project after opening a file.
                self.undo.record("load project", &self.st.project);
                let _ = self.host.cmd_tx.try_send(SequencerCommand::LoadProject(project.clone()));
                self.playing = false;
                self.st = HeadlessState::from_project(project, Some(path.clone()));
                self.st.set_status(format!("loaded {}", path.display()));
            }
            Err(e) => self.st.set_status(format!("load failed: {e}")),
        }
    }

    /// Per-frame side effects: drain MIDI input and poll JAM hot-reload.
    /// Per-tick side-effect polling (MIDI, JAM, dialog results, status from
    /// background threads). Runs off the repaint timer, NOT from `render`, so it
    /// stays responsive without forcing a redraw. Returns `true` when something
    /// changed that the view should be repainted to reflect.
    fn pump(&mut self) -> bool {
        let mut changed = false;

        // 0. A file picked by the Load dialog → open it.
        let picked = self.load_result.lock().ok().and_then(|mut s| s.take());
        if let Some(path) = picked {
            self.load_project(path);
            changed = true;
        }

        // 0b. Save As: set the path then save.
        if let Some(path) = self.save_as_result.lock().ok().and_then(|mut s| s.take()) {
            self.st.project_path = Some(path);
            self.save_project();
            changed = true;
        }

        // 0c. A WAV picked by +WAV or Import Sample → add audio clip, creating
        //     an Audio track first if the selected track is not one.
        let wav = self.audio_clip_result.lock().ok().and_then(|mut s| s.take());
        if let Some(path) = wav {
            changed = true;
            let path_str = path.to_string_lossy().to_string();
            let is_audio = self.st.selected_track.as_ref()
                .and_then(|tid| self.st.project.tracks.iter().find(|t| &t.id == tid))
                .map(|t| t.track_type == daw_core::project::TrackType::Audio)
                .unwrap_or(false);
            self.undo.record("import sample", &self.st.project);
            let tid = if is_audio {
                self.st.selected_track.clone().unwrap()
            } else {
                let id = self.st.project.add_track(daw_core::project::TrackType::Audio);
                self.st.selected_track = Some(id.clone());
                id
            };
            match daw_core::audio::wav_duration_sec(&path) {
                Ok(dur) => {
                    let start = self.st.playhead_seconds();
                    self.st.project.add_audio_clip(&tid, &path_str, start, dur);
                    self.push_live();
                    self.st.set_status(format!("imported sample: {path_str}"));
                }
                Err(e) => self.st.set_status(format!("clip load failed: {e}")),
            }
        }

        // 0d. A WAV picked for a Sampler pad → set/replace that pad and reload.
        let pad = self.pad_load_result.lock().ok().and_then(|mut s| s.take());
        if let Some((tid, pitch, path)) = pad {
            changed = true;
            let path_str = path.to_string_lossy().to_string();
            self.undo.record("load pad", &self.st.project);
            if self.st.project.set_sampler_pad(&tid, pitch, path_str.clone()) {
                self.push_live();
                let name = std::path::Path::new(&path_str).file_name()
                    .map(|s| s.to_string_lossy().to_string()).unwrap_or(path_str);
                self.st.set_status(format!("pad p{pitch} ← {name}"));
            } else {
                self.st.set_status("select a Sampler track to load a pad".to_string());
            }
        }

        // 0c. Ensure waveform peaks are cached for every audio clip (once per path).
        let paths: Vec<String> = self.st.project.audio_clips.iter().map(|c| c.path.clone()).collect();
        for p in paths {
            if !self.st.peaks_cache.contains_key(&p) {
                if let Ok(pk) = daw_core::audio::wav_peaks(std::path::Path::new(&p), 512) {
                    self.st.peaks_cache.insert(p.clone(), std::sync::Arc::new(pk));
                    changed = true; // waveform can now be drawn
                }
                if let Ok(d) = daw_core::audio::wav_duration_sec(std::path::Path::new(&p)) {
                    self.st.peaks_dur_cache.insert(p.clone(), d);
                }
            }
        }

        // 0d'. Background ⚖ auto-balance finished: apply through the undo funnel.
        if let Some(plan) = self.balance_result.lock().ok().and_then(|mut s| s.take()) {
            self.undo.record("balance", &self.st.project);
            let mut max_db = 0f32;
            let n = plan.len();
            for r in &plan {
                self.st.project.set_track_volume(&r.track_id, r.new_volume);
                if r.old_volume > 0.0 && r.new_volume > 0.0 {
                    max_db = max_db.max((20.0 * (r.new_volume / r.old_volume).log10()).abs());
                }
            }
            self.push_live();
            self.st.set_status(format!("balanced {n} tracks (max change {max_db:.1} dB)"));
            changed = true;
        }

        // 0d. CLAP discovery results (once) + per-instance param metadata.
        if let Some(found) = self.clap_scan.lock().ok().and_then(|mut s| s.take()) {
            // Only announce CLAP discovery when something turned up — most
            // users have no CLAP plugins, and "0 plugins found" reads like an error.
            if !found.is_empty() {
                self.st.set_status(format!("CLAP: {} plugins found", found.len()));
            }
            self.st.clap_plugins = found;
            changed = true;
        }
        while let Ok((uid, name, params)) = self.host.clap_meta_rx.try_recv() {
            self.st.clap_meta.insert(uid, (name, params));
            changed = true;
        }

        // 1. MIDI input → preview the selected track / route CC.
        while let Ok(ev) = self.host.midi_ev_rx.try_recv() {
            changed = true;
            match ev {
                daw_io::midi::MidiEvent::NoteOn { pitch, velocity } => {
                    if let Some(tid) = self.st.selected_track.clone() {
                        let _ = self.host.cmd_tx.try_send(SequencerCommand::PreviewNote {
                            track_id: tid, pitch, velocity,
                        });
                    }
                    if self.midi_recording {
                        if let Some((_, len)) = self.open_pr_pattern() {
                            let ph = self.st.playhead();
                            self.recorder.note_on(pitch, velocity, ph, len);
                        }
                    }
                }
                daw_io::midi::MidiEvent::NoteOff { pitch } => {
                    if let Some(tid) = self.st.selected_track.clone() {
                        let _ = self.host.cmd_tx.try_send(SequencerCommand::PreviewNoteOff {
                            track_id: tid, pitch,
                        });
                    }
                    if self.midi_recording {
                        if let Some((idx, len)) = self.open_pr_pattern() {
                            let ph = self.st.playhead();
                            if let Some(note) = self.recorder.note_off(pitch, ph, len) {
                                self.insert_recorded_note(idx, note);
                            }
                        }
                    }
                }
                daw_io::midi::MidiEvent::ControlChange { cc, value } => {
                    let ch = self.host.midi_channel.load(std::sync::atomic::Ordering::Relaxed);
                    let _ = self.host.cmd_tx.try_send(SequencerCommand::RawCC { channel: ch, cc, value });
                }
            }
        }

        // 2. JAM hot-reload: reload the project file when its mtime changes.
        if self.st.jam_mode {
            if let Some(path) = self.st.project_path.clone() {
                let mtime = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok());
                if mtime != self.st.project_mtime {
                    let first_observation = self.st.project_mtime.is_none();
                    self.st.project_mtime = mtime;
                    if !first_observation {
                        if let Ok(p) = daw_core::project::format::load(&path) {
                            // External edits (LLM/scripts) are undoable in-app.
                            self.undo.record("JAM reload", &self.st.project);
                            self.st.project = p;
                            self.push_live();
                            self.st.set_status(format!("JAM reloaded {}", path.display()));
                            changed = true;
                        }
                    }
                }
            }
        }

        // 3. Status messages posted by background threads (e.g. WAV export).
        if let Ok(mut slot) = self.export_result.lock() {
            if let Some(msg) = slot.take() {
                self.st.set_status(msg);
                changed = true;
            }
        }

        // 4. Audio recording: roll-tick latch, finished takes, live meter.
        changed |= self.poll_recording();

        changed
    }

    /// A slim toolbar under the transport: tonality (transpose), swing, humanize,
    /// metronome, panic.
    fn song_toolbar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let accent = self.accent();
        let key = format!("{} {}", self.st.project.project.key, self.st.project.project.scale);
        let swing = self.st.project.project.swing;
        let humanize = self.st.project.project.humanize;
        let metro = self.st.project.project.metronome;
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .h(px(30.))
            .flex_shrink_0()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .text_color(rgb(TEXT_DIM))
            // Tonality / transpose
            .child(cell_btn("key-down", "−", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.transpose(-1); cx.notify(); })))
            .child(div().min_w(px(72.)).text_color(rgb(TEXT)).child(key))
            .child(cell_btn("key-up", "+", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.transpose(1); cx.notify(); })))
            // Swing
            .child(div().child("Swing"))
            .child(cell_btn("sw-down", "−", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.adjust_swing(-0.05); cx.notify(); })))
            .child(div().min_w(px(40.)).text_color(rgb(TEXT)).child(format!("{:.0}%", swing * 100.0)))
            .child(cell_btn("sw-up", "+", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.adjust_swing(0.05); cx.notify(); })))
            // Humanize (click cycles)
            .child(
                cell_btn("humanize", format!("Humanize {:.0}%", humanize * 100.0), humanize > 0.0, accent)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.cycle_humanize(); cx.notify(); }))
            )
            // Metronome toggle
            .child(
                cell_btn("metro", "Metro", metro, accent)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.toggle_metronome(); cx.notify(); }))
            )
            // Loop region + punch toggles (one region, three switches).
            .child(
                cell_btn("loop", "⟳ Loop", self.st.project.loop_region.enabled, accent)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.toggle_loop(); cx.notify(); }))
            )
            .child(
                cell_btn("punch-in", "⇥In", self.st.project.loop_region.punch_in, RECORD)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.toggle_punch_in(); cx.notify(); }))
            )
            .child(
                cell_btn("punch-out", "Out⇤", self.st.project.loop_region.punch_out, RECORD)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.toggle_punch_out(); cx.notify(); }))
            )
            // Panic
            .child(
                cell_btn("panic", "Panic", false, RECORD)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.panic_engine(); cx.notify(); }))
            )
            // Arrange scroll controls (work regardless of mouse-wheel behaviour).
            .child(div().flex_1())
            .child(div().text_color(rgb(TEXT_DIM)).child("Arr:"))
            .child(cell_btn("arr-left", "◀", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| {
                let step = 4.0 * this.bar_w();
                this.st.arrange_scroll_x = (this.st.arrange_scroll_x - step).max(0.0);
                cx.notify();
            })))
            .child(cell_btn("arr-right", "▶", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| {
                let step = 4.0 * this.bar_w();
                let content_w = this.st.project.song_bars() as f32 * this.bar_w();
                let max_off = (content_w - 100.0).max(0.0);
                this.st.arrange_scroll_x = (this.st.arrange_scroll_x + step).clamp(0.0, max_off);
                cx.notify();
            })))
            .child(cell_btn("arr-fit", "⤢", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| {
                this.st.arrange_zoom = 1.0;
                this.st.arrange_scroll_x = 0.0;
                cx.notify();
            })))
            // Track add buttons
            .child(div().text_color(rgb(TEXT_DIM)).child("Track:"))
            .child(cell_btn("add-mel", "+Mel", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.add_track(daw_core::project::TrackType::Melodic); cx.notify(); })))
            .child(cell_btn("add-drum", "+Drum", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.add_track(daw_core::project::TrackType::Drum); cx.notify(); })))
            .child(cell_btn("add-aud", "+Aud", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.add_track(daw_core::project::TrackType::Audio); cx.notify(); })))
            // Always available: the pump auto-creates an Audio track when the
            // selection isn't one, so +WAV works from anywhere.
            .child(
                cell_btn("add-wav", "+WAV", false, PANEL_ALT)
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        let slot = this.audio_clip_result.clone();
                        std::thread::spawn(move || {
                            if let Some(f) = rfd::FileDialog::new()
                                .add_filter("WAV", &["wav"])
                                .pick_file()
                            {
                                *slot.lock().unwrap() = Some(f);
                            }
                        });
                        cx.notify();
                    }))
            )
    }

    /// Slim top row with File / Settings buttons.
    fn menu_bar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let file_open = self.st.open_menu == Some(TopMenu::File);
        let settings_open = self.st.open_menu == Some(TopMenu::Settings);
        let fx_open = self.st.fx_window_open;
        let accent = self.accent();
        div()
            .flex()
            .items_center()
            .h(px(24.))
            .flex_shrink_0()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .text_color(rgb(TEXT))
            // App wordmark (left of the menus).
            .child(
                div()
                    .px_3()
                    .h_full()
                    .flex()
                    .items_center()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(self.accent()))
                    .child("daw-ai"),
            )
            .child(
                div()
                    .id("menu-file")
                    .px_3()
                    .h_full()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .bg(rgb(if file_open { PANEL_ALT } else { PANEL }))
                    .text_color(rgb(TEXT))
                    .child("File ▾")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        let m = TopMenu::File;
                        this.st.open_menu = if this.st.open_menu == Some(m) { None } else { Some(m) };
                        cx.notify();
                    }))
            )
            .child(
                div()
                    .id("menu-settings")
                    .px_3()
                    .h_full()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .bg(rgb(if settings_open { PANEL_ALT } else { PANEL }))
                    .text_color(rgb(TEXT))
                    .child("Settings ▾")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        let m = TopMenu::Settings;
                        let opening = this.st.open_menu != Some(m);
                        if opening {
                            // Device enumeration is too slow for render(); do it once on open.
                            this.input_devices = daw_core::audio::input::list_input_devices();
                        }
                        this.st.open_menu = if opening { Some(m) } else { None };
                        cx.notify();
                    }))
            )
            .child(
                div()
                    .id("menu-audio-effects")
                    .px_3()
                    .h_full()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .bg(rgb(if fx_open { PANEL_ALT } else { PANEL }))
                    .text_color(rgb(if fx_open { accent } else { TEXT }))
                    .child("Audio Effects")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.st.fx_window_open = !this.st.fx_window_open;
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
    }

    /// Overlay: full-window scrim (closes on outside-click) + the open dropdown panel.
    /// Returns `None` when no menu is open.
    fn menu_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let open = self.st.open_menu?;
        let panel = match open {
            TopMenu::File => self.file_menu(cx).into_any_element(),
            TopMenu::Settings => self.settings_panel(cx).into_any_element(),
        };
        let overlay = div()
            .absolute()
            .inset_0()
            .child(
                // scrim: covers the window *below* the 24px menu bar so an
                // outside click closes the dropdown. It must NOT overlap the menu
                // buttons, or a second click on File would hit the scrim (close)
                // AND the button beneath (re-open), leaving the menu stuck open.
                div()
                    .id("menu-scrim")
                    .absolute()
                    .top(px(24.))
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
            .child(panel);
        Some(overlay.into_any_element())
    }

    /// Overlay panel for Audio Effects workspace.
    /// Returns `None` when `fx_window_open` is false.
    /// Covers the body area below menu (24) + transport (48) + toolbar (30) + status (18) = 120px.
    fn audio_effects_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.st.fx_window_open { return None; }

        let accent = self.accent();

        let track_name = self.st.selected_track.as_ref()
            .and_then(|tid| self.st.project.tracks.iter().find(|t| &t.id == tid))
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "no track".to_string());

        let title = format!("Audio Effects \u{2014} {}", track_name);

        let header = div()
            .flex()
            .items_center()
            .h(px(32.))
            .px_3()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(TEXT))
                    .child(title)
            )
            .child(
                div()
                    .id("fx-close")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(PANEL_ALT))
                    .text_color(rgb(TEXT_DIM))
                    .child("\u{2715}")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.st.fx_window_open = false;
                        cx.notify();
                    }))
            );

        // ── Left column: Effect Chain editor ────────────────────────────────
        let left_col = div()
            .w(px(360.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .overflow_hidden()
            .child(section_title("Effect Chain"))
            .child(
                div()
                    .id("fx-chain-scroll")
                    .flex_1()
                    .overflow_scroll()
                    .child(self.fx_editor(cx))
            );

        // ── Right column: Visualizations ────────────────────────────────────
        // Snapshot vis buffer once and share across all scopes.
        let samples: Vec<f32> = self.host.vis_buf.lock().ok()
            .map(|b| b.clone())
            .unwrap_or_default();
        let sr = self.host.sample_rate.load(std::sync::atomic::Ordering::Relaxed).max(44100);

        // Box height (px) for each scope.
        const H: f32 = 90.0;

        // ── Oscilloscope ─────────────────────────────────────────────────────
        let cols = 160usize;
        let osc_bars = if samples.is_empty() {
            vec![0.0f32; cols]
        } else {
            let n = samples.len();
            (0..cols).map(|c| {
                let a = c * n / cols;
                let b = ((c + 1) * n / cols).max(a + 1).min(n);
                samples[a..b].iter().map(|s| s.abs()).fold(0.0f32, f32::max)
            }).collect()
        };
        let osc_children: Vec<_> = osc_bars.iter().map(|&mag| {
            let h = (mag * H).clamp(1.0, H);
            div()
                .w(px(2.))
                .h(px(h))
                .bg(rgb(accent))
        }).collect();
        let oscilloscope_box = div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(section_title("Oscilloscope"))
            .child(
                div()
                    .h(px(H))
                    .flex()
                    .items_center()
                    .px_2()
                    .gap_px()
                    .overflow_hidden()
                    .children(osc_children)
            );

        // ── Spectrum ─────────────────────────────────────────────────────────
        let spec_bars = daw_core::dsp::spectrum(&samples, sr, 64, 20.0, 20000.0, -60.0);
        let spec_children: Vec<_> = spec_bars.iter().map(|&level| {
            let h = (level * H).clamp(1.0, H);
            let color = if level >= 0.85 { RECORD } else if level >= 0.6 { METER_WARN } else { accent };
            div()
                .flex_1()
                .h(px(h))
                .bg(rgb(color))
        }).collect();
        let spectrum_box = div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(section_title("Spectrum"))
            .child(
                div()
                    .h(px(H))
                    .flex()
                    .items_end()
                    .px_2()
                    .gap_px()
                    .overflow_hidden()
                    .children(spec_children)
            );

        // ── Filter / LFO ─────────────────────────────────────────────────────
        // Read selected track data for filter cutoff and modulators.
        let (cutoff01, modulators) = self.st.selected_track.as_ref()
            .and_then(|tid| self.st.project.tracks.iter().find(|t| &t.id == tid))
            .map(|t| (t.effects.cutoff, t.modulators.clone()))
            .unwrap_or((1.0, vec![]));

        // LP response curve: 64 log-spaced frequency columns.
        // Dim the accent by averaging its R/G/B channels with BG.
        let dim_accent = {
            let ar = ((accent >> 16) & 0xff) / 2 + ((BG >> 16) & 0xff) / 2;
            let ag = ((accent >> 8) & 0xff) / 2 + ((BG >> 8) & 0xff) / 2;
            let ab = (accent & 0xff) / 2 + (BG & 0xff) / 2;
            (ar << 16) | (ag << 8) | ab
        };
        let f_min = 20.0f32;
        let f_max = (sr as f32 / 2.0).min(20000.0);
        let lp_children: Vec<_> = (0..64usize).map(|i| {
            let f = f_min * (f_max / f_min).powf(i as f32 / 64.0);
            let mag = lp_magnitude(cutoff01, f, sr);
            let h = (mag * H).clamp(1.0, H);
            div()
                .flex_1()
                .h(px(h))
                .bg(rgb(dim_accent))
        }).collect();

        // LFO modulator bars.
        let t_secs = self.st.playhead_seconds() as f32;
        let lfo_section: gpui::Div = if modulators.is_empty() {
            div()
                .px_2()
                .py_1()
                .text_color(rgb(TEXT_DIM))
                .child("no LFOs")
        } else {
            let lfo_bars: Vec<_> = modulators.iter().map(|m| {
                let base = daw_core::sequencer::engine::mod_base01(m.dest, {
                    // Re-borrow the track for mod_base01
                    self.st.project.tracks.iter()
                        .find(|t| self.st.selected_track.as_deref() == Some(&t.id))
                        .unwrap()
                });
                let val = daw_core::sequencer::engine::modulated_value(m, base, t_secs);
                let bar_h = (val * (H - 4.0)).clamp(2.0, H - 4.0);
                let label = m.dest.label();
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .child(
                        div()
                            .w(px(14.))
                            .h(px(bar_h))
                            .bg(rgb(accent))
                    )
                    .child(
                        div()
                            .text_color(rgb(TEXT_DIM))
                            .child(label)
                    )
            }).collect();
            div()
                .flex()
                .items_end()
                .gap_2()
                .px_2()
                .h(px(H))
                .children(lfo_bars)
        };

        let filter_lfo_box = div()
            .flex()
            .flex_col()
            .child(section_title("Filter / LFO"))
            .child(
                div()
                    .h(px(H))
                    .flex()
                    .items_end()
                    .px_2()
                    .gap_px()
                    .overflow_hidden()
                    .children(lp_children)
            )
            .child(lfo_section);

        let right_col = div()
            .id("fx-right-scroll")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_scroll()
            .child(oscilloscope_box)
            .child(spectrum_box)
            .child(filter_lfo_box);

        let body = div()
            .flex()
            .flex_1()
            .min_h(px(0.))
            .child(left_col)
            .child(right_col);

        let panel = div()
            .absolute()
            .left(px(0.))
            .top(px(120.))
            .right(px(0.))
            .bottom(px(0.))
            .bg(rgb(BG))
            .flex()
            .flex_col()
            .child(header)
            .child(body);

        Some(panel.into_any_element())
    }

    /// File dropdown — five action rows.
    fn file_menu(&self, cx: &mut Context<Self>) -> gpui::Div {
        // Each row is built individually to attach distinct on_click closures.
        div()
            .absolute()
            .left(px(0.))
            .top(px(24.))
            .min_w(px(160.))
            .flex()
            .flex_col()
            .bg(rgb(PANEL_ALT))
            .border_1()
            .border_color(rgb(BORDER))
            .rounded_md()
            .shadow_md()
            .overflow_hidden()
            .child(
                div().id("fm-new").px_3().py_1().cursor_pointer().text_color(rgb(TEXT))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child("New")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.undo.record("new project", &this.st.project);
                        this.st.project = daw_core::project::Project::default_project();
                        this.st.project_path = None;
                        if let Some(id) = this.st.project.tracks.first().map(|t| t.id.clone()) {
                            this.st.select_track(&id);
                        }
                        this.sync_midi_strip();
                        this.push_live();
                        this.st.open_menu = None;
                        this.st.set_status("new project (unsaved — Save As to name it)");
                        cx.notify();
                    }))
            )
            .child(
                div().id("fm-load").px_3().py_1().cursor_pointer().text_color(rgb(TEXT))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child("Load")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.open_dialog();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
            .child({
                let label = self.undo.undo_label().map(|l| format!("Undo {l}")).unwrap_or_else(|| "Undo".into());
                let fg = if self.undo.can_undo() { TEXT } else { TEXT_DIM };
                div().id("fm-undo").px_3().py_1().cursor_pointer().text_color(rgb(fg))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child(label)
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.undo();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            })
            .child({
                let label = self.undo.redo_label().map(|l| format!("Redo {l}")).unwrap_or_else(|| "Redo".into());
                let fg = if self.undo.can_redo() { TEXT } else { TEXT_DIM };
                div().id("fm-redo").px_3().py_1().cursor_pointer().text_color(rgb(fg))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child(label)
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.redo();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            })
            .child(
                div().id("fm-save").px_3().py_1().cursor_pointer().text_color(rgb(TEXT))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child("Save")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.save_project();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
            .child(
                div().id("fm-saveas").px_3().py_1().cursor_pointer().text_color(rgb(TEXT))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child("Save As…")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.save_as_dialog();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
            .child(
                div().id("fm-export").px_3().py_1().cursor_pointer().text_color(rgb(TEXT))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child("Export WAV")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.export_wav();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
            .child(
                div().id("fm-import").px_3().py_1().cursor_pointer().text_color(rgb(TEXT))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child("Import Sample")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.import_sample_dialog();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
    }

    /// Settings dropdown panel — MIDI, Audio (read-only), Appearance.
    fn settings_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let midi_ports = daw_io::list_midi_ports();
        let cur_device = self.host.midi_device.clone();
        let cur_device_display = cur_device.clone().unwrap_or_else(|| "none".into());
        let midi_ch = self.host.midi_channel.load(std::sync::atomic::Ordering::Relaxed);
        let sr = self.host.sample_rate.load(std::sync::atomic::Ordering::Relaxed);
        let accent = self.accent();

        let port_rows = midi_ports.into_iter().enumerate().map(|(i, name)| {
            let cur = Some(&name) == cur_device.as_ref();
            let fg = if cur { accent } else { TEXT };
            div()
                .id(SharedString::from(format!("midip-{i}")))
                .px_2()
                .py_1()
                .cursor_pointer()
                .bg(rgb(PANEL))
                .text_color(rgb(fg))
                .whitespace_nowrap()
                .child(name.clone())
                .on_click(cx.listener(move |this, _e, _w, cx| {
                    this.host.set_midi_port(i);
                    cx.notify();
                }))
        });

        let swatch_row = div()
            .flex()
            .gap_2()
            .px_3()
            .py_2()
            .children(ACCENTS.iter().enumerate().map(|(i, &c)| {
                let selected = self.st.accent_idx == i;
                div()
                    .id(SharedString::from(format!("acc-{i}")))
                    .w(px(20.))
                    .h(px(20.))
                    .rounded_md()
                    .bg(rgb(c))
                    .cursor_pointer()
                    .border_2()
                    .border_color(rgb(if selected { TEXT } else { PANEL }))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.st.accent_idx = i;
                        cx.notify();
                    }))
            }));

        div()
            .absolute()
            .left(px(80.))
            .top(px(24.))
            .w(px(320.))
            .flex()
            .flex_col()
            .bg(rgb(PANEL_ALT))
            .border_1()
            .border_color(rgb(BORDER))
            .rounded_md()
            .shadow_md()
            .overflow_hidden()
            // MIDI section
            .child(section_title("MIDI"))
            .child(
                div().px_3().py_1().text_color(rgb(TEXT_DIM))
                    .child(format!("Device: {}", cur_device_display))
            )
            .children(port_rows)
            // Live MIDI routes to the selected track's strip automatically
            // (no manual channel pick since the per-track mixer landed).
            .child(
                div().px_3().py_1().text_color(rgb(TEXT_DIM))
                    .child(format!("Plays strip: {} (selected track)", midi_ch))
            )
            // Audio section
            .child(section_title("Audio"))
            .child(
                div().px_3().py_1().text_color(rgb(TEXT_DIM))
                    .child(if sr > 0 { format!("Sample rate: {sr} Hz") } else { "Sample rate: —".into() })
            )
            .child(
                div().px_3().py_1().text_color(rgb(TEXT_DIM))
                    .child(format!("Soundfont: {}", self.sf2_path))
            )
            // Input (recording) section
            .child(section_title("Input"))
            .children({
                let rows: Vec<_> = self.input_devices.iter().enumerate().map(|(i, d)| {
                    let selected = self.config.input_device.as_deref() == Some(d.name.as_str());
                    let name = d.name.clone();
                    div()
                        .id(SharedString::from(format!("in-dev-{i}")))
                        .px_3().py_1().cursor_pointer()
                        .text_color(rgb(if selected { accent } else { TEXT_DIM }))
                        .whitespace_nowrap()
                        .child(format!("{} {} ({} ch)", if selected { "●" } else { "○" }, trunc(&name, 26), d.channels))
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            this.config.input_device = Some(name.clone());
                            let _ = this.config.save();
                            this.reopen_capture_if_armed();
                            cx.notify();
                        }))
                }).collect();
                rows
            })
            .child({
                use daw_core::audio::input::InputChannels;
                let label = match self.config.input_channels {
                    InputChannels::Mono(c) => format!("Mono — input {}", c + 1),
                    InputChannels::Stereo(l, r) => format!("Stereo — inputs {}+{}", l + 1, r + 1),
                };
                let max_ch = self.input_devices.iter()
                    .find(|d| self.config.input_device.as_deref() == Some(d.name.as_str()))
                    .map(|d| d.channels).unwrap_or(2);
                div().flex().items_center().gap_2().px_3().py_1()
                    .child(div().text_color(rgb(TEXT_DIM)).child("Channel:"))
                    .child(div().id("in-ch-down").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(PANEL)).text_color(rgb(TEXT)).child("−")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            use daw_core::audio::input::InputChannels;
                            this.config.input_channels = match this.config.input_channels {
                                InputChannels::Mono(c) => InputChannels::Mono(c.saturating_sub(1)),
                                InputChannels::Stereo(l, _) => {
                                    let l = l.saturating_sub(1);
                                    InputChannels::Stereo(l, l + 1)
                                }
                            };
                            let _ = this.config.save();
                            this.reopen_capture_if_armed();
                            cx.notify();
                        })))
                    .child(div().min_w(px(140.)).text_color(rgb(TEXT)).child(label))
                    .child(div().id("in-ch-up").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(PANEL)).text_color(rgb(TEXT)).child("+")
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            use daw_core::audio::input::InputChannels;
                            let max = max_ch.max(1) - 1;
                            this.config.input_channels = match this.config.input_channels {
                                InputChannels::Mono(c) => InputChannels::Mono((c + 1).min(max)),
                                InputChannels::Stereo(l, _) => {
                                    let l = (l + 1).min(max.saturating_sub(1));
                                    InputChannels::Stereo(l, l + 1)
                                }
                            };
                            let _ = this.config.save();
                            this.reopen_capture_if_armed();
                            cx.notify();
                        })))
                    .child(div().id("in-ch-mode").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(PANEL)).text_color(rgb(TEXT)).child("mono/stereo")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            use daw_core::audio::input::InputChannels;
                            this.config.input_channels = match this.config.input_channels {
                                InputChannels::Mono(c) => InputChannels::Stereo(c, c + 1),
                                InputChannels::Stereo(l, _) => InputChannels::Mono(l),
                            };
                            let _ = this.config.save();
                            this.reopen_capture_if_armed();
                            cx.notify();
                        })))
            })
            .child(
                div().flex().items_center().gap_2().px_3().py_1()
                    .child(div().text_color(rgb(TEXT_DIM)).child("Count-in:"))
                    .child(div().id("ci-down").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(PANEL)).text_color(rgb(TEXT)).child("−")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            this.config.count_in_bars = this.config.count_in_bars.saturating_sub(1);
                            let _ = this.config.save();
                            cx.notify();
                        })))
                    .child(div().min_w(px(56.)).text_color(rgb(TEXT)).child(format!("{} bars", self.config.count_in_bars)))
                    .child(div().id("ci-up").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(PANEL)).text_color(rgb(TEXT)).child("+")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            this.config.count_in_bars = (this.config.count_in_bars + 1).min(4);
                            let _ = this.config.save();
                            cx.notify();
                        })))
            )
            .child(
                div().flex().items_center().gap_2().px_3().py_1()
                    .child(div().text_color(rgb(TEXT_DIM)).child("Latency trim:"))
                    .child(div().id("lt-down").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(PANEL)).text_color(rgb(TEXT)).child("−")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            this.config.latency_trim_ms = (this.config.latency_trim_ms - 2.0).max(0.0);
                            let _ = this.config.save();
                            cx.notify();
                        })))
                    .child(div().min_w(px(64.)).text_color(rgb(TEXT)).child(format!("{:.0} ms", self.config.latency_trim_ms)))
                    .child(div().id("lt-up").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(PANEL)).text_color(rgb(TEXT)).child("+")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            this.config.latency_trim_ms = (this.config.latency_trim_ms + 2.0).min(200.0);
                            let _ = this.config.save();
                            cx.notify();
                        })))
            )
            // Appearance section
            .child(section_title("Appearance"))
            .child(swatch_row)
    }

    /// Transport master-bus meter (post-limiter, from the engine mixer).
    fn viz_meter(&self) -> gpui::Div {
        let accent = self.accent();
        let peak = self.host.master_meter.read().0.min(1.0);
        let bars = 12usize;
        let lit = (peak * bars as f32).round() as usize;
        let cells = (0..bars).map(move |i| {
            let on = i < lit;
            let color = if i >= bars - 2 { RECORD } else if i >= bars - 5 { METER_WARN } else { accent };
            div().w(px(4.)).h(px(12.)).rounded_sm()
                .bg(rgb(if on { color } else { PANEL_ALT }))
        });
        div().flex().items_center().gap_1().children(cells)
    }
}

/// Clip blocks take their track's color so a lane reads as one unit.
fn clip_color(track_idx: usize) -> u32 {
    track_color(track_idx)
}

/// Pick `cols` evenly-spaced peak magnitudes from `peaks` over the
/// [start_frac, end_frac] sub-range (0..=1 of the source).
fn waveform_columns(peaks: &[f32], start_frac: f64, end_frac: f64, cols: usize) -> Vec<f32> {
    if peaks.is_empty() || cols == 0 { return vec![0.0; cols]; }
    let n = peaks.len();
    let lo = (start_frac.clamp(0.0, 1.0) * n as f64) as usize;
    let hi = ((end_frac.clamp(0.0, 1.0) * n as f64).ceil() as usize).max(lo + 1).min(n);
    (0..cols).map(|c| {
        let a = lo + c * (hi - lo) / cols;
        let b = (lo + (c + 1) * (hi - lo) / cols).max(a + 1).min(n);
        peaks[a..b].iter().cloned().fold(0.0f32, f32::max)
    }).collect()
}

/// Top menu-bar dropdown identity.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TopMenu { File, Settings }

/// Marker payload for a panel-splitter drag (resizing, not drop). Shared by all
/// Which panel splitter a resize drag belongs to. `on_drag_move` fires for every
/// `ResizeDrag` handler regardless of which splitter was grabbed, so the payload
/// must carry identity and each handler gates on it — otherwise dragging one
/// splitter would also resize the other panels.
#[derive(Clone, Copy, PartialEq)]
enum Splitter { Dock, Browser, Mixer }

#[derive(Clone)]
struct ResizeDrag(Splitter);

/// Invisible drag preview for splitter drags (we only use the move stream).
/// Mark rects (x, y, w, h in px) previewing `pattern` inside a clip that is
/// `clip_w`×`clip_h` px and covers `span_ticks` of timeline; the pattern
/// loops at its natural period. Capped at 256 marks.
fn clip_preview_marks(
    pattern: &daw_core::project::Pattern,
    clip_w: f32,
    clip_h: f32,
    span_ticks: u32,
) -> Vec<(f32, f32, f32, f32)> {
    use daw_core::project::Pattern;
    const CAP: usize = 256;
    let span_ticks = span_ticks.max(1) as f32;
    let ppt = clip_w / span_ticks; // px per tick
    let nat = daw_core::sequencer::engine::pattern_length_ticks(pattern).max(1);
    let mut out: Vec<(f32, f32, f32, f32)> = Vec::new();
    let mut rep_start: u32 = 0;
    'reps: while (rep_start as f32) < span_ticks {
        match pattern {
            Pattern::PianoRoll(pr) => {
                if pr.notes.is_empty() {
                    break;
                }
                let lo = pr.notes.iter().map(|n| n.pitch).min().unwrap() as f32;
                let hi = pr.notes.iter().map(|n| n.pitch).max().unwrap() as f32;
                let range = (hi - lo).max(1.0);
                let mark_h = 2.0f32;
                let usable = (clip_h - mark_h - 2.0).max(1.0);
                for n in &pr.notes {
                    if out.len() >= CAP {
                        break 'reps;
                    }
                    let x = (rep_start + n.tick) as f32 * ppt;
                    if x >= clip_w {
                        continue;
                    }
                    let w = (n.duration as f32 * ppt).max(1.5).min(clip_w - x);
                    let y = 1.0 + usable * (1.0 - (n.pitch as f32 - lo) / range);
                    out.push((x, y, w, mark_h));
                }
            }
            Pattern::Step(sp) => {
                if sp.rows.is_empty() {
                    break;
                }
                let rows = sp.rows.len() as f32;
                let row_h = (clip_h - 2.0) / rows;
                let mark_h = (row_h - 1.0).clamp(1.5, 3.0);
                for (ri, row) in sp.rows.iter().enumerate() {
                    for si in 0..sp.steps as usize {
                        if row.step_velocity(si).is_none() {
                            continue;
                        }
                        if out.len() >= CAP {
                            break 'reps;
                        }
                        let x = (rep_start + si as u32 * 30) as f32 * ppt;
                        if x >= clip_w {
                            continue;
                        }
                        let w = (15.0 * ppt).clamp(2.0, 6.0).min(clip_w - x);
                        let y = 1.0 + ri as f32 * row_h;
                        out.push((x, y, w, mark_h));
                    }
                }
            }
        }
        rep_start += nat;
    }
    out
}

/// One-line hover tooltip (clip names etc.).
struct TextTooltip {
    text: SharedString,
}
impl Render for TextTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(BORDER))
            .text_color(rgb(TEXT))
            .whitespace_nowrap()
            .child(self.text.clone())
    }
}

struct EmptyDrag;
impl Render for EmptyDrag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(0.)).h(px(0.))
    }
}

/// Dragging a note's start cell onto a later cell in the same pitch row
/// stretches the note to end there (legato across bars).
#[derive(Clone)]
struct NoteStretch {
    pid: String,
    pitch: u8,
    tick: u32,
}

/// Payload carried while dragging an arrangement clip to a new bar/lane.
#[derive(Clone)]
struct ClipDrag {
    track: String,
    bar: u32,
    name: String,
    color: u32,
}

// `on_drag_move` fires for EVERY element whose handler matches the drag payload
// TYPE, so each of these payloads carries the dragged item's identity; each
// handler reads it via `e.drag(cx)` and ignores moves for other items. Without
// this, dragging one clip's handle would resize/move every track's clip at once.

/// The pattern clip (track + start bar) whose right-edge resize handle is dragged.
#[derive(Clone)]
struct ResizeClip { track: String, bar: u32 }

/// The track whose header bottom-edge height handle is dragged.
#[derive(Clone)]
struct TrackResize { track: String }

/// The audio clip (by index) being dragged to move.
#[derive(Clone)]
struct AudioMove { idx: usize }

/// The audio clip (by index) whose left edge is dragged to crop its start.
#[derive(Clone)]
struct AudioCropL { idx: usize }

/// The audio clip (by index) whose right edge is dragged to crop its end.
#[derive(Clone)]
struct AudioCropR { idx: usize }

/// The little floating block rendered under the cursor during a clip drag.
struct ClipDragPreview {
    name: String,
    color: u32,
}

impl Render for ClipDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(self.color))
            .text_color(rgb(BG))
            .whitespace_nowrap()
            .child(self.name.clone())
    }
}

/// A small clickable cell button used in track headers (toggle or stepper).
/// `on` highlights it with `on_color`; the caller chains `.on_click(...)`.
fn cell_btn(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on: bool,
    on_color: u32,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .flex_shrink_0()
        .px_2()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .whitespace_nowrap()
        .bg(rgb(if on { on_color } else { PANEL_ALT }))
        .text_color(rgb(if on { BG } else { TEXT }))
        .child(label.into())
}

/// Centered placeholder child for dock tabs that aren't yet implemented.
fn dock_hint(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(TEXT_DIM))
        .child(text.into())
}

/// Truncate a string to at most `n` chars.
fn trunc(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect()
    }
}

/// Return the note name for a MIDI pitch (e.g. 60 → "C4", 36 → "C2").
fn note_name(pitch: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let octave = (pitch / 12) as i32 - 1;
    format!("{}{}", NAMES[(pitch % 12) as usize], octave)
}

/// Short instrument summary shown in a track header.
/// GM preset number → (preset name, category) from the browser catalog, if known.
fn gm_preset_name(preset: u8) -> Option<(&'static str, &'static str)> {
    for (cat, presets) in GM_CATEGORIES {
        for (name, prog) in *presets {
            if *prog == preset {
                return Some((name, cat));
            }
        }
    }
    None
}

fn instrument_label(t: &daw_core::project::Track) -> String {
    use daw_core::project::Instrument::*;
    match &t.instrument {
        // Show the readable instrument (e.g. "Acoustic Bass") so the plugged
        // preset is obvious. Only the melodic bank (0) maps to GM names; other
        // banks (e.g. 128 = drums) keep the raw bank:preset.
        Soundfont { preset, bank, .. } => match (*bank == 0).then(|| gm_preset_name(*preset)).flatten() {
            Some((name, cat)) => format!("{name} {cat}"),
            None => format!("SF {bank}:{preset}"),
        },
        AudioFile { .. } => "Audio".to_string(),
        Sampler { pads } => format!("Sampler ({} pads)", pads.len()),
        Clap(slot) => slot.id.clone(),
    }
}

/// Three-letter track-type badge.
fn track_type_badge(t: &daw_core::project::Track) -> &'static str {
    use daw_core::project::TrackType::*;
    match t.track_type {
        Drum => "DRM",
        Melodic => "MEL",
        Audio => "AUD",
    }
}

/// GM families → (name, program) presets. Compact subset of the 128 GM sounds.
const GM_CATEGORIES: &[(&str, &[(&str, u8)])] = &[
    ("Piano", &[("Grand", 0), ("Bright", 1), ("E.Piano", 4), ("Harpsi", 6)]),
    ("Organ", &[("Drawbar", 16), ("Perc", 17), ("Rock", 18), ("Church", 19)]),
    ("Guitar", &[("Nylon", 24), ("Steel", 25), ("Jazz", 26), ("Clean", 27)]),
    ("Bass", &[("Acoustic", 32), ("Finger", 33), ("Pick", 34), ("Synth", 38)]),
    ("Strings", &[("Violin", 40), ("Cello", 42), ("Ensemble", 48), ("Pad", 51)]),
    ("Brass", &[("Trumpet", 56), ("Trombone", 57), ("Sax", 65), ("Section", 61)]),
    ("Synth Lead", &[("Square", 80), ("Saw", 81), ("Calliope", 82), ("Charang", 84)]),
    ("Synth Pad", &[("New Age", 88), ("Warm", 89), ("Sweep", 95), ("Halo", 94)]),
];

/// A panel header used atop the browser and mixer docks.
fn section_title(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .px_3()
        .py_2()
        .text_color(rgb(TEXT_DIM))
        .border_b_1()
        .border_color(rgb(BORDER))
        .child(text.into())
}

/// A small rounded pill used across the transport bar.
fn pill(text: impl Into<SharedString>, bg: u32, fg: u32) -> gpui::Div {
    div()
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .child(text.into())
}

impl Render for DawView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `render` is side-effect free: engine polling + the playhead mirror run on
        // the repaint timer (see `run`), which only re-renders when playing or when
        // something changed — so an idle window doesn't rebuild the UI 60×/second.

        // Window width — a conservative bound for timeline virtualization.
        let vis_w = f32::from(window.viewport_size().width);
        let accent = self.accent();
        let playing = self.playing;
        let recording = self.midi_recording;
        let bpm = self.st.bpm();
        let tick = self.st.playhead();
        let bar = tick / daw_core::sequencer::TICKS_PER_BAR + 1;
        let beat = (tick % daw_core::sequencer::TICKS_PER_BAR) / daw_core::sequencer::TICKS_PER_BEAT + 1;
        let status_text = self.st.status.clone();

        let transport = div()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .h(px(48.))
            .flex_shrink_0()
            .bg(rgb(PANEL))
            .text_color(rgb(TEXT))
            .border_b_1()
            .border_color(rgb(BORDER))
            // Play / Pause toggle.
            .child(
                div()
                    .id("transport-play")
                    .px_4()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(accent))
                    .text_color(rgb(BG))
                    .active(|s| s.opacity(0.8))
                    .child(if playing { "⏸ Pause" } else { "▶ Play" })
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.toggle_play();
                        cx.notify();
                    })),
            )
            .child(pill("⏹ Stop", PANEL_ALT, TEXT).id("transport-stop").cursor_pointer().on_click(
                cx.listener(|this, _ev, _window, cx| {
                    this.stop();
                    cx.notify();
                }),
            ))
            // Record: audio takes on an audio track, MIDI record otherwise.
            .child({
                use daw_core::audio::input::RecordPhase;
                let audio_sel = self.selected_track_is_audio();
                let (label, bg, fg) = if audio_sel {
                    match self.audio_rec.phase {
                        RecordPhase::Idle => ("⏺ Rec", PANEL_ALT, TEXT),
                        RecordPhase::Armed => ("⏺ Armed", METER_WARN, BG),
                        RecordPhase::AwaitingRoll => ("⏺ …", RECORD, BG),
                        RecordPhase::Recording => ("⏺ REC", RECORD, BG),
                    }
                } else {
                    ("⏺ Rec", if recording { RECORD } else { PANEL_ALT },
                     if recording { BG } else { rgb_record_fg(playing) })
                };
                pill(label, bg, fg)
                    .id("transport-rec")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.toggle_record();
                        cx.notify();
                    }))
            })
            .children(self.input_meter())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        pill("−", PANEL_ALT, TEXT).id("bpm-down").cursor_pointer().on_click(
                            cx.listener(|this, _e, _w, cx| {
                                this.nudge_bpm(-1.0);
                                cx.notify();
                            }),
                        ),
                    )
                    .child(div().min_w(px(70.)).text_color(rgb(TEXT)).child(format!("{bpm:.0} BPM")))
                    .child(
                        pill("+", PANEL_ALT, TEXT).id("bpm-up").cursor_pointer().on_click(
                            cx.listener(|this, _e, _w, cx| {
                                this.nudge_bpm(1.0);
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_color(rgb(TEXT_DIM)).child(format!("{bar}.{beat}  ·  tick {tick}")))
                    .child(self.viz_meter())
                    .child(pill(self.project_name(), PANEL_ALT, TEXT))
                    .child(pill(if self.st.jam_mode { "JAM ●" } else { "JAM" }, PANEL_ALT,
                                if self.st.jam_mode { accent } else { TEXT }).id("jam").cursor_pointer()
                        .on_click(cx.listener(|this, _e, _w, cx| { this.st.toggle_jam(); cx.notify(); })))
                    .child(
                        pill(if self.st.mixer_open { "Mixer ◧" } else { "Mixer ◨" }, PANEL_ALT, TEXT)
                            .id("toggle-mixer")
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _ev, _w, cx| {
                                this.st.mixer_open = !this.st.mixer_open;
                                cx.notify();
                            })),
                    )
                    .child(
                        pill("Arrange ⛶", PANEL_ALT, if self.st.arrange_full { accent } else { TEXT })
                            .id("toggle-arrange")
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _ev, _w, cx| {
                                this.st.arrange_full = !this.st.arrange_full;
                                cx.notify();
                            })),
                    )
                    .child(pill(
                        match &self.host.midi_device {
                            Some(d) => format!("🎹 {}", trunc(d, 14)),
                            None => "🎹 —".to_string(),
                        },
                        PANEL_ALT, TEXT_DIM,
                    )),
            );

        let main_row = if self.st.arrange_full {
            // Full-window Arrange mode: just track-headers + timeline, no browser/dock/mixer.
            div()
                .flex_1()
                .flex()
                .min_h(px(0.))
                .child(self.track_headers(cx))
                .child(self.timeline(cx, vis_w))
        } else {
            // Normal layout: browser | center[track_headers+timeline / dock / editor_dock] [| mixer].
            // Center: track/arrange canvas above the editor dock.
            // Horizontal splitter above the editor dock (drag to set dock height).
            let dock_rz = div()
                .id("rz-dock")
                .h(px(5.))
                .flex_shrink_0()
                .bg(rgb(PANEL_ALT))
                .cursor_pointer()
                .on_drag(ResizeDrag(Splitter::Dock), |_, _, _, cx| cx.new(|_| EmptyDrag))
                .on_drag_move(cx.listener(|this, e: &DragMoveEvent<ResizeDrag>, window, cx| {
                    if e.drag(cx).0 != Splitter::Dock { return; }
                    let vh = f32::from(window.viewport_size().height);
                    this.st.dock_h = (vh - f32::from(e.event.position.y)).clamp(80.0, (vh - 160.0).max(80.0));
                    cx.notify();
                }));

            let center = div()
                .flex_1()
                .flex()
                .flex_col()
                .min_h(px(0.))
                // Allow center to shrink below its content width so the mixer dock
                // (a flex_shrink_0 sibling) isn't pushed off the right edge; the
                // timeline clips via overflow_hidden.
                .min_w(px(0.))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .min_h(px(0.))
                        .min_w(px(0.))
                        .child(self.track_headers(cx))
                        .child(self.timeline(cx, vis_w)),
                )
                .child(dock_rz)
                .child(self.editor_dock(cx));

            let browser = self.browser(cx);
            // Vertical splitter between browser and center (drag to set browser width).
            let browser_rz = div()
                .id("rz-browser")
                .w(px(5.))
                .flex_shrink_0()
                .bg(rgb(PANEL_ALT))
                .cursor_pointer()
                .on_drag(ResizeDrag(Splitter::Browser), |_, _, _, cx| cx.new(|_| EmptyDrag))
                .on_drag_move(cx.listener(|this, e: &DragMoveEvent<ResizeDrag>, _w, cx| {
                    if e.drag(cx).0 != Splitter::Browser { return; }
                    this.st.browser_w = f32::from(e.event.position.x).clamp(120.0, 520.0);
                    cx.notify();
                }));
            let mut row = div()
                .flex_1()
                .flex()
                .min_h(px(0.))
                .child(browser)
                .child(browser_rz)
                .child(center);
            if self.st.mixer_open {
                // Vertical splitter between center and mixer (drag to set mixer width).
                let mixer_rz = div()
                    .id("rz-mixer")
                    .w(px(5.))
                    .flex_shrink_0()
                    .bg(rgb(PANEL_ALT))
                    .cursor_pointer()
                    .on_drag(ResizeDrag(Splitter::Mixer), |_, _, _, cx| cx.new(|_| EmptyDrag))
                    .on_drag_move(cx.listener(|this, e: &DragMoveEvent<ResizeDrag>, window, cx| {
                        if e.drag(cx).0 != Splitter::Mixer { return; }
                        let vw = f32::from(window.viewport_size().width);
                        this.st.mixer_w = (vw - f32::from(e.event.position.x)).clamp(140.0, 520.0);
                        cx.notify();
                    }));
                row = row.child(mixer_rz).child(self.mixer(cx));
            }
            row
        };

        let toolbar = self.song_toolbar(cx);

        let status = div().h(px(18.)).px_4().flex_shrink_0().bg(rgb(BG)).text_color(rgb(TEXT_DIM))
            .whitespace_nowrap().overflow_hidden()
            .child(SharedString::from(status_text));

        let menu_bar = self.menu_bar(cx);
        // Loud, actionable banner when the soundfont is absent (audio disabled).
        let sf2_banner = self.sf2_missing.then(|| {
            div()
                .h(px(22.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .px_3()
                .bg(rgb(RECORD))
                .text_color(rgb(0xffffff))
                .child("No soundfont — audio is disabled. Run  ./fetch-assets.sh  then restart.")
        });
        let overlay = self.menu_overlay(cx);
        let fx_overlay = self.audio_effects_overlay(cx);

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .track_focus(&self.root_focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
                let m = &ev.keystroke.modifiers;
                if !m.control {
                    return;
                }
                match ev.keystroke.key.as_str() {
                    "z" if m.shift => { this.redo(); cx.notify(); }
                    "z" => { this.undo(); cx.notify(); }
                    "y" => { this.redo(); cx.notify(); }
                    _ => {}
                }
            }))
            .child(menu_bar)
            .children(sf2_banner)
            .child(transport)
            .child(toolbar)
            .child(status)
            .child(main_row)
            .children(overlay)
            .children(fx_overlay)
    }
}

/// Record pill text color: dim normally; this spike has no record state yet.
fn rgb_record_fg(_playing: bool) -> u32 {
    RECORD
}

/// One-pole low-pass filter magnitude at frequency `f` Hz given a normalised
/// cutoff (0..1 → quadratic scale over [0, sr/2]).
/// `|H(f)| = 1 / sqrt(1 + (f/fc)^2)`
fn lp_magnitude(cutoff01: f32, f: f32, sr: u32) -> f32 {
    let fc = cutoff01.clamp(0.0, 1.0).powi(2) * (sr as f32 / 2.0);
    if fc <= 0.0 { return 0.0; }
    1.0 / (1.0 + (f / fc).powi(2)).sqrt()
}

/// Entry point called from `src/main.rs` under `--gui`.
pub fn run(host: Host, project: Project, path: Option<PathBuf>, sf2_path: String) {
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // Stable identity so the `daw-ai.desktop` launcher associates with this
                // window (Wayland app_id / XWayland WM_CLASS) — needed to pin it.
                app_id: Some("daw-ai".to_string()),
                ..Default::default()
            },
            |window, cx| {
                let export_result = std::sync::Arc::new(std::sync::Mutex::new(None));
                let load_result = std::sync::Arc::new(std::sync::Mutex::new(None));
                let audio_clip_result = std::sync::Arc::new(std::sync::Mutex::new(None));
                let pad_load_result = std::sync::Arc::new(std::sync::Mutex::new(None));
                let save_as_result = std::sync::Arc::new(std::sync::Mutex::new(None));
                let view = cx.new(|cx| DawView {
                    st: HeadlessState::from_project(project, path.clone()),
                    host,
                    playing: false,
                    export_result,
                    load_result,
                    audio_clip_result,
                    pad_load_result,
                    focus: cx.focus_handle(),
                    save_as_result,
                    sf2_path: sf2_path.clone(),
                    sf2_missing: !std::path::Path::new(&sf2_path).exists(),
                    search_focus: cx.focus_handle(),
                    recorder: daw_core::sequencer::MidiRecorder::new(),
                    midi_recording: false,
                    audio_rec: Default::default(),
                    config: daw_io::config::UserConfig::load(),
                    input_devices: Vec::new(),
                    undo: daw_core::undo::UndoStack::new(),
                    root_focus: cx.focus_handle(),
                    rename_focus: cx.focus_handle(),
                    balance_result: Default::default(),
                    clap_scan: {
                        let slot: std::sync::Arc<std::sync::Mutex<Option<Vec<daw_core::audio::clap::ClapPluginDesc>>>> =
                            Default::default();
                        let bg = std::sync::Arc::clone(&slot);
                        // Discovery loads every bundle once — keep it off the UI thread.
                        std::thread::spawn(move || {
                            let found = daw_core::audio::clap::discover();
                            if let Ok(mut s) = bg.lock() {
                                *s = Some(found);
                            }
                        });
                        slot
                    },
                });

                // Focus the root so window-level shortcuts (Ctrl+Z/Y) work
                // before any input field is clicked.
                view.read(cx).root_focus.clone().focus(window);
                // Route live MIDI to the initially-selected track's strip.
                view.read(cx).sync_midi_strip();

                // Engine/IO poll loop (~60 Hz). Polling is cheap and always runs, but
                // we only request a repaint (`cx.notify`) when the transport is playing
                // (the playhead is moving) or `pump`/the playhead changed something —
                // so an idle window costs ~0 CPU instead of rebuilding the whole UI
                // (e.g. a long arrangement timeline) 60 times a second. User input
                // handlers call `cx.notify` themselves, so interaction stays responsive.
                let weak = view.downgrade();
                cx.spawn(async move |cx| {
                    let mut last_tick = u32::MAX;
                    loop {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(16))
                            .await;
                        let r = weak.update(cx, |v, cx| {
                            let mut repaint = v.pump();
                            let tick = v.host.playback_tick.load(Ordering::Relaxed);
                            if tick != last_tick {
                                v.st.set_playhead(tick);
                                last_tick = tick;
                                repaint = true;
                            }
                            if v.playing || repaint {
                                cx.notify();
                            }
                        });
                        if r.is_err() {
                            break; // view dropped → window closed
                        }
                    }
                })
                .detach();

                view
            },
        )
        .unwrap();
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_marks_scale_and_repeat() {
        use daw_core::project::{Note, Pattern, PianoRollPattern, StepPattern, StepRow};
        // 1-bar piano roll, two notes; clip = 2 bars at 60px/bar → repeats.
        let pr = Pattern::PianoRoll(PianoRollPattern {
            id: "p".into(),
            name: "P".into(),
            bars: 1,
            ticks_per_bar: 480,
            notes: vec![
                Note { pitch: 60, tick: 0, duration: 240, vel: 100 },
                Note { pitch: 72, tick: 240, duration: 120, vel: 100 },
            ],
        });
        let marks = clip_preview_marks(&pr, 120.0, 40.0, 960);
        assert_eq!(marks.len(), 4, "two notes × two repetitions: {marks:?}");
        let m0 = &marks[0];
        assert!((m0.0 - 0.0).abs() < 0.01, "x starts at 0");
        assert!((m0.2 - 30.0).abs() < 0.01, "240/960 ticks over 120px = 30px wide: {marks:?}");
        let m1 = &marks[1];
        assert!(m1.1 < m0.1, "pitch 72 above pitch 60: {} < {}", m1.1, m0.1);
        assert!((marks[2].0 - 60.0).abs() < 0.01, "second repetition at one bar: {marks:?}");

        // Step pattern: hits map to columns; rows stack.
        let sp = Pattern::Step(StepPattern {
            id: "s".into(),
            name: "S".into(),
            bars: 1,
            steps: 16,
            rows: vec![
                StepRow {
                    name: "k".into(),
                    pitch: 36,
                    vel: 100,
                    steps: {
                        let mut v = vec![0u8; 16];
                        v[0] = 1;
                        v[8] = 1;
                        v
                    },
                },
                StepRow {
                    name: "h".into(),
                    pitch: 42,
                    vel: 100,
                    steps: {
                        let mut v = vec![0u8; 16];
                        v[4] = 1;
                        v
                    },
                },
            ],
            gate: 0.5,
        });
        let marks = clip_preview_marks(&sp, 60.0, 40.0, 480);
        assert_eq!(marks.len(), 3, "{marks:?}");
        assert!(marks.iter().any(|m| (m.0 - 30.0).abs() < 0.01), "step 8 at halfway: {marks:?}");

        // Cap: a pathological pattern stays bounded.
        let big = Pattern::PianoRoll(PianoRollPattern {
            id: "b".into(),
            name: "B".into(),
            bars: 1,
            ticks_per_bar: 480,
            notes: (0..500)
                .map(|i| Note { pitch: 60, tick: i % 480, duration: 30, vel: 100 })
                .collect(),
        });
        assert!(clip_preview_marks(&big, 6000.0, 40.0, 48_000).len() <= 256);
    }
}

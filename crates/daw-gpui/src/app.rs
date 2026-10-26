mod transport;
mod controls;
use controls::*;
mod arrangement;
mod browser;
mod editors;
mod effects;
mod mixing;
mod menus;
mod window_frame;
mod layout;

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
const TEXT_DIM: u32 = 0xa0a0ab;
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
const TRACK_H: f32 = 72.0;
const STEP_W: f32 = 22.0;
// Piano Roll cell dimensions.
const PR_ROW_H: f32 = 12.0;
const PR_COL_W: f32 = 18.0;
// Two compact rows keep track controls visible while leaving room for the music.
const TRACK_HEADER_W: f32 = 460.0;
const BAR_W: f32 = 64.0;
const MIN_BAR_W: f32 = 8.0;
const MAX_BAR_W: f32 = 200.0;
const RULER_H: f32 = 20.0;
const LOOP_LANE_H: f32 = 8.0;

type PadPick = (String, u8, PathBuf, Option<(u8, u8)>);
type DeviceScan = (Vec<daw_core::audio::input::InputDeviceInfo>, Vec<String>);
type WaveformScan = (PathBuf, Vec<(String, Result<(Vec<f32>, f64), String>)>);

/// GPUI root view: owns the headless state and the engine `Host`.
pub struct DawView {
    st: HeadlessState,
    host: Host,
    playing: bool,
    last_session_poll: std::time::Instant,
    /// Set by a background export thread; drained into `st.status` each frame.
    export_result: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    export_control: Option<std::sync::Arc<daw_core::render_control::RenderControl>>,
    /// Set by the background "Load" file dialog; the picked path is drained and
    /// opened each frame.
    load_result: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    /// Set by the background "+WAV" file dialog; drained each frame and added as
    /// an audio clip on the selected track at the current playhead position.
    audio_clip_result: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    /// Set by the per-pad "load sample" dialog: (track_id, pitch, wav path),
    /// drained each frame into the track's Sampler via `set_sampler_pad`.
    pad_load_result: std::sync::Arc<std::sync::Mutex<Option<PadPick>>>,
    /// Focus handle for the Code tab REPL input.
    focus: FocusHandle,
    /// Set by the background "Save As" file dialog; drained into project_path + save each frame.
    save_as_result: std::sync::Arc<std::sync::Mutex<Option<Result<std::path::PathBuf, ()>>>>,
    save_as_pending: bool,
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
    input_scan: std::sync::Arc<std::sync::Mutex<Option<DeviceScan>>>,
    input_scan_running: bool,
    midi_ports: Vec<String>,
    waveform_scan: std::sync::Arc<std::sync::Mutex<Option<WaveformScan>>>,
    waveform_running: bool,
    waveform_failed: std::collections::HashSet<PathBuf>,
    close_confirmation: bool,
    close_after_save: bool,
    piano_scroll: gpui::ScrollHandle,
    mix_scroll: gpui::ScrollHandle,
    mix_vertical_scroll: gpui::ScrollHandle,
    /// Snapshot undo/redo over `st.project`.
    undo: daw_core::undo::UndoStack,
    /// Focus handle for window-level shortcuts (Ctrl+Z etc.).
    root_focus: FocusHandle,
    /// Filled once by the background CLAP discovery scan; drained in pump().
    clap_scan: std::sync::Arc<std::sync::Mutex<Option<Vec<daw_core::audio::clap::ClapPluginDesc>>>>,
    clap_scanning: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Focus handle for the inline rename editor.
    rename_focus: FocusHandle,
    /// Result of a background ⚖ auto-balance run; drained in pump().
    balance_result: std::sync::Arc<std::sync::Mutex<Option<(String, Result<Vec<daw_core::audio::balance::BalanceResult>, String>)>>>,
    balance_running: bool,
}

impl DawView {
    fn new(host: Host, project: Project, path: Option<PathBuf>, sf2_path: String, cx: &mut Context<Self>) -> Self {
        Self {
            st: HeadlessState::from_project(project, path),
            host,
            playing: false,
            last_session_poll: std::time::Instant::now(),
            export_result: Default::default(),
            export_control: None,
            load_result: Default::default(),
            audio_clip_result: Default::default(),
            pad_load_result: Default::default(),
            focus: cx.focus_handle(),
            save_as_result: Default::default(),
            save_as_pending: false,
            sf2_path: sf2_path.clone(),
            sf2_missing: !std::path::Path::new(&sf2_path).exists(),
            search_focus: cx.focus_handle(),
            recorder: daw_core::sequencer::MidiRecorder::new(),
            midi_recording: false,
            audio_rec: Default::default(),
            config: daw_io::config::UserConfig::load(),
            input_devices: Vec::new(),
            input_scan: Default::default(),
            input_scan_running: false,
            midi_ports: Vec::new(),
            waveform_scan: Default::default(),
            waveform_running: false,
            waveform_failed: Default::default(),
            close_confirmation: false,
            close_after_save: false,
            piano_scroll: gpui::ScrollHandle::new(),
            mix_scroll: gpui::ScrollHandle::new(),
            mix_vertical_scroll: gpui::ScrollHandle::new(),
            undo: daw_core::undo::UndoStack::new(),
            root_focus: cx.focus_handle(),
            rename_focus: cx.focus_handle(),
            balance_result: Default::default(),
            balance_running: false,
            clap_scan: Default::default(),
            clap_scanning: Default::default(),
        }
    }

    fn timeline_width(&self, window: &Window) -> f32 {
        let width = f32::from(window.viewport_size().width);
        if self.st.arrange_full { return (width - TRACK_HEADER_W).max(1.); }
        let layout = layout::PanelLayout::new(width, window_frame::content_height(window),
            self.st.browser_open.then_some(self.st.browser_w), self.st.mixer_open.then_some(self.st.mixer_w),
            self.st.dock_h, self.sf2_missing);
        (width - TRACK_HEADER_W - layout.browser.map_or(0., |w| w + 5.)
            - layout.mixer.map_or(0., |w| w + 5.)).max(1.)
    }

    /// Current accent color (Appearance setting).
    fn accent(&self) -> u32 { ACCENTS[self.st.accent_idx % ACCENTS.len()] }

    fn send_command(&self, command: SequencerCommand) -> Result<(), String> {
        if let Some(session) = &self.host.session {
            if daw_io::session::is_project_edit(&command) {
                session.submit(&self.st.project).map_err(|e| {
                    let text = e.to_string();
                    *self.host.engine_error.lock().unwrap() = Some(text.clone()); text
                })?;
                // Solo is a local audition mask, separate from the saved mute.
                if !matches!(command, SequencerCommand::SetTrackMute { .. }) { return Ok(()); }
            }
        }
        self.host.cmd_tx.try_send(command).map_err(|e| e.to_string())
    }

    fn reconcile_session(&mut self) {
        if let Some(session) = &self.host.session {
            match session.reconcile(&mut self.st.project) {
                Ok(backup) => {
                    self.st.selected_track = self.st.project.tracks.first().map(|t| t.id.clone());
                    self.st.peaks_cache.clear();
                    self.st.peaks_dur_cache.clear();
                    self.waveform_failed.clear();
                    // Clear a frontend conflict diagnostic, preserving engine failures.
                    if let Ok(mut error) = self.host.engine_error.lock() {
                        if error.as_ref().is_some_and(|e| e.contains("revision_conflict")) { *error = None; }
                    }
                    self.st.set_status(backup.map(|p| format!("session refreshed; local copy: {}", p.display())).unwrap_or_else(|| "session synchronized".into()));
                }
                Err(e) => self.st.set_status(format!("sync failed; local work retained: {e}")),
            }
        }
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
        if let Some(session) = &self.host.session {
            if let Err(e) = session.transport(daw_core::session::Command::Seek { tick }) { self.st.set_status(e.to_string()); }
        } else { let _ = self.send_command(SequencerCommand::Seek(tick)); }
    }

    /// Move the playhead to an absolute timeline position in seconds. Used when
    /// clicking an audio clip, which is placed by seconds rather than bars, so
    /// Play starts from exactly where the clip begins.
    fn seek_to_sec(&mut self, sec: f64) {
        let tick = daw_core::sequencer::engine::sec_to_tick(sec, self.st.project.project.bpm);
        if let Some(session) = &self.host.session {
            if let Err(e) = session.transport(daw_core::session::Command::Seek { tick }) { self.st.set_status(e.to_string()); }
        } else { let _ = self.send_command(SequencerCommand::Seek(tick)); }
    }

    /// Nudge the tempo and push it to the engine.
    fn nudge_bpm(&mut self, delta: f64) {
        self.undo.record("BPM", &self.st.project);
        let bpm = (self.st.project.project.bpm + delta).clamp(20.0, 300.0);
        self.st.project.project.bpm = bpm;
        let _ = self.send_command(SequencerCommand::SetBpm(bpm));
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
        if let Some(session) = &self.host.session {
            if let Err(e) = session.submit(&self.st.project) { *self.host.engine_error.lock().unwrap() = Some(e.to_string()); }
            return;
        }
        let _ = self.send_command(SequencerCommand::UpdateLive(self.st.project.clone()));
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
        if let Some(session) = &self.host.session {
            let result = session.history(false, &mut self.st.project);
            self.st.set_status(match result { Ok(()) => "undo: shared session".into(), Err(e) => e.to_string() });
            return;
        }
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
        if let Some(session) = &self.host.session {
            let result = session.history(true, &mut self.st.project);
            self.st.set_status(match result { Ok(()) => "redo: shared session".into(), Err(e) => e.to_string() });
            return;
        }
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
        let _ = self.send_command(SequencerCommand::SetSwing(sw));
    }

    fn adjust_timing(&mut self, d: f32) {
        self.undo.record("timing", &self.st.project);
        let t = (self.st.project.project.timing + d).clamp(0.0, 1.0);
        self.st.project.set_timing(t);
        let _ = self.send_command(SequencerCommand::SetTiming(t));
    }

    fn toggle_metronome(&mut self) {
        self.undo.record("metronome", &self.st.project);
        let on = !self.st.project.project.metronome;
        self.st.project.set_metronome(on);
        let _ = self.send_command(SequencerCommand::SetMetronome(on));
    }

    fn panic_engine(&self) {
        let _ = self.send_command(SequencerCommand::Panic);
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
            let _ = self.send_command(SequencerCommand::SetTrackEffects {
                track_id: track_id.to_string(),
                effects: t.effects.clone(),
            });
        }
    }

    /// Push the project's return-bus config to the engine (applied live).
    fn push_returns(&self) {
        let _ = self.send_command(SequencerCommand::SetReturns(
            self.st.project.returns.clone(),
        ));
    }

    fn nudge_return(&mut self, param: daw_core::project::ReturnParam, delta: f32) {
        self.undo.record("return param", &self.st.project);
        if param == daw_core::project::ReturnParam::DlyTimeMs {
            self.st.project.returns.delay.time_ms = self.st.project.returns.delay.effective_time_ms(self.st.project.project.bpm);
        }
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
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, w, cx| {
                this.rename_focus.focus(w); cx.stop_propagation();
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                cx.stop_propagation();
                if ev.keystroke.modifiers.control || ev.keystroke.modifiers.platform || ev.keystroke.modifiers.alt { return; }
                match ev.keystroke.key.as_str() {
                    "enter" => { this.commit_rename(); this.root_focus.focus(w); }
                    "escape" => { this.st.renaming = None; this.root_focus.focus(w); }
                    "space" => this.st.rename_buf.push(' '),
                    "backspace" => {
                        this.st.rename_buf.pop();
                    }
                    _ => {
                        if let Some(ch) = &ev.keystroke.key_char {
                            this.st.rename_buf.push_str(ch);
                        } else if ev.keystroke.key.chars().count() == 1 {
                            this.st.rename_buf.push_str(&ev.keystroke.key);
                        }
                    }
                }
                cx.notify();
            }))
    }

    fn push_loop_region(&self) {
        let _ = self.send_command(SequencerCommand::SetLoopRegion(
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
        let _ = self.send_command(SequencerCommand::SetClapParam { uid, param_id, value });
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
            let stepped=info.stepped;
            let step=if stepped {((info.max-info.min)/20.).round().max(1.)} else {((info.max-info.min)/20.).max(1e-6)};
            let (uid_d, uid_i, uid_r) = (uid.clone(), uid.clone(), uid.clone());
            let (lo, hi, id) = (info.min, info.max, info.id);
            if info.readonly {
                return div().flex().gap_2().h(px(26.)).flex_shrink_0().text_color(rgb(TEXT_DIM)).child(info.name).child("Read only");
            }
            div()
                .flex()
                .items_center()
                .gap_2()
                .h(px(26.)).flex_shrink_0()
                .child(div().w(px(72.)).flex_shrink_0().whitespace_nowrap().overflow_hidden().text_color(rgb(TEXT_DIM)).child(info.name.clone()))
                .child(cell_btn(format!("cp-{uid}-{id}"), "−", false, PANEL_ALT).on_click(cx.listener(move |this, e: &gpui::ClickEvent, _w, cx| {
                    let d=if e.modifiers().shift && !stepped {step/10.} else {step};
                    let value=if stepped {(cur-d).round()} else {cur-d};
                    this.nudge_clap_param(uid_d.clone(), id, value.clamp(lo, hi));
                    cx.notify();
                })))
                .child(div().w(px(64.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(TEXT)).child(format!("{cur:.2}")))
                .child(cell_btn(format!("cp+{uid}-{id}"), "+", false, PANEL_ALT).on_click(cx.listener(move |this, e: &gpui::ClickEvent, _w, cx| {
                    let d=if e.modifiers().shift && !stepped {step/10.} else {step};
                    let value=if stepped {(cur+d).round()} else {cur+d};
                    this.nudge_clap_param(uid_i.clone(), id, value.clamp(lo, hi));
                    cx.notify();
                })))
                .child(cell_btn(format!("cp-reset-{uid}-{id}"),"Reset",false,PANEL_ALT).on_click(cx.listener(move |this,_,_,cx| {
                    this.nudge_clap_param(uid_r.clone(),id,info.default); cx.notify();
                })))
        });

        div().flex().flex_col().gap_1().child(header).children(rows)
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
        div().flex_shrink_0().child(div().id("pattern-picker-scroll")
            .overflow_x_scroll().flex().gap_1().px_2().py_1().bg(rgb(BG))
            .border_b_1().border_color(rgb(BORDER)).children(editor).children(chips))
    }

    /// Push a track's *effective* mute (own flag OR solo-silenced) to the engine.
    fn push_mute(&self, id: &str) {
        let muted = self.st.effective_muted(id);
        let _ = self.send_command(SequencerCommand::SetTrackMute {
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
        let _ = self.send_command(SequencerCommand::SetTrackVolume {
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
        let _ = self.send_command(SequencerCommand::SetTrackPan {
            track_id: id.to_string(),
            pan: p,
        });
    }

    fn is_dirty(&self) -> bool {
        daw_core::project::format::project_revision(&self.st.project).map_or(true, |hash| hash != self.st.saved_hash)
    }

    fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.st.renaming.is_some() { self.commit_rename(); }
        if self.midi_recording || self.audio_rec.phase != daw_core::audio::input::RecordPhase::Idle {
            self.stop();
        }
        if !self.is_dirty() { return true; }
        self.close_confirmation = true;
        self.st.open_menu = None;
        self.root_focus.focus(window);
        cx.notify();
        false
    }

    fn save_project(&mut self) {
        if self.st.renaming.is_some() { self.commit_rename(); }
        if self.st.project_path.is_none() {
            self.save_as_dialog();
            return;
        }
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
        let persisted = self.host.session.as_ref().filter(|s| Some(s.project_directory().as_path()) != path.parent()).map(|s| s.runtime_project(&self.st.project)).unwrap_or_else(|| self.st.project.clone());
        match daw_core::project::format::save_if_unchanged(&persisted, &path, self.st.file_revision.as_deref()) {
            Ok(revision) => {
                self.st.file_revision = Some(revision);
                self.st.saved_hash = daw_core::project::format::project_revision(&self.st.project).unwrap_or_default();
                self.st.set_status(format!("saved {}", path.display()));
                // Record our own write's mtime so JAM doesn't reload what we just saved.
                self.st.project_mtime = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok());
                self.st.project_path = Some(path);
            }
            Err(e) => self.st.set_status(format!("save failed: {e}")),
        }
    }

    fn export_audio(&mut self, format: daw_core::audio::ExportFormat) {
        if let Some(control) = &self.export_control {
            control.cancel();
            self.st.set_status("export cancellation requested");
            return;
        }
        let source = self.st.project.clone();
        let revision = self.host.session.as_ref().and_then(|s| s.inspect().ok()).map(|s| s.revision);
        let project = self.host.session.as_ref().map(|s| s.runtime_project(&self.st.project)).unwrap_or_else(|| self.st.project.clone());
        let sf2 = std::path::PathBuf::from(&self.sf2_path);
        let base = self.host.session.as_ref().map(|s| s.project_directory()).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let out = base.join("renders").join(daw_io::session::operation_id()).join(format!("export.{}", format.extension()));
        let result = self.export_result.clone();
        let control = std::sync::Arc::new(daw_core::render_control::RenderControl::default());
        self.export_control = Some(control.clone());
        self.st.set_status(format!("exporting {}… (choose Export again to cancel)", format.label()));
        std::thread::spawn(move || {
            let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> anyhow::Result<()> {
                format.check_available()?;
                let assets = daw_core::project::format::asset_fingerprints(&project, &sf2)?;
                let (audio, sample_rate) = daw_core::audio::export::render_range_samples(&project, &sf2, true, None, &control)?;
                control.check()?;
                anyhow::ensure!(assets == daw_core::project::format::asset_fingerprints(&project, &sf2)?, "assets changed during render");
                std::fs::create_dir_all(out.parent().unwrap())?;
                format.write(&out, &audio, sample_rate, &control)?;
                std::fs::write(out.with_file_name("project.json"), serde_json::to_vec_pretty(&source)?)?;
                let report = serde_json::json!({"project_hash": daw_core::project::format::project_revision(&source)?, "session_revision": revision, "engine_version": env!("CARGO_PKG_VERSION"), "build": env!("DAW_BUILD_REVISION"), "sample_rate": sample_rate, "normalized": true, "format": format.extension(), "bitrate_kbps": if format == daw_core::audio::ExportFormat::Mp3 { Some(daw_core::audio::mp3::BITRATE_KBPS) } else { None }, "pcm_peak_ceiling_dbfs": if format == daw_core::audio::ExportFormat::Mp3 { -2 } else { -1 }, "assets": assets});
                std::fs::write(out.with_file_name("report.json"), serde_json::to_vec_pretty(&report)?)?;
                Ok(())
            }));
            let message = match rendered {
                Ok(Ok(())) => format!("exported {}", out.display()),
                Ok(Err(e)) => format!("export failed: {e}"),
                Err(_) => "export worker panicked".into(),
            };
            if let Ok(mut slot) = result.lock() { *slot = Some(message); }
        });
    }

    /// Open a native "Save As" file dialog; the chosen path is drained in pump(),
    /// which sets project_path and calls save_project().
    fn save_as_dialog(&mut self) {
        if self.save_as_pending { return; }
        self.save_as_pending = true;
        let slot = self.save_as_result.clone();
        std::thread::spawn(move || {
            let picked = rfd::FileDialog::new()
                .add_filter("daw project", &["json"])
                .set_file_name("song.daw.json")
                .save_file();
            *slot.lock().unwrap() = Some(picked.ok_or(()));
        });
    }

    fn load_pad_variant_dialog(&mut self, tid: String, pitch: u8, min: u8, max: u8) {
        let slot = self.pad_load_result.clone();
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new().add_filter("WAV sample", &["wav"]).pick_file() {
                *slot.lock().unwrap() = Some((tid, pitch, path, Some((min, max))));
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
                *slot.lock().unwrap() = Some((track_id, pitch, f, None));
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
                if let Some(session) = &self.host.session {
                    if let Err(e) = session.import(&project, &path) { self.st.set_status(e.to_string()); return; }
                    if let Err(e) = session.transport(daw_core::session::Command::Stop) { self.st.set_status(e.to_string()); return; }
                } else {
                    let _ = self.send_command(SequencerCommand::LoadProject(project.clone()));
                }
                self.playing = false;
                self.st = HeadlessState::from_project(project, Some(path.clone()));
                self.waveform_failed.clear();
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
        if let Some(session) = &self.host.session {
            if self.last_session_poll.elapsed() >= std::time::Duration::from_millis(50) {
            self.last_session_poll = std::time::Instant::now();
            match session.sync(&mut self.st.project) {
                Ok(updated) => {
                    changed |= updated;
                    if updated { self.st.selected_track = self.st.project.tracks.first().map(|t| t.id.clone()); }
                }
                Err(e) => { let text = e.to_string(); changed |= self.st.status != text; self.st.set_status(text); }
            }
            }
            let playing = session.playing();
            changed |= self.playing != playing;
            self.playing = playing;
        }
        if let Some(error) = self.host.engine_error.lock().ok().and_then(|e| e.clone()) {
            changed |= self.st.status != error;
            self.st.set_status(error);
        }

        if let Some((devices, ports)) = self.input_scan.lock().ok().and_then(|mut slot| slot.take()) {
            self.input_devices = devices;
            self.midi_ports = ports;
            self.input_scan_running = false;
            changed = true;
        }

        // 0. A file picked by the Load dialog → open it.
        let picked = self.load_result.lock().ok().and_then(|mut s| s.take());
        if let Some(path) = picked {
            self.load_project(path);
            changed = true;
        }

        // Save As writes resolved asset paths so moving the project preserves sound.
        let save_as = self.save_as_result.lock().ok().and_then(|mut s| s.take());
        if save_as.is_some() { self.save_as_pending = false; }
        if let Some(Err(())) = &save_as { self.close_after_save = false; changed = true; }
        if let Some(Ok(path)) = save_as {
            let project = self.host.session.as_ref().map(|s| s.runtime_project(&self.st.project)).unwrap_or_else(|| self.st.project.clone());
            match daw_core::project::format::save_if_unchanged(&project, &path, None) {
                Ok(revision) => {
                    let imported = self.host.session.as_ref().map(|s| s.import(&project, &path)).transpose();
                    match imported {
                        Ok(_) => {
                            self.st.project = project;
                            self.st.project_path = Some(path.clone());
                            self.st.file_revision = Some(revision);
                            self.st.saved_hash = daw_core::project::format::project_revision(&self.st.project).unwrap_or_default();
                            self.st.project_mtime = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok());
                            self.st.set_status(format!("saved {}", path.display()));
                        }
                        Err(e) => self.st.set_status(format!("copy saved to {}; session conflict: {e}", path.display())),
                    }
                }
                Err(e) => self.st.set_status(format!("Save As failed: {e}")),
            }
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
        if let Some((tid, pitch, path, velocity_range)) = pad {
            changed = true;
            let path_str = path.to_string_lossy().to_string();
            let before = self.st.project.clone();
            let updated = if let Some((min_velocity, max_velocity)) = velocity_range {
                self.st.project.add_sampler_variant(&tid, pitch, daw_core::project::SampleVariant {
                    path: path_str.clone(), min_velocity, max_velocity, gain: 1.,
                })
            } else if self.st.project.set_sampler_pad(&tid, pitch, path_str.clone()) { Ok(()) }
            else { Err("select a Sampler track to load a pad".into()) };
            match updated {
                Ok(()) => {
                    self.undo.record("sampler takes", &before);
                    self.push_live();
                    let name = std::path::Path::new(&path_str).file_name()
                        .map(|s| s.to_string_lossy().to_string()).unwrap_or(path_str);
                    self.st.set_status(format!("pad p{pitch} ← {name}"));
                }
                Err(error) => self.st.set_status(error),
            }
        }

        // 0c. Ensure waveform peaks are cached for every audio clip (once per path).
        let base = self.host.session.as_ref().map(|s| s.project_directory()).unwrap_or_else(|| self.st.project_path.as_deref().and_then(std::path::Path::parent).unwrap_or(std::path::Path::new(".")).to_path_buf());
        let legacy = std::env::current_dir().unwrap_or_default();
        let ready = self.waveform_scan.lock().ok().and_then(|mut slot| slot.take());
        if let Some((source_base, results)) = ready {
            self.waveform_running = false;
            if source_base == base {
                for (path, result) in results {
                    match result {
                        Ok((peaks, duration)) => {
                            self.st.peaks_cache.insert(path.clone(), std::sync::Arc::new(peaks));
                            self.st.peaks_dur_cache.insert(path, duration);
                        }
                        Err(error) => {
                            self.waveform_failed.insert(daw_core::project::format::resolve_asset_path(&path, &base, &legacy));
                            self.st.set_status(format!("waveform unavailable for {path}: {error}"));
                        }
                    }
                }
                changed = true;
            }
        }
        if !self.waveform_running {
            let paths: std::collections::HashSet<String> = self.st.project.audio_clips.iter()
                .filter(|c| !self.st.peaks_cache.contains_key(&c.path)
                    && !self.waveform_failed.contains(&daw_core::project::format::resolve_asset_path(&c.path, &base, &legacy)))
                .map(|c| c.path.clone()).take(64).collect();
            if !paths.is_empty() {
                self.waveform_running = true;
                let slot = self.waveform_scan.clone();
                std::thread::spawn(move || {
                    let results = paths.into_iter().map(|path| {
                        let resolved = daw_core::project::format::resolve_asset_path(&path, &base, &legacy);
                        let result = daw_core::audio::wav_peaks(&resolved, 512)
                            .and_then(|peaks| daw_core::audio::wav_duration_sec(&resolved).map(|duration| (peaks, duration)))
                            .map_err(|e| e.to_string());
                        (path, result)
                    }).collect();
                    if let Ok(mut slot) = slot.lock() { *slot = Some((base, results)); }
                });
            }
        }

        // Background balance is tied to its source snapshot; stale plans never apply.
        let balance = self.balance_result.lock().ok().and_then(|mut s| s.take());
        if let Some((hash, result)) = balance {
            self.balance_running = false;
            match result {
                Err(error) => self.st.set_status(format!("balance failed: {error}")),
                Ok(_) if daw_core::project::format::project_revision(&self.st.project).unwrap_or_default() != hash => self.st.set_status("project changed during analysis; rerun balance"),
                Ok(plan) => {
                    let n = plan.len();
                    self.edit("balance", |project| { for r in plan { project.set_track_volume(&r.track_id, r.new_volume); } });
                    self.st.set_status(format!("balanced {n} tracks"));
                }
            }
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
                        let _ = self.send_command(SequencerCommand::PreviewNote {
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
                        let _ = self.send_command(SequencerCommand::PreviewNoteOff {
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
                    let _ = self.send_command(SequencerCommand::RawCC { channel: ch, cc, value });
                }
            }
        }

        // 2. JAM hot-reload: reload the project file when its mtime changes.
        if self.st.jam_mode {
            if let Some(path) = self.st.project_path.clone() {
                let mtime = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok());
                if mtime != self.st.project_mtime {
                    let first_observation = self.st.project_mtime.is_none();
                    if first_observation { self.st.project_mtime = mtime; }
                    if !first_observation {
                        if daw_core::project::format::project_revision(&self.st.project).ok().as_deref() != Some(self.st.saved_hash.as_str()) {
                            self.st.set_status("JAM conflict: external edit detected; save local work to a new file before loading it");
                            return true;
                        }
                        if let Ok((p, revision)) = daw_core::project::format::load_versioned(&path) {
                            self.st.project_mtime = mtime;
                            self.st.file_revision = Some(revision);
                            self.st.saved_hash = daw_core::project::format::project_revision(&p).unwrap_or_default();
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
                self.export_control = None;
                self.st.set_status(msg);
                changed = true;
            }
        }

        // 4. Audio recording: roll-tick latch, finished takes, live meter.
        changed |= self.poll_recording();

        changed
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

/// A stationary clipping window for manually scrolled track content.
fn track_viewport() -> gpui::Div {
    div().relative().flex_1().min_h(px(0.)).min_w(px(0.)).overflow_hidden()
}

/// Explicit document bounds avoid flex shrinking and keep scrolling independent
/// from the stationary viewport mask. Shared by track headers, lanes and ruler.
fn scroll_document(width: f32, height: f32, x: f32, y: f32) -> gpui::Div {
    div().absolute().left(px(-x)).top(px(-y))
        .w(px(width)).h(px(height)).flex().flex_col()
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
impl Render for DawView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `render` is side-effect free: engine polling + the playhead mirror run on
        // the repaint timer (see `run`), which only re-renders when playing or when
        // something changed — so an idle window doesn't rebuild the UI 60×/second.

        // Window width — a conservative bound for timeline virtualization.
        let vis_w = f32::from(window.viewport_size().width);
        let layout = layout::PanelLayout::new(vis_w, window_frame::content_height(window),
            self.st.browser_open.then_some(self.st.browser_w),
            self.st.mixer_open.then_some(self.st.mixer_w), self.st.dock_h, self.sf2_missing);
        self.st.dock_h = layout.dock_height;
        let timeline_width = self.timeline_width(window);
        self.st.arrange_scroll_x = self.st.arrange_scroll_x.clamp(0.,
            (self.st.project.song_bars() as f32 * self.bar_w() - timeline_width).max(0.));
        // Re-clamp vertical track scroll so a window resize, dock drag, or removed
        // track can never leave the view scrolled past the last track.
        let max_scroll_y = self.tracks_max_scroll_y(window_frame::content_height(window));
        if self.st.arrange_scroll_y > max_scroll_y {
            self.st.arrange_scroll_y = max_scroll_y;
        }
        let accent = self.accent();
        let playing = self.playing;
        let recording = self.midi_recording;
        let bpm = self.st.bpm();
        let tick = self.st.playhead();
        let bar = tick / daw_core::sequencer::TICKS_PER_BAR + 1;
        let beat = (tick % daw_core::sequencer::TICKS_PER_BAR) / daw_core::sequencer::TICKS_PER_BEAT + 1;
        let live_status = self.host.session.as_ref().map(|s| s.status_line());
        let status_text = if self.st.status.is_empty() && live_status.is_some() {
            live_status.unwrap()
        } else if self.st.status.is_empty() {
            "Select a clip to edit · Wheel: timeline scroll · Ctrl+wheel: zoom · Shift+wheel: tracks".to_string()
        } else {
            live_status.map(|s| format!("{} · {s}", self.st.status)).unwrap_or_else(|| self.st.status.clone())
        };

        let transport = div()
            .id("transport")
            .overflow_x_scroll()
            .whitespace_nowrap()
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
            .children(self.host.session.as_ref().map(|_| {
                cell_btn("sync-session", "Sync session", false, PANEL_ALT)
                    .on_click(cx.listener(|this, _, _, cx| { this.reconcile_session(); cx.notify(); }))
            }))
            // Play / Pause toggle.
            .child(
                div()
                    .id("transport-play").flex_shrink_0()
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
                    .child(pill(format!("{}{}", self.project_name(), if self.is_dirty() { " · unsaved" } else { "" }), PANEL_ALT, TEXT))
                    .child(pill(if self.st.jam_mode { "JAM ●" } else { "JAM" }, PANEL_ALT,
                                if self.st.jam_mode { accent } else { TEXT }).id("jam").cursor_pointer()
                        .on_click(cx.listener(|this, _e, _w, cx| { this.st.toggle_jam(); cx.notify(); })))
                    .child(pill("Browser", PANEL_ALT, if layout.browser.is_some() { accent } else { TEXT })
                        .id("toggle-browser").cursor_pointer().on_click(cx.listener(|this, _, _, cx| {
                            this.st.browser_open = !this.st.browser_open; cx.notify();
                        })))
                    .child(
                        pill("Mixing", PANEL_ALT, if self.st.active_dock == DockTab::Mixing { accent } else { TEXT })
                            .id("toggle-mixer")
                            .debug_selector(|| "toggle-mixer".into())
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _ev, _w, cx| {
                                this.st.active_dock = DockTab::Mixing;
                                this.st.arrange_full = false;
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

        let main_row = if self.st.active_dock == DockTab::Mixing && !self.st.arrange_full {
            div().flex_1().min_h_0().min_w_0().flex().flex_col()
                .child(self.editor_tabs(cx))
                .child(self.mixing_workspace(vis_w, cx))
        } else if self.st.arrange_full {
            // Full-window Arrange mode: just track-headers + timeline, no browser/dock/mixer.
            div()
                .flex_1()
                .flex()
                .min_h(px(0.))
                .child(self.track_headers(cx))
                .child(self.timeline(cx, timeline_width))
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
                        .child(self.timeline(cx, timeline_width)),
                )
                .child(dock_rz)
                .child(self.editor_dock(cx));

            let browser = layout.browser.map(|width| self.browser(cx, width));
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
                .children(browser)
                .children(layout.browser.map(|_| browser_rz))
                .child(center);
            if let Some(width) = layout.mixer {
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
                row = row.child(mixer_rz).child(self.mixer(cx, width));
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
        let close_overlay = self.close_overlay(cx);

        let content = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .track_focus(&self.root_focus)
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, w, _| {
                this.root_focus.focus(w);
            }))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                if this.close_confirmation {
                    if ev.keystroke.key == "escape" {
                        this.close_confirmation = false;
                        this.close_after_save = false;
                        cx.notify();
                    }
                    cx.stop_propagation();
                    return;
                }
                if (ev.keystroke.modifiers.control || ev.keystroke.modifiers.platform) && ev.keystroke.key == "s" && this.st.open_menu.is_none() {
                    this.save_project(); cx.stop_propagation(); cx.notify(); return;
                }
                if ev.keystroke.key == "escape" && (this.st.open_menu.is_some() || this.st.fx_window_open) {
                    if this.st.open_menu.take().is_none() { this.st.fx_window_open = false; }
                    this.root_focus.focus(w);
                    cx.stop_propagation();
                    cx.notify();
                } else if this.st.open_menu.is_some() {
                    // Dropdowns own keyboard input; hidden editors and global
                    // transport/undo shortcuts must not act beneath them.
                    cx.stop_propagation();
                }
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                if this.search_focus.is_focused(w) || this.rename_focus.is_focused(w) || this.focus.is_focused(w) { return; }
                let m = &ev.keystroke.modifiers;
                if ev.keystroke.key == "space" && !m.control { this.toggle_play(); cx.notify(); return; }
                if m.control && ev.keystroke.key == "s" { this.save_project(); cx.notify(); return; }
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
            .children(fx_overlay)
            .children(overlay)
            .children(close_overlay);
        window_frame::frame(content, window, cx.listener(|this, _, w, cx| {
            if this.confirm_close(w, cx) { w.remove_window(); }
        }))
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
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("daw-ai".into()),
                    ..Default::default()
                }),
                // GPUI 0.2.2 records Server even when Wayland has no decoration
                // protocol (GNOME), hiding our fallback without drawing native
                // chrome. Request Client explicitly; X11 can fall back to Server
                // when its compositor cannot support client decorations.
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(size(px(800.), px(480.))),
                is_movable: true,
                is_resizable: true,
                is_minimizable: true,
                // Stable identity so the `daw-ai.desktop` launcher associates with this
                // window (Wayland app_id / XWayland WM_CLASS) — needed to pin it.
                app_id: Some("daw-ai".to_string()),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| DawView::new(host, project, path.clone(), sf2_path.clone(), cx));
                view.update(cx,|view,_|view.start_clap_scan());

                let close_view = view.downgrade();
                window.on_window_should_close(cx, move |window, cx| {
                    close_view.update(cx, |v, cx| v.confirm_close(window, cx)).unwrap_or(true)
                });
                let window_handle = window.window_handle();

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
                            let close = v.close_after_save && !v.is_dirty();
                            if v.playing || repaint {
                                cx.notify();
                            }
                            close
                        });
                        if matches!(r, Ok(true)) {
                            let _ = cx.update_window(window_handle, |_, window, _| window.remove_window());
                            break;
                        }
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

    #[gpui::test]
    fn file_menu_starts_mp3_export_and_other_export_action_cancels_it(cx: &mut gpui::TestAppContext) {
        let (host, _commands, _audio) = Host::headless();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut v = DawView::new(host, Project::default_project(), None, String::new(), cx);
            v.st.open_menu = Some(TopMenu::File);
            v.sf2_missing = false;
            v
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("fm-export").is_some(), "WAV remains available");
        let button = cx.debug_bounds("fm-export-mp3").expect("MP3 export is visible in File");
        cx.simulate_click(button.center(), gpui::Modifiers::default());
        view.read_with(cx, |v, _| {
            assert!(v.st.open_menu.is_none());
            assert!(v.st.status.starts_with("exporting MP3"));
            assert!(v.export_control.is_some());
        });
        view.update(cx, |v, cx| { v.st.open_menu = Some(TopMenu::File); cx.notify(); });
        cx.run_until_parked();
        let button = cx.debug_bounds("fm-export").unwrap();
        cx.simulate_click(button.center(), gpui::Modifiers::default());
        view.read_with(cx, |v, _| {
            assert!(v.export_control.as_ref().unwrap().cancelled.load(Ordering::Relaxed));
            assert_eq!(v.st.status, "export cancellation requested");
        });
    }

    #[derive(Default)]
    struct OverlayProbe {
        open: bool,
        track_clicks: usize,
        track_presses: usize,
        track_scrolls: usize,
        effect_clicks: usize,
        effect_scrolls: usize,
        dismissals: usize,
    }
    impl Render for OverlayProbe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().relative()
                .child(div().id("covered-track").absolute().inset_0()
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, _| this.track_presses += 1))
                    .on_click(cx.listener(|this, _, _, _| this.track_clicks += 1))
                    .on_scroll_wheel(cx.listener(|this, _, _, _| this.track_scrolls += 1)))
                .when(self.open, |root| root
                    .child(overlay_surface().id("probe-scrim").absolute().top(px(24.)).left_0().right_0().bottom_0()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open = false; this.dismissals += 1; cx.stop_propagation(); cx.notify();
                        })))
                    .child(overlay_surface().id("probe-panel").absolute().left(px(40.)).top(px(40.)).w(px(180.)).h(px(120.))
                        .on_scroll_wheel(cx.listener(|this, _, _, _| this.effect_scrolls += 1))
                        .child(cell_btn("probe-fx", "+", false, PANEL_ALT).w(px(60.)).h(px(40.))
                            .on_click(cx.listener(|this, _, _, _| this.effect_clicks += 1)))))
        }
    }

    #[gpui::test]
    fn overlay_controls_blank_space_and_dismissal_do_not_reach_tracks(cx: &mut gpui::TestAppContext) {
        use gpui::{point, Modifiers};
        let (probe, cx) = cx.add_window_view(|_, _| OverlayProbe { open: true, ..Default::default() });
        cx.simulate_click(point(px(60.), px(60.)), Modifiers::default());
        cx.simulate_click(point(px(180.), px(140.)), Modifiers::default());
        probe.read_with(cx, |state, _| {
            assert_eq!(state.effect_clicks, 1, "the effects button must still work");
            assert!(state.open, "panel clicks must not dismiss the dropdown behind it");
            assert_eq!(state.track_clicks, 0);
            assert_eq!(state.track_presses, 0, "no hidden clip drag may start");
        });
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: point(px(100.), px(100.)),
            delta: gpui::ScrollDelta::Lines(point(0., -3.)),
            modifiers: Modifiers::default(), touch_phase: gpui::TouchPhase::Moved,
        });
        probe.read_with(cx, |state, _| {
            assert_eq!(state.effect_scrolls, 1);
            assert_eq!(state.track_scrolls, 0);
        });
        // A backdrop click dismisses the menu without activating that position's track.
        cx.simulate_click(point(px(300.), px(200.)), Modifiers::default());
        probe.read_with(cx, |state, _| {
            assert!(!state.open); assert_eq!(state.dismissals, 1);
            assert_eq!(state.track_clicks, 0); assert_eq!(state.track_presses, 0);
        });
        // Once closed, the next click should work normally.
        cx.simulate_click(point(px(300.), px(200.)), Modifiers::default());
        probe.read_with(cx, |state, _| { assert_eq!(state.track_clicks, 1); assert_eq!(state.track_presses, 1); });
    }

    #[gpui::test]
    fn scrolling_keeps_offscreen_tracks_and_clip_ends_visible(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        use gpui::{canvas, point};

        let cx = cx.add_empty_window();
        let row_count = 18;
        let document_h = row_count as f32 * TRACK_H;
        // Render repeatedly in the same window, moving both axes and resizing.
        // The last two tracks and bars 30–40 start outside the initial viewport.
        for (width, height, x, y, bar_w) in [
            (640.0, 288.0, 0.0, 0.0, 64.0),
            (640.0, 144.0, 29.0 * 64.0, document_h - 144.0, 64.0),
            (320.0, 216.0, 35.0 * 64.0, document_h - 216.0, 64.0),
            (320.0, 144.0, 30.0 * 32.0, document_h - 144.0, 32.0),
            (640.0, 288.0, 0.0, 0.0, 64.0),
        ] {
            let painted = Rc::new(RefCell::new(Vec::new()));
            let output = painted.clone();
            cx.draw(point(px(0.), px(0.)), size(px(width), px(height)), |_, _| {
                div().w(px(width)).h(px(height)).flex().flex_col().overflow_hidden()
                    .child(track_viewport().child(
                        scroll_document(40.0 * bar_w, document_h, x, y)
                            .children((0..row_count).map(|row| {
                                let output = output.clone();
                                div().h(px(TRACK_H)).flex_shrink_0().child(
                                    canvas(|_, _, _| (), move |bounds, _, window, _| {
                                        output.borrow_mut().push((row, bounds, window.content_mask().bounds));
                                    }).size_full(),
                                )
                            })),
                    ))
            });
            let painted = painted.borrow();
            let visible: Vec<_> = painted.iter().filter_map(|(row, bounds, mask)| {
                let visible = bounds.intersect(mask);
                if visible.size.height <= px(0.) || visible.size.width <= px(0.) {
                    return None;
                }
                assert_eq!(visible.origin.x, px(0.), "left edge after scrolling");
                assert_eq!(visible.size.width, px(width), "clips must cover the viewport after scrolling");
                Some(*row)
            }).collect();
            let first = (y / TRACK_H) as usize;
            let end = ((y + height) / TRACK_H) as usize;
            assert_eq!(visible, (first..end).collect::<Vec<_>>(), "scrolled tracks must paint");
        }
    }

    #[gpui::test]
    fn whole_gui_renders_docks_menus_and_effects_after_resize(cx: &mut gpui::TestAppContext) {
        let (host, _commands, _audio) = Host::headless();
        let mut project = Project::default_project();
        project.tracks[0].instrument = serde_json::from_value(serde_json::json!({"type":"sampler","pads":[{"pitch":36,"path":"base.wav","variants":[{"path":"soft.wav","min_velocity":1,"max_velocity":63}]}]})).unwrap();
        let original = project.tracks[0].clone();
        for i in 0..18 {
            let mut track = original.clone();
            track.id = format!("extra-{i}");
            track.name = format!("Long track name {i}");
            project.tracks.push(track);
        }
        project.patterns.push(daw_core::project::Pattern::PianoRoll(daw_core::project::PianoRollPattern {
            id: "review-piano".into(), name: "Long off-grid phrase".into(), bars: 40, ticks_per_bar: 480,
            notes: vec![daw_core::project::Note { pitch: 60, tick: 62, duration: 55, vel: 80 }], legato: false,
        }));
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = DawView::new(host, project, None, String::new(), cx);
            view.sf2_missing = false;
            view
        });
        for (width, height) in [(1280., 800.), (800., 480.), (1920., 1080.)] {
            cx.simulate_resize(size(px(width), px(height)));
            for tab in DockTab::ALL {
                view.update(cx, |v, cx| {
                    v.st.active_dock = tab; v.st.mixer_open = true;
                    if tab == DockTab::PianoRoll { v.st.selected_pattern = Some("review-piano".into()); }
                    else { v.st.selected_pattern = v.st.project.tracks[0].patterns.first().cloned(); }
                    v.st.browser_w = 520.; v.st.mixer_w = 520.; v.st.dock_h = 1000.;
                    v.st.browser_search = "a".into();
                    v.st.open_menu = None; v.st.fx_window_open = false;
                    cx.notify();
                });
                cx.run_until_parked();
            }
            for menu in [TopMenu::File, TopMenu::Settings] {
                view.update(cx, |v, cx| { v.st.open_menu = Some(menu); cx.notify(); });
                cx.run_until_parked();
            }
            view.update(cx, |v, cx| { v.st.open_menu = None; v.st.fx_window_open = true; cx.notify(); });
            cx.run_until_parked();
            view.update(cx, |v, cx| { v.close_confirmation = true; cx.notify(); });
            cx.run_until_parked();
            view.update(cx, |v, cx| { v.close_confirmation = false; cx.notify(); });
        }
    }

    #[gpui::test]
    fn rename_restores_shortcuts_and_control_click_does_not_change_selection(cx: &mut gpui::TestAppContext) {
        let (host, commands, _audio) = Host::headless();
        let project = Project::default_project();
        let track_id = project.tracks[0].id.clone();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut v = DawView::new(host, project, None, String::new(), cx);
            v.sf2_missing = false;
            v.st.arrange_full = true;
            v
        });
        cx.update(|window, cx| view.update(cx, |v, cx| {
            v.start_rename(RenameTarget::Track(track_id.clone()), String::new());
            v.rename_focus.focus(window); cx.notify();
        }));
        cx.run_until_parked();
        cx.simulate_keystrokes("w a r m space k e y s enter");
        view.read_with(cx, |v, _| {
            assert_eq!(v.st.project.tracks[0].name, "warm keys");
            assert!(v.st.renaming.is_none());
        });
        cx.simulate_keystrokes("space");
        assert!(commands.try_iter().any(|c| matches!(c, SequencerCommand::Play)));
        // A mute button in the second header must not select that track or reset
        // the musician's selected pattern through the parent row click handler.
        let selected = view.read_with(cx, |v, _| v.st.selected_track.clone());
        cx.simulate_click(gpui::point(px(305.), px(120. + RULER_H + LOOP_LANE_H + TRACK_H + 18.)), gpui::Modifiers::default());
        view.read_with(cx, |v, _| { assert_eq!(v.st.selected_track, selected); assert!(v.st.project.tracks[1].muted, "mute button must still act"); });
    }

    #[gpui::test]
    fn piano_roll_click_removes_authored_off_grid_note(cx: &mut gpui::TestAppContext) {
        let (host, _commands, _audio) = Host::headless();
        let mut project = Project::default_project();
        project.patterns.push(daw_core::project::Pattern::PianoRoll(daw_core::project::PianoRollPattern {
            id: "offgrid".into(), name: "Off grid".into(), bars: 8, ticks_per_bar: 480, legato: false,
            notes: vec![daw_core::project::Note { pitch: 60, tick: 62, duration: 55, vel: 80 }],
        }));
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut v = DawView::new(host, project, None, String::new(), cx);
            v.sf2_missing = false;
            v.st.active_dock = DockTab::PianoRoll;
            v.st.selected_pattern = Some("offgrid".into());
            v.st.dock_h = 350.;
            v
        });
        view.update(cx, |v, cx| {
            v.piano_scroll.set_offset(gpui::point(px(0.), px(-200.)));
            cx.notify();
        });
        cx.run_until_parked();
        let position = view.read_with(cx, |v, _| {
            let origin = v.piano_scroll.bounds().origin;
            origin + gpui::point(px(8. + 44. + 2. * PR_COL_W + 9.),
                px(8. + 28. * (PR_ROW_H + 1.) + 6.) + v.piano_scroll.offset().y)
        });
        cx.simulate_click(position, gpui::Modifiers { shift: true, ..Default::default() });
        view.read_with(cx, |v, _| {
            let pattern=v.st.project.patterns.iter().find(|p| p.id()=="offgrid").unwrap();
            let daw_core::project::Pattern::PianoRoll(pr)=pattern else { panic!("piano") };
            assert_eq!((pr.notes[0].tick,pr.notes[0].duration,pr.notes[0].vel),(62,55,96));
        });
        cx.simulate_click(position, gpui::Modifiers::default());
        view.read_with(cx, |v, _| {
            let pattern = v.st.project.patterns.iter().find(|p| p.id() == "offgrid").unwrap();
            let daw_core::project::Pattern::PianoRoll(pattern) = pattern else { panic!("piano pattern") };
            assert!(pattern.notes.is_empty(), "remove the original tick 62 rather than create a snapped duplicate");
        });
    }

    #[gpui::test]
    fn closing_dirty_project_opens_modal_and_escape_cancels(cx: &mut gpui::TestAppContext) {
        let (host, _commands, _audio) = Host::headless();
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut v = DawView::new(host, Project::default_project(), None, String::new(), cx);
            v.sf2_missing = false;
            v.root_focus.focus(window);
            v
        });
        cx.update(|window, cx| view.update(cx, |v, cx| {
            assert!(!v.is_dirty());
            v.st.project.project.bpm += 1.;
            assert!(!v.confirm_close(window, cx));
        }));
        cx.run_until_parked();
        view.read_with(cx, |v, _| assert!(v.close_confirmation));
        cx.simulate_keystrokes("space");
        view.read_with(cx, |v, _| assert!(!v.playing));
        cx.simulate_keystrokes("escape");
        view.read_with(cx, |v, _| assert!(!v.close_confirmation));
        view.update(cx, |v, _| {
            v.close_confirmation = true; v.close_after_save = true; v.save_as_pending = true;
            *v.save_as_result.lock().unwrap() = Some(Err(()));
            v.pump();
            assert!(!v.close_after_save, "cancelling Save As must cancel deferred closure");
            assert!(!v.save_as_pending);
            assert!(v.is_dirty());
        });
    }

    #[gpui::test]
    fn save_conflict_keeps_close_pending_and_project_intact(cx: &mut gpui::TestAppContext) {
        let (host, _commands, _audio) = Host::headless();
        let directory = std::env::temp_dir().join(format!("daw-ui-save-{}", daw_io::session::operation_id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("song.daw.json");
        let project = Project::default_project();
        daw_core::project::format::save(&project, &path).unwrap();
        let (view, cx) = cx.add_window_view(|_, cx| DawView::new(host, project.clone(), Some(path.clone()), String::new(), cx));
        // Simulate another writer after opening, then attempt Save & close.
        let mut external = project;
        external.project.bpm = 140.;
        daw_core::project::format::save(&external, &path).unwrap();
        view.update(cx, |v, _| {
            v.st.project.project.bpm = 90.;
            v.close_confirmation = true; v.close_after_save = true;
            v.save_project();
            assert!(v.is_dirty());
            assert!(v.close_confirmation);
            assert!(v.st.status.starts_with("save failed:"));
            assert_eq!(v.st.project.project.bpm, 90.);
        });
        assert_eq!(daw_core::project::format::load(&path).unwrap().project.bpm, 140.);
        std::fs::remove_dir_all(directory).unwrap();
    }

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
            ], legato: false,
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
            legato: false,
        });
        assert!(clip_preview_marks(&big, 6000.0, 40.0, 48_000).len() <= 256);
    }
}

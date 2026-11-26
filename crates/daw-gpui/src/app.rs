mod transport;
mod controls;
use controls::*;
mod arrangement;
mod comments;
mod automation;
mod browser;
mod editors;
mod effects;
mod fx_library;
mod output_analyzer;
mod guitar;
mod inputs;
mod mixing;
mod menus;
mod model_m_midi;
mod window_frame;
mod layout;
mod theme;

use crate::state::{AudioDragKind, DockTab, HeadlessState, RenameTarget};
use daw_core::project::Project;
use daw_core::sequencer::SequencerCommand;
use daw_io::Host;
use gpui::{
    div, prelude::*, px, rgb, size, App, Application, Bounds, Context, DragMoveEvent, FocusHandle,
    KeyDownEvent, SharedString, Window, WindowBounds, WindowOptions,
};
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use theme::{Theme, ON_CLIP};

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
const PR_ROW_H: f32 = 16.0;
const PR_KEY_W: f32 = 88.0;
const PR_KEYBOARD_GUTTER: f32 = PR_KEY_W + 4.0;
const PR_COL_W: f32 = 18.0;
// Two compact rows keep track controls visible while leaving room for the music.
const TRACK_HEADER_W: f32 = 460.0;
const HEADER_H: f32 = 32.0;
const TRANSPORT_H: f32 = 44.;
const TOOLS_H: f32 = 32.;
const STATUS_H: f32 = 22.;
const ARRANGE_HEADER_H: f32 = HEADER_H + TRANSPORT_H + TOOLS_H;
const BAR_W: f32 = 64.0;
const MIN_BAR_W: f32 = 8.0;
const MAX_BAR_W: f32 = 200.0;
const RULER_H: f32 = 32.0;
const ARRANGE_SCROLLBAR_H: f32 = 16.0;
const LOOP_LANE_H: f32 = 8.0;
const SOUNDFONT_BANNER_H: f32 = 22.;
const ARRANGE_CHROME_H: f32 = ARRANGE_HEADER_H + STATUS_H + RULER_H + LOOP_LANE_H + ARRANGE_SCROLLBAR_H;

type PadPick = (String, u8, PathBuf, Option<(u8, u8)>);
type DeviceScan = (Vec<daw_core::audio::input::InputDeviceInfo>, Vec<String>);
type WaveformScan = (PathBuf, Vec<(String, Result<(Vec<f32>, f64), String>)>);

/// GPUI root view: owns the headless state and the engine `Host`.
pub struct DawView {
    st: HeadlessState,
    host: Host,
    playing: bool,
    last_session_poll: std::time::Instant,
    live_status: Option<String>,
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
    browser_scroll: gpui::ScrollHandle,
    fx_library: fx_library::EffectLibrary,
    fx_controls_group: Option<&'static str>,
    fx_slot_details: Option<String>,
    output_analyzer: output_analyzer::OutputAnalyzerView,
    /// Real-time MIDI → piano-roll recorder (records into the open pattern).
    recorder: daw_core::sequencer::MidiRecorder,
    /// True while armed/recording MIDI into the open piano-roll pattern.
    midi_recording: bool,
    midi_track: Option<String>,
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
    /// What the unsaved-changes dialog guards: closing, New or Load.
    confirm_then: ConfirmThen,
    /// Engine failure last drawn beside the status line.
    engine_error_shown: Option<String>,
    /// External file state already reported by JAM (conflict or parse error),
    /// so a broken or conflicting file is announced once, not every tick.
    jam_reported: Option<(Option<std::time::SystemTime>, JamIssue)>,
    /// Track an audio take was started on; the finished take lands there
    /// even if another track was selected while it recorded.
    rec_track: Option<String>,
    armed_track: Option<String>,
    armed_input: Option<daw_core::project::TrackInput>,
    capture_guitar: bool,
    audio_take_active: bool,
    audio_take_finishing: bool,
    input_track: Option<String>,
    /// Whether the playhead was inside the visible timeline last frame
    /// (page-follow only when playback carries it out of view).
    playhead_was_visible: bool,
    timeline_scroll_grab: Option<f32>,
    volume_automation_open: bool,
    comment_menu: Option<(comments::CommentLocation, gpui::Point<gpui::Pixels>)>,
    comment_draft: Option<comments::CommentDraft>,
    comment_focus: FocusHandle,
    /// Latency text last drawn on the Amp button (repaint only when it changes).
    amp_readout_shown: Option<String>,
    ampforge_save_at: Option<std::time::Instant>,
    /// Set by the cabinet-IR file dialog; drained in pump().
    ir_pick: std::sync::Arc<std::sync::Mutex<Option<(bool, std::path::PathBuf)>>>,
    cabinet_export_result: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    guitar_window: Option<gpui::WindowHandle<guitar::GuitarWindow>>,
    /// Cached unsaved-changes flag for the title pill: (when computed, value).
    dirty_shown: std::cell::Cell<(Option<std::time::Instant>, bool)>,
    piano_scroll: gpui::ScrollHandle,
    piano_scroll_grab: Option<f32>,
    piano_scrolled_pattern: Option<String>,
    piano_step_input: Option<(String, u32)>,
    piano_note_lights: [u64; 2],
    mix_scroll: gpui::ScrollHandle,
    mix_vertical_scroll: gpui::ScrollHandle,
    /// Snapshot undo/redo over `st.project`.
    undo: daw_core::undo::UndoStack,
    /// Focus handle for window-level shortcuts (Ctrl+Z etc.).
    root_focus: FocusHandle,
    /// Filled once by the background CLAP discovery scan; drained in pump().
    clap_scan: std::sync::Arc<std::sync::Mutex<Option<Vec<daw_core::audio::clap::ClapPluginDesc>>>>,
    clap_scanning: std::sync::Arc<std::sync::atomic::AtomicBool>,
    plugin_import: Option<std::sync::mpsc::Receiver<browser::PluginImportResult>>,
    /// Focus handle for the inline rename editor.
    rename_focus: FocusHandle,
    /// Result of a background ⚖ auto-balance run; drained in pump().
    balance_result: std::sync::Arc<std::sync::Mutex<Option<(String, Result<Vec<daw_core::audio::balance::BalanceResult>, String>)>>>,
    balance_running: bool,
}

impl DawView {
    fn new(mut host: Host, project: Project, path: Option<PathBuf>, sf2_path: String, cx: &mut Context<Self>) -> Self {
        host.enable_keylab_controls();
        let live_status = host.session.as_ref().map(|session| session.status_line());
        let mut view = Self {
            st: HeadlessState::from_project(project, path),
            host,
            playing: false,
            last_session_poll: std::time::Instant::now(),
            live_status,
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
            browser_scroll: gpui::ScrollHandle::new(),
            fx_library: fx_library::EffectLibrary::new(cx),
            fx_controls_group: None,
            fx_slot_details: None,
            output_analyzer: Default::default(),
            recorder: daw_core::sequencer::MidiRecorder::new(),
            midi_recording: false,
            midi_track: None,
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
            confirm_then: ConfirmThen::Close,
            engine_error_shown: None,
            jam_reported: None,
            rec_track: None,
            armed_track: None,
            armed_input: None,
            capture_guitar: false,
            audio_take_active: false,
            audio_take_finishing: false,
            input_track: None,
            playhead_was_visible: true,
            timeline_scroll_grab: None,
            volume_automation_open: false,
            comment_menu: None,
            comment_draft: None,
            comment_focus: cx.focus_handle(),
            ir_pick: Default::default(),
            cabinet_export_result: Default::default(),
            guitar_window: None,
            amp_readout_shown: None,
            ampforge_save_at: None,
            dirty_shown: Default::default(),
            piano_scroll: gpui::ScrollHandle::new(),
            piano_scroll_grab: None,
            piano_scrolled_pattern: None,
            piano_step_input: None,
            piano_note_lights: [0; 2],
            mix_scroll: gpui::ScrollHandle::new(),
            mix_vertical_scroll: gpui::ScrollHandle::new(),
            undo: daw_core::undo::UndoStack::new(),
            root_focus: cx.focus_handle(),
            rename_focus: cx.focus_handle(),
            balance_result: Default::default(),
            balance_running: false,
            clap_scan: Default::default(),
            clap_scanning: Default::default(),
            plugin_import: None,
        };
        view.sync_midi_strip();
        view
    }

    fn timeline_width(&self, window: &Window) -> f32 {
        (f32::from(window.viewport_size().width) - TRACK_HEADER_W).max(1.)
    }

    fn workspace_splitter(
        &self,
        panel: Splitter,
        height: f32,
        resizable: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme();
        let id = match panel {
            Splitter::Dock => "rz-dock",
            Splitter::Mixer => "rz-mixer",
        };
        let splitter = div()
            .id(id)
            .debug_selector(move || id.into())
            .h(px(layout::SPLITTER_H))
            .flex_shrink_0()
            .bg(rgb(theme.panel_alt));
        if !resizable {
            return splitter;
        }
        let accent = self.accent();
        splitter
            .cursor(gpui::CursorStyle::ResizeUpDown)
            .hover(move |s| s.bg(rgb(accent)))
            .on_drag(
                ResizeDrag { panel, height, origin: std::cell::Cell::new(None) },
                |drag, _, window, cx| {
                    drag.origin.set(Some(window.mouse_position().y));
                    cx.new(|_| EmptyDrag)
                },
            )
            .on_drag_move(cx.listener(move |this, e: &DragMoveEvent<ResizeDrag>, _, cx| {
                let drag = e.drag(cx);
                if drag.panel != panel { return; }
                if let Some(origin) = drag.origin.get() {
                    let height = (drag.height + f32::from(origin - e.event.position.y)).max(0.);
                    match panel {
                        Splitter::Dock => this.st.dock_h = height,
                        Splitter::Mixer => this.st.mixer_h = height,
                    }
                    cx.notify();
                }
            }))
    }

    /// Current accent color (Appearance setting).
    fn accent(&self) -> u32 {
        let accents = self.theme().accents;
        accents[self.st.accent_idx % accents.len()]
    }

    fn theme(&self) -> Theme {
        if self.config.light_theme { Theme::LIGHT } else { Theme::DARK }
    }

    fn send_command(&self, command: SequencerCommand) -> Result<(), String> {
        if let Some(session) = &self.host.session {
            if daw_io::session::is_project_edit(&command) {
                session.submit(&self.st.project).map_err(|e| e.to_string())?;
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
                    self.st.reconcile_selection();
                    self.sync_midi_strip();
                    self.st.peaks_cache.clear();
                    self.st.peaks_dur_cache.clear();
                    self.waveform_failed.clear();
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

    /// Move the playhead to an absolute timeline tick.
    fn seek_to_tick(&mut self, tick: u32) {
        if let Some(session) = &self.host.session {
            if let Err(e) = session.transport(daw_core::session::Command::Seek { tick }) { self.st.set_status(e.to_string()); }
        } else { let _ = self.send_command(SequencerCommand::Seek(tick)); }
    }

    /// Nudge the tempo and push it to the engine.
    fn nudge_bpm(&mut self, delta: f64) {
        self.undo.record("BPM", &self.st.project);
        let bpm = (self.st.project.project.bpm + delta).clamp(daw_core::musical_time::MIN_BPM, daw_core::musical_time::MAX_BPM);
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
        self.validate_track_monitor();
        if let Some(session) = &self.host.session {
            // A rejected submit leaves the local project differing from the
            // session; the session poll in `pump` reports it every 50 ms.
            let _ = session.submit(&self.st.project);
        } else {
            let _ = self.send_command(SequencerCommand::UpdateLive(self.st.project.clone()));
        }
        // A structural edit rebuilds the strips from the saved mute flags;
        // re-apply the local solo mask so a lit S keeps meaning what it shows.
        if !self.st.soloed.is_empty() {
            for t in &self.st.project.tracks { self.push_mute(&t.id); }
        }
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
            self.validate_track_monitor();
            self.st.reconcile_selection();
            self.sync_midi_strip();
            self.st.set_status(match result { Ok(()) => "undo: shared session".into(), Err(e) => e.to_string() });
            return;
        }
        match self.undo.undo(self.st.project.clone()) {
            Some((label, snap)) => {
                self.st.project = snap;
                self.st.reconcile_selection();
                self.sync_midi_strip();
                self.push_live();
                self.st.set_status(format!("undid: {label}"));
            }
            None => self.st.set_status("nothing to undo"),
        }
    }

    fn redo(&mut self) {
        if let Some(session) = &self.host.session {
            let result = session.history(true, &mut self.st.project);
            self.validate_track_monitor();
            self.st.reconcile_selection();
            self.sync_midi_strip();
            self.st.set_status(match result { Ok(()) => "redo: shared session".into(), Err(e) => e.to_string() });
            return;
        }
        match self.undo.redo(self.st.project.clone()) {
            Some((label, snap)) => {
                self.st.project = snap;
                self.st.reconcile_selection();
                self.sync_midi_strip();
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
        self.host.disable_track_monitor();
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
        let before = self.st.project.clone();
        if self.st.project.remove_track(id) {
            self.undo.record("remove track", &before);
            self.st.soloed.remove(id);
            // Fix selection if we removed the selected track.
            if self.st.selected_track.as_deref() == Some(id) {
                let first = self.st.project.tracks.first().map(|t| t.id.clone());
                match first {
                    Some(fid) => self.st.select_track(&fid),
                    None => self.st.selected_track = None,
                }
            }
            self.sync_midi_strip();
            self.push_live();
        }
    }

    fn cycle_track_type(&mut self, id: &str) {
        self.edit("track type", |p| { p.cycle_track_type(id); });
        self.sync_midi_strip();
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
        let theme = self.theme();
        let buf = self.st.rename_buf.clone();
        div()
            .id("rename-input")
            .key_context("RenameInput")
            .track_focus(&self.rename_focus)
            .px_1()
            .bg(rgb(theme.bg))
            .border_1()
            .border_color(rgb(self.accent()))
            .rounded_sm()
            .text_color(rgb(theme.text))
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

    fn add_clap_fx(&mut self, desc_path: String, desc_id: String) -> bool {
        let Some(tid) = self.st.selected_track.clone() else {
            self.st.set_status("select a track first".to_string());
            return false;
        };
        self.undo.record("add clap fx", &self.st.project);
        if self.st.project.add_clap_fx(&tid, &desc_path, &desc_id).is_some() {
            self.push_live();
            self.st.set_status(format!("+ {desc_id}"));
            return true;
        }
        false
    }

    fn set_clap_instrument(&mut self, desc_path: String, desc_id: String) {
        let Some(tid) = self.st.selected_track.clone() else {
            self.st.set_status("select a track first".to_string());
            return;
        };
        if !self.st.confirm_clap_instrument(&tid, &desc_path, &desc_id, std::time::Instant::now()) {
            return;
        }
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

    fn open_flow_editor(&mut self, uid: &str, path: &str, id: &str) {
        let imported = id.starts_with(daw_core::audio::flow::ID_PREFIX);
        let launch = || -> anyhow::Result<()> {
            let session = self.host.session.as_ref()
                .ok_or_else(|| anyhow::anyhow!("The plugin editor needs a running session"))?;
            let path = session.project_directory().join(path).canonicalize()?;
            let path = path.to_str().ok_or_else(|| anyhow::anyhow!("invalid plugin path"))?;
            anyhow::ensure!(daw_core::audio::clap::is_available_plugin_path(path),
                "The plugin is outside the installed plugin directories");
            let plugin = std::path::Path::new(path);
            anyhow::ensure!(imported || (plugin.with_extension("editor.html").is_file()
                && plugin.with_extension("editor.json").is_file()),
                "Model-M editor assets are missing. Rebuild the local CLAP bundle.");
            let mut command = std::process::Command::new("python3");
            command
                .args(["-c", include_str!("../../../plugins/flow-model-m/editor.py")])
                .arg("--session").arg(&session.socket)
                .args(["--slot", uid, "--plugin", path, "--open", "--parent"])
                .arg(std::process::id().to_string());
            if imported {
                command.args(["--flow-id", id, "--javascript", include_str!("app/flow_editor.js")]);
            } else {
                command.args(["--javascript", include_str!("../../../plugins/flow-model-m/editor.js")]);
            }
            let mut child = command.spawn()?;
            std::thread::spawn(move || { let _ = child.wait(); });
            Ok(())
        };
        match launch() {
            Ok(()) => self.st.set_status("Opening the plugin editor in a new browser window."),
            Err(error) => self.st.set_status(format!("Plugin editor: {error}")),
        }
    }

    /// One CLAP slot section for the FX dock: header (name + remove) and
    /// generic param stepper rows from the relay-shipped metadata.
    fn clap_slot_section(
        &self,
        slot: &daw_core::project::ClapSlot,
        removable: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = self.theme();
        let uid = slot.uid.clone();
        let metadata = self.st.clap_meta.get(&uid);
        let name = metadata.map(|(name, _)| name.as_str())
            .or_else(|| self.st.clap_plugins.iter()
                .find(|plugin| plugin.id == slot.id && plugin.path == slot.path)
                .map(|plugin| plugin.name.as_str()))
            .unwrap_or(&slot.id);
        let params = metadata.map_or(&[][..], |(_, params)| params.as_slice());
        let expanded = self.fx_slot_details.as_ref() == Some(&uid);

        let mut header = div()
            .flex()
            .items_center()
            .gap_1()
            .children((!params.is_empty()).then(|| {
                let uid = uid.clone();
                theme.cell_btn_with_help(format!("fx-details-{uid}"), if expanded { "▾" } else { "▸" },
                    expanded, self.accent(), "Plugin parameters".into())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.fx_slot_details = (!expanded).then(|| uid.clone());
                        cx.notify();
                    }))
            }))
            .child(div().flex_1().min_w_0().truncate().text_color(rgb(theme.text)).child(name.to_owned()));
        if slot.id == model_m_midi::PLUGIN_ID || slot.id.starts_with(daw_core::audio::flow::ID_PREFIX) {
            let editor_uid = uid.clone();
            let editor_path = slot.path.clone();
            let editor_id = slot.id.clone();
            header = header.child(
                theme.cell_btn(format!("editor-{uid}"), "Open editor", false, theme.panel_alt)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_flow_editor(&editor_uid, &editor_path, &editor_id);
                        cx.notify();
                    })),
            );
        }
        if removable {
            let uid_x = uid.clone();
            header = header.child(
                theme.cell_btn(format!("clapx-{uid}"), "✕", false, theme.panel_alt).on_click(cx.listener(
                    move |this, _e, _w, cx| {
                        this.remove_clap_fx(&uid_x);
                        cx.notify();
                    },
                )),
            );
        }

        let rows = params.iter().filter(|_| expanded).map(|info| {
            let cur = slot
                .params
                .get(&info.id.to_string())
                .copied()
                .unwrap_or(info.default);
            let stepped=info.stepped;
            let step=if stepped {((info.max-info.min)/20.).round().max(1.)} else {((info.max-info.min)/20.).max(1e-6)};
            let (uid_d, uid_i, uid_r) = (uid.clone(), uid.clone(), uid.clone());
            let (lo, hi, id, default) = (info.min, info.max, info.id, info.default);
            if info.readonly {
                return div().flex().gap_2().h(px(26.)).flex_shrink_0().text_color(rgb(theme.text_dim)).child(info.name.clone()).child("Read only");
            }
            div()
                .flex()
                .items_center()
                .gap_2()
                .h(px(26.)).flex_shrink_0()
                .child(div().w(px(72.)).flex_shrink_0().whitespace_nowrap().overflow_hidden().text_color(rgb(theme.text_dim)).child(info.name.clone()))
                .child(theme.cell_btn(format!("cp-{uid}-{id}"), "−", false, theme.panel_alt).on_click(cx.listener(move |this, e: &gpui::ClickEvent, _w, cx| {
                    let d=if e.modifiers().shift && !stepped {step/10.} else {step};
                    let value=if stepped {(cur-d).round()} else {cur-d};
                    this.nudge_clap_param(uid_d.clone(), id, value.clamp(lo, hi));
                    cx.notify();
                })))
                .child(div().w(px(64.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(theme.text)).child(format!("{cur:.2}")))
                .child(theme.cell_btn(format!("cp+{uid}-{id}"), "+", false, theme.panel_alt).on_click(cx.listener(move |this, e: &gpui::ClickEvent, _w, cx| {
                    let d=if e.modifiers().shift && !stepped {step/10.} else {step};
                    let value=if stepped {(cur+d).round()} else {cur+d};
                    this.nudge_clap_param(uid_i.clone(), id, value.clamp(lo, hi));
                    cx.notify();
                })))
                .child(theme.cell_btn(format!("cp-reset-{uid}-{id}"),"Reset",false,theme.panel_alt).on_click(cx.listener(move |this,_,_,cx| {
                    this.nudge_clap_param(uid_r.clone(), id, default); cx.notify();
                })))
        });

        div().flex().flex_col().gap_1().p_1().rounded_md().bg(rgb(theme.bg)).child(header).children(rows)
    }

    /// Clickable chips for the selected track's patterns; highlights the open one.
    fn pattern_picker(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
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
                .bg(rgb(if is_sel { theme.panel_alt } else { theme.panel }))
                .text_color(rgb(if is_sel { theme.text } else { theme.text_dim }))
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
            .overflow_x_scroll().flex().gap_1().px_2().py_1().bg(rgb(theme.bg))
            .border_b_1().border_color(rgb(theme.border)).children(editor).children(chips))
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

    /// `is_dirty` for the title pill, refreshed at most four times a second.
    /// The exact check serializes and hashes the whole project, which is too
    /// much to do on every painted frame of a large song.
    fn is_dirty_for_display(&self) -> bool {
        let (at, dirty) = self.dirty_shown.get();
        if at.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(250)) { return dirty; }
        let dirty = self.is_dirty();
        self.dirty_shown.set((Some(std::time::Instant::now()), dirty));
        dirty
    }

    fn is_dirty(&self) -> bool {
        daw_core::project::format::project_revision(&self.st.project).map_or(true, |hash| hash != self.st.saved_hash)
    }

    fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let _ = self.save_ampforge_state();
        if self.st.renaming.is_some() { self.commit_rename(); }
        if self.midi_recording || self.audio_rec.phase != daw_core::audio::input::RecordPhase::Idle {
            self.stop();
        }
        if !self.is_dirty() { return true; }
        self.confirm_then = ConfirmThen::Close;
        self.close_confirmation = true;
        self.st.open_menu = None;
        self.root_focus.focus(window);
        cx.notify();
        false
    }

    /// New / Open replace the whole project: ask first when there is unsaved work.
    pub(crate) fn request_replace(&mut self, then: ConfirmThen, window: &mut Window) {
        if self.st.renaming.is_some() { self.commit_rename(); }
        if !self.is_dirty() {
            self.replace_project(then);
            return;
        }
        self.confirm_then = then;
        self.close_confirmation = true;
        self.root_focus.focus(window);
    }

    /// Carry out a confirmed New / Open.
    pub(crate) fn replace_project(&mut self, then: ConfirmThen) {
        match then {
            ConfirmThen::New => self.new_project(),
            ConfirmThen::Open => self.open_dialog(),
            ConfirmThen::Close => {}
        }
    }

    fn new_project(&mut self) {
        self.comment_menu = None;
        self.comment_draft = None;
        self.host.disable_track_monitor();
        if self.playing || self.midi_recording { self.stop(); }
        self.undo.record("new project", &self.st.project);
        self.st.project = daw_core::project::Project::default_project();
        self.midi_track = None;
        self.st.project_path = None;
        self.st.file_revision = None;
        // An untouched new project is not "unsaved work".
        self.st.saved_hash = daw_core::project::format::project_revision(&self.st.project).unwrap_or_default();
        self.st.project_mtime = None;
        self.st.soloed.clear();
        self.st.track_heights.clear();
        self.st.selected_return = None;
        self.st.arrange_scroll_x = 0.;
        self.st.arrange_scroll_y = 0.;
        self.piano_scrolled_pattern = None;
        self.piano_step_input = None;
        self.st.peaks_cache.clear();
        self.st.peaks_dur_cache.clear();
        self.waveform_failed.clear();
        self.jam_reported = None;
        if let Some(id) = self.st.project.tracks.first().map(|t| t.id.clone()) {
            self.st.select_track(&id);
        }
        self.sync_midi_strip();
        self.push_live();
        self.st.set_status("new project — Save As to name it");
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
                self.comment_menu = None;
                self.comment_draft = None;
                self.host.disable_track_monitor();
                // A loaded file is a new document with its own history. Keeping
                // the old steps let Ctrl+Z put the previous song under the new
                // file's path, where Ctrl+S would overwrite the wrong file.
                self.undo = daw_core::undo::UndoStack::new();
                if let Some(session) = &self.host.session {
                    if let Err(e) = session.import(&project, &path) { self.st.set_status(e.to_string()); return; }
                    if let Err(e) = session.transport(daw_core::session::Command::Stop) { self.st.set_status(e.to_string()); return; }
                } else {
                    let _ = self.send_command(SequencerCommand::LoadProject(project.clone()));
                }
                self.playing = false;
                self.piano_scrolled_pattern = None;
                self.piano_step_input = None;
                // Keep the musician's workspace (layout, zoom, JAM, accent).
                let old = std::mem::replace(&mut self.st, HeadlessState::from_project(project, Some(path.clone())));
                self.st.mixer_h = old.mixer_h; self.st.dock_h = old.dock_h;
                self.st.mixer_open = old.mixer_open;
                self.st.arrange_zoom = old.arrange_zoom; self.st.arrange_full = old.arrange_full;
                self.st.accent_idx = old.accent_idx; self.st.jam_mode = old.jam_mode;
                self.st.active_dock = old.active_dock; self.st.browser_category = old.browser_category;
                self.st.clap_plugins = old.clap_plugins;
                self.midi_track = None;
                self.jam_reported = None;
                self.sync_midi_strip();
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
        let mut changed = self.pump_ampforge();
        if let Some(session) = &self.host.session {
            let playing = session.playing();
            if self.last_session_poll.elapsed() >= std::time::Duration::from_millis(50) {
            self.last_session_poll = std::time::Instant::now();
            let live_status = Some(session.status_line());
            changed |= self.live_status != live_status;
            self.live_status = live_status;
            match session.sync(&mut self.st.project) {
                Ok(updated) => {
                    changed |= updated;
                    // An agent's edit must not yank the musician's selection
                    // back to the first track; only repair what vanished.
                    if updated { self.validate_track_monitor(); self.st.reconcile_selection(); self.sync_midi_strip(); }
                }
                Err(e) => {
                    let text = format!("{e} — press Sync session to accept the shared version");
                    changed |= self.st.status != text; self.st.set_status(text);
                }
            }
            }
            changed |= self.playing != playing;
            self.playing = playing;
        }
        // Engine failures are shown beside the status line (see `render`),
        // not written over it, so later feedback (saved, exported…) stays visible.
        let engine_error = self.host.engine_error.lock().ok().and_then(|e| e.clone());
        if engine_error != self.engine_error_shown {
            self.engine_error_shown = engine_error;
            changed = true;
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
            match daw_core::audio::wav_duration_sec(&path) {
                Ok(dur) => {
                    // Only now touch the project: an unreadable file must not
                    // leave an empty track and an undo step behind.
                    self.undo.record("import sample", &self.st.project);
                    let tid = if is_audio {
                        self.st.selected_track.clone().unwrap()
                    } else {
                        let id = self.st.project.add_track(daw_core::project::TrackType::Audio);
                        self.st.select_track(&id);
                        id
                    };
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
        if !self.clap_scanning.load(Ordering::Acquire) {
            if let Some(found) = self.clap_scan.lock().ok().and_then(|mut s| s.take()) {
                if !found.is_empty() {
                    self.st.set_status(format!("{} plugins found", found.len()));
                }
                self.st.clap_plugins = found;
                changed = true;
            }
            changed |= self.pump_plugin_import();
        }
        while let Ok((uid, name, params)) = self.host.clap_meta_rx.try_recv() {
            self.st.clap_meta.insert(uid, (name, params));
            changed = true;
        }

        // 1. MIDI input → preview the selected track / route CC.
        let mut model_m_updates = std::collections::BTreeMap::new();
        while let Ok(ev) = self.host.midi_ev_rx.try_recv() {
            changed = true;
            match ev {
                daw_io::midi::MidiEvent::NoteOn { pitch, velocity } => {
                    // The MIDI input callback has already sent the note to the selected strip.
                    if self.midi_recording {
                        if let Some((_, len)) = self.open_pr_pattern() {
                            let ph = self.st.playhead();
                            self.recorder.note_on(pitch, velocity, ph, len);
                        }
                    }
                }
                daw_io::midi::MidiEvent::NoteOff { pitch } => {
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
                    if model_m_midi::Control::midi_cc(cc, value).is_some_and(
                        |control| self.collect_model_m_control(control, &mut model_m_updates)
                    ) { continue; }
                    let ch = self.host.midi_channel.load(std::sync::atomic::Ordering::Relaxed);
                    let _ = self.send_command(SequencerCommand::RawCC { channel: ch, cc, value });
                }
                daw_io::midi::MidiEvent::KeyLab(control) => {
                    if let Some(control) = model_m_midi::Control::keylab(control) {
                        self.collect_model_m_control(control, &mut model_m_updates);
                    }
                }
            }
        }
        self.apply_model_m_controls(model_m_updates);
        if self.st.active_dock == DockTab::PianoRoll && !self.st.arrange_full {
            let pressed = self.piano_pressed_keys();
            changed |= self.piano_note_lights != pressed;
            self.piano_note_lights = pressed;
        }

        // 2. JAM hot-reload: reload the project file when its mtime changes.
        if self.st.jam_mode {
            if let Some(path) = self.st.project_path.clone() {
                let mtime = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok());
                if mtime != self.st.project_mtime {
                    let first_observation = self.st.project_mtime.is_none();
                    if first_observation { self.st.project_mtime = mtime; }
                    if !first_observation {
                        let dirty = daw_core::project::format::project_revision(&self.st.project).ok().as_deref() != Some(self.st.saved_hash.as_str());
                        if dirty {
                            // Report once per external write and keep polling the
                            // rest of the app (export results, recording).
                            if self.jam_reported != Some((mtime, JamIssue::Conflict)) {
                                self.jam_reported = Some((mtime, JamIssue::Conflict));
                                self.st.set_status("JAM conflict: the file changed on disk while you have unsaved edits — Save As to keep yours, or undo them to load the new version");
                                changed = true;
                            }
                        } else if self.jam_reported != Some((mtime, JamIssue::Unreadable)) {
                            match daw_core::project::format::load_versioned(&path) {
                                Ok((p, revision)) => {
                                    self.st.project_mtime = mtime;
                                    self.jam_reported = None;
                                    self.st.file_revision = Some(revision);
                                    self.st.saved_hash = daw_core::project::format::project_revision(&p).unwrap_or_default();
                                    // External edits (LLM/scripts) are undoable in-app.
                                    self.undo.record("JAM reload", &self.st.project);
                                    self.st.project = p;
                                    self.st.reconcile_selection();
                                    self.sync_midi_strip();
                                    self.push_live();
                                    self.st.set_status(format!("JAM reloaded {}", path.display()));
                                }
                                // A half-written or invalid file: say why once and
                                // keep the sounding project; the next write retries.
                                Err(e) => {
                                    self.jam_reported = Some((mtime, JamIssue::Unreadable));
                                    self.st.set_status(format!("JAM: file not loaded, keeping current project — {e}"));
                                }
                            }
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

        // 5. Guitar amp: a picked cabinet IR; live latency readout while on.
        changed |= self.poll_cabinet_ir();
        if let Some(message) = self.cabinet_export_result.lock().ok().and_then(|mut slot| slot.take()) {
            self.st.set_status(message);
            changed = true;
        }
        if let Some(readout) = self.monitor_readout() {
            if self.amp_readout_shown.as_deref() != Some(readout.as_str()) || self.st.open_menu.is_some() {
                self.amp_readout_shown = Some(readout);
                changed = true;
            }
        } else if self.amp_readout_shown.take().is_some() { changed = true; }

        changed
    }

    /// Transport master-bus meter (post-limiter, from the engine mixer).
    fn viz_meter(&self) -> gpui::Div {
        let theme = self.theme();
        let accent = self.accent();
        let peak = self.host.master_meter.read().0.min(1.0);
        let bars = 12usize;
        let lit = (peak * bars as f32).round() as usize;
        let cells = (0..bars).map(move |i| {
            let on = i < lit;
            let color = if i >= bars - 2 { RECORD } else if i >= bars - 5 { METER_WARN } else { accent };
            div().w(px(4.)).h(px(12.)).rounded_sm()
                .bg(rgb(if on { color } else { theme.panel_alt }))
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

/// Why JAM did not load an external write of the project file.
#[derive(Clone, Copy, PartialEq, Eq)]
enum JamIssue { Conflict, Unreadable }

/// The action waiting behind the unsaved-changes dialog.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ConfirmThen { Close, New, Open }

/// Top menu-bar dropdown identity.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TopMenu { File, Settings, AgentSession, Microphone, Midi }

/// Identify the splitter being dragged, since both receive drag-move events.
#[derive(Clone, Copy, PartialEq)]
enum Splitter { Dock, Mixer }

struct ResizeDrag {
    panel: Splitter,
    height: f32,
    origin: std::cell::Cell<Option<gpui::Pixels>>,
}

/// Mark rects (x, y, w, h in px) previewing `pattern` inside a clip that is
/// `clip_w`×`clip_h` px and covers `span_ticks` of timeline; the pattern
/// loops at its natural period. Capped at 256 marks.
#[cfg(test)]
fn clip_preview_marks(
    pattern: &daw_core::project::Pattern,
    clip_w: f32,
    clip_h: f32,
    span_ticks: u32,
) -> Vec<(f32, f32, f32, f32)> {
    clip_preview_marks_in(pattern, clip_w, clip_h, span_ticks, 0.0, clip_w)
}

/// As `clip_preview_marks`, keeping only marks that overlap the clip-local
/// x-range `[vis_lo, vis_hi]` (the part of the clip on screen). A song-long
/// clip then draws the notes in view instead of spending its 256-mark budget
/// on bars scrolled far off screen.
fn clip_preview_marks_in(
    pattern: &daw_core::project::Pattern,
    clip_w: f32,
    clip_h: f32,
    span_ticks: u32,
    vis_lo: f32,
    vis_hi: f32,
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
                let Some(lo) = pr.active_notes().map(|n| n.pitch).min() else {
                    break;
                };
                let lo = f32::from(lo);
                let hi = pr.active_notes().map(|n| f32::from(n.pitch)).fold(lo, f32::max);
                let range = (hi - lo).max(1.0);
                let mark_h = 2.0f32;
                let usable = (clip_h - mark_h - 2.0).max(1.0);
                for n in pr.active_notes() {
                    if out.len() >= CAP {
                        break 'reps;
                    }
                    let x = (rep_start + n.tick) as f32 * ppt;
                    if x >= clip_w {
                        continue;
                    }
                    let w = (n.duration as f32 * ppt).max(1.5).min(clip_w - x);
                    if x + w < vis_lo || x > vis_hi { continue; }
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
                        if x + w < vis_lo || x > vis_hi { continue; }
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

fn scrollbar_thumb(width: f32, max_scroll: f32) -> f32 {
    (width * width / (width + max_scroll)).clamp(24_f32.min(width), width)
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
    theme: Theme,
    text: SharedString,
}
impl Render for TextTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(theme.panel))
            .border_1()
            .border_color(rgb(theme.border))
            .text_color(rgb(theme.text))
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
            .text_color(rgb(ON_CLIP))
            .whitespace_nowrap()
            .child(self.name.clone())
    }
}

fn header_group() -> gpui::Div {
    div().flex().flex_shrink_0().items_center().gap_1()
}

impl Theme {
    fn header_divider(self) -> gpui::Div {
        div().w(px(1.)).h(px(15.)).flex_shrink_0().bg(rgb(self.border))
    }
}

impl Render for DawView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme();
        // `render` is side-effect free: engine polling + the playhead mirror run on
        // the repaint timer (see `run`), which only re-renders when playing or when
        // something changed — so an idle window doesn't rebuild the UI 60×/second.

        // Window width — a conservative bound for timeline virtualization.
        let vis_w = f32::from(window.viewport_size().width);
        let content_h = window_frame::content_height(window);
        let layout = layout::PanelLayout::new(content_h, &self.st, self.sf2_missing);
        let timeline_width = self.timeline_width(window);
        let max_x = self.timeline_max_scroll(timeline_width);
        self.st.arrange_scroll_x = self.st.arrange_scroll_x.clamp(0., max_x);
        // Page-follow: when playback carries the playhead out of the visible
        // timeline, bring it back one bar from the left edge. A playhead the
        // musician scrolled away from is left alone until it re-enters view.
        {
            let bw = self.bar_w();
            let x = self.st.playhead() as f32 / self.st.project.ticks_per_bar() as f32 * bw;
            let mut visible = x >= self.st.arrange_scroll_x && x <= self.st.arrange_scroll_x + timeline_width;
            if self.playing && !visible && self.playhead_was_visible {
                self.st.arrange_scroll_x = (x - bw).clamp(0., max_x);
                visible = true;
            }
            self.playhead_was_visible = visible;
        }
        // Re-clamp vertical track scroll so a window resize, dock drag, or removed
        // track can never leave the view scrolled past the last track.
        let max_scroll_y = self.tracks_max_scroll_y(content_h);
        if self.st.arrange_scroll_y > max_scroll_y {
            self.st.arrange_scroll_y = max_scroll_y;
        }
        let accent = self.accent();
        let playing = self.playing;
        let recording = self.midi_recording;
        let bpm = self.st.bpm();
        let tick = self.st.playhead();
        let tpb = self.st.project.ticks_per_bar();
        let tp_beat = self.st.project.ticks_per_meter_beat();
        let bar = tick / tpb + 1;
        let beat = (tick % tpb) / tp_beat + 1;
        let live_status = self.live_status.clone();
        let engine_error = self.engine_error_shown.clone();
        let status_text = if self.st.status.is_empty() {
            live_status.unwrap_or_else(|| "Select a clip to edit · Wheel: tracks · Shift+wheel: timeline · Ctrl+wheel: zoom".to_string())
        } else {
            live_status.map(|s| format!("{} · {s}", self.st.status)).unwrap_or_else(|| self.st.status.clone())
        };
        // The live status line already carries the engine failure.
        let status_text = match engine_error {
            Some(error) if self.host.session.is_none() => format!("⚠ {error} · {status_text}"),
            _ => status_text,
        };

        let playback = header_group()
            // Play / Pause toggle.
            .child(
                div()
                    .id("transport-play").flex_shrink_0()
                    .px_3()
                    .py_1()
                    .rounded_sm()
                    .debug_selector(|| "transport-play".into())
                    .cursor_pointer()
                    .bg(rgb(accent))
                    .text_color(rgb(theme::contrast_text(accent)))
                    .active(|s| s.opacity(0.8))
                    .child(if playing { "⏸ Pause" } else { "▶ Play" })
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.toggle_play();
                        cx.notify();
                    })),
            )
            .child(pill("■ Stop", theme.panel_alt, theme.text).id("transport-stop").debug_selector(|| "transport-stop".into()).cursor_pointer().on_click(
                cx.listener(|this, _ev, _window, cx| {
                    this.stop();
                    cx.notify();
                }),
            ))
            // Record: audio takes on an audio track, MIDI record otherwise.
            .child({
                use daw_core::audio::input::RecordPhase;
                let audio_sel = self.selected_track_is_audio();
                let (label, bg, fg) = if self.audio_take_active && self.audio_rec.phase != RecordPhase::AwaitingRoll {
                    ("⏺ REC", RECORD, ON_CLIP)
                } else if audio_sel {
                    match self.audio_rec.phase {
                        RecordPhase::Idle => ("⏺ Rec", theme.panel_alt, theme.text),
                        RecordPhase::Armed => ("⏺ Armed", METER_WARN, ON_CLIP),
                        RecordPhase::AwaitingRoll => ("⏺ …", RECORD, ON_CLIP),
                        RecordPhase::Recording => ("⏺ REC", RECORD, ON_CLIP),
                    }
                } else {
                    ("⏺ Rec", if recording { RECORD } else { theme.panel_alt },
                     if recording { ON_CLIP } else { theme.record_text })
                };
                pill(label, bg, fg)
                    .id("transport-rec")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.toggle_record();
                        cx.notify();
                    }))
            })
            .child(
                div()
                    .debug_selector(|| "transport-position".into())
                    .flex()
                    .flex_col()
                    .justify_center()
                    .flex_shrink_0()
                    .min_w(px(110.))
                    .px_2()
                    .rounded_sm()
                    .bg(rgb(theme.bg))
                    .child(div().font_family("monospace").text_size(px(15.)).line_height(px(18.)).text_color(rgb(theme.text))
                        .child(transport::playhead_time_with_map(tick, bpm, &self.st.project.tempo_changes)))
                    .child(div().font_family("monospace").text_size(px(10.)).line_height(px(12.)).text_color(rgb(theme.text_dim))
                        .child(format!("{bar}.{beat}  ·  tick {tick}"))),
            );
        let tempo = header_group()
            .child(theme.cell_btn("bpm-down", "−", false, theme.panel_alt)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.nudge_bpm(-1.0);
                    cx.notify();
                })))
            .child(div().min_w(px(59.)).font_family("monospace").text_center()
                .text_color(rgb(theme.text)).child(format!("{bpm:.0} BPM")))
            .child(theme.cell_btn("bpm-up", "+", false, theme.panel_alt)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.nudge_bpm(1.0);
                    cx.notify();
                })));
        let workspace = header_group()
            .child(theme.cell_btn("jam", "JAM", self.st.jam_mode, accent)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.st.toggle_jam();
                    cx.notify();
                })))
            .child(theme.cell_btn_with_help(
                "toggle-mixer", "Mixer", self.st.mixer_open && !self.st.arrange_full, accent,
                "Show or hide the mixer below the arrangement".into(),
            ).on_click(cx.listener(|this, _, _, cx| {
                this.st.mixer_open = this.st.arrange_full || !this.st.mixer_open;
                this.st.arrange_full = false;
                cx.notify();
            })))
            .child(theme.cell_btn_with_help(
                "toggle-arrange", "Arrange ⛶", self.st.arrange_full, accent,
                "Expand the arrangement or restore the lower panels".into(),
            ).on_click(cx.listener(|this, _, _, cx| {
                this.st.arrange_full = !this.st.arrange_full;
                cx.notify();
            })));

        let transport = div()
            .id("transport")
            .flex()
            .items_center()
            .h(px(TRANSPORT_H))
            .gap_2()
            .px_2()
            .flex_shrink_0()
            .bg(rgb(theme.panel))
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(rgb(theme.text))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(div().id("transport-scroll").flex_1().min_w_0().h_full()
                .overflow_x_scroll().whitespace_nowrap()
                .flex().items_center().gap_2()
                .child(playback)
                .child(theme.header_divider())
                .child(tempo)
                .child(self.cycle_controls(cx))
                .child(theme.header_divider())
                .child(self.key_controls(cx))
                .children(self.host.session.as_ref().map(|_| {
                    theme.cell_btn("sync-session", "Sync", false, theme.panel_alt)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.reconcile_session();
                            cx.notify();
                        }))
                })))
            .child(theme.header_divider())
            .child(workspace);

        let arrangement = div()
            .debug_selector(|| "arrangement".into())
            .flex_1()
            .flex()
            .min_h_0()
            .min_w_0()
            .child(self.track_headers(cx))
            .child(self.timeline(cx, timeline_width));
        let mixer = layout.mixer_height.map(|height| {
            div().h(px(height)).flex_shrink_0().overflow_hidden().flex().flex_col()
                .child(self.mixing_workspace(vis_w, cx))
        });
        let editor = layout.editor_height.map(|height| self.editor_dock(height, cx));
        let main_row = div().flex_1().min_h_0().min_w_0().flex().flex_col()
            .child(arrangement)
            .children(layout.mixer_height.map(|height| {
                self.workspace_splitter(Splitter::Mixer, height, layout.editor_height.is_some(), cx)
            }))
            .children(mixer)
            .children(layout.editor_height.map(|height| {
                self.workspace_splitter(Splitter::Dock, height, true, cx)
            }))
            .children(editor)
            .children((layout.mixer_height.is_some() && layout.editor_height.is_none())
                .then(|| self.editor_tabs(true, cx)));

        let toolbar = self.song_toolbar(cx);

        let project_name = self.project_name();
        let dirty = self.is_dirty_for_display();
        let status = div()
            .h(px(STATUS_H)).px_2().flex_shrink_0().flex().items_center().gap_2()
            .bg(rgb(theme.panel)).border_t_1().border_color(rgb(theme.border)).text_size(px(11.)).line_height(px(16.))
            .text_color(rgb(theme.text_dim))
            .child(div().id("status-project").max_w(px(200.)).min_w_0().truncate().text_color(rgb(theme.text))
                .child(if dirty { format!("{project_name} · unsaved") } else { project_name.to_string() })
                .tooltip(move |_, cx| cx.new(|_| TextTooltip { theme, text: project_name.clone() }).into()))
            .child(theme.header_divider())
            .child(div().flex_1().min_w_0().truncate().child(SharedString::from(status_text)))
            .child(header_group()
                .child(div().whitespace_nowrap().child(match &self.host.midi_device {
                    Some(device) => format!("MIDI · {}", trunc(device, 20)),
                    None => "MIDI · —".into(),
                }))
                .child(self.viz_meter()));

        let menu_bar = self.menu_bar(cx, vis_w);
        let instrument_browser = self.instrument_browser(cx, window);
        // Loud, actionable banner when the soundfont is absent (audio disabled).
        let sf2_banner = self.sf2_missing.then(|| {
            div()
                .h(px(SOUNDFONT_BANNER_H))
                .flex_shrink_0()
                .flex()
                .items_center()
                .px_3()
                .bg(rgb(RECORD))
                .text_color(rgb(ON_CLIP))
                .child("No soundfont — audio is disabled. Run  ./fetch-assets.sh  then restart.")
        });
        let overlay = self.menu_overlay(cx);
        let fx_overlay = self.audio_effects_overlay(cx);
        let close_overlay = self.close_overlay(cx);
        let input_overlay = self.track_input_overlay(cx);
        let comment_overlay = self.comment_overlay(window, cx);

        let piano_visible = layout.editor_height.is_some() && self.st.active_dock == DockTab::PianoRoll;
        let content = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(theme.bg))
            .track_focus(&self.root_focus)
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, w, _| {
                this.root_focus.focus(w);
            }))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                if !this.close_confirmation && (this.comment_menu.is_some() || this.comment_draft.is_some()) {
                    if this.comment_draft.is_some() {
                        this.comment_key(ev, w, cx);
                    } else if ev.keystroke.key == "escape" {
                        this.dismiss_comment(w);
                    }
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if this.input_track.is_some() {
                    if ev.keystroke.key == "escape" { this.input_track = None; cx.notify(); }
                    cx.stop_propagation();
                    return;
                }
                if this.close_confirmation {
                    if ev.keystroke.key == "escape" {
                        this.close_confirmation = false;
                        this.close_after_save = false;
                        this.confirm_then = ConfirmThen::Close;
                        cx.notify();
                    }
                    cx.stop_propagation();
                    return;
                }
                let command = ev.keystroke.modifiers.control || ev.keystroke.modifiers.platform;
                if command && ev.keystroke.key == "k" && this.st.open_menu.is_none() && !this.st.fx_window_open {
                    this.focus_instrument_browser(w);
                    cx.stop_propagation(); cx.notify(); return;
                }
                if command && ev.keystroke.key == "s" && this.st.open_menu.is_none() {
                    this.save_project(); cx.stop_propagation(); cx.notify(); return;
                }
                if ev.keystroke.key == "escape" && this.st.open_menu.is_none() && this.fx_library.open
                    && (this.st.fx_window_open || this.fx_library.focus.is_focused(w))
                {
                    this.close_effect_library(w);
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if ev.keystroke.key == "escape" && (this.st.open_menu.is_some() || this.st.fx_window_open) {
                    if this.st.open_menu.take().is_none() { this.st.fx_window_open = false; }
                    this.root_focus.focus(w);
                    cx.stop_propagation();
                    cx.notify();
                } else if ev.keystroke.key == "escape" && this.st.browser_open {
                    this.dismiss_instrument_browser(w);
                    cx.stop_propagation();
                    cx.notify();
                } else if this.st.open_menu.is_some() {
                    // Dropdowns own keyboard input; hidden editors and global
                    // transport/undo shortcuts must not act beneath them.
                    cx.stop_propagation();
                }
            }))
            .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, w, cx| {
                if this.search_focus.is_focused(w) || this.fx_library.focus.is_focused(w)
                    || this.rename_focus.is_focused(w) || this.focus.is_focused(w) { return; }
                if piano_visible && !this.st.browser_open && !this.st.fx_window_open
                    && this.handle_piano_step_key(ev)
                {
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                let m = &ev.keystroke.modifiers;
                if ev.keystroke.key == "space" && !m.control { this.toggle_play(); cx.notify(); return; }
                if m.control && ev.keystroke.key == "s" { this.save_project(); cx.notify(); return; }
                if !m.control {
                    match ev.keystroke.key.as_str() {
                        "home" => {
                            this.seek_to_tick(0);
                            this.scroll_timeline_to(0., w);
                            cx.notify();
                            return;
                        }
                        "end" => {
                            this.scroll_timeline_to(this.arrangement_end_offset(w), w);
                            cx.notify();
                            return;
                        }
                        _ => {}
                    }
                }
                if !m.control {
                    return;
                }
                match ev.keystroke.key.as_str() {
                    "n" => { this.request_replace(ConfirmThen::New, w); cx.notify(); }
                    "o" => { this.request_replace(ConfirmThen::Open, w); cx.notify(); }
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
            .child(main_row)
            .child(status)
            .child(instrument_browser)
            .children(fx_overlay)
            .children(overlay)
            .children(input_overlay)
            .children(comment_overlay)
            .children(close_overlay);
        window_frame::frame(theme, content, window, cx.listener(|this, _, w, cx| {
            if this.confirm_close(w, cx) { w.remove_window(); }
        }))
    }
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
                            if v.st.fx_window_open {
                                repaint |= v.output_analyzer.poll(&v.host);
                            }
                            let tick = v.host.playback_tick.load(Ordering::Relaxed);
                            if tick != last_tick {
                                v.st.set_playhead(tick);
                                last_tick = tick;
                                repaint = true;
                            }
                            // "Save & …" finished: close, or carry out New / Open.
                            let mut close = false;
                            if v.close_after_save && !v.is_dirty() {
                                v.close_after_save = false;
                                match std::mem::replace(&mut v.confirm_then, ConfirmThen::Close) {
                                    ConfirmThen::Close => close = true,
                                    then => {
                                        v.close_confirmation = false;
                                        v.replace_project(then);
                                        repaint = true;
                                    }
                                }
                            }
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
            let theme = Theme::DARK;
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
                        .child(theme.cell_btn("probe-fx", "+", false, theme.panel_alt).w(px(60.)).h(px(40.))
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
    fn instrument_browser_opens_from_header_without_shifting_the_timeline(cx: &mut gpui::TestAppContext) {
        let (host, commands, _audio) = Host::headless();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = DawView::new(host, Project::default_project(), None, String::new(), cx);
            view.sf2_missing = false;
            for (name, id) in [("Model-M Modular Synth", model_m_midi::PLUGIN_ID), ("Other Synth", "example.other")] {
                view.st.clap_plugins.push(daw_core::audio::clap::ClapPluginDesc {
                    path: "/tmp/daw-browser-test.clap".into(), id: id.into(), name: name.into(),
                    vendor: "Example".into(), is_instrument: true, is_effect: false,
                    features: Vec::new(),
                });
            }
            view
        });
        cx.simulate_resize(size(px(800.), px(480.)));
        cx.run_until_parked();
        let search = cx.debug_bounds("browser-search").expect("search is visible");
        assert_eq!(search.center().x, px(400.));
        assert!(cx.debug_bounds("instrument-results").is_none());
        let timeline_width = cx.update(|window, cx| view.read(cx).timeline_width(window));

        cx.simulate_mouse_move(search.center(), None, gpui::Modifiers::default());
        cx.run_until_parked();
        let results = cx.debug_bounds("instrument-results").expect("hover opens the library");
        assert!(results.bottom() <= px(480.));
        assert!(cx.debug_bounds("import-plugin").is_some());
        cx.simulate_mouse_move(results.center(), None, gpui::Modifiers::default());
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert!(view.st.browser_open));
        assert_eq!(cx.update(|window, cx| view.read(cx).timeline_width(window)), timeline_width);

        let outside = gpui::point(px(30.), px(280.));
        cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert!(!view.st.browser_open));
        cx.simulate_click(outside, gpui::Modifiers::default());
        cx.simulate_keystrokes("ctrl-k m o o g space m");
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert_eq!(view.st.browser_search, "moog m"));
        let model = cx.debug_bounds("clap-/tmp/daw-browser-test.clap-local.daw-ai.flow.model-m-v4").unwrap();
        let filtered = cx.debug_bounds("instrument-results").unwrap();
        assert!(filtered.bottom() - model.bottom() < px(18.), "only Model-M matches");
        cx.simulate_click(model.center(), gpui::Modifiers::default());
        view.read_with(cx, |view, _| {
            assert_eq!(view.st.pending_instrument.as_ref().map(|(_, _, id, _)| id.as_str()), Some(model_m_midi::PLUGIN_ID));
        });
        cx.simulate_click(model.center(), gpui::Modifiers::default());
        view.read_with(cx, |view, _| {
            assert!(matches!(&view.st.project.tracks[0].instrument,
                daw_core::project::Instrument::Clap(slot) if slot.id == model_m_midi::PLUGIN_ID));
        });
        assert!(!commands.try_iter().any(|cmd| matches!(cmd, SequencerCommand::Play)));
        cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
        view.read_with(cx, |view, _| assert!(view.st.browser_open));
        cx.simulate_keystrokes("escape");
        view.read_with(cx, |view, _| assert!(!view.st.browser_open));
        cx.simulate_keystrokes("space");
        assert!(commands.try_iter().any(|cmd| matches!(cmd, SequencerCommand::Play)));
        for width in [800., 1280., 1440.] {
            cx.simulate_resize(size(px(width), px(600.)));
            cx.run_until_parked();
            let search = cx.debug_bounds("browser-search").unwrap();
            let settings = cx.debug_bounds("menu-settings").unwrap();
            let effects = cx.debug_bounds("menu-audio-effects").unwrap();
            assert!(settings.right() < search.left() && search.right() < effects.left(),
                "header width {width}: settings {settings:?}, search {search:?}, effects {effects:?}");
        }
    }

    #[gpui::test]
    fn imported_instrument_is_available_without_replacing_the_track(cx: &mut gpui::TestAppContext) {
        let (host, _commands, _audio) = Host::headless();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = DawView::new(host, Project::default_project(), None, String::new(), cx);
            view.sf2_missing = false;
            view
        });
        let id = format!("{}test", daw_core::audio::flow::ID_PREFIX);
        let path = "/tmp/imported.fmx";
        let (send, receive) = std::sync::mpsc::channel();
        send.send(Some(Ok(daw_core::audio::clap::ClapPluginDesc {
            path: path.into(), id: id.clone(), name: "Imported Synth".into(),
            vendor: "Flow Music".into(), is_instrument: true, is_effect: false,
            features: Vec::new(),
        }))).unwrap();
        view.update(cx, |view, cx| {
            view.plugin_import = Some(receive);
            assert!(view.pump());
            assert!(matches!(view.st.project.tracks[0].instrument, daw_core::project::Instrument::Soundfont { .. }));
            assert_eq!(view.st.browser_search, "Imported Synth");
            cx.notify();
        });
        cx.update(|window, cx| view.read(cx).root_focus.focus(window));
        cx.simulate_keystrokes("ctrl-k");
        cx.run_until_parked();
        let row = cx.debug_bounds("clap-/tmp/imported.fmx-local.daw-ai.fmx.test").unwrap();
        cx.simulate_click(row.center(), gpui::Modifiers::default());
        cx.simulate_click(row.center(), gpui::Modifiers::default());
        view.read_with(cx, |view, _| {
            assert!(matches!(&view.st.project.tracks[0].instrument,
                daw_core::project::Instrument::Clap(slot) if slot.id == id && slot.path == path));
        });
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
            cc_events: Vec::new(), pitch_bends: Vec::new(), legato_overlap_ticks: 0, legato_max_gap_ticks: None, swing: None,
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
                    v.st.browser_open = true; v.st.mixer_h = 520.; v.st.dock_h = 1000.;
                    v.st.browser_search = "a".into();
                    v.st.open_menu = None; v.st.fx_window_open = false;
                    cx.notify();
                });
                cx.run_until_parked();
            }
            for menu in [TopMenu::File, TopMenu::Settings, TopMenu::AgentSession] {
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
        let mute = cx.debug_bounds("m-t2").expect("second track mute button");
        cx.simulate_click(mute.center(), gpui::Modifiers::default());
        view.read_with(cx, |v, _| { assert_eq!(v.st.selected_track, selected); assert!(v.st.project.tracks[1].muted, "mute button must still act"); });
    }

    #[gpui::test]
    fn piano_roll_click_removes_authored_off_grid_note(cx: &mut gpui::TestAppContext) {
        let (host, _commands, _audio) = Host::headless();
        let mut project = Project::default_project();
        project.patterns.push(daw_core::project::Pattern::PianoRoll(daw_core::project::PianoRollPattern {
            id: "offgrid".into(), name: "Off grid".into(), bars: 8, ticks_per_bar: 480, legato: false,
            notes: vec![daw_core::project::Note { pitch: 60, tick: 62, duration: 55, vel: 80 }],
            cc_events: Vec::new(), pitch_bends: Vec::new(), legato_overlap_ticks: 0, legato_max_gap_ticks: None, swing: None,
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
            v.piano_scroll.set_offset(gpui::point(px(0.), px(-300.)));
            cx.notify();
        });
        cx.run_until_parked();
        let position = view.read_with(cx, |v, _| {
            let origin = v.piano_scroll.bounds().origin;
            origin + gpui::point(px(8. + PR_KEYBOARD_GUTTER + 2. * PR_COL_W + 9.),
                px(8. + 28. * (PR_ROW_H + 1.) + 6.) + v.piano_scroll.offset().y)
        });
        cx.simulate_click(position, gpui::Modifiers { shift: true, ..Default::default() });
        view.read_with(cx, |v, _| {
            let pattern=v.st.project.patterns.iter().find(|p| p.id()=="offgrid").unwrap();
            let daw_core::project::Pattern::PianoRoll(pr)=pattern else { panic!("piano") };
            assert_eq!((pr.notes[0].tick,pr.notes[0].duration,pr.notes[0].vel),(62,55,96));
        });
        let empty_cell = position + gpui::point(px(2. * PR_COL_W), px(0.));
        cx.simulate_click(empty_cell, gpui::Modifiers::default());
        cx.simulate_click(position, gpui::Modifiers::default());
        view.read_with(cx, |v, _| {
            let pattern = v.st.project.patterns.iter().find(|p| p.id() == "offgrid").unwrap();
            let daw_core::project::Pattern::PianoRoll(pattern) = pattern else { panic!("piano pattern") };
            assert_eq!(pattern.notes.len(), 1);
            assert_eq!(pattern.notes[0].tick, 120, "keep only the note created in the empty cell");
        });
    }

    #[gpui::test]
    fn setup_icons_open_their_device_controls_at_minimum_width(cx: &mut gpui::TestAppContext) {
        let (host, _commands, _audio) = Host::headless();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = DawView::new(host, Project::default_project(), None, String::new(), cx);
            view.config.light_theme = true;
            view.input_scan_running = true;
            view.midi_ports = vec!["Test keyboard".into()];
            view
        });
        cx.simulate_resize(size(px(800.), px(480.)));
        cx.run_until_parked();
        let mic = cx.debug_bounds("mic-setup").unwrap();
        let midi = cx.debug_bounds("midi-setup").unwrap();
        let amp = cx.debug_bounds("amp-setup").unwrap();
        assert!(mic.right() <= midi.left() && midi.right() <= amp.left());
        assert!(amp.right() <= px(800.));
        assert!(cx.debug_bounds("arr-end").unwrap().right() <= mic.left());

        cx.simulate_click(mic.center(), gpui::Modifiers::default());
        assert!(cx.debug_bounds("in-dev-0").is_some());
        assert!(cx.debug_bounds("in-channel-0").is_some());
        assert!(cx.debug_bounds("midip-0").is_none());

        cx.simulate_click(midi.center(), gpui::Modifiers::default());
        assert!(cx.debug_bounds("midip-0").is_some());
        view.read_with(cx, |view, _| assert!(view.st.open_menu == Some(TopMenu::Midi)));
        cx.simulate_keystrokes("escape");
        view.read_with(cx, |view, _| {
            assert!(view.st.open_menu.is_none());
            assert_eq!(view.theme().bg, 0xffffff);
        });

        let settings = cx.debug_bounds("menu-settings").unwrap();
        cx.simulate_click(settings.center(), gpui::Modifiers::default());
        assert!(cx.debug_bounds("theme-light").is_some());
        assert!(cx.debug_bounds("theme-dark").is_some());
    }

    #[gpui::test]
    fn volume_lane_edits_the_track_without_changing_clips(cx: &mut gpui::TestAppContext) {
        use gpui::{point, Modifiers, MouseButton};

        let (host, commands, _audio) = Host::headless();
        let project = Project::default_project();
        let clips_before = serde_json::to_value(&project.arrangement).unwrap();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = DawView::new(host, project, None, String::new(), cx);
            view.sf2_missing = false;
            view
        });
        cx.simulate_resize(size(px(1120.), px(720.)));
        cx.run_until_parked();

        let toggle = cx.debug_bounds("volume-automation").unwrap();
        cx.simulate_click(toggle.center(), Modifiers::default());
        let lane = cx.debug_bounds("volume-lane-t1").unwrap();
        let dot = point(lane.left() + px(50.), lane.bottom() - px(12.));
        cx.simulate_click(dot, Modifiers::default());
        let original = view.read_with(cx, |view, _| view.st.project.tracks[0].volume_automation[0]);
        assert!(original.tick > 0 && original.gain < 1.0);
        assert!(commands.try_iter().any(|command| matches!(command,
            SequencerCommand::UpdateLive(p) if p.tracks[0].volume_automation == [original])));

        let end = point(lane.left() + px(100.), lane.top() + px(12.));
        cx.simulate_mouse_down(dot, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(dot + point(px(8.), px(0.)), MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        let moved = view.read_with(cx, |view, _| view.st.project.tracks[0].volume_automation[0]);
        assert!(moved.tick > original.tick && moved.gain > 1.0);

        cx.simulate_mouse_down(end, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Right, Modifiers::default());
        view.read_with(cx, |view, _| {
            assert!(view.st.project.tracks[0].volume_automation.is_empty());
            assert_eq!(serde_json::to_value(&view.st.project.arrangement).unwrap(), clips_before);
        });
    }

    #[gpui::test]
    fn arrangement_navigation_keeps_the_ruler_and_tracks_in_sync(cx: &mut gpui::TestAppContext) {
        use gpui::{point, Modifiers, MouseButton, ScrollDelta, ScrollWheelEvent, TouchPhase};

        let (host, commands, _audio) = Host::headless();
        let mut project = Project::default_project();
        for _ in 0..8 {
            project.add_track(daw_core::project::TrackType::Melodic);
        }
        project.arrangement[0].len_bars = Some(48);
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = DawView::new(host, project, None, String::new(), cx);
            view.sf2_missing = false;
            view.st.arrange_full = true;
            view
        });
        cx.simulate_resize(size(px(1000.), px(600.)));
        cx.run_until_parked();
        let beat_one = cx.debug_bounds("timeline-beat-0-0").unwrap();
        let beat_four = cx.debug_bounds("timeline-beat-0-3").unwrap();
        assert_eq!(beat_four.left() - beat_one.left(), px(48.));
        let area = cx.debug_bounds("arrangement").unwrap();
        let position = point(beat_four.center().x, area.center().y);
        let wheel = |modifiers| ScrollWheelEvent {
            position,
            delta: ScrollDelta::Lines(point(0., -1.)),
            modifiers,
            touch_phase: TouchPhase::Moved,
        };

        cx.simulate_event(wheel(Modifiers::default()));
        view.read_with(cx, |view, _| {
            assert_eq!((view.st.arrange_scroll_x, view.st.arrange_scroll_y), (0., 48.));
        });
        cx.simulate_event(wheel(Modifiers { shift: true, ..Default::default() }));
        view.read_with(cx, |view, _| {
            assert_eq!((view.st.arrange_scroll_x, view.st.arrange_scroll_y), (48., 48.));
        });

        let end = cx.debug_bounds("arr-end").unwrap();
        cx.simulate_click(end.center(), Modifiers::default());
        let end_offset = cx.update(|window, cx| view.read(cx).arrangement_end_offset(window));
        assert!(end_offset > 0.);
        view.read_with(cx, |view, _| assert_eq!(view.st.arrange_scroll_x, end_offset));
        assert!(cx.debug_bounds("timeline-beat-47-0").is_some());

        let start = cx.debug_bounds("arr-start").unwrap();
        cx.simulate_click(start.center(), Modifiers::default());
        view.read_with(cx, |view, _| assert_eq!(view.st.arrange_scroll_x, 0.));
        assert!(!commands.try_iter().any(|command| matches!(command, SequencerCommand::Seek(_))));

        let rail = cx.debug_bounds("timeline-scrollbar").unwrap();
        let thumb = cx.debug_bounds("timeline-scrollbar-thumb").unwrap().center();
        let finish = point(rail.right() - px(1.), thumb.y);
        cx.simulate_mouse_down(thumb, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(thumb + point(px(10.), px(0.)), MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(finish, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(finish, MouseButton::Left, Modifiers::default());
        let max_offset = cx.update(|window, cx| {
            let view = view.read(cx);
            view.timeline_max_scroll(view.timeline_width(window))
        });
        view.read_with(cx, |view, _| {
            assert_eq!(view.st.arrange_scroll_x, max_offset);
            assert!(max_offset > end_offset);
            assert_eq!(view.st.arrange_scroll_y, 48.);
        });
    }

    #[gpui::test]
    fn right_click_adds_whole_track_and_bar_comments(cx: &mut gpui::TestAppContext) {
        use gpui::{point, Modifiers, MouseButton};

        let (host, commands, _audio) = Host::headless();
        let mut project = Project::default_project();
        project.arrangement[0].len_bars = Some(4);
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = DawView::new(host, project, None, String::new(), cx);
            view.sf2_missing = false;
            view.st.arrange_full = true;
            view.st.arrange_zoom = 2.;
            view.st.arrange_scroll_x = 192.;
            view
        });
        cx.simulate_resize(size(px(1000.), px(600.)));
        cx.run_until_parked();

        let ruler = cx.debug_bounds("timeline-beat-2-1").unwrap().center();
        let clip = cx.debug_bounds("clip-t1-0").unwrap().center();
        let position = point(ruler.x, clip.y);
        cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
        let menu = cx.debug_bounds("comment-menu-item").unwrap();
        cx.simulate_click(menu.center(), Modifiers::default());
        cx.simulate_keystrokes("a d d space f i l l enter");

        view.read_with(cx, |view, _| {
            assert_eq!(view.st.project.tracks[0].comment(Some(2)), Some("add fill"));
            assert_eq!(view.st.project.tracks[0].comment(None), None);
        });
        assert!(!commands.try_iter().any(|c| matches!(c, SequencerCommand::Seek(_) | SequencerCommand::Play)));

        let pin = cx.debug_bounds("comment-pin-t1-2").unwrap();
        cx.simulate_click(pin.center(), Modifiers::default());
        cx.simulate_keystrokes("ctrl-a s o f t e r enter");
        view.read_with(cx, |view, _| {
            let data = serde_json::to_string(&view.st.project).unwrap();
            let project: Project = serde_json::from_str(&data).unwrap();
            project.validate().unwrap();
            assert_eq!(project.tracks[0].comment(Some(2)), Some("softer"));
        });

        let header = point(px(30.), clip.y);
        cx.simulate_mouse_down(header, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_up(header, MouseButton::Right, Modifiers::default());
        let menu = cx.debug_bounds("comment-menu-item").unwrap();
        cx.simulate_click(menu.center(), Modifiers::default());
        cx.simulate_keystrokes("d r y enter");
        assert!(cx.debug_bounds("comment-pin-t1-track").is_some());
        view.read_with(cx, |view, _| {
            assert_eq!(view.st.project.tracks[0].comment(None), Some("dry"));
            assert_eq!(view.st.project.tracks[0].comment(Some(2)), Some("softer"));
        });
    }

    #[gpui::test]
    fn arrangement_clicks_seek_to_the_pointer_at_any_zoom_and_scroll(cx: &mut gpui::TestAppContext) {
        use daw_core::project::{AudioClip, TrackType};
        use daw_core::sequencer::{TICKS_PER_BAR, TICKS_PER_BEAT};
        use gpui::{point, Modifiers};

        let (host, commands, _audio) = Host::headless();
        let mut project = Project::default_project();
        project.arrangement[0].len_bars = Some(4);
        let track = project.add_track(TrackType::Audio);
        project.audio_clips.push(AudioClip {
            track,
            path: "test.wav".into(),
            start_sec: 3.5,
            offset_sec: 0.,
            len_sec: 2.5,
            gain: 1.,
        });
        let (_, cx) = cx.add_window_view(|_, cx| {
            let mut view = DawView::new(host, project, None, String::new(), cx);
            view.sf2_missing = false;
            view.st.arrange_full = true;
            view.st.arrange_zoom = 2.;
            view.st.arrange_scroll_x = 192.;
            view
        });
        cx.simulate_resize(size(px(1000.), px(600.)));
        cx.run_until_parked();

        let ruler = cx.debug_bounds("timeline-beat-2-1").unwrap();
        let x = ruler.center().x;
        let midi = cx.debug_bounds("clip-t1-0").unwrap();
        let audio = cx.debug_bounds("aclip-0").unwrap();
        let expected = 2 * TICKS_PER_BAR + 3 * TICKS_PER_BEAT / 2;
        for y in [ruler.center().y, midi.center().y, audio.center().y] {
            cx.simulate_click(point(x, y), Modifiers::default());
            let seeks: Vec<_> = commands.try_iter().filter_map(|command| match command {
                SequencerCommand::Seek(tick) => Some(tick),
                _ => None,
            }).collect();
            assert_eq!(seeks, [expected]);
        }

        cx.simulate_keystrokes("space");
        let mut commands = commands.try_iter();
        assert!(matches!(commands.next(), Some(SequencerCommand::Play)));
        assert!(!commands.any(|command| matches!(command, SequencerCommand::Seek(_))));
    }

    #[gpui::test]
    fn piano_roll_scrollbar_reaches_the_end_of_a_100_bar_pattern(cx: &mut gpui::TestAppContext) {
        use gpui::{Modifiers, MouseButton, ScrollDelta, ScrollWheelEvent, TouchPhase, point};

        let (host, _commands, _audio) = Host::headless();
        let mut project = Project::default_project();
        project.patterns.push(daw_core::project::Pattern::PianoRoll(daw_core::project::PianoRollPattern {
            id: "long-scroll".into(), name: "Long part".into(), bars: 100, ticks_per_bar: 480,
            legato: false, notes: vec![],
            cc_events: Vec::new(), pitch_bends: Vec::new(), legato_overlap_ticks: 0, legato_max_gap_ticks: None, swing: None,
        }));
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = DawView::new(host, project, None, String::new(), cx);
            view.sf2_missing = false;
            view.st.active_dock = DockTab::PianoRoll;
            view.st.selected_pattern = Some("long-scroll".into());
            view.st.dock_h = 300.;
            view
        });
        cx.simulate_resize(size(px(1000.), px(700.)));
        cx.run_until_parked();
        let grid = cx.debug_bounds("piano-scroll-viewport").unwrap();
        let initial = view.read_with(cx, |view, _| view.piano_scroll.offset());
        cx.simulate_event(ScrollWheelEvent {
            position: grid.center(), delta: ScrollDelta::Lines(point(0., -2.)),
            modifiers: Modifiers { shift: true, ..Default::default() },
            touch_phase: TouchPhase::Moved,
        });
        let shifted = view.read_with(cx, |view, _| view.piano_scroll.offset());
        assert!(shifted.x < initial.x && shifted.y == initial.y);
        cx.simulate_event(ScrollWheelEvent {
            position: grid.center(), delta: ScrollDelta::Pixels(point(px(0.), px(-29.))),
            modifiers: Modifiers::default(), touch_phase: TouchPhase::Moved,
        });
        let wheeled = view.read_with(cx, |view, _| view.piano_scroll.offset());
        assert!(wheeled.y < shifted.y && wheeled.x == shifted.x);
        cx.simulate_event(ScrollWheelEvent {
            position: grid.center(), delta: ScrollDelta::Pixels(point(px(-30.), px(0.))),
            modifiers: Modifiers::default(), touch_phase: TouchPhase::Moved,
        });
        assert!(view.read_with(cx, |view, _| view.piano_scroll.offset().x) < wheeled.x);

        let rail = cx.debug_bounds("piano-time-scrollbar").unwrap();
        let start = point(rail.left() + px(8.), rail.center().y);
        let finish = point(rail.right() - px(1.), rail.center().y);
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(start + point(px(10.), px(0.)), MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(finish, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(finish, MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, _| {
            assert_eq!(view.piano_scroll.offset().x, -view.piano_scroll.max_offset().width);
            assert_eq!(view.piano_scroll.offset().y, wheeled.y);
        });
        cx.simulate_click(start, Modifiers::default());
        assert_eq!(view.read_with(cx, |view, _| view.piano_scroll.offset().x), px(0.));
    }

    #[gpui::test]
    fn laptop_piano_auditions_and_step_enters_notes_only_while_enabled(cx: &mut gpui::TestAppContext) {
        use daw_core::project::Pattern;

        let (host, commands, _audio) = Host::headless();
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut project = Project::default_project();
            let Pattern::PianoRoll(pattern) = &mut project.patterns[1] else { panic!("piano roll") };
            pattern.notes.clear();
            let mut view = DawView::new(host, project, None, String::new(), cx);
            view.sf2_missing = false;
            view.st.select_track("t2");
            view.st.active_dock = DockTab::PianoRoll;
            view.st.dock_h = 350.;
            view.root_focus.focus(window);
            view
        });
        cx.simulate_resize(size(px(1000.), px(700.)));
        cx.run_until_parked();

        cx.simulate_keystrokes("z");
        let keyboard = cx.debug_bounds("pr-computer-piano").unwrap().center();
        cx.simulate_click(keyboard, gpui::Modifiers::default());
        cx.simulate_keystrokes("z s right w left w escape b");
        view.read_with(cx, |view, _| {
            let Pattern::PianoRoll(pattern) = &view.st.project.patterns[1] else { panic!("piano roll") };
            let notes: Vec<_> = pattern.notes.iter().map(|note| (note.pitch, note.tick, note.duration)).collect();
            assert_eq!(notes, [(60, 0, 30), (61, 30, 30), (74, 90, 30)]);
            assert!(view.piano_step_input.is_none());
        });

        cx.simulate_keystrokes("l ctrl-z");
        view.read_with(cx, |view, _| {
            let Pattern::PianoRoll(pattern) = &view.st.project.patterns[1] else { panic!("piano roll") };
            assert!(pattern.notes.len() < 3, "Ctrl+Z should undo the step entry");
        });

        let preview: Vec<_> = commands.try_iter().filter_map(|command| match command {
            SequencerCommand::Audition { track_id, notes } => {
                assert_eq!(track_id, "t2");
                Some(notes[0].pitch)
            }
            _ => None,
        }).collect();
        assert_eq!(preview, [60, 61, 74, 74]);
    }

    #[gpui::test]
    fn piano_keyboard_stays_visible_on_horizontal_scroll_and_auditions_sharps(cx: &mut gpui::TestAppContext) {
        let (host, commands, _audio) = Host::headless();
        let mut project = Project::default_project();
        project.patterns.push(daw_core::project::Pattern::PianoRoll(daw_core::project::PianoRollPattern {
            id: "keyboard.1".into(), name: "Keys".into(), bars: 8, ticks_per_bar: 480, legato: false,
            notes: vec![daw_core::project::Note { pitch: 60, tick: 0, duration: 120, vel: 88 }],
            cc_events: Vec::new(), pitch_bends: Vec::new(), legato_overlap_ticks: 0, legato_max_gap_ticks: None, swing: None,
        }));
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = DawView::new(host, project, None, String::new(), cx);
            view.sf2_missing = false;
            view.st.active_dock = DockTab::PianoRoll;
            view.st.selected_pattern = Some("keyboard.1".into());
            view.st.dock_h = 350.;
            view
        });
        cx.simulate_resize(size(px(1000.), px(700.)));
        cx.run_until_parked();
        let natural = cx.debug_bounds("piano-key-60").expect("view opens around the authored note");
        let sharp = cx.debug_bounds("piano-key-61").unwrap();
        assert!(sharp.size.width < natural.size.width);
        view.update(cx, |view, cx| {
            let offset = view.piano_scroll.offset();
            view.piano_scroll.set_offset(gpui::point(px(-350.), offset.y));
            cx.notify();
        });
        cx.run_until_parked();
        let scrolled = cx.debug_bounds("piano-key-61").unwrap();
        assert_eq!(sharp.origin.x, scrolled.origin.x);
        cx.simulate_click(scrolled.center(), gpui::Modifiers::default());
        let notes: Vec<_> = commands.try_iter().filter_map(|cmd| match cmd {
            SequencerCommand::Audition { notes, .. } => Some(notes),
            _ => None,
        }).flatten().map(|note| note.pitch).collect();
        assert_eq!(notes, [61]);
    }

    #[gpui::test]
    fn new_asks_before_discarding_unsaved_work_and_load_starts_fresh_history(cx: &mut gpui::TestAppContext) {
        let (host, _commands, _audio) = Host::headless();
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut v = DawView::new(host, Project::default_project(), None, String::new(), cx);
            v.sf2_missing = false;
            v.root_focus.focus(window);
            v
        });
        // Clean project: New proceeds at once and is not itself "unsaved".
        cx.update(|window, cx| view.update(cx, |v, _| {
            v.request_replace(ConfirmThen::New, window);
            assert!(!v.close_confirmation);
            assert!(!v.is_dirty(), "an untouched new project has nothing to save");
        }));
        // Dirty project: New waits behind the dialog; Escape keeps the work.
        cx.update(|window, cx| view.update(cx, |v, _| {
            v.st.project.project.bpm = 97.;
            v.request_replace(ConfirmThen::New, window);
            assert!(v.close_confirmation);
            assert_eq!(v.confirm_then, ConfirmThen::New);
            assert_eq!(v.st.project.project.bpm, 97.);
        }));
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        view.read_with(cx, |v, _| {
            assert!(!v.close_confirmation);
            assert_eq!(v.confirm_then, ConfirmThen::Close);
            assert_eq!(v.st.project.project.bpm, 97.);
        });
        // Opening a file drops the previous song's undo steps, so Ctrl+Z can
        // never put the old song under the new file's path.
        let dir = std::env::temp_dir().join(format!("daw-gui-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("other.daw.json");
        let mut other = Project::default_project();
        other.project.bpm = 141.;
        daw_core::project::format::save_if_unchanged(&other, &path, None).unwrap();
        view.update(cx, |v, _| {
            v.st.mixer_h = 333.;
            v.undo.record("edit", &v.st.project.clone());
            v.load_project(path.clone());
            assert_eq!(v.st.project.project.bpm, 141.);
            assert!(!v.undo.can_undo());
            assert_eq!(v.st.mixer_h, 333., "layout survives opening a file");
            v.undo();
            assert_eq!(v.st.project.project.bpm, 141.);
        });
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A song the size of a transcribed cover: six tracks, each one 100-bar
    /// piano-roll clip of ~700 notes.
    fn heavy_project() -> Project {
        let mut p = Project::default_project();
        p.tracks.clear(); p.patterns.clear(); p.arrangement.clear();
        for t in 0..6 {
            let id = p.add_track(daw_core::project::TrackType::Melodic);
            let pid = format!("long{t}");
            let notes = (0..700u32).map(|i| daw_core::project::Note {
                pitch: 48 + ((i * 7 + t * 5) % 30) as u8, tick: i * 68, duration: 50, vel: 90,
            }).collect();
            p.patterns.retain(|pat| !p.tracks.iter().any(|tr| tr.id == id && tr.patterns.contains(&pat.id().to_string())));
            p.patterns.push(daw_core::project::Pattern::PianoRoll(daw_core::project::PianoRollPattern {
                id: pid.clone(), name: format!("Long {t}"), bars: 100, ticks_per_bar: 480, legato: false, notes,
                cc_events: Vec::new(), pitch_bends: Vec::new(), legato_overlap_ticks: 0, legato_max_gap_ticks: None, swing: None,
            }));
            let track = p.tracks.iter_mut().find(|tr| tr.id == id).unwrap();
            track.patterns = vec![pid.clone()];
            p.arrangement.push(daw_core::project::ArrangementSlot { track: id, pattern: pid, bar: 0, len_bars: Some(100), offset_ticks: 0, gain: 1.0 });
        }
        p.validate().unwrap();
        p
    }

    /// Frame cost on a large song. Run with:
    /// `cargo test -p daw-gpui --release frame_cost -- --ignored --nocapture`
    #[gpui::test]
    #[ignore = "benchmark: prints milliseconds per frame"]
    fn frame_cost_on_a_large_song(cx: &mut gpui::TestAppContext) {
        let project = std::env::var_os("DAW_GUI_BENCH_PROJECT");
        let (width, height, dock) = if project.is_some() {
            (1648., 927., 445.)
        } else {
            (1280., 800., 200.)
        };
        let project = project.map_or_else(heavy_project, |path| daw_core::project::format::load(std::path::PathBuf::from(path)).unwrap());
        let (host, _commands, _audio) = Host::headless();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut v = DawView::new(host, project, None, String::new(), cx);
            v.sf2_missing = false;
            v.st.dock_h = dock;
            v
        });
        cx.simulate_resize(size(px(width), px(height)));
        cx.run_until_parked();
        for (tab, mixer_open) in [(DockTab::Sequencer, false), (DockTab::PianoRoll, false), (DockTab::PianoRoll, true)] {
            view.update(cx, |v, cx| {
                v.st.active_dock = tab;
                v.st.mixer_open = mixer_open;
                let first = v.st.project.tracks[0].id.clone();
                v.st.select_track(&first);
                v.playing = true;
                cx.notify();
            });
            cx.run_until_parked();
            let frames = 60;
            let started = std::time::Instant::now();
            for i in 0..frames {
                view.update(cx, |v, cx| { v.pump(); v.st.set_playhead(i * 40); cx.notify(); });
                cx.run_until_parked();
            }
            println!("{tab:?} (mixer {mixer_open}): {:.2} ms/frame while playing", started.elapsed().as_secs_f64() * 1000. / frames as f64);
        }
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
                Note { pitch: 42, tick: 600, duration: 120, vel: 100 },
            ], legato: false,
            cc_events: Vec::new(), pitch_bends: Vec::new(), legato_overlap_ticks: 0, legato_max_gap_ticks: None, swing: None,
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
            swing: None,
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
            cc_events: Vec::new(), pitch_bends: Vec::new(), legato_overlap_ticks: 0, legato_max_gap_ticks: None, swing: None,
        });
        assert!(clip_preview_marks(&big, 6000.0, 40.0, 48_000).len() <= 256);
    }
}

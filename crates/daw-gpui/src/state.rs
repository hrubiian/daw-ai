use crate::app::TopMenu;
use daw_core::project::Project;
#[cfg(test)]
use daw_core::sequencer::TICKS_PER_BAR;
use daw_core::sequencer::TICKS_PER_BEAT;
use std::collections::HashSet;
use std::path::PathBuf;

/// Editors in the bottom dock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockTab {
    Sequencer,
    PianoRoll,
    Fx,
    Code,
}

impl DockTab {
    pub fn label(self) -> &'static str {
        match self {
            DockTab::Sequencer => "Sequencer",
            DockTab::PianoRoll => "Piano Roll",
            DockTab::Fx => "FX",
            DockTab::Code => "Code",
        }
    }
    pub const ALL: [DockTab; 4] = [
        DockTab::Sequencer,
        DockTab::PianoRoll,
        DockTab::Fx,
        DockTab::Code,
    ];
}

/// The GPUI-independent slice of app state, so logic is unit-testable without a
/// window or the engine running. `app.rs`'s root view wraps this plus the `Host`.
/// What's being renamed inline (double-click on a name).
#[derive(Debug, Clone, PartialEq)]
pub enum RenameTarget {
    Track(String),
    Pattern(String),
    /// (pattern id, row index)
    Row(String, usize),
}

pub struct HeadlessState {
    pub project: Project,
    pub project_path: Option<PathBuf>,
    pub file_revision: Option<String>,
    pub saved_hash: String,
    pub playhead: u32,
    /// Currently selected track id (drives header highlight + which dock content).
    pub selected_track: Option<String>,
    /// Soloed track ids. Non-empty → only these tracks sound.
    pub soloed: HashSet<String>,
    pub active_dock: DockTab,
    pub mixer_open: bool,
    pub browser_open: bool,
    /// Pattern currently shown in the editor dock (defaults to the selected
    /// track's first pattern; changed by the pattern picker or a clip click).
    pub selected_pattern: Option<String>,
    /// Which GM category is selected in the instrument browser.
    pub browser_category: usize,
    /// JAM mode: hot-reload the project file when it changes on disk.
    pub jam_mode: bool,
    /// Transient status line (save/export/jam/repl feedback).
    pub status: String,
    /// Single-line mini-notation REPL buffer (Code tab).
    pub repl_buf: String,
    /// Last-seen project file mtime, for JAM change detection.
    pub project_mtime: Option<std::time::SystemTime>,
    /// Preferred mixer and editor heights in pixels.
    pub mixer_h: f32,
    pub dock_h: f32,
    /// Waveform peaks cached by WAV path (computed lazily on first render).
    pub peaks_cache: std::collections::HashMap<String, std::sync::Arc<Vec<f32>>>,
    /// Source-duration (seconds) cache, keyed by WAV path.
    pub peaks_dur_cache: std::collections::HashMap<String, f64>,
    /// Per-track lane heights (px), keyed by track id. Absent = default (TRACK_H).
    pub track_heights: std::collections::HashMap<String, f32>,
    /// Full-window Arrange mode: arrangement fills the window below the header.
    pub arrange_full: bool,
    /// Arrangement zoom factor; effective bar width = BAR_W * arrange_zoom (clamped in app).
    pub arrange_zoom: f32,
    /// Horizontal scroll offset (px) of the arrange timeline; 0 = bar 1 at the left.
    pub arrange_scroll_x: f32,
    /// Vertical scroll offset (px) of the track area; 0 = first track at the top.
    /// Shared by the track-header column and the arrange lanes so they stay aligned.
    pub arrange_scroll_y: f32,
    /// Which top menu dropdown is open (None = closed).
    pub open_menu: Option<TopMenu>,
    /// Instrument-browser search query (empty = normal category view).
    pub browser_search: String,
    /// Accent-color index into app::ACCENTS.
    pub accent_idx: usize,
    /// Whether the Audio Effects overlay is open.
    pub fx_window_open: bool,
    /// Selected return bus (0 = reverb, 1 = delay): FX dock shows its params.
    pub selected_return: Option<usize>,
    /// CLAP discovery results (background scan at startup).
    pub clap_plugins: Vec<daw_core::audio::clap::ClapPluginDesc>,
    /// Param metadata per live CLAP slot uid: (plugin name, params).
    pub clap_meta: std::collections::HashMap<String, (String, Vec<daw_core::audio::clap::ClapParamInfo>)>,
    /// Inline rename state: target + edit buffer (double-click a name).
    pub renaming: Option<RenameTarget>,
    pub rename_buf: String,
    /// Anchor of the audio-clip drag in progress (`None` between drags).
    pub audio_drag: Option<AudioDragAnchor>,
    /// A CLAP instrument click waiting for its confirming second click:
    /// (track id, plugin path, plugin id, when armed).
    pub pending_instrument: Option<(String, String, String, std::time::Instant)>,
}

/// How long an armed instrument swap waits for its confirming click.
const INSTRUMENT_CONFIRM_WINDOW: std::time::Duration = std::time::Duration::from_secs(5);

/// What an audio-clip drag moves: the whole clip or one of its edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioDragKind {
    Move,
    CropStart,
    CropEnd,
}

/// Cursor x and clip placement captured on a drag's first move event.
/// Every later event recomputes the placement from this anchor, so several
/// mouse moves between repaints (whose element bounds are a frame stale)
/// can't accumulate, and grabbing mid-clip doesn't snap its edge to the cursor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioDragAnchor {
    idx: usize,
    kind: AudioDragKind,
    mouse_x: f32,
    start_sec: f64,
    offset_sec: f64,
    len_sec: f64,
}

impl HeadlessState {
    /// True while clip `idx` is being dragged as `kind` (anchor captured).
    pub fn is_audio_drag(&self, idx: usize, kind: AudioDragKind) -> bool {
        self.audio_drag.is_some_and(|a| a.idx == idx && a.kind == kind)
    }

    /// Drag clip `idx` (body or edge) to cursor x `mouse_x` at `pps` pixels
    /// per second. The first call of a drag anchors it; later calls place the
    /// clip at the anchor plus the cursor's travel. Returns whether the
    /// clip's placement changed.
    pub fn drag_audio_clip(&mut self, idx: usize, kind: AudioDragKind, mouse_x: f32, pps: f32) -> bool {
        let Some(c) = self.project.audio_clips.get(idx) else { return false };
        let before = (c.start_sec, c.offset_sec, c.len_sec);
        if !self.is_audio_drag(idx, kind) {
            self.audio_drag = Some(AudioDragAnchor {
                idx, kind, mouse_x, start_sec: before.0, offset_sec: before.1, len_sec: before.2,
            });
        }
        let Some(a) = self.audio_drag else { return false };
        let delta = ((mouse_x - a.mouse_x) / pps.max(f32::EPSILON)) as f64;
        // Restore the anchored placement, then apply the whole travel once.
        if let Some(c) = self.project.audio_clips.get_mut(idx) {
            c.start_sec = a.start_sec;
            c.offset_sec = a.offset_sec;
            c.len_sec = a.len_sec;
        }
        match kind {
            AudioDragKind::Move => self.project.move_audio_clip(idx, a.start_sec + delta),
            AudioDragKind::CropStart => self.project.crop_audio_clip_start(idx, delta),
            AudioDragKind::CropEnd => self.project.crop_audio_clip_end(idx, a.len_sec + delta),
        }
        let c = &self.project.audio_clips[idx];
        (c.start_sec, c.offset_sec, c.len_sec) != before
    }

    /// The drag was released: the next drag re-anchors at its own grab.
    pub fn end_audio_drag(&mut self) {
        self.audio_drag = None;
    }

    pub fn from_project(project: Project, project_path: Option<PathBuf>) -> Self {
        let file_revision = project_path.as_ref().and_then(|path| daw_core::project::format::load_versioned(path).ok().map(|(disk, revision)| {
            if daw_core::project::format::project_revision(&disk).ok() == daw_core::project::format::project_revision(&project).ok() { revision } else { "stale-loaded-project".into() }
        }));
        let saved_hash = daw_core::project::format::project_revision(&project).unwrap_or_default();
        let selected_track = project.tracks.first().map(|t| t.id.clone());
        let selected_pattern = selected_track
            .as_ref()
            .and_then(|tid| project.tracks.iter().find(|t| &t.id == tid))
            .and_then(|t| t.patterns.first().cloned());
        Self {
            project,
            project_path,
            file_revision,
            saved_hash,
            playhead: 0,
            selected_track,
            soloed: HashSet::new(),
            active_dock: DockTab::Sequencer,
            mixer_open: false,
            browser_open: false,
            selected_pattern,
            browser_category: 0,
            jam_mode: false,
            status: String::new(),
            repl_buf: String::new(),
            project_mtime: None,
            mixer_h: 304.0,
            dock_h: 200.0,
            peaks_cache: std::collections::HashMap::new(),
            peaks_dur_cache: std::collections::HashMap::new(),
            track_heights: std::collections::HashMap::new(),
            arrange_full: false,
            arrange_zoom: 1.0,
            arrange_scroll_x: 0.0,
            arrange_scroll_y: 0.0,
            open_menu: None,
            browser_search: String::new(),
            accent_idx: 0,
            fx_window_open: false,
            selected_return: None,
            clap_plugins: Vec::new(),
            clap_meta: std::collections::HashMap::new(),
            renaming: None,
            rename_buf: String::new(),
            audio_drag: None,
            pending_instrument: None,
        }
    }

    pub fn bpm(&self) -> f64 {
        self.project.project.bpm
    }

    pub fn playhead(&self) -> u32 {
        self.playhead
    }

    pub fn set_playhead(&mut self, tick: u32) {
        self.playhead = tick;
    }

    /// Zero-based bar index the playhead currently sits in.
    pub fn current_bar(&self) -> u32 {
        self.playhead / self.project.ticks_per_bar()
    }

    pub fn select_track(&mut self, id: &str) {
        self.selected_return = None;
        self.selected_track = Some(id.to_string());
        self.selected_pattern = self
            .project
            .tracks
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.patterns.first().cloned());
    }

    /// Repair the selection after the project was replaced (undo, a shared
    /// session update, a JAM reload): keep the selected track and pattern
    /// when they still exist, else fall back to the nearest valid choice.
    pub fn reconcile_selection(&mut self) {
        let track_alive = self.selected_track.as_ref()
            .is_some_and(|id| self.project.tracks.iter().any(|t| &t.id == id));
        if !track_alive {
            self.selected_track = self.project.tracks.first().map(|t| t.id.clone());
        }
        let patterns = self.track_pattern_ids();
        if !self.selected_pattern.as_ref().is_some_and(|p| patterns.contains(p)) {
            self.selected_pattern = patterns.first().cloned();
        }
        let ids: HashSet<&str> = self.project.tracks.iter().map(|t| t.id.as_str()).collect();
        self.soloed.retain(|id| ids.contains(id.as_str()));
        self.track_heights.retain(|id, _| ids.contains(id.as_str()));
        if self.renaming.is_some() { self.renaming = None; }
    }

    pub fn select_pattern(&mut self, id: &str) {
        self.selected_pattern = Some(id.to_string());
    }

    pub fn selected_pattern_id(&self) -> Option<String> {
        self.selected_pattern.clone()
    }

    /// Pattern ids belonging to the selected track, in track order.
    pub fn track_pattern_ids(&self) -> Vec<String> {
        self.selected_track
            .as_ref()
            .and_then(|tid| self.project.tracks.iter().find(|t| &t.id == tid))
            .map(|t| t.patterns.clone())
            .unwrap_or_default()
    }

    pub fn set_browser_category(&mut self, idx: usize) {
        self.browser_category = idx;
    }

    /// Current playhead position in seconds (from tick + project bpm).
    pub fn playhead_seconds(&self) -> f64 {
        let ticks_per_sec = (self.project.project.bpm / 60.0) * TICKS_PER_BEAT as f64;
        self.playhead as f64 / ticks_per_sec.max(1.0)
    }

    pub fn set_status(&mut self, s: impl Into<String>) { self.status = s.into(); }

    /// Whether a click on CLAP instrument `plugin_id` should replace track
    /// `tid`'s instrument now. The first track is selected on open, so a
    /// single browser click used to silently swap out its sound: the first
    /// click only arms the swap (the status line says so), and a second click
    /// on the same plugin within a few seconds confirms it. A track that
    /// already plays this plugin is left alone (re-picking resets its patch).
    pub fn confirm_clap_instrument(&mut self, tid: &str, path: &str, plugin_id: &str, now: std::time::Instant) -> bool {
        let Some(track) = self.project.tracks.iter().find(|t| t.id == tid) else { return false };
        let current = match &track.instrument {
            daw_core::project::Instrument::Clap(s) if s.path == path && s.id == plugin_id => {
                self.pending_instrument = None;
                self.status = format!("{} already plays {plugin_id}", track.name);
                return false;
            }
            daw_core::project::Instrument::Clap(s) => s.id.clone(),
            daw_core::project::Instrument::Soundfont { preset, soundfont: Some(sf), .. } => format!("{sf} preset {preset}"),
            daw_core::project::Instrument::Soundfont { preset, .. } => format!("GM preset {preset}"),
            daw_core::project::Instrument::Sampler { .. } => "sampler".into(),
            daw_core::project::Instrument::AudioFile { .. } => "audio".into(),
            daw_core::project::Instrument::Sfz { path } => format!("sfz {path}"),
        };
        let confirmed = matches!(&self.pending_instrument,
            Some((t, p, id, at)) if t == tid && p == path && id == plugin_id && now.duration_since(*at) <= INSTRUMENT_CONFIRM_WINDOW);
        if confirmed {
            self.pending_instrument = None;
        } else {
            self.status = format!("click {plugin_id} again to replace {} ({current})", track.name);
            self.pending_instrument = Some((tid.into(), path.into(), plugin_id.into(), now));
        }
        confirmed
    }
    pub fn toggle_jam(&mut self) {
        self.jam_mode = !self.jam_mode;
        // Re-arm the hot-reload guard so re-enabling JAM re-observes (doesn't
        // instantly reload a file edited while JAM was off).
        self.project_mtime = None;
    }
    pub fn repl_push(&mut self, c: char) { self.repl_buf.push(c); }
    pub fn repl_backspace(&mut self) { self.repl_buf.pop(); }
    pub fn repl_clear(&mut self) { self.repl_buf.clear(); }

    pub fn toggle_solo(&mut self, id: &str) {
        if !self.soloed.remove(id) {
            self.soloed.insert(id.to_string());
        }
    }

    /// Whether `id` should be silenced right now: its own mute flag, or — when
    /// anything is soloed — not being one of the soloed tracks.
    pub fn effective_muted(&self, id: &str) -> bool {
        let own = self.project.tracks.iter()
            .find(|t| t.id == id)
            .map(|t| t.muted)
            .unwrap_or(false);
        let solo_silenced = !self.soloed.is_empty() && !self.soloed.contains(id);
        own || solo_silenced
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaced_project_keeps_a_surviving_selection_and_repairs_a_stale_one() {
        let mut st = HeadlessState::from_project(Project::default_project(), None);
        let second = st.project.tracks[1].id.clone();
        st.select_track(&second);
        st.soloed.insert(second.clone());
        let pattern = st.selected_pattern.clone();
        // An agent edit that leaves the track in place keeps the selection.
        st.project.project.bpm += 3.;
        st.reconcile_selection();
        assert_eq!(st.selected_track.as_deref(), Some(second.as_str()));
        assert_eq!(st.selected_pattern, pattern);
        assert!(st.soloed.contains(&second));
        // Removing it falls back to a track and pattern that exist.
        st.project.remove_track(&second);
        st.reconcile_selection();
        let first = st.project.tracks[0].id.clone();
        assert_eq!(st.selected_track.as_deref(), Some(first.as_str()));
        assert_eq!(st.selected_pattern, st.project.tracks[0].patterns.first().cloned());
        assert!(st.soloed.is_empty(), "solo of a removed track must not silence the rest");
    }

    fn st_with_clip(start_sec: f64) -> HeadlessState {
        let mut p = Project::default_project();
        let t = p.add_track(daw_core::project::TrackType::Audio);
        p.audio_clips.push(daw_core::project::AudioClip {
            track: t, path: "take.wav".into(), start_sec, offset_sec: 0.0, len_sec: 4.0, gain: 1.0,
        });
        HeadlessState::from_project(p, None)
    }

    #[test]
    fn audio_clip_drag_follows_the_cursor_without_jumping_or_running_away() {
        // 100 px per second; the user grabs the clip mid-body at x = 300.
        let mut st = st_with_clip(2.0);
        let clip = |st: &HeadlessState| st.project.audio_clips[0].clone();
        st.drag_audio_clip(0, AudioDragKind::Move, 300.0, 100.0);
        assert_eq!(clip(&st).start_sec, 2.0, "grabbing doesn't jump the clip");
        // Several move events arrive before the next repaint (same stale
        // bounds): the clip must land at the cursor delta, not accumulate.
        for _ in 0..5 {
            st.drag_audio_clip(0, AudioDragKind::Move, 350.0, 100.0);
        }
        assert!((clip(&st).start_sec - 2.5).abs() < 1e-9, "moved +0.5 s once, got {}", clip(&st).start_sec);
        st.drag_audio_clip(0, AudioDragKind::Move, 250.0, 100.0);
        assert!((clip(&st).start_sec - 1.5).abs() < 1e-9, "and back left");
        // A new drag (after release) re-anchors at its own grab point.
        st.end_audio_drag();
        st.drag_audio_clip(0, AudioDragKind::Move, 900.0, 100.0);
        assert!((clip(&st).start_sec - 1.5).abs() < 1e-9, "re-grab elsewhere: no jump");
    }

    #[test]
    fn audio_clip_crop_drags_are_anchored_too() {
        let mut st = st_with_clip(2.0);
        let clip = |st: &HeadlessState| st.project.audio_clips[0].clone();
        // Right edge: +1 s, repeated events don't keep growing it.
        st.drag_audio_clip(0, AudioDragKind::CropEnd, 600.0, 100.0);
        for _ in 0..3 { st.drag_audio_clip(0, AudioDragKind::CropEnd, 700.0, 100.0); }
        assert!((clip(&st).len_sec - 5.0).abs() < 1e-9, "len 4 → 5, got {}", clip(&st).len_sec);
        st.end_audio_drag();
        // Left edge: trim 0.5 s off the front; the audio under the cut stays put.
        st.drag_audio_clip(0, AudioDragKind::CropStart, 200.0, 100.0);
        for _ in 0..3 { st.drag_audio_clip(0, AudioDragKind::CropStart, 250.0, 100.0); }
        let c = clip(&st);
        assert!((c.start_sec - 2.5).abs() < 1e-9 && (c.offset_sec - 0.5).abs() < 1e-9 && (c.len_sec - 4.5).abs() < 1e-9,
            "trimmed once: {c:?}");
    }

    #[test]
    fn state_reads_project_metadata() {
        let project = Project::default_project();
        let bpm = project.project.bpm;
        let st = HeadlessState::from_project(project, None);
        assert_eq!(st.bpm(), bpm);
        assert_eq!(st.playhead(), 0);
        assert_eq!(st.selected_track.as_deref(), Some("t1")); // first track selected
        assert!(!st.mixer_open);
        assert_eq!(st.active_dock, DockTab::Sequencer);
    }

    #[test]
    fn set_playhead_updates() {
        let mut st = HeadlessState::from_project(Project::default_project(), None);
        st.set_playhead(480);
        assert_eq!(st.playhead(), 480);
    }

    #[test]
    fn tick_to_bar_uses_ticks_per_bar() {
        let mut st = HeadlessState::from_project(Project::default_project(), None);
        assert_eq!(st.current_bar(), 0);
        st.set_playhead(TICKS_PER_BAR);
        assert_eq!(st.current_bar(), 1);            // exactly one bar in
        st.set_playhead(TICKS_PER_BAR * 2 - 1);
        assert_eq!(st.current_bar(), 1);            // still in bar 1, not yet bar 2
    }

    #[test]
    fn selecting_track_picks_its_first_pattern() {
        let mut st = HeadlessState::from_project(Project::default_project(), None);
        // default: first track t1 selected, whose first pattern is p1.
        assert_eq!(st.selected_pattern.as_deref(), Some("p1"));
        st.select_track("t2");
        assert_eq!(st.selected_pattern.as_deref(), Some("p2"));
        // explicit pattern selection overrides
        st.select_pattern("p1");
        assert_eq!(st.selected_pattern_id(), Some("p1".to_string()));
    }

    #[test]
    fn track_pattern_ids_lists_selected_tracks_patterns() {
        let st = HeadlessState::from_project(Project::default_project(), None);
        // t1 has ["p1"]; selecting it lists p1
        assert_eq!(st.track_pattern_ids(), vec!["p1".to_string()]);
    }

    #[test]
    fn browser_category_defaults_zero_and_sets() {
        let mut st = HeadlessState::from_project(Project::default_project(), None);
        assert_eq!(st.browser_category, 0);
        st.set_browser_category(4);
        assert_eq!(st.browser_category, 4);
    }

    #[test]
    fn phase4_state_defaults_and_setters() {
        let mut st = HeadlessState::from_project(Project::default_project(), None);
        assert!(!st.jam_mode);
        assert_eq!(st.status, "");
        assert_eq!(st.repl_buf, "");
        st.set_status("saved");
        assert_eq!(st.status, "saved");
        st.toggle_jam();
        assert!(st.jam_mode);
        st.repl_push('c');
        st.repl_push('4');
        assert_eq!(st.repl_buf, "c4");
        st.repl_backspace();
        assert_eq!(st.repl_buf, "c");
    }

    #[test]
    fn clap_instrument_click_asks_before_replacing_a_tracks_sound() {
        let mut st = HeadlessState::from_project(Project::default_project(), None);
        let tid = st.project.tracks[0].id.clone();
        let t0 = std::time::Instant::now();
        let surge = ("/usr/lib/clap/Surge XT.clap", "org.surge-synth-team.surge-xt");
        assert!(!st.confirm_clap_instrument(&tid, surge.0, surge.1, t0), "first click only arms the swap");
        assert!(st.status.contains("again"), "status says how to confirm: {}", st.status);
        assert!(st.confirm_clap_instrument(&tid, surge.0, surge.1, t0 + std::time::Duration::from_secs(2)), "second click confirms");
        // A second click long after the first is a new first click.
        assert!(!st.confirm_clap_instrument(&tid, surge.0, surge.1, t0 + std::time::Duration::from_secs(10)));
        assert!(!st.confirm_clap_instrument(&tid, surge.0, surge.1, t0 + std::time::Duration::from_secs(20)), "stale arm expired");
        // Clicking a different plugin re-arms instead of confirming.
        assert!(!st.confirm_clap_instrument(&tid, "/x.clap", "other.synth", t0 + std::time::Duration::from_secs(21)));
        // A track that already plays this plugin keeps its sound (re-picking would reset it).
        st.project.set_track_clap_instrument(&tid, surge.0, surge.1).unwrap();
        let t1 = t0 + std::time::Duration::from_secs(30);
        assert!(!st.confirm_clap_instrument(&tid, surge.0, surge.1, t1));
        assert!(!st.confirm_clap_instrument(&tid, surge.0, surge.1, t1), "same plugin never replaces itself");
    }

    #[test]
    fn solo_makes_other_tracks_effectively_muted() {
        let mut st = HeadlessState::from_project(Project::default_project(), None);
        // nothing soloed → effective mute follows the track's own flag
        assert!(!st.effective_muted("t1"));
        st.toggle_solo("t2");
        assert!(st.effective_muted("t1"));   // not soloed → silenced
        assert!(!st.effective_muted("t2"));  // soloed → audible
        st.toggle_solo("t2");                // un-solo
        assert!(!st.effective_muted("t1"));
    }
}

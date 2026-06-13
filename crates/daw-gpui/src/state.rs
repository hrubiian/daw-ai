use crate::app::TopMenu;
use daw_core::project::Project;
use daw_core::sequencer::{TICKS_PER_BAR, TICKS_PER_BEAT};
use std::collections::HashSet;
use std::path::PathBuf;

/// Which editor occupies the bottom dock. Phase 1 only lays the tabs out;
/// Phase 2/3/4 fill in the editors behind them.
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
    pub playhead: u32,
    /// Currently selected track id (drives header highlight + which dock content).
    pub selected_track: Option<String>,
    /// Soloed track ids. Non-empty → only these tracks sound.
    pub soloed: HashSet<String>,
    pub active_dock: DockTab,
    pub mixer_open: bool,
    /// Pattern currently shown in the editor dock (defaults to the selected
    /// track's first pattern; changed by the pattern picker or a clip click).
    pub selected_pattern: Option<String>,
    /// Which GM category column is expanded in the browser sidebar.
    pub browser_category: usize,
    /// JAM mode: hot-reload the project file when it changes on disk.
    pub jam_mode: bool,
    /// Transient status line (save/export/jam/repl feedback).
    pub status: String,
    /// Single-line mini-notation REPL buffer (Code tab).
    pub repl_buf: String,
    /// Last-seen project file mtime, for JAM change detection.
    pub project_mtime: Option<std::time::SystemTime>,
    /// User-adjustable panel sizes (px): left browser width, right mixer width,
    /// bottom editor-dock height. Dragged via the panel splitters.
    pub browser_w: f32,
    pub mixer_w: f32,
    pub dock_h: f32,
    /// Waveform peaks cached by WAV path (computed lazily on first render).
    pub peaks_cache: std::collections::HashMap<String, std::sync::Arc<Vec<f32>>>,
    /// Source-duration (seconds) cache, keyed by WAV path.
    pub peaks_dur_cache: std::collections::HashMap<String, f64>,
    /// Per-track lane heights (px), keyed by track id. Absent = default (TRACK_H).
    pub track_heights: std::collections::HashMap<String, f32>,
    /// Full-window Arrange mode: arrangement fills the window (browser/dock/mixer hidden).
    pub arrange_full: bool,
    /// Arrangement zoom factor; effective bar width = BAR_W * arrange_zoom (clamped in app).
    pub arrange_zoom: f32,
    /// Horizontal scroll offset (px) of the arrange timeline; 0 = bar 1 at the left.
    pub arrange_scroll_x: f32,
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
}

impl HeadlessState {
    pub fn from_project(project: Project, project_path: Option<PathBuf>) -> Self {
        let selected_track = project.tracks.first().map(|t| t.id.clone());
        let selected_pattern = selected_track
            .as_ref()
            .and_then(|tid| project.tracks.iter().find(|t| &t.id == tid))
            .and_then(|t| t.patterns.first().cloned());
        Self {
            project,
            project_path,
            playhead: 0,
            selected_track,
            soloed: HashSet::new(),
            active_dock: DockTab::Sequencer,
            mixer_open: false,
            selected_pattern,
            browser_category: 0,
            jam_mode: false,
            status: String::new(),
            repl_buf: String::new(),
            project_mtime: None,
            browser_w: 200.0,
            mixer_w: 220.0,
            dock_h: 200.0,
            peaks_cache: std::collections::HashMap::new(),
            peaks_dur_cache: std::collections::HashMap::new(),
            track_heights: std::collections::HashMap::new(),
            arrange_full: false,
            arrange_zoom: 1.0,
            arrange_scroll_x: 0.0,
            open_menu: None,
            browser_search: String::new(),
            accent_idx: 0,
            fx_window_open: false,
            selected_return: None,
            clap_plugins: Vec::new(),
            clap_meta: std::collections::HashMap::new(),
            renaming: None,
            rename_buf: String::new(),
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
        self.playhead / TICKS_PER_BAR
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

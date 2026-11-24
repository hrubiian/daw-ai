//! Panel sizes shared by layout, splitter drags and arrangement scrolling.
use super::*;

pub(super) const SPLITTER_H: f32 = 5.;
const MIN_TRACKS_H: f32 = 88.;
const MIN_EDITOR_H: f32 = 124.;
const MIN_MIXER_H: f32 = 184.;

pub(super) fn instrument_browser_width(window_width: f32) -> f32 {
    (window_width - 500.).clamp(260., 360.)
}

pub(super) struct PanelLayout {
    pub mixer_height: Option<f32>,
    pub editor_height: Option<f32>,
}

impl PanelLayout {
    pub fn new(height: f32, state: &HeadlessState, banner: bool) -> Self {
        if state.arrange_full {
            return Self {
                mixer_height: None,
                editor_height: None,
            };
        }

        let chrome = ARRANGE_CHROME_H + if banner { SOUNDFONT_BANNER_H } else { 0. };
        let available =
            (height - chrome - MIN_TRACKS_H - SPLITTER_H).max(0.);
        if !state.mixer_open {
            return Self {
                mixer_height: None,
                editor_height: Some(state.dock_h.clamp(80., available.max(80.))),
            };
        }

        let pair = available - SPLITTER_H;
        if pair >= MIN_MIXER_H + MIN_EDITOR_H {
            let mixer = state.mixer_h.clamp(MIN_MIXER_H, pair - MIN_EDITOR_H);
            return Self {
                mixer_height: Some(mixer),
                editor_height: Some(state.dock_h.clamp(MIN_EDITOR_H, pair - mixer)),
            };
        }

        Self {
            mixer_height: Some((available - editors::EDITOR_TABS_H).max(0.)),
            editor_height: None,
        }
    }

    pub fn bottom_height(&self) -> f32 {
        let mixer = self.mixer_height.map_or(0., |height| height + SPLITTER_H);
        let editor = match self.editor_height {
            Some(height) => height + SPLITTER_H,
            None if self.mixer_height.is_some() => editors::EDITOR_TABS_H,
            None => 0.,
        };
        mixer + editor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_and_editor_leave_room_for_tracks_on_resize() {
        let mut state = HeadlessState::from_project(Project::default_project(), None);
        state.mixer_open = true;
        state.mixer_h = 500.;
        state.dock_h = 500.;
        for (height, banner) in [(448., true), (600., false), (768., false), (1048., true)] {
            let layout = PanelLayout::new(height, &state, banner);
            assert!(layout.mixer_height.is_some());
            assert_eq!(layout.editor_height.is_some(), height >= 600.);
            let chrome = ARRANGE_CHROME_H + if banner { SOUNDFONT_BANNER_H } else { 0. };
            assert!(
                layout.bottom_height() + chrome + MIN_TRACKS_H <= height
            );
        }
        state.arrange_full = true;
        assert_eq!(PanelLayout::new(768., &state, false).bottom_height(), 0.);
    }
}

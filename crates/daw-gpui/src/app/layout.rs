//! Keep the arrangement usable when docks or the desktop window change size.
use super::*;

const MIN_TIMELINE: f32 = 200.;

pub(super) struct PanelLayout {
    pub browser: Option<f32>,
    pub mixer: Option<f32>,
    pub dock_height: f32,
}

impl PanelLayout {
    pub fn new(
        width: f32,
        height: f32,
        browser: Option<f32>,
        mixer: Option<f32>,
        dock: f32,
        banner: bool,
    ) -> Self {
        let mut budget = (width - TRACK_HEADER_W - MIN_TIMELINE).max(0.);
        // Keep the mixer when requested; collapse the browser first on small screens.
        let mixer = mixer.filter(|_| budget >= 145.).map(|w| {
            let w = w.clamp(140., 520.).min(budget - 5.);
            budget -= w + 5.;
            w
        });
        let browser = browser
            .filter(|_| budget >= 125.)
            .map(|w| w.clamp(120., 520.).min(budget - 5.));
        let chrome = 120. + if banner { 22. } else { 0. };
        let max_dock = (height - chrome - RULER_H - LOOP_LANE_H - 100. - 5.).max(80.);
        Self {
            browser,
            mixer,
            dock_height: dock.clamp(80., max_dock),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn docks_leave_room_for_timeline_and_tracks_after_resize() {
        for width in [800., 900., 1280., 1920.] {
            let layout = PanelLayout::new(width, 448., Some(520.), Some(520.), 700., true);
            let used = layout.browser.map_or(0., |w| w + 5.) + layout.mixer.map_or(0., |w| w + 5.);
            assert!(width - used - TRACK_HEADER_W >= MIN_TIMELINE);
            assert!(layout.dock_height + 142. + RULER_H + LOOP_LANE_H + 105. <= 448.);
        }
        let small = PanelLayout::new(800., 448., Some(200.), Some(220.), 200., false);
        assert!(small.mixer.is_none());
        let mid = PanelLayout::new(900., 600., Some(200.), Some(220.), 200., false);
        assert!(mid.browser.is_none());
        assert!(mid.mixer.is_some());
    }
}

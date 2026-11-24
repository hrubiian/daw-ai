use super::*;

pub(super) enum SetupIcon {
    Microphone,
    Keyboard,
}

impl SetupIcon {
    pub(super) fn render(self, color: u32) -> gpui::Div {
        let color = rgb(color);
        let mark = |x, y, width, height| {
            div().absolute().left(px(x)).top(px(y)).w(px(width)).h(px(height))
        };
        let icon = div().relative().size(px(16.));
        match self {
            Self::Microphone => icon
                .child(mark(5., 0., 6., 10.).rounded_full().bg(color))
                .child(mark(3., 6., 10., 7.).border_b_1().border_l_1().border_r_1()
                    .border_color(color).rounded_b(px(5.)))
                .child(mark(7.5, 13., 1., 2.).bg(color))
                .child(mark(5., 15., 6., 1.).bg(color)),
            Self::Keyboard => icon
                .child(mark(0., 2., 16., 12.).rounded_sm().border_1().border_color(color))
                .children([4., 8., 12.].map(|x| mark(x - 0.5, 3., 1., 10.).bg(color)))
                .children([4., 8.].map(|x| mark(x - 1., 3., 2., 6.).bg(color))),
        }
    }
}

impl Theme {
    pub(super) fn cell_btn(
        self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        on: bool,
        on_color: u32,
    ) -> gpui::Stateful<gpui::Div> {
        let id = id.into();
        let label = label.into();
        let help = control_help(id.as_ref()).map(SharedString::from).unwrap_or_else(|| label.clone());
        self.cell_btn_with_help(id, label, on, on_color, help)
    }

    pub(super) fn cell_btn_with_help(
        self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        on: bool,
        on_color: u32,
        help: SharedString,
    ) -> gpui::Stateful<gpui::Div> {
        self.control(id.into(), on, on_color, help).child(label.into())
    }

    pub(super) fn control(
        self,
        id: SharedString,
        on: bool,
        on_color: u32,
        help: SharedString,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id.clone())
            .debug_selector(|| id.to_string())
            .tooltip(move |_w, cx| cx.new(|_| TextTooltip { theme: self, text: help.clone() }).into())
            .hover(|s| s.opacity(0.85))
            .active(|s| s.opacity(0.7))
            .flex_shrink_0()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .whitespace_nowrap()
            .bg(rgb(if on { on_color } else { self.panel_alt }))
            .text_color(rgb(if on { theme::contrast_text(on_color) } else { self.text }))
            .on_click(|_, _, cx| cx.stop_propagation())
    }

    /// Centered placeholder child for dock tabs that aren't yet implemented.
    pub(super) fn dock_hint(self, text: impl Into<SharedString>) -> gpui::Div {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(self.text_dim))
            .child(text.into())
    }

    /// A panel header used atop the browser and mixer docks.
    pub(super) fn section_title(self, text: impl Into<SharedString>) -> gpui::Div {
        div().flex_shrink_0()
            .px_3()
            .py_2()
            .text_color(rgb(self.text_dim))
            .border_b_1()
            .border_color(rgb(self.border))
            .child(text.into())
    }
}

/// Opaque to pointer hit-testing, including wheel events. A painted background
/// alone does not prevent GPUI from dispatching to covered sibling controls.
pub(super) fn overlay_surface() -> gpui::Div {
    div().occlude()
}

/// Readable pan position instead of an internal signed scalar.
pub(super) fn pan_label(pan: f32) -> String {
    if pan.abs() < 0.05 { "Center".into() }
    else { format!("{}{:.0}", if pan < 0.0 { "L" } else { "R" }, pan.abs() * 100.0) }
}

pub(super) fn control_help(id: &str) -> Option<&'static str> {
    let help = match id {
        "sync-session" => "Refresh session; differing local work is saved to a recovery copy first",
        "add-mel" => "Add a MIDI instrument track",
        "add-drum" => "Add a drum track",
        "add-aud" => "Add an audio track",
        "add-wav" => "Import a WAV file at the playhead",
        "arr-left" => "Scroll four bars earlier",
        "arr-right" => "Scroll four bars later",
        "arr-start" => "Scroll to the start",
        "arr-end" => "Scroll to the end of the arrangement",
        "arr-fit" => "Reset timeline zoom and return to the start",
        "key-down" => "Transpose down one semitone",
        "key-up" => "Transpose up one semitone",
        "sw-down" => "Decrease project swing",
        "sw-up" => "Increase project swing",
        "humanize" => "Cycle note velocity variation",
        "tm-down" => "Decrease note timing variation",
        "tm-up" => "Increase note timing variation",
        "metro" => "Toggle the metronome",
        "punch-in" => "Start recording at the loop region start",
        "punch-out" => "Stop recording at the loop region end",
        "panic" => "Silence all sounding notes",
        "mix-balance" => "Analyze track levels and balance the mix (Ctrl+Z to undo)",
        _ if id.starts_with("mix-level-up-") => "Increase level · Shift for fine adjustment",
        _ if id.starts_with("mix-level-down-") => "Decrease level · Shift for fine adjustment",
        _ if id.starts_with("mix-send-up-") => "Increase effect send · Shift for fine adjustment",
        _ if id.starts_with("mix-send-down-") => "Decrease effect send · Shift for fine adjustment",
        _ if id.starts_with("pad-gate-") => "One-shot plays the whole sample · Note length fades it out when the note ends (808s, pads, chops)",
        _ if id.starts_with("m-") => "Mute track",
        _ if id.starts_with("s-") => "Solo track",
        _ if id.starts_with("vd-") => "Decrease track volume",
        _ if id.starts_with("vu-") => "Increase track volume",
        _ if id.starts_with("pd-") => "Pan left",
        _ if id.starts_with("pu-") => "Pan right",
        _ if id.starts_with("tt-") => "Cycle track type",
        _ if id.starts_with("tp-") => "Add a pattern to this track",
        _ if id.starts_with("tx-") => "Remove track (Ctrl+Z to undo)",
        _ => return None,
    };
    Some(help)
}

/// Truncate a string to at most `n` chars.
pub(super) fn trunc(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect()
    }
}

/// Return the note name for a MIDI pitch (e.g. 60 → "C4", 36 → "C2").
pub(super) fn note_name(pitch: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let octave = (pitch / 12) as i32 - 1;
    format!("{}{}", NAMES[(pitch % 12) as usize], octave)
}

/// Short instrument summary shown in a track header.
/// GM preset number → (preset name, category) from the browser catalog, if known.
pub(super) fn gm_preset_name(preset: u8) -> Option<(&'static str, &'static str)> {
    for (cat, presets) in GM_CATEGORIES {
        for (name, prog) in *presets {
            if *prog == preset {
                return Some((name, cat));
            }
        }
    }
    None
}

pub(super) fn instrument_label(t: &daw_core::project::Track) -> String {
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
        Sampler { pads } => format!("Sampler ({} pads, {} takes)", pads.len(), pads.iter().map(|p| p.variants.len()).sum::<usize>()),
        Clap(slot) => slot.id.clone(),
    }
}

/// Three-letter track-type badge.
pub(super) fn track_type_badge(t: &daw_core::project::Track) -> &'static str {
    use daw_core::project::TrackType::*;
    match t.track_type {
        Drum => "DRM",
        Melodic => "MEL",
        Audio => "AUD",
    }
}

/// GM families → (name, program) presets. Compact subset of the 128 GM sounds.
pub(super) const GM_CATEGORIES: &[(&str, &[(&str, u8)])] = &[
    ("Piano", &[("Grand", 0), ("Bright", 1), ("E.Piano", 4), ("Harpsi", 6)]),
    ("Organ", &[("Drawbar", 16), ("Perc", 17), ("Rock", 18), ("Church", 19)]),
    ("Guitar", &[("Nylon", 24), ("Steel", 25), ("Jazz", 26), ("Clean", 27)]),
    ("Bass", &[("Acoustic", 32), ("Finger", 33), ("Pick", 34), ("Synth", 38)]),
    ("Strings", &[("Violin", 40), ("Cello", 42), ("Ensemble", 48), ("Pad", 51)]),
    ("Brass", &[("Trumpet", 56), ("Trombone", 57), ("Sax", 65), ("Section", 61)]),
    ("Synth Lead", &[("Square", 80), ("Saw", 81), ("Calliope", 82), ("Charang", 84)]),
    ("Synth Pad", &[("New Age", 88), ("Warm", 89), ("Sweep", 95), ("Halo", 94)]),
];

/// A small rounded pill used across the transport bar.
pub(super) fn pill(text: impl Into<SharedString>, bg: u32, fg: u32) -> gpui::Div {
    div()
        .flex_shrink_0()
        .whitespace_nowrap()
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .child(text.into())
}

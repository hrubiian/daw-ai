#[derive(Clone, Copy)]
pub(super) struct Theme {
    pub(super) bg: u32,
    pub(super) panel: u32,
    pub(super) panel_alt: u32,
    pub(super) border: u32,
    pub(super) text: u32,
    pub(super) text_dim: u32,
    pub(super) accents: [u32; 3],
    pub(super) playhead: u32,
    pub(super) record_text: u32,
    pub(super) step_beat: u32,
    pub(super) step_playing: u32,
}

pub(super) const ON_CLIP: u32 = 0x16161a;

impl Theme {
    pub(super) const DARK: Self = Self {
        bg: 0x16161a,
        panel: 0x212126,
        panel_alt: 0x2e2e35,
        border: 0x0e0e11,
        text: 0xd6d6db,
        text_dim: 0xa0a0ab,
        accents: [0xff9d4d, 0x4dd0ff, 0x6ee87a],
        playhead: 0x4ade80,
        record_text: 0xff4d47,
        step_beat: 0x383838,
        step_playing: 0x4a4a4a,
    };

    pub(super) const LIGHT: Self = Self {
        bg: 0xffffff,
        panel: 0xf4f4f7,
        panel_alt: 0xe7e8ed,
        border: 0xced0d8,
        text: 0x252531,
        text_dim: 0x575a69,
        accents: [0x9b4300, 0x00617f, 0x17663b],
        playhead: 0x14783d,
        record_text: 0xa62520,
        step_beat: 0xdadce5,
        step_playing: 0xbdc6d8,
    };
}

/// Choose white or near-black text on a saturated control fill.
pub(super) fn contrast_text(fill: u32) -> u32 {
    let linear = |shift: u32| {
        let channel = ((fill >> shift) & 0xff) as f32 / 255.;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(16) + 0.7152 * linear(8) + 0.0722 * linear(0);
    if luminance > 0.19 {
        ON_CLIP
    } else {
        0xffffff
    }
}

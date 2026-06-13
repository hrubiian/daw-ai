use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Rotary knob parameters (KeyLab61 user-preset CCs from Knobs.txt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnobParam {
    Cutoff,    // Knob 1 — CC 74
    Resonance, // Knob 2 — CC 71
    LfoRate,   // Knob 3 — CC 76
    LfoAmt,    // Knob 4 — CC 77
    Chorus,    // Knob 5 — CC 93
    Param1,    // Knob 6 — CC 18
    Param2,    // Knob 7 — CC 19
    Param3,    // Knob 8 — CC 16
    Param4,    // Knob 9 — CC 17
}

/// Linear fader parameters (KeyLab61 user-preset CCs from Faders.txt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaderParam {
    Attack,   // Fader 1 — CC 73
    Decay,    // Fader 2 — CC 75
    Sustain,  // Fader 3 — CC 79
    Release,  // Fader 4 — CC 72
    HiPass,   // Fader 6 — CC 81  (Hi-Pass Filter Frequency)
    Volume,   // Fader 9 — CC 85  (Master)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonAction {
    RecordToggle,
    PlayPause,
    Panic,
    TrackNext,    // Arrow ->  (CC 29)
    TrackPrev,    // Arrow <-  (CC 28)
    TabBrowser,   // Preset    (CC 117)
    EncoderLeft,  // Rolling encoder left  (CC 114, value ≤ 63)
    EncoderRight, // Rolling encoder right (CC 114, value ≥ 65)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ButtonBinding {
    pub cc: u8,
    #[serde(default = "default_min_value")]
    pub min_value: u8,
    /// If set, the CC value must also be ≤ max_value (used for encoder direction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_value: Option<u8>,
}

fn default_min_value() -> u8 { 1 }

// Intermediate type used only by serde; MidiMap is reconstructed via From.
#[derive(Deserialize)]
struct MidiMapRaw {
    #[serde(default = "bool_true")] pitch_bend: bool,
    #[serde(default = "bool_true")] monitor:    bool,
    #[serde(default)]               knobs:      HashMap<KnobParam, u8>,
    #[serde(default)]               faders:     HashMap<FaderParam, u8>,
    #[serde(default)]               buttons:    HashMap<ButtonAction, ButtonBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "MidiMapRaw")]
pub struct MidiMap {
    #[serde(default = "bool_true")] pub pitch_bend: bool,
    #[serde(default = "bool_true")] pub monitor:    bool,
    #[serde(default)]               pub knobs:      HashMap<KnobParam, u8>,
    #[serde(default)]               pub faders:     HashMap<FaderParam, u8>,
    #[serde(default)]               pub buttons:    HashMap<ButtonAction, ButtonBinding>,
    // Reverse maps for O(1) CC → param lookup; not persisted.
    #[serde(skip)] knob_by_cc:  HashMap<u8, KnobParam>,
    #[serde(skip)] fader_by_cc: HashMap<u8, FaderParam>,
}

impl From<MidiMapRaw> for MidiMap {
    fn from(raw: MidiMapRaw) -> Self {
        let knob_by_cc  = raw.knobs.iter().map(|(&k, &c)| (c, k)).collect();
        let fader_by_cc = raw.faders.iter().map(|(&f, &c)| (c, f)).collect();
        MidiMap {
            pitch_bend: raw.pitch_bend,
            monitor:    raw.monitor,
            knobs:      raw.knobs,
            faders:     raw.faders,
            buttons:    raw.buttons,
            knob_by_cc,
            fader_by_cc,
        }
    }
}

fn bool_true() -> bool { true }

impl Default for MidiMap {
    fn default() -> Self {
        let mut knobs = HashMap::new();
        knobs.insert(KnobParam::Cutoff,    74);
        knobs.insert(KnobParam::Resonance, 71);
        knobs.insert(KnobParam::LfoRate,   76);
        knobs.insert(KnobParam::LfoAmt,    77);
        knobs.insert(KnobParam::Chorus,    93);
        knobs.insert(KnobParam::Param1,    18);
        knobs.insert(KnobParam::Param2,    19);
        knobs.insert(KnobParam::Param3,    16);
        knobs.insert(KnobParam::Param4,    17);

        let mut faders = HashMap::new();
        faders.insert(FaderParam::Attack,  73);
        faders.insert(FaderParam::Decay,   75);
        faders.insert(FaderParam::Sustain, 79);
        faders.insert(FaderParam::Release, 72);
        faders.insert(FaderParam::HiPass,  81);
        faders.insert(FaderParam::Volume,  85);

        let mut buttons = HashMap::new();
        buttons.insert(ButtonAction::RecordToggle, ButtonBinding { cc: 116, min_value:   1, max_value: None });
        buttons.insert(ButtonAction::PlayPause,    ButtonBinding { cc: 113, min_value:   1, max_value: None });
        buttons.insert(ButtonAction::Panic,        ButtonBinding { cc: 119, min_value:   1, max_value: None });
        buttons.insert(ButtonAction::TrackNext,    ButtonBinding { cc:  29, min_value: 127, max_value: None });
        buttons.insert(ButtonAction::TrackPrev,    ButtonBinding { cc:  28, min_value: 127, max_value: None });
        buttons.insert(ButtonAction::TabBrowser,   ButtonBinding { cc: 117, min_value:   1, max_value: None });
        buttons.insert(ButtonAction::EncoderLeft,  ButtonBinding { cc: 114, min_value:   1, max_value: Some(63) });
        buttons.insert(ButtonAction::EncoderRight, ButtonBinding { cc: 114, min_value:  65, max_value: None });

        let knob_by_cc  = knobs.iter().map(|(&k, &c)| (c, k)).collect();
        let fader_by_cc = faders.iter().map(|(&f, &c)| (c, f)).collect();
        MidiMap { pitch_bend: true, monitor: true, knobs, faders, buttons, knob_by_cc, fader_by_cc }
    }
}

impl MidiMap {
    /// Loads from `~/.config/daw-ai/midi_map.json`.
    /// Creates the file with defaults if it does not exist or cannot be parsed.
    pub fn load_or_create() -> Self {
        let path = Self::config_path();
        if path.exists() {
            if let Ok(data) = std::fs::read_to_string(&path) {
                if let Ok(map) = serde_json::from_str::<MidiMap>(&data) {
                    return map;
                }
            }
        }
        let default = MidiMap::default();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(&default) {
            let _ = std::fs::write(&path, json);
        }
        default
    }

    fn config_path() -> std::path::PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        std::path::PathBuf::from(home).join(".config/daw-ai/midi_map.json")
    }

    /// Returns the knob parameter mapped to `cc`, if any.
    pub fn knob_for_cc(&self, cc: u8) -> Option<KnobParam> {
        self.knob_by_cc.get(&cc).copied()
    }

    /// Returns the fader parameter mapped to `cc`, if any.
    pub fn fader_for_cc(&self, cc: u8) -> Option<FaderParam> {
        self.fader_by_cc.get(&cc).copied()
    }

    /// Returns true if `cc` is mapped to any continuous controller (knob or fader).
    pub fn is_continuous(&self, cc: u8) -> bool {
        self.knob_for_cc(cc).is_some() || self.fader_for_cc(cc).is_some()
    }

    /// Returns the button action mapped to `cc` if value is within the binding's range.
    pub fn button_for_cc(&self, cc: u8, value: u8) -> Option<ButtonAction> {
        self.buttons
            .iter()
            .find(|(_, b)| {
                b.cc == cc
                    && value >= b.min_value
                    && b.max_value.map_or(true, |max| value <= max)
            })
            .map(|(&a, _)| a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fader_ccs_are_mapped() {
        let map = MidiMap::default();
        assert_eq!(map.fader_for_cc(73), Some(FaderParam::Attack));
        assert_eq!(map.fader_for_cc(75), Some(FaderParam::Decay));
        assert_eq!(map.fader_for_cc(79), Some(FaderParam::Sustain));
        assert_eq!(map.fader_for_cc(72), Some(FaderParam::Release));
        assert_eq!(map.fader_for_cc(81), Some(FaderParam::HiPass));
        assert_eq!(map.fader_for_cc(85), Some(FaderParam::Volume));
    }

    #[test]
    fn knob_ccs_are_mapped() {
        let map = MidiMap::default();
        assert_eq!(map.knob_for_cc(74), Some(KnobParam::Cutoff));
        assert_eq!(map.knob_for_cc(71), Some(KnobParam::Resonance));
        assert_eq!(map.knob_for_cc(76), Some(KnobParam::LfoRate));
        assert_eq!(map.knob_for_cc(77), Some(KnobParam::LfoAmt));
        assert_eq!(map.knob_for_cc(93), Some(KnobParam::Chorus));
        assert_eq!(map.knob_for_cc(18), Some(KnobParam::Param1));
        assert_eq!(map.knob_for_cc(19), Some(KnobParam::Param2));
        assert_eq!(map.knob_for_cc(16), Some(KnobParam::Param3));
        assert_eq!(map.knob_for_cc(17), Some(KnobParam::Param4));
    }

    #[test]
    fn is_continuous_covers_both() {
        let map = MidiMap::default();
        assert!(map.is_continuous(74));  // knob
        assert!(map.is_continuous(73));  // fader
        assert!(!map.is_continuous(99)); // unmapped
    }

    #[test]
    fn unmapped_cc_returns_none() {
        let map = MidiMap::default();
        assert_eq!(map.knob_for_cc(99),  None);
        assert_eq!(map.fader_for_cc(99), None);
    }

    #[test]
    fn button_ccs_are_mapped() {
        let map = MidiMap::default();
        assert_eq!(map.button_for_cc(116, 127), Some(ButtonAction::RecordToggle));
        assert_eq!(map.button_for_cc(113, 127), Some(ButtonAction::PlayPause));
        assert_eq!(map.button_for_cc( 29, 127), Some(ButtonAction::TrackNext));
        assert_eq!(map.button_for_cc( 28, 127), Some(ButtonAction::TrackPrev));
        assert_eq!(map.button_for_cc(117, 127), Some(ButtonAction::TabBrowser));
    }

    #[test]
    fn encoder_left_right_split_by_value() {
        let map = MidiMap::default();
        assert_eq!(map.button_for_cc(114, 63), Some(ButtonAction::EncoderLeft));
        assert_eq!(map.button_for_cc(114, 65), Some(ButtonAction::EncoderRight));
        assert_eq!(map.button_for_cc(114, 64), None); // center — no action
    }

    #[test]
    fn round_trip_serialization() {
        let map = MidiMap::default();
        let json = serde_json::to_string(&map).unwrap();
        let back: MidiMap = serde_json::from_str(&json).unwrap();
        assert_eq!(back.knob_for_cc(74),  Some(KnobParam::Cutoff));
        assert_eq!(back.knob_for_cc(76),  Some(KnobParam::LfoRate));
        assert_eq!(back.fader_for_cc(73), Some(FaderParam::Attack));
        assert_eq!(back.button_for_cc(29, 127), Some(ButtonAction::TrackNext));
        assert_eq!(back.button_for_cc(114, 63), Some(ButtonAction::EncoderLeft));
        assert!(back.pitch_bend);
    }
}

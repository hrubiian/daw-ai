//! Persistent user configuration (input device, channel, count-in, latency
//! trim). JSON at `$XDG_CONFIG_HOME/daw-ai/config.json` (fallback
//! `~/.config/daw-ai/config.json`). Missing or unreadable file = defaults.

use daw_core::audio::input::InputChannels;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UserConfig {
    /// cpal device name; `None` = system default input.
    pub input_device: Option<String>,
    pub input_channels: InputChannels,
    /// Bars of metronome count-in before a recording take (0 = none).
    pub count_in_bars: u8,
    /// Recording latency compensation, cropped off the head of each take.
    pub latency_trim_ms: f32,
    /// Live guitar amp / cabinet monitoring.
    pub guitar: GuitarConfig,
    /// Saved tones exclude interface routing and buffer size.
    pub guitar_presets: Vec<crate::guitar_presets::GuitarPreset>,
    /// Use the desktop GUI's white color scheme.
    pub light_theme: bool,
}

/// Settings for the low-latency instrument monitor. Monitoring itself is
/// never restored on at startup: an open input feeding the speakers must be
/// something the musician switched on in this session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuitarConfig {
    /// Dedicated monitor input. Legacy configs inherit the recording default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<GuitarInput>,
    pub amp: daw_core::audio::amp::AmpSettings,
    /// Cached external tone, retained while the built-in rack is selected.
    pub ampforge: Option<daw_core::project::ClapSlot>,
    pub ampforge_enabled: bool,
    /// Requested device buffer in frames (smaller = lower latency).
    pub buffer_frames: u32,
    /// Play through the input device's own outputs (the interface the
    /// headphones are on) instead of the system default output.
    pub output_same_as_input: bool,
    /// Cabinet WAV path or bundled `builtin:` id, used in IR mode.
    pub ir_path: Option<String>,
    pub cabinet: crate::cabinet::CabinetRecipe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuitarInput {
    pub device: Option<String>,
    pub channel: u16,
}

impl GuitarConfig {
    pub fn active_ampforge(&self) -> anyhow::Result<Option<&daw_core::project::ClapSlot>> {
        if !self.ampforge_enabled { return Ok(None); }
        let slot = self.ampforge.as_ref().ok_or_else(|| anyhow::anyhow!("AmpForge is selected but no plugin is configured"))?;
        anyhow::ensure!(slot.id == daw_core::audio::live_plugin::AMPFORGE_ID, "the guitar backend requires the AmpForge CLAP plugin");
        Ok(Some(slot))
    }
}

impl Default for GuitarConfig {
    fn default() -> Self {
        GuitarConfig { input: None, amp: Default::default(), ampforge: None, ampforge_enabled: false, buffer_frames: 128, output_same_as_input: true, ir_path: None, cabinet: Default::default() }
    }
}

impl Default for UserConfig {
    fn default() -> Self {
        UserConfig {
            input_device: None,
            input_channels: InputChannels::Mono(0),
            count_in_bars: 1,
            latency_trim_ms: 12.0,
            guitar: GuitarConfig::default(),
            guitar_presets: Vec::new(),
            light_theme: false,
        }
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("daw-ai").join("config.json")
}

impl UserConfig {
    pub fn guitar_input(&self) -> GuitarInput {
        self.guitar.input.clone().unwrap_or_else(|| GuitarInput {
            device: self.input_device.clone(),
            channel: match self.input_channels {
                InputChannels::Mono(channel) | InputChannels::Stereo(channel, _) => channel,
            },
        })
    }

    pub fn load_from(path: &Path) -> UserConfig {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn load() -> UserConfig {
        Self::load_from(&config_path())
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&config_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_missing_file_defaults() {
        let dir = std::env::temp_dir().join("daw_cli_config_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let _ = std::fs::remove_file(&path);

        // Missing file → defaults.
        let cfg = UserConfig::load_from(&path);
        assert_eq!(cfg.input_channels, InputChannels::Mono(0));
        assert_eq!(cfg.count_in_bars, 1);
        assert!(cfg.input_device.is_none());

        // Round trip.
        let cfg = UserConfig {
            input_device: Some("UMC404HD 192k".into()),
            input_channels: InputChannels::Mono(0),
            count_in_bars: 2,
            latency_trim_ms: 20.0,
            guitar: GuitarConfig {
                input: Some(GuitarInput { device: None, channel: 3 }),
                amp: daw_core::audio::amp::AmpSettings { gain: 0.8, ..Default::default() },
                buffer_frames: 64, output_same_as_input: true, ir_path: Some("cab.wav".into()),
                ..Default::default()
            },
            guitar_presets: crate::guitar_presets::factory_presets(),
            light_theme: true,
        };
        cfg.save_to(&path).unwrap();
        let loaded = UserConfig::load_from(&path);
        assert_eq!(loaded.input_device.as_deref(), Some("UMC404HD 192k"));
        assert_eq!(loaded.count_in_bars, 2);
        assert_eq!(loaded.latency_trim_ms, 20.0);
        assert_eq!(loaded.guitar, cfg.guitar, "guitar amp settings persist");
        assert_eq!(loaded.guitar_input(), GuitarInput { device: None, channel: 3 });
        assert_eq!(loaded.guitar_presets, cfg.guitar_presets, "named tones persist");
        assert!(loaded.light_theme);

        // A config written before the guitar amp existed still loads.
        std::fs::write(&path, r#"{"count_in_bars":3}"#).unwrap();
        let old = UserConfig::load_from(&path);
        assert!(!old.light_theme);
        assert_eq!(old.guitar_input(), GuitarInput { device: None, channel: 0 });
        assert_eq!((old.count_in_bars, old.guitar), (3, GuitarConfig::default()));

        // Corrupt file → defaults, not a panic.
        std::fs::write(&path, "{not json").unwrap();
        let cfg = UserConfig::load_from(&path);
        assert_eq!(cfg.count_in_bars, 1);
        std::fs::remove_file(&path).ok();
    }
}

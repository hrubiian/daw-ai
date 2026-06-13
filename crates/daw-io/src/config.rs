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
}

impl Default for UserConfig {
    fn default() -> Self {
        UserConfig {
            input_device: None,
            input_channels: InputChannels::Mono(0),
            count_in_bars: 1,
            latency_trim_ms: 12.0,
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
        };
        cfg.save_to(&path).unwrap();
        let loaded = UserConfig::load_from(&path);
        assert_eq!(loaded.input_device.as_deref(), Some("UMC404HD 192k"));
        assert_eq!(loaded.count_in_bars, 2);
        assert_eq!(loaded.latency_trim_ms, 20.0);

        // Corrupt file → defaults, not a panic.
        std::fs::write(&path, "{not json").unwrap();
        let cfg = UserConfig::load_from(&path);
        assert_eq!(cfg.count_in_bars, 1);
        std::fs::remove_file(&path).ok();
    }
}

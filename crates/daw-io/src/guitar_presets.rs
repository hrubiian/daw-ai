//! Portable tones: amp and cabinet, without changing interface routing.
use crate::{cabinet::CabinetRecipe, config::GuitarConfig};
use daw_core::audio::amp::{AmpSettings, Cabinet};
use daw_core::audio::pedals::{DriveMode, DriveSettings, PedalSettings};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuitarPreset {
    pub name: String,
    pub amp: AmpSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ampforge: Option<daw_core::project::ClapSlot>,
    pub ir_path: Option<String>,
    #[serde(default)]
    pub cabinet: CabinetRecipe,
}

impl GuitarPreset {
    pub fn capture(name: String, config: &GuitarConfig) -> Self {
        Self {
            name,
            amp: config.amp,
            ampforge: if config.ampforge_enabled {
                config.ampforge.clone()
            } else {
                None
            },
            ir_path: config.ir_path.clone(),
            cabinet: config.cabinet.clone(),
        }
    }

    pub fn apply(&self, config: &mut GuitarConfig) {
        config.amp = self.amp;
        config.ampforge_enabled = self.ampforge.is_some();
        if self.ampforge.is_some() {
            config.ampforge = self.ampforge.clone();
        }
        config.ir_path = self.ir_path.clone();
        config.cabinet = self.cabinet.clone();
    }

    pub fn matches(&self, config: &GuitarConfig) -> bool {
        self.amp == config.amp
            && self.ampforge.as_ref()
                == if config.ampforge_enabled {
                    config.ampforge.as_ref()
                } else {
                    None
                }
            && self.ir_path == config.ir_path
            && self.cabinet == config.cabinet
    }
}

pub fn factory_presets() -> Vec<GuitarPreset> {
    let make = |name: &str,
                gain,
                bass,
                mid,
                treble,
                gate,
                primary: &str,
                secondary: Option<&str>,
                blend,
                low,
                high| GuitarPreset {
        name: name.into(),
        ampforge: None,
        amp: AmpSettings {
            amp_on: true,
            gain,
            bass,
            mid,
            treble,
            level: 0.35,
            gate,
            cabinet: Cabinet::Ir,
            pedals: Default::default(),
        },
        ir_path: Some(primary.into()),
        cabinet: CabinetRecipe {
            secondary_ir_path: secondary.map(str::to_string),
            blend,
            low_cut_hz: low,
            high_cut_hz: high,
            invert_b: false,
        },
    };
    vec![
        make(
            "Rock",
            0.60,
            0.55,
            0.65,
            0.55,
            0.30,
            "builtin:closed-back-4x12",
            Some("builtin:open-back-1x12"),
            0.25,
            75.0,
            6500.0,
        ),
        make(
            "Jazz",
            0.12,
            0.60,
            0.60,
            0.32,
            0.10,
            "builtin:open-back-1x12",
            None,
            0.0,
            50.0,
            4200.0,
        ),
        make(
            "Metal",
            0.90,
            0.55,
            0.35,
            0.62,
            0.55,
            "builtin:closed-back-4x12",
            None,
            0.0,
            100.0,
            5600.0,
        ),
        thrash_rhythm(),
    ]
}

/// A rhythm-tone approximation for this rig, rather than a model of the
/// original recording chain. The boost removes lows before saturation;
/// cabinet cuts and the mid scoop shape the distorted signal afterwards.
fn thrash_rhythm() -> GuitarPreset {
    GuitarPreset {
        name: "Thrash Rhythm".into(),
        ampforge: None,
        amp: AmpSettings {
            amp_on: true,
            gain: 0.74,
            bass: 0.54,
            mid: 0.24,
            treble: 0.67,
            level: 0.35,
            gate: 0.48,
            cabinet: Cabinet::Ir,
            pedals: PedalSettings {
                drive: DriveSettings {
                    mode: DriveMode::Boost,
                    gain: 0.30,
                    tone: 0.64,
                    level: 0.60,
                },
                // Keep fast palm-muted rhythms clear: no modulation,
                // echoes, or reverb tails between notes.
                ..PedalSettings::BYPASSED
            },
        },
        ir_path: Some("builtin:closed-back-4x12".into()),
        cabinet: CabinetRecipe {
            secondary_ir_path: Some("builtin:open-back-1x12".into()),
            blend: 0.10,
            low_cut_hz: 90.0,
            high_cut_hz: 6200.0,
            invert_b: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_tones_round_trip_and_switching_builtin_keeps_the_cached_plugin() {
        let mut config = GuitarConfig { ampforge_enabled: true, ..Default::default() };
        config.ampforge = Some(daw_core::project::ClapSlot {
            uid: "guitar-ampforge".into(),
            path: "/plugins/ampforge.clap".into(),
            id: daw_core::audio::live_plugin::AMPFORGE_ID.into(),
            state: Some("c3RhdGU=".into()),
            params: [("22".into(), 18.)].into_iter().collect(),
        });
        let saved = GuitarPreset::capture("My AmpForge".into(), &config);
        let restored: GuitarPreset =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert!(restored.matches(&config));
        factory_presets()[0].apply(&mut config);
        assert!(!config.ampforge_enabled);
        assert!(
            config.ampforge.is_some(),
            "switching to built-in retains the external tone"
        );
        assert!(factory_presets()[0].matches(&config));
        restored.apply(&mut config);
        assert!(config.ampforge_enabled);
        assert!(saved.matches(&config));
        assert_eq!(
            config.active_ampforge().unwrap().unwrap().state.as_deref(),
            Some("c3RhdGU=")
        );
        let legacy: GuitarConfig = serde_json::from_str("{}").unwrap();
        assert!(!legacy.ampforge_enabled);
    }

    #[test]
    fn saved_pedal_chains_round_trip_and_old_presets_default_to_bypass() {
        use daw_core::audio::pedals::{PedalPreset, PedalSettings};
        let mut config = GuitarConfig::default();
        for pedals in [
            PedalPreset::LeadEcho,
            PedalPreset::WideClean,
            PedalPreset::JetFlange,
        ] {
            config.amp.pedals = PedalSettings::preset(pedals);
            let saved = GuitarPreset::capture("My pedalboard".into(), &config);
            let text = serde_json::to_string(&saved).unwrap();
            let restored: GuitarPreset = serde_json::from_str(&text).unwrap();
            assert!(restored.matches(&config));
            let mut legacy: serde_json::Value = serde_json::from_str(&text).unwrap();
            legacy["amp"].as_object_mut().unwrap().remove("pedals");
            let old: GuitarPreset = serde_json::from_value(legacy).unwrap();
            assert_eq!(old.amp.pedals, PedalSettings::BYPASSED);
            restored.apply(&mut config);
            assert_eq!(config.amp.pedals, saved.amp.pedals);
        }
    }

    #[test]
    fn factory_and_custom_tones_preserve_routing_and_round_trip() {
        let mut config = GuitarConfig {
            buffer_frames: 256,
            output_same_as_input: false,
            ..Default::default()
        };
        for preset in factory_presets() {
            preset.apply(&mut config);
            assert!(preset.matches(&config));
            assert_eq!(config.buffer_frames, 256);
            assert!(!config.output_same_as_input);
            let ir = crate::cabinet::build_ir(&config, 48000).unwrap();
            assert!(!ir.is_empty());
            let saved = GuitarPreset::capture("My tone".into(), &config);
            let loaded: GuitarPreset =
                serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
            assert!(loaded.matches(&config));
            config.amp.gain = (config.amp.gain + 0.01).min(1.0);
            assert!(!loaded.matches(&config), "edits make the tone custom");
        }
    }

    #[test]
    fn thrash_rhythm_renders_pick_attacks_and_decays_without_clipping() {
        use daw_core::audio::amp::GuitarRig;
        let mut config = GuitarConfig::default();
        thrash_rhythm().apply(&mut config);
        for rate in [44_100, 48_000] {
            let mut rig = GuitarRig::new(rate);
            rig.set(config.amp);
            rig.set_ir(&crate::cabinet::build_ir(&config, rate).unwrap());
            // Original test signal: a low-E harmonic burst, followed by
            // silence. Check DSP stability/headroom, not a subjective match.
            let mut peak = 0f32;
            let mut energy = 0f64;
            let mut tail = 0f32;
            for frame in 0..rate / 2 {
                let time = frame as f32 / rate as f32;
                let input = if time < 0.12 {
                    let phase = std::f32::consts::TAU * 82.41 * time;
                    (phase.sin() + 0.35 * (phase * 2.).sin() + 0.2 * (phase * 3.).sin())
                        * 0.10
                        * (-time * 14.).exp()
                } else {
                    0.0
                };
                let output = rig.process(input);
                assert!(output.is_finite());
                peak = peak.max(output.abs());
                energy += (output * output) as f64;
                if time > 0.4 {
                    tail = tail.max(output.abs());
                }
            }
            assert!(
                peak > 0.02 && energy > 0.1,
                "preset must produce audible output at {rate} Hz"
            );
            assert!(peak < 0.95, "leave output headroom at {rate} Hz: {peak}");
            assert!(
                tail < 0.001,
                "rhythm stops must decay without an effect tail at {rate} Hz: {tail}"
            );
        }
    }
}

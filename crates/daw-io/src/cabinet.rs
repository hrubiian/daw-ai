//! Built-in modeled cabinets and the A/B cabinet recipe. The built-in IRs are
//! synthesized from a short filter chain (no measured or vendor data), so
//! installed binaries need no assets directory; custom WAV paths keep their
//! existing meaning.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CabinetRecipe {
    pub secondary_ir_path: Option<String>,
    /// 0 = A only, 1 = B only. IRs are individually normalized before mixing.
    pub blend: f32,
    pub invert_b: bool,
    pub low_cut_hz: f32,
    pub high_cut_hz: f32,
}

impl Default for CabinetRecipe {
    fn default() -> Self {
        Self {
            secondary_ir_path: None,
            blend: 0.0,
            invert_b: false,
            low_cut_hz: 20.0,
            high_cut_hz: 20000.0,
        }
    }
}

/// Mix and filter once when controls change, leaving one bounded FIR in the
/// live callback. No second convolution or additional block latency.
pub fn build_ir(
    config: &crate::config::GuitarConfig,
    sample_rate: u32,
) -> anyhow::Result<Vec<f32>> {
    let recipe = &config.cabinet;
    anyhow::ensure!(
        recipe.blend.is_finite() && (0.0..=1.0).contains(&recipe.blend),
        "blend must be between 0 and 1"
    );
    anyhow::ensure!(
        recipe.low_cut_hz.is_finite() && (20.0..=400.0).contains(&recipe.low_cut_hz),
        "low cut must be 20–400 Hz"
    );
    anyhow::ensure!(
        recipe.high_cut_hz.is_finite() && (1000.0..=20000.0).contains(&recipe.high_cut_hz),
        "high cut must be 1000–20000 Hz"
    );
    let a = if recipe.blend < 1.0 {
        load_ir(
            config
                .ir_path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("choose IR A"))?,
            sample_rate,
        )?
    } else {
        Vec::new()
    };
    let b = if recipe.blend > 0.0 {
        load_ir(
            recipe
                .secondary_ir_path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("choose IR B before blending"))?,
            sample_rate,
        )?
    } else {
        Vec::new()
    };
    let len = a.len().max(b.len());
    let polarity = if recipe.invert_b { -1.0 } else { 1.0 };
    let mut mixed: Vec<f32> = (0..len)
        .map(|i| {
            a.get(i).copied().unwrap_or(0.0) * (1.0 - recipe.blend)
                + b.get(i).copied().unwrap_or(0.0) * recipe.blend * polarity
        })
        .collect();
    daw_core::audio::amp::filter_cabinet_ir(
        &mut mixed,
        sample_rate,
        recipe.low_cut_hz,
        recipe.high_cut_hz,
    );
    anyhow::ensure!(
        !mixed.is_empty() && mixed.iter().all(|v| v.is_finite()),
        "IR contains invalid samples"
    );
    // Do not normalize a blended result: nearly cancelling phases must stay
    // quiet, rather than being amplified into full-level noise.
    Ok(mixed)
}

/// Export a portable mono 48/44.1/etc. kHz PCM cabinet, without the amp stage.
pub fn write_ir(path: &std::path::Path, ir: &[f32], sample_rate: u32) -> anyhow::Result<()> {
    anyhow::ensure!(
        !ir.is_empty() && ir.iter().all(|v| v.is_finite()),
        "invalid cabinet samples"
    );
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )?;
    for value in ir {
        writer.write_sample((value.clamp(-1.0, 1.0) * 32767.0).round() as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

pub struct CabinetIr {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    model: CabinetModel,
}

/// A modeled cabinet: a speaker/box filter chain plus a few early cabinet
/// reflections `(delay in samples at 48 kHz, gain)`.
struct CabinetModel {
    filters: &'static [Biquad],
    reflections: &'static [(usize, f64)],
}

#[derive(Clone, Copy)]
enum Biquad {
    HighPass(f64, f64),
    LowPass(f64, f64),
    /// Frequency, Q, gain in dB.
    Peak(f64, f64, f64),
}

pub const BUNDLED_IRS: &[CabinetIr] = &[
    CabinetIr {
        id: "builtin:open-back-1x12",
        name: "Open-back 1×12",
        description: "Bright, airy mids and a looser low end",
        model: CabinetModel {
            filters: &[
                Biquad::HighPass(85.0, 0.65),
                Biquad::Peak(180.0, 0.9, 2.0),
                Biquad::Peak(650.0, 1.2, -3.0),
                Biquad::Peak(2400.0, 0.9, 3.0),
                Biquad::LowPass(5600.0, 0.707),
                Biquad::LowPass(5600.0, 0.707),
            ],
            reflections: &[(31, -0.16), (79, 0.10), (157, -0.04)],
        },
    },
    CabinetIr {
        id: "builtin:closed-back-4x12",
        name: "Closed-back 4×12",
        description: "Tight bass, forward mids and a darker top end",
        model: CabinetModel {
            filters: &[
                Biquad::HighPass(100.0, 0.75),
                Biquad::Peak(135.0, 1.1, 4.0),
                Biquad::Peak(480.0, 1.0, -4.0),
                Biquad::Peak(1600.0, 1.0, 3.0),
                Biquad::Peak(3200.0, 1.8, -2.0),
                Biquad::LowPass(3900.0, 0.707),
                Biquad::LowPass(3900.0, 0.707),
            ],
            reflections: &[(19, 0.20), (53, -0.12), (113, 0.06)],
        },
    },
];

const MODEL_RATE: u32 = 48_000;
const MODEL_TAPS: usize = daw_core::audio::amp::MAX_IR_TAPS;

impl CabinetModel {
    /// Impulse response at 48 kHz: filtered impulse, early reflections, a
    /// short fade-out tail and peak normalization.
    fn render(&self) -> Vec<f32> {
        let mut samples = vec![0.0f64; MODEL_TAPS];
        samples[0] = 1.0;
        for filter in self.filters {
            filter.apply(&mut samples);
        }
        let direct = samples.clone();
        for &(delay, gain) in self.reflections {
            for i in delay..MODEL_TAPS {
                samples[i] += gain * direct[i - delay];
            }
        }
        for i in 0..128 {
            samples[MODEL_TAPS - 1 - i] *= i as f64 / 128.0;
        }
        let peak = samples.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        samples.iter().map(|v| (v * 0.9 / peak) as f32).collect()
    }
}

impl Biquad {
    /// RBJ cookbook biquad, transposed direct form II, in place.
    fn apply(self, samples: &mut [f64]) {
        let (freq, q) = match self {
            Biquad::HighPass(f, q) | Biquad::LowPass(f, q) | Biquad::Peak(f, q, _) => (f, q),
        };
        let w = 2.0 * std::f64::consts::PI * freq / MODEL_RATE as f64;
        let (c, s) = (w.cos(), w.sin());
        let alpha = s / (2.0 * q);
        let (b, a) = match self {
            Biquad::HighPass(..) => (
                [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0],
                [1.0 + alpha, -2.0 * c, 1.0 - alpha],
            ),
            Biquad::LowPass(..) => (
                [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0],
                [1.0 + alpha, -2.0 * c, 1.0 - alpha],
            ),
            Biquad::Peak(_, _, gain_db) => {
                let g = 10f64.powf(gain_db / 40.0);
                (
                    [1.0 + alpha * g, -2.0 * c, 1.0 - alpha * g],
                    [1.0 + alpha / g, -2.0 * c, 1.0 - alpha / g],
                )
            }
        };
        let b = b.map(|v| v / a[0]);
        let (a1, a2) = (a[1] / a[0], a[2] / a[0]);
        let (mut z1, mut z2) = (0.0, 0.0);
        for x in samples.iter_mut() {
            let y = b[0] * *x + z1;
            z1 = b[1] * *x - a1 * y + z2;
            z2 = b[2] * *x - a2 * y;
            *x = y;
        }
    }
}

pub fn bundled_ir(source: &str) -> Option<&'static CabinetIr> {
    BUNDLED_IRS.iter().find(|ir| ir.id == source)
}

/// Resolve a saved source at the actual monitor rate, including after restart.
pub fn load_ir(source: &str, sample_rate: u32) -> anyhow::Result<Vec<f32>> {
    if let Some(preset) = bundled_ir(source) {
        let samples = preset.model.render();
        let ir = daw_core::audio::amp::prepare_ir(&samples, MODEL_RATE, sample_rate);
        anyhow::ensure!(!ir.is_empty(), "{} is silent", preset.name);
        Ok(ir)
    } else {
        crate::host::load_cabinet_ir(std::path::Path::new(source), sample_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_endpoints_phase_and_filters_preserve_meaning() {
        let mut config = crate::config::GuitarConfig {
            ir_path: Some(BUNDLED_IRS[0].id.into()),
            ..Default::default()
        };
        config.cabinet.secondary_ir_path = Some(BUNDLED_IRS[1].id.into());
        let a = load_ir(BUNDLED_IRS[0].id, 48000).unwrap();
        let b = load_ir(BUNDLED_IRS[1].id, 48000).unwrap();
        assert_eq!(
            build_ir(&config, 48000).unwrap(),
            a,
            "old single IR stays unchanged"
        );
        config.cabinet.blend = 1.0;
        config.ir_path = Some("/missing/inactive-a.wav".into());
        assert_eq!(
            build_ir(&config, 48000).unwrap(),
            b,
            "100% B ignores inactive A"
        );
        config.ir_path = Some(BUNDLED_IRS[1].id.into());
        config.cabinet.blend = 0.5;
        config.cabinet.invert_b = true;
        let cancelled = build_ir(&config, 48000).unwrap();
        assert!(
            cancelled.iter().all(|v| *v == 0.0),
            "phase cancellation is not normalized away"
        );
        config.cabinet.invert_b = false;
        let flat = build_ir(&config, 48000).unwrap();
        config.cabinet.low_cut_hz = 300.0;
        config.cabinet.high_cut_hz = 2500.0;
        let filtered = build_ir(&config, 48000).unwrap();
        let magnitude = |ir: &[f32], hz: f32| {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, v) in ir.iter().enumerate() {
                let w = 2.0 * std::f32::consts::PI * hz * i as f32 / 48000.0;
                re += v * w.cos();
                im += v * w.sin();
            }
            re.hypot(im)
        };
        assert!(magnitude(&filtered, 80.0) < magnitude(&flat, 80.0) * 0.2);
        assert!(magnitude(&filtered, 8000.0) < magnitude(&flat, 8000.0) * 0.2);
        assert!(filtered.len() <= daw_core::audio::amp::MAX_IR_TAPS);
        config.cabinet.blend = f32::NAN;
        assert!(build_ir(&config, 48000).is_err());
        config.cabinet.blend = 0.5;
        config.cabinet.secondary_ir_path = None;
        assert!(build_ir(&config, 48000).is_err());
    }

    #[test]
    fn exported_custom_cabinet_is_a_reloadable_pcm_wav() {
        let config = crate::guitar_presets::factory_presets().remove(0);
        let mut guitar = crate::config::GuitarConfig::default();
        config.apply(&mut guitar);
        let ir = build_ir(&guitar, 48000).unwrap();
        let path =
            std::env::temp_dir().join(format!("daw-cabinet-export-{}.wav", std::process::id()));
        write_ir(&path, &ir, 48000).unwrap();
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().sample_rate, 48000);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.duration() as usize, ir.len());
        assert!(!load_ir(path.to_str().unwrap(), 44100).unwrap().is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn embedded_cabinets_are_distinct_finite_normalized_and_resampled() {
        for rate in [44100, 48000, 96000] {
            let mut loaded = Vec::new();
            for preset in BUNDLED_IRS {
                let ir = load_ir(preset.id, rate).unwrap();
                assert!(ir.len() <= daw_core::audio::amp::MAX_IR_TAPS);
                assert!(ir.iter().all(|v| v.is_finite()));
                assert!(ir[0].abs() > 0.0, "no leading silence");
                assert!((ir.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-4);
                loaded.push(ir);
            }
            assert_ne!(loaded[0], loaded[1]);
        }
        assert!(load_ir("builtin:missing", 48000).is_err());
        assert!(load_ir("/nonexistent/cab.wav", 48000).is_err());
    }

    #[test]
    fn saved_bundled_selection_reloads_without_an_assets_directory() {
        let mut config = crate::config::UserConfig::default();
        for preset in BUNDLED_IRS {
            config.guitar.ir_path = Some(preset.id.into());
            config.guitar.amp.cabinet = daw_core::audio::amp::Cabinet::Ir;
            let saved = serde_json::to_string(&config).unwrap();
            let restored: crate::config::UserConfig = serde_json::from_str(&saved).unwrap();
            assert_eq!(config.guitar, restored.guitar);
            assert!(!load_ir(restored.guitar.ir_path.as_deref().unwrap(), 48000)
                .unwrap()
                .is_empty());
        }
    }
}

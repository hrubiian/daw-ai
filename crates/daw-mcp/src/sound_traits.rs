//! Measurements of a single auditioned sound — pitch, envelope, brightness,
//! noisiness — for Claude to refine a synth patch against the musician's
//! words. Numbers guide; the musician judges by ear.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct SoundTraits {
    pub pitch_hz: Option<f32>,
    pub cents_off: Option<f32>,
    pub attack_ms: f32,
    pub decay_ms: Option<f32>,
    pub sustain_level: f32,
    pub brightness_hz: f32,
    pub noisiness: f32,
    pub envelope_db: Vec<f32>,
    /// Peak and RMS level in dBFS.
    pub peak_db: f32,
    pub rms_db: f32,
    /// Samples at or past full scale (|x| ≥ 0.999).
    pub clipped_samples: usize,
    /// Fraction of spectral energy below 150 Hz. Near 1.0 means a sound
    /// small speakers barely reproduce.
    pub low_share: f32,
}

/// Low-end share above which a sound may be inaudible on small speakers.
const LOW_WARN: f32 = 0.85;

/// Fine amplitude envelope: max |x| over a 5 ms window, one value per ms.
fn fine_envelope(x: &[f32], sr: u32) -> Vec<f32> {
    let hop = (sr / 1000).max(1) as usize;
    let win = hop * 5;
    (0..x.len().div_ceil(hop))
        .map(|i| {
            let end = ((i + 1) * hop).min(x.len());
            x[end.saturating_sub(win)..end].iter().fold(0.0f32, |m, s| m.max(s.abs()))
        })
        .collect()
}

/// Pitch by normalized autocorrelation over `w` (30–2000 Hz): the shortest
/// lag whose correlation is a local peak within 90% of the best, refined by
/// parabolic interpolation; `None` when the best clarity is under 0.5.
fn detect_pitch(w: &[f32], sr: u32) -> Option<f32> {
    let energy: f32 = w.iter().map(|s| s * s).sum();
    if energy < 1e-9 {
        return None;
    }
    let min_lag = (sr as f32 / 2000.0) as usize;
    let max_lag = ((sr as f32 / 30.0) as usize).min(w.len() / 2);
    if max_lag <= min_lag + 2 {
        return None;
    }
    let r: Vec<f32> = (0..=max_lag + 1)
        .map(|lag| {
            let n = w.len() - lag;
            let c: f32 = (0..n).map(|i| w[i] * w[i + lag]).sum();
            c / energy * w.len() as f32 / n as f32
        })
        .collect();
    let best = r[min_lag..=max_lag].iter().cloned().fold(f32::MIN, f32::max);
    if best < 0.5 {
        return None;
    }
    let lag = (min_lag.max(1)..=max_lag).find(|&l| r[l] >= 0.9 * best && r[l] >= r[l - 1] && r[l] >= r[l + 1])?;
    let (a, b, c) = (r[lag - 1], r[lag], r[lag + 1]);
    let denom = a - 2.0 * b + c;
    let shift = if denom.abs() > 1e-12 { 0.5 * (a - c) / denom } else { 0.0 };
    Some(sr as f32 / (lag as f32 + shift))
}

pub fn sound_traits(mono: &[f32], sr: u32, intended_pitch: Option<u8>) -> SoundTraits {
    let sr = sr.max(1);
    // Coarse envelope for the report: RMS per 50 ms, in dB.
    let block = (sr / 20).max(1) as usize;
    let envelope_db: Vec<f32> = mono
        .chunks(block)
        .map(|c| {
            let rms = (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt();
            20.0 * rms.max(1e-6).log10()
        })
        .collect();

    let env = fine_envelope(mono, sr);
    let peak_i = env.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i).unwrap_or(0);
    let peak = env.get(peak_i).copied().unwrap_or(0.0);
    let (attack_ms, decay_ms, sustain_level) = if peak <= 1e-6 {
        (0.0, None, 0.0)
    } else {
        let t10 = env.iter().position(|v| *v >= 0.1 * peak).unwrap_or(0);
        let t90 = env.iter().position(|v| *v >= 0.9 * peak).unwrap_or(t10);
        let floor = peak * 0.1; // −20 dB
        let decay = env[peak_i..].iter().position(|v| *v <= floor).map(|d| d as f32);
        let tail = &env[env.len() * 2 / 3..];
        let sustain = tail.iter().sum::<f32>() / tail.len().max(1) as f32 / peak;
        (t90.saturating_sub(t10) as f32, decay, sustain)
    };

    // Pitch over the loudest 100 ms.
    let win = (sr / 10) as usize;
    let loudest = mono
        .chunks(block)
        .enumerate()
        .max_by(|a, b| {
            let e = |c: &[f32]| c.iter().map(|s| s * s).sum::<f32>();
            e(a.1).total_cmp(&e(b.1))
        })
        .map(|(i, _)| i * block)
        .unwrap_or(0);
    let start = loudest.min(mono.len().saturating_sub(win));
    let pitch_hz = detect_pitch(&mono[start..(start + win).min(mono.len())], sr);
    let cents_off = match (pitch_hz, intended_pitch) {
        (Some(f), Some(p)) => Some(1200.0 * (f / (440.0 * 2f32.powf((p as f32 - 69.0) / 12.0))).log2()),
        _ => None,
    };

    // Spectral centroid and flatness (Welch power spectrum, DC skipped).
    const FRAME: usize = 2048;
    let power = daw_core::dsp::power_spectrum(mono, FRAME);
    let bin_hz = sr as f32 / FRAME as f32;
    let bands = &power[1..];
    let total: f32 = bands.iter().sum();
    let low_share = if total <= 1e-12 {
        0.0
    } else {
        bands.iter().enumerate().filter(|(k, _)| ((k + 1) as f32) * bin_hz < 150.0).map(|(_, p)| p).sum::<f32>() / total
    };
    let peak = mono.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let rms = (mono.iter().map(|s| s * s).sum::<f32>() / mono.len().max(1) as f32).sqrt();
    let peak_db = 20.0 * peak.max(1e-6).log10();
    let rms_db = 20.0 * rms.max(1e-6).log10();
    let clipped_samples = mono.iter().filter(|s| s.abs() >= 0.999).count();
    let (brightness_hz, noisiness) = if total <= 1e-12 {
        (0.0, 0.0)
    } else {
        let centroid = bands.iter().enumerate().map(|(k, p)| (k + 1) as f32 * bin_hz * p).sum::<f32>() / total;
        let mean = total / bands.len() as f32;
        let log_mean = bands.iter().map(|p| p.max(1e-12).ln()).sum::<f32>() / bands.len() as f32;
        (centroid, (log_mean.exp() / mean).clamp(0.0, 1.0))
    };

    SoundTraits { pitch_hz, cents_off, attack_ms, decay_ms, sustain_level, brightness_hz, noisiness, envelope_db, peak_db, rms_db, clipped_samples, low_share }
}

impl SoundTraits {
    pub fn to_text(&self) -> String {
        let pitch = match (self.pitch_hz, self.cents_off) {
            (Some(f), Some(c)) => format!("{f:.1} Hz ({c:+.0} cents from the intended note)"),
            (Some(f), None) => format!("{f:.1} Hz"),
            _ => "none (unpitched or noisy)".into(),
        };
        let decay = self.decay_ms.map(|d| format!("{d:.0} ms to −20 dB")).unwrap_or_else(|| "doesn't fall 20 dB (held)".into());
        let clip = if self.clipped_samples > 0 {
            format!(" — CLIPPING: {} samples at full scale; lower volume/drive", self.clipped_samples)
        } else {
            String::new()
        };
        let low = if self.low_share > LOW_WARN {
            format!("\nlow end: {:.0}% of the energy is below 150 Hz — may be inaudible on small speakers; add body (raise pitch/cutoff, add harmonics)", self.low_share * 100.0)
        } else {
            format!("\nlow end: {:.0}% of the energy below 150 Hz", self.low_share * 100.0)
        };
        format!(
            "level: peak {:.1} dBFS, RMS {:.1} dBFS{clip}{low}\npitch: {pitch}\nattack: {:.0} ms | decay: {decay} | sustain: {:.0}% of peak\nbrightness: {:.0} Hz centroid | noisiness: {:.2} (0 tonal, 1 noise)\nenvelope (dB per 50 ms): {}",
            self.peak_db,
            self.rms_db,
            self.attack_ms,
            self.sustain_level * 100.0,
            self.brightness_hz,
            self.noisiness,
            self.envelope_db.iter().map(|d| format!("{d:.0}")).collect::<Vec<_>>().join(" ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SR: u32 = 44_100;

    /// 220 Hz sine: 10 ms linear attack, then exponential decay reaching
    /// −20 dB 400 ms after the peak; 1 s long.
    fn plucked_sine() -> Vec<f32> {
        let k = std::f32::consts::LN_10 / 0.4;
        (0..SR as usize).map(|i| {
            let t = i as f32 / SR as f32;
            let env = if t < 0.01 { t / 0.01 } else { (-(t - 0.01) * k).exp() };
            env * (std::f32::consts::TAU * 220.0 * t).sin()
        }).collect()
    }

    fn noise(n: usize) -> Vec<f32> {
        let mut x: u32 = 12345;
        (0..n).map(|_| { x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0 }).collect()
    }

    #[test]
    fn sine_with_attack_and_decay() {
        let t = sound_traits(&plucked_sine(), SR, Some(57));
        let hz = t.pitch_hz.expect("pitch detected");
        assert!((hz - 220.0).abs() <= 2.0, "pitch {hz}");
        assert!(t.cents_off.unwrap().abs() <= 15.0, "cents {:?}", t.cents_off);
        assert!((t.attack_ms - 10.0).abs() <= 5.0, "attack {}", t.attack_ms);
        let d = t.decay_ms.expect("decays 20 dB");
        assert!((d - 400.0).abs() <= 60.0, "decay {d}");
        assert!(t.sustain_level < 0.1, "percussive, sustain {}", t.sustain_level);
        assert_eq!(t.envelope_db.len(), 20, "one value per 50 ms");
        assert!(t.to_text().contains("pitch") && t.to_text().contains("attack"));
    }

    #[test]
    fn brightness_and_noise() {
        let sine: Vec<f32> = (0..SR as usize).map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / SR as f32).sin() * 0.5).collect();
        let t = sound_traits(&sine, SR, None);
        assert!((t.brightness_hz - 1000.0).abs() <= 60.0, "brightness {}", t.brightness_hz);
        assert!(t.noisiness < 0.1, "sine noisiness {}", t.noisiness);
        assert!(t.cents_off.is_none(), "no intended pitch, no cents");
        let n = sound_traits(&noise(SR as usize), SR, None);
        assert!(n.noisiness > 0.5, "noise noisiness {}", n.noisiness);
        assert!(n.pitch_hz.is_none(), "noise has no pitch, got {:?}", n.pitch_hz);
    }

    #[test]
    fn short_sounds_ignore_the_silent_tail() {
        // Renders carry seconds of silence after a short hit; spectral traits
        // must describe the sound, not the silence.
        let mut burst = noise(SR as usize / 10);
        burst.resize(2 * SR as usize, 0.0);
        let n = sound_traits(&burst, SR, None);
        assert!(n.noisiness > 0.5, "noise burst noisiness {}", n.noisiness);
        let mut ping: Vec<f32> = (0..SR as usize / 5).map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / SR as f32).sin() * 0.5).collect();
        ping.resize(2 * SR as usize, 0.0);
        let t = sound_traits(&ping, SR, None);
        assert!((t.brightness_hz - 1000.0).abs() <= 60.0, "ping brightness {}", t.brightness_hz);
    }

    #[test]
    fn level_clipping_and_low_end_are_reported() {
        // A hot 60 Hz tone: clips, and nearly all its energy is sub-150 Hz.
        let boom: Vec<f32> = (0..SR as usize).map(|i| (1.4 * (std::f32::consts::TAU * 60.0 * i as f32 / SR as f32).sin()).clamp(-1.0, 1.0)).collect();
        let t = sound_traits(&boom, SR, None);
        assert!(t.peak_db > -0.1, "peak {}", t.peak_db);
        assert!(t.clipped_samples > 1000, "clipped {}", t.clipped_samples);
        assert!(t.low_share > 0.85, "low share {}", t.low_share);
        let text = t.to_text();
        assert!(text.contains("CLIPPING") && text.contains("small speakers"), "{text}");
        // A moderate 1 kHz tone: no warnings.
        let mid: Vec<f32> = (0..SR as usize).map(|i| 0.25 * (std::f32::consts::TAU * 1000.0 * i as f32 / SR as f32).sin()).collect();
        let m = sound_traits(&mid, SR, None);
        assert!((m.peak_db - (-12.04)).abs() < 0.2, "peak {}", m.peak_db);
        assert_eq!(m.clipped_samples, 0);
        assert!(m.low_share < 0.05, "low share {}", m.low_share);
        assert!(!m.to_text().contains("CLIPPING") && !m.to_text().contains("small speakers"));
    }

    #[test]
    fn silence_is_safe() {
        let t = sound_traits(&vec![0.0; 22_050], SR, Some(60));
        assert!(t.pitch_hz.is_none() && t.decay_ms.is_none());
        assert!(t.attack_ms.is_finite() && t.brightness_hz.is_finite() && t.noisiness.is_finite() && t.sustain_level.is_finite());
        let e = sound_traits(&[], SR, None);
        assert!(e.envelope_db.is_empty());
    }
}

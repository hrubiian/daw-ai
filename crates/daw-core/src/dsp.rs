//! Pure DSP analysis helpers (FFT spectrum) shared by frontends.
use rustfft::{num_complex::Complex, FftPlanner};

/// Hann-windowed magnitude spectrum of `samples`, bucketed onto a log frequency
/// axis from `f_min`..`f_max` into `bins` bars, each normalized 0.0..=1.0 by a dB
/// floor. Returns `vec![0.0; bins]` for empty input.
pub fn spectrum(samples: &[f32], sample_rate: u32, bins: usize,
                f_min: f32, f_max: f32, floor_db: f32) -> Vec<f32> {
    if samples.is_empty() || bins == 0 || sample_rate == 0 { return vec![0.0; bins]; }
    const N: usize = 2048;
    // last N samples, zero-padded at the front if fewer
    let mut buf = vec![Complex { re: 0.0f32, im: 0.0f32 }; N];
    let take = samples.len().min(N);
    let start = samples.len() - take;
    for i in 0..take {
        // Hann window over the filled region's right side
        let w = {
            let n = (N - take + i) as f32;
            0.5 - 0.5 * (std::f32::consts::TAU * n / (N as f32 - 1.0)).cos()
        };
        buf[N - take + i].re = samples[start + i] * w;
    }
    FftPlanner::<f32>::new().plan_fft_forward(N).process(&mut buf);

    let nyq = sample_rate as f32 / 2.0;
    let f_max = f_max.min(nyq).max(f_min + 1.0);
    let bin_hz = sample_rate as f32 / N as f32;
    let mut out = vec![0.0f32; bins];
    for b in 0..bins {
        // log-spaced band [lo, hi)
        let lo = f_min * (f_max / f_min).powf(b as f32 / bins as f32);
        let hi = f_min * (f_max / f_min).powf((b + 1) as f32 / bins as f32);
        let k0 = (lo / bin_hz).floor() as usize;
        let k1 = ((hi / bin_hz).ceil() as usize).max(k0 + 1).min(N / 2);
        let mut mag = 0.0f32;
        for k in k0..k1 { mag = mag.max(buf[k].norm() / (N as f32 / 2.0)); }
        let db = 20.0 * (mag.max(1e-9)).log10();
        out[b] = ((db - floor_db) / -floor_db).clamp(0.0, 1.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectrum_empty_is_zeros() {
        assert_eq!(spectrum(&[], 48000, 32, 20.0, 20000.0, -60.0), vec![0.0; 32]);
    }

    #[test]
    fn spectrum_len_and_range() {
        let s: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
        let out = spectrum(&s, 48000, 48, 20.0, 20000.0, -60.0);
        assert_eq!(out.len(), 48);
        assert!(out.iter().all(|v| *v >= 0.0 && *v <= 1.0));
    }

    #[test]
    fn spectrum_sine_peaks_in_its_band() {
        let sr = 48000u32;
        let freq = 1000.0f32;
        let s: Vec<f32> = (0..4096)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / sr as f32).sin())
            .collect();
        let bins = 48;
        let (f_min, f_max) = (20.0f32, 20000.0f32);
        let out = spectrum(&s, sr, bins, f_min, f_max, -60.0);
        // bucket index whose band contains `freq`
        let peak = (0..bins).max_by(|&a, &b| out[a].partial_cmp(&out[b]).unwrap()).unwrap();
        let lo = f_min * (f_max / f_min).powf(peak as f32 / bins as f32);
        let hi = f_min * (f_max / f_min).powf((peak + 1) as f32 / bins as f32);
        assert!(lo <= freq && freq <= hi, "peak band [{lo},{hi}] should contain {freq}");
        assert!(out[peak] > 0.5, "1kHz sine should be strong, got {}", out[peak]);
    }
}

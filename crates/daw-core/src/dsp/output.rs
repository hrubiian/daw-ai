//! Recent stereo mixer output and analysis for live visualizers.
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::{f32::consts::TAU, sync::Arc};

const FRAMES: usize = 16_384;
pub const SPECTRUM_BARS: usize = 96;
pub const SPECTRUM_MIN_HZ: f32 = 30.0;
pub const SPECTRUM_MAX_HZ: f32 = 20_000.0;
pub const NOTE_NAMES: [&str; 12] = [
    "C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B",
];

/// Fixed-size stereo ring. Writing never allocates or moves older samples.
pub struct OutputHistory {
    frames: Vec<[f32; 2]>,
    next: usize,
    len: usize,
}

impl Default for OutputHistory {
    fn default() -> Self {
        Self {
            frames: vec![[0.0; 2]; FRAMES],
            next: 0,
            len: 0,
        }
    }
}

impl OutputHistory {
    pub fn push(&mut self, left: &[f32], right: Option<&[f32]>) {
        for (i, &left) in left.iter().enumerate() {
            self.frames[self.next] = [left, right.map_or(0.0, |right| right[i])];
            self.next = (self.next + 1) % FRAMES;
        }
        self.len = (self.len + left.len()).min(FRAMES);
    }

    pub fn recent(&self, limit: usize) -> impl Iterator<Item = [f32; 2]> + '_ {
        let count = self.len.min(limit);
        let start = (self.next + FRAMES - count) % FRAMES;
        let end = (start + count).min(FRAMES);
        let wrapped = count - (end - start);
        self.frames[start..end]
            .iter()
            .chain(&self.frames[..wrapped])
            .copied()
    }

    pub fn recent_mono(&self, limit: usize) -> impl Iterator<Item = f32> + '_ {
        self.recent(limit).map(|[left, right]| {
            let mono = (left + right) * 0.5;
            if mono.is_finite() {
                mono
            } else {
                0.0
            }
        })
    }
}

#[derive(Clone)]
pub struct OutputAnalysis {
    pub waveform: Vec<[f32; 2]>,
    pub peak: [f32; 2],
    pub rms: f32,
    pub spectrum: [f32; SPECTRUM_BARS],
    pub dominant_hz: Option<f32>,
    pub pitch_energy: [f32; 12],
}

impl Default for OutputAnalysis {
    fn default() -> Self {
        Self {
            waveform: Vec::new(),
            peak: [0.0; 2],
            rms: 0.0,
            spectrum: [0.0; SPECTRUM_BARS],
            dominant_hz: None,
            pitch_energy: [0.0; 12],
        }
    }
}

impl OutputAnalysis {
    pub fn estimated_triad(&self) -> Option<(usize, bool)> {
        let pitches: Vec<_> = (0..12).filter(|&i| self.pitch_energy[i] >= 0.33).collect();
        if pitches.len() != 3 {
            return None;
        }
        for &root in &pitches {
            for (third, minor) in [(4, false), (3, true)] {
                let mut triad = [root, (root + third) % 12, (root + 7) % 12];
                triad.sort_unstable();
                if pitches == triad {
                    return Some((root, minor));
                }
            }
        }
        None
    }
}

/// Reuses a window, FFT plan and scratch space between snapshots.
pub struct OutputAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    bins: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    magnitudes: Vec<f32>,
}

impl Default for OutputAnalyzer {
    fn default() -> Self {
        let fft = FftPlanner::new().plan_fft_forward(FRAMES);
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        Self {
            fft,
            scratch,
            window: (0..FRAMES)
                .map(|i| 0.5 - 0.5 * (TAU * i as f32 / (FRAMES - 1) as f32).cos())
                .collect(),
            bins: vec![Complex::default(); FRAMES],
            magnitudes: vec![0.0; FRAMES / 2 + 1],
        }
    }
}

impl OutputAnalyzer {
    pub fn analyze(&mut self, samples: &[[f32; 2]], rate: u32) -> OutputAnalysis {
        let mut output = OutputAnalysis::default();
        if samples.is_empty() || rate == 0 {
            return output;
        }
        let recent = &samples[samples.len().saturating_sub((rate / 12) as usize)..];
        let mut sum = 0.0;
        for frame in recent {
            for (peak, &value) in output.peak.iter_mut().zip(frame) {
                *peak = peak.max(value.abs());
                sum += value * value;
            }
        }
        output.rms = (sum / (2 * recent.len()).max(1) as f32).sqrt();
        output
            .waveform
            .extend_from_slice(&samples[samples.len().saturating_sub((rate / 60) as usize)..]);
        if output.rms < 0.0002 {
            return output;
        }

        // Combine the two channel powers after the FFT. A stereo downmix
        // could cancel the very notes and frequency bands we want to show.
        self.magnitudes.fill(0.0);
        let take = samples.len().min(FRAMES);
        let actual = &samples[samples.len() - take..];
        let window_start = (FRAMES - take) / 2;
        let window = &self.window[window_start..window_start + take];
        let scale = 2.0 / window.iter().sum::<f32>();
        for channel in 0..2 {
            self.bins.fill(Complex::default());
            for ((bin, frame), weight) in self.bins.iter_mut().zip(actual).zip(window) {
                bin.re = frame[channel] * weight;
            }
            self.fft
                .process_with_scratch(&mut self.bins, &mut self.scratch);
            for (magnitude, bin) in self.magnitudes.iter_mut().zip(&self.bins) {
                *magnitude += bin.norm_sqr();
            }
        }
        for magnitude in &mut self.magnitudes {
            *magnitude = (*magnitude * 0.5).sqrt() * scale;
        }
        let hz_per_bin = rate as f32 / FRAMES as f32;
        let max_hz = SPECTRUM_MAX_HZ.min(rate as f32 / 2.0);
        if max_hz <= SPECTRUM_MIN_HZ {
            return output;
        }
        for (i, bar) in output.spectrum.iter_mut().enumerate() {
            let edge = |i: usize| {
                SPECTRUM_MIN_HZ
                    * (SPECTRUM_MAX_HZ / SPECTRUM_MIN_HZ).powf(i as f32 / SPECTRUM_BARS as f32)
                    / hz_per_bin
            };
            let low = edge(i).ceil() as usize;
            if low >= self.magnitudes.len() {
                break;
            }
            let high = (edge(i + 1).ceil() as usize)
                .max(low + 1)
                .min(self.magnitudes.len());
            let mag = self.magnitudes[low..high]
                .iter()
                .copied()
                .fold(0.0, f32::max);
            *bar = ((20.0 * mag.max(1e-9).log10() + 72.0) / 72.0).clamp(0.0, 1.0);
        }

        let first = (SPECTRUM_MIN_HZ / hz_per_bin).ceil() as usize;
        let last = (max_hz / hz_per_bin).floor() as usize;
        output.dominant_hz = (first..last)
            .max_by(|&a, &b| self.magnitudes[a].total_cmp(&self.magnitudes[b]))
            .filter(|&bin| self.magnitudes[bin] >= 0.0002)
            .map(|bin| self.peak_frequency(bin) * hz_per_bin);
        output.pitch_energy = self.pitch_classes(hz_per_bin);
        output
    }

    fn peak_frequency(&self, bin: usize) -> f32 {
        if bin == 0 || bin + 1 >= self.magnitudes.len() {
            return bin as f32;
        }
        let log = |i: usize| self.magnitudes[i].max(1e-9).ln();
        let (left, center, right) = (log(bin - 1), log(bin), log(bin + 1));
        let curvature = left - 2.0 * center + right;
        let offset = if curvature.abs() > 1e-6 {
            0.5 * (left - right) / curvature
        } else {
            0.0
        };
        bin as f32 + offset.clamp(-0.5, 0.5)
    }

    fn pitch_classes(&self, hz_per_bin: f32) -> [f32; 12] {
        let mut peaks = Vec::new();
        let start = (65.0 / hz_per_bin).floor().max(1.0) as usize;
        let end = ((2200.0 / hz_per_bin) as usize).min(self.magnitudes.len() - 2);
        let strongest = (start..=end)
            .map(|i| self.magnitudes[i])
            .fold(0.0, f32::max);
        let floor = (strongest * 0.12).max(0.0003);
        for bin in start..=end {
            let mag = self.magnitudes[bin];
            if mag < floor || mag <= self.magnitudes[bin - 1] || mag < self.magnitudes[bin + 1] {
                continue;
            }
            peaks.push((self.peak_frequency(bin) * hz_per_bin, mag));
        }

        let mut chroma = [0.0f32; 12];
        for (i, &(hz, mag)) in peaks.iter().enumerate() {
            // Discount upper partials that line up with a stronger lower tone.
            let partial = peaks[..i].iter().any(|&(lower, amplitude)| {
                let multiple = (hz / lower).round();
                (2.0..=6.0).contains(&multiple)
                    && (hz - lower * multiple).abs() < hz * 0.008
                    && amplitude >= mag
            });
            let midi = 69.0 + 12.0 * (hz / 440.0).log2();
            let note = midi.round();
            if (midi - note).abs() > 0.34 {
                continue;
            }
            let class = (note as i32).rem_euclid(12) as usize;
            chroma[class] = chroma[class].max(if partial { mag * 0.2 } else { mag });
        }
        let strongest = chroma.iter().copied().fold(0.0, f32::max);
        if strongest >= floor {
            for energy in &mut chroma {
                *energy /= strongest;
            }
        } else {
            chroma.fill(0.0);
        }
        chroma
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_a_triad_even_when_stereo_channels_are_out_of_phase() {
        let samples: Vec<_> = (0..FRAMES)
            .map(|i| {
                let phase = TAU * i as f32 / 48000.0;
                let sample = [261.626, 329.628, 391.995]
                    .iter()
                    .map(|hz| (phase * hz).sin() * 0.15)
                    .sum::<f32>();
                [sample, -sample]
            })
            .collect();
        let mut analyzer = OutputAnalyzer::default();
        let analysis = analyzer.analyze(&samples, 48000);
        assert_eq!(analysis.estimated_triad(), Some((0, false)));
        assert!(analysis.spectrum.iter().any(|&peak| peak > 0.6));
        assert!(analysis.peak.iter().all(|&peak| peak > 0.4));
        let quiet = analyzer.analyze(&[[0.0; 2]; 512], 48000);
        assert!(quiet.dominant_hz.is_none());
        assert!(quiet.estimated_triad().is_none());
    }

    #[test]
    fn recent_stereo_output_stays_in_order_when_the_ring_wraps() {
        let mut history = OutputHistory::default();
        history.push(&vec![0.0; FRAMES - 1], None);
        history.push(&[0.2, 0.4, 0.6], Some(&[0.1, 0.3, 0.5]));
        assert_eq!(
            history.recent(3).collect::<Vec<_>>(),
            [[0.2, 0.1], [0.4, 0.3], [0.6, 0.5]]
        );
    }
}

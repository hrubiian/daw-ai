//! Deterministic "listening": numeric analysis of rendered audio.
//! LLMs can't hear — this turns a WAV into peak/RMS/clipping, a per-bar
//! loudness map, stereo width, and a 7-band spectral balance.

pub struct AnalysisReport {
    pub duration_sec: f64,
    pub peak_db: f32,
    pub rms_db: f32,
    pub clipped_samples: usize,
    /// (bar number starting at 1, RMS dBFS) — empty/overloaded bars at a glance.
    pub bar_rms_db: Vec<(u32, f32)>,
    /// L/R correlation: +1 mono … 0 wide … −1 phase trouble.
    pub stereo_correlation: f32,
    /// (band name, average dB) — 7 bands, sub → air.
    pub bands: Vec<(&'static str, f32)>,
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-7).log10()
}

const SPEC_FLOOR_DB: f32 = -90.0;
const SPEC_BINS: usize = 64;
const SPEC_F_LO: f32 = 20.0;
const SPEC_F_HI: f32 = 16_000.0;
const SPEC_WIN: usize = 2048;

pub fn analyze(l: &[f32], r: &[f32], sample_rate: u32, bpm: f64) -> AnalysisReport {
    let n = l.len().min(r.len()).max(1);
    let mut peak = 0f32;
    let mut sum_sq = 0f64;
    let mut clipped = 0usize;
    let mut dot = 0f64;
    let mut l_sq = 0f64;
    let mut r_sq = 0f64;
    for i in 0..n.min(l.len()) {
        let (a, b) = (l[i], r[i]);
        peak = peak.max(a.abs()).max(b.abs());
        sum_sq += (a as f64 * a as f64 + b as f64 * b as f64) / 2.0;
        if a.abs() >= 0.999 || b.abs() >= 0.999 {
            clipped += 1;
        }
        dot += a as f64 * b as f64;
        l_sq += a as f64 * a as f64;
        r_sq += b as f64 * b as f64;
    }
    let rms = (sum_sq / n as f64).sqrt() as f32;
    let corr = if l_sq > 1e-12 && r_sq > 1e-12 {
        (dot / (l_sq.sqrt() * r_sq.sqrt())) as f32
    } else {
        1.0
    };

    // Per-bar RMS (mono mix; 4/4 bars).
    let spb = 240.0 / bpm.max(1.0);
    let bar_len = (spb * sample_rate as f64) as usize;
    let mut bar_rms_db = Vec::new();
    if bar_len > 0 {
        let bars = n.div_ceil(bar_len);
        for bi in 0..bars {
            let lo = bi * bar_len;
            let hi = ((bi + 1) * bar_len).min(n);
            let s: f64 = (lo..hi)
                .map(|i| {
                    let m = (l[i] + r[i]) / 2.0;
                    m as f64 * m as f64
                })
                .sum();
            bar_rms_db.push((bi as u32 + 1, db((s / (hi - lo).max(1) as f64).sqrt() as f32)));
        }
    }

    // 7-band summary: average daw-core spectra over consecutive windows
    // (spectrum() analyzes only its last 2048 samples and returns values
    // normalized 0..1 against SPEC_FLOOR_DB — invert to dB after averaging).
    let mono: Vec<f32> = (0..n).map(|i| (l[i] + r[i]) / 2.0).collect();
    let mut acc = vec![0f32; SPEC_BINS];
    let mut windows = 0usize;
    let mut pos = 0usize;
    while pos < mono.len() {
        let end = (pos + SPEC_WIN).min(mono.len());
        let spec = daw_core::dsp::spectrum(&mono[pos..end], sample_rate, SPEC_BINS, SPEC_F_LO, SPEC_F_HI, SPEC_FLOOR_DB);
        for (a, v) in acc.iter_mut().zip(spec) {
            *a += v;
        }
        windows += 1;
        pos = end;
    }
    if windows > 0 {
        for a in &mut acc {
            *a /= windows as f32;
        }
    }
    const BANDS: [(&str, f32, f32); 7] = [
        ("sub", 20.0, 60.0),
        ("bass", 60.0, 150.0),
        ("low-mid", 150.0, 500.0),
        ("mid", 500.0, 2_000.0),
        ("high-mid", 2_000.0, 6_000.0),
        ("presence", 6_000.0, 10_000.0),
        ("air", 10_000.0, 16_000.0),
    ];
    let bands = BANDS
        .iter()
        .map(|(name, lo, hi)| {
            // spectrum buckets are log-spaced SPEC_F_LO..SPEC_F_HI.
            let vals: Vec<f32> = acc
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    let frac = (*i as f32 + 0.5) / SPEC_BINS as f32;
                    let f = SPEC_F_LO * (SPEC_F_HI / SPEC_F_LO).powf(frac);
                    f >= *lo && f < *hi
                })
                .map(|(_, v)| v * -SPEC_FLOOR_DB + SPEC_FLOOR_DB) // 0..1 → dB
                .collect();
            let avg = if vals.is_empty() {
                SPEC_FLOOR_DB
            } else {
                vals.iter().sum::<f32>() / vals.len() as f32
            };
            (*name, avg)
        })
        .collect();

    AnalysisReport {
        duration_sec: n as f64 / sample_rate.max(1) as f64,
        peak_db: db(peak),
        rms_db: db(rms),
        clipped_samples: clipped,
        bar_rms_db,
        stereo_correlation: corr,
        bands,
    }
}

/// Analyze a WAV file at its native sample rate.
pub fn analyze_wav_file(path: &std::path::Path, bpm: f64) -> Result<AnalysisReport, String> {
    let sr = hound::WavReader::open(path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?
        .spec()
        .sample_rate;
    let (l, r) = daw_core::audio::load_wav_stereo(path, sr).map_err(|e| e.to_string())?;
    Ok(analyze(&l, &r, sr, bpm))
}

impl AnalysisReport {
    pub fn to_text(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "duration {:.2}s | peak {:.1} dBFS | RMS loudness {:.1} dBFS{}",
            self.duration_sec,
            self.peak_db,
            self.rms_db,
            if self.clipped_samples > 0 {
                format!(" | CLIPPING: {} samples", self.clipped_samples)
            } else {
                String::new()
            }
        );
        let _ = writeln!(
            s,
            "stereo correlation {:+.2} (+1 mono, 0 wide, <0 phase trouble)",
            self.stereo_correlation
        );
        let _ = writeln!(
            s,
            "spectral balance (avg dB): {}",
            self.bands
                .iter()
                .map(|(n, v)| format!("{n} {v:.0}"))
                .collect::<Vec<_>>()
                .join(" · ")
        );
        let _ = write!(s, "per-bar RMS dBFS:");
        for (bar, v) in &self.bar_rms_db {
            let _ = write!(s, " {bar}:{v:.0}");
        }
        let _ = writeln!(s);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, sr: u32, secs: f32, amp: f32) -> (Vec<f32>, Vec<f32>) {
        let n = (sr as f32 * secs) as usize;
        let v: Vec<f32> = (0..n)
            .map(|i| (i as f32 / sr as f32 * freq * std::f32::consts::TAU).sin() * amp)
            .collect();
        (v.clone(), v)
    }

    #[test]
    fn sine_minus6_pins_levels_and_band() {
        let sr = 44_100;
        let (l, r) = sine(440.0, sr, 2.0, 0.501); // ≈ -6 dBFS peak
        let rep = analyze(&l, &r, sr, 120.0);
        assert!((rep.peak_db - (-6.0)).abs() < 0.2, "peak {}", rep.peak_db);
        assert!((rep.rms_db - (-9.0)).abs() < 0.5, "rms {}", rep.rms_db);
        assert_eq!(rep.clipped_samples, 0);
        assert!((rep.stereo_correlation - 1.0).abs() < 1e-3, "L==R fully correlated");
        let max_band = rep.bands.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        assert_eq!(max_band.0, "low-mid", "440 Hz dominates low-mid, got {:?}", rep.bands);
    }

    #[test]
    fn clipping_and_silence_map() {
        let sr = 44_100;
        // 1 bar at 120bpm = 2.0s. Bar 1 = clipped, bar 2 = silence.
        let bar = (sr as f32 * 2.0) as usize;
        let mut l = vec![1.5f32; bar];
        l.extend(vec![0.0f32; bar]);
        let r = l.clone();
        let rep = analyze(&l, &r, sr, 120.0);
        assert!(rep.clipped_samples > 0);
        assert_eq!(rep.bar_rms_db.len(), 2, "{:?}", rep.bar_rms_db);
        assert!(rep.bar_rms_db[0].1 > -1.0, "hot bar: {:?}", rep.bar_rms_db);
        assert!(rep.bar_rms_db[1].1 < -90.0, "silent bar: {:?}", rep.bar_rms_db);
        let text = rep.to_text();
        assert!(text.contains("CLIPPING"), "{text}");
    }
}

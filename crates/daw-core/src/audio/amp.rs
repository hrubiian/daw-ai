//! Guitar amp + speaker cabinet for live monitoring.
//!
//! The dry path is sample-by-sample, with no block latency: filters are IIR
//! biquads and the optional impulse response is convolved directly in the
//! time domain (no FFT blocks). Time-based pedals add intentional wet delays.
//! The monitor stream in `monitor.rs` owns
//! one `GuitarRig` and calls `process` per input sample.
//!
//! Signal path: noise gate → drive pedal → input high-pass → gain → two soft-clip stages
//! (2× oversampled) → bass/mid/treble → cabinet (built-in model or loaded
//! IR) → modulation → echo → reverb → level → safety clip.

use serde::{Deserialize, Serialize};
use super::pedals::{GuitarPedals, PedalSettings};

/// Longest impulse response convolved (≈43 ms at 48 kHz). Longer files are
/// truncated with a short fade: a guitar cabinet's useful response is shorter.
pub const MAX_IR_TAPS: usize = 2048;

/// Which speaker simulation follows the amp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cabinet {
    /// No speaker simulation (for an external cab sim, or a clean DI).
    Off,
    /// Built-in 4×12-style voicing made of filters; needs no files.
    #[default]
    BuiltIn,
    /// A loaded impulse response (falls back to `BuiltIn` until one is set).
    Ir,
}

/// Amp knobs use `0..=1` (tone controls are flat at 0.5); pedal settings
/// include explicit Hz/ms values and their own bounds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AmpSettings {
    /// Amp section on. Off passes the gated input straight to the cabinet.
    pub amp_on: bool,
    /// Drive: 0 is clean, 1 is heavy saturation.
    pub gain: f32,
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    /// Output level; 0.5 is roughly unity for a clean signal.
    pub level: f32,
    /// Noise-gate threshold: 0 = off, 1 = closes below about -30 dBFS.
    pub gate: f32,
    pub cabinet: Cabinet,
    pub pedals: PedalSettings,
}

impl Default for AmpSettings {
    fn default() -> Self {
        AmpSettings { amp_on: true, gain: 0.45, bass: 0.5, mid: 0.5, treble: 0.5, level: 0.5, gate: 0.25, cabinet: Cabinet::BuiltIn, pedals: PedalSettings::BYPASSED }
    }
}

// ── filters ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Biquad { b0: f32, b1: f32, b2: f32, a1: f32, a2: f32, z1: f32, z2: f32 }

impl Biquad {
    fn identity() -> Self { Biquad { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: 0.0, z2: 0.0 } }

    fn norm(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Biquad { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, z1: 0.0, z2: 0.0 }
    }

    fn wc(freq: f32, sr: f32) -> (f32, f32) {
        let w = 2.0 * std::f32::consts::PI * (freq / sr).clamp(1e-5, 0.49);
        (w.cos(), w.sin())
    }

    fn lowpass(freq: f32, q: f32, sr: f32) -> Self {
        let (c, s) = Self::wc(freq, sr); let alpha = s / (2.0 * q);
        Self::norm((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    fn highpass(freq: f32, q: f32, sr: f32) -> Self {
        let (c, s) = Self::wc(freq, sr); let alpha = s / (2.0 * q);
        Self::norm((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    fn peaking(freq: f32, gain_db: f32, q: f32, sr: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let (c, s) = Self::wc(freq, sr); let alpha = s / (2.0 * q);
        Self::norm(1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a)
    }

    fn low_shelf(freq: f32, gain_db: f32, sr: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let (c, s) = Self::wc(freq, sr);
        let beta = 2.0 * a.sqrt() * s / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        Self::norm(
            a * ((a + 1.0) - (a - 1.0) * c + beta), 2.0 * a * ((a - 1.0) - (a + 1.0) * c), a * ((a + 1.0) - (a - 1.0) * c - beta),
            (a + 1.0) + (a - 1.0) * c + beta, -2.0 * ((a - 1.0) + (a + 1.0) * c), (a + 1.0) + (a - 1.0) * c - beta)
    }

    fn high_shelf(freq: f32, gain_db: f32, sr: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let (c, s) = Self::wc(freq, sr);
        let beta = 2.0 * a.sqrt() * s / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        Self::norm(
            a * ((a + 1.0) + (a - 1.0) * c + beta), -2.0 * a * ((a - 1.0) + (a + 1.0) * c), a * ((a + 1.0) + (a - 1.0) * c - beta),
            (a + 1.0) - (a - 1.0) * c + beta, 2.0 * ((a - 1.0) - (a + 1.0) * c), (a + 1.0) - (a - 1.0) * c - beta)
    }

    /// New coefficients, keeping the filter's state (no click on a knob turn).
    fn retune(&mut self, to: Biquad) {
        let (z1, z2) = (self.z1, self.z2);
        *self = to; self.z1 = z1; self.z2 = z2;
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        // Flush denormals: a decaying tail must not slow the callback down.
        if self.z1.abs() < 1e-20 { self.z1 = 0.0; }
        if self.z2.abs() < 1e-20 { self.z2 = 0.0; }
        y
    }
}

/// Asymmetric soft clip: `tanh(x + b) - tanh(b)` rewritten with the tanh
/// addition identity so that zero in gives exactly zero out (the subtracted
/// form leaves a 1e-10 offset that would keep a silent rig "sounding").
/// The bias adds even harmonics; output stays inside (-1, 1).
#[inline]
fn shape(x: f32) -> f32 {
    const T: f32 = 0.178; // tanh(0.18)
    let th = x.tanh();
    th * (1.0 - T) / (1.0 + T * th)
}

/// Resample (linear) and trim an impulse response for `GuitarRig::set_ir`.
/// Returns at most `MAX_IR_TAPS` taps at `to_sr`, normalised so the cabinet
/// does not change the overall level much, with a fade over the last 64 taps.
pub fn prepare_ir(samples: &[f32], from_sr: u32, to_sr: u32) -> Vec<f32> {
    if samples.is_empty() || from_sr == 0 || to_sr == 0 { return Vec::new(); }
    // Skip leading silence so the IR's own pre-delay adds no latency.
    let peak = samples.iter().fold(0f32, |a, x| a.max(x.abs()));
    if peak <= 0.0 { return Vec::new(); }
    let start = samples.iter().position(|x| x.abs() >= peak * 0.02).unwrap_or(0);
    let src = &samples[start..];
    let ratio = from_sr as f64 / to_sr as f64;
    let n = (((src.len() as f64) / ratio) as usize).clamp(1, MAX_IR_TAPS);
    let mut out: Vec<f32> = (0..n).map(|i| {
        let pos = i as f64 * ratio; let k = pos as usize; let f = (pos - k as f64) as f32;
        let a = src.get(k).copied().unwrap_or(0.0); let b = src.get(k + 1).copied().unwrap_or(0.0);
        a + (b - a) * f
    }).collect();
    let fade = 64.min(n);
    for i in 0..fade { out[n - 1 - i] *= i as f32 / fade as f32; }
    // Normalise by energy: unit-energy IRs keep broadband level about equal.
    let energy = out.iter().map(|x| x * x).sum::<f32>().sqrt();
    if energy > 0.0 { for x in &mut out { *x /= energy; } }
    out
}

// ── the rig ─────────────────────────────────────────────────────────────────

/// Bake cabinet cuts into FIR taps off the audio thread. The boundary values
/// bypass their filters, keeping older single-IR configurations unchanged.
pub fn filter_cabinet_ir(ir: &mut [f32], sample_rate: u32, low_hz: f32, high_hz: f32) {
    let sr = sample_rate.max(8000) as f32;
    let mut hp = Biquad::highpass(low_hz.clamp(20.0, 400.0), 0.707, sr);
    let mut lp = Biquad::lowpass(high_hz.clamp(1000.0, 20000.0), 0.707, sr);
    for value in ir.iter_mut() {
        if low_hz > 20.0 { *value = hp.process(*value); }
        if high_hz < 20000.0 { *value = lp.process(*value); }
    }
}

pub struct GuitarRig {
    sr: f32,
    settings: AmpSettings,
    // gate
    env: f32,
    gate_gain: f32,
    gate_open: bool,
    // amp
    input_hp: Biquad,
    prev_in: f32,
    inter_lp: f32,
    inter_lp_k: f32,
    dc_x: f32,
    dc_y: f32,
    pre_gain: f32,
    stage2_gain: f32,
    makeup: f32,
    tone: [Biquad; 3],
    // cabinet
    cab: [Biquad; 6],
    ir: Vec<f32>,
    ir_line: Vec<f32>,
    ir_pos: usize,
    level: f32,
    pedals: GuitarPedals,
}

impl GuitarRig {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000) as f32;
        let mut rig = GuitarRig {
            sr, settings: AmpSettings::default(),
            env: 0.0, gate_gain: 1.0, gate_open: true,
            input_hp: Biquad::highpass(90.0, 0.707, sr), prev_in: 0.0,
            inter_lp: 0.0, inter_lp_k: 1.0 - (-2.0 * std::f32::consts::PI * 6500.0 / (2.0 * sr)).exp(),
            dc_x: 0.0, dc_y: 0.0, pre_gain: 1.0, stage2_gain: 1.0, makeup: 1.0,
            tone: [Biquad::identity(); 3],
            cab: [
                Biquad::highpass(75.0, 0.9, sr),
                Biquad::peaking(110.0, 3.5, 1.2, sr),
                Biquad::peaking(420.0, -3.0, 1.0, sr),
                Biquad::peaking(2600.0, 5.0, 1.4, sr),
                Biquad::lowpass(5000.0, 0.8, sr),
                Biquad::lowpass(5000.0, 0.8, sr),
            ],
            ir: Vec::new(), ir_line: vec![0.0; MAX_IR_TAPS], ir_pos: 0,
            level: 1.0,
            pedals: GuitarPedals::new(sample_rate),
        };
        rig.set(AmpSettings::default());
        rig
    }

    pub fn settings(&self) -> AmpSettings { self.settings }

    /// Apply new control values. Cheap enough to call from the audio callback
    /// when a control changes; filter state is kept, so turns do not click.
    pub fn set(&mut self, s: AmpSettings) {
        let c = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.5 };
        let s = AmpSettings { gain: c(s.gain), bass: c(s.bass), mid: c(s.mid), treble: c(s.treble), level: c(s.level), gate: c(s.gate), pedals: s.pedals.sanitized(), ..s };
        self.settings = s;
        self.pedals.set(s.pedals);
        // -6 dB (clean) .. +40 dB into the first stage.
        self.pre_gain = 10f32.powf((s.gain * 46.0 - 6.0) / 20.0);
        self.stage2_gain = 1.0 + s.gain * 3.0;
        // More drive means a hotter, more compressed signal: pull it back.
        self.makeup = 1.0 / (1.0 + s.gain * 1.6);
        let sr = self.sr;
        self.tone[0].retune(Biquad::low_shelf(120.0, (s.bass - 0.5) * 24.0, sr));
        self.tone[1].retune(Biquad::peaking(700.0, (s.mid - 0.5) * 20.0, 0.7, sr));
        self.tone[2].retune(Biquad::high_shelf(3000.0, (s.treble - 0.5) * 24.0, sr));
        self.level = s.level * 2.0;
    }

    /// Install an impulse response already at this rig's sample rate (see
    /// `prepare_ir`). An empty slice removes it. Not for the audio callback:
    /// build the rig-side buffer elsewhere and swap it with `swap_ir`.
    pub fn set_ir(&mut self, ir: &[f32]) {
        self.ir = ir[..ir.len().min(MAX_IR_TAPS)].to_vec();
        self.ir_line.iter_mut().for_each(|x| *x = 0.0);
    }

    /// Swap in a prepared IR buffer, returning the old one so the caller can
    /// drop it off the audio thread.
    pub fn swap_ir(&mut self, mut ir: Vec<f32>) -> Vec<f32> {
        ir.truncate(MAX_IR_TAPS);
        self.ir_line.iter_mut().for_each(|x| *x = 0.0);
        std::mem::replace(&mut self.ir, ir)
    }

    pub fn has_ir(&self) -> bool { !self.ir.is_empty() }

    #[inline]
    fn gate(&mut self, x: f32) -> f32 {
        let g = self.settings.gate;
        if g <= 0.0 { return x; }
        // Fast-attack / slow-release envelope of the input.
        let a = x.abs();
        let k = if a > self.env { 0.02 } else { 1.0 / (0.08 * self.sr) };
        self.env += k * (a - self.env);
        let threshold = 10f32.powf((-80.0 + g * 50.0) / 20.0);
        // Hysteresis: opens at the threshold, closes 6 dB below it.
        if self.env > threshold { self.gate_open = true; } else if self.env < threshold * 0.5 { self.gate_open = false; }
        let target = if self.gate_open { 1.0 } else { 0.0 };
        let rate = if target > self.gate_gain { 1.0 / (0.002 * self.sr) } else { 1.0 / (0.06 * self.sr) };
        self.gate_gain += (target - self.gate_gain).clamp(-rate, rate);
        x * self.gate_gain
    }

    #[inline]
    fn amp(&mut self, x: f32) -> f32 {
        let x = self.input_hp.process(x) * self.pre_gain;
        // 2× oversampling of the clipper: shape the interpolated midpoint and
        // the sample, low-pass at the doubled rate, keep the second output.
        let mut y = 0.0;
        for input in [(self.prev_in + x) * 0.5, x] {
            let s1 = shape(input);
            self.inter_lp += self.inter_lp_k * (s1 - self.inter_lp);
            y = shape(self.inter_lp * self.stage2_gain);
        }
        self.prev_in = x;
        // DC blocker (the asymmetric clipper adds an offset).
        let out = y - self.dc_x + 0.995 * self.dc_y;
        self.dc_x = y; self.dc_y = out;
        let mut t = out * self.makeup;
        for f in &mut self.tone { t = f.process(t); }
        t
    }

    #[inline]
    fn cabinet(&mut self, x: f32) -> f32 {
        match self.settings.cabinet {
            Cabinet::Off => x,
            Cabinet::Ir if !self.ir.is_empty() => {
                let n = self.ir_line.len(); // power of two
                self.ir_pos = (self.ir_pos + 1) & (n - 1);
                self.ir_line[self.ir_pos] = x;
                // Direct convolution, newest sample first: no block delay.
                let (head, tail) = self.ir_line.split_at(self.ir_pos + 1);
                let mut acc = 0.0f32;
                let first = head.len().min(self.ir.len());
                for (h, s) in self.ir[..first].iter().zip(head.iter().rev()) { acc += h * s; }
                for (h, s) in self.ir[first..].iter().zip(tail.iter().rev()) { acc += h * s; }
                acc
            }
            _ => { let mut y = x; for f in &mut self.cab { y = f.process(y); } y }
        }
    }

    /// One input sample in, one output sample out; dry audio has no block delay.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        let gated = self.gate(x);
        let g = self.pedals.pre_amp(gated);
        let a = if self.settings.amp_on { self.amp(g) } else { g };
        let cab = self.cabinet(a);
        let c = self.pedals.post_cabinet(cab) * self.level;
        // Safety: never hand the output more than full scale.
        if c.abs() > 0.9 { c.signum() * (0.9 + 0.1 * ((c.abs() - 0.9) * 10.0).tanh()) } else { c }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / 48000.0).sin() * amp).collect()
    }
    /// Magnitude of `freq` in `x` (Goertzel-style correlation).
    fn mag(x: &[f32], freq: f32) -> f32 {
        let (mut re, mut im) = (0f32, 0f32);
        for (i, v) in x.iter().enumerate() {
            let p = 2.0 * std::f32::consts::PI * freq * i as f32 / 48000.0;
            re += v * p.cos(); im += v * p.sin();
        }
        2.0 * (re * re + im * im).sqrt() / x.len() as f32
    }
    fn run(rig: &mut GuitarRig, x: &[f32]) -> Vec<f32> { x.iter().map(|&v| rig.process(v)).collect() }
    fn rig(s: AmpSettings) -> GuitarRig { let mut r = GuitarRig::new(48000); r.set(s); r }
    const OPEN: AmpSettings = AmpSettings { amp_on: true, gain: 0.0, bass: 0.5, mid: 0.5, treble: 0.5, level: 0.5, gate: 0.0, cabinet: Cabinet::Off, pedals: PedalSettings::BYPASSED };

    #[test]
    fn silence_stays_silent_and_output_is_bounded() {
        let mut r = rig(AmpSettings { gain: 1.0, level: 1.0, ..AmpSettings::default() });
        let quiet = run(&mut r, &[0.0; 4096]);
        assert!(quiet.iter().all(|v| *v == 0.0), "first non-zero: {:?}", quiet.iter().enumerate().find(|(_, v)| **v != 0.0));
        let loud: Vec<f32> = sine(110.0, 8.0, 9600);
        let out = run(&mut r, &loud);
        assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 1.0), "peak {}", out.iter().fold(0f32, |a, v| a.max(v.abs())));
        assert!(run(&mut r, &[f32::NAN, f32::INFINITY, 0.0]).iter().all(|v| v.is_finite()));
    }

    #[test]
    fn the_rig_adds_no_latency() {
        // An impulse must come out on the very sample it went in, with the
        // amp and either cabinet engaged.
        for cabinet in [Cabinet::Off, Cabinet::BuiltIn, Cabinet::Ir] {
            let mut r = rig(AmpSettings { cabinet, ..OPEN });
            r.set_ir(&prepare_ir(&[0.0, 0.0, 0.0, 1.0, 0.5, -0.25, 0.1], 48000, 48000));
            let mut x = vec![0.0f32; 64]; x[0] = 0.5;
            let out = run(&mut r, &x);
            assert!(out[0].abs() > 1e-4, "{cabinet:?}: first output sample {}", out[0]);
        }
    }

    #[test]
    fn gain_adds_harmonics_and_clean_stays_clean() {
        let x = sine(220.0, 0.2, 48000);
        let thd = |gain: f32| {
            let mut r = rig(AmpSettings { gain, ..OPEN });
            let out = run(&mut r, &x);
            let tail = &out[24000..];
            (mag(tail, 440.0) + mag(tail, 660.0)) / mag(tail, 220.0)
        };
        let (clean, driven) = (thd(0.0), thd(0.9));
        assert!(clean < 0.06, "clean setting distorts: {clean}");
        assert!(driven > 0.25 && driven > clean * 5.0, "drive adds harmonics: {driven} vs {clean}");
    }

    #[test]
    fn built_in_cabinet_rolls_off_fizz_and_rumble() {
        let level = |freq: f32, cabinet: Cabinet| {
            let mut r = rig(AmpSettings { amp_on: false, cabinet, ..OPEN });
            let out = run(&mut r, &sine(freq, 0.1, 24000));
            mag(&out[12000..], freq)
        };
        let db = |a: f32, b: f32| 20.0 * (a / b).log10();
        assert!((db(level(1000.0, Cabinet::Off), 0.1)).abs() < 0.2, "no cabinet is transparent");
        assert!(db(level(10000.0, Cabinet::BuiltIn), level(1000.0, Cabinet::BuiltIn)) < -20.0, "fizz above 5 kHz is cut");
        assert!(db(level(30.0, Cabinet::BuiltIn), level(1000.0, Cabinet::BuiltIn)) < -10.0, "sub rumble is cut");
        assert!(db(level(2600.0, Cabinet::BuiltIn), level(1000.0, Cabinet::BuiltIn)) > 2.0, "presence peak");
    }

    #[test]
    fn tone_controls_boost_and_cut_their_bands() {
        let level = |freq: f32, s: AmpSettings| {
            let mut r = rig(s);
            let out = run(&mut r, &sine(freq, 0.02, 24000));
            mag(&out[12000..], freq)
        };
        let flat = level(100.0, OPEN);
        assert!(level(100.0, AmpSettings { bass: 1.0, ..OPEN }) > flat * 2.0);
        assert!(level(100.0, AmpSettings { bass: 0.0, ..OPEN }) < flat * 0.5);
        let flat = level(6000.0, OPEN);
        assert!(level(6000.0, AmpSettings { treble: 1.0, ..OPEN }) > flat * 2.0);
        let flat = level(700.0, OPEN);
        assert!(level(700.0, AmpSettings { mid: 0.0, ..OPEN }) < flat * 0.6);
    }

    #[test]
    fn gate_closes_on_hiss_and_opens_for_playing() {
        let mut r = rig(AmpSettings { gate: 0.5, ..OPEN }); // threshold -55 dBFS
        let hiss = sine(3000.0, 0.0005, 24000); // -66 dBFS
        let out = run(&mut r, &hiss);
        assert!(out[20000..].iter().all(|v| v.abs() < 1e-6), "hiss is gated out");
        let out = run(&mut r, &sine(196.0, 0.1, 4800));
        assert!(out[200..].iter().fold(0f32, |a, v| a.max(v.abs())) > 0.03, "a played note opens it within a few ms");
        // Gate off passes the hiss.
        let mut r = rig(OPEN);
        assert!(run(&mut r, &hiss)[20000..].iter().fold(0f32, |a, v| a.max(v.abs())) > 1e-4);
    }

    #[test]
    fn impulse_response_is_trimmed_resampled_and_convolved() {
        // Leading silence is removed, length is capped, energy is normalised.
        let mut raw = vec![0.0f32; 500]; raw.extend((0..8000).map(|i| (0.97f32).powi(i) * if i % 2 == 0 { 1.0 } else { -0.4 }));
        let ir = prepare_ir(&raw, 96000, 48000);
        assert!(ir.len() <= MAX_IR_TAPS && ir.len() > 1000);
        assert!(ir[0].abs() > 0.01, "starts at the first real sample");
        assert!((ir.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 0.05);
        assert!(prepare_ir(&[0.0; 64], 48000, 48000).is_empty() && prepare_ir(&[], 48000, 48000).is_empty());
        // Convolving with a two-tap IR gives exactly that FIR's output.
        let mut r = rig(AmpSettings { amp_on: false, cabinet: Cabinet::Ir, ..OPEN });
        r.set_ir(&[0.5, 0.25]);
        let out = run(&mut r, &[1.0, 0.0, 0.0, 0.8, 0.0]);
        for (got, want) in out.iter().zip([0.5, 0.25, 0.0, 0.4, 0.2]) { assert!((got - want).abs() < 1e-6, "{out:?}"); }
        // The delay line wraps correctly over many samples.
        let old = r.swap_ir(vec![1.0]);
        assert_eq!(old, vec![0.5, 0.25]);
        let x = sine(440.0, 0.3, 5000);
        for (a, b) in run(&mut r, &x).iter().zip(&x) { assert!((a - b).abs() < 1e-6); }
    }

    #[test]
    fn settings_round_trip_and_tolerate_missing_fields() {
        let s: AmpSettings = serde_json::from_str(r#"{"gain":0.8,"cabinet":"ir"}"#).unwrap();
        assert_eq!((s.gain, s.cabinet, s.bass, s.amp_on), (0.8, Cabinet::Ir, 0.5, true));
        assert_eq!(serde_json::from_str::<AmpSettings>(&serde_json::to_string(&s).unwrap()).unwrap(), s);
    }
}

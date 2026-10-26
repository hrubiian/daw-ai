//! Per-track mixer graph (slice A): each track renders into its own strip —
//! synth / sampler pads / audio-clip voices → insert FX → gain/pan/mute —
//! then strips sum into a master bus with a soft limiter and meters.
//!
//! This module is cpal-free: the live engine callback and the offline WAV
//! exporter both drive the same `Mixer`, so exports sound like playback by
//! construction. See `docs/superpowers/specs/2026-06-10-mixer-graph-slice-a-design.md`.

use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

/// Strip index that routes to the utility synth (metronome, previews).
pub const STRIP_UTILITY: u8 = u8::MAX;
/// Hard cap on mixer strips; tracks beyond this are silent.
pub const MAX_STRIPS: usize = 32;

// ── Biquad filter (Audio EQ Cookbook, TDF-II, f64 coefficients) ───────────────

struct Biquad {
    b0: f64, b1: f64, b2: f64,
    a1: f64, a2: f64,
    z1: f64, z2: f64,
}

impl Biquad {
    fn identity() -> Self {
        Biquad { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: 0.0, z2: 0.0 }
    }

    fn process(&mut self, x: f32) -> f32 {
        let xd = x as f64;
        let y = self.b0 * xd + self.z1;
        self.z1 = self.b1 * xd - self.a1 * y + self.z2;
        self.z2 = self.b2 * xd - self.a2 * y;
        y as f32
    }

    fn low_shelf(freq: f32, gain_db: f32, sr: f32) -> Self {
        if gain_db.abs() < 0.01 { return Self::identity(); }
        let a      = (10.0f64).powf(gain_db as f64 / 40.0);
        let w0     = 2.0 * PI * freq as f64 / sr as f64;
        let cos_w  = w0.cos();
        let alpha  = w0.sin() / 2.0 * 2.0f64.sqrt();
        let b0 =   a * ((a+1.0) - (a-1.0)*cos_w + 2.0*a.sqrt()*alpha);
        let b1 = 2.0*a*((a-1.0) - (a+1.0)*cos_w);
        let b2 =   a * ((a+1.0) - (a-1.0)*cos_w - 2.0*a.sqrt()*alpha);
        let a0 =       (a+1.0) + (a-1.0)*cos_w + 2.0*a.sqrt()*alpha;
        let a1 =  -2.0*((a-1.0) + (a+1.0)*cos_w);
        let a2 =       (a+1.0) + (a-1.0)*cos_w - 2.0*a.sqrt()*alpha;
        Biquad { b0: b0/a0, b1: b1/a0, b2: b2/a0, a1: a1/a0, a2: a2/a0, z1: 0.0, z2: 0.0 }
    }

    fn high_shelf(freq: f32, gain_db: f32, sr: f32) -> Self {
        if gain_db.abs() < 0.01 { return Self::identity(); }
        let a      = (10.0f64).powf(gain_db as f64 / 40.0);
        let w0     = 2.0 * PI * freq as f64 / sr as f64;
        let cos_w  = w0.cos();
        let alpha  = w0.sin() / 2.0 * 2.0f64.sqrt();
        let b0 =   a * ((a+1.0) + (a-1.0)*cos_w + 2.0*a.sqrt()*alpha);
        let b1 =-2.0*a*((a-1.0) + (a+1.0)*cos_w);
        let b2 =   a * ((a+1.0) + (a-1.0)*cos_w - 2.0*a.sqrt()*alpha);
        let a0 =       (a+1.0) - (a-1.0)*cos_w + 2.0*a.sqrt()*alpha;
        let a1 =   2.0*((a-1.0) - (a+1.0)*cos_w);
        let a2 =       (a+1.0) - (a-1.0)*cos_w - 2.0*a.sqrt()*alpha;
        Biquad { b0: b0/a0, b1: b1/a0, b2: b2/a0, a1: a1/a0, a2: a2/a0, z1: 0.0, z2: 0.0 }
    }

    fn peaking(freq: f32, gain_db: f32, q: f32, sr: f32) -> Self {
        if gain_db.abs() < 0.01 { return Self::identity(); }
        let a     = (10.0f64).powf(gain_db as f64 / 40.0);
        let w0    = 2.0 * PI * freq as f64 / sr as f64;
        let alpha = w0.sin() / (2.0 * q as f64);
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * w0.cos();
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a2 = 1.0 - alpha / a;
        Biquad { b0: b0/a0, b1: b1/a0, b2: b2/a0, a1: b1/a0, a2: a2/a0, z1: 0.0, z2: 0.0 }
    }
}

// ── Compressor (peak follower, soft-knee gain) ────────────────────────────────

struct Compressor {
    threshold: f32,
    ratio:     f32,
    attack:    f32,
    release:   f32,
    envelope:  f32,
}

impl Compressor {
    fn bypass() -> Self {
        Compressor { threshold: 1.0, ratio: 1.0, attack: 0.0, release: 0.0, envelope: 0.0 }
    }

    fn new(threshold_db: f32, ratio: f32, attack_ms: f32, release_ms: f32, sr: f32) -> Self {
        let threshold = 10.0f32.powf(threshold_db / 20.0);
        let attack    = if attack_ms  > 0.0 { (-1.0 / (attack_ms  * 0.001 * sr)).exp() } else { 0.0 };
        let release   = if release_ms > 0.0 { (-1.0 / (release_ms * 0.001 * sr)).exp() } else { 0.0 };
        Compressor { threshold, ratio: ratio.max(1.0), attack, release, envelope: 0.0 }
    }

    fn process(&mut self, x: f32) -> f32 {
        let x_abs = x.abs();
        self.envelope = if x_abs > self.envelope {
            self.attack * self.envelope + (1.0 - self.attack) * x_abs
        } else {
            self.release * self.envelope + (1.0 - self.release) * x_abs
        };
        if self.ratio <= 1.0 || self.envelope <= self.threshold { return x; }
        let env_db = 20.0 * (self.envelope + 1e-30).log10();
        let thr_db = 20.0 * (self.threshold + 1e-30).log10();
        let gr_db  = (env_db - thr_db) * (1.0 - 1.0 / self.ratio);
        x * 10.0f32.powf(-gr_db / 20.0)
    }
}

// ── EQ + compressor block (stereo) ────────────────────────────────────────────

struct EqComp {
    low_l: Biquad, low_r: Biquad,
    mid_l: Biquad, mid_r: Biquad,
    hi_l:  Biquad, hi_r:  Biquad,
    comp_l: Compressor, comp_r: Compressor,
}

impl EqComp {
    fn bypass() -> Self {
        EqComp {
            low_l: Biquad::identity(), low_r: Biquad::identity(),
            mid_l: Biquad::identity(), mid_r: Biquad::identity(),
            hi_l:  Biquad::identity(), hi_r:  Biquad::identity(),
            comp_l: Compressor::bypass(), comp_r: Compressor::bypass(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn update(&mut self,
              eq_low: f32, eq_mid: f32, eq_high: f32, mid_freq: f32,
              thr_db: f32, ratio: f32, atk_ms: f32, rel_ms: f32, sr: f32) {
        self.low_l = Biquad::low_shelf(200.0, eq_low, sr);
        self.low_r = Biquad::low_shelf(200.0, eq_low, sr);
        let mf = mid_freq.clamp(200.0, 8000.0);
        self.mid_l = Biquad::peaking(mf, eq_mid, 0.7, sr);
        self.mid_r = Biquad::peaking(mf, eq_mid, 0.7, sr);
        self.hi_l  = Biquad::high_shelf(6000.0, eq_high, sr);
        self.hi_r  = Biquad::high_shelf(6000.0, eq_high, sr);
        if ratio > 1.0 && thr_db < 0.0 {
            self.comp_l = Compressor::new(thr_db, ratio, atk_ms, rel_ms, sr);
            self.comp_r = Compressor::new(thr_db, ratio, atk_ms, rel_ms, sr);
        } else {
            self.comp_l = Compressor::bypass();
            self.comp_r = Compressor::bypass();
        }
    }

    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let l = self.hi_l.process(self.mid_l.process(self.low_l.process(l)));
        let r = self.hi_r.process(self.mid_r.process(self.low_r.process(r)));
        (self.comp_l.process(l), self.comp_r.process(r))
    }
}

// ── Insert chain (full per-strip FX: EQ → comp → drive → LP → delay) ─────────

/// Full per-strip insert chain configured from a track's `EffectChain`.
/// A freshly-constructed chain is bit-transparent (verified by test).
pub struct InsertChain {
    eq_comp:        EqComp,
    drive:          f32,
    lp_alpha:       f32, // 1.0 = fully open (bypass)
    lp_l:           f32,
    lp_r:           f32,
    delay_buf_l:    Vec<f32>,
    delay_buf_r:    Vec<f32>,
    delay_pos:      usize,
    delay_samples:  usize,
    delay_feedback: f32,
    delay_mix:      f32,
    sample_rate:    f32,
}

impl InsertChain {
    pub fn new(sample_rate: u32) -> Self {
        let max_delay = (sample_rate as usize) * 2 + 1;
        InsertChain {
            eq_comp:        EqComp::bypass(),
            drive:          0.0,
            lp_alpha:       1.0,
            lp_l:           0.0,
            lp_r:           0.0,
            delay_buf_l:    vec![0.0; max_delay],
            delay_buf_r:    vec![0.0; max_delay],
            delay_pos:      0,
            delay_samples:  0,
            delay_feedback: 0.4,
            delay_mix:      0.0,
            sample_rate:    sample_rate as f32,
        }
    }

    /// Map a track's effect chain onto the DSP stages.
    pub fn configure(&mut self, fx: &crate::project::EffectChain) {
        self.configure_raw(
            fx.delay_time_ms, fx.delay_feedback, fx.delay_mix, fx.cutoff, fx.drive,
            fx.eq_low_gain, fx.eq_mid_gain, fx.eq_high_gain, fx.eq_mid_freq,
            fx.comp_threshold, fx.comp_ratio, fx.comp_attack_ms, fx.comp_release_ms,
        );
    }

    /// Loose-parameter form, shared with the legacy `SetClipEffects` event.
    #[allow(clippy::too_many_arguments)]
    pub fn configure_raw(&mut self,
        delay_time_ms: u32, delay_feedback: f32, delay_mix: f32, cutoff: f32, drive: f32,
        eq_low_gain: f32, eq_mid_gain: f32, eq_high_gain: f32, eq_mid_freq: f32,
        comp_threshold: f32, comp_ratio: f32, comp_attack_ms: f32, comp_release_ms: f32,
    ) {
        self.eq_comp.update(
            eq_low_gain, eq_mid_gain, eq_high_gain, eq_mid_freq,
            comp_threshold, comp_ratio, comp_attack_ms, comp_release_ms,
            self.sample_rate,
        );
        self.drive          = drive.clamp(0.0, 1.0);
        self.lp_alpha       = cutoff.clamp(0.0, 1.0).powi(2);
        self.delay_samples  = (((delay_time_ms as f32 / 1000.0) * self.sample_rate) as usize)
                                  .min(self.delay_buf_l.len().saturating_sub(1));
        self.delay_feedback = delay_feedback.clamp(0.0, 0.95);
        self.delay_mix      = delay_mix.clamp(0.0, 1.0);
    }

    /// Process one frame. Stage order mirrors the pre-mixer engine clip path
    /// (EQ+comp → drive → low-pass → ping-pong delay) so audio tracks keep
    /// their exact sound.
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let (mut l, mut r) = self.eq_comp.process(l, r);

        if self.drive > 0.0 {
            let gain = 1.0 + self.drive * 9.0;
            l = (l * gain).tanh();
            r = (r * gain).tanh();
        }

        // One-pole low-pass (alpha = 1.0 → bypass)
        self.lp_l += self.lp_alpha * (l - self.lp_l);
        self.lp_r += self.lp_alpha * (r - self.lp_r);
        l = self.lp_l;
        r = self.lp_r;

        if self.delay_samples > 0 && self.delay_mix > 0.0 {
            let max_delay = self.delay_buf_l.len();
            let read = (self.delay_pos + max_delay - self.delay_samples) % max_delay;
            let dl = self.delay_buf_l[read];
            let dr = self.delay_buf_r[read];
            self.delay_buf_l[self.delay_pos] = l + dl * self.delay_feedback;
            self.delay_buf_r[self.delay_pos] = r + dr * self.delay_feedback;
            l = l * (1.0 - self.delay_mix) + dl * self.delay_mix;
            r = r * (1.0 - self.delay_mix) + dr * self.delay_mix;
            self.delay_pos = (self.delay_pos + 1) % max_delay;
        }

        (l, r)
    }
}

// ── Meters + master limiter ───────────────────────────────────────────────────

/// Lock-free per-strip meter, shared with the UI.
pub struct StripMeter {
    /// Post-fader per-block max-abs (f32 bits).
    pub peak: AtomicU32,
    /// Latched at >= 0.999; the UI resets it.
    pub clipped: AtomicBool,
}

impl StripMeter {
    pub fn new() -> Self {
        StripMeter { peak: AtomicU32::new(0f32.to_bits()), clipped: AtomicBool::new(false) }
    }
    pub fn store(&self, peak: f32) {
        self.peak.store(peak.to_bits(), Ordering::Relaxed);
        if peak >= 0.999 {
            self.clipped.store(true, Ordering::Relaxed);
        }
    }
    pub fn read(&self) -> (f32, bool) {
        (f32::from_bits(self.peak.load(Ordering::Relaxed)), self.clipped.load(Ordering::Relaxed))
    }
    pub fn reset_clip(&self) {
        self.clipped.store(false, Ordering::Relaxed);
    }
}

impl Default for StripMeter {
    fn default() -> Self {
        Self::new()
    }
}

/// Soft master limiter: transparent below 0.5, tanh-shaped above, |out| < 1.0
/// (ceiling 0.999 — f32 tanh saturates to exactly 1.0 for large inputs).
/// Continuous at the knee: f(x) = sign(x)·(0.5 + 0.499·tanh(2·(|x| − 0.5))).
pub fn master_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= 0.5 {
        x
    } else {
        x.signum() * (0.5 + 0.499 * ((a - 0.5) * 2.0).tanh())
    }
}

// ── Return-bus DSP: Freeverb + wet-only ping-pong delay (slice B) ─────────────

/// Original Freeverb tunings (44.1 kHz), scaled at construction.
const COMB_TUNINGS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNINGS: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;
const FIXED_GAIN: f32 = 0.015;

struct Comb {
    buf: Vec<f32>,
    pos: usize,
    filter_store: f32,
    feedback: f32,
    damp1: f32,
    damp2: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Comb {
            buf: vec![0.0; len.max(1)],
            pos: 0,
            filter_store: 0.0,
            feedback: 0.84,
            damp1: 0.2,
            damp2: 0.8,
        }
    }
    fn process(&mut self, input: f32) -> f32 {
        let output = self.buf[self.pos];
        self.filter_store = output * self.damp2 + self.filter_store * self.damp1;
        self.buf[self.pos] = input + self.filter_store * self.feedback;
        self.pos = (self.pos + 1) % self.buf.len();
        output
    }
}

struct Allpass {
    buf: Vec<f32>,
    pos: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Allpass { buf: vec![0.0; len.max(1)], pos: 0 }
    }
    fn process(&mut self, input: f32) -> f32 {
        let bufout = self.buf[self.pos];
        self.buf[self.pos] = input + bufout * 0.5;
        self.pos = (self.pos + 1) % self.buf.len();
        bufout - input
    }
}

/// Public-domain Freeverb (Schroeder/Moorer): 8 combs + 4 allpasses per
/// channel, right channel offset by STEREO_SPREAD samples. Output is 100%
/// wet — the dry path is the strip → master sum (send-bus convention).
pub struct Freeverb {
    combs_l: Vec<Comb>,
    combs_r: Vec<Comb>,
    aps_l: Vec<Allpass>,
    aps_r: Vec<Allpass>,
    wet1: f32,
    wet2: f32,
    tone: super::spatial::ToneFilter,
    predelay: super::spatial::Predelay,
}

impl Freeverb {
    pub fn new(sample_rate: u32) -> Self {
        let scale = sample_rate as f64 / 44_100.0;
        let sz = |n: usize| ((n as f64 * scale).round() as usize).max(1);
        let mut fv = Freeverb {
            combs_l: COMB_TUNINGS.iter().map(|&n| Comb::new(sz(n))).collect(),
            combs_r: COMB_TUNINGS.iter().map(|&n| Comb::new(sz(n + STEREO_SPREAD))).collect(),
            aps_l: ALLPASS_TUNINGS.iter().map(|&n| Allpass::new(sz(n))).collect(),
            aps_r: ALLPASS_TUNINGS.iter().map(|&n| Allpass::new(sz(n + STEREO_SPREAD))).collect(),
            wet1: 0.75,
            wet2: 0.25,
            tone: super::spatial::ToneFilter::new(sample_rate),
            predelay: super::spatial::Predelay::new(sample_rate),
        };
        fv.set(0.5, 0.5, 1.0);
        fv
    }

    #[cfg(test)]
    fn new_set(sample_rate: u32, room: f32, damp: f32, width: f32) -> Self {
        let mut fv = Self::new(sample_rate);
        fv.set(room, damp, width);
        fv
    }

    /// Map 0..1 params onto the comb network (original Freeverb scalings).
    pub fn set(&mut self, room_size: f32, damping: f32, width: f32) {
        let feedback = 0.7 + 0.28 * room_size.clamp(0.0, 1.0);
        let damp1 = damping.clamp(0.0, 1.0) * 0.4;
        for c in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            c.feedback = feedback;
            c.damp1 = damp1;
            c.damp2 = 1.0 - damp1;
        }
        let w = width.clamp(0.0, 1.0);
        self.wet1 = w / 2.0 + 0.5;
        self.wet2 = (1.0 - w) / 2.0;
    }

    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let (l, r) = self.tone.process(l, r);
        let input = self.predelay.process((l + r) * FIXED_GAIN);
        let mut out_l = 0f32;
        let mut out_r = 0f32;
        for c in &mut self.combs_l {
            out_l += c.process(input);
        }
        for c in &mut self.combs_r {
            out_r += c.process(input);
        }
        for a in &mut self.aps_l {
            out_l = a.process(out_l);
        }
        for a in &mut self.aps_r {
            out_r = a.process(out_r);
        }
        (
            out_l * self.wet1 + out_r * self.wet2,
            out_r * self.wet1 + out_l * self.wet2,
        )
    }
}

/// Wet-only ping-pong delay: mono-summed input enters the left line; each
/// line's output crosses to the other attenuated by `feedback`, so echoes
/// alternate L→R→L and decay.
pub struct PingPongDelay {
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    pos: usize,
    delay_samples: usize,
    feedback: f32,
    sample_rate: f32,
    mode: crate::project::EchoMode,
    width: f32,
    modulation_samples: f32,
    phase: f32,
    phase_step: f32,
    tone: super::spatial::ToneFilter,
    previous_delay: usize,
    fade: usize,
    started: bool,
}

impl PingPongDelay {
    pub fn new(sample_rate: u32) -> Self {
        // A whole note at the minimum supported 20 BPM is 12 seconds.
        let max = (sample_rate as usize) * 12 + (sample_rate as usize) / 50 + 2;
        let mut d = PingPongDelay {
            buf_l: vec![0.0; max],
            buf_r: vec![0.0; max],
            pos: 0,
            delay_samples: 0,
            feedback: 0.35,
            sample_rate: sample_rate as f32,
            mode: crate::project::EchoMode::PingPong, width: 1., modulation_samples: 0.,
            phase: 0., phase_step: 0., tone: super::spatial::ToneFilter::new(sample_rate),
            previous_delay: 0, fade: 0, started: false,
        };
        d.set(300, 0.35);
        d
    }

    pub fn set(&mut self, time_ms: u32, feedback: f32) {
        let samples = (((time_ms as f32 / 1000.0) * self.sample_rate) as usize)
            .clamp(1, self.buf_l.len() - 1);
        if samples != self.delay_samples {
            self.previous_delay = self.delay_samples;
            self.fade = if self.started { 128 } else { 0 };
            self.delay_samples = samples;
        }
        self.feedback = feedback.clamp(0.0, 0.95);
    }

    pub fn configure(&mut self, settings: &crate::project::DelayReturn, bpm: f64) {
        self.set(settings.effective_time_ms(bpm), settings.feedback);
        self.mode = settings.mode;
        self.width = settings.width.clamp(0., 1.);
        self.modulation_samples = settings.modulation_ms.clamp(0., 20.)*self.sample_rate/1000.;
        self.phase_step = std::f32::consts::TAU*settings.modulation_hz.clamp(0.05, 8.)/self.sample_rate;
        self.tone.set(settings.low_cut_hz, settings.high_cut_hz);
    }

    fn read(&self, delay: f32) -> (f32, f32) {
        let delay=delay.clamp(1., (self.buf_l.len()-1) as f32);
        let whole=delay as usize;
        let fraction=delay-whole as f32;
        let a=(self.pos+self.buf_l.len()-whole)%self.buf_l.len();
        if fraction==0. { return (self.buf_l[a],self.buf_r[a]); }
        let b=(a+self.buf_l.len()-1)%self.buf_l.len();
        (self.buf_l[a]*(1.-fraction)+self.buf_l[b]*fraction,
         self.buf_r[a]*(1.-fraction)+self.buf_r[b]*fraction)
    }

    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let max = self.buf_l.len();
        let modulation = if self.modulation_samples>0. { self.phase.sin()*self.modulation_samples } else { 0. };
        self.phase+=self.phase_step;
        if self.phase>=std::f32::consts::TAU { self.phase-=std::f32::consts::TAU; }
        let (mut dl, mut dr)=self.read(self.delay_samples as f32+modulation);
        if self.fade>0 {
            let (old_l, old_r)=self.read(self.previous_delay.max(1) as f32+modulation);
            let blend=self.fade as f32/128.; dl=dl*(1.-blend)+old_l*blend; dr=dr*(1.-blend)+old_r*blend;
            self.fade-=1;
        }
        let (dl, dr)=self.tone.process(dl, dr);
        if self.mode==crate::project::EchoMode::PingPong {
            let mono = (l + r) * 0.5;
            self.buf_l[self.pos] = mono + dr * self.feedback;
            self.buf_r[self.pos] = dl * self.feedback;
        } else {
            self.buf_l[self.pos]=l+dl*self.feedback;
            self.buf_r[self.pos]=r+dr*self.feedback;
        }
        self.pos = (self.pos + 1) % max;
        self.started=true;
        // Skip the width matrix at unity for legacy sample-for-sample output.
        if self.width==1. { (dl, dr) } else { let mid=(dl+dr)*0.5; let side=(dl-dr)*0.5*self.width; (mid+side, mid-side) }
    }
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Engine events, routed by strip index (= track index in the project).
/// `STRIP_UTILITY` targets the always-present utility synth (metronome,
/// previews). The `LoadClip*`/`PlayClip`/`StopClip`/`SetClipEffects` variants
/// are the legacy TUI preview path, handled by the engine callback directly
/// (not by `Mixer::handle_event`).
/// Convert a microtonal detune in cents to a 14-bit MIDI pitch-bend value
/// (0..=16383, where 8192 is no bend). Assumes the synth's default ±2-semitone
/// (±200-cent) bend range; magnitudes beyond that clamp to the 14-bit endpoints.
pub fn bend14_from_cents(cents: f32) -> i32 {
    let v = (8192.0 + cents / 200.0 * 8192.0).round();
    (v as i32).clamp(0, 16383)
}

#[derive(Debug)]
pub enum AudioEvent {
    Scheduled(Box<super::scheduling::ScheduledEvent>),
    PublishU32 { target: Arc<std::sync::atomic::AtomicU32>, value: u32 },
    PublishU8 { target: Arc<std::sync::atomic::AtomicU8>, value: u8 },
    Applied { revision: u64, ack: Arc<std::sync::atomic::AtomicU64> },
    NoteOn  { strip: u8, pitch: u8, velocity: u8 },
    NoteOff { strip: u8, pitch: u8 },
    SetProgram { strip: u8, bank: u8, preset: u8 },
    /// Strip fader/pan/mute — real mixer math, not GM CCs.
    SetStripGain { strip: u8, gain: f32 },
    SetStripPan  { strip: u8, pan: f32 },
    SetStripMute { strip: u8, muted: bool },
    /// Full per-strip insert chain + the GM CC subset to the strip's synth.
    SetStripEffects { strip: u8, effects: crate::project::EffectChain },
    /// GM CC effects only (reverb/chorus/cutoff/resonance/LFO) to a strip synth.
    SetMidiEffects { strip: u8, reverb: f32, chorus: f32, cutoff: f32, resonance: f32, lfo_depth: f32 },
    RawCC     { strip: u8, cc: u8, value: u8 },
    PitchBend { strip: u8, lsb: u8, msb: u8 },
    /// Trigger a one-shot sampler pad on a strip (gain = velocity 0..1).
    TriggerSample { strip: u8, pitch: u8, gain: f32 },
    // Arranged audio clips — LoadAudioClips is intercepted by the relay thread,
    // which loads the WAVs at engine SR and sends back SetAudioClips.
    LoadAudioClips { specs: Vec<(u32, String)> }, // (clip_id, path)
    SetAudioClips  { clips: Vec<(u32, Arc<Vec<f32>>, Arc<Vec<f32>>)> },
    PreparedAudioClips { clips: std::collections::HashMap<u32, (Arc<Vec<f32>>, Arc<Vec<f32>>)> },
    /// Start clip `clip_id` on `strip` from `offset_sec` for `len_sec` at the
    /// clip's own gain (track volume/pan/mute are applied by the strip).
    TriggerAudioClip { strip: u8, clip_id: u32, offset_sec: f32, len_sec: f32, gain: f32 },
    /// Stop all audio-clip voices on every strip (transport stop / seek).
    StopAudioClips,
    /// Build strips off-thread (relay) → swapped in via SetStrips.
    LoadStrips { specs: Vec<StripSpec> },
    SetStrips  { strips: Vec<Strip> },
    /// Configure both return buses from the project's `Returns` (live-safe:
    /// updates coefficients without clearing DSP buffers/tails).
    SetReturns { returns: crate::project::Returns },
    /// Tempo used by synced return effects, shared by live and offline rendering.
    SetTempo { bpm: f64 },
    // ── CLAP hosting ─────────────────────────────────────────────────────────
    /// Install a freshly built plugin processor (relay/exporter → mixer).
    AddClapInstance { uid: String, processor: super::clap::ClapProcessor },
    /// Engine-intercepted: extract the processor and return it to the relay
    /// over the garbage channel (never dropped on the RT thread).
    RemoveClapInstance { uid: String },
    /// Queue a parameter change on whichever processor has `uid`.
    SetClapParam { uid: String, param_id: u32, value: f64 },
    /// Relay-intercepted: build instances for these specs (diffed).
    LoadClapInstances { specs: Vec<super::clap::ClapSlotSpec> },
    /// Relay-intercepted: collect state blobs from all live instances.
    CollectClapState { reply: crossbeam_channel::Sender<Vec<(String, Vec<u8>)>> },
    /// Immediately silence every synth voice on all strips + utility.
    Panic,
    // ── Legacy TUI preview path (engine-handled, unchanged semantics) ────────
    LoadClipFromFile { path: String, volume: f32 },
    LoadClip  { left: Vec<f32>, right: Vec<f32>, volume: f32 },
    PlayClip,
    StopClip,
    #[allow(clippy::too_many_arguments)]
    SetClipEffects {
        delay_time_ms: u32, delay_feedback: f32, delay_mix: f32, cutoff: f32, drive: f32,
        eq_low_gain: f32, eq_mid_gain: f32, eq_high_gain: f32, eq_mid_freq: f32,
        comp_threshold: f32, comp_ratio: f32, comp_attack_ms: f32, comp_release_ms: f32,
    },
}

// ── Strip construction specs ──────────────────────────────────────────────────

/// What the relay thread needs to build one strip off the RT thread.
#[derive(Debug, Clone, PartialEq)]
pub enum StripKind {
    /// Soundfont instrument. `font: None` = the project's main soundfont.
    Melodic { font: Option<String>, bank: u8, preset: u8 },
    Drum    { font: Option<String>, bank: u8, preset: u8 },
    Sampler { pads: Vec<crate::project::SamplePad> },
    Audio,
    /// CLAP instrument — no synth; the processor is looked up by uid.
    Clap,
}

#[derive(Debug, Clone)]
pub struct StripSpec {
    pub kind:    StripKind,
    pub gain:    f32,
    pub pan:     f32,
    pub muted:   bool,
    pub effects: crate::project::EffectChain,
    /// CLAP instrument slot uid, if the track's instrument is a plugin.
    pub clap_instrument: Option<String>,
    /// CLAP insert-chain slot uids, in order.
    pub clap_fx: Vec<String>,
    /// Constant microtonal detune in cents, applied as a fixed pitch bend to the
    /// strip's synth at build time. 0 = standard 12-TET.
    pub tuning_cents: f32,
}

impl StripSpec {
    /// One spec per project track, in track order (strip index = track index).
    /// Shared by the live sequencer and the offline exporter so both build
    /// identical mixers.
    pub fn from_project(project: &crate::project::Project) -> Vec<StripSpec> {
        use crate::project::{Instrument, TrackType};
        project.tracks.iter().map(|t| StripSpec {
            kind: match (&t.instrument, t.track_type) {
                (Instrument::Sampler { pads }, _) => StripKind::Sampler {
                    pads: pads.clone(),
                },
                (Instrument::Clap(_), _) => StripKind::Clap,
                (_, TrackType::Audio) => StripKind::Audio,
                (Instrument::Soundfont { preset, bank, soundfont }, ty) => {
                    let font = soundfont.clone().filter(|p| !p.is_empty());
                    if ty == TrackType::Drum {
                        StripKind::Drum { font, bank: *bank, preset: *preset }
                    } else {
                        StripKind::Melodic { font, bank: *bank, preset: *preset }
                    }
                }
                _ => StripKind::Audio,
            },
            gain: t.volume,
            pan: t.pan,
            muted: t.muted,
            effects: t.effects.clone(),
            clap_instrument: match &t.instrument {
                Instrument::Clap(s) => Some(s.uid.clone()),
                _ => None,
            },
            clap_fx: t.clap_fx.iter().map(|s| s.uid.clone()).collect(),
            tuning_cents: t.tuning_cents,
        }).collect()
    }
}

// ── Strip + Mixer ─────────────────────────────────────────────────────────────

const MAX_BLOCK: usize = 8192;

const MAX_SAMPLE_VOICES: usize = 128;
pub static SAMPLER_VOICE_STEALS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct SampleTake {
    left: Arc<Vec<f32>>,
    right: Arc<Vec<f32>>,
    gain: f32,
}
struct VelocityLayer {
    min: u8,
    max: u8,
    takes: Vec<SampleTake>,
    next: usize,
}

/// Built off-thread. Matching velocity layers rotate deterministically, while
/// uncovered velocities use the original pad. Existing voices retain their take.
pub struct SamplerPadAudio {
    fallback: SampleTake,
    layers: Vec<VelocityLayer>,
}
impl SamplerPadAudio {
    pub fn new(left: Arc<Vec<f32>>, right: Arc<Vec<f32>>, gain: f32) -> Self {
        Self { fallback: SampleTake { left, right, gain }, layers: Vec::new() }
    }
    fn add_take(&mut self, min: u8, max: u8, left: Arc<Vec<f32>>, right: Arc<Vec<f32>>, gain: f32) {
        let take = SampleTake { left, right, gain };
        if let Some(layer) = self.layers.iter_mut().find(|l| l.min == min && l.max == max) {
            layer.takes.push(take);
        } else {
            self.layers.push(VelocityLayer { min, max, takes: vec![take], next: 0 });
        }
    }
    fn select(&mut self, velocity: u8) -> (Option<usize>, usize) {
        for (index, layer) in self.layers.iter_mut().enumerate() {
            if velocity >= layer.min && velocity <= layer.max {
                let take = layer.next;
                layer.next = (take + 1) % layer.takes.len();
                return (Some(index), take);
            }
        }
        (None, 0)
    }
}
struct SampleVoice {
    pitch: u8,
    layer: Option<usize>,
    take: usize,
    position: usize,
    gain: f32,
}

/// One mixer channel. Built off the RT thread (relay), processed on it.
pub struct Strip {
    /// Per-strip synthesizer; None for sampler/audio strips.
    pub synth: Option<rustysynth::Synthesizer>,
    /// GM channel inside this strip's synth: 9 for drums, else 0.
    pub gm_channel: i32,
    /// Pitch -> preloaded base sample, velocity layers and alternate takes.
    pub pads: std::collections::HashMap<u8, SamplerPadAudio>,
    /// Active one-shots retain indices into their chosen preloaded take.
    sample_voices: Vec<SampleVoice>,
    /// Active audio-clip voices: (left, right, pos, end, clip gain).
    clip_voices: Vec<(Arc<Vec<f32>>, Arc<Vec<f32>>, usize, usize, f32, usize)>,
    pub inserts: InsertChain,
    pub gain:  f32,
    pub pan:   f32,
    pub muted: bool,
    /// Post-fader send levels to the shared return buses (0..1).
    pub send_reverb: f32,
    pub send_delay:  f32,
    /// CLAP instrument slot uid (processor lives in `Mixer.clap`).
    pub clap_instrument: Option<String>,
    /// CLAP insert chain, in order (uids into `Mixer.clap`).
    pub clap_fx: Vec<String>,
    pub meter: Arc<StripMeter>,
}

impl Strip {
    fn trigger_sample(&mut self, pitch: u8, gain: f32) {
        if !gain.is_finite() || gain <= 0. { return; }
        let Some(pad) = self.pads.get_mut(&pitch) else { return };
        let (layer, take) = pad.select((gain * 127.).round().clamp(1., 127.) as u8);
        if self.sample_voices.len() == MAX_SAMPLE_VOICES {
            self.sample_voices.remove(0);
            SAMPLER_VOICE_STEALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.sample_voices.push(SampleVoice { pitch, layer, take, position: 0, gain });
    }

    /// A synthless strip (sampler/audio) with bypass inserts at unity.
    pub fn new(sample_rate: u32) -> Self {
        Strip {
            synth: None,
            gm_channel: 0,
            pads: std::collections::HashMap::new(),
            sample_voices: Vec::with_capacity(MAX_SAMPLE_VOICES),
            clip_voices: Vec::new(),
            inserts: InsertChain::new(sample_rate),
            gain: 1.0,
            pan: 0.0,
            muted: false,
            send_reverb: 0.0,
            send_delay: 0.0,
            clap_instrument: None,
            clap_fx: Vec::new(),
            meter: Arc::new(StripMeter::new()),
        }
    }

    fn synth_msg(&mut self, msg: i32, d1: i32, d2: i32) {
        if let Some(s) = self.synth.as_mut() {
            s.process_midi_message(self.gm_channel, msg, d1, d2);
        }
    }
}

impl std::fmt::Debug for Strip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Strip")
            .field("synth", &self.synth.is_some())
            .field("gm_channel", &self.gm_channel)
            .field("gain", &self.gain)
            .field("pan", &self.pan)
            .field("muted", &self.muted)
            .finish()
    }
}

/// FX unit of one return bus.
pub enum ReturnFx {
    Reverb(Freeverb),
    Delay(PingPongDelay),
}

/// One send-return bus: strips accumulate into `in_l/in_r` (post-fader ×
/// send level); `render_block` runs the FX, meters, and sums into master.
pub struct ReturnBus {
    fx: ReturnFx,
    pub gain: f32,
    pub muted: bool,
    pub meter: Arc<StripMeter>,
    in_l: Vec<f32>,
    in_r: Vec<f32>,
}

impl ReturnBus {
    fn new(fx: ReturnFx, meter: Arc<StripMeter>) -> Self {
        ReturnBus {
            fx,
            gain: 1.0,
            muted: false,
            meter,
            in_l: vec![0.0; MAX_BLOCK],
            in_r: vec![0.0; MAX_BLOCK],
        }
    }
}

pub struct Mixer {
    pub strips: Vec<Strip>,
    /// Always-available synth for metronome/utility events (None if no soundfont).
    pub utility: Option<rustysynth::Synthesizer>,
    /// Shared audio-clip buffers: clip_id -> (L, R).
    pub audio_clips: std::collections::HashMap<u32, (Arc<Vec<f32>>, Arc<Vec<f32>>)>,
    /// Send-return buses: [0] = reverb, [1] = delay.
    pub returns: [ReturnBus; 2],
    /// Live CLAP processors keyed by slot uid — outside the strip rebuild
    /// cycle so project edits never re-instantiate plugins.
    pub clap: std::collections::HashMap<String, super::clap::ClapProcessor>,
    pub master_meter: Arc<StripMeter>,
    pub sample_rate: u32,
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
    bpm: f64,
    returns_settings: crate::project::Returns,
}

/// Linear de-click envelope for a voice of length `len`, position
/// `pos_in_voice`. `fade_in` ramps 0→1 over the first `fade` samples
/// (skipped for percussive one-shots to keep their attack); the tail always
/// ramps 1→0 over the last `fade` samples.
fn declick_env(pos_in_voice: usize, len: usize, fade: usize, fade_in: bool) -> f32 {
    if len == 0 { return 0.0; }
    let fade = fade.min(len / 2).max(1);
    if fade_in && pos_in_voice < fade {
        pos_in_voice as f32 / fade as f32
    } else if pos_in_voice + fade >= len {
        (len - pos_in_voice.min(len)) as f32 / fade as f32
    } else {
        1.0
    }
}

impl Mixer {
    pub fn new(
        sample_rate: u32,
        utility: Option<rustysynth::Synthesizer>,
        master_meter: Arc<StripMeter>,
        return_meters: [Arc<StripMeter>; 2],
    ) -> Self {
        let [rm0, rm1] = return_meters;
        Mixer {
            strips: Vec::new(),
            utility,
            audio_clips: std::collections::HashMap::new(),
            returns: [
                ReturnBus::new(ReturnFx::Reverb(Freeverb::new(sample_rate)), rm0),
                ReturnBus::new(ReturnFx::Delay(PingPongDelay::new(sample_rate)), rm1),
            ],
            clap: std::collections::HashMap::new(),
            master_meter,
            sample_rate,
            scratch_l: vec![0.0; MAX_BLOCK],
            scratch_r: vec![0.0; MAX_BLOCK],
            bpm: 120.,
            returns_settings: crate::project::Returns::default(),
        }
    }

    fn strip_mut(&mut self, idx: u8) -> Option<&mut Strip> {
        self.strips.get_mut(idx as usize)
    }

    /// Extract a processor for teardown elsewhere (never drop on the RT thread).
    pub fn remove_clap(&mut self, uid: &str) -> Option<super::clap::ClapProcessor> {
        self.clap.remove(uid)
    }

    /// Route one event. Unknown strip indices are dropped silently. The legacy
    /// preview-clip variants and `LoadStrips`/`LoadAudioClips` (relay-handled)
    /// are no-ops here.
    pub fn handle_event(&mut self, ev: AudioEvent) {
        match ev {
            AudioEvent::PublishU32 { target, value } => { target.store(value, std::sync::atomic::Ordering::Release); },
            AudioEvent::PublishU8 { target, value } => { target.store(value, std::sync::atomic::Ordering::Release); },
            AudioEvent::Applied { revision, ack } => { ack.store(revision, std::sync::atomic::Ordering::Release); },
            AudioEvent::NoteOn { strip, pitch, velocity } => {
                if strip == STRIP_UTILITY {
                    if let Some(s) = self.utility.as_mut() {
                        s.note_on(9, pitch as i32, velocity as i32);
                    }
                } else {
                    let Mixer { strips, clap, .. } = self;
                    if let Some(st) = strips.get_mut(strip as usize) {
                        // CLAP instrument strips: queue a plugin note event.
                        if let Some(p) = st.clap_instrument.as_ref().and_then(|u| clap.get_mut(u)) {
                            p.queue_note_on(pitch, velocity);
                        } else if st.synth.is_none() && st.pads.contains_key(&pitch) {
                            // Sampler strips: a matching pad plays as a one-shot.
                            st.trigger_sample(pitch, velocity as f32 / 127.0);
                        } else if let Some(s) = st.synth.as_mut() {
                            s.note_on(st.gm_channel, pitch as i32, velocity as i32);
                        }
                    }
                }
            }
            AudioEvent::NoteOff { strip, pitch } => {
                if strip == STRIP_UTILITY {
                    if let Some(s) = self.utility.as_mut() {
                        s.note_off(9, pitch as i32);
                    }
                } else {
                    let Mixer { strips, clap, .. } = self;
                    if let Some(st) = strips.get_mut(strip as usize) {
                        if let Some(p) = st.clap_instrument.as_ref().and_then(|u| clap.get_mut(u)) {
                            p.queue_note_off(pitch);
                        } else if let Some(s) = st.synth.as_mut() {
                            s.note_off(st.gm_channel, pitch as i32);
                        }
                    }
                }
            }
            AudioEvent::SetProgram { strip, bank, preset } => {
                if let Some(st) = self.strip_mut(strip) {
                    st.synth_msg(0xB0, 0, (bank >> 7) as i32);
                    st.synth_msg(0xB0, 32, (bank & 0x7F) as i32);
                    st.synth_msg(0xC0, preset as i32, 0);
                }
            }
            AudioEvent::SetStripGain { strip, gain } => {
                if let Some(st) = self.strip_mut(strip) { st.gain = gain.clamp(0.0, 2.0); }
            }
            AudioEvent::SetStripPan { strip, pan } => {
                if let Some(st) = self.strip_mut(strip) { st.pan = pan.clamp(-1.0, 1.0); }
            }
            AudioEvent::SetStripMute { strip, muted } => {
                let Mixer { strips, clap, .. } = self;
                if let Some(st) = strips.get_mut(strip as usize) {
                    st.muted = muted;
                    if muted {
                        st.synth_msg(0xB0, 123, 0); // All Notes Off, so unmute is clean
                        if let Some(p) = st.clap_instrument.as_ref().and_then(|u| clap.get_mut(u)) {
                            p.queue_all_notes_off();
                        }
                    }
                }
            }
            AudioEvent::SetStripEffects { strip, effects } => {
                if let Some(st) = self.strip_mut(strip) {
                    st.inserts.configure(&effects);
                    // `reverb` is the send to the shared reverb bus (slice B);
                    // CC91 is no longer forwarded. Other GM CCs still apply.
                    st.send_reverb = effects.reverb.clamp(0.0, 1.0);
                    st.send_delay = effects.send_delay.clamp(0.0, 1.0);
                    let cc = |v: f32| (v.clamp(0.0, 1.0) * 127.0) as i32;
                    st.synth_msg(0xB0, 93, cc(effects.chorus));
                    st.synth_msg(0xB0, 74, cc(effects.cutoff));
                    st.synth_msg(0xB0, 71, cc(effects.resonance));
                    st.synth_msg(0xB0, 1,  cc(effects.lfo_depth));
                }
            }
            AudioEvent::SetMidiEffects { strip, reverb, chorus, cutoff, resonance, lfo_depth } => {
                if let Some(st) = self.strip_mut(strip) {
                    st.send_reverb = reverb.clamp(0.0, 1.0);
                    let cc = |v: f32| (v.clamp(0.0, 1.0) * 127.0) as i32;
                    st.synth_msg(0xB0, 93, cc(chorus));
                    st.synth_msg(0xB0, 74, cc(cutoff));
                    st.synth_msg(0xB0, 71, cc(resonance));
                    st.synth_msg(0xB0, 1,  cc(lfo_depth));
                }
            }
            AudioEvent::SetReturns { returns } => {
                let rv = &mut self.returns[0];
                rv.gain = returns.reverb.gain.clamp(0.0, 1.25);
                rv.muted = returns.reverb.muted;
                if let ReturnFx::Reverb(f) = &mut rv.fx {
                    f.set(returns.reverb.room_size, returns.reverb.damping, returns.reverb.width);
                    f.tone.set(returns.reverb.low_cut_hz, returns.reverb.high_cut_hz);
                    f.predelay.set(returns.reverb.predelay_ms);
                }
                let dl = &mut self.returns[1];
                dl.gain = returns.delay.gain.clamp(0.0, 1.25);
                dl.muted = returns.delay.muted;
                if let ReturnFx::Delay(d) = &mut dl.fx {
                    d.configure(&returns.delay, self.bpm);
                }
                self.returns_settings = returns;
            }
            AudioEvent::SetTempo { bpm } => {
                self.bpm = bpm.clamp(20., 300.);
                if let ReturnFx::Delay(d) = &mut self.returns[1].fx {
                    d.configure(&self.returns_settings.delay, self.bpm);
                }
            }
            AudioEvent::RawCC { strip, cc, value } => {
                if strip == STRIP_UTILITY {
                    if let Some(s) = self.utility.as_mut() {
                        s.process_midi_message(9, 0xB0, cc as i32, value as i32);
                    }
                } else if let Some(st) = self.strip_mut(strip) {
                    st.synth_msg(0xB0, cc as i32, value as i32);
                }
            }
            AudioEvent::PitchBend { strip, lsb, msb } => {
                if let Some(st) = self.strip_mut(strip) {
                    st.synth_msg(0xE0, lsb as i32, msb as i32);
                }
            }
            AudioEvent::TriggerSample { strip, pitch, gain } => {
                if let Some(st) = self.strip_mut(strip) {
                    if st.pads.contains_key(&pitch) {
                        st.trigger_sample(pitch, gain);
                    }
                }
            }
            AudioEvent::PreparedAudioClips { clips } => { self.audio_clips = clips; }
            AudioEvent::SetAudioClips { clips } => {
                self.audio_clips.clear();
                for (id, l, r) in clips {
                    self.audio_clips.insert(id, (l, r));
                }
            }
            AudioEvent::TriggerAudioClip { strip, clip_id, offset_sec, len_sec, gain } => {
                let sr = self.sample_rate;
                let Some((l, r)) = self.audio_clips.get(&clip_id) else { return };
                let (l, r) = (Arc::clone(l), Arc::clone(r));
                let start = (offset_sec.max(0.0) * sr as f32) as usize;
                let len   = (len_sec.max(0.0) * sr as f32) as usize;
                let end   = (start + len).min(l.len());
                if start < end {
                    if let Some(st) = self.strip_mut(strip) {
                        st.clip_voices.push((l, r, start, end, gain, start));
                    }
                }
            }
            AudioEvent::StopAudioClips => {
                // Ramp clip voices out (~3 ms) via declick_env rather than a hard cut.
                let fade = ((self.sample_rate as f32 * 0.003) as usize).max(1);
                for st in &mut self.strips {
                    for v in &mut st.clip_voices {
                        v.3 = (v.2 + fade).min(v.3); // end = min(end, pos + fade)
                    }
                }
            }
            AudioEvent::SetStrips { mut strips } => {
                strips.truncate(MAX_STRIPS);
                self.strips = strips;
            }
            AudioEvent::Panic => {
                for st in &mut self.strips {
                    st.synth_msg(0xB0, 120, 0); // All Sound Off
                    st.synth_msg(0xB0, 123, 0); // All Notes Off
                    st.sample_voices.clear();
                    for pad in st.pads.values_mut() { for layer in &mut pad.layers { layer.next = 0; } }
                }
                for p in self.clap.values_mut() {
                    p.queue_all_notes_off();
                }
                if let Some(s) = self.utility.as_mut() {
                    for ch in 0..16 {
                        s.process_midi_message(ch, 0xB0, 120, 0);
                        s.process_midi_message(ch, 0xB0, 123, 0);
                    }
                }
            }
            AudioEvent::AddClapInstance { uid, processor } => {
                self.clap.insert(uid, processor);
            }
            AudioEvent::SetClapParam { uid, param_id, value } => {
                if let Some(p) = self.clap.get_mut(&uid) {
                    p.queue_param(param_id, value);
                }
            }
            // Relay/engine-handled or legacy variants: no-ops here.
            AudioEvent::Scheduled(_)
            | AudioEvent::RemoveClapInstance { .. }
            | AudioEvent::LoadClapInstances { .. }
            | AudioEvent::CollectClapState { .. }
            | AudioEvent::LoadStrips { .. }
            | AudioEvent::LoadAudioClips { .. }
            | AudioEvent::LoadClipFromFile { .. }
            | AudioEvent::LoadClip { .. }
            | AudioEvent::PlayClip
            | AudioEvent::StopClip
            | AudioEvent::SetClipEffects { .. } => {}
        }
    }

    /// Render `out_l.len()` frames (<= 8192). Zeroes the outputs first.
    pub fn render_block(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let frames = out_l.len().min(out_r.len()).min(MAX_BLOCK);
        out_l[..frames].fill(0.0);
        out_r[..frames].fill(0.0);

        let fade = ((self.sample_rate as f32 * 0.003) as usize).max(1);
        let Mixer { strips, utility, returns, clap, master_meter, scratch_l, scratch_r, .. } = self;

        for rb in returns.iter_mut() {
            rb.in_l[..frames].fill(0.0);
            rb.in_r[..frames].fill(0.0);
        }

        for st in strips.iter_mut() {
            let sl = &mut scratch_l[..frames];
            let sr = &mut scratch_r[..frames];
            sl.fill(0.0);
            sr.fill(0.0);

            if let Some(s) = st.synth.as_mut() {
                s.render(sl, sr);
            }

            // CLAP instrument: renders the strip's source into the scratch
            // (these strips have no rustysynth synth).
            if let Some(p) = st.clap_instrument.as_ref().and_then(|u| clap.get_mut(u)) {
                p.process(sl, sr);
            }

            // Sampler one-shots (advance even while muted, so unmute stays in sync).
            st.sample_voices.retain_mut(|voice| {
                let Some(pad) = st.pads.get(&voice.pitch) else { return false };
                let take = match voice.layer {
                    None => &pad.fallback,
                    Some(layer) => &pad.layers[layer].takes[voice.take],
                };
                let (l, r) = (&take.left, &take.right);
                let len = l.len();
                if voice.position >= len { return false; }
                let gain = take.gain * voice.gain;
                for i in 0..frames {
                    let k = voice.position + i;
                    if k >= len { break; }
                    let env = declick_env(k, len, fade, false);
                    sl[i] += l[k] * gain * env;
                    sr[i] += r[k] * gain * env;
                }
                voice.position += frames;
                voice.position < len
            });

            // Audio-clip voices (same advance-while-muted behavior).
            st.clip_voices.retain_mut(|(l, r, pos, end, gain, start)| {
                if *pos >= *end {
                    return false;
                }
                let vlen = end.saturating_sub(*start);
                for i in 0..frames {
                    let k = *pos + i;
                    if k >= *end {
                        break;
                    }
                    let env = declick_env(k - *start, vlen, fade, true);
                    sl[i] += l[k] * *gain * env;
                    sr[i] += r[k] * *gain * env;
                }
                *pos += frames;
                *pos < *end
            });

            if st.muted {
                st.meter.store(0.0);
                continue;
            }

            // Built-in inserts (per frame, pre-fader).
            for i in 0..frames {
                let (l, r) = st.inserts.process(sl[i], sr[i]);
                sl[i] = l;
                sr[i] = r;
            }

            // CLAP insert chain (per block, in slot order, in place).
            // Missing uids are skipped: effects pass through.
            for uid in &st.clap_fx {
                if let Some(p) = clap.get_mut(uid) {
                    p.process(sl, sr);
                }
            }

            // Fader/pan → sends → meter → accumulate into master.
            let gl = st.gain * (1.0 - st.pan.max(0.0));
            let gr = st.gain * (1.0 + st.pan.min(0.0));
            let mut peak = 0f32;
            for i in 0..frames {
                let l = sl[i] * gl;
                let r = sr[i] * gr;
                peak = peak.max(l.abs()).max(r.abs());
                // Post-fader sends into the shared return buses.
                if st.send_reverb > 0.0 {
                    returns[0].in_l[i] += l * st.send_reverb;
                    returns[0].in_r[i] += r * st.send_reverb;
                }
                if st.send_delay > 0.0 {
                    returns[1].in_l[i] += l * st.send_delay;
                    returns[1].in_r[i] += r * st.send_delay;
                }
                out_l[i] += l;
                out_r[i] += r;
            }
            st.meter.store(peak);
        }

        // Return buses: FX → gain → meter → (unless muted) sum into master.
        // Muted buses keep processing so tails stay warm across unmute.
        for rb in returns.iter_mut() {
            let mut peak = 0f32;
            for i in 0..frames {
                let (l, r) = match &mut rb.fx {
                    ReturnFx::Reverb(f) => f.process(rb.in_l[i], rb.in_r[i]),
                    ReturnFx::Delay(d) => d.process(rb.in_l[i], rb.in_r[i]),
                };
                let l = l * rb.gain;
                let r = r * rb.gain;
                peak = peak.max(l.abs()).max(r.abs());
                if !rb.muted {
                    out_l[i] += l;
                    out_r[i] += r;
                }
            }
            rb.meter.store(peak);
        }

        // Utility synth (metronome/previews), dry to master.
        if let Some(s) = utility.as_mut() {
            let sl = &mut scratch_l[..frames];
            let sr = &mut scratch_r[..frames];
            sl.fill(0.0);
            sr.fill(0.0);
            s.render(sl, sr);
            for i in 0..frames {
                out_l[i] += sl[i];
                out_r[i] += sr[i];
            }
        }

        // Master limiter + meter.
        let mut peak = 0f32;
        for i in 0..frames {
            out_l[i] = master_limit(out_l[i]);
            out_r[i] = master_limit(out_r[i]);
            peak = peak.max(out_l[i].abs()).max(out_r[i].abs());
        }
        master_meter.store(peak);
    }
}

/// Build strips from specs (relay thread / exporter — never the RT thread).
/// Soundfonts are cached per path; a failed load falls back to the main font
/// so a bad reference never goes silent. Returns the strips and a parallel
/// vec of their meter handles for the UI.
pub fn build_strips(
    specs: &[StripSpec],
    main_sf2: &std::path::Path,
    sample_rate: u32,
) -> (Vec<Strip>, Vec<Arc<StripMeter>>) {
    let (strips, meters, _) = build_strips_report(specs, main_sf2, sample_rate);
    (strips, meters)
}

/// Build on a worker and report assets that could not be loaded.
pub fn build_strips_report(specs: &[StripSpec], main_sf2: &std::path::Path, sample_rate: u32)
    -> (Vec<Strip>, Vec<Arc<StripMeter>>, Vec<String>) {
    use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};
    let mut errors = Vec::new();
    let settings = SynthesizerSettings::new(sample_rate as i32);
    let mut fonts: std::collections::HashMap<String, Option<Arc<SoundFont>>> =
        std::collections::HashMap::new();
    let main_key = main_sf2.to_string_lossy().to_string();

    let load_font = |path: &str,
                         fonts: &mut std::collections::HashMap<String, Option<Arc<SoundFont>>>|
     -> Option<Arc<SoundFont>> {
        fonts
            .entry(path.to_string())
            .or_insert_with(|| {
                std::fs::File::open(path)
                    .ok()
                    .and_then(|mut f| SoundFont::new(&mut f).ok())
                    .map(Arc::new)
            })
            .clone()
    };
    let main_font = load_font(&main_key, &mut fonts);

    let mut strips = Vec::new();
    let mut meters = Vec::new();
    for spec in specs.iter().take(MAX_STRIPS) {
        let mut strip = Strip::new(sample_rate);
        match &spec.kind {
            StripKind::Melodic { font, bank, preset } | StripKind::Drum { font, bank, preset } => {
                let drum = matches!(spec.kind, StripKind::Drum { .. });
                let sf = font
                    .as_deref()
                    .and_then(|p| load_font(p, &mut fonts))
                    .or_else(|| main_font.clone());
                if let Some(path) = font {
                    if fonts.get(path).is_some_and(|f| f.is_none()) { errors.push(format!("soundfont could not be loaded: {path}")); }
                }
                if sf.is_none() { errors.push(format!("soundfont could not be loaded: {}", main_sf2.display())); }
                if let Some(sf) = sf {
                    if let Ok(mut synth) = Synthesizer::new(&sf, &settings) {
                        let ch = if drum { 9 } else { 0 };
                        synth.process_midi_message(ch, 0xB0, 0, (*bank >> 7) as i32);
                        synth.process_midi_message(ch, 0xB0, 32, (*bank & 0x7F) as i32);
                        synth.process_midi_message(ch, 0xC0, *preset as i32, 0);
                        strip.gm_channel = ch;
                        strip.synth = Some(synth);
                    }
                }
            }
            StripKind::Sampler { pads } => {
                for pad in pads {
                    match super::load_wav_stereo(std::path::Path::new(&pad.path), sample_rate) {
                        Ok((left, right)) => {
                            let mut loaded = SamplerPadAudio::new(Arc::new(left), Arc::new(right), pad.gain);
                            for variant in &pad.variants {
                                match super::load_wav_stereo(std::path::Path::new(&variant.path), sample_rate) {
                                    Ok((left, right)) => loaded.add_take(variant.min_velocity, variant.max_velocity,
                                        Arc::new(left), Arc::new(right), pad.gain * variant.gain),
                                    Err(e) => errors.push(format!("sampler variant {}: {e}", variant.path)),
                                }
                            }
                            strip.pads.insert(pad.pitch, loaded);
                        }
                        Err(e) => errors.push(format!("sampler pad {}: {e}", pad.path)),
                    }
                }
            }
            // CLAP instruments need no build here — the processor is created
            // by the relay's instance diff and looked up by uid at render.
            StripKind::Audio | StripKind::Clap => {}
        }
        strip.clap_instrument = spec.clap_instrument.clone();
        strip.clap_fx = spec.clap_fx.clone();
        strip.gain = spec.gain.clamp(0.0, 2.0);
        strip.pan = spec.pan.clamp(-1.0, 1.0);
        strip.muted = spec.muted;
        // `reverb` = send to the shared reverb bus (CC91 no longer sent).
        strip.send_reverb = spec.effects.reverb.clamp(0.0, 1.0);
        strip.send_delay = spec.effects.send_delay.clamp(0.0, 1.0);
        strip.inserts.configure(&spec.effects);
        if let Some(s) = strip.synth.as_mut() {
            let cc = |v: f32| (v.clamp(0.0, 1.0) * 127.0) as i32;
            let ch = strip.gm_channel;
            s.process_midi_message(ch, 0xB0, 93, cc(spec.effects.chorus));
            s.process_midi_message(ch, 0xB0, 74, cc(spec.effects.cutoff));
            s.process_midi_message(ch, 0xB0, 71, cc(spec.effects.resonance));
            s.process_midi_message(ch, 0xB0, 1,  cc(spec.effects.lfo_depth));
            // Microtonal detune: a constant pitch bend held for the strip's life,
            // so every note on this voice sounds `tuning_cents` off 12-TET.
            if spec.tuning_cents != 0.0 {
                let bend = bend14_from_cents(spec.tuning_cents);
                s.process_midi_message(ch, 0xE0, bend & 0x7F, (bend >> 7) & 0x7F);
            }
        }
        meters.push(Arc::clone(&strip.meter));
        strips.push(strip);
    }
    (strips, meters, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cents_to_bend_maps_center_and_semitones() {
        // 14-bit pitch bend: 8192 is center (no detune). Default synth bend
        // range is ±2 semitones (±200 cents), so ±100 cents == ±half the range.
        assert_eq!(bend14_from_cents(0.0), 8192);
        assert_eq!(bend14_from_cents(100.0), 12288); // +1 semitone
        assert_eq!(bend14_from_cents(-100.0), 4096); // -1 semitone
        assert_eq!(bend14_from_cents(50.0), 10240); // +quarter tone (off 12-TET)
        // Beyond the bend range clamps into the valid 14-bit span.
        assert_eq!(bend14_from_cents(250.0), 16383);
        assert_eq!(bend14_from_cents(-250.0), 0);
    }

    #[test]
    fn insert_chain_default_is_transparent() {
        let mut c = InsertChain::new(48_000);
        for i in 0..256 {
            let x = ((i as f32) * 0.013).sin() * 0.5;
            let (l, r) = c.process(x, -x);
            assert!((l - x).abs() < 1e-6 && (r + x).abs() < 1e-6, "frame {i}");
        }
    }

    #[test]
    fn insert_chain_drive_saturates_and_bounds() {
        let mut c = InsertChain::new(48_000);
        let mut fx = crate::project::EffectChain::default();
        fx.drive = 1.0;
        c.configure(&fx);
        let (l, _) = c.process(0.8, 0.8);
        assert!(l.abs() <= 1.0 && (l - 0.8).abs() > 0.01, "drive must alter and bound, got {l}");
    }

    #[test]
    fn master_limit_transparent_then_bounded() {
        assert_eq!(master_limit(0.25), 0.25);
        assert_eq!(master_limit(-0.5), -0.5);
        assert!(master_limit(10.0) < 1.0 && master_limit(10.0) > 0.9);
        assert!(master_limit(-10.0) > -1.0);
        // continuity at the knee
        assert!((master_limit(0.5001) - 0.5001).abs() < 1e-3);
    }

    fn strip_with_pad(gain: f32, pan: f32) -> Strip {
        // 4-sample constant 0.5 mono pad at pitch 60.
        let buf = Arc::new(vec![0.5f32; 4]);
        let mut s = Strip::new(48_000);
        s.pads.insert(60u8, SamplerPadAudio::new(Arc::clone(&buf), buf, 1.0f32));
        s.gain = gain;
        s.pan = pan;
        s
    }

    fn mixer_with(strips: Vec<Strip>) -> Mixer {
        let mut m = Mixer::new(
            48_000,
            None,
            Arc::new(StripMeter::new()),
            [Arc::new(StripMeter::new()), Arc::new(StripMeter::new())],
        );
        m.handle_event(AudioEvent::SetStrips { strips });
        m
    }

    #[test]
    fn strips_are_isolated_and_metered() {
        let mut m = mixer_with(vec![strip_with_pad(1.0, 0.0), strip_with_pad(1.0, 0.0)]);
        m.handle_event(AudioEvent::TriggerSample { strip: 0, pitch: 60, gain: 1.0 });
        let (mut l, mut r) = (vec![0f32; 8], vec![0f32; 8]);
        m.render_block(&mut l, &mut r);
        assert!((l[0] - 0.5).abs() < 1e-6, "got {}", l[0]);
        assert!((m.strips[0].meter.read().0 - 0.5).abs() < 1e-6);
        assert_eq!(m.strips[1].meter.read().0, 0.0, "untouched strip stays silent");
    }

    #[test]
    fn gain_pan_mute_math() {
        // gain 0.5, pan hard right: L = 0.25*(1-1) = 0, R = 0.25*(1+0) = 0.25.
        let mut m = mixer_with(vec![strip_with_pad(0.5, 1.0)]);
        m.handle_event(AudioEvent::TriggerSample { strip: 0, pitch: 60, gain: 1.0 });
        let (mut l, mut r) = (vec![0f32; 4], vec![0f32; 4]);
        m.render_block(&mut l, &mut r);
        assert!(l[0].abs() < 1e-6 && (r[0] - 0.25).abs() < 1e-6, "L {} R {}", l[0], r[0]);

        // Mute: silent output, but the voice advances (unmute stays in sync).
        let mut m = mixer_with(vec![strip_with_pad(1.0, 0.0)]);
        m.handle_event(AudioEvent::SetStripMute { strip: 0, muted: true });
        m.handle_event(AudioEvent::TriggerSample { strip: 0, pitch: 60, gain: 1.0 });
        let (mut l, mut r) = (vec![0f32; 4], vec![0f32; 4]);
        m.render_block(&mut l, &mut r);
        assert!(l.iter().all(|s| s.abs() < 1e-9));
        m.handle_event(AudioEvent::SetStripMute { strip: 0, muted: false });
        m.render_block(&mut l, &mut r);
        assert!(l.iter().all(|s| s.abs() < 1e-9), "voice advanced while muted; nothing left");
    }

    #[test]
    fn audio_clip_voice_routes_through_strip() {
        let mut m = mixer_with(vec![strip_with_pad(1.0, 0.0)]);
        let buf = Arc::new(vec![0.25f32; 48_000]);
        m.handle_event(AudioEvent::SetAudioClips { clips: vec![(7, Arc::clone(&buf), buf)] });
        m.handle_event(AudioEvent::TriggerAudioClip {
            strip: 0, clip_id: 7, offset_sec: 0.0, len_sec: 0.5, gain: 2.0,
        });
        // Render past the ~3 ms de-click fade-in (48k → ~144 samples) and
        // check the steady-state mid-voice level: 0.25 * gain 2.0 = 0.5.
        let (mut l, mut r) = (vec![0f32; 512], vec![0f32; 512]);
        m.render_block(&mut l, &mut r);
        assert!((l[400] - 0.5).abs() < 1e-6, "0.25 * clip gain 2.0, got {}", l[400]);
        // The first sample is ramped in (de-click), not full scale.
        assert!(l[0].abs() < 0.5, "clip fades in, got {}", l[0]);
    }

    #[test]
    fn declick_fades_clip_edges_but_keeps_sample_attack() {
        // Sample one-shot: attack preserved (no fade-in) — sample 0 is full.
        let mut m = mixer_with(vec![strip_with_pad(1.0, 0.0)]);
        m.handle_event(AudioEvent::TriggerSample { strip: 0, pitch: 60, gain: 1.0 });
        let (mut l, mut r) = (vec![0f32; 8], vec![0f32; 8]);
        m.render_block(&mut l, &mut r);
        assert!((l[0] - 0.5).abs() < 1e-6, "sample attack kept, got {}", l[0]);
        // StopAudioClips ramps a clip out instead of cutting hard.
        let buf = Arc::new(vec![0.5f32; 48_000]);
        m.handle_event(AudioEvent::SetAudioClips { clips: vec![(1, Arc::clone(&buf), buf)] });
        m.handle_event(AudioEvent::TriggerAudioClip { strip: 0, clip_id: 1, offset_sec: 0.0, len_sec: 1.0, gain: 1.0 });
        let (mut l, mut r) = (vec![0f32; 512], vec![0f32; 512]);
        m.render_block(&mut l, &mut r);   // get past fade-in
        m.handle_event(AudioEvent::StopAudioClips);
        let (mut l2, mut r2) = (vec![0f32; 512], vec![0f32; 512]);
        m.render_block(&mut l2, &mut r2);
        assert!(l2[0].abs() > 1e-6, "stop ramps out (not an instant zero), got {}", l2[0]);
        assert!(l2[300].abs() < 1e-6, "ramp completes within the fade window");
    }

    #[test]
    fn master_sums_and_limits() {
        // Two hot strips (pad 0.5 * gain 2.0 each) sum to 2.0 → limited < 1.0.
        let mut m = mixer_with(vec![strip_with_pad(2.0, 0.0), strip_with_pad(2.0, 0.0)]);
        m.handle_event(AudioEvent::TriggerSample { strip: 0, pitch: 60, gain: 1.0 });
        m.handle_event(AudioEvent::TriggerSample { strip: 1, pitch: 60, gain: 1.0 });
        let (mut l, mut r) = (vec![0f32; 4], vec![0f32; 4]);
        m.render_block(&mut l, &mut r);
        assert!(l[0] < 1.0 && l[0] > 0.9, "got {}", l[0]);
        assert!(m.master_meter.read().0 < 1.0);
    }

    #[test]
    fn unknown_strip_event_is_dropped() {
        let mut m = mixer_with(vec![strip_with_pad(1.0, 0.0)]);
        m.handle_event(AudioEvent::TriggerSample { strip: 9, pitch: 60, gain: 1.0 });
        m.handle_event(AudioEvent::SetStripGain { strip: 200, gain: 1.0 });
        let (mut l, mut r) = (vec![0f32; 4], vec![0f32; 4]);
        m.render_block(&mut l, &mut r); // and rendering still works
        assert!(l[0].abs() < 1e-9);
    }

    fn tail_energy(fv: &mut Freeverb, impulse: f32, skip: usize, take: usize) -> f32 {
        let mut e = 0f32;
        for i in 0..(skip + take) {
            let x = if i == 0 { impulse } else { 0.0 };
            let (l, r) = fv.process(x, x);
            if i >= skip {
                e += l.abs() + r.abs();
            }
        }
        e
    }

    #[test]
    fn freeverb_impulse_has_decaying_tail() {
        // Energy exists well after the impulse…
        let early = tail_energy(&mut Freeverb::new_set(48_000, 0.5, 0.5, 1.0), 1.0, 2_000, 24_000);
        assert!(early > 0.01, "no reverb tail, energy {early}");
        // …and a later window has less energy than an earlier one (decay).
        let mut fv2 = Freeverb::new_set(48_000, 0.5, 0.5, 1.0);
        let win1 = tail_energy(&mut fv2, 1.0, 4_800, 9_600);
        let mut fv3 = Freeverb::new_set(48_000, 0.5, 0.5, 1.0);
        let win2 = tail_energy(&mut fv3, 1.0, 48_000, 9_600);
        assert!(win2 < win1, "tail must decay: {win2} !< {win1}");
    }

    #[test]
    fn freeverb_bigger_room_longer_tail() {
        let mut small = Freeverb::new_set(48_000, 0.1, 0.5, 1.0);
        let mut big = Freeverb::new_set(48_000, 0.95, 0.5, 1.0);
        let e_small = tail_energy(&mut small, 1.0, 48_000, 24_000);
        let e_big = tail_energy(&mut big, 1.0, 48_000, 24_000);
        assert!(e_big > e_small * 2.0, "big {e_big} vs small {e_small}");
    }

    #[test]
    fn freeverb_bounded_for_sustained_input() {
        // Worst case (room 1.0 → comb fb 0.98): steady-state gain is
        // 8 combs × 0.015/(1−0.98) = 6.0 per channel. Bounded, never NaN.
        let mut fv = Freeverb::new_set(48_000, 1.0, 0.0, 1.0);
        let mut max = 0f32;
        for _ in 0..96_000 {
            let (l, r) = fv.process(0.5, 0.5);
            max = max.max(l.abs()).max(r.abs());
            assert!(l.is_finite() && r.is_finite());
        }
        assert!(max < 8.0, "runaway reverb, peak {max}");
    }

    fn mixer_with_returns(strips: Vec<Strip>) -> (Mixer, [Arc<StripMeter>; 2]) {
        let rm = [Arc::new(StripMeter::new()), Arc::new(StripMeter::new())];
        let mut m = Mixer::new(
            48_000,
            None,
            Arc::new(StripMeter::new()),
            [Arc::clone(&rm[0]), Arc::clone(&rm[1])],
        );
        m.handle_event(AudioEvent::SetStrips { strips });
        (m, rm)
    }

    #[test]
    fn send_feeds_reverb_return_post_fader() {
        let mut s = strip_with_pad(1.0, 0.0);
        s.send_reverb = 1.0;
        let (mut m, rm) = mixer_with_returns(vec![s]);
        m.handle_event(AudioEvent::TriggerSample { strip: 0, pitch: 60, gain: 1.0 });
        let (mut l, mut r) = (vec![0f32; 4096], vec![0f32; 4096]);
        m.render_block(&mut l, &mut r);
        m.render_block(&mut l, &mut r); // tail block: the 4-sample pad is long gone
        assert!(rm[0].read().0 > 0.0, "reverb return meter silent");
        assert!(l.iter().any(|s| s.abs() > 1e-6), "no wet tail on master");

        // Post-fader: gain 0 → nothing reaches the bus.
        let mut s = strip_with_pad(0.0, 0.0);
        s.send_reverb = 1.0;
        let (mut m, rm) = mixer_with_returns(vec![s]);
        m.handle_event(AudioEvent::TriggerSample { strip: 0, pitch: 60, gain: 1.0 });
        let (mut l, mut r) = (vec![0f32; 4096], vec![0f32; 4096]);
        m.render_block(&mut l, &mut r);
        assert_eq!(rm[0].read().0, 0.0, "post-fader send must be silent at gain 0");
    }

    #[test]
    fn muted_return_meters_but_does_not_sum() {
        let mut s = strip_with_pad(1.0, 0.0);
        s.send_delay = 1.0;
        let (mut m, rm) = mixer_with_returns(vec![s]);
        let mut returns = crate::project::Returns::default();
        returns.delay.time_ms = 1; // echo lands inside the first block
        returns.delay.muted = true;
        m.handle_event(AudioEvent::SetReturns { returns: returns.clone() });
        m.handle_event(AudioEvent::TriggerSample { strip: 0, pitch: 60, gain: 1.0 });
        let (mut l, mut r) = (vec![0f32; 4096], vec![0f32; 4096]);
        m.render_block(&mut l, &mut r);
        assert!(rm[1].read().0 > 0.0, "muted return still meters");
        // Master only contains the 4-sample dry pad; everything after is silent.
        assert!(l[64..].iter().all(|s| s.abs() < 1e-9), "muted return leaked to master");

        // Unmute via SetReturns → wet appears on master.
        returns.delay.muted = false;
        m.handle_event(AudioEvent::SetReturns { returns });
        m.handle_event(AudioEvent::TriggerSample { strip: 0, pitch: 60, gain: 1.0 });
        m.render_block(&mut l, &mut r);
        assert!(l[64..].iter().any(|s| s.abs() > 1e-9), "unmuted return now sums");
    }

    #[test]
    fn set_strip_effects_updates_sends() {
        let (mut m, _rm) = mixer_with_returns(vec![strip_with_pad(1.0, 0.0)]);
        let mut fx = crate::project::EffectChain::default();
        fx.reverb = 0.8;
        fx.send_delay = 0.3;
        m.handle_event(AudioEvent::SetStripEffects { strip: 0, effects: fx });
        assert_eq!(m.strips[0].send_reverb, 0.8);
        assert_eq!(m.strips[0].send_delay, 0.3);
    }

    #[test]
    fn ping_pong_echoes_alternate_and_decay() {
        let sr = 48_000u32;
        let mut d = PingPongDelay::new(sr);
        d.set(100, 0.5); // 100 ms = 4800 samples
        let n = 4_800usize;
        let mut out_l = Vec::new();
        let mut out_r = Vec::new();
        for i in 0..(n * 3 + 8) {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = d.process(x, x);
            out_l.push(l);
            out_r.push(r);
        }
        // First echo on L at n (R silent); second on R at 2n, quieter via fb.
        assert!(out_l[n].abs() > 0.4, "L echo at n, got {}", out_l[n]);
        assert!(out_r[n].abs() < 1e-6, "R silent at n, got {}", out_r[n]);
        assert!(out_r[2 * n].abs() > 0.1, "R echo at 2n, got {}", out_r[2 * n]);
        assert!(out_r[2 * n].abs() < out_l[n].abs(), "echoes decay");
    }

    #[test]
    fn spatial_echo_defaults_keep_legacy_samples() {
        let mut legacy=PingPongDelay::new(8000);
        let mut modern=PingPongDelay::new(8000);
        legacy.set(123,0.47);
        let config=crate::project::DelayReturn { time_ms:123, feedback:0.47, ..Default::default() };
        modern.configure(&config,91.);
        for i in 0..10000 {
            let x=if i%911==0 {0.7} else {0.};
            assert_eq!(legacy.process(x,x*0.3),modern.process(x,x*0.3));
        }
    }

    #[test]
    fn stereo_echo_width_tone_and_tempo_are_audible() {
        use crate::project::{DelayReturn,EchoMode,EchoDivision};
        let mut d=PingPongDelay::new(8000);
        let mut config=DelayReturn { time_ms:10,mode:EchoMode::Stereo,feedback:0.,..Default::default() };
        d.configure(&config,120.);
        let mut out=(0.,0.);
        for i in 0..=80 {out=d.process(if i==0 {1.} else {0.},if i==0 {0.25} else {0.});}
        assert_eq!(out,(1.,0.25),"stereo mode preserves independent channels");
        let mut mono=PingPongDelay::new(8000); config.width=0.; mono.configure(&config,120.);
        for i in 0..=80 {out=mono.process(if i==0 {1.} else {0.},0.);assert_eq!(out.0,out.1);}
        let mut plain=PingPongDelay::new(8000); config.width=1.; plain.configure(&config,120.);
        let mut dark=PingPongDelay::new(8000); config.high_cut_hz=500.; dark.configure(&config,120.);
        let (mut bright_energy,mut dark_energy)=(0.,0.);
        for i in 0..2000 {let x=if i%2==0 {1.} else {-1.};let a=plain.process(x,0.);let b=dark.process(x,0.);if i>200 {bright_energy+=a.0*a.0;dark_energy+=b.0*b.0;}}
        assert!(dark_energy<bright_energy*0.1,"tone filtering must change audio");
        let (mut m,_)=mixer_with_returns(Vec::new());
        let mut returns=crate::project::Returns::default(); returns.delay.sync=Some(EchoDivision::Quarter);
        m.handle_event(AudioEvent::SetReturns {returns});
        m.handle_event(AudioEvent::SetTempo {bpm:60.});
        let ReturnFx::Delay(d)=&m.returns[1].fx else {panic!("echo")};
        assert_eq!(d.delay_samples,m.sample_rate as usize);
    }

    #[test]
    fn reverb_predelay_shifts_the_impulse_and_keeps_storage() {
        let mut dry=Freeverb::new(8000); let mut spaced=Freeverb::new(8000);
        spaced.predelay.set(25);
        let mut expected=Vec::new(); let mut actual=Vec::new();
        for i in 0..3000 {let x=if i==0 {1.} else {0.};expected.push(dry.process(x,0.));actual.push(spaced.process(x,0.));}
        assert!(actual[..200].iter().all(|&(l,r)|l==0.&&r==0.));
        assert_eq!(&actual[200..],&expected[..2800]);
        let mut echo=PingPongDelay::new(8000);
        let address=echo.buf_l.as_ptr();let size=echo.buf_l.len();
        let config=crate::project::DelayReturn { time_ms:7,modulation_ms:20.,modulation_hz:8.,feedback:0.95,low_cut_hz:200.,high_cut_hz:3000.,..Default::default() };
        echo.configure(&config,20.);
        for i in 0..20000 {
            if i%500==0 {echo.set(1+(i%12000) as u32,0.95);}
            let (l,r)=echo.process(if i%97==0 {0.25} else {0.},0.);
            assert!(l.is_finite()&&r.is_finite());
        }
        assert_eq!(echo.buf_l.as_ptr(),address);assert_eq!(echo.buf_l.len(),size);
    }

    fn testplug(name: &str) -> Option<String> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug")
            .join(name);
        if p.exists() {
            Some(p.to_string_lossy().to_string())
        } else {
            assert!(std::env::var_os("DAW_REQUIRE_TEST_ASSETS").is_none(), "required audio test assets missing"); eprintln!("skip: {name} not built (cargo build --workspace)");
            None
        }
    }

    #[test]
    fn clap_fx_processes_strip_audio() {
        let Some(path) = testplug("libdaw_clap_testplug_gain.so") else { return };
        let mut inst =
            crate::audio::clap::ClapInstance::load(&path, "test.gain", 48_000, 8192).unwrap();
        let gid = inst.handle.param_infos[0].id;
        let mut s = strip_with_pad(1.0, 0.0);
        s.clap_fx = vec!["c1".into()];
        let mut m = mixer_with(vec![s]);
        m.handle_event(AudioEvent::AddClapInstance {
            uid: "c1".into(),
            processor: inst.processor.take().unwrap(),
        });
        m.handle_event(AudioEvent::SetClapParam { uid: "c1".into(), param_id: gid, value: 2.0 });
        m.handle_event(AudioEvent::TriggerSample { strip: 0, pitch: 60, gain: 1.0 });
        let (mut l, mut r) = (vec![0f32; 8], vec![0f32; 8]);
        m.render_block(&mut l, &mut r);
        // 0.5 pad × clap gain 2.0 = 1.0 → shaped by the master limiter.
        let expect = master_limit(1.0);
        assert!((l[0] - expect).abs() < 1e-6, "pad 0.5 doubled by clap gain → limited {expect}, got {}", l[0]);
        if let Some(p) = m.remove_clap("c1") {
            inst.handle.deactivate(p);
        }
    }

    #[test]
    fn clap_instrument_strip_follows_notes() {
        let Some(path) = testplug("libdaw_clap_testplug_tone.so") else { return };
        let mut inst =
            crate::audio::clap::ClapInstance::load(&path, "test.tone", 48_000, 8192).unwrap();
        let mut s = Strip::new(48_000);
        s.clap_instrument = Some("c9".into());
        let mut m = mixer_with(vec![s]);
        m.handle_event(AudioEvent::AddClapInstance {
            uid: "c9".into(),
            processor: inst.processor.take().unwrap(),
        });
        m.handle_event(AudioEvent::NoteOn { strip: 0, pitch: 60, velocity: 100 });
        let (mut l, mut r) = (vec![0f32; 8], vec![0f32; 8]);
        m.render_block(&mut l, &mut r);
        assert!((l[0] - 0.25).abs() < 1e-6, "tone instrument sounds, got {}", l[0]);
        m.handle_event(AudioEvent::NoteOff { strip: 0, pitch: 60 });
        m.render_block(&mut l, &mut r);
        assert!(l[0].abs() < 1e-9, "silent after note off, got {}", l[0]);
        if let Some(p) = m.remove_clap("c9") {
            inst.handle.deactivate(p);
        }
    }

    #[test]
    fn missing_clap_uid_passes_audio_through() {
        let mut s = strip_with_pad(1.0, 0.0);
        s.clap_fx = vec!["ghost".into()];
        let mut m = mixer_with(vec![s]);
        m.handle_event(AudioEvent::TriggerSample { strip: 0, pitch: 60, gain: 1.0 });
        let (mut l, mut r) = (vec![0f32; 8], vec![0f32; 8]);
        m.render_block(&mut l, &mut r);
        assert!((l[0] - 0.5).abs() < 1e-6, "missing plugin passes through, got {}", l[0]);
    }

    #[test]
    fn strip_meter_round_trip_and_clip_latch() {
        let m = StripMeter::new();
        m.store(0.42);
        let (p, c) = m.read();
        assert!((p - 0.42).abs() < 1e-6 && !c);
        m.store(1.0);
        assert!(m.read().1);
        m.reset_clip();
        assert!(!m.read().1);
    }
}

#[cfg(test)]
mod sampler_expression_tests {
    use super::*;
    fn buffer(v: f32) -> Arc<Vec<f32>> { Arc::new(vec![v; 512]) }
    fn strip() -> Strip {
        let mut s = Strip::new(44100);
        let mut pad = SamplerPadAudio::new(buffer(0.05), buffer(0.05), 1.);
        pad.add_take(1, 63, buffer(0.1), buffer(0.1), 1.);
        pad.add_take(1, 63, buffer(0.2), buffer(0.2), 1.);
        pad.add_take(80, 127, buffer(0.4), buffer(0.4), 1.);
        s.pads.insert(36, pad);
        s
    }
    fn mixer(s: Strip) -> Mixer {
        let mut m = Mixer::new(44100, None, Default::default(), Default::default());
        m.strips.push(s); m
    }
    #[test]
    fn velocity_layers_rotate_across_different_velocities_and_use_fallback() {
        let mut m = mixer(strip());
        for (velocity, expected) in [(40, 0.1), (48, 0.2), (100, 0.4), (40, 0.1), (70, 0.05)] {
            m.strips[0].sample_voices.clear();
            m.handle_event(AudioEvent::NoteOn { strip: 0, pitch: 36, velocity });
            let mut l = [0.;1]; let mut r = [0.;1]; m.render_block(&mut l, &mut r);
            assert!((l[0] - expected * velocity as f32 / 127.).abs() < 1e-6, "velocity {velocity}: {}", l[0]);
        }
    }
    #[test]
    fn overlapping_takes_keep_their_sound_and_panic_resets_rotation() {
        let mut m = mixer(strip());
        m.handle_event(AudioEvent::TriggerSample { strip: 0, pitch: 36, gain: 40./127. });
        m.handle_event(AudioEvent::TriggerSample { strip: 0, pitch: 36, gain: 40./127. });
        let mut l=[0.;1];let mut r=[0.;1];m.render_block(&mut l,&mut r);
        assert!((l[0] - 0.3*40./127.).abs()<1e-6);
        m.handle_event(AudioEvent::Panic);
        m.handle_event(AudioEvent::TriggerSample { strip: 0, pitch: 36, gain: 0. });
        m.handle_event(AudioEvent::TriggerSample { strip: 0, pitch: 36, gain: 40./127. });
        m.render_block(&mut l,&mut r);
        assert!((l[0] - 0.1*40./127.).abs()<1e-6);
    }
    #[test]
    fn dense_sampler_input_keeps_voice_storage_bounded() {
        let mut s=strip();let capacity=s.sample_voices.capacity();
        let before=SAMPLER_VOICE_STEALS.load(std::sync::atomic::Ordering::Relaxed);
        for _ in 0..MAX_SAMPLE_VOICES+10 { s.trigger_sample(36, 0.5); }
        assert_eq!(s.sample_voices.len(),MAX_SAMPLE_VOICES);
        assert_eq!(s.sample_voices.capacity(),capacity);
        assert!(SAMPLER_VOICE_STEALS.load(std::sync::atomic::Ordering::Relaxed)>=before+10);
    }
}

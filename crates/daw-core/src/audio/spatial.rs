//! Fixed-storage helpers for spatial effects. Configuration never resizes.

pub(crate) struct ToneFilter {
    sr: f32,
    highpass: bool,
    lowpass: bool,
    hp_alpha: f32,
    lp_alpha: f32,
    prev_x: [f32; 2],
    hp: [f32; 2],
    lp: [f32; 2],
}
impl ToneFilter {
    pub fn new(sr: u32) -> Self {
        Self {
            sr: sr as f32,
            highpass: false,
            lowpass: false,
            hp_alpha: 1.,
            lp_alpha: 1.,
            prev_x: [0.; 2],
            hp: [0.; 2],
            lp: [0.; 2],
        }
    }
    pub fn set(&mut self, low: f32, high: f32) {
        self.highpass = low > 0.;
        self.lowpass = high < 20_000.;
        self.hp_alpha = (-std::f32::consts::TAU * low.clamp(0., self.sr * 0.45) / self.sr).exp();
        self.lp_alpha =
            1. - (-std::f32::consts::TAU * high.clamp(1., self.sr * 0.45) / self.sr).exp();
    }
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let mut out = [l, r];
        for (i, x) in out.iter_mut().enumerate() {
            let input = *x;
            self.hp[i] = if self.highpass {
                self.hp_alpha * (self.hp[i] + input - self.prev_x[i])
            } else {
                input
            };
            self.prev_x[i] = input;
            self.lp[i] = if self.lowpass {
                self.lp[i] + self.lp_alpha * (self.hp[i] - self.lp[i])
            } else {
                self.hp[i]
            };
            *x = self.lp[i];
        }
        (out[0], out[1])
    }
}

pub(crate) struct Predelay {
    buf: Vec<f32>,
    pos: usize,
    samples: usize,
    previous: usize,
    fade: usize,
    sr: u32,
    started: bool,
}
impl Predelay {
    pub fn new(sr: u32) -> Self {
        Self {
            buf: vec![0.; sr as usize / 4 + 1],
            pos: 0,
            samples: 0,
            previous: 0,
            fade: 0,
            sr,
            started: false,
        }
    }
    pub fn set(&mut self, ms: u32) {
        let samples = ((ms.min(250) as u64 * self.sr as u64) / 1000) as usize;
        if samples != self.samples {
            self.previous = self.samples;
            self.samples = samples.min(self.buf.len() - 1);
            self.fade = if self.started { 128 } else { 0 };
        }
    }
    pub fn process(&mut self, input: f32) -> f32 {
        let read = |samples: usize| {
            if samples == 0 {
                input
            } else {
                self.buf[(self.pos + self.buf.len() - samples) % self.buf.len()]
            }
        };
        let mut out = read(self.samples);
        if self.fade > 0 {
            let blend = self.fade as f32 / 128.;
            out = out * (1. - blend) + read(self.previous) * blend;
            self.fade -= 1;
        }
        self.buf[self.pos] = input;
        self.pos = (self.pos + 1) % self.buf.len();
        self.started = true;
        out
    }
}

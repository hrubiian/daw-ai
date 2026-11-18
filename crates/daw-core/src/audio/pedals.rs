//! Fixed-order guitar pedals. All delay storage is allocated at construction;
//! parameter updates and sample processing do not allocate or touch devices.
use super::mixer::Freeverb;
use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveMode {
    #[default]
    Off,
    Boost,
    Overdrive,
    Distortion,
    Fuzz,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModulationMode {
    #[default]
    Off,
    Flanger,
    Chorus,
    Tremolo,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuitarEchoMode {
    #[default]
    Off,
    Digital,
    Tape,
    Slapback,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReverbMode {
    #[default]
    Off,
    Room,
    Hall,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriveSettings {
    pub mode: DriveMode,
    pub gain: f32,
    pub tone: f32,
    pub level: f32,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModulationSettings {
    pub mode: ModulationMode,
    pub rate_hz: f32,
    pub depth: f32,
    pub feedback: f32,
    pub mix: f32,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuitarEchoSettings {
    pub mode: GuitarEchoMode,
    pub time_ms: f32,
    pub feedback: f32,
    pub mix: f32,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReverbSettings {
    pub mode: ReverbMode,
    pub size: f32,
    pub damping: f32,
    pub mix: f32,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PedalSettings {
    pub drive: DriveSettings,
    pub modulation: ModulationSettings,
    pub echo: GuitarEchoSettings,
    pub reverb: ReverbSettings,
}

impl PedalSettings {
    /// Older rigs/presets remain dry when these fields are missing.
    pub const BYPASSED: Self = Self {
        drive: DriveSettings {
            mode: DriveMode::Off,
            gain: 0.4,
            tone: 0.6,
            level: 0.5,
        },
        modulation: ModulationSettings {
            mode: ModulationMode::Off,
            rate_hz: 0.3,
            depth: 0.6,
            feedback: 0.25,
            mix: 0.35,
        },
        echo: GuitarEchoSettings {
            mode: GuitarEchoMode::Off,
            time_ms: 330.0,
            feedback: 0.3,
            mix: 0.2,
        },
        reverb: ReverbSettings {
            mode: ReverbMode::Off,
            size: 0.45,
            damping: 0.5,
            mix: 0.15,
        },
    };

    pub fn sanitized(mut self) -> Self {
        let c = |v: f32, min: f32, max: f32, fallback: f32| {
            if v.is_finite() {
                v.clamp(min, max)
            } else {
                fallback
            }
        };
        self.drive.gain = c(self.drive.gain, 0., 1., 0.4);
        self.drive.tone = c(self.drive.tone, 0., 1., 0.6);
        self.drive.level = c(self.drive.level, 0., 1., 0.5);
        self.modulation.rate_hz = c(self.modulation.rate_hz, 0.05, 8., 0.3);
        self.modulation.depth = c(self.modulation.depth, 0., 1., 0.6);
        self.modulation.feedback = c(self.modulation.feedback, -0.85, 0.85, 0.25);
        self.modulation.mix = c(self.modulation.mix, 0., 1., 0.35);
        let max_time = if self.echo.mode == GuitarEchoMode::Slapback {
            250.
        } else {
            1500.
        };
        self.echo.time_ms = c(self.echo.time_ms, 20., max_time, 110.);
        self.echo.feedback = if self.echo.mode == GuitarEchoMode::Slapback {
            0.
        } else {
            c(self.echo.feedback, 0., 0.85, 0.3)
        };
        self.echo.mix = c(self.echo.mix, 0., 0.7, 0.2);
        self.reverb.size = c(self.reverb.size, 0., 1., 0.45);
        self.reverb.damping = c(self.reverb.damping, 0., 1., 0.5);
        self.reverb.mix = c(self.reverb.mix, 0., 0.7, 0.15);
        self
    }

    /// Pedal-only starting points; these never change the amp/cabinet.
    pub fn preset(preset: PedalPreset) -> Self {
        let mut s = Self::BYPASSED;
        match preset {
            PedalPreset::Dry => {}
            PedalPreset::TightRhythm => {
                s.drive = DriveSettings {
                    mode: DriveMode::Boost,
                    gain: 0.25,
                    tone: 0.6,
                    level: 0.6,
                };
            }
            PedalPreset::WideClean => {
                s.modulation = ModulationSettings {
                    mode: ModulationMode::Chorus,
                    rate_hz: 0.8,
                    depth: 0.5,
                    feedback: 0.,
                    mix: 0.3,
                };
                s.reverb = ReverbSettings {
                    mode: ReverbMode::Room,
                    size: 0.4,
                    damping: 0.55,
                    mix: 0.15,
                };
            }
            PedalPreset::LeadEcho => {
                s.echo = GuitarEchoSettings {
                    mode: GuitarEchoMode::Tape,
                    time_ms: 360.,
                    feedback: 0.32,
                    mix: 0.22,
                };
                s.reverb = ReverbSettings {
                    mode: ReverbMode::Hall,
                    size: 0.6,
                    damping: 0.55,
                    mix: 0.12,
                };
            }
            PedalPreset::JetFlange => {
                s.modulation = ModulationSettings {
                    mode: ModulationMode::Flanger,
                    rate_hz: 0.22,
                    depth: 0.8,
                    feedback: 0.6,
                    mix: 0.5,
                };
            }
        }
        s
    }
}

impl Default for PedalSettings {
    fn default() -> Self {
        Self::BYPASSED
    }
}
impl Default for DriveSettings {
    fn default() -> Self {
        PedalSettings::BYPASSED.drive
    }
}
impl Default for ModulationSettings {
    fn default() -> Self {
        PedalSettings::BYPASSED.modulation
    }
}
impl Default for GuitarEchoSettings {
    fn default() -> Self {
        PedalSettings::BYPASSED.echo
    }
}
impl Default for ReverbSettings {
    fn default() -> Self {
        PedalSettings::BYPASSED.reverb
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PedalPreset {
    Dry,
    TightRhythm,
    WideClean,
    LeadEcho,
    JetFlange,
}

struct DelayLine {
    samples: Vec<f32>,
    pos: usize,
}
impl DelayLine {
    fn new(len: usize) -> Self {
        Self {
            samples: vec![0.; len.max(4)],
            pos: 0,
        }
    }
    fn read(&self, delay: f32) -> f32 {
        let delay = delay.clamp(1., (self.samples.len() - 2) as f32);
        let whole = delay.floor() as usize;
        let fraction = delay - whole as f32;
        let newer = (self.pos + self.samples.len() - whole) % self.samples.len();
        let older = (newer + self.samples.len() - 1) % self.samples.len();
        self.samples[newer] + fraction * (self.samples[older] - self.samples[newer])
    }
    fn write(&mut self, sample: f32) {
        self.samples[self.pos] = sample;
        self.pos = (self.pos + 1) % self.samples.len();
    }
    fn clear(&mut self) {
        self.samples.fill(0.);
        self.pos = 0;
    }
}

pub struct GuitarPedals {
    sr: f32,
    settings: PedalSettings,
    drive_hp: f32,
    drive_previous: f32,
    drive_lp: f32,
    drive_dc: f32,
    drive_dc_previous: f32,
    drive_tone_k: f32,
    drive_hp_k: f32,
    echo_tone_k: f32,
    modulation: DelayLine,
    modulation_phase: f32,
    echo: DelayLine,
    echo_tone: f32,
    echo_phase: f32,
    echo_delay: f32,
    echo_previous_delay: f32,
    echo_fade: usize,
    reverb: Freeverb,
}

impl GuitarPedals {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(8000);
        let mut pedals = Self {
            sr: rate as f32,
            settings: PedalSettings::BYPASSED,
            drive_hp: 0.,
            drive_previous: 0.,
            drive_lp: 0.,
            drive_dc: 0.,
            drive_dc_previous: 0.,
            drive_tone_k: 1.,
            drive_hp_k: (-TAU * 150. / rate as f32).exp(),
            echo_tone_k: 1. - (-TAU * 3000. / rate as f32).exp(),
            modulation: DelayLine::new(rate as usize / 20 + 4),
            modulation_phase: 0.,
            echo: DelayLine::new(rate as usize * 3 / 2 + rate as usize / 100 + 4),
            echo_tone: 0.,
            echo_phase: 0.,
            echo_delay: rate as f32 * 0.33,
            echo_previous_delay: rate as f32 * 0.33,
            echo_fade: 0,
            reverb: Freeverb::new(rate),
        };
        pedals.set(PedalSettings::BYPASSED);
        pedals
    }

    pub fn set(&mut self, settings: PedalSettings) {
        let s = settings.sanitized();
        if s.drive.mode != self.settings.drive.mode {
            self.drive_hp = 0.;
            self.drive_previous = 0.;
            self.drive_lp = 0.;
            self.drive_dc = 0.;
            self.drive_dc_previous = 0.;
        }
        if s.modulation.mode != self.settings.modulation.mode {
            self.modulation.clear();
            self.modulation_phase = 0.;
        }
        if s.echo.mode != self.settings.echo.mode {
            self.echo.clear();
            self.echo_tone = 0.;
            self.echo_phase = 0.;
        }
        let delay = s.echo.time_ms * self.sr / 1000.;
        if delay != self.echo_delay {
            self.echo_previous_delay = self.echo_delay;
            self.echo_delay = delay;
            self.echo_fade = if s.echo.mode == self.settings.echo.mode {
                (self.sr * 0.02) as usize
            } else {
                0
            };
        }
        if s.reverb.mode != self.settings.reverb.mode {
            self.reverb.clear();
        }
        let room = match s.reverb.mode {
            ReverbMode::Hall => 0.55 + s.reverb.size * 0.4,
            _ => s.reverb.size * 0.55,
        };
        self.reverb.set(room, s.reverb.damping, 0.);
        self.drive_tone_k = 1. - (-TAU * (700. + s.drive.tone * 9300.) / self.sr).exp();
        self.settings = s;
    }

    /// Drive sits before the amp. Bypass is exactly transparent.
    #[inline]
    pub fn pre_amp(&mut self, input: f32) -> f32 {
        let s = self.settings.drive;
        if s.mode == DriveMode::Off {
            return input;
        }
        let input = if input.is_finite() {
            input.clamp(-8., 8.)
        } else {
            0.
        };
        self.drive_hp = self.drive_hp_k * (self.drive_hp + input - self.drive_previous);
        self.drive_previous = input;
        let x = match s.mode {
            DriveMode::Boost => self.drive_hp * (1. + 3. * s.gain),
            DriveMode::Overdrive => (self.drive_hp * (1. + 18. * s.gain)).tanh() * 0.65,
            DriveMode::Distortion => (self.drive_hp * (2. + 50. * s.gain)).clamp(-0.8, 0.8),
            DriveMode::Fuzz => {
                ((self.drive_hp * (5. + 85. * s.gain) + 0.15).tanh() - 0.15f32.tanh()) * 0.65
            }
            DriveMode::Off => input,
        };
        self.drive_lp += self.drive_tone_k * (x - self.drive_lp);
        let output = self.drive_lp - self.drive_dc_previous + 0.995 * self.drive_dc;
        self.drive_dc_previous = self.drive_lp;
        self.drive_dc = output;
        output * s.level * 2.
    }

    /// Modulation → echo → reverb, following the cabinet; dry paths are immediate.
    #[inline]
    pub fn post_cabinet(&mut self, input: f32) -> f32 {
        let mut x = input;
        let m = self.settings.modulation;
        if m.mode != ModulationMode::Off {
            let lfo = (TAU * self.modulation_phase).sin();
            self.modulation_phase = (self.modulation_phase + m.rate_hz / self.sr).fract();
            if m.mode == ModulationMode::Tremolo {
                let wet = x * (1. - m.depth * (0.5 + 0.5 * lfo));
                x += m.mix * (wet - x);
            } else {
                let delay_ms = if m.mode == ModulationMode::Flanger {
                    0.8 + (0.5 + 0.5 * lfo) * m.depth * 7.
                } else {
                    18. + lfo * m.depth * 8.
                };
                let wet = self.modulation.read(delay_ms * self.sr / 1000.);
                self.modulation.write((x + wet * m.feedback).clamp(-8., 8.));
                x += m.mix * (wet - x);
            }
        }
        let echo = self.settings.echo;
        if echo.mode != GuitarEchoMode::Off {
            let wow = if echo.mode == GuitarEchoMode::Tape {
                (TAU * self.echo_phase).sin() * self.sr * 0.0006
            } else {
                0.
            };
            self.echo_phase = (self.echo_phase + 0.4 / self.sr).fract();
            let mut wet = self.echo.read(self.echo_delay + wow);
            if self.echo_fade > 0 {
                let fraction = self.echo_fade as f32 / (self.sr * 0.02).max(1.);
                wet += fraction * (self.echo.read(self.echo_previous_delay + wow) - wet);
                self.echo_fade -= 1;
            }
            if echo.mode == GuitarEchoMode::Tape {
                self.echo_tone += self.echo_tone_k * (wet - self.echo_tone);
                wet = self.echo_tone;
            }
            self.echo.write((x + wet * echo.feedback).clamp(-8., 8.));
            x += echo.mix * (wet - x);
        }
        let r = self.settings.reverb;
        if r.mode != ReverbMode::Off {
            let (left, right) = self.reverb.process(x.clamp(-8., 8.), x.clamp(-8., 8.));
            x += r.mix * ((left + right) * 0.5 - x);
        }
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::amp::{AmpSettings, Cabinet, GuitarRig};

    fn magnitude(samples: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0., 0.);
        for (i, x) in samples.iter().enumerate() {
            let w = TAU * hz * i as f32 / 48000.;
            re += x * w.cos();
            im += x * w.sin();
        }
        2. * re.hypot(im) / samples.len() as f32
    }

    #[test]
    fn bypass_is_exact_and_old_settings_stay_dry() {
        let mut pedals = GuitarPedals::new(48000);
        for i in 0..10000 {
            let x = (i as f32 * 0.07).sin() * 0.5;
            let driven = pedals.pre_amp(x);
            assert_eq!(driven, x);
            assert_eq!(pedals.post_cabinet(driven), x);
        }
        let old: AmpSettings = serde_json::from_str(r#"{"gain":0.9,"cabinet":"ir"}"#).unwrap();
        assert_eq!(old.pedals, PedalSettings::BYPASSED);
        let partial: PedalSettings =
            serde_json::from_str(r#"{"echo":{"mode":"digital","time_ms":250.0}}"#).unwrap();
        assert_eq!(partial.drive.mode, DriveMode::Off);
        assert_eq!(partial.echo.mix, 0.2);
    }

    #[test]
    fn digital_echo_has_the_requested_time_feedback_and_immediate_dry_signal() {
        let mut pedals = GuitarPedals::new(48000);
        let mut settings = PedalSettings::BYPASSED;
        settings.echo = GuitarEchoSettings {
            mode: GuitarEchoMode::Digital,
            time_ms: 100.,
            feedback: 0.5,
            mix: 0.4,
        };
        pedals.set(settings);
        let out: Vec<f32> = (0..15000)
            .map(|i| pedals.post_cabinet(if i == 0 { 0.5 } else { 0. }))
            .collect();
        assert!((out[0] - 0.3).abs() < 1e-6);
        assert!(out[1..4800].iter().all(|v| *v == 0.));
        assert!((out[4800] - 0.2).abs() < 1e-6);
        assert!((out[9600] - 0.1).abs() < 1e-6);
        assert!((out[14400] - 0.05).abs() < 1e-6);
        settings.echo.mode = GuitarEchoMode::Slapback;
        pedals.set(settings);
        let slap: Vec<f32> = (0..12000)
            .map(|i| pedals.post_cabinet(if i == 0 { 0.5 } else { 0. }))
            .collect();
        assert!((slap[4800] - 0.2).abs() < 1e-6);
        assert!(
            slap[4801..].iter().all(|v| *v == 0.),
            "slapback has one repeat"
        );
    }

    #[test]
    fn tape_echo_darkens_repeats_and_delay_edits_remain_finite() {
        let render = |mode| {
            let mut pedals = GuitarPedals::new(48000);
            let mut s = PedalSettings::BYPASSED;
            s.echo = GuitarEchoSettings {
                mode,
                time_ms: 100.,
                feedback: 0.5,
                mix: 0.4,
            };
            pedals.set(s);
            let mut out = Vec::new();
            for i in 0..24000 {
                let x = if i < 6000 {
                    (TAU * 8000. * i as f32 / 48000.).sin() * 0.2
                } else {
                    0.
                };
                if i == 12000 {
                    s.echo.time_ms = 70.;
                    pedals.set(s);
                }
                out.push(pedals.post_cabinet(x));
            }
            assert!(out.iter().all(|x| x.is_finite()));
            out[7000..10000].iter().map(|x| x * x).sum::<f32>()
        };
        assert!(render(GuitarEchoMode::Tape) < render(GuitarEchoMode::Digital) * 0.5);
    }

    #[test]
    fn modulation_generates_sidebands_and_drive_adds_harmonics() {
        for mode in [
            ModulationMode::Flanger,
            ModulationMode::Chorus,
            ModulationMode::Tremolo,
        ] {
            let mut pedals = GuitarPedals::new(48000);
            let mut s = PedalSettings::BYPASSED;
            s.modulation = ModulationSettings {
                mode,
                rate_hz: 2.,
                depth: 0.8,
                feedback: 0.3,
                mix: 0.5,
            };
            pedals.set(s);
            let out: Vec<_> = (0..60000)
                .map(|i| pedals.post_cabinet((TAU * 880. * i as f32 / 48000.).sin() * 0.2))
                .skip(12000)
                .collect();
            // A chorus can put most energy into higher sidebands. Measure
            // all energy outside the carrier, rather than one Bessel null.
            let carrier = magnitude(&out, 880.);
            let sideband =
                out.iter().map(|x| x * x).sum::<f32>() / out.len() as f32 - carrier * carrier * 0.5;
            assert!(
                sideband > 0.00001,
                "{mode:?}: no modulation sidebands: {sideband}"
            );
        }
        for mode in [
            DriveMode::Boost,
            DriveMode::Overdrive,
            DriveMode::Distortion,
            DriveMode::Fuzz,
        ] {
            let mut pedals = GuitarPedals::new(48000);
            let mut s = PedalSettings::BYPASSED;
            s.drive = DriveSettings {
                mode,
                gain: 0.7,
                tone: 0.8,
                level: 0.5,
            };
            pedals.set(s);
            let out: Vec<_> = (0..12288)
                .map(|i| pedals.pre_amp((TAU * 375. * i as f32 / 48000.).sin() * 0.2))
                .skip(4096)
                .collect();
            let ratio = magnitude(&out, 1125.) / magnitude(&out, 375.);
            if mode == DriveMode::Boost {
                assert!(ratio < 0.001);
            } else {
                assert!(ratio > 0.01, "{mode:?}: no added harmonics: {ratio}");
            }
        }
    }

    #[test]
    fn hall_decays_longer_than_room_and_bypass_discards_old_tails() {
        let tail = |mode| {
            let mut pedals = GuitarPedals::new(48000);
            let mut s = PedalSettings::BYPASSED;
            s.reverb = ReverbSettings {
                mode,
                size: 0.7,
                damping: 0.4,
                mix: 0.4,
            };
            pedals.set(s);
            let out: Vec<_> = (0..96000)
                .map(|i| pedals.post_cabinet(if i == 0 { 0.5 } else { 0. }))
                .collect();
            assert!((out[0] - 0.3).abs() < 1e-6, "dry onset stays immediate");
            let energy: f32 = out[24000..].iter().map(|x| x * x).sum();
            assert!(energy > 0.);
            pedals.set(PedalSettings::BYPASSED);
            assert_eq!(pedals.post_cabinet(0.25), 0.25);
            pedals.set(s);
            assert!(
                (0..96000).all(|_| pedals.post_cabinet(0.) == 0.),
                "bypass must not resurrect a stale tail"
            );
            energy
        };
        assert!(tail(ReverbMode::Hall) > tail(ReverbMode::Room) * 2.);
    }

    #[test]
    fn full_rig_is_bounded_across_rates_modes_and_invalid_controls() {
        for rate in [8000, 44100, 48000, 96000] {
            let mut rig = GuitarRig::new(rate);
            for (index, drive) in [
                DriveMode::Off,
                DriveMode::Boost,
                DriveMode::Overdrive,
                DriveMode::Distortion,
                DriveMode::Fuzz,
            ]
            .into_iter()
            .enumerate()
            {
                let mut p = PedalSettings::preset(PedalPreset::LeadEcho);
                p.drive.mode = drive;
                p.modulation = ModulationSettings {
                    mode: [
                        ModulationMode::Flanger,
                        ModulationMode::Chorus,
                        ModulationMode::Tremolo,
                    ][index % 3],
                    rate_hz: f32::NAN,
                    depth: 1.,
                    feedback: 20.,
                    mix: 1.,
                };
                p.echo.feedback = 20.;
                p.echo.mix = 1.;
                p.reverb.size = 1.;
                p.reverb.damping = f32::INFINITY;
                rig.set(AmpSettings {
                    gain: 1.,
                    level: 1.,
                    cabinet: Cabinet::Off,
                    pedals: p,
                    ..Default::default()
                });
                for i in 0..rate / 5 {
                    let x = if i % 997 == 0 {
                        f32::NAN
                    } else {
                        (i as f32 * 0.13).sin() * 4.
                    };
                    let out = rig.process(x);
                    assert!(
                        out.is_finite() && out.abs() <= 1.,
                        "{rate} Hz {drive:?}: {out}"
                    );
                }
            }
        }
    }
}

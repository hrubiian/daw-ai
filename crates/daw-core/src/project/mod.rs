pub mod format;
mod validation;
mod spatial_presets;
pub use spatial_presets::SpatialPreset;

use serde::{Deserialize, Serialize};

// ── Effects ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectChain {
    // MIDI CC effects (synth/drum tracks)
    #[serde(default)]                        pub reverb:          f32,
    #[serde(default)]                        pub chorus:          f32,
    #[serde(default = "EffectChain::open")]  pub cutoff:          f32,
    #[serde(default)]                        pub resonance:       f32,
    #[serde(default)]                        pub lfo_depth:       f32,
    /// Send level to the shared delay return bus (0..1). `reverb` above is
    /// the send level to the shared reverb return (slice B repurposed it
    /// from a GM CC91 amount — same 0..1 meaning, better reverb).
    #[serde(default)]                        pub send_delay:      f32,
    // DSP effects (audio tracks)
    #[serde(default)]                        pub delay_time_ms:   u32,
    #[serde(default = "EffectChain::dfb")]   pub delay_feedback:  f32,
    #[serde(default)]                        pub delay_mix:       f32,
    #[serde(default)]                        pub drive:           f32,
    // 3-band parametric EQ (audio tracks, applied as biquad filters)
    #[serde(default)]                        pub eq_low_gain:     f32, // dB, -12..+12
    #[serde(default)]                        pub eq_mid_gain:     f32,
    #[serde(default)]                        pub eq_high_gain:    f32,
    #[serde(default = "EffectChain::mf")]    pub eq_mid_freq:     f32, // Hz, 200..8000
    // Compressor (audio tracks; bypass when ratio ≤ 1.0)
    #[serde(default)]                        pub comp_threshold:  f32, // dBFS, -60..0
    #[serde(default = "EffectChain::cr")]    pub comp_ratio:      f32, // 1..20
    #[serde(default = "EffectChain::ca")]    pub comp_attack_ms:  f32,
    #[serde(default = "EffectChain::crel")]  pub comp_release_ms: f32,
}

impl EffectChain {
    fn open() -> f32 { 1.0 }
    fn dfb()  -> f32 { 0.4 }
    fn mf()   -> f32 { 1000.0 }
    fn cr()   -> f32 { 1.0 }
    fn ca()   -> f32 { 10.0 }
    fn crel() -> f32 { 100.0 }
}

impl Default for EffectChain {
    fn default() -> Self {
        EffectChain {
            reverb: 0.0, chorus: 0.0, cutoff: 1.0, resonance: 0.0, lfo_depth: 0.0,
            send_delay: 0.0,
            delay_time_ms: 0, delay_feedback: 0.4, delay_mix: 0.0, drive: 0.0,
            eq_low_gain: 0.0, eq_mid_gain: 0.0, eq_high_gain: 0.0, eq_mid_freq: 1000.0,
            comp_threshold: 0.0, comp_ratio: 1.0, comp_attack_ms: 10.0, comp_release_ms: 100.0,
        }
    }
}

/// A tweakable effect-chain parameter. `MIDI` params apply to soundfont/drum
/// tracks; `AUDIO` params to audio tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectParam {
    Reverb, Chorus, Cutoff, Resonance, LfoDepth, SendDelay,
    DelayTimeMs, DelayFeedback, DelayMix, Drive,
    EqLowGain, EqMidGain, EqHighGain, EqMidFreq,
    CompThreshold, CompRatio, CompAttackMs, CompReleaseMs,
}

impl EffectParam {
    pub const MIDI: [EffectParam; 6] = [
        EffectParam::Reverb, EffectParam::Chorus, EffectParam::Cutoff,
        EffectParam::Resonance, EffectParam::LfoDepth, EffectParam::SendDelay,
    ];
    pub const AUDIO: [EffectParam; 15] = [
        EffectParam::DelayTimeMs, EffectParam::DelayFeedback, EffectParam::DelayMix,
        EffectParam::Cutoff, EffectParam::Drive,
        EffectParam::EqLowGain, EffectParam::EqMidGain, EffectParam::EqHighGain, EffectParam::EqMidFreq,
        EffectParam::CompThreshold, EffectParam::CompRatio, EffectParam::CompAttackMs, EffectParam::CompReleaseMs,
        EffectParam::Reverb, EffectParam::SendDelay,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EffectParam::Reverb => "Reverb", EffectParam::Chorus => "Chorus",
            EffectParam::Cutoff => "Cutoff", EffectParam::Resonance => "Reson",
            EffectParam::LfoDepth => "LFO", EffectParam::SendDelay => "Dly Send",
            EffectParam::DelayTimeMs => "Delay ms",
            EffectParam::DelayFeedback => "Dly FB", EffectParam::DelayMix => "Dly Mix",
            EffectParam::Drive => "Drive", EffectParam::EqLowGain => "EQ Lo",
            EffectParam::EqMidGain => "EQ Mid", EffectParam::EqHighGain => "EQ Hi",
            EffectParam::EqMidFreq => "EQ Frq", EffectParam::CompThreshold => "Cmp Thr",
            EffectParam::CompRatio => "Cmp Rat", EffectParam::CompAttackMs => "Cmp Atk",
            EffectParam::CompReleaseMs => "Cmp Rel",
        }
    }

    pub fn value(self, fx: &EffectChain) -> f32 {
        match self {
            EffectParam::Reverb => fx.reverb, EffectParam::Chorus => fx.chorus,
            EffectParam::Cutoff => fx.cutoff, EffectParam::Resonance => fx.resonance,
            EffectParam::LfoDepth => fx.lfo_depth, EffectParam::SendDelay => fx.send_delay,
            EffectParam::DelayTimeMs => fx.delay_time_ms as f32,
            EffectParam::DelayFeedback => fx.delay_feedback, EffectParam::DelayMix => fx.delay_mix,
            EffectParam::Drive => fx.drive, EffectParam::EqLowGain => fx.eq_low_gain,
            EffectParam::EqMidGain => fx.eq_mid_gain, EffectParam::EqHighGain => fx.eq_high_gain,
            EffectParam::EqMidFreq => fx.eq_mid_freq, EffectParam::CompThreshold => fx.comp_threshold,
            EffectParam::CompRatio => fx.comp_ratio, EffectParam::CompAttackMs => fx.comp_attack_ms,
            EffectParam::CompReleaseMs => fx.comp_release_ms,
        }
    }

    fn apply(self, fx: &mut EffectChain, delta: f32) {
        match self {
            EffectParam::Reverb => fx.reverb = (fx.reverb + delta).clamp(0.0, 1.0),
            EffectParam::Chorus => fx.chorus = (fx.chorus + delta).clamp(0.0, 1.0),
            EffectParam::Cutoff => fx.cutoff = (fx.cutoff + delta).clamp(0.0, 1.0),
            EffectParam::Resonance => fx.resonance = (fx.resonance + delta).clamp(0.0, 1.0),
            EffectParam::LfoDepth => fx.lfo_depth = (fx.lfo_depth + delta).clamp(0.0, 1.0),
            EffectParam::SendDelay => fx.send_delay = (fx.send_delay + delta).clamp(0.0, 1.0),
            EffectParam::DelayTimeMs => fx.delay_time_ms = (fx.delay_time_ms as f32 + delta).clamp(0.0, 2000.0) as u32,
            EffectParam::DelayFeedback => fx.delay_feedback = (fx.delay_feedback + delta).clamp(0.0, 0.95),
            EffectParam::DelayMix => fx.delay_mix = (fx.delay_mix + delta).clamp(0.0, 1.0),
            EffectParam::Drive => fx.drive = (fx.drive + delta).clamp(0.0, 1.0),
            EffectParam::EqLowGain => fx.eq_low_gain = (fx.eq_low_gain + delta).clamp(-12.0, 12.0),
            EffectParam::EqMidGain => fx.eq_mid_gain = (fx.eq_mid_gain + delta).clamp(-12.0, 12.0),
            EffectParam::EqHighGain => fx.eq_high_gain = (fx.eq_high_gain + delta).clamp(-12.0, 12.0),
            EffectParam::EqMidFreq => fx.eq_mid_freq = (fx.eq_mid_freq + delta).clamp(200.0, 8000.0),
            EffectParam::CompThreshold => fx.comp_threshold = (fx.comp_threshold + delta).clamp(-60.0, 0.0),
            EffectParam::CompRatio => fx.comp_ratio = (fx.comp_ratio + delta).clamp(1.0, 20.0),
            EffectParam::CompAttackMs => fx.comp_attack_ms = (fx.comp_attack_ms + delta).clamp(0.5, 100.0),
            EffectParam::CompReleaseMs => fx.comp_release_ms = (fx.comp_release_ms + delta).clamp(10.0, 1000.0),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub version: String,
    pub project: ProjectMeta,
    pub tracks: Vec<Track>,
    pub patterns: Vec<Pattern>,
    pub arrangement: Vec<ArrangementSlot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio_clips: Vec<AudioClip>,
    /// Shared send-return buses (reverb + delay), fed post-fader per track.
    #[serde(default)]
    pub returns: Returns,
    /// Transport loop/punch region.
    #[serde(default)]
    pub loop_region: LoopRegion,
}

// ── Return buses (slice B: two fixed sends — reverb + delay) ─────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Returns {
    #[serde(default)] pub reverb: ReverbReturn,
    #[serde(default)] pub delay:  DelayReturn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReverbReturn {
    #[serde(default = "ReverbReturn::g")]  pub gain:      f32, // 0..1.25
    #[serde(default)]                      pub muted:     bool,
    #[serde(default = "ReverbReturn::h")]  pub room_size: f32, // 0..1
    #[serde(default = "ReverbReturn::h")]  pub damping:   f32, // 0..1
    #[serde(default = "ReverbReturn::g")]  pub width:     f32, // 0..1
    #[serde(default)] pub predelay_ms: u32,
    #[serde(default)] pub low_cut_hz: f32,
    #[serde(default = "return_high_cut")] pub high_cut_hz: f32,
}
impl ReverbReturn {
    fn g() -> f32 { 1.0 }
    fn h() -> f32 { 0.5 }
}
impl Default for ReverbReturn {
    fn default() -> Self {
        ReverbReturn { gain: 1.0, muted: false, room_size: 0.5, damping: 0.5, width: 1.0,
            predelay_ms: 0, low_cut_hz: 0., high_cut_hz: return_high_cut() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelayReturn {
    #[serde(default = "DelayReturn::g")]  pub gain:     f32, // 0..1.25
    #[serde(default)]                     pub muted:    bool,
    #[serde(default = "DelayReturn::t")]  pub time_ms:  u32, // 1..2000
    #[serde(default = "DelayReturn::fb")] pub feedback: f32, // 0..0.95
    /// None keeps the legacy millisecond setting. Synced echoes follow tempo.
    #[serde(default)] pub sync: Option<EchoDivision>,
    #[serde(default)] pub mode: EchoMode,
    #[serde(default)] pub low_cut_hz: f32,
    #[serde(default = "return_high_cut")] pub high_cut_hz: f32,
    #[serde(default = "DelayReturn::g")] pub width: f32,
    #[serde(default)] pub modulation_ms: f32,
    #[serde(default = "echo_mod_rate")] pub modulation_hz: f32,
}
fn return_high_cut() -> f32 { 20_000. }
fn echo_mod_rate() -> f32 { 0.3 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EchoMode { #[default] PingPong, Stereo }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EchoDivision { Sixteenth, Eighth, EighthDotted, EighthTriplet, Quarter, QuarterDotted, Half, Whole }
impl EchoDivision {
    pub const ALL: [Self; 8] = [Self::Sixteenth, Self::EighthTriplet, Self::Eighth, Self::EighthDotted, Self::Quarter, Self::QuarterDotted, Self::Half, Self::Whole];
    pub fn beats(self) -> f64 { match self {
        Self::Sixteenth => 0.25, Self::Eighth => 0.5, Self::EighthDotted => 0.75,
        Self::EighthTriplet => 1./3., Self::Quarter => 1., Self::QuarterDotted => 1.5,
        Self::Half => 2., Self::Whole => 4.,
    } }
    pub fn label(self) -> &'static str { match self {
        Self::Sixteenth => "1/16", Self::Eighth => "1/8", Self::EighthDotted => "1/8 dot",
        Self::EighthTriplet => "1/8 tri", Self::Quarter => "1/4", Self::QuarterDotted => "1/4 dot",
        Self::Half => "1/2", Self::Whole => "1/1",
    } }
}
impl DelayReturn {
    fn g() -> f32 { 1.0 }
    fn t() -> u32 { 300 }
    fn fb() -> f32 { 0.35 }
    pub fn effective_time_ms(&self, bpm: f64) -> u32 {
        self.sync.map_or(self.time_ms, |division| (60_000. / bpm.clamp(20., 300.) * division.beats()).round() as u32)
    }
}
impl Default for DelayReturn {
    fn default() -> Self {
        DelayReturn { gain: 1.0, muted: false, time_ms: 300, feedback: 0.35,
            sync: None, mode: EchoMode::default(), low_cut_hz: 0., high_cut_hz: return_high_cut(),
            width: 1., modulation_ms: 0., modulation_hz: echo_mod_rate() }
    }
}

/// Transport loop/punch region (bars; `end_bar` exclusive). One region,
/// three switches — loop playback + punch-in/out recording gates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopRegion {
    #[serde(default)]                   pub start_bar: u32,
    #[serde(default = "LoopRegion::e")] pub end_bar:   u32,
    #[serde(default)]                   pub enabled:   bool,
    #[serde(default)]                   pub punch_in:  bool,
    #[serde(default)]                   pub punch_out: bool,
}
impl LoopRegion {
    fn e() -> u32 { 4 }
    /// Region bounds in ticks.
    pub fn ticks(&self) -> (u32, u32) {
        (self.start_bar * crate::sequencer::TICKS_PER_BAR,
         self.end_bar * crate::sequencer::TICKS_PER_BAR)
    }
}
impl Default for LoopRegion {
    fn default() -> Self {
        LoopRegion { start_bar: 0, end_bar: 4, enabled: false, punch_in: false, punch_out: false }
    }
}

/// A tweakable return-bus parameter (mixer return strips / FX dock).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnParam {
    RevGain, RevRoomSize, RevDamping, RevWidth,
    DlyGain, DlyTimeMs, DlyFeedback,
    RevPredelay, RevLowCut, RevHighCut,
    DlyLowCut, DlyHighCut, DlyWidth, DlyModDepth, DlyModRate,
}

impl ReturnParam {
    pub const REVERB: [ReturnParam; 7] = [
        ReturnParam::RevGain, ReturnParam::RevRoomSize,
        ReturnParam::RevDamping, ReturnParam::RevWidth,
        Self::RevPredelay, Self::RevLowCut, Self::RevHighCut,
    ];
    pub const DELAY: [ReturnParam; 8] = [
        ReturnParam::DlyGain, ReturnParam::DlyTimeMs, ReturnParam::DlyFeedback,
        Self::DlyLowCut, Self::DlyHighCut, Self::DlyWidth, Self::DlyModDepth, Self::DlyModRate,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ReturnParam::RevGain => "Gain",    ReturnParam::RevRoomSize => "Room",
            ReturnParam::RevDamping => "Damp", ReturnParam::RevWidth => "Width",
            ReturnParam::DlyGain => "Gain",    ReturnParam::DlyTimeMs => "Time ms",
            ReturnParam::DlyFeedback => "Feedbk",
            Self::RevPredelay => "Pre-delay ms", Self::RevLowCut | Self::DlyLowCut => "Low cut Hz",
            Self::RevHighCut | Self::DlyHighCut => "High cut Hz", Self::DlyWidth => "Width",
            Self::DlyModDepth => "Wobble ms", Self::DlyModRate => "Wobble Hz",
        }
    }

    pub fn value(self, r: &Returns) -> f32 {
        match self {
            ReturnParam::RevGain => r.reverb.gain,
            ReturnParam::RevRoomSize => r.reverb.room_size,
            ReturnParam::RevDamping => r.reverb.damping,
            ReturnParam::RevWidth => r.reverb.width,
            ReturnParam::DlyGain => r.delay.gain,
            ReturnParam::DlyTimeMs => r.delay.time_ms as f32,
            ReturnParam::DlyFeedback => r.delay.feedback,
            Self::RevPredelay => r.reverb.predelay_ms as f32,
            Self::RevLowCut => r.reverb.low_cut_hz, Self::RevHighCut => r.reverb.high_cut_hz,
            Self::DlyLowCut => r.delay.low_cut_hz, Self::DlyHighCut => r.delay.high_cut_hz,
            Self::DlyWidth => r.delay.width, Self::DlyModDepth => r.delay.modulation_ms,
            Self::DlyModRate => r.delay.modulation_hz,
        }
    }

    fn apply(self, r: &mut Returns, delta: f32) {
        match self {
            ReturnParam::RevGain => r.reverb.gain = (r.reverb.gain + delta).clamp(0.0, 1.25),
            ReturnParam::RevRoomSize => r.reverb.room_size = (r.reverb.room_size + delta).clamp(0.0, 1.0),
            ReturnParam::RevDamping => r.reverb.damping = (r.reverb.damping + delta).clamp(0.0, 1.0),
            ReturnParam::RevWidth => r.reverb.width = (r.reverb.width + delta).clamp(0.0, 1.0),
            ReturnParam::DlyGain => r.delay.gain = (r.delay.gain + delta).clamp(0.0, 1.25),
            ReturnParam::DlyTimeMs => { r.delay.sync = None; r.delay.time_ms = (r.delay.time_ms as f32 + delta).clamp(1.0, 12000.0) as u32; },
            ReturnParam::DlyFeedback => r.delay.feedback = (r.delay.feedback + delta).clamp(0.0, 0.95),
            Self::RevPredelay => r.reverb.predelay_ms = (r.reverb.predelay_ms as f32 + delta).clamp(0., 250.) as u32,
            Self::RevLowCut => r.reverb.low_cut_hz = (r.reverb.low_cut_hz+delta).clamp(0., 2000.).min(r.reverb.high_cut_hz-10.),
            Self::RevHighCut => r.reverb.high_cut_hz = (r.reverb.high_cut_hz+delta).clamp(500., 20000.).max(r.reverb.low_cut_hz+10.),
            Self::DlyLowCut => r.delay.low_cut_hz = (r.delay.low_cut_hz+delta).clamp(0., 2000.).min(r.delay.high_cut_hz-10.),
            Self::DlyHighCut => r.delay.high_cut_hz = (r.delay.high_cut_hz+delta).clamp(500., 20000.).max(r.delay.low_cut_hz+10.),
            Self::DlyWidth => r.delay.width = (r.delay.width+delta).clamp(0., 1.),
            Self::DlyModDepth => r.delay.modulation_ms = (r.delay.modulation_ms+delta).clamp(0., 20.),
            Self::DlyModRate => r.delay.modulation_hz = (r.delay.modulation_hz+delta).clamp(0.05, 8.),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub name: String,
    pub bpm: f64,
    pub time_signature: [u8; 2],
    pub key: String,
    pub scale: String,
    #[serde(default)]
    pub swing: f32,
    #[serde(default)]
    pub metronome: bool,
    /// Velocity humanization amount, 0.0 (off) ..= 1.0 (±35% variation).
    #[serde(default)]
    pub humanize: f32,
    /// Note-timing humanization, 0.0 (dead-on grid) ..= 1.0 (±~32nd jitter).
    #[serde(default)]
    pub timing: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub track_type: TrackType,
    pub instrument: Instrument,
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub patterns: Vec<String>,
    #[serde(default)]
    pub effects: EffectChain,
    /// Per-track LFO modulators (the modulation matrix).
    #[serde(default)]
    pub modulators: Vec<Modulator>,
    /// CLAP insert-effect chain, processed after the built-in effects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clap_fx: Vec<ClapSlot>,
    /// Constant microtonal detune for the whole track, in cents (100 = one
    /// semitone up, negative = down). Applied as a fixed pitch bend to the
    /// strip's synth, so every note on this track sounds this many cents off
    /// 12-TET — letting a voice sit between the piano keys. Effective range is
    /// the synth's default ±200-cent bend. Defaults to 0 (older files unchanged).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub tuning_cents: f32,
}

// ── Modulation (per-track LFO matrix) ──────────────────────────────────────────

/// A parameter an LFO can drive.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModDest { Cutoff, Resonance, Volume, Pan }

/// LFO waveform.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LfoShape { Sine, Triangle, Saw, Square }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Modulator {
    pub dest:    ModDest,
    pub shape:   LfoShape,
    /// LFO frequency in Hz (e.g. 0.1 ≈ a slow ~10 s breath).
    pub rate_hz: f32,
    /// Modulation amount, 0..1, added/subtracted around the base value.
    pub depth:   f32,
    /// Starting phase offset, 0..1.
    #[serde(default)]
    pub phase:   f32,
}

impl ModDest {
    /// MIDI CC number this destination maps to for soundfont tracks.
    pub fn cc(self) -> u8 {
        match self {
            ModDest::Cutoff    => 74,
            ModDest::Resonance => 71,
            ModDest::Volume    => 7,
            ModDest::Pan       => 10,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ModDest::Cutoff => "cutoff", ModDest::Resonance => "reson",
            ModDest::Volume => "volume", ModDest::Pan => "pan",
        }
    }
}

impl LfoShape {
    pub fn label(self) -> &'static str {
        match self {
            LfoShape::Sine => "sine", LfoShape::Triangle => "tri",
            LfoShape::Saw => "saw", LfoShape::Square => "sqr",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackType {
    Drum,
    Melodic,
    Audio,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Instrument {
    Soundfont {
        preset: u8,
        bank: u8,
        /// Optional path to a specific `.sf2`. `None` (the default) uses the
        /// project's main soundfont — so existing projects are unaffected.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        soundfont: Option<String>,
    },
    AudioFile { path: String },
    /// One-shot drum sampler: each pad maps a MIDI pitch to a `.wav` sample,
    /// triggered (polyphonically) when the pattern plays that pitch.
    Sampler { pads: Vec<SamplePad> },
    /// A hosted CLAP instrument plugin.
    Clap(ClapSlot),
}

/// One hosted CLAP plugin slot (instrument or insert effect).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClapSlot {
    /// Stable identity — plugin instances are cached by this across edits.
    pub uid: String,
    /// Path to the `.clap` bundle.
    pub path: String,
    /// Plugin id within the bundle (e.g. "org.surge-synth-team.surge-xt").
    pub id: String,
    /// Opaque plugin state, base64 (captured on save).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Param overrides set through the UI: stringified param id -> value.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub params: std::collections::BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SamplePad {
    pub pitch: u8,
    pub path: String,
    #[serde(default = "SamplePad::unity")]
    pub gain: f32,
    /// Matching velocity ranges select a timbre; equal ranges rotate takes.
    /// Empty keeps legacy single-sample playback. Gaps use `path` as fallback.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<SampleVariant>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleVariant {
    pub path: String,
    pub min_velocity: u8,
    pub max_velocity: u8,
    #[serde(default = "SamplePad::unity")]
    pub gain: f32,
}

impl SamplePad {
    fn unity() -> f32 { 1.0 }
    pub fn validate_variants(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.variants.len() <= 32, "at most 32 variants per sampler pad");
        for (i, v) in self.variants.iter().enumerate() {
            anyhow::ensure!(!v.path.trim().is_empty(), "empty sampler variant path");
            anyhow::ensure!(v.min_velocity >= 1 && v.min_velocity <= v.max_velocity
                && v.max_velocity <= 127, "invalid sampler velocity range");
            anyhow::ensure!(v.gain.is_finite() && (0.0..=2.0).contains(&v.gain), "invalid sampler variant gain");
            for other in &self.variants[..i] {
                let overlap = v.min_velocity <= other.max_velocity && other.min_velocity <= v.max_velocity;
                anyhow::ensure!(!overlap || (v.min_velocity == other.min_velocity && v.max_velocity == other.max_velocity),
                    "sampler velocity ranges must be disjoint or identical for alternate takes");
            }
        }
        Ok(())
    }
}

/// Minimum cropped length of an audio clip, in seconds.
pub const MIN_CLIP_SEC: f64 = 0.01;

fn default_gain() -> f32 { 1.0 }

/// A raw WAV placed on an audio track's timeline. Times are in seconds
/// (sample-rate-agnostic); the engine rounds to samples at playback time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioClip {
    /// id of the (TrackType::Audio) track this clip belongs to.
    pub track: String,
    /// Source WAV path.
    pub path: String,
    /// Position on the song timeline, seconds from the start.
    pub start_sec: f64,
    /// Crop-in: seconds into the source where playback begins.
    pub offset_sec: f64,
    /// Cropped duration in seconds.
    pub len_sec: f64,
    /// Linear gain, default 1.0.
    #[serde(default = "default_gain")]
    pub gain: f32,
}

/// Resolve the set of soundfonts a project needs.
///
/// Returns the ordered list of unique soundfont paths (index 0 is always the
/// main/default soundfont) and, for each track, the index of the soundfont it
/// plays through. Tracks with no explicit soundfont (or non-soundfont tracks)
/// map to index 0. This is the routing table the engine/exporter use to send
/// each track's MIDI to the right synthesizer.
pub fn resolve_soundfonts(tracks: &[Track], default_path: &str) -> (Vec<String>, Vec<usize>) {
    let mut paths = vec![default_path.to_string()];
    let per_track = tracks.iter().map(|t| {
        match &t.instrument {
            Instrument::Soundfont { soundfont: Some(p), .. } if p != default_path && !p.is_empty() => {
                if let Some(i) = paths.iter().position(|x| x == p) {
                    i
                } else {
                    paths.push(p.clone());
                    paths.len() - 1
                }
            }
            _ => 0,
        }
    }).collect();
    (paths, per_track)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Pattern {
    Step(StepPattern),
    PianoRoll(PianoRollPattern),
}

impl Pattern {
    pub fn id(&self) -> &str {
        match self {
            Pattern::Step(p) => &p.id,
            Pattern::PianoRoll(p) => &p.id,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            Pattern::Step(p) => &p.name,
            Pattern::PianoRoll(p) => &p.name,
        }
    }
    pub fn bars(&self) -> u32 {
        match self {
            // 16th-note steps: a pattern spans ceil(steps/16) bars.
            Pattern::Step(p) => ((p.steps as u32) + 15) / 16,
            Pattern::PianoRoll(p) => p.bars as u32,
        }
    }
}

/// Returns the MIDI channel assignment for each track, in order.
/// Drum tracks → channel 9. Melodic tracks → 0..=8 in order. Audio tracks → None.
pub fn channel_assignments(tracks: &[Track]) -> Vec<Option<u8>> {
    let mut result = Vec::with_capacity(tracks.len());
    let mut melodic_ch: u8 = 0;
    for track in tracks {
        // Sampler tracks play their own audio, not the shared synth.
        if matches!(track.instrument, Instrument::Sampler { .. }) {
            result.push(None);
            continue;
        }
        match track.track_type {
            TrackType::Audio   => result.push(None),
            TrackType::Drum    => result.push(Some(9)),
            TrackType::Melodic => {
                let ch = melodic_ch.min(8);
                melodic_ch += 1;
                result.push(Some(ch));
            }
        }
    }
    result
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepPattern {
    pub id: String,
    pub name: String,
    pub bars: u8,
    pub steps: u16,
    pub rows: Vec<StepRow>,
    /// Note length as a fraction of one step. `0.5` (default) reproduces the
    /// legacy half-step gate; `>1.0` overlaps into the next step (legato),
    /// `<0.5` is tighter/staccato.
    #[serde(default = "StepPattern::default_gate")]
    pub gate: f32,
}

impl StepPattern {
    fn default_gate() -> f32 { 0.5 }

    /// Note length in ticks for one step, given total steps in this pattern.
    pub fn step_note_ticks(&self, ticks_per_step: u32) -> u32 {
        ((ticks_per_step as f32) * self.gate).round().max(1.0) as u32
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRow {
    pub name: String,
    pub pitch: u8,
    pub vel: u8,
    /// Per-step velocity. `0` = off, `1` = on at the row's default `vel`
    /// (back-compat with on/off projects), `2..=127` = explicit velocity.
    pub steps: Vec<u8>,
}

impl StepRow {
    /// Velocity for step `idx`, or `None` if the step is off or out of range.
    pub fn step_velocity(&self, idx: usize) -> Option<u8> {
        match self.steps.get(idx).copied() {
            None | Some(0) => None,
            Some(1)        => Some(self.vel),
            Some(v)        => Some(v),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PianoRollPattern {
    pub id: String,
    pub name: String,
    pub bars: u8,
    pub ticks_per_bar: u32,
    pub notes: Vec<Note>,
    /// Legato: connect each note to the next (note-off at the next note's
    /// start). Off by default. Tuned for melodic lines.
    #[serde(default)]
    pub legato: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub pitch: u8,
    pub tick: u32,
    pub duration: u32,
    pub vel: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArrangementSlot {
    pub track: String,
    pub pattern: String,
    pub bar: u32,
    /// Sub-bar start offset in ticks (480/bar). `0` = exactly on the bar line;
    /// e.g. `240` starts the clip half a bar late. The clip's absolute start is
    /// `bar * TICKS_PER_BAR + offset_ticks`. Kept in `[0, TICKS_PER_BAR)` —
    /// whole bars fold into `bar`. Defaults to 0 so older files load unchanged.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub offset_ticks: u32,
    /// Clip length in bars. `None` = the pattern's natural bar count. `Some(n)`
    /// = the clip spans `n` bars and the pattern loops to fill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub len_bars: Option<u32>,
}

fn is_zero_u32(n: &u32) -> bool { *n == 0 }
fn is_zero_f32(x: &f32) -> bool { *x == 0.0 }

/// Fold whole bars out of a tick offset so the invariant `offset < TICKS_PER_BAR`
/// always holds: `(bar, off)` → `(bar + off/TPB, off % TPB)`.
fn normalize_bar_offset(bar: u32, offset_ticks: u32) -> (u32, u32) {
    (bar + offset_ticks / ARR_TICKS_PER_BAR, offset_ticks % ARR_TICKS_PER_BAR)
}

/// Ticks per bar on the arrangement timeline (4/4, 480/bar). Mirrors
/// `sequencer::TICKS_PER_BAR`; duplicated here so `daw-core::project` has no
/// dependency on the sequencer module for the bar→tick conversion.
pub const ARR_TICKS_PER_BAR: u32 = 480;

impl ArrangementSlot {
    /// Absolute start position on the timeline, in ticks.
    pub fn start_tick(&self) -> u32 {
        self.bar * ARR_TICKS_PER_BAR + self.offset_ticks
    }
}

impl Project {
    pub fn default_project() -> Self {
        Project {
            version: "0.1".into(),
            project: ProjectMeta {
                name: "New Song".into(),
                bpm: 120.0,
                time_signature: [4, 4],
                key: "C".into(),
                scale: "major".into(),
                swing: 0.0,
                metronome: false,
                humanize: 0.0, timing: 0.0,
            },
            tracks: vec![
                Track {
                    id: "t1".into(),
                    name: "Drums".into(),
                    track_type: TrackType::Drum,
                    instrument: Instrument::Soundfont { preset: 0, bank: 128, soundfont: None },
                    volume: 0.8,
                    pan: 0.0,
                    muted: false,
                    patterns: vec!["p1".into()],
                    effects: EffectChain::default(),
                    modulators: Vec::new(),
                    clap_fx: Vec::new(),
                    tuning_cents: 0.0,
                },
                Track {
                    id: "t2".into(),
                    name: "Bass".into(),
                    track_type: TrackType::Melodic,
                    instrument: Instrument::Soundfont { preset: 32, bank: 0, soundfont: None },
                    volume: 0.75,
                    pan: 0.0,
                    muted: false,
                    patterns: vec!["p2".into()],
                    effects: EffectChain::default(),
                    modulators: Vec::new(),
                    clap_fx: Vec::new(),
                    tuning_cents: 0.0,
                },
            ],
            patterns: vec![
                Pattern::Step(StepPattern {
                    id: "p1".into(),
                    name: "Drums A".into(),
                    bars: 1,
                    steps: 16,
                    rows: vec![
                        StepRow { name: "Kick".into(),   pitch: 36, vel: 100, steps: vec![1,0,0,0, 1,0,0,0, 1,0,0,0, 1,0,0,0] },
                        StepRow { name: "Snare".into(),  pitch: 38, vel: 80,  steps: vec![0,0,0,0, 1,0,0,0, 0,0,0,0, 1,0,0,0] },
                        StepRow { name: "Hi-hat".into(), pitch: 42, vel: 60,  steps: vec![1,0,1,0, 1,0,1,0, 1,0,1,0, 1,0,1,0] },
                    ],
                    gate: StepPattern::default_gate(),
                }),
                Pattern::PianoRoll(PianoRollPattern {
                    id: "p2".into(),
                    name: "Bass Line A".into(),
                    bars: 2,
                    ticks_per_bar: 480,
                    notes: vec![
                        Note { pitch: 36, tick: 0,    duration: 240, vel: 80 },
                        Note { pitch: 38, tick: 480,  duration: 240, vel: 75 },
                        Note { pitch: 41, tick: 720,  duration: 120, vel: 70 },
                    ],
                    legato: false,
                }),
            ],
            arrangement: vec![
                ArrangementSlot { track: "t1".into(), pattern: "p1".into(), bar: 0, offset_ticks: 0, len_bars: None },
                ArrangementSlot { track: "t2".into(), pattern: "p2".into(), bar: 0, offset_ticks: 0, len_bars: None },
            ],
            audio_clips: Vec::new(),
            returns: Returns::default(),
            loop_region: LoopRegion::default(),
        }
    }

    fn track_mut(&mut self, id: &str) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// Set a track's volume by id. Returns `false` if no such track.
    pub fn set_track_volume(&mut self, id: &str, volume: f32) -> bool {
        match self.track_mut(id) {
            Some(t) => { t.volume = volume; true }
            None => false,
        }
    }

    /// Change an existing note's dynamics without moving or stretching it.
    pub fn set_note_velocity(&mut self, pid: &str, pitch: u8, tick: u32, velocity: u8) -> bool {
        if velocity == 0 || velocity > 127 { return false; }
        let Some(Pattern::PianoRoll(pattern)) = self.patterns.iter_mut().find(|p| p.id() == pid) else { return false };
        let Some(note) = pattern.notes.iter_mut().find(|n| n.pitch == pitch && n.tick == tick) else { return false };
        note.vel = velocity;
        true
    }

    /// Set note duration (minimum 30 ticks), growing pattern length when needed.
    pub fn set_note_duration(&mut self, pid: &str, pitch: u8, tick: u32, duration: u32) -> bool {
        for p in &mut self.patterns {
            if let Pattern::PianoRoll(pr) = p {
                if pr.id == pid {
                    let Some(n) = pr.notes.iter_mut().find(|n| n.pitch == pitch && n.tick == tick)
                    else {
                        return false;
                    };
                    n.duration = duration.max(30);
                    let end = tick + n.duration;
                    let tpb = pr.ticks_per_bar.max(1);
                    let need = ((end + tpb - 1) / tpb).min(64) as u8;
                    if need > pr.bars {
                        pr.bars = need;
                    }
                    return true;
                }
            }
        }
        false
    }

    /// Grow/shrink a piano-roll pattern by whole bars (clamped 1..=64).
    /// Notes beyond a shortened length are kept (they just don't play).
    pub fn resize_pr_bars(&mut self, pid: &str, delta: i32) -> bool {
        for p in &mut self.patterns {
            if let Pattern::PianoRoll(pr) = p {
                if pr.id == pid {
                    let new = (pr.bars as i32 + delta).clamp(1, 64) as u8;
                    if new == pr.bars {
                        return false;
                    }
                    pr.bars = new;
                    return true;
                }
            }
        }
        false
    }

    /// Rename a track. `false` for unknown ids.
    pub fn set_track_name(&mut self, id: &str, name: &str) -> bool {
        match self.track_mut(id) {
            Some(t) => { t.name = name.to_string(); true }
            None => false,
        }
    }

    /// Rename a pattern (step or piano-roll). `false` for unknown ids.
    pub fn set_pattern_name(&mut self, pid: &str, name: &str) -> bool {
        for p in &mut self.patterns {
            match p {
                Pattern::Step(sp) if sp.id == pid => { sp.name = name.to_string(); return true; }
                Pattern::PianoRoll(pr) if pr.id == pid => { pr.name = name.to_string(); return true; }
                _ => {}
            }
        }
        false
    }

    /// Rename a step-pattern row. `false` for unknown pattern/row.
    pub fn set_row_name(&mut self, pid: &str, row: usize, name: &str) -> bool {
        let Some(sp) = self.step_pattern_mut(pid) else { return false };
        match sp.rows.get_mut(row) {
            Some(r) => { r.name = name.to_string(); true }
            None => false,
        }
    }

    /// Set a track's pan (-1..=1) by id. Returns `false` if no such track.
    pub fn set_track_pan(&mut self, id: &str, pan: f32) -> bool {
        match self.track_mut(id) {
            Some(t) => { t.pan = pan; true }
            None => false,
        }
    }

    /// Set a track's mute flag by id. Returns `false` if no such track.
    pub fn set_track_muted(&mut self, id: &str, muted: bool) -> bool {
        match self.track_mut(id) {
            Some(t) => { t.muted = muted; true }
            None => false,
        }
    }

    /// Set (or insert) the Sampler pad for `pitch` on track `id` to `path`.
    /// Updates the pad in place if one already exists at that pitch, else appends
    /// a new pad at unity gain. Returns `false` if the track is missing or isn't
    /// a Sampler.
    pub fn set_sampler_pad(&mut self, id: &str, pitch: u8, path: String) -> bool {
        let Some(t) = self.track_mut(id) else { return false };
        let Instrument::Sampler { pads } = &mut t.instrument else { return false };
        match pads.iter_mut().find(|p| p.pitch == pitch) {
            Some(pad) => { pad.path = path; pad.variants.clear(); },
            None => pads.push(SamplePad { pitch, path, gain: 1.0, variants: Vec::new() }),
        }
        true
    }

    /// Replace alternate takes atomically; the legacy pad remains the fallback.
    pub fn set_sampler_variants(&mut self, id: &str, pitch: u8, variants: Vec<SampleVariant>) -> Result<(), String> {
        let Some(t) = self.track_mut(id) else { return Err(format!("no track {id}")) };
        let Instrument::Sampler { pads } = &mut t.instrument else { return Err("track is not a sampler".into()) };
        let pad = pads.iter_mut().find(|p| p.pitch == pitch).ok_or_else(|| format!("no pad at pitch {pitch}"))?;
        let mut updated = pad.clone();
        updated.variants = variants;
        updated.validate_variants().map_err(|e| e.to_string())?;
        *pad = updated;
        Ok(())
    }

    pub fn add_sampler_variant(&mut self, id: &str, pitch: u8, variant: SampleVariant) -> Result<(), String> {
        let variants = self.tracks.iter().find(|t| t.id == id).and_then(|t| match &t.instrument {
            Instrument::Sampler { pads } => pads.iter().find(|p| p.pitch == pitch).map(|p| p.variants.clone()),
            _ => None,
        }).ok_or_else(|| "select an existing sampler pad first".to_string())?;
        let mut variants = variants;
        variants.push(variant);
        self.set_sampler_variants(id, pitch, variants)
    }

    /// Explicit velocity: 0 disables, 2..127 sets dynamics; 1 is the legacy row-default marker.
    pub fn set_step_velocity(&mut self, pattern_id: &str, row: usize, step: usize, velocity: u8) -> bool {
        if velocity == 1 || velocity > 127 { return false; }
        let Some(Pattern::Step(pattern)) = self.patterns.iter_mut().find(|p| p.id() == pattern_id) else { return false };
        let Some(cell) = pattern.rows.get_mut(row).and_then(|r| r.steps.get_mut(step)) else { return false };
        *cell = velocity;
        true
    }

    /// Toggle a step cell on/off in a step pattern. Returns the new on/off state,
    /// or `None` if the pattern isn't a step pattern or the row/step is out of range.
    /// Turning a cell on uses the legacy "on at row velocity" marker (`1`).
    pub fn toggle_step(&mut self, pattern_id: &str, row: usize, step: usize) -> Option<bool> {
        let pat = self.patterns.iter_mut().find(|p| p.id() == pattern_id)?;
        let Pattern::Step(sp) = pat else { return None };
        let cell = sp.rows.get_mut(row)?.steps.get_mut(step)?;
        *cell = if *cell == 0 { 1 } else { 0 };
        Some(*cell != 0)
    }

    /// Toggle a note in a piano-roll pattern: if a note already starts at
    /// `(pitch, tick)` it is removed; otherwise one of `duration` ticks at the
    /// row's default velocity (80) is added and the note list is re-sorted.
    /// Returns `Some(true)` if a note now exists there, `Some(false)` if removed,
    /// or `None` if the pattern isn't a piano-roll pattern / doesn't exist.
    pub fn toggle_note(&mut self, pattern_id: &str, pitch: u8, tick: u32, duration: u32) -> Option<bool> {
        let pat = self.patterns.iter_mut().find(|p| p.id() == pattern_id)?;
        let Pattern::PianoRoll(pr) = pat else { return None };
        if let Some(idx) = pr.notes.iter().position(|n| n.pitch == pitch && n.tick == tick) {
            pr.notes.remove(idx);
            Some(false)
        } else {
            // TODO: use the pattern/track default velocity instead of a fixed 80
            pr.notes.push(Note { pitch, tick, duration, vel: 80 });
            pr.notes.sort_by_key(|n| (n.tick, n.pitch));
            Some(true)
        }
    }

    /// Total bars the arrangement spans, with a minimum of 8 so a fresh/empty
    /// project still shows a usable timeline. Mirrors the TUI's `song_bars`.
    pub fn song_bars(&self) -> u32 {
        let pattern_bars = self.arrangement.iter()
            .filter_map(|s| {
                let pat = self.patterns.iter().find(|p| p.id() == s.pattern)?;
                // Use the clip's effective length so resized clips extend the timeline.
                Some(s.bar + s.len_bars.unwrap_or_else(|| pat.bars()))
            })
            .max()
            .unwrap_or(0);

        // Audio clips contribute their end position, converted to bars via BPM.
        let bps = self.project.time_signature[0].max(1) as f64; // beats per bar
        let bar_seconds = (60.0 / self.project.bpm) * bps;
        let audio_bars = self.audio_clips.iter()
            .map(|c| ((c.start_sec + c.len_sec) / bar_seconds).ceil() as u32)
            .max()
            .unwrap_or(0);

        pattern_bars.max(audio_bars).max(8)
    }

    /// Apply a clamped delta to an effect parameter on a track. `false` if no such track.
    pub fn nudge_effect(&mut self, track_id: &str, param: EffectParam, delta: f32) -> bool {
        match self.track_mut(track_id) {
            Some(t) => { param.apply(&mut t.effects, delta); true }
            None => false,
        }
    }

    /// Nudge a return-bus parameter (clamped per param).
    pub fn nudge_return(&mut self, param: ReturnParam, delta: f32) {
        param.apply(&mut self.returns, delta);
    }

    /// Toggle mute on return 0 (reverb) or 1 (delay). Other indices: no-op.
    pub fn toggle_return_mute(&mut self, idx: usize) {
        match idx {
            0 => self.returns.reverb.muted = !self.returns.reverb.muted,
            1 => self.returns.delay.muted = !self.returns.delay.muted,
            _ => {}
        }
    }

    /// Set the loop region (bars). `end` is clamped to at least `start + 1`.
    pub fn set_loop_region(&mut self, start_bar: u32, end_bar: u32) {
        self.loop_region.start_bar = start_bar;
        self.loop_region.end_bar = end_bar.max(start_bar + 1);
    }
    pub fn toggle_loop(&mut self) {
        self.loop_region.enabled = !self.loop_region.enabled;
    }
    pub fn toggle_punch_in(&mut self) {
        self.loop_region.punch_in = !self.loop_region.punch_in;
    }
    pub fn toggle_punch_out(&mut self) {
        self.loop_region.punch_out = !self.loop_region.punch_out;
    }

    /// Next stable CLAP slot uid ("c<N>"): max-scan so hand-edited files
    /// can't collide.
    fn next_clap_uid(&self) -> String {
        let max = self
            .tracks
            .iter()
            .flat_map(|t| {
                t.clap_fx
                    .iter()
                    .map(|s| s.uid.as_str())
                    .chain(match &t.instrument {
                        Instrument::Clap(s) => Some(s.uid.as_str()),
                        _ => None,
                    })
            })
            .filter_map(|u| u.strip_prefix('c').and_then(|n| n.parse::<u64>().ok()))
            .max()
            .unwrap_or(0);
        format!("c{}", max + 1)
    }

    /// Append a CLAP effect to a track's chain. Returns the new slot's uid.
    pub fn add_clap_fx(&mut self, track_id: &str, path: &str, plugin_id: &str) -> Option<String> {
        let uid = self.next_clap_uid();
        let t = self.track_mut(track_id)?;
        t.clap_fx.push(ClapSlot {
            uid: uid.clone(),
            path: path.into(),
            id: plugin_id.into(),
            state: None,
            params: Default::default(),
        });
        Some(uid)
    }

    pub fn remove_clap_fx(&mut self, track_id: &str, uid: &str) -> bool {
        match self.track_mut(track_id) {
            Some(t) => {
                let before = t.clap_fx.len();
                t.clap_fx.retain(|s| s.uid != uid);
                t.clap_fx.len() != before
            }
            None => false,
        }
    }

    /// Make a track's instrument a CLAP plugin. Returns the slot uid.
    pub fn set_track_clap_instrument(
        &mut self,
        track_id: &str,
        path: &str,
        plugin_id: &str,
    ) -> Option<String> {
        let uid = self.next_clap_uid();
        let t = self.track_mut(track_id)?;
        t.instrument = Instrument::Clap(ClapSlot {
            uid: uid.clone(),
            path: path.into(),
            id: plugin_id.into(),
            state: None,
            params: Default::default(),
        });
        Some(uid)
    }

    /// Set a param override on whichever slot (instrument or fx) has `uid`.
    pub fn set_clap_param(&mut self, uid: &str, param_id: u32, value: f64) -> bool {
        for t in &mut self.tracks {
            if let Instrument::Clap(s) = &mut t.instrument {
                if s.uid == uid {
                    s.params.insert(param_id.to_string(), value);
                    return true;
                }
            }
            if let Some(s) = t.clap_fx.iter_mut().find(|s| s.uid == uid) {
                s.params.insert(param_id.to_string(), value);
                return true;
            }
        }
        false
    }

    /// Store a captured state blob (base64) on the slot with `uid`.
    pub fn set_clap_state(&mut self, uid: &str, state_b64: String) -> bool {
        for t in &mut self.tracks {
            if let Instrument::Clap(s) = &mut t.instrument {
                if s.uid == uid {
                    s.state = Some(state_b64);
                    return true;
                }
            }
            if let Some(s) = t.clap_fx.iter_mut().find(|s| s.uid == uid) {
                s.state = Some(state_b64);
                return true;
            }
        }
        false
    }

    /// Set the soundfont preset on a soundfont track, preserving bank + soundfont path.
    /// `false` for unknown tracks or non-soundfont instruments.
    pub fn set_track_preset(&mut self, track_id: &str, preset: u8) -> bool {
        match self.track_mut(track_id) {
            Some(t) => match &mut t.instrument {
                Instrument::Soundfont { preset: p, .. } => { *p = preset; true }
                _ => false,
            },
            None => false,
        }
    }

    /// Transpose all piano-roll notes by `semitones` and rotate the project key.
    pub fn transpose(&mut self, semitones: i32) {
        for pattern in &mut self.patterns {
            if let Pattern::PianoRoll(pr) = pattern {
                for n in &mut pr.notes {
                    n.pitch = (n.pitch as i32 + semitones).clamp(0, 127) as u8;
                }
            }
        }
        const KEYS: [&str; 12] = ["C","C#","D","D#","E","F","F#","G","G#","A","A#","B"];
        let idx = KEYS.iter().position(|&k| k == self.project.key.as_str()).unwrap_or(0);
        self.project.key = KEYS[((idx as i32 + semitones).rem_euclid(12)) as usize].to_string();
    }

    /// Cycle humanize 0 → 25 → 50 → 75 → 100 → 0 %.
    pub fn cycle_humanize(&mut self) {
        let next = ((self.project.humanize * 4.0).round() as i32 + 1) % 5;
        self.project.humanize = next as f32 / 4.0;
    }

    pub fn set_swing(&mut self, swing: f32) { self.project.swing = swing.clamp(0.0, 1.0); }
    pub fn set_timing(&mut self, timing: f32) { self.project.timing = timing.clamp(0.0, 1.0); }

    /// Toggle legato on a piano-roll pattern. `false` for unknown/step ids.
    pub fn set_legato(&mut self, pid: &str, on: bool) -> bool {
        for p in &mut self.patterns {
            if let Pattern::PianoRoll(pr) = p {
                if pr.id == pid { pr.legato = on; return true; }
            }
        }
        false
    }
    pub fn set_metronome(&mut self, on: bool) { self.project.metronome = on; }

    fn pattern_bars(&self, pid: &str) -> u32 {
        self.patterns.iter().find(|p| p.id() == pid).map(|p| p.bars()).unwrap_or(1)
    }

    /// Effective clip length in bars at `(track, bar)`: explicit `len_bars` or the
    /// pattern's natural bar count. `None` if there's no clip there.
    pub fn clip_len_bars(&self, track_id: &str, bar: u32) -> Option<u32> {
        let s = self.arrangement.iter().find(|s| s.track == track_id && s.bar == bar)?;
        Some(s.len_bars.unwrap_or_else(|| self.pattern_bars(&s.pattern)))
    }

    /// Set a clip's length in bars (min 1). False if no clip at `(track, bar)`.
    pub fn resize_clip(&mut self, track_id: &str, bar: u32, new_len_bars: u32) -> bool {
        let n = new_len_bars.max(1);
        match self.arrangement.iter_mut().find(|s| s.track == track_id && s.bar == bar) {
            Some(s) => { s.len_bars = Some(n); true }
            None => false,
        }
    }

    /// Per-bar effective span (bars) for a track's clips — used by overlap-clearing.
    fn track_clip_spans(&self, track_id: &str) -> std::collections::HashMap<u32, u32> {
        self.arrangement.iter()
            .filter(|s| s.track == track_id)
            .map(|s| (s.bar, s.len_bars.unwrap_or_else(|| self.pattern_bars(&s.pattern))))
            .collect()
    }

    /// Append a full-length audio clip on `track` at `start_sec` (clamped ≥0).
    /// `src_len_sec` is the source WAV's duration. Returns the new clip's index.
    pub fn add_audio_clip(&mut self, track: &str, path: &str, start_sec: f64, src_len_sec: f64) -> usize {
        self.audio_clips.push(AudioClip {
            track: track.to_string(),
            path: path.to_string(),
            start_sec: start_sec.max(0.0),
            offset_sec: 0.0,
            len_sec: src_len_sec.max(MIN_CLIP_SEC),
            gain: 1.0,
        });
        self.audio_clips.len() - 1
    }

    /// Reposition a clip on the timeline (start clamped ≥0).
    pub fn move_audio_clip(&mut self, idx: usize, new_start_sec: f64) {
        if let Some(c) = self.audio_clips.get_mut(idx) {
            c.start_sec = new_start_sec.max(0.0);
        }
    }

    /// Drag the left edge by `delta_sec` (positive = trim from the front):
    /// offset, len, and start move together so the audio under the cut stays put.
    /// Clamps so offset ≥ 0 and len ≥ MIN_CLIP_SEC.
    pub fn crop_audio_clip_start(&mut self, idx: usize, delta_sec: f64) {
        if let Some(c) = self.audio_clips.get_mut(idx) {
            let lo = -c.offset_sec;              // most we can move left
            let hi = c.len_sec - MIN_CLIP_SEC;   // most we can move right
            let d = delta_sec.clamp(lo, hi);
            c.offset_sec += d;
            c.start_sec  = (c.start_sec + d).max(0.0);
            c.len_sec    = (c.len_sec - d).max(MIN_CLIP_SEC);
        }
    }

    /// Set the clip's cropped length (clamped ≥ MIN_CLIP_SEC).
    pub fn crop_audio_clip_end(&mut self, idx: usize, new_len_sec: f64) {
        if let Some(c) = self.audio_clips.get_mut(idx) {
            c.len_sec = new_len_sec.max(MIN_CLIP_SEC);
        }
    }

    /// Set a clip's linear gain (clamped 0..=4).
    pub fn set_audio_clip_gain(&mut self, idx: usize, gain: f32) {
        if let Some(c) = self.audio_clips.get_mut(idx) {
            c.gain = gain.clamp(0.0, 4.0);
        }
    }

    /// Remove a clip by index (no-op if out of range).
    pub fn remove_audio_clip(&mut self, idx: usize) {
        if idx < self.audio_clips.len() {
            self.audio_clips.remove(idx);
        }
    }

    /// Place/cycle a pattern for `track_id` at `bar`. False if the track has no patterns.
    pub fn place_pattern(&mut self, track_id: &str, bar: u32) -> bool {
        let track_pats = match self.tracks.iter().find(|t| t.id == track_id) {
            Some(t) if !t.patterns.is_empty() => t.patterns.clone(),
            _ => return false,
        };
        let existing = self.arrangement.iter()
            .find(|s| s.track == track_id && s.bar == bar).map(|s| s.pattern.clone());
        let spans = self.track_clip_spans(track_id);
        self.arrangement.retain(|s| {
            if s.track != track_id { return true; }
            let span = spans.get(&s.bar).copied().unwrap_or(1);
            !(s.bar <= bar && bar < s.bar + span)
        });
        let next = match existing {
            Some(cur) => {
                let i = track_pats.iter().position(|p| p == &cur).unwrap_or(0);
                let ni = (i + 1) % track_pats.len();
                if ni == 0 && track_pats.len() == 1 { return true; }
                track_pats[ni].clone()
            }
            None => track_pats[0].clone(),
        };
        self.arrangement.push(ArrangementSlot { track: track_id.to_string(), pattern: next, bar, offset_ticks: 0, len_bars: None });
        true
    }

    /// Place a specific pattern (`pattern_id`) at `bar` on `track_id`, clearing
    /// any clips it overlaps. Returns `false` if the track is missing or the
    /// pattern doesn't belong to it.
    pub fn place_pattern_id(&mut self, track_id: &str, bar: u32, pattern_id: &str) -> bool {
        let belongs = self.tracks.iter().find(|t| t.id == track_id)
            .is_some_and(|t| t.patterns.iter().any(|p| p == pattern_id));
        if !belongs {
            return false;
        }
        let spans = self.track_clip_spans(track_id);
        self.arrangement.retain(|s| {
            if s.track != track_id {
                return true;
            }
            let span = spans.get(&s.bar).copied().unwrap_or(1);
            !(s.bar <= bar && bar < s.bar + span)
        });
        self.arrangement.push(ArrangementSlot {
            track: track_id.to_string(), pattern: pattern_id.to_string(), bar, offset_ticks: 0, len_bars: None,
        });
        true
    }

    /// Advance the clip starting at `bar` on `track_id` to the track's next
    /// pattern (wrapping), preserving its length. Returns `false` if no clip
    /// starts at `bar` or the track has no patterns.
    pub fn cycle_clip_pattern(&mut self, track_id: &str, bar: u32) -> bool {
        let pats = match self.tracks.iter().find(|t| t.id == track_id) {
            Some(t) if !t.patterns.is_empty() => t.patterns.clone(),
            _ => return false,
        };
        let Some(slot) = self.arrangement.iter_mut()
            .find(|s| s.track == track_id && s.bar == bar) else { return false };
        let i = pats.iter().position(|p| p == &slot.pattern).unwrap_or(0);
        slot.pattern = pats[(i + 1) % pats.len()].clone();
        true
    }

    /// Remove the clip covering `bar` on `track_id`.
    pub fn remove_clip(&mut self, track_id: &str, bar: u32) -> bool {
        let spans = self.track_clip_spans(track_id);
        let before = self.arrangement.len();
        self.arrangement.retain(|s| {
            if s.track != track_id { return true; }
            let span = spans.get(&s.bar).copied().unwrap_or(1);
            !(s.bar <= bar && bar < s.bar + span)
        });
        self.arrangement.len() != before
    }

    /// Move the clip at `(from_track, from_bar)` to `(to_track, to_bar)` with a
    /// sub-bar `to_offset_ticks`, keeping its pattern and explicit length. Clears
    /// any clips overlapping the destination first. No-op (false) if there's no
    /// clip at the source or the destination equals the source position.
    pub fn move_clip(&mut self, from_track: &str, from_bar: u32, to_track: &str, to_bar: u32, to_offset_ticks: u32) -> bool {
        let (to_bar, to_offset) = normalize_bar_offset(to_bar, to_offset_ticks);
        if from_track == to_track && from_bar == to_bar && to_offset == 0 {
            // Same bar, on the grid: nothing to do (offset 0 == old behavior).
            if self.arrangement.iter().any(|s| s.track == from_track && s.bar == from_bar && s.offset_ticks == 0) {
                return false;
            }
        }
        let Some((pattern, len_bars)) = self.arrangement.iter()
            .find(|s| s.track == from_track && s.bar == from_bar)
            .map(|s| (s.pattern.clone(), s.len_bars)) else { return false };
        self.arrangement.retain(|s| !(s.track == from_track && s.bar == from_bar));
        let spans = self.track_clip_spans(to_track);
        self.arrangement.retain(|s| {
            if s.track != to_track { return true; }
            let span = spans.get(&s.bar).copied().unwrap_or(1);
            !(s.bar <= to_bar && to_bar < s.bar + span)
        });
        self.arrangement.push(ArrangementSlot { track: to_track.to_string(), pattern, bar: to_bar, offset_ticks: to_offset, len_bars });
        true
    }

    /// Set the sub-bar start offset (ticks) of the clip at `(track, bar)`. The
    /// offset is normalized into `[0, TICKS_PER_BAR)`; any whole bars fold into
    /// the slot's `bar`. False if there's no clip there.
    pub fn set_clip_offset(&mut self, track_id: &str, bar: u32, offset_ticks: u32) -> bool {
        let (new_bar, new_off) = normalize_bar_offset(bar, offset_ticks);
        match self.arrangement.iter_mut().find(|s| s.track == track_id && s.bar == bar) {
            Some(s) => { s.bar = new_bar; s.offset_ticks = new_off; true }
            None => false,
        }
    }

    fn next_track_id(&self) -> String {
        let n = self.tracks.iter()
            .filter_map(|t| t.id.trim_start_matches('t').parse::<usize>().ok())
            .max().unwrap_or(0) + 1;
        format!("t{n}")
    }
    fn next_pattern_id(&self) -> String {
        let n = self.patterns.iter()
            .filter_map(|p| p.id().trim_start_matches('p').parse::<usize>().ok())
            .max().unwrap_or(0) + 1;
        format!("p{n}")
    }

    /// Add a track of `track_type` (with a default attached pattern for Drum/Melodic). Returns new id.
    pub fn add_track(&mut self, track_type: TrackType) -> String {
        let tid = self.next_track_id();
        let pid = self.next_pattern_id();
        let (instrument, patterns) = match track_type {
            TrackType::Audio => (Instrument::AudioFile { path: String::new() }, vec![]),
            TrackType::Drum => {
                self.patterns.push(Pattern::Step(StepPattern {
                    id: pid.clone(), name: format!("Pattern {}", &pid[1..]), bars: 1, steps: 16,
                    rows: vec![
                        StepRow { name: "Kick".into(),  pitch: 36, vel: 85, steps: vec![0; 16] },
                        StepRow { name: "Snare".into(), pitch: 38, vel: 70, steps: vec![0; 16] },
                        StepRow { name: "HH".into(),    pitch: 42, vel: 52, steps: vec![0; 16] },
                    ],
                    gate: 0.5,
                }));
                (Instrument::Soundfont { preset: 0, bank: 128, soundfont: None }, vec![pid.clone()])
            }
            TrackType::Melodic => {
                self.patterns.push(Pattern::PianoRoll(PianoRollPattern {
                    id: pid.clone(), name: format!("Pattern {}", &pid[1..]), bars: 4,
                    ticks_per_bar: 480, notes: vec![], legato: false,
                }));
                (Instrument::Soundfont { preset: 0, bank: 0, soundfont: None }, vec![pid.clone()])
            }
        };
        let label = match track_type { TrackType::Drum => "Drums", TrackType::Melodic => "Synth", TrackType::Audio => "Audio" };
        self.tracks.push(Track {
            id: tid.clone(), name: format!("{label} {}", &tid[1..]), track_type,
            instrument, volume: 0.8, pan: 0.0, muted: false, patterns,
            effects: EffectChain::default(), modulators: Vec::new(),
            clap_fx: Vec::new(), tuning_cents: 0.0,
        });
        tid
    }

    /// Remove a track by id, dropping its arrangement slots and patterns owned only by it.
    pub fn remove_track(&mut self, track_id: &str) -> bool {
        let Some(pos) = self.tracks.iter().position(|t| t.id == track_id) else { return false };
        let removed = self.tracks.remove(pos);
        self.arrangement.retain(|s| s.track != removed.id);
        self.patterns.retain(|pat| {
            let owned = removed.patterns.iter().any(|pid| pid == pat.id());
            let used = self.tracks.iter().any(|t| t.patterns.iter().any(|pid| pid == pat.id()));
            !owned || used
        });
        true
    }

    /// Cycle a track Drum↔Melodic (Audio unchanged), swapping the soundfont bank.
    pub fn cycle_track_type(&mut self, track_id: &str) {
        if let Some(t) = self.tracks.iter_mut().find(|t| t.id == track_id) {
            match t.track_type {
                TrackType::Drum => { t.track_type = TrackType::Melodic;
                    t.instrument = Instrument::Soundfont { preset: 0, bank: 0, soundfont: None }; }
                TrackType::Melodic => { t.track_type = TrackType::Drum;
                    t.instrument = Instrument::Soundfont { preset: 0, bank: 128, soundfont: None }; }
                TrackType::Audio => {}
            }
        }
    }

    /// Add a new blank pattern matching the track's type (Step for Drum, PianoRoll
    /// for Melodic), attach it, return its id. None for unknown/Audio tracks.
    pub fn add_pattern(&mut self, track_id: &str) -> Option<String> {
        let ttype = self.tracks.iter().find(|t| t.id == track_id)?.track_type.clone();
        let pid = self.next_pattern_id();
        match ttype {
            TrackType::Drum => self.patterns.push(Pattern::Step(StepPattern {
                id: pid.clone(), name: format!("Pattern {}", &pid[1..]), bars: 1, steps: 16,
                rows: vec![
                    StepRow { name: "Kick".into(),  pitch: 36, vel: 85, steps: vec![0; 16] },
                    StepRow { name: "Snare".into(), pitch: 38, vel: 70, steps: vec![0; 16] },
                    StepRow { name: "HH".into(),    pitch: 42, vel: 52, steps: vec![0; 16] },
                ],
                gate: 0.5,
            })),
            TrackType::Melodic => self.patterns.push(Pattern::PianoRoll(PianoRollPattern {
                id: pid.clone(), name: format!("Pattern {}", &pid[1..]), bars: 4, ticks_per_bar: 480, notes: vec![], legato: false,
            })),
            TrackType::Audio => return None,
        }
        self.tracks.iter_mut().find(|t| t.id == track_id)?.patterns.push(pid.clone());
        Some(pid)
    }

    /// Compile mini-notation `src` into the track's first pattern (replacing it),
    /// or a new `live_<track>` pattern if the track has none. Returns the note
    /// count on success, or a human error string. Mirrors the TUI REPL.
    pub fn apply_mini_notation(&mut self, track_id: &str, src: &str) -> Result<usize, String> {
        if src.trim().is_empty() {
            return Err("empty — type some mini-notation".into());
        }
        let track = self.tracks.iter().find(|t| t.id == track_id)
            .ok_or_else(|| format!("no track {track_id}"))?;
        let (pid, bars) = match track.patterns.first().cloned() {
            Some(pid) => {
                let bars = self.patterns.iter().find(|p| p.id() == pid)
                    .map(|p| p.bars() as u8).unwrap_or(1).max(1);
                (pid, bars)
            }
            None => (format!("live_{track_id}"), 1),
        };
        let pat = crate::mini_notation::compile(src, &pid, "live", bars, 480, 100)?;
        let n = pat.notes.len();
        let pattern = Pattern::PianoRoll(pat);
        if let Some(slot) = self.patterns.iter_mut().find(|p| p.id() == pid) {
            *slot = pattern;
        } else {
            self.patterns.push(pattern);
            if let Some(t) = self.tracks.iter_mut().find(|t| t.id == track_id) {
                if !t.patterns.iter().any(|x| x == &pid) {
                    t.patterns.push(pid);
                }
            }
        }
        Ok(n)
    }

    fn step_pattern_mut(&mut self, pid: &str) -> Option<&mut StepPattern> {
        self.patterns.iter_mut().find_map(|p| match p {
            Pattern::Step(sp) if sp.id == pid => Some(sp),
            _ => None,
        })
    }

    /// Grow/shrink a step pattern's step count by `delta` steps, clamped 16..256.
    pub fn resize_steps(&mut self, pid: &str, delta: i32) -> bool {
        let Some(sp) = self.step_pattern_mut(pid) else { return false };
        let new = (sp.steps as i32 + delta).clamp(4, 256) as u16;
        if new == sp.steps { return false; }
        sp.steps = new;
        for r in &mut sp.rows { r.steps.resize(new as usize, 0); }
        true
    }

    pub fn add_step_row(&mut self, pid: &str) -> bool {
        let Some(sp) = self.step_pattern_mut(pid) else { return false };
        let n = sp.steps as usize;
        let i = sp.rows.len() + 1;
        sp.rows.push(StepRow { name: format!("Row {i}"), pitch: 36, vel: 80, steps: vec![0; n] });
        true
    }

    pub fn nudge_row_pitch(&mut self, pid: &str, row: usize, delta: i32) -> bool {
        let Some(sp) = self.step_pattern_mut(pid) else { return false };
        let Some(r) = sp.rows.get_mut(row) else { return false };
        r.pitch = (r.pitch as i32 + delta).clamp(0, 127) as u8;
        true
    }

    pub fn set_gate(&mut self, pid: &str, gate: f32) -> bool {
        let Some(sp) = self.step_pattern_mut(pid) else { return false };
        sp.gate = gate.clamp(0.1, 2.0);
        true
    }

    /// Nudge a step cell's velocity by `delta`. Off + positive delta turns it on at
    /// the row default; crossing ≤0 turns it off; otherwise clamps 1..127.
    pub fn nudge_step_velocity(&mut self, pid: &str, row: usize, step: usize, delta: i32) -> bool {
        let Some(sp) = self.step_pattern_mut(pid) else { return false };
        let Some(r) = sp.rows.get_mut(row) else { return false };
        let default_vel = r.vel as i32;
        let Some(cell) = r.steps.get_mut(step) else { return false };
        let cur = match *cell { 0 => 0i32, 1 => default_vel, v => v as i32 };
        let nv = if cur == 0 && delta > 0 { default_vel } else { cur + delta };
        *cell = if nv <= 0 { 0 } else { nv.clamp(1, 127) as u8 };
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_velocity_semantics() {
        let row = StepRow {
            name: "K".into(), pitch: 36, vel: 90,
            steps: vec![0, 1, 100, 5],
        };
        assert_eq!(row.step_velocity(0), None);        // off
        assert_eq!(row.step_velocity(1), Some(90));    // 1 -> row default
        assert_eq!(row.step_velocity(2), Some(100));   // explicit
        assert_eq!(row.step_velocity(3), Some(5));     // explicit (ghost)
        assert_eq!(row.step_velocity(99), None);       // out of range
    }

    #[test]
    fn clip_offset_start_tick_and_normalize() {
        // start_tick = bar*480 + offset.
        let s = ArrangementSlot { track: "t1".into(), pattern: "p1".into(), bar: 2, offset_ticks: 240, len_bars: None };
        assert_eq!(s.start_tick(), 2 * 480 + 240);
        // A zero offset behaves exactly like the old whole-bar placement.
        let s0 = ArrangementSlot { track: "t1".into(), pattern: "p1".into(), bar: 3, offset_ticks: 0, len_bars: None };
        assert_eq!(s0.start_tick(), 3 * 480);
        // Whole bars fold out of the offset.
        assert_eq!(normalize_bar_offset(1, 600), (2, 120)); // 600 = one bar + 120
        assert_eq!(normalize_bar_offset(0, 480), (1, 0));
        assert_eq!(normalize_bar_offset(4, 240), (4, 240)); // already in range
    }

    #[test]
    fn set_clip_offset_folds_whole_bars() {
        let mut p = Project::default_project();
        // default_project places clips at bar 0 on t1/t2.
        assert!(p.set_clip_offset("t1", 0, 240));
        let s = p.arrangement.iter().find(|s| s.track == "t1").unwrap();
        assert_eq!((s.bar, s.offset_ticks), (0, 240));
        // An offset past a bar moves the slot's bar and keeps offset < 480.
        assert!(p.set_clip_offset("t1", 0, 600));
        let s = p.arrangement.iter().find(|s| s.track == "t1").unwrap();
        assert_eq!((s.bar, s.offset_ticks), (1, 120));
        // No clip at that (track, bar) → false.
        assert!(!p.set_clip_offset("t1", 9, 60));
    }

    #[test]
    fn move_clip_carries_offset() {
        let mut p = Project::default_project();
        assert!(p.move_clip("t1", 0, "t1", 4, 120));
        let s = p.arrangement.iter().find(|s| s.track == "t1").unwrap();
        assert_eq!((s.bar, s.offset_ticks), (4, 120));
        assert_eq!(s.start_tick(), 4 * 480 + 120);
    }

    #[test]
    fn resolve_soundfonts_routing() {
        fn sf_track(id: &str, sf: Option<&str>) -> Track {
            Track {
                id: id.into(), name: id.into(), track_type: TrackType::Melodic,
                instrument: Instrument::Soundfont { preset: 0, bank: 0, soundfont: sf.map(String::from) },
                volume: 1.0, pan: 0.0, muted: false, patterns: vec![],
                effects: EffectChain::default(), modulators: vec![],
                clap_fx: vec![], tuning_cents: 0.0,
            }
        }
        let tracks = vec![
            sf_track("t1", None),                 // -> main (0)
            sf_track("t2", Some("synths.sf2")),   // -> 1
            sf_track("t3", Some("synths.sf2")),   // -> 1 (deduped)
            sf_track("t4", Some("drums.sf2")),    // -> 2
            sf_track("t5", Some("main.sf2")),     // equals default -> 0
        ];
        let (paths, idx) = resolve_soundfonts(&tracks, "main.sf2");
        assert_eq!(paths, vec!["main.sf2", "synths.sf2", "drums.sf2"]);
        assert_eq!(idx, vec![0, 1, 1, 2, 0]);
    }

    #[test]
    fn track_setters_mutate_by_id() {
        let mut p = Project::default_project();
        assert!(p.set_track_volume("t2", 0.5));
        assert!(p.set_track_pan("t2", -0.25));
        assert!(p.set_track_muted("t1", true));
        let t1 = p.tracks.iter().find(|t| t.id == "t1").unwrap();
        let t2 = p.tracks.iter().find(|t| t.id == "t2").unwrap();
        assert_eq!(t2.volume, 0.5);
        assert_eq!(t2.pan, -0.25);
        assert!(t1.muted);
        // unknown id is a no-op and reports false
        assert!(!p.set_track_volume("nope", 1.0));
    }

    #[test]
    fn song_bars_covers_audio_clips() {
        let mut p = Project::default_project(); // bpm 120, 4/4 -> bar = 2.0s
        // A clip ending at 20s -> 10 bars; min floor is 8, arrangement empty.
        p.audio_clips.push(AudioClip {
            track: "a".into(), path: "x.wav".into(),
            start_sec: 12.0, offset_sec: 0.0, len_sec: 8.0, gain: 1.0,
        });
        assert_eq!(p.song_bars(), 10);
    }

    #[test]
    fn song_bars_spans_arrangement() {
        let p = Project::default_project();
        // bass pattern p2 is 2 bars placed at bar 0 → 2 bars; floored to a minimum of 8.
        assert_eq!(p.song_bars(), 8);
        // a slot placed beyond bar 8 raises the result above the floor
        let mut p2 = Project::default_project();
        p2.arrangement[0] = ArrangementSlot { track: "t1".into(), pattern: "p1".into(), bar: 10, offset_ticks: 0, len_bars: None };
        assert_eq!(p2.song_bars(), 11); // 10 + 1-bar drum pattern p1
        // resizing that clip to 5 bars extends the timeline to cover it
        p2.resize_clip("t1", 10, 5);
        assert_eq!(p2.song_bars(), 15); // 10 + 5
    }

    #[test]
    fn toggle_step_flips_cell_and_reports_state() {
        let mut p = Project::default_project();
        // p1 is the drum step pattern; row 0 ("Kick") step 0 starts on (1).
        assert_eq!(p.toggle_step("p1", 0, 0), Some(false)); // was on → now off
        assert_eq!(p.toggle_step("p1", 0, 0), Some(true));  // back on
        // out-of-range row/step or unknown / wrong-type pattern → None
        assert_eq!(p.toggle_step("p1", 99, 0), None);
        assert_eq!(p.toggle_step("p1", 0, 999), None);
        assert_eq!(p.toggle_step("p2", 0, 0), None); // p2 is a piano-roll pattern
        assert_eq!(p.toggle_step("nope", 0, 0), None);
    }

    #[test]
    fn toggle_note_adds_then_removes() {
        let mut p = Project::default_project();
        // p2 is the piano-roll pattern; (pitch 50, tick 240) has no note yet.
        assert_eq!(p.toggle_note("p2", 50, 240, 120), Some(true));  // added
        let added = matches!(&p.patterns[1], Pattern::PianoRoll(pr)
            if pr.notes.iter().any(|n| n.pitch == 50 && n.tick == 240 && n.duration == 120));
        assert!(added);
        assert_eq!(p.toggle_note("p2", 50, 240, 120), Some(false)); // removed
        // notes stay sorted by (tick, pitch) after an add
        assert_eq!(p.toggle_note("p2", 48, 0, 120), Some(true));
        let pr_pat = p.patterns.iter().find(|pat| pat.id() == "p2").unwrap();
        if let Pattern::PianoRoll(pr) = pr_pat {
            let mut sorted = pr.notes.clone();
            sorted.sort_by_key(|n| (n.tick, n.pitch));
            assert_eq!(pr.notes, sorted);
        }
        // wrong-type / unknown pattern → None
        assert_eq!(p.toggle_note("p1", 60, 0, 120), None); // p1 is a step pattern
        assert_eq!(p.toggle_note("nope", 60, 0, 120), None);
    }

    #[test]
    fn nudge_effect_clamps_per_param() {
        let mut p = Project::default_project();
        assert!(p.nudge_effect("t1", EffectParam::Reverb, -0.5));
        assert_eq!(effects_of(&p, "t1").reverb, 0.0);
        assert!(p.nudge_effect("t1", EffectParam::Reverb, 5.0));
        assert_eq!(effects_of(&p, "t1").reverb, 1.0);
        assert!(p.nudge_effect("t2", EffectParam::EqLowGain, -100.0));
        assert_eq!(effects_of(&p, "t2").eq_low_gain, -12.0);
        assert!(p.nudge_effect("t2", EffectParam::DelayTimeMs, 50.0));
        assert_eq!(effects_of(&p, "t2").delay_time_ms, 50);
        assert!(!p.nudge_effect("nope", EffectParam::Reverb, 0.1));
    }

    #[test]
    fn set_track_preset_changes_soundfont_only() {
        let mut p = Project::default_project();
        assert!(p.set_track_preset("t2", 40));
        if let Instrument::Soundfont { preset, bank, .. } = &p.tracks[1].instrument {
            assert_eq!(*preset, 40);
            assert_eq!(*bank, 0);
        } else {
            panic!("t2 should still be a soundfont");
        }
        assert!(!p.set_track_preset("nope", 1));
    }

    #[test]
    fn set_sampler_pad_adds_then_updates() {
        let mut p = Project::default_project();
        p.tracks[0].instrument = Instrument::Sampler { pads: vec![] };

        assert!(p.set_sampler_pad("t1", 36, "k.wav".into()));
        assert!(p.set_sampler_pad("t1", 36, "kick2.wav".into())); // update in place
        assert!(p.set_sampler_pad("t1", 38, "snare.wav".into())); // new pad

        let Instrument::Sampler { pads } = &p.tracks[0].instrument else {
            panic!("t1 should be a sampler");
        };
        assert_eq!(pads.len(), 2, "same pitch updates, not duplicates");
        let kick = pads.iter().find(|pad| pad.pitch == 36).unwrap();
        assert_eq!(kick.path, "kick2.wav");
        assert!((kick.gain - 1.0).abs() < 1e-6);
        assert!(pads.iter().any(|pad| pad.pitch == 38 && pad.path == "snare.wav"));

        // Non-sampler track and missing track both fail.
        assert!(!p.set_sampler_pad("t2", 36, "x.wav".into()));
        assert!(!p.set_sampler_pad("nope", 36, "x.wav".into()));
    }

    #[test]
    fn place_pattern_id_places_specific_and_validates() {
        let mut p = Project::default_project();
        p.tracks[0].patterns.push("p1b".into()); // t1: [p1, p1b]

        assert!(p.place_pattern_id("t1", 4, "p1b"));
        let slot = p.arrangement.iter().find(|s| s.track == "t1" && s.bar == 4).unwrap();
        assert_eq!(slot.pattern, "p1b");

        // Pattern not on this track, and missing track → false.
        assert!(!p.place_pattern_id("t1", 8, "p2")); // p2 belongs to t2
        assert!(!p.place_pattern_id("nope", 0, "p1"));

        // Placing again at the same bar replaces (clears overlap), not duplicates.
        assert!(p.place_pattern_id("t1", 4, "p1"));
        let at4: Vec<_> = p.arrangement.iter().filter(|s| s.track == "t1" && s.bar == 4).collect();
        assert_eq!(at4.len(), 1);
        assert_eq!(at4[0].pattern, "p1");
    }

    #[test]
    fn cycle_clip_pattern_advances_in_place_and_keeps_len() {
        let mut p = Project::default_project();
        p.tracks[0].patterns.push("p1b".into()); // t1: [p1, p1b]; default has p1 @ bar 0
        p.arrangement.iter_mut().find(|s| s.track == "t1" && s.bar == 0).unwrap().len_bars = Some(3);

        assert!(p.cycle_clip_pattern("t1", 0));
        let s = p.arrangement.iter().find(|s| s.track == "t1" && s.bar == 0).unwrap();
        assert_eq!(s.pattern, "p1b");
        assert_eq!(s.len_bars, Some(3), "cycling preserves clip length");

        assert!(p.cycle_clip_pattern("t1", 0)); // wraps back to p1
        assert_eq!(p.arrangement.iter().find(|s| s.track == "t1" && s.bar == 0).unwrap().pattern, "p1");

        assert!(!p.cycle_clip_pattern("t1", 99)); // no clip there
    }

    fn effects_of<'a>(p: &'a Project, id: &str) -> &'a EffectChain {
        &p.tracks.iter().find(|t| t.id == id).unwrap().effects
    }

    #[test]
    fn apply_mini_notation_installs_pattern() {
        let mut p = Project::default_project();
        let n = p.apply_mini_notation("t2", "c4 e4 g4").expect("should compile");
        assert_eq!(n, 3);
        let pat = p.patterns.iter().find(|p| p.id() == "p2").unwrap();
        match pat {
            Pattern::PianoRoll(pr) => assert_eq!(pr.notes.len(), 3),
            _ => panic!("p2 should be piano-roll after REPL eval"),
        }
        assert!(p.apply_mini_notation("t2", "   ").is_err());
        assert!(p.apply_mini_notation("nope", "c4").is_err());
    }

    #[test]
    fn transpose_shifts_notes_and_key() {
        let mut p = Project::default_project();
        p.transpose(2);
        let pr = match p.patterns.iter().find(|p| p.id() == "p2").unwrap() {
            Pattern::PianoRoll(pr) => pr, _ => panic!(),
        };
        assert_eq!(pr.notes[0].pitch, 38);
        assert_eq!(p.project.key, "D");
        p.transpose(-2);
        assert_eq!(p.project.key, "C");
    }

    #[test]
    fn cycle_humanize_steps_quarters() {
        let mut p = Project::default_project();
        p.project.humanize = 0.0;
        p.cycle_humanize();
        assert!((p.project.humanize - 0.25).abs() < 1e-6);
    }

    #[test]
    fn place_and_remove_clip() {
        let mut p = Project::default_project();
        let before = p.arrangement.len();
        assert!(p.place_pattern("t1", 4));
        assert!(p.arrangement.iter().any(|s| s.track == "t1" && s.bar == 4 && s.pattern == "p1"));
        assert_eq!(p.arrangement.len(), before + 1);
        assert!(p.remove_clip("t1", 4));
        assert!(!p.arrangement.iter().any(|s| s.track == "t1" && s.bar == 4));
    }

    #[test]
    fn move_clip_repositions_and_changes_lane() {
        let mut p = Project::default_project();
        // default: t1/p1 at bar 0. Move within the lane to bar 3.
        assert!(p.move_clip("t1", 0, "t1", 3, 0));
        assert!(!p.arrangement.iter().any(|s| s.track == "t1" && s.bar == 0));
        assert!(p.arrangement.iter().any(|s| s.track == "t1" && s.bar == 3 && s.pattern == "p1"));
        // Move across lanes to t2 @ bar 3 (keeps the pattern, clears t2's overlap there).
        assert!(p.move_clip("t1", 3, "t2", 3, 0));
        assert!(p.arrangement.iter().any(|s| s.track == "t2" && s.bar == 3 && s.pattern == "p1"));
        // No clip at the source, and source==dest on the grid, are no-ops.
        assert!(!p.move_clip("t1", 99, "t1", 1, 0));
        assert!(!p.move_clip("t2", 3, "t2", 3, 0));
    }

    #[test]
    fn resize_clip_sets_and_clamps_length() {
        let mut p = Project::default_project();
        assert!(p.resize_clip("t1", 0, 4));
        assert_eq!(p.clip_len_bars("t1", 0), Some(4));
        assert!(p.resize_clip("t1", 0, 0)); // clamps to >= 1
        assert_eq!(p.clip_len_bars("t1", 0), Some(1));
        assert!(!p.resize_clip("t1", 99, 2));
    }

    #[test]
    fn clip_len_bars_falls_back_to_pattern() {
        let p = Project::default_project();
        assert_eq!(p.clip_len_bars("t1", 0), Some(1)); // p1 is a 1-bar pattern
    }

    #[test]
    fn move_clip_preserves_length() {
        let mut p = Project::default_project();
        p.resize_clip("t1", 0, 3);
        assert!(p.move_clip("t1", 0, "t1", 4, 0));
        assert_eq!(p.clip_len_bars("t1", 4), Some(3));
    }

    #[test]
    fn place_overlap_respects_effective_length() {
        let mut p = Project::default_project();
        p.resize_clip("t1", 0, 4); // clip covers bars 0..4
        assert!(p.place_pattern("t1", 2)); // placing inside it clears the overlap
        assert!(!p.arrangement.iter().any(|s| s.track == "t1" && s.bar == 0));
        assert!(p.arrangement.iter().any(|s| s.track == "t1" && s.bar == 2));
    }

    #[test]
    fn add_and_remove_track() {
        let mut p = Project::default_project();
        let n = p.tracks.len();
        let id = p.add_track(TrackType::Melodic);
        assert_eq!(p.tracks.len(), n + 1);
        assert_eq!(p.tracks.iter().find(|t| t.id == id).unwrap().patterns.len(), 1);
        assert!(p.remove_track(&id));
        assert_eq!(p.tracks.len(), n);
    }

    #[test]
    fn cycle_track_type_swaps_drum_melodic() {
        let mut p = Project::default_project();
        p.cycle_track_type("t2");
        assert_eq!(p.tracks.iter().find(|t| t.id == "t2").unwrap().track_type, TrackType::Drum);
    }

    #[test]
    fn add_pattern_matches_track_type() {
        let mut p = Project::default_project();
        let pid_drum = p.add_pattern("t1").unwrap();
        assert!(matches!(p.patterns.iter().find(|x| x.id()==pid_drum).unwrap(), Pattern::Step(_)));
        let pid_mel = p.add_pattern("t2").unwrap();
        assert!(matches!(p.patterns.iter().find(|x| x.id()==pid_mel).unwrap(), Pattern::PianoRoll(_)));
        assert!(p.tracks.iter().find(|t|t.id=="t1").unwrap().patterns.contains(&pid_drum));
    }

    #[test]
    fn resize_and_edit_steps() {
        let mut p = Project::default_project();
        assert!(p.resize_steps("p1", 16)); // 16 → 32
        if let Pattern::Step(sp) = p.patterns.iter().find(|x| x.id() == "p1").unwrap() {
            assert_eq!(sp.steps, 32);
            assert_eq!(sp.rows[0].steps.len(), 32);
        } else { panic!() }
        assert!(p.resize_steps("p1", -16)); // 32 → 16
        assert!(p.add_step_row("p1"));
        let rows_now = if let Pattern::Step(sp) = p.patterns.iter().find(|x| x.id()=="p1").unwrap() { sp.rows.len() } else { 0 };
        assert_eq!(rows_now, 4);
        assert!(p.nudge_row_pitch("p1", 0, 1)); // kick 36 → 37
        assert!(p.set_gate("p1", 0.1));
        assert!(p.nudge_step_velocity("p1", 0, 1, 8)); // an off cell → on
        // wrong-type / unknown
        assert!(!p.resize_steps("p2", 16));   // p2 is piano-roll
        assert!(!p.add_step_row("nope"));
    }

    #[test]
    fn audio_clips_serde_roundtrip_and_backcompat() {
        // A project with no audio clips must not emit the field (byte back-compat).
        let mut p = Project::default_project();
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("audio_clips"), "empty audio_clips must be skipped");

        // With a clip, it round-trips and gain defaults to 1.0 when absent.
        p.audio_clips.push(AudioClip {
            track: "t1".into(), path: "kick.wav".into(),
            start_sec: 1.5, offset_sec: 0.0, len_sec: 2.0, gain: 1.0,
        });
        let json = serde_json::to_string(&p).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back.audio_clips.len(), 1);
        assert_eq!(back.audio_clips[0].start_sec, 1.5);

        let no_gain = r#"{"track":"t","path":"a.wav","start_sec":0.0,"offset_sec":0.0,"len_sec":1.0}"#;
        let c: AudioClip = serde_json::from_str(no_gain).unwrap();
        assert_eq!(c.gain, 1.0);
    }

    #[test]
    fn gate_default_reproduces_half_step() {
        let p = StepPattern {
            id: "p".into(), name: "p".into(), bars: 1, steps: 16,
            rows: vec![], gate: StepPattern::default_gate(),
        };
        // 16 steps over a 480-tick bar -> 30 ticks/step; legacy length was tps/2.
        let tps = 480 / 16;
        assert_eq!(p.step_note_ticks(tps), tps / 2);
        // legato gate overlaps into the next step; staccato is shorter; min 1.
        let mut legato = p.clone(); legato.gate = 1.5;
        assert_eq!(legato.step_note_ticks(tps), (tps as f32 * 1.5) as u32);
        let mut tight = p.clone(); tight.gate = 0.1;
        assert!(tight.step_note_ticks(tps) >= 1);
    }

    fn clip_project() -> Project {
        let mut p = Project::default_project();
        p.audio_clips.push(AudioClip {
            track: "audio1".into(), path: "loop.wav".into(),
            start_sec: 4.0, offset_sec: 0.0, len_sec: 8.0, gain: 1.0,
        });
        p
    }

    #[test]
    fn add_audio_clip_appends_full_length() {
        let mut p = Project::default_project();
        let i = p.add_audio_clip("audio1", "vox.wav", -3.0, 5.0);
        assert_eq!(i, 0);
        let c = &p.audio_clips[0];
        assert_eq!(c.start_sec, 0.0); // negative start clamped to 0
        assert_eq!(c.offset_sec, 0.0);
        assert_eq!(c.len_sec, 5.0);
        assert_eq!(c.gain, 1.0);
    }

    #[test]
    fn move_audio_clip_clamps_to_zero() {
        let mut p = clip_project();
        p.move_audio_clip(0, 10.0);
        assert_eq!(p.audio_clips[0].start_sec, 10.0);
        p.move_audio_clip(0, -2.0);
        assert_eq!(p.audio_clips[0].start_sec, 0.0);
    }

    #[test]
    fn crop_start_moves_offset_len_and_start_together() {
        let mut p = clip_project(); // start 4, offset 0, len 8
        p.crop_audio_clip_start(0, 2.0); // trim 2s off the front
        let c = &p.audio_clips[0];
        assert_eq!(c.start_sec, 6.0);
        assert_eq!(c.offset_sec, 2.0);
        assert_eq!(c.len_sec, 6.0);
    }

    #[test]
    fn crop_start_clamps_at_source_in_point_and_min_len() {
        let mut p = clip_project(); // offset 0 -> cannot go negative
        p.crop_audio_clip_start(0, -5.0);
        assert_eq!(p.audio_clips[0].offset_sec, 0.0);
        assert_eq!(p.audio_clips[0].start_sec, 4.0);
        assert_eq!(p.audio_clips[0].len_sec, 8.0);
        // Trimming more than the length leaves at least MIN_CLIP_SEC.
        p.crop_audio_clip_start(0, 100.0);
        assert!(p.audio_clips[0].len_sec >= MIN_CLIP_SEC);
    }

    #[test]
    fn crop_end_clamps_min() {
        let mut p = clip_project();
        p.crop_audio_clip_end(0, 3.0);
        assert_eq!(p.audio_clips[0].len_sec, 3.0);
        p.crop_audio_clip_end(0, 0.0);
        assert_eq!(p.audio_clips[0].len_sec, MIN_CLIP_SEC);
    }

    #[test]
    fn set_gain_and_remove() {
        let mut p = clip_project();
        p.set_audio_clip_gain(0, 9.0);
        assert_eq!(p.audio_clips[0].gain, 4.0); // clamped
        p.remove_audio_clip(0);
        assert!(p.audio_clips.is_empty());
        p.remove_audio_clip(5); // out of range: no panic
    }

    #[test]
    fn returns_schema_defaults_and_back_compat() {
        // Round-trip keeps defaults.
        let p = Project::default_project();
        let json = serde_json::to_string(&p).unwrap();
        let p2: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(p2.returns.reverb.gain, 1.0);
        assert_eq!(p2.returns.reverb.room_size, 0.5);
        assert_eq!(p2.returns.delay.time_ms, 300);
        assert_eq!(p2.tracks[0].effects.send_delay, 0.0);

        // A pre-slice-B file: strip the new field entirely.
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v.as_object_mut().unwrap().remove("returns");
        let p3: Project = serde_json::from_value(v).unwrap();
        assert!(!p3.returns.delay.muted);
        assert_eq!(p3.returns.delay.feedback, 0.35);
    }

    #[test]
    fn nudge_return_clamps_and_mutes_toggle() {
        let mut p = Project::default_project();
        p.nudge_return(ReturnParam::RevRoomSize, 10.0);
        assert_eq!(p.returns.reverb.room_size, 1.0);
        p.nudge_return(ReturnParam::DlyTimeMs, -5000.0);
        assert_eq!(p.returns.delay.time_ms, 1);
        p.nudge_return(ReturnParam::DlyGain, 10.0);
        assert_eq!(p.returns.delay.gain, 1.25);
        p.toggle_return_mute(0);
        assert!(p.returns.reverb.muted);
        p.toggle_return_mute(0);
        assert!(!p.returns.reverb.muted);
    }

    #[test]
    fn timing_and_legato_schema() {
        let mut p = Project::default_project();
        assert_eq!(p.project.timing, 0.0);
        p.set_timing(0.5);
        assert_eq!(p.project.timing, 0.5);
        p.set_timing(9.0);
        assert_eq!(p.project.timing, 1.0, "clamped");
        assert!(p.set_legato("p2", true));
        assert!(matches!(p.patterns.iter().find(|x| x.id()=="p2").unwrap(),
            Pattern::PianoRoll(pr) if pr.legato));
        assert!(!p.set_legato("p1", true), "step pattern refuses");
        let j = serde_json::to_string(&p).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&j).unwrap();
        v["project"].as_object_mut().unwrap().remove("timing");
        v["patterns"][1].as_object_mut().unwrap().remove("legato");
        let p2: Project = serde_json::from_value(v).unwrap();
        assert_eq!(p2.project.timing, 0.0);
        assert!(matches!(&p2.patterns[1], Pattern::PianoRoll(pr) if !pr.legato));
    }

    #[test]
    fn note_duration_and_pr_bars_ops() {
        let mut p = Project::default_project();
        // p2 is the default piano-roll with a note at pitch 36, tick 0.
        assert!(p.set_note_duration("p2", 36, 0, 960)); // 2 bars
        let (bars, dur) = match p.patterns.iter().find(|x| x.id() == "p2").unwrap() {
            Pattern::PianoRoll(pr) => (
                pr.bars,
                pr.notes.iter().find(|n| n.pitch == 36 && n.tick == 0).unwrap().duration,
            ),
            _ => panic!(),
        };
        assert_eq!(dur, 960);
        assert_eq!(bars, 2, "pattern auto-grew to fit the note");
        assert!(p.set_note_duration("p2", 36, 0, 5));
        assert!(matches!(p.patterns.iter().find(|x| x.id() == "p2").unwrap(),
            Pattern::PianoRoll(pr) if pr.notes.iter().any(|n| n.tick == 0 && n.duration == 30)));
        assert!(!p.set_note_duration("p2", 99, 0, 480), "unknown note");
        assert!(p.resize_pr_bars("p2", 3)); // 2 -> 5
        assert!(matches!(p.patterns.iter().find(|x| x.id() == "p2").unwrap(),
            Pattern::PianoRoll(pr) if pr.bars == 5));
        for _ in 0..10 {
            p.resize_pr_bars("p2", -2);
        }
        assert!(matches!(p.patterns.iter().find(|x| x.id() == "p2").unwrap(),
            Pattern::PianoRoll(pr) if pr.bars == 1), "clamped at 1");
        assert!(!p.resize_pr_bars("p1", 1), "step patterns refuse");
    }

    #[test]
    fn rename_ops() {
        let mut p = Project::default_project();
        assert!(p.set_track_name("t1", "Kicks"));
        assert_eq!(p.tracks[0].name, "Kicks");
        assert!(!p.set_track_name("nope", "x"));
        assert!(p.set_pattern_name("p1", "Main Beat"));
        assert!(matches!(p.patterns.iter().find(|x| x.id() == "p1").unwrap(),
            Pattern::Step(sp) if sp.name == "Main Beat"));
        assert!(p.set_row_name("p1", 0, "Kick"));
        assert!(!p.set_row_name("p1", 99, "x"));
    }

    #[test]
    fn resize_steps_by_four_min_four() {
        let mut p = Project::default_project();
        assert!(p.resize_steps("p1", 4)); // 16 -> 20
        assert!(p.resize_steps("p1", -4)); // back to 16
        for _ in 0..10 {
            p.resize_steps("p1", -4);
        }
        match p.patterns.iter().find(|x| x.id() == "p1").unwrap() {
            Pattern::Step(sp) => assert_eq!(sp.steps, 4, "clamped at 4"),
            _ => panic!(),
        }
    }

    #[test]
    fn loop_region_defaults_ops_and_back_compat() {
        let mut p = Project::default_project();
        assert!(!p.loop_region.enabled && p.loop_region.end_bar == 4);

        p.set_loop_region(2, 6);
        assert_eq!((p.loop_region.start_bar, p.loop_region.end_bar), (2, 6));
        p.set_loop_region(5, 5); // degenerate -> end clamped past start
        assert_eq!((p.loop_region.start_bar, p.loop_region.end_bar), (5, 6));
        p.toggle_loop();
        assert!(p.loop_region.enabled);
        p.toggle_punch_in();
        p.toggle_punch_out();
        assert!(p.loop_region.punch_in && p.loop_region.punch_out);

        // Old files (no loop_region) load with the default.
        let json = serde_json::to_string(&p).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v.as_object_mut().unwrap().remove("loop_region");
        let p2: Project = serde_json::from_value(v).unwrap();
        assert!(!p2.loop_region.enabled && p2.loop_region.start_bar == 0);
    }

    #[test]
    fn clap_schema_round_trip_and_back_compat() {
        let mut p = Project::default_project();
        let t0 = p.tracks[0].id.clone();
        let uid = p.add_clap_fx(&t0, "/tmp/gain.clap", "test.gain").unwrap();
        assert_eq!(uid, "c1");
        let uid2 = p.set_track_clap_instrument(&t0, "/tmp/tone.clap", "test.tone").unwrap();
        assert_eq!(uid2, "c2");
        p.set_clap_param(&uid, 1, 0.7);

        let json = serde_json::to_string(&p).unwrap();
        let p2: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(p2.tracks[0].clap_fx[0].id, "test.gain");
        assert_eq!(p2.tracks[0].clap_fx[0].params.get("1"), Some(&0.7));
        assert!(matches!(&p2.tracks[0].instrument, Instrument::Clap(s) if s.id == "test.tone"));

        // Old files (no clap_fx) deserialize fine.
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v["tracks"][0].as_object_mut().unwrap().remove("clap_fx");
        let p3: Project = serde_json::from_value(v).unwrap();
        assert!(p3.tracks[0].clap_fx.is_empty());

        // uid generation survives manual edits (max-scan).
        let mut p4 = Project::default_project();
        p4.tracks[0].clap_fx.push(ClapSlot {
            uid: "c41".into(),
            path: String::new(),
            id: String::new(),
            state: None,
            params: Default::default(),
        });
        let t = p4.tracks[0].id.clone();
        assert_eq!(p4.add_clap_fx(&t, "x", "y").unwrap(), "c42");

        // state blob storage + removal
        assert!(p.set_clap_state(&uid, "AAAA".into()));
        assert_eq!(p.tracks[0].clap_fx[0].state.as_deref(), Some("AAAA"));
        let t0c = p.tracks[0].id.clone();
        assert!(p.remove_clap_fx(&t0c, &uid));
        assert!(p.tracks[0].clap_fx.is_empty());
    }

    #[test]
    fn send_delay_is_an_effect_param() {
        let mut p = Project::default_project();
        let id = p.tracks[0].id.clone();
        assert!(p.nudge_effect(&id, EffectParam::SendDelay, 0.6));
        assert_eq!(p.tracks[0].effects.send_delay, 0.6);
        assert!(EffectParam::MIDI.contains(&EffectParam::SendDelay));
        assert!(EffectParam::AUDIO.contains(&EffectParam::SendDelay));
    }
}

#[cfg(test)]
mod expressive_sampler_tests {
    use super::*;

    fn pad() -> SamplePad {
        serde_json::from_value(serde_json::json!({"pitch":36,"path":"base.wav"})).unwrap()
    }
    fn take(min: u8, max: u8) -> SampleVariant {
        SampleVariant { path: "take.wav".into(), min_velocity: min, max_velocity: max, gain: 1. }
    }
    #[test]
    fn legacy_pads_and_valid_layers_roundtrip_without_losing_data() {
        let mut p = pad();
        assert!(p.variants.is_empty());
        assert!(serde_json::to_value(&p).unwrap().get("variants").is_none());
        p.variants = vec![take(1, 63), take(1, 63), take(64, 127)];
        p.validate_variants().unwrap();
        assert_eq!(p, serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap());
    }
    #[test]
    fn invalid_sampler_edits_preserve_the_previous_pad() {
        let mut p = Project::default_project();
        p.tracks[0].instrument = Instrument::Sampler { pads: vec![pad()] };
        p.set_sampler_variants("t1", 36, vec![take(1, 63)]).unwrap();
        assert!(p.set_sampler_variants("t1", 36, vec![take(1, 63), take(60, 127)]).is_err());
        assert!(p.set_sampler_variants("t1", 36, vec![take(0, 127)]).is_err());
        assert!(p.set_sampler_variants("t1", 36, vec![take(64, 63)]).is_err());
        assert!(p.set_sampler_variants("t1", 36, vec![take(1, 127); 33]).is_err());
        let Instrument::Sampler { pads } = &p.tracks[0].instrument else { panic!("sampler") };
        assert_eq!(pads[0].variants.len(), 1);
        p.set_sampler_pad("t1", 36, "new-base.wav".into());
        let Instrument::Sampler { pads } = &p.tracks[0].instrument else { panic!("sampler") };
        assert!(pads[0].variants.is_empty(), "replacing the instrument sample clears obsolete takes");
    }
    #[test]
    fn dynamics_edit_preserves_note_timing_and_legacy_step_defaults() {
        let mut p = Project::default_project();
        let Pattern::Step(sp) = &p.patterns[0] else { panic!("steps") };
        let id = sp.id.clone();
        assert!(p.set_step_velocity(&id, 0, 0, 42));
        let Pattern::Step(sp) = &p.patterns[0] else { panic!("steps") };
        assert_eq!(sp.rows[0].step_velocity(0), Some(42));
        assert!(!p.set_step_velocity(&id, 0, 0, 1));
        p.patterns.push(Pattern::PianoRoll(PianoRollPattern {
            id: "phrase".into(), name: "Phrase".into(), bars: 1, ticks_per_bar: 480, legato: false,
            notes: vec![Note { pitch: 60, tick: 62, duration: 79, vel: 80 }],
        }));
        assert!(p.set_note_velocity("phrase", 60, 62, 108));
        let Pattern::PianoRoll(pr) = p.patterns.last().unwrap() else { panic!("piano") };
        assert_eq!((pr.notes[0].tick, pr.notes[0].duration, pr.notes[0].vel), (62, 79, 108));
    }
}

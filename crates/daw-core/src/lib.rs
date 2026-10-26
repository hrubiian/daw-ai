pub mod dsp;
pub mod project;
pub mod undo;
pub use project::EffectChain;
pub mod generative;
pub mod mini_notation;
pub mod audio {
    pub mod balance;
    pub mod clap;
    pub mod engine;
    pub mod export;
    pub mod input;
    pub mod mixer;
    pub mod mp3;
    pub mod scheduling;
    pub(crate) mod spatial;
    pub use engine::{AudioEngine, RecordController, RecordState, load_wav_stereo, wav_peaks, wav_duration_sec, sec_to_sample};
    pub use balance::{auto_balance, compute_balance, BalanceResult};
    pub use export::{export_mp3, export_wav, render_project_samples, ExportFormat};
    pub use mixer::{AudioEvent, Freeverb, Mixer, PingPongDelay, ReturnBus, ReturnFx, Strip, StripKind, StripMeter, StripSpec, MAX_STRIPS, STRIP_UTILITY};
}
pub mod sequencer {
    pub mod engine;
    pub mod record;
    pub use engine::{Sequencer, SequencerCommand, TICKS_PER_BAR, TICKS_PER_BEAT};
    pub use record::MidiRecorder;
}

pub mod session;

pub mod render_control;

pub mod musical_time;

//! Shared, non-TUI I/O for daw-ai frontends: MIDI input, MIDI mapping, and the
//! engine `Host` that boots the audio engine + sequencer + MIDI input.
//!
//! `daw-core` stays free of I/O dependencies; `midir` lives here.
pub mod config;
pub mod host;
pub mod midi;
pub mod midi_map;

pub use host::Host;
pub use midi::{connect_midi_port, list_midi_ports};

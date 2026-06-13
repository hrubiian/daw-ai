//! GPUI desktop frontend for daw-ai.
//!
//! Phase 0: a graphite window with a transport bar, a live playhead counter mirrored
//! from the engine, and working Play/Pause. Reuses `daw-core` and `daw-io` exactly as
//! the TUI does. Later phases add the arrangement canvas, editor dock, mixer, and more.
pub mod app;
pub mod state;

pub use app::run;

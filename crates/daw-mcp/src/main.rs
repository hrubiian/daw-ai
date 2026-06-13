//! daw-mcp: stdio MCP server over one .daw.json project.
//! Usage: daw-mcp <song.daw.json>
//!
//! Every tool is a one-line wrapper over the sync `DawTools` core; a GUI
//! opened on the same file in JAM mode hears every edit live.

use daw_mcp::tools::{DawTools, NoteArg, StepRowArg};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
    ErrorData as McpError, ServerHandler, ServiceExt,
    transport::stdio,
};
use std::sync::Arc;

/// Tool failures become `isError` tool results (not protocol errors), so
/// the model always sees the actionable message.
fn reply(r: Result<String, String>) -> Result<CallToolResult, McpError> {
    Ok(match r {
        Ok(text) => CallToolResult::success(vec![Content::text(text)]),
        Err(e) => CallToolResult::error(vec![Content::text(e)]),
    })
}

#[derive(Clone)]
struct DawServer {
    tools: Arc<DawTools>,
    tool_router: ToolRouter<DawServer>,
}

// ── Tool argument structs (doc comments become the JSON schema docs) ─────────

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct JsonArg {
    /// The full project JSON document.
    json: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct TrackTypeArg {
    /// drum | melodic | audio
    track_type: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct TrackIdArg {
    track_id: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SetTrackArgs {
    track_id: String,
    /// 0.0..=1.0
    volume: Option<f32>,
    /// -1.0 (left) ..= 1.0 (right)
    pan: Option<f32>,
    muted: Option<bool>,
    name: Option<String>,
    /// GM program 0-127 (soundfont tracks).
    preset: Option<u8>,
    /// Path to a .sf2 (empty string = project default).
    soundfont: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct TempoArgs {
    /// Beats per minute, 20-300.
    bpm: f64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SwingArgs {
    /// 0.0 (straight) ..= 1.0 (triplet swing).
    amount: f32,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct StepsArgs {
    pattern_id: String,
    /// One row per drum sound.
    rows: Vec<StepRowArg>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct NotesArgs {
    pattern_id: String,
    /// Replaces the pattern's notes.
    notes: Vec<NoteArg>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct MiniNotationArgs {
    track_id: String,
    /// Mini-notation, e.g. "c3 e3 g3 c4" or "c3 . e3 g3" (. = rest); replaces
    /// the track's first pattern with these notes.
    source: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct PlaceClipArgs {
    track_id: String,
    pattern_id: String,
    /// Bar to place the clip at (0-based).
    bar: u32,
    /// Clip length in bars (pattern loops to fill); omit for natural length.
    len_bars: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ClipAtArgs {
    track_id: String,
    bar: u32,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ResizeClipArgs {
    track_id: String,
    bar: u32,
    len_bars: u32,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct EffectsArgs {
    track_id: String,
    /// EffectChain fields to set, e.g. {"reverb": 0.3, "send_delay": 0.2,
    /// "drive": 0.1, "cutoff": 0.8, "eq_low_gain": 3.0}.
    params: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ReturnsArgs {
    /// {"reverb": {gain,muted,room_size,damping,width}, "delay": {gain,muted,time_ms,feedback}} — any subset.
    params: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct LoopRegionArgs {
    start_bar: u32,
    /// Exclusive.
    end_bar: u32,
    /// Loop playback inside the region.
    enabled: Option<bool>,
    punch_in: Option<bool>,
    punch_out: Option<bool>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddAudioClipArgs {
    track_id: String,
    /// Path to a WAV file.
    path: String,
    start_sec: f64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AudioClipArgs {
    /// Index from project_summary's audio-clips list.
    index: usize,
    move_to_sec: Option<f64>,
    gain: Option<f32>,
    len_sec: Option<f64>,
    remove: Option<bool>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ClapAddArgs {
    track_id: String,
    /// Plugin id from list_clap_plugins (e.g. "org.surge-synth-team.surge-xt").
    plugin_id: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ClapRemoveArgs {
    track_id: String,
    /// Slot uid (e.g. "c1") from project_summary.
    uid: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ClapParamArgs {
    /// Slot uid (e.g. "c1").
    uid: String,
    param_id: u32,
    value: f64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct RenderArgs {
    /// First bar (0-based); omit for the song start.
    start_bar: Option<u32>,
    /// End bar (exclusive); omit for the song end.
    end_bar: Option<u32>,
    /// Track ids to solo (all others muted in the render only).
    tracks: Option<Vec<String>>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct WavPathArgs {
    /// Path to any WAV (e.g. a recorded take).
    path: String,
}

#[tool_router]
impl DawServer {
    fn new(tools: DawTools) -> Self {
        Self { tools: Arc::new(tools), tool_router: Self::tool_router() }
    }

    #[tool(description = "Compact project overview: tracks, patterns, arrangement, clips, loop region, returns. Call this first.")]
    fn project_summary(&self) -> Result<CallToolResult, McpError> {
        reply(self.tools.project_summary())
    }

    #[tool(description = "The full project as JSON (the open .daw.json format).")]
    fn get_project(&self) -> Result<CallToolResult, McpError> {
        reply(self.tools.get_project())
    }

    #[tool(description = "Replace the whole project with the given JSON (validated before saving). Escape hatch when no granular tool fits.")]
    fn set_project(&self, Parameters(a): Parameters<JsonArg>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_project(a.json))
    }

    #[tool(description = "Add a track (drum | melodic | audio). Returns its id.")]
    fn add_track(&self, Parameters(a): Parameters<TrackTypeArg>) -> Result<CallToolResult, McpError> {
        reply(self.tools.add_track(a.track_type))
    }

    #[tool(description = "Remove a track by id.")]
    fn remove_track(&self, Parameters(a): Parameters<TrackIdArg>) -> Result<CallToolResult, McpError> {
        reply(self.tools.remove_track(a.track_id))
    }

    #[tool(description = "Update track properties: volume, pan, mute, name, GM preset, soundfont path. All optional.")]
    fn set_track(&self, Parameters(a): Parameters<SetTrackArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_track(a.track_id, a.volume, a.pan, a.muted, a.name, a.preset, a.soundfont))
    }

    #[tool(description = "Set the tempo in BPM (20-300).")]
    fn set_tempo(&self, Parameters(a): Parameters<TempoArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_tempo(a.bpm))
    }

    #[tool(description = "Set swing 0.0-1.0 (offbeat 8th-note delay).")]
    fn set_swing(&self, Parameters(a): Parameters<SwingArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_swing(a.amount))
    }

    #[tool(description = "Add a new pattern to a track. Returns its id.")]
    fn add_pattern(&self, Parameters(a): Parameters<TrackIdArg>) -> Result<CallToolResult, McpError> {
        reply(self.tools.add_pattern(a.track_id))
    }

    #[tool(description = "Replace a step pattern's rows. Each row: pitch + a step string like 'x...x...x...x...' ('x' hit, '.' rest, '1'-'9' velocity).")]
    fn set_steps(&self, Parameters(a): Parameters<StepsArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_steps(a.pattern_id, a.rows))
    }

    #[tool(description = "Replace a piano-roll pattern's notes (480 ticks/bar, 120/beat).")]
    fn set_notes(&self, Parameters(a): Parameters<NotesArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_notes(a.pattern_id, a.notes))
    }

    #[tool(description = "Write a melody with mini-notation (e.g. 'c3 e3 g3 c4', '.' = rest) onto a track's first pattern. The natural way to compose melodies.")]
    fn apply_mini_notation(&self, Parameters(a): Parameters<MiniNotationArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.apply_mini_notation(a.track_id, a.source))
    }

    #[tool(description = "Place a pattern clip on the arrangement timeline at a bar (optionally stretched to len_bars).")]
    fn place_clip(&self, Parameters(a): Parameters<PlaceClipArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.place_clip(a.track_id, a.pattern_id, a.bar, a.len_bars))
    }

    #[tool(description = "Remove the clip at a bar on a track.")]
    fn remove_clip(&self, Parameters(a): Parameters<ClipAtArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.remove_clip(a.track_id, a.bar))
    }

    #[tool(description = "Resize the clip at a bar to len_bars (the pattern loops to fill).")]
    fn resize_clip(&self, Parameters(a): Parameters<ResizeClipArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.resize_clip(a.track_id, a.bar, a.len_bars))
    }

    #[tool(description = "Set effect-chain fields on a track: reverb (send), send_delay, chorus, cutoff, resonance, lfo_depth, delay_*, drive, eq_*, comp_*.")]
    fn set_effects(&self, Parameters(a): Parameters<EffectsArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_effects(a.track_id, a.params))
    }

    #[tool(description = "Configure the shared return buses (reverb: room_size/damping/width/gain/muted; delay: time_ms/feedback/gain/muted).")]
    fn set_returns(&self, Parameters(a): Parameters<ReturnsArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_returns(a.params))
    }

    #[tool(description = "Set the loop/punch region (bars; end exclusive) and its toggles.")]
    fn set_loop_region(&self, Parameters(a): Parameters<LoopRegionArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_loop_region(a.start_bar, a.end_bar, a.enabled, a.punch_in, a.punch_out))
    }

    #[tool(description = "Add a WAV file as an audio clip on a track at start_sec.")]
    fn add_audio_clip(&self, Parameters(a): Parameters<AddAudioClipArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.add_audio_clip(a.track_id, a.path, a.start_sec))
    }

    #[tool(description = "Move/crop/gain/remove an audio clip by index (see project_summary).")]
    fn audio_clip(&self, Parameters(a): Parameters<AudioClipArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.audio_clip(a.index, a.move_to_sec, a.gain, a.len_sec, a.remove))
    }

    #[tool(description = "List installed CLAP plugins (instruments and effects).")]
    fn list_clap_plugins(&self) -> Result<CallToolResult, McpError> {
        reply(self.tools.list_clap_plugins())
    }

    #[tool(description = "Append a CLAP effect plugin to a track's insert chain.")]
    fn add_clap_fx(&self, Parameters(a): Parameters<ClapAddArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.add_clap_fx(a.track_id, a.plugin_id))
    }

    #[tool(description = "Make a CLAP plugin the track's instrument.")]
    fn set_clap_instrument(&self, Parameters(a): Parameters<ClapAddArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_clap_instrument(a.track_id, a.plugin_id))
    }

    #[tool(description = "Remove a CLAP effect slot by uid.")]
    fn remove_clap_fx(&self, Parameters(a): Parameters<ClapRemoveArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.remove_clap_fx(a.track_id, a.uid))
    }

    #[tool(description = "Set a CLAP plugin parameter (param ids are plugin-specific).")]
    fn set_clap_param(&self, Parameters(a): Parameters<ClapParamArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_clap_param(a.uid, a.param_id, a.value))
    }

    #[tool(description = "Auto-balance: solo-render each track, measure loudness over its active bars, and set volumes so every track matches the median. Run after composing/arranging; then `listen` to confirm.")]
    fn auto_balance(&self) -> Result<CallToolResult, McpError> {
        reply(self.tools.auto_balance())
    }

    #[tool(description = "LISTEN: render a bar range and return a numeric report — peak/RMS dBFS, clipping, per-bar loudness, stereo width, 7-band spectral balance. tracks=[…] solos a subset. This is how you hear the project.")]
    fn listen(&self, Parameters(a): Parameters<RenderArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.listen(a.start_bar, a.end_bar, a.tracks))
    }

    #[tool(description = "Render a bar range to a WAV for the human to audition. Returns the path.")]
    fn render(&self, Parameters(a): Parameters<RenderArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.render(a.start_bar, a.end_bar, a.tracks))
    }

    #[tool(description = "Run the numeric listening analysis on any WAV file (e.g. a recorded take).")]
    fn analyze_wav(&self, Parameters(a): Parameters<WavPathArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.analyze_wav(a.path))
    }
}

#[tool_handler]
impl ServerHandler for DawServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions(
                "daw-ai project server. Start with project_summary. Edit with granular tools \
                 (apply_mini_notation for melodies, set_steps for drums), then `listen` to hear \
                 the result as numbers. A GUI in JAM mode picks up every edit live."
                    .to_string(),
            )
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: daw-mcp <song.daw.json>");
        std::process::exit(2);
    });
    let service = DawServer::new(DawTools::new(path.into())).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

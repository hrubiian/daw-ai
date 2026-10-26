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

async fn blocking(f: impl FnOnce() -> Result<String, String> + Send + 'static) -> Result<CallToolResult, McpError> {
    static LIMIT: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> = std::sync::OnceLock::new();
    let permit = match LIMIT.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2))).clone().try_acquire_owned() {
        Ok(p) => p, Err(_) => return reply(Err("analysis busy: retry when a current job completes".into())),
    };
    reply(tokio::task::spawn_blocking(move || { let _permit = permit; f() }).await.unwrap_or_else(|e| Err(format!("analysis worker failed: {e}"))))
}

#[derive(Clone)]
struct DawServer {
    tools: Arc<DawTools>,
    tool_router: ToolRouter<DawServer>,
}

// ── Tool argument structs (doc comments become the JSON schema docs) ─────────

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct JobIdArg { id: String }
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct RatingArg { id: String, score: u8, comment: String }
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct CompareArg { a: String, b: String }
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct HistoryArgs {
    /// Offset into render IDs sorted lexicographically (default 0).
    offset: Option<usize>,
    /// Reports per page, 1–100 (default 20).
    limit: Option<usize>,
}

impl DawServer {
    fn render_base(&self) -> Result<std::path::PathBuf, String> {
        if self.tools.store.live_socket.is_some() {
            self.tools.store.inspect_live()?.project_directory.ok_or_else(|| "live session has no project directory".into())
        } else {
            Ok(self.tools.store.path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf())
        }
    }
}


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
    /// 0.0..=2.0; 1.0 is unity, 2.0 is approximately +6 dB.
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
struct LegatoArgs {
    pattern_id: String,
    on: bool,
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
    /// Sub-bar start offset in ticks (480/bar; 240 = half a bar, 120 = one
    /// beat). Omit or 0 to start exactly on the bar line.
    offset_ticks: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ClipOffsetArgs {
    track_id: String,
    /// Bar the clip starts in (0-based).
    bar: u32,
    /// Start offset within the bar, in ticks (480/bar; 240 = half a bar,
    /// 120 = one beat). Whole bars fold into `bar`.
    offset_ticks: u32,
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
struct SamplerVariantsArgs {
    track_id: String,
    pitch: u8,
    /// Array of {path,min_velocity,max_velocity,gain?}. Ranges 1..127 must be
    /// disjoint or identical; equal ranges rotate takes. Max 32, [] clears them.
    variants: serde_json::Value,
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
    /// Nested reverb/echo fields; any subset. Includes tone, pre-delay, tempo sync and echo modulation.
    params: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SpatialPresetArgs { preset: String }

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ClapUidArgs { uid: String }

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

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct DiffArgs {
    /// Path to another .daw.json to compare against (the "before" version).
    other_path: String,
}

#[tool_router]
impl DawServer {
    fn new(tools: DawTools) -> Self {
        Self { tools: Arc::new(tools), tool_router: Self::tool_router() }
    }

    #[tool(description = "Start a bounded background listening job for an immutable project revision. Returns job id; poll job_status or use cancel_job. Artifacts include WAV, project snapshot and provenance report. Does not block live edits.")]
    fn start_listen(&self, Parameters(a): Parameters<RenderArgs>) -> Result<CallToolResult, McpError> {
        let result = (|| -> Result<String, String> {
            let (mut project, revision, directory) = if self.tools.store.live_socket.is_some() {
                let state = self.tools.store.inspect_live()?;
                (state.project, Some(state.revision), state.project_directory.unwrap_or_else(|| std::path::PathBuf::from(".")))
            } else { (self.tools.store.load()?, None, self.tools.store.path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf()) };
            if let Some(tracks) = a.tracks {
                for id in &tracks { if !project.tracks.iter().any(|t| &t.id == id) { return Err(format!("missing track {id}")); } }
                for track in &mut project.tracks { track.muted |= !tracks.contains(&track.id); }
            }
            let range = if a.start_bar.is_some() || a.end_bar.is_some() { Some((a.start_bar.unwrap_or(0), a.end_bar.unwrap_or_else(|| project.song_bars()))) } else { None };
            daw_mcp::jobs::manager().submit(project, revision, directory, self.tools.sf2_path.clone(), range)
        })(); reply(result)
    }
    #[tool(description = "Get progress, errors, structured listening measurements and revision-linked artifacts for a listening job.")]
    fn job_status(&self, Parameters(a): Parameters<JobIdArg>) -> Result<CallToolResult, McpError> {
        reply(self.render_base().and_then(|base| daw_mcp::jobs::manager().status_in(&base, &a.id)).and_then(|s| serde_json::to_string_pretty(&s).map_err(|e| e.to_string())))
    }
    #[tool(description = "Record the musician's own 1–5 rating and comment for a completed listening job. Do not invent ratings on their behalf.")]
    fn rate_render(&self, Parameters(a): Parameters<RatingArg>) -> Result<CallToolResult, McpError> {
        reply(self.render_base().and_then(|base| daw_mcp::jobs::manager().rate_in(&base, &a.id, a.score, &a.comment)).map(|p| p.display().to_string()))
    }
    #[tool(description = "Compare two completed revision-linked renders with matching settings. Returns audio artifact paths and structured diagnostics for A/B listening; metrics do not measure musical quality.")]
    fn compare_renders(&self, Parameters(a): Parameters<CompareArg>) -> Result<CallToolResult, McpError> {
        reply(self.render_base().and_then(|base| daw_mcp::jobs::manager().compare_in(&base, &a.a, &a.b)).and_then(|v| serde_json::to_string_pretty(&v).map_err(|e| e.to_string())))
    }
    #[tool(description = "List durable listening reports in the current project directory, including earlier MCP runs. Paginated by lexicographically sorted render ID; returns malformed-entry errors without hiding valid reports. A saved running job owned by another process is unavailable, not claimed failed.")]
    fn render_history(&self, Parameters(a): Parameters<HistoryArgs>) -> Result<CallToolResult, McpError> {
        reply(self.render_base().and_then(|base| daw_mcp::jobs::manager().history(&base, a.offset.unwrap_or(0), a.limit.unwrap_or(20)))
            .and_then(|v| serde_json::to_string_pretty(&v).map_err(|e| e.to_string())))
    }
    #[tool(description = "Cooperatively cancel a background listening job.")]
    fn cancel_job(&self, Parameters(a): Parameters<JobIdArg>) -> Result<CallToolResult, McpError> {
        reply(daw_mcp::jobs::manager().cancel(&a.id))
    }

    #[tool(description = "Inspect the connected live session, including revision, transport, engine application state and errors. Offline mode returns an error.")]
    fn session_state(&self) -> Result<CallToolResult, McpError> {
        reply(self.tools.store.inspect_live().and_then(|s| serde_json::to_string_pretty(&s).map_err(|e| e.to_string())))
    }

    #[tool(description = "Apply a version 1 session request encoded as JSON: operation_id, expected_revision, and command. Commands: inspect, preview (label + edits; validates without committing), batch (label + edits), undo, redo, play, pause, stop, seek (tick). Batch edits: tempo (bpm), track (id, optional volume/pan/muted), remove_track (id), replace (project). Refresh session_state after revision_conflict. Live mode only.")]
    fn session_command(&self, Parameters(a): Parameters<JsonArg>) -> Result<CallToolResult, McpError> {
        let result = serde_json::from_str(&a.json).map_err(|e| format!("invalid session request: {e}"))
            .and_then(|req| self.tools.store.live_request(req))
            .and_then(|r| {
                let text = serde_json::to_string_pretty(&r).map_err(|e| e.to_string())?;
                if r.error.is_some() { Err(text) } else { Ok(text) }
            });
        reply(result)
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

    #[tool(description = "Set note-timing humanization 0.0-1.0 (looseness; ~32nd jitter at full) for a less mechanical feel.")]
    fn set_timing(&self, Parameters(a): Parameters<SwingArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_timing(a.amount))
    }

    #[tool(description = "Toggle legato on a piano-roll pattern: notes connect to the next note's start (smoother melodic lines).")]
    fn set_legato(&self, Parameters(a): Parameters<LegatoArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_legato(a.pattern_id, a.on))
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

    #[tool(description = "Place a pattern clip on the arrangement timeline at a bar (optionally stretched to len_bars; optional sub-bar offset_ticks for off-grid placement, 480/bar).")]
    fn place_clip(&self, Parameters(a): Parameters<PlaceClipArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.place_clip(a.track_id, a.pattern_id, a.bar, a.len_bars, a.offset_ticks))
    }

    #[tool(description = "Nudge a clip off the bar line: set its sub-bar start offset in ticks (480/bar; 240 = half a bar, 120 = one beat).")]
    fn set_clip_offset(&self, Parameters(a): Parameters<ClipOffsetArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_clip_offset(a.track_id, a.bar, a.offset_ticks))
    }

    #[tool(description = "Remove the clip at a bar on a track.")]
    fn remove_clip(&self, Parameters(a): Parameters<ClipAtArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.remove_clip(a.track_id, a.bar))
    }

    #[tool(description = "Resize the clip at a bar to len_bars (the pattern loops to fill).")]
    fn resize_clip(&self, Parameters(a): Parameters<ResizeClipArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.resize_clip(a.track_id, a.bar, a.len_bars))
    }

    #[tool(description = "Set sampler velocity layers and round-robin alternate takes for an existing pad. Variants: [{path,min_velocity,max_velocity,gain?}]. Distinct ranges choose soft/hard sounds; identical ranges rotate deterministically. Uncovered velocities use the base sample. [] clears takes. Changes are validated and use shared live undo.")]
    fn set_sampler_variants(&self, Parameters(a): Parameters<SamplerVariantsArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_sampler_variants(a.track_id, a.pitch, a.variants))
    }

    #[tool(description = "Set effect-chain fields on a track: reverb (send), send_delay, chorus, cutoff, resonance, lfo_depth, delay_*, drive, eq_*, comp_*.")]
    fn set_effects(&self, Parameters(a): Parameters<EffectsArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_effects(a.track_id, a.params))
    }

    #[tool(description = "Configure shared spatial returns. Reverb: room_size, damping, width, predelay_ms (0–250), low_cut_hz, high_cut_hz, gain, muted. Echo: time_ms (1–12000), sync (null for free, or sixteenth/eighth/eighth_dotted/eighth_triplet/quarter/quarter_dotted/half/whole), mode (ping_pong/stereo), feedback, low_cut_hz, high_cut_hz, width, modulation_ms (0–20), modulation_hz (0.05–8), gain, muted. Synced echoes follow project tempo. Track reverb/send_delay set send amounts; bus edits affect all sending tracks.")]
    fn set_returns(&self, Parameters(a): Parameters<ReturnsArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_returns(a.params))
    }

    #[tool(description = "Apply a shared space/echo preset: small_room, wide_hall, dark_space, tape_echo, ping_pong, slapback. Preserves bus gain and mute; affects all tracks sending to that bus. Undoable in a live session.")]
    fn apply_spatial_preset(&self, Parameters(a): Parameters<SpatialPresetArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.apply_spatial_preset(a.preset))
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

    #[tool(description = "Rescan installed Linux CLAP plugins after installation. Refreshes this MCP process's plugin catalog.")]
    async fn rescan_clap_plugins(&self) -> Result<CallToolResult, McpError> {
        let tools=self.tools.clone(); blocking(move ||tools.rescan_clap_plugins()).await
    }

    #[tool(description = "Inspect a CLAP slot's parameter ids, bounds, defaults, integer/read-only flags and saved overrides. Loads a separate instance on a worker; does not change the project or read live automation/internal edits. Use these ids before set_clap_param.")]
    async fn get_clap_params(&self, Parameters(a): Parameters<ClapUidArgs>) -> Result<CallToolResult, McpError> {
        let tools=self.tools.clone(); blocking(move ||tools.get_clap_params(a.uid)).await
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
    async fn auto_balance(&self) -> Result<CallToolResult, McpError> {
        let tools = self.tools.clone();
        blocking(move || tools.auto_balance()).await
    }

    #[tool(description = "ANALYZE MIX: solo-render each track and report where each lives in the spectrum plus which tracks mask each other (frequency collisions, low-band first). Use after composing to find a muddy/cluttered mix that whole-mix `listen` can't localize.")]
    async fn analyze_mix(&self) -> Result<CallToolResult, McpError> {
        let tools = self.tools.clone();
        blocking(move || tools.analyze_mix()).await
    }

    #[tool(description = "ANALYZE ARRANGEMENT: report the song's density (tracks active per bar) and dynamic arc as sparklines, and flag flat/static textures or a missing intro build. Use to check the piece has shape — a build, variation, sections — not just a constant wall of sound.")]
    async fn analyze_arrangement(&self) -> Result<CallToolResult, McpError> {
        let tools = self.tools.clone();
        blocking(move || tools.analyze_arrangement()).await
    }

    #[tool(description = "ANALYZE GROOVE: symbolic timing/feel report — how notes sit against the 16th grid (on-grid % + micro-timing spread) plus swing/humanize/timing. Flags dead-mechanical patterns. Fast, no render. Use to check the rhythm feels human, not robotic.")]
    fn analyze_groove(&self) -> Result<CallToolResult, McpError> {
        reply(self.tools.analyze_groove())
    }

    #[tool(description = "DIFF PROJECT: describe the musical changes from another .daw.json (the 'before') to the current project — added/removed tracks, volume/instrument/effect edits, note-count changes, arrangement moves. Semantic, not a JSON line-diff. Pass other_path (e.g. a git-stashed copy) to review what an edit changed.")]
    fn diff_project(&self, Parameters(a): Parameters<DiffArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.diff_project(a.other_path))
    }

    #[tool(description = "SNAPSHOT: render the project and save an analyzed fingerprint (peak/RMS/clipping/spectral balance + track & note counts) as a CI baseline beside the project. Run when the mix is in a good state.")]
    async fn snapshot(&self) -> Result<CallToolResult, McpError> {
        let tools = self.tools.clone();
        blocking(move || tools.snapshot()).await
    }

    #[tool(description = "CHECK: re-render and report regressions vs the saved snapshot — FAIL on new clipping, WARN on peak jumps / band drift / lost tracks or notes, INFO on loudness moves. 'Does this song still pass?' Run after edits.")]
    async fn check(&self) -> Result<CallToolResult, McpError> {
        let tools = self.tools.clone();
        blocking(move || tools.check()).await
    }

    #[tool(description = "LISTEN: render a bar range and return a numeric report — peak/RMS dBFS, clipping, per-bar loudness, stereo width, 7-band spectral balance. tracks=[…] solos a subset. This is how you hear the project.")]
    async fn listen(&self, Parameters(a): Parameters<RenderArgs>) -> Result<CallToolResult, McpError> {
        let tools = self.tools.clone();
        blocking(move || tools.listen(a.start_bar, a.end_bar, a.tracks)).await
    }

    #[tool(description = "Render a bar range to a WAV for the human to audition. Returns the path.")]
    async fn render(&self, Parameters(a): Parameters<RenderArgs>) -> Result<CallToolResult, McpError> {
        let tools = self.tools.clone();
        blocking(move || tools.render(a.start_bar, a.end_bar, a.tracks)).await
    }

    #[tool(description = "Run the numeric listening analysis on any WAV file (e.g. a recorded take).")]
    async fn analyze_wav(&self, Parameters(a): Parameters<WavPathArgs>) -> Result<CallToolResult, McpError> {
        let tools = self.tools.clone();
        blocking(move || tools.analyze_wav(a.path)).await
    }
}

#[tool_handler]
impl ServerHandler for DawServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions(
                "daw-ai project server. Start with project_summary; in live mode also inspect session_state. \
                 Compose short editable phrases with explicit velocities, note lengths, intentional timing, rests and musical development. \
                 Global swing and random humanization do not replace phrasing. Use set_notes for detailed performances and set_sampler_variants for velocity-layered or alternate sample takes. \
                 Use start_listen for cancellable revision-linked audio renders and diagnostics. Audition the audio with the musician before expanding an arrangement; numerical analysis does not establish musical quality, and do not invent listening judgments or ratings. \
                 Live mode shares revision-checked edits and undo with the musician; inspect engine application status. Offline mode edits files only."
                    .to_string(),
            )
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let first = args.next().ok_or_else(|| anyhow::anyhow!("usage: daw-mcp <project> | --list-sessions | --session <socket>"))?;
    if first == "--list-sessions" {
        println!("{}", serde_json::to_string_pretty(&daw_io::session::discover()?)?);
        return Ok(());
    }
    let tools = if first == "--session" {
        let socket = args.next().ok_or_else(|| anyhow::anyhow!("--session needs a socket path"))?;
        let mut tools = DawTools::new("live.daw.json".into());
        tools.store = daw_mcp::store::ProjectStore::live(socket.into()).map_err(anyhow::Error::msg)?;
        tools
    } else {
        DawTools::new(daw_core::project::format::absolute(std::path::Path::new(&first))?)
    };
    let service = DawServer::new(tools).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

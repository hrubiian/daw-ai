//! daw-mcp: stdio MCP server over a live session or a .daw.json file.
//! Usage: daw-mcp --live or daw-mcp <song.daw.json>
//!
//! Every tool is a one-line wrapper over the sync `DawTools` core; a GUI
//! opened on the same file in JAM mode hears every edit live.

use daw_mcp::{
    store::ProjectStore,
    tools::{DawTools, NoteArg, StepRowArg},
};
use rmcp::{
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
    service::RequestContext,
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    transport::stdio,
};
use std::{path::PathBuf, sync::{Arc, Mutex}};

/// Tool failures become `isError` tool results (not protocol errors), so
/// the model always sees the actionable message.
fn reply(r: Result<String, String>) -> Result<CallToolResult, McpError> {
    Ok(match r {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
    })
}

async fn blocking(f: impl FnOnce() -> Result<String, String> + Send + 'static) -> Result<CallToolResult, McpError> {
    reply(blocking_result(f).await)
}

async fn blocking_result<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    static LIMIT: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> = std::sync::OnceLock::new();
    let permit = match LIMIT.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2))).clone().try_acquire_owned() {
        Ok(p) => p, Err(_) => return Err("analysis busy: retry when a current job completes".into()),
    };
    tokio::task::spawn_blocking(move || { let _permit = permit; f() }).await.unwrap_or_else(|e| Err(format!("analysis worker failed: {e}")))
}

#[derive(Clone)]
struct DawServer {
    tools: Arc<DawTools>,
    automatic_session: Option<Arc<AutomaticSession>>,
    tool_router: ToolRouter<DawServer>,
}

#[derive(Default)]
struct AutomaticSession {
    selected: Mutex<Option<(PathBuf, Arc<DawTools>)>>,
}

impl AutomaticSession {
    fn tools(&self, inspect: bool) -> Result<Arc<DawTools>, String> {
        let sockets = daw_io::session::discover()
            .map_err(|error| format!("Could not discover live DAW sessions: {error}"))?;
        let socket = match sockets.as_slice() {
            [socket] => socket,
            [] => return Err("No live DAW session found. Open daw-ai and try again.".into()),
            _ => return Err("Multiple live DAW sessions found. Leave one open or select a socket with --list-sessions and --session.".into()),
        };
        let store = ProjectStore::live(socket.clone())?;
        let mut selected = self.selected.lock().map_err(|_| "Could not lock the live DAW selection")?;
        if let Some((previous, tools)) = selected.as_ref() {
            if previous == socket && tools.store.path == store.path {
                return Ok(tools.clone());
            }
            if !inspect {
                return Err("The live DAW session changed. Call session_state or project_summary to check the open song before editing.".into());
            }
        }
        let tools = Arc::new(DawTools::with_store(store));
        *selected = Some((socket.clone(), tools.clone()));
        Ok(tools)
    }
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
    /// Constant detune in cents, -200..=200 (0 = 12-TET). Useful for matching
    /// a sample or a sung take that sits between the keys.
    tuning_cents: Option<f32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SamplerPadArgs {
    track_id: String,
    /// MIDI pitch that triggers this pad (GM drums: 36 kick, 38 snare, 42 hat).
    pitch: u8,
    /// .wav path, relative to the project file or absolute.
    path: String,
    /// Pad gain 0..=2 (default 1.0).
    gain: Option<f32>,
    /// true: the sample follows the note length (note-off fades it in ~30 ms),
    /// so one long sample covers every duration. false (default): one-shot.
    gate: Option<bool>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ModulatorsArgs {
    track_id: String,
    /// Array of {dest: cutoff|resonance|volume|pan, shape: sine|triangle|saw|square,
    /// rate_hz: 0..100, depth: 0..1, phase?: 0..1}. [] removes all LFOs.
    modulators: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct TempoArgs {
    /// Beats per minute, 20-400.
    bpm: f64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct TimeSignatureArgs {
    /// Beats per bar, 1-16 (e.g. 7 for 7/4).
    numerator: u8,
    /// Beat unit: 2, 4, 8, or 16.
    denominator: u8,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct TempoChangesArgs {
    /// Ordered tempo change points [{tick, bpm, ramp?}]; tick is arrangement ticks, bpm 20-400, ramp=true linearly ramps from previous tempo. Empty clears.
    points: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct LegatoArgs {
    pattern_id: String,
    on: bool,
    /// Overlap past next note's onset in ticks (0-240; default 0). Use 8-20 for expressive bowing/wind overlap.
    overlap_ticks: Option<u16>,
    /// Maximum rest gap in ticks that legato will bridge (omit to preserve; e.g. 90 preserves intentional phrasing rests).
    max_gap_ticks: Option<u32>,
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
    /// Per-clip gain multiplier (0.0..=2.0; default 1.0) for section-level dynamics.
    gain: Option<f32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ClipGainArgs {
    track_id: String,
    /// Bar the clip starts in (0-based).
    bar: u32,
    /// Gain multiplier (0.0..=2.0; 1.0 = unity).
    gain: f32,
    /// Sub-bar offset if disambiguating clips within the same bar.
    offset_ticks: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct VolumeAutomationArgs {
    track_id: String,
    /// Ordered breakpoints [{tick, gain}] where tick is arrangement ticks (480/bar) and gain is 0.0..=2.0 (empty clears).
    points: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct PatternCcArgs {
    pattern_id: String,
    /// MIDI CC points [{tick, cc, value}] inside the pattern; cc in {1=mod/vibrato, 11=expression, 64=sustain pedal, 74=brightness}, value 0..=127.
    events: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct PatternSwingArgs {
    pattern_id: String,
    /// Optional 8th-note swing 0.0..=1.0 for this pattern (omit or null to clear override).
    swing: Option<f32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct TrackPitchBendRangeArgs {
    track_id: String,
    /// Pitch bend range in semitones (1..=24, default 2). Applied via GM RPN 0.
    semitones: u8,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct TrackSfzArgs {
    track_id: String,
    /// Path to an `.sfz` instrument file (relative to project/workspace root or absolute).
    path: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct PatternPitchBendsArgs {
    pattern_id: String,
    /// Pitch bend points [{tick, cents}] inside the pattern; cents in -2400.0..=2400.0 relative to track tuning_cents.
    events: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AnalyzeMixArgs {
    /// Optional protagonist track id to compute per-bar 500-6000 Hz Lead-to-Accompaniment Ratio (LAR) and foreground burial warnings.
    lead_track_id: Option<String>,
    /// Optional start bar (0-based) for LAR window.
    start_bar: Option<u32>,
    /// Optional end bar (exclusive) for LAR window.
    end_bar: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AnalyzeMelodyArgs {
    /// Track id of the melodic lead to inspect across arranged clips.
    track_id: String,
    /// Optional start bar (0-based).
    start_bar: Option<u32>,
    /// Optional end bar (exclusive).
    end_bar: Option<u32>,
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
struct FindPatchesArgs {
    /// Words to match in the patch name or category ("" for all).
    query: String,
    /// Category folder, e.g. Basses, Pads, Leads, Percussion, Plucks, FX, Keys, Winds.
    category: Option<String>,
    /// Most results (default 20).
    limit: Option<usize>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct LoadPatchArgs {
    /// Patch path from find_patches.
    patch_path: String,
    /// Existing track to load onto; omit to create a new track for this sound.
    track_id: Option<String>,
    /// Name for a new track (defaults to the patch name), e.g. "Dusty Tom".
    name: Option<String>,
    /// Required to replace a named track's non-Surge instrument.
    replace: Option<bool>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ShapeSoundArgs {
    /// Surge XT slot uid (from load_patch / project_summary).
    uid: String,
    /// Controls to set: names from describe_sound; values are text in the
    /// readout's units ("800 Hz", "120 ms") or a number in the control's range.
    controls: serde_json::Map<String, serde_json::Value>,
}

#[derive(serde::Deserialize, schemars::JsonSchema, Clone, Copy)]
struct AuditionNoteArg {
    /// MIDI pitch (60 = middle C).
    pitch: u8,
    /// 1-127.
    velocity: u8,
    /// Start, ms from the phrase start.
    start_ms: u32,
    /// Length in ms.
    dur_ms: u32,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AuditionArgs {
    uid: String,
    /// Built-in phrase: hit | note | chord | line (default note), in the song's key.
    kind: Option<String>,
    /// Explicit phrase instead of `kind` (at most 64 notes, 16 s).
    notes: Option<Vec<AuditionNoteArg>>,
    /// Octave of the phrase root (3 = MIDI 48-59; default 4). Lower for bass, higher for leads.
    octave: Option<i8>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct PrintArgs {
    uid: String,
    /// Built-in phrase to print (hit | note | chord | line) when no bar range is given.
    kind: Option<String>,
    /// Explicit phrase to print instead of `kind`.
    notes: Option<Vec<AuditionNoteArg>>,
    /// Print the track's own part from this bar (0-based) instead of a phrase.
    start_bar: Option<u32>,
    /// End bar (exclusive) of that part; default the song end.
    end_bar: Option<u32>,
    /// Bar to place the printed clip at (default start_bar or 0).
    at_bar: Option<u32>,
}

fn phrase_kind(kind: Option<String>) -> Result<Option<daw_core::audition::PhraseKind>, String> {
    kind.map(|k| serde_json::from_value(serde_json::Value::String(k.clone()))
        .map_err(|_| format!("unknown phrase kind '{k}' (hit | note | chord | line)")))
        .transpose()
}

fn audition_notes(notes: Option<Vec<AuditionNoteArg>>) -> Option<Vec<daw_core::audition::AuditionNote>> {
    notes.map(|v| v.into_iter().map(|n| daw_core::audition::AuditionNote {
        pitch: n.pitch, velocity: n.velocity, start_ms: n.start_ms, dur_ms: n.dur_ms,
    }).collect())
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct RevertSoundArgs {
    uid: String,
    /// Version number from sound_history.
    version: usize,
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
struct CheckTracksArgs {
    /// Track IDs to check. Omit for every track in project order.
    track_ids: Option<Vec<String>>,
    /// First bar, zero-based. Defaults to 0.
    start_bar: Option<u32>,
    /// Exclusive end bar. Defaults to start_bar + 8. Maximum span is 32 bars.
    end_bar: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ListenTrackArgs {
    track_id: String,
    /// Start bar of the excerpt, zero-based. Defaults to 0.
    start_bar: Option<u32>,
    /// Excerpt duration, 0.25–6 seconds. Defaults to 3.
    duration_seconds: Option<f64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ListeningJobArgs {
    #[serde(flatten)]
    render: RenderArgs,
    /// Bypass DAW inserts and shared effects in the render snapshot. Instrument
    /// envelopes and plugin-internal effects remain active. Defaults to false.
    dry: Option<bool>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AuditionExcerptArgs {
    /// Completed listening job ID.
    id: String,
    /// Seconds from the start of the render, including a range render.
    offset_seconds: f64,
    /// Excerpt duration, 0.25–6 seconds.
    duration_seconds: f64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct PrepareAuditionArgs {
    /// First completed listening job ID.
    first: String,
    /// Second completed listening job ID with the same range and settings.
    second: String,
    /// Seconds from the start of both renders.
    offset_seconds: f64,
    /// Equal excerpt duration, 5–30 seconds.
    duration_seconds: f64,
    /// A focused question the listener can answer, without revealing the variants.
    question: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AuditionChoiceArgs {
    /// ID returned by prepare_audition.
    id: String,
    /// The musician's own A/B preference.
    preference: daw_mcp::audition::Preference,
    /// The musician's own comment, ideally pointing to a specific phrase or sound.
    comment: Option<String>,
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
    fn new(tools: DawTools, automatic: bool) -> Self {
        Self {
            tools: Arc::new(tools),
            automatic_session: automatic.then(|| Arc::new(AutomaticSession::default())),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Start a bounded background listening job for an immutable project revision. Returns job id; poll job_status or use cancel_job. Artifacts include WAV, project snapshot and provenance report. Does not block live edits.")]
    fn start_listen(&self, Parameters(args): Parameters<ListeningJobArgs>) -> Result<CallToolResult, McpError> {
        let ListeningJobArgs { render: a, dry } = args;
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
            daw_mcp::jobs::manager().submit_with_dry(
                project, revision, directory, self.tools.sf2_path.clone(), range, dry.unwrap_or(false),
            )
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

    #[tool(description = "Return a short mono WAV from a completed render as MCP audio. Use an audio-capable client/model to judge the sound; text-only clients should offer the WAV to the musician.")]
    async fn audition_excerpt(&self, Parameters(a): Parameters<AuditionExcerptArgs>) -> Result<CallToolResult, McpError> {
        let base = self.render_base();
        match blocking_result(move || {
            let report = daw_mcp::jobs::manager().status_in(&base?, &a.id)?;
            let bytes = daw_mcp::audition::preview(&report, a.offset_seconds, a.duration_seconds).map_err(|e| e.to_string())?;
            Ok((report, bytes))
        }).await {
            Ok((report, bytes)) => {
                use base64::Engine;
                let audio = ContentBlock::audio(
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                    "audio/wav",
                );
                let paths = report.artifacts.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ");
                Ok(CallToolResult::success(vec![ContentBlock::text(format!("Mono excerpt from render {}. Full render artifacts: {paths}", report.id)), audio]))
            }
            Err(error) => reply(Err(error)),
        }
    }

    #[tool(description = "Prepare two stereo WAVs for a listener to compare. Uses the same excerpt from two completed renders, matches integrated loudness with gain only and randomizes which revision is called A or B. Requires FFmpeg. Ask the listener the supplied question.")]
    async fn prepare_audition(&self, Parameters(a): Parameters<PrepareAuditionArgs>) -> Result<CallToolResult, McpError> {
        let base = self.render_base();
        blocking(move || {
            let base = base?;
            let jobs = daw_mcp::jobs::manager();
            let first = jobs.status_in(&base, &a.first)?;
            let second = jobs.status_in(&base, &a.second)?;
            let pair = daw_mcp::audition::prepare(&base, first, second, a.offset_seconds, a.duration_seconds, a.question).map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&pair).map_err(|e| e.to_string())
        }).await
    }

    #[tool(description = "Record the musician's explicit A/B choice and words, and reveal which render was assigned to A and B. Call only after the listener responds. Keep the feedback tied to that listening question.")]
    fn record_audition_choice(&self, Parameters(a): Parameters<AuditionChoiceArgs>) -> Result<CallToolResult, McpError> {
        reply(self.render_base().and_then(|base| {
            let choice = daw_mcp::audition::record_choice(&base, &a.id, a.preference, a.comment.as_deref().unwrap_or_default()).map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&choice).map_err(|e| e.to_string())
        }))
    }

    #[tool(description = "Read the daw-ai composition playbook for developing an editable musical idea, rendering alternatives, and iterating from listening feedback.")]
    fn composition_guide(&self) -> Result<CallToolResult, McpError> {
        reply(Ok(include_str!("../../../docs/music-collaboration.md").into()))
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

    #[tool(description = "Apply a version 1 session request encoded as JSON: operation_id, expected_revision, and command. Commands: inspect, preview (label + edits; validates without committing), batch (label + edits; one undo), undo, redo, play, pause, stop, seek (tick). Edits: add_piano_roll (track_id, pattern_id, name, bars, notes), set_notes (pattern_id, notes), place_clip (track_id, pattern_id, bar, optional offset_ticks/len_bars), remove_clip (track_id, bar, optional offset_ticks), tempo, track, remove_track, replace. Notes use pitch, tick, duration, vel. Bars are zero-based. Pause before edits that load audio resources. Refresh session_state after revision_conflict. Live mode only.")]
    fn session_command(&self, Parameters(a): Parameters<JsonArg>) -> Result<CallToolResult, McpError> {
        let result = serde_json::from_str(&a.json).map_err(|e| format!("invalid session request: {e}"))
            .and_then(|req| self.tools.store.live_request(req))
            .and_then(|r| {
                let text = serde_json::to_string_pretty(&r).map_err(|e| e.to_string())?;
                if r.error.is_some() { Err(text) } else { Ok(text) }
            });
        reply(result)
    }

    #[tool(description = "Compact project overview: tracks, track comments, patterns, arrangement, clips, loop region, returns. Call this first.")]
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

    #[tool(description = "Update track properties: volume, pan, mute, name, GM preset, soundfont path, tuning_cents. All optional.")]
    fn set_track(&self, Parameters(a): Parameters<SetTrackArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_track(a.track_id, a.volume, a.pan, a.muted, a.name, a.preset, a.soundfont, a.tuning_cents))
    }

    #[tool(description = "Set the tempo in BPM (20-400).")]
    fn set_tempo(&self, Parameters(a): Parameters<TempoArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_tempo(a.bpm))
    }

    #[tool(description = "Set the project time signature (numerator 1-16, denominator 2|4|8|16, e.g. 7/4).")]
    fn set_time_signature(&self, Parameters(a): Parameters<TimeSignatureArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_time_signature(a.numerator, a.denominator))
    }

    #[tool(description = "Set arrangement tempo changes [{tick, bpm, ramp?}] (empty clears).")]
    fn set_tempo_changes(&self, Parameters(a): Parameters<TempoChangesArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_tempo_changes(a.points))
    }

    #[tool(description = "Set swing 0.0-1.0 (offbeat 8th-note delay).")]
    fn set_swing(&self, Parameters(a): Parameters<SwingArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_swing(a.amount))
    }

    #[tool(description = "Set or clear a per-pattern 8th-note swing override (0.0-1.0, or null to clear) on a step or piano-roll pattern.")]
    fn set_pattern_swing(&self, Parameters(a): Parameters<PatternSwingArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_pattern_swing(a.pattern_id, a.swing))
    }

    #[tool(description = "Set pitch-bend sensitivity in semitones (1..=24, default 2) on a track via GM RPN 0.")]
    fn set_track_pitch_bend_range(&self, Parameters(a): Parameters<TrackPitchBendRangeArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_track_pitch_bend_range(a.track_id, a.semitones))
    }

    #[tool(description = "Assign a multi-sampled `.sfz` instrument file to a track.")]
    fn set_track_sfz(&self, Parameters(a): Parameters<TrackSfzArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_track_sfz(a.track_id, a.path))
    }

    #[tool(description = "Set note-timing humanization 0.0-1.0 (looseness; ~32nd jitter at full) for a less mechanical feel.")]
    fn set_timing(&self, Parameters(a): Parameters<SwingArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_timing(a.amount))
    }

    #[tool(description = "Toggle legato on a piano-roll pattern, with optional overlap_ticks (0..=240) and max_gap_ticks (rest threshold beyond which legato does not bridge).")]
    fn set_legato(&self, Parameters(a): Parameters<LegatoArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_legato(a.pattern_id, a.on, a.overlap_ticks, a.max_gap_ticks))
    }

    #[tool(description = "Set MIDI CC events [{tick, cc, value}] on a piano-roll pattern (cc: 1=vibrato/mod, 11=expression, 64=sustain pedal, 74=brightness; value 0..=127).")]
    fn set_pattern_cc(&self, Parameters(a): Parameters<PatternCcArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_pattern_cc(a.pattern_id, a.events))
    }

    #[tool(description = "Set pitch bend events [{tick, cents}] on a piano-roll pattern (cents -2400..=2400 relative to track tuning_cents) for horn scoops, falls, and expressive bends.")]
    fn set_pattern_pitch_bends(&self, Parameters(a): Parameters<PatternPitchBendsArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_pattern_pitch_bends(a.pattern_id, a.events))
    }

    #[tool(description = "Set volume automation points [{tick, gain}] on a track (absolute arrangement ticks, 120 per quarter note; a bar is the project's ticks_per_bar, 480 in 4/4; gain 0.0..=2.0). Gain follows a smooth curve between points. Empty clears automation.")]
    fn set_volume_automation(&self, Parameters(a): Parameters<VolumeAutomationArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_volume_automation(a.track_id, a.points))
    }

    #[tool(description = "Add a new pattern to a track. Returns its id.")]
    fn add_pattern(&self, Parameters(a): Parameters<TrackIdArg>) -> Result<CallToolResult, McpError> {
        reply(self.tools.add_pattern(a.track_id))
    }

    #[tool(description = "Replace a step pattern's rows. Each row: pitch + a step string like 'x...x...x...x...' ('x' hit, '.' rest, '1'-'9' velocity).")]
    fn set_steps(&self, Parameters(a): Parameters<StepsArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_steps(a.pattern_id, a.rows))
    }

    #[tool(description = "Replace a piano-roll pattern's notes (120 ticks per quarter note; a bar is the project's ticks_per_bar, 480 in 4/4, 360 in 3/4).")]
    fn set_notes(&self, Parameters(a): Parameters<NotesArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_notes(a.pattern_id, a.notes))
    }

    #[tool(description = "Write a melody with mini-notation (e.g. 'c3 e3 g3 c4', '.' = rest) onto a track's first pattern. The natural way to compose melodies.")]
    fn apply_mini_notation(&self, Parameters(a): Parameters<MiniNotationArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.apply_mini_notation(a.track_id, a.source))
    }

    #[tool(description = "Place a pattern clip on the arrangement timeline at a bar (optionally stretched to len_bars, sub-bar offset_ticks, and clip gain 0.0..=2.0).")]
    fn place_clip(&self, Parameters(a): Parameters<PlaceClipArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.place_clip(a.track_id, a.pattern_id, a.bar, a.len_bars, a.offset_ticks, a.gain))
    }

    #[tool(description = "Set per-clip gain multiplier (0.0..=2.0) on a placed clip for section-level dynamics and foreground spotlighting.")]
    fn set_clip_gain(&self, Parameters(a): Parameters<ClipGainArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_clip_gain(a.track_id, a.bar, a.gain, a.offset_ticks))
    }

    #[tool(description = "Nudge a clip off the bar line: set its sub-bar start offset in ticks (120 = one quarter note; must stay below the project's ticks_per_bar, 480 in 4/4).")]
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

    #[tool(description = "Create or replace a sampler pad: map a .wav to a MIDI pitch on a drum or melodic track (a track without a sampler becomes one, replacing its SoundFont instrument). gate=true makes the sample follow note lengths — use it for 808s, pads and chops written with set_notes; leave it false for one-shot drums. Then add velocity layers with set_sampler_variants.")]
    fn set_sampler_pad(&self, Parameters(a): Parameters<SamplerPadArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_sampler_pad(a.track_id, a.pitch, a.path, a.gain, a.gate))
    }

    #[tool(description = "Replace a track's LFO modulators (movement over time). dest cutoff sweeps the track's low-pass around effects.cutoff on every track type; volume (tremolo) and pan (auto-pan) oscillate around the track's fader/pan. depth is the swing (0..1) either side of that base value; rate_hz 0.1 is a slow ~10 s breath, 4-8 a wobble. [] removes them.")]
    fn set_modulators(&self, Parameters(a): Parameters<ModulatorsArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.set_modulators(a.track_id, a.modulators))
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

    #[tool(description = "List installed CLAP and imported Flow Music plugins, including whether each is an instrument or effect.")]
    fn list_clap_plugins(&self) -> Result<CallToolResult, McpError> {
        reply(self.tools.list_clap_plugins())
    }

    #[tool(description = "Refresh this MCP process's catalog after installing a CLAP plugin or importing a Flow Music .fmx file.")]
    async fn rescan_clap_plugins(&self) -> Result<CallToolResult, McpError> {
        let tools=self.tools.clone(); blocking(move ||tools.rescan_clap_plugins()).await
    }

    #[tool(description = "Inspect a CLAP slot's parameter ids, bounds, defaults, integer/read-only flags and saved overrides. Loads a separate instance on a worker; does not change the project or read live automation/internal edits. Use these ids before set_clap_param.")]
    async fn get_clap_params(&self, Parameters(a): Parameters<ClapUidArgs>) -> Result<CallToolResult, McpError> {
        let tools=self.tools.clone(); blocking(move ||tools.get_clap_params(a.uid)).await
    }

    #[tool(description = "SOUND DESIGN 1/5: search Surge XT's free patches by name/category to find a starting point for a described sound (\"deep tom\" → Percussion, \"dusty pad\" → Pads).")]
    async fn find_patches(&self, Parameters(a): Parameters<FindPatchesArgs>) -> Result<CallToolResult, McpError> {
        let tools=self.tools.clone(); blocking(move ||tools.find_patches(a.query,a.category,a.limit)).await
    }

    #[tool(description = "SOUND DESIGN 2/5: load a Surge XT patch onto its own new track (named after the sound), or onto a named track (replace=true needed if it plays something other than Surge XT). Live mode: heard in the GUI at once.")]
    async fn load_patch(&self, Parameters(a): Parameters<LoadPatchArgs>) -> Result<CallToolResult, McpError> {
        let tools=self.tools.clone(); blocking(move ||tools.load_patch(a.patch_path,a.track_id,a.name,a.replace)).await
    }

    #[tool(description = "SOUND DESIGN 3/5: the sound's musical controls (oscillators, filter, envelopes, LFO, drive, voice) with current values in readable units and a hint for each.")]
    async fn describe_sound(&self, Parameters(a): Parameters<ClapUidArgs>) -> Result<CallToolResult, McpError> {
        let tools=self.tools.clone(); blocking(move ||tools.describe_sound(a.uid)).await
    }

    #[tool(description = "SOUND DESIGN 4/5: set musical controls, e.g. {\"filter_cutoff\": \"600 Hz\", \"amp_decay\": \"180 ms\", \"drive\": 0.3}. All are checked before anything is saved. Live mode: heard in the GUI at once.")]
    async fn shape_sound(&self, Parameters(a): Parameters<ShapeSoundArgs>) -> Result<CallToolResult, McpError> {
        let tools=self.tools.clone(); blocking(move ||tools.shape_sound(a.uid,a.controls)).await
    }

    #[tool(description = "SOUND DESIGN 5/5: play a short phrase on the sound in the song's key — in the open GUI in live mode — and report its measured traits (pitch vs intended note, attack, decay, sustain, brightness, noisiness) plus a WAV path. The musician listens and judges; use the numbers to steer the next shape_sound.")]
    async fn audition_sound(&self, Parameters(a): Parameters<AuditionArgs>) -> Result<CallToolResult, McpError> {
        let kind = match phrase_kind(a.kind) { Ok(k) => k, Err(e) => return reply(Err(e)) };
        let tools=self.tools.clone(); blocking(move ||tools.audition_sound(a.uid, kind, audition_notes(a.notes), a.octave)).await
    }

    #[tool(description = "Print a designed sound to audio: render a phrase (or the track's part over bars) to a WAV beside the project and place it as a clip on a new audio track \"<name> (print)\" — to keep the sound raw.")]
    async fn print_sound(&self, Parameters(a): Parameters<PrintArgs>) -> Result<CallToolResult, McpError> {
        let kind = match phrase_kind(a.kind) { Ok(k) => k, Err(e) => return reply(Err(e)) };
        let tools=self.tools.clone(); blocking(move ||tools.print_sound(a.uid, audition_notes(a.notes), kind, a.start_bar, a.end_bar, a.at_bar)).await
    }

    #[tool(description = "List the versions of a designed sound made in this session (load, each shape, reverts).")]
    fn sound_history(&self, Parameters(a): Parameters<ClapUidArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.sound_history(a.uid))
    }

    #[tool(description = "Return a designed sound to an earlier version from sound_history.")]
    fn revert_sound(&self, Parameters(a): Parameters<RevertSoundArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.revert_sound(a.uid, a.version))
    }

    #[tool(description = "Append a CLAP or imported Flow Music effect to an audio or instrument track's insert chain. Use the plugin ID from list_clap_plugins.")]
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

    #[tool(description = "ANALYZE MIX: solo-render each track over its active bars, report spectral placement and overlapping masking pairs, and optionally compute per-bar 500-6000 Hz Lead-to-Accompaniment Ratio (LAR) for lead_track_id.")]
    async fn analyze_mix(&self, Parameters(a): Parameters<AnalyzeMixArgs>) -> Result<CallToolResult, McpError> {
        let tools = self.tools.clone();
        blocking(move || tools.analyze_mix(a.lead_track_id, a.start_bar, a.end_bar)).await
    }

    #[tool(description = "ANALYZE MELODY: audit a track's arranged melody for pitch contour, rhythmic diversity, breathing rests, motivic recurrence, and accompaniment range overlaps before or after orchestrating.")]
    fn analyze_melody(&self, Parameters(a): Parameters<AnalyzeMelodyArgs>) -> Result<CallToolResult, McpError> {
        reply(self.tools.analyze_melody(a.track_id, a.start_bar, a.end_bar))
    }

    #[tool(description = "ANALYZE ARRANGEMENT: report the song's density (tracks active per bar) and dynamic arc as sparklines, and flag flat/static textures or a missing intro build. Use to check the piece has shape — a build, variation, sections — not just a constant wall of sound.")]
    async fn analyze_arrangement(&self) -> Result<CallToolResult, McpError> {
        let tools = self.tools.clone();
        blocking(move || tools.analyze_arrangement()).await
    }

    #[tool(description = "Summarize piano-roll note placement against the 16th grid and report swing, humanize, and timing settings. Use compare_groove to check how two sources line up in the arrangement.")]
    fn analyze_groove(&self) -> Result<CallToolResult, McpError> {
        reply(self.tools.analyze_groove())
    }

    #[tool(description = "Compare two selected sources' scheduled note onsets over 1–32 arrangement bars. Optional MIDI pitch filters isolate hits such as kick. Reports exact matches, signed offsets from nearby reference hits, and counts per bar. Use start_listen with dry=true to investigate audible attacks and echoes.")]
    async fn compare_groove(
        &self,
        Parameters(args): Parameters<daw_mcp::groove_compare::GrooveCompareArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tools = self.tools.clone();
        blocking(move || {
            let project = tools.store.load()?;
            let report = daw_mcp::groove_compare::compare(&project, args).map_err(|e| e.to_string())?;
            serde_json::to_string(&report).map_err(|e| e.to_string())
        }).await
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

    #[tool(description = "Check tracks one by one over the same arrangement range. Reports saved mute state, isolated post-master peak and RMS, stereo correlation, spectrum, per-bar clip occupancy, quiet bars, adjacent audible level changes, and track-specific render errors. Bars are zero-based.")]
    async fn check_tracks(&self, Parameters(a): Parameters<CheckTracksArgs>) -> Result<CallToolResult, McpError> {
        let tools = self.tools.clone();
        blocking(move || tools.check_tracks(a.track_ids, a.start_bar, a.end_bar)).await
    }

    #[tool(description = "Audition one track as a short stereo WAV and get measurements for the same excerpt. Uses the saved automation, effects, and sends. Can audition a muted track without changing its saved mute or the live transport.")]
    async fn listen_track(&self, Parameters(a): Parameters<ListenTrackArgs>) -> Result<CallToolResult, McpError> {
        let tools = self.tools.clone();
        match blocking_result(move || tools.listen_track(a.track_id, a.start_bar, a.duration_seconds)).await {
            Ok((report, wav)) => {
                use base64::Engine;
                let audio = ContentBlock::audio(base64::engine::general_purpose::STANDARD.encode(wav), "audio/wav");
                Ok(CallToolResult::success(vec![ContentBlock::text(report), audio]))
            }
            Err(error) => reply(Err(error)),
        }
    }

    #[tool(description = "Render a bar range and return a numerical report: peak/RMS dBFS, clipping, per-bar loudness, stereo width, and spectral balance. tracks=[…] solos a subset. For audio use a completed start_listen job and audition_excerpt.")]
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
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let bound;
        let server = if let Some(automatic) = &self.automatic_session {
            let automatic = automatic.clone();
            let inspect = matches!(
                request.name.as_ref(),
                "session_state" | "project_summary" | "get_project"
            );
            let resolved = tokio::task::spawn_blocking(move || automatic.tools(inspect))
                .await
                .map_err(|error| format!("Live DAW discovery failed: {error}"))
                .and_then(|result| result);
            let tools = match resolved {
                Ok(tools) => tools,
                Err(error) => return reply(Err(error)).map(Into::into),
            };
            // Pin all reads and writes in this call to the same socket.
            bound = Self {
                tools,
                automatic_session: None,
                tool_router: self.tool_router.clone(),
            };
            &bound
        } else {
            self
        };
        self.tool_router.call(ToolCallContext::new(server, request, context)).await
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions(
                "daw-ai project server. Start with project_summary; in live mode also inspect session_state. \
                 Read track comments as musical context for the requested work. Comments can be scoped to the whole track or to a specific bar. \
                 Follow the Two-Stave Melody Gate before orchestrating: write a lead melody + chord reduction first with rests, varied durations, motif development, and expressive CC/legato (set_pattern_cc, set_legato), then verify it with analyze_melody and listen. \
                 Keep one foreground protagonist clear per section using set_volume_automation, set_clip_gain, and analyze_mix(lead_track_id=...) rather than static track volumes alone. \
                 Use set_notes for detailed performances, set_sampler_pad to build sampler kits (gate=true for sounds that should follow note length), set_sampler_variants for velocity-layered or alternate takes, and set_modulators for filter/volume/pan movement. \
                 Use check_tracks to find quiet bars, level transitions and render errors per track, then listen_track for an isolated stereo excerpt. Use composition_guide for the composition loop. Write the motif and its answer first, then render comparable revisions with start_listen. Use audition_excerpt for actual audio if the client supports it, and prepare_audition to ask the musician one focused question with loudness-matched A/B clips. Treat numeric diagnostics as technical measurements and record only real listener feedback. \
                 Live mode shares revision-checked edits and undo with the musician; inspect engine application status. Offline mode edits files only. \
                 Designing sounds from words (Surge XT): find_patches → load_patch → describe_sound → shape_sound → audition_sound, then let the musician steer; the musician judges by ear."
                    .to_string(),
            )
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let first = args.next().ok_or_else(|| anyhow::anyhow!("usage: daw-mcp <project> | --live | --list-sessions | --session <socket>"))?;
    if first == "--list-sessions" {
        println!("{}", serde_json::to_string_pretty(&daw_io::session::discover()?)?);
        return Ok(());
    }
    let server = match first.as_str() {
        "--live" => DawServer::new(DawTools::new("live.daw.json".into()), true),
        "--session" => {
            let socket = args.next().ok_or_else(|| anyhow::anyhow!("--session needs a socket path"))?;
            let store = ProjectStore::live(socket.into()).map_err(anyhow::Error::msg)?;
            DawServer::new(DawTools::with_store(store), false)
        }
        _ => DawServer::new(
            DawTools::new(daw_core::project::format::absolute(std::path::Path::new(&first))?),
            false,
        ),
    };
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

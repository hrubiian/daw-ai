# Local collaboration and reliable rendering

Run the GUI or TUI normally. Each process publishes one local Unix socket. Discover and explicitly select the musician's session:

```sh
cargo build --workspace --locked
target/debug/daw-mcp --list-sessions
target/debug/daw-mcp --session /path/from/discovery.sock
```

The MCP process speaks stdio MCP after selection. A lost connection returns an error; it never switches to editing a file. Existing `daw-mcp song.daw.json` remains an offline workflow. Socket access is restricted to the current user; the protocol is Linux-first, version 1, newline-delimited JSON with a 16 MiB request limit and connection timeouts.

## Edits and playback

Call `session_state` for the project, revision, transport and engine diagnostics. `session_command` accepts a JSON string containing a request:

```json
{
  "version": 1,
  "operation_id": "arrangement-pass-001",
  "expected_revision": 0,
  "command": {
    "type": "batch",
    "label": "Agent: quieter bass and slower tempo",
    "edits": [
      {"type": "tempo", "bpm": 110},
      {"type": "track", "id": "t2", "volume": 0.65}
    ]
  }
}
```

Read the actual revision and track IDs first. A batch validates completely before committing and creates one shared undo step. Stale revisions return `revision_conflict` without modifying the session. Refresh and reconsider the edit; do not blindly replay a replacement project. Reusing an operation ID with the same request is idempotent while it remains in the 128-entry retry cache. Use a fresh ID for a revised request.

Use `preview` instead of `batch` to receive a validated proposed project without changing the project or undo history. After the musician reviews it, submit a batch against the same revision; an intervening edit requires a refreshed preview. Preview is optional and does not itself authorize destructive actions.

Commands also include `undo`, `redo`, `play`, `pause`, `stop`, and `seek` with a tick value. One bar is 480 ticks in supported 4/4 projects. GUI Ctrl+Z/Ctrl+Shift+Z and TUI Ctrl+Z/Ctrl+Y access the same session history. Opening another project stores its asset directory with undo history.

An accepted edit can still be `pending`: while playing, project swaps occur at the next bar. `applied_revision` advances only when the audio callback consumes the acknowledgement. Pausing flushes a pending edit. `applied` means the engine consumed the update, not that a listener necessarily heard it. Initialization, asset, plugin, stream and queue failures are reported separately. The GUI status includes the current revision and latest edit. “Sync session” (TUI Ctrl+U) preserves differing local work in a uniquely named recovery project before refreshing from the authoritative session.

## Files and recovery

Loading, session batches and saves share semantic validation. Invalid IDs, references, unsupported versions/meters, note ranges and parameter values return errors. Saves use unique temporary files, file/directory synchronization, an atomic rename, a `.bak` containing the previous file and a persistent `.lock` sidecar. Application-controlled writers compare the exact loaded file revision under that lock; a stale save retains local work and reports a conflict.

External editors that ignore the advisory lock cannot participate in the atomic compare-and-swap protocol. Changes detected before a save are rejected. JAM imports reject conflicts with unsaved work; use Save As to retain local work before explicitly loading the external file. Backups are recoverable by opening the `.bak` file. Do not delete a `.lock` while any editor is running.

Save As resolves asset paths before moving to the new project directory. Relative asset paths resolve against the project directory, with a repository-relative fallback for legacy example files. The launch project path is made absolute before changing the working directory.

## Listening jobs and experiments

`start_listen` captures a project snapshot and returns a job ID. Optional `start_bar`/`end_bar` are zero-based, end-exclusive; optional `tracks` selects a subset. `job_status` reports progress, completion or failure. `cancel_job` requests cooperative cancellation. Two background jobs may run simultaneously; the default timeout is five minutes and render warm-up is bounded to thirty minutes of timeline. Native plugin calls must return before cancellation or timeout can be observed.

Range rendering warms the mixer from the song start, discards the prefix and stops after the requested endpoint. This preserves instruments and effects while avoiding storage and rendering of the unused suffix. It is not constant-time seeking. A regression compares a non-silent range, including reverb, against a full-render slice.

Completed jobs write unique `renders/<job-id>/` directories containing `audio.wav`, `project.json`, and `report.json`. Reports include a project hash, live revision when available, engine package version, sample rate, normalization policy, warm-up policy, asset/plugin hashes and structured measurements. WAVs are unnormalized for listening. Selected-track renders hash the effective rendered snapshot, including its mute mask.

`compare_renders` returns two completed reports and audio artifacts for A/B listening; settings must match. `rate_render` stores the musician's explicit 1–5 rating and comment as a separate event alongside an artifact. Do not infer ratings from measurements. Masking, groove and arrangement measurements are diagnostics, not musical-quality scores.

The job manager retains up to 32 reports in memory; artifacts and ratings remain on disk. `render_history` lists saved reports from the current project directory across restarts (default 20 entries, maximum 100, lexicographic ID order; use `next_offset` for the next page). `job_status`, `compare_renders`, and `rate_render` also read these durable reports. Artifact paths are rebased when the project directory moves. Corrupt entries are reported individually. Running and terminal states are atomically checkpointed, including failed/cancelled jobs. A saved running job not owned by this MCP process is shown as `unavailable`: it may still be running elsewhere or have been interrupted; inspection does not change its report. Cancellation only controls jobs owned by the current process. GUI export is limited to one background job; invoking either Export WAV or Export MP3 again requests cancellation. TUI Ctrl+E cancels a running background export. GUI exports write unique directories with project and provenance files. MP3 export uses FFmpeg/libmp3lame at 320 kbps, including full-song effect tails; see [MP3 export](mp3-export.md). Legacy synchronous tools remain available and run on bounded blocking workers; cancellation is exposed through the new background job API.

## Verification and remaining qualification

```sh
bash scripts/check.sh
DAW_REQUIRE_TEST_ASSETS=1 cargo test --workspace --locked
cargo run -p daw-core --example session_bench --release
```

The CI matrix includes ordinary Linux builds and a required-soundfont job, builds CLAP fixtures, exercises the MCP wire protocol and runs GPUI headless regressions. Hardware playback tests remain explicit/ignored. The GUI displays package version, git revision, dirty marker and build timestamp; `install-desktop.sh` builds a locked release before installing.

Callback work drains at most 1,024 events per block. Replaced strip and audio-buffer collections are retired through a bounded queue on a worker. Diagnostics expose queue drops, late scheduled events, maximum callback time, callback overruns and stream errors. Counters are cumulative for the process lifetime. This is not a complete realtime certification: voice growth, plugin event/storage behavior and legacy recording allocation still require further audit. Sequenced events now carry device-frame timestamps through the relay. The producer prepares two callback buffers plus 10 ms ahead; the audio callback splits rendering at event positions. The UI playhead and transport/gate publications are scheduled alongside audio. Pause/seek invalidate old performance events while preserving project updates and their acknowledgements. Both event intake and scheduled dispatch are bounded to 1,024 events per callback; future events use a preallocated 4,096-entry queue and scheduling envelopes retire off-thread. Overload can still cause late delivery and is counted. Direct MIDI/preview input retains immediate callback delivery. Hardware recording alignment and whole-mix live/offline sample identity remain unqualified; controlled sampler timing tests pass across buffer sizes.

Still outstanding from the larger roadmap: a richer graphical merge/approval workflow, in-app experiment browsing and A/B switching, complete background-job migration of legacy analysis, deeper callback allocation tests and controlled live/offline equivalence tests. Automation lanes and richer MIDI expression remain contingent on observed collaboration needs. Cloud collaboration, broad platform support, plugin sandboxing and time-stretch remain deferred as planned.

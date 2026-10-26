# daw-ai

An LLM-native DAW built in Rust (formerly daw-cli). Open-source, keyboard-driven, LLM-friendly.

## Architecture

Cargo workspace: `crates/daw-core` (audio + sequencer + project format, no TUI deps) and `crates/daw-tui` (ratatui frontend).

**Do not add TUI or I/O dependencies to `daw-core`.** The compiler enforces this.

## Running

Download `assets/GeneralUser.sf2` (free GM soundfont) before first run.

```bash
cargo run                          # new project (TUI)
cargo run -- my-song.daw.json     # open existing (TUI)
cargo run -- --gui                 # GPUI desktop frontend
./run-gui.sh                       # GPUI under XWayland (resizable, decorated window on GNOME/Wayland)
```

On GNOME/Wayland a native `--gui` window is borderless and non-resizable (gpui 0.2.2
server-decoration limitation); `./run-gui.sh` runs it under XWayland for a normal
resizable window. It passes args through (`./run-gui.sh my-song.daw.json`).

### MCP server

`cargo run -p daw-mcp -- my-song.daw.json` starts a stdio MCP server over one
project file (granular edit tools + `listen` rendering/analysis). `.mcp.json`
registers it for Claude Code sessions in this repo; combine with the GUI's
JAM mode for live round-trips.

## Testing

```bash
cargo test --workspace            # unit tests
cargo test -p daw-core smoke_plays_note -- --ignored --nocapture  # audio smoke test (needs speakers)
```

## Key files

- `crates/daw-core/src/project/mod.rs` — all data types (Project, Track, Pattern, Note)
- `crates/daw-core/src/audio/engine.rs` — cpal + rustysynth audio engine
- `crates/daw-core/src/sequencer/engine.rs` — BPM clock, pattern → AudioEvent
- `crates/daw-tui/src/app.rs` — App state, Tab enum
- `crates/daw-tui/src/views/` — one file per tab

## Project file format

Projects are `.daw.json` files. Fully human- and LLM-editable. See `docs/project-format.md` for the schema.

## Keybindings

| Key | Action |
|-----|--------|
| `1`–`5` | Switch tab |
| `Space` | Play / Pause |
| `+` / `-` | BPM up / down |
| `Ctrl+S` | Save project |
| `q` | Quit (auto-saves) |


## Shared sessions and verification

Use `target/debug/daw-mcp --list-sessions`, then explicitly select a running
session with `--session <socket>`. Read the current revision before edits;
validated batches are one shared undo step. A failed live connection must not
silently fall back to file editing. File-based MCP remains available offline.
See [Live collaboration](docs/live-collaboration.md) for the protocol and limits.

Run `bash scripts/check.sh` for the workspace build, MCP wire checks, tests and
Clippy. Set `DAW_REQUIRE_TEST_ASSETS=1` for soundfont-required checks. FFmpeg with
libmp3lame is required for MP3 tests/export. Diagnostics do not establish musical
quality; use the musician's own listening feedback and ratings.

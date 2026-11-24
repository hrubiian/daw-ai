# daw-ai

**The LLM-native DAW.** A full digital audio workstation in Rust — desktop
GUI, terminal UI, and an MCP server that lets AI agents compose, mix, and
*hear* your project. Linux-first.

![Rust](https://img.shields.io/badge/rust-2021-orange)
![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)
![Plugins](https://img.shields.io/badge/plugins-CLAP-purple)
![AI](https://img.shields.io/badge/agents-MCP-green)

![First Light in the dark theme with the beat ruler, mixer, and piano roll visible](assets/screenshots/desktop-workspace.png)

## Why this exists

- **The project file is the API.** Songs are human-readable `.daw.json`
  ([format docs](docs/project-format.md)); a local shared session lets a musician
  and an LLM edit with revision checks and shared undo. File-based MCP and conflict-checked JAM import remain
  available for offline work.
- **Agents can audition revisions.** The MCP server exposes composition,
  mixing, and render tools. `listen` reports signal measurements, and
  `audition_excerpt` delivers short audio clips to supporting MCP clients.
  `prepare_audition` makes matched A/B clips for the musician. See the
  [composition workflow](docs/music-collaboration.md).
- **It's a real DAW.** Per-track mixer with insert FX, reverb/delay send
  buses and live meters; CLAP plugin hosting (instruments and effects);
  transport-synced audio recording with count-in and loop/punch takes;
  a piano roll with multi-bar legato; WAV and MP3 export through the shared
  mixer. Scheduling, device/plugin settings and normalization can affect
  live/offline results.

## Quickstart

Building on Linux needs a recent stable Rust and a few system libraries
(Debian/Ubuntu names):

```bash
sudo apt-get install -y libasound2-dev libxkbcommon-x11-dev libwayland-dev \
  libvulkan-dev libxcb1-dev libfontconfig1-dev libssl-dev pkg-config fluid-soundfont-gm
```

From the repository, open **First Light**, an eight-bar demo that uses only
the default General MIDI SoundFont:

```bash
./fetch-assets.sh --quickstart
cargo run --release --locked -- --gui examples/quickstart.daw.json
```

The setup links the distribution's FluidR3 GM SoundFont to `assets/GeneralUser.sf2`
if needed. On other Linux distributions, put a General MIDI `.sf2` file at that
location. Press **Space** to play the demo.

If you hear silence, run `cargo run --release --locked -- doctor examples/quickstart.daw.json`
to check the project, SoundFont, output configuration, MIDI inputs and CLAP paths.
Use `doctor --headless` to skip hardware checks. For a render without audio hardware,
run `cargo run --release --locked -- examples/quickstart.daw.json --export-wav first-light.wav`.

`ffmpeg` is optional (needed for MP3 export); Surge XT is optional
(sound design from words through the MCP server).

- GNOME/Wayland: use `./run-gui.sh` for a normal resizable window
  on systems that need XWayland. Native Wayland also has client window controls.
- Terminal UI: `cargo run --release --locked -- examples/quickstart.daw.json` ([TUI guide](docs/tui.md)).
- See [Examples](examples/README.md) for the longer, sample-based MCP demo.

## Desktop workspace

Click the bar or beat ruler to place the playhead exactly where playback should
start. Use the **Timeline** controls to scroll to the start or end of the song,
or move the view four bars at a time. Hold **Ctrl** while scrolling to zoom around
the pointer.

Open **Mixer** to show channel strips below the arrangement. Edit notes in the
**Piano Roll** or **Sequencer**, and adjust **Swing**, **Humanize**, and **Timing**
in **FX**. Drag the panel dividers to resize them, or choose **Arrange ⛶** for more
timeline space.

Use the microphone and keyboard icons beside **Amp setup** to configure your
audio input and MIDI controller. Switch between **Dark** and **White** in
**Settings → Appearance**.

Hover over **Browse instruments…** or press **Ctrl+K** to search SoundFont and
CLAP instruments for the selected track.

![The instrument browser open over First Light in the white theme](assets/screenshots/instrument-browser.png)

### Import a Flow Music plugin

For an instrument, open **Browse instruments… → Import plugin…** and pick a
Google Flow Music `.fmx` export. Find it under **Flow Music** and click it twice
to confirm replacing the selected melodic track's instrument.

For an effect, select the track and use **Audio Effects → Add effect → Import plugin…**.
Click **Add** next to the imported effect to insert it. Effects process audio
clips and instrument tracks during playback and export.

To hear a microphone through those effects live, choose its device and channel
under **In: …** on an audio track, then click **Monitor** on the same track.
Monitoring also arms the track. **Rec** starts the take using the chosen recording
source. Click **Monitor** again to turn it off while keeping the track armed.

Choose **Open editor** in the **FX** pane to open the original panel in a new
browser window. Parameter changes are saved with the project. The browser editor
uses Python 3 and a browser with new-window support, such as Chrome or Firefox.

Imported plugins are available to future projects from the local library
at `~/.local/share/daw-ai/flow-plugins` (or under `$XDG_DATA_HOME` when set).
You can also install an instrument with

```bash
cargo run -- --import-plugin /path/to/plugin.fmx
```

The importer supports compatible Flow Music ABI 1 instruments and stereo effects.
Native VST2/VST3 binaries require separate host support.

## Compose with Claude

```bash
cargo build -p daw-mcp     # .mcp.json already registers it for Claude Code
```

For **First Light**, open the quickstart project in the GUI and use
**Agent Session → Copy MCP configuration** to
[set up automatic live connections](docs/live-collaboration.md). Then ask your
agent:

> *"Write a 124 bpm house groove, 8 bars. Then listen to bars 0–8 and fix
> whatever's wrong with the mix."*

In a live session, the agent and GUI share revisions and undo. `auto_balance`
levels the tracks by measured loudness, and `listen` renders the result and
reports levels and spectral balance. In file mode, JAM can follow changes to
the same file.

## Features

| Area | Today |
|---|---|
| **Sequencing** | 16th-note step grid with per-step velocity, piano roll with drag-stretch legato across bars, Strudel-style mini-notation REPL, arrangement timeline with clip content previews, loop region |
| **Mixing** | Docked mixer with channel strips, EQ/comp/delay/drive/filter, shaped reverb, synced stereo echoes, sends, live meters, master limiter and auto-balance |
| **Recording** | Transport-synced takes with count-in, punch-in/out, loop-cycle takes (one WAV per pass), MIDI recording into patterns, input metering with clip latch |
| **Plugins** | CLAP hosting: instruments and insert effects, plugin discovery, generic param editing, state saved in the project |
| **AI** | MCP server (~30 tools), JSON project format, JAM hot-reload, listening/analysis tools, auto-balance |
| **Guitar** | Low-latency live amp rig (gate, boost, amp, modeled cabinets with A/B blend, pedals), custom cabinet IRs, presets, recording dry or through the rig, optional AmpForge CLAP backend ([guide](docs/guitar-amp.md)) |
| **I/O** | GM soundfonts (per-track `.sf2`), velocity-layered samplers with alternate takes, audio clips, MIDI input, WAV and MP3 export |

## Architecture

| Crate | Role |
|---|---|
| `daw-core` | Engine, mixer graph, sequencer, project format, analysis — no UI dependencies |
| `daw-io` | Host glue: audio/MIDI device handling, capture, guitar monitor, user config, the engine relay |
| `daw-gpui` | Desktop GUI (gpui) |
| `daw-tui` | Terminal UI (ratatui) |
| `daw-mcp` | MCP server over a selected live session or project file (rmcp, stdio) |

Every feature was built spec-first — designed, planned, then implemented
and verified one slice at a time.

## Docs

- [Terminal UI guide & keybindings](docs/tui.md)
- [The `.daw.json` project format](docs/project-format.md)
- [Desktop UI and Mixing](docs/desktop-ui.md)
- [Live collaboration, safe saves and background rendering](docs/live-collaboration.md)
- [Creative effects and CLAP controls](docs/creative-effects.md)
- [MP3 export](docs/mp3-export.md)
- [Listening and analysis tools](docs/analysis-and-ci.md)
- [Musical collaboration workflow](docs/music-collaboration.md)
- [Guitar amp and low-latency monitoring](docs/guitar-amp.md)
- [Audio engine audit](docs/audio-engine-audit.md) · [Desktop UI review](docs/ui-review.md)
- [CLAUDE.md](CLAUDE.md) — working on the codebase (with an AI or without)

## License

MIT OR Apache-2.0, at your option. The GeneralUser GS soundfont and
fetched samples carry their own free licenses — see `fetch-assets.sh`.

---

## About this project

daw-ai is an experimental research project. It serves as a test bench for
measuring how well current state-of-the-art language models can compose,
arrange and mix music when given a real audio engine and an audio feedback
loop (the `listen` tool).

Contact: nazariihrubiian@gmail.com

## Verification

Run `bash scripts/check.sh` for the Linux build, MCP protocol checks, workspace
tests and Clippy. Set `DAW_REQUIRE_TEST_ASSETS=1` when the test soundfont is
installed. After building, `python3 scripts/test-quickstart.py` runs the clean
demo smoke test with the system FluidR3 GM package and FFmpeg. MP3 integration
tests require FFmpeg with libmp3lame. Hardware tests
remain explicit; headless checks do not certify every audio device or plugin.
See [CLAUDE.md](CLAUDE.md) for the crate layout and conventions. Issues and
pull requests are welcome.

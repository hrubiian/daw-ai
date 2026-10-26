# daw-gpui

**Mixing** is a dedicated workspace tab, also opened by the transport's Mixing
button. Channel strips show names/instruments, level in dB, pan, mute/solo,
reverb/echo sends and live peak meters. Use +/− or the wheel over a level bar;
Shift makes fine changes. Effects… opens that track's existing effects panel.
Reverb/Echo returns and the post-limiter master meter are included. Scroll
sideways for more tracks and vertically in short windows; the master stays
visible, and wide windows also pin the shared returns. Tabs do not reset playback.

**Audio Effects → Track / Reverb / Echo** now exposes DSP inserts on every
track, tempo-synced echo tone/width/modulation, reverb pre-delay/tone, spatial
presets and installed CLAP effect controls. Shift+click for fine increments.
See [Creative effects](../../docs/creative-effects.md) for plugin setup and limits.

GPUI desktop frontend for daw-ai. Reuses `daw-core` (audio/sequencer/format) and
`daw-io` (MIDI + engine `Host`) exactly as the TUI does.

**File → Export MP3** saves the whole song as a stereo 320 kbps MP3, including
effect tails, in the project's `renders/<unique-id>/export.mp3`. Export runs in
the background; choose Export again to cancel. FFmpeg with libmp3lame is required.
WAV export is still available. See [MP3 export](../../docs/mp3-export.md).

Run it:

```bash
cargo run -- --gui                 # new project
cargo run -- --gui my-song.daw.json
```

(`cargo run` with no `--gui` still launches the ratatui TUI.)

## System dependencies (Linux)

GPUI links against system graphics/input libraries. On Debian/Ubuntu you need the
**dev** packages (the unversioned `.so` symlinks), not just the runtime libs:

```bash
sudo apt install libxkbcommon-x11-dev libxkbcommon-dev \
                 libwayland-dev libvulkan-dev vulkan-tools \
                 libxcb1-dev
```

The linker error `unable to find library -lxkbcommon-x11` means
`libxkbcommon-x11-dev` is missing (its `libxkbcommon-x11.so` symlink is what the
linker resolves). A Vulkan-capable GPU/driver and a running X11 or Wayland session
are required to actually open a window.

## Interface

The desktop frontend includes an arrangement timeline, instrument browser,
resizable editors, a Mixing workspace, step sequencer, piano roll, effects, recording,
and project import/export.

- Track headers group name, mute/solo and track actions above volume, pan and
  instrument controls. Hover controls for help; double-click a name to rename.
- Volume is shown as a percentage; pan reads Center, L or R with an amount.
- Scroll the timeline with the wheel, zoom with Ctrl+wheel, and scroll tracks
  with Shift+wheel. Scrolling over track headers also scrolls tracks vertically.
- A green playhead crosses the ruler and visible track lanes at the current
  playback position, including positions between bars. It follows zoom, scroll,
  seeks and loop returns, and stays at the paused position.
- Transport and song toolbars support horizontal scrolling when they overflow.
- Search instruments by name or category; Esc clears the search.
- Ctrl+Z undoes project edits; Ctrl+Shift+Z or Ctrl+Y redoes them.

See [Desktop UI](../../docs/desktop-ui.md) for mixing, navigation and verification.

## Window and panel behavior

- Browser toggles its side dock; Mixing opens the dedicated mixing tab.
  Side docks collapse automatically
  when necessary to leave at least 200 px for the timeline; widening the window
  restores requested docks. The editor dock height is bounded after resizing.
- Browser lists, mixer channels, pattern tabs, Settings, effects and sequencer
  grids scroll when their content exceeds the available space.
- The piano roll uses sixteenth-note columns and displays notes between grid
  lines. Clicking a note removes its original onset; dragging preserves that
  onset while changing duration. Long patterns render only nearby columns.
- Click the workspace to return keyboard focus from a text field. Enter/Escape
  ends inline rename and restores transport shortcuts. Ctrl+S also works while
  an input field is focused; unnamed projects open Save As.
- Unsaved projects show an indicator and offer Save, Discard or Cancel on close.
  Cancelling Save As or encountering a save conflict keeps the window open.
- Audio/MIDI input discovery and waveform extraction run in background workers.

Run `cargo test -p daw-gpui --lib --offline` for device-free whole-view and
interaction regressions. Native window movement and driver behavior still need
acceptance checks on the target desktop.

Sampler dynamics: in the FX tab, use +Soft/+Hard to load alternate takes for an
existing pad (velocities 1–63 / 64–127). Repeated hits rotate takes within their
layer. Shift+click a step or piano-roll note for a stronger accent; Alt+click for
a softer note. Hover a note to inspect velocity. Onset and duration are retained.

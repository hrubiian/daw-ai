# daw-gpui

GPUI desktop frontend for daw-ai. Reuses `daw-core` (audio/sequencer/format) and
`daw-io` (MIDI + engine `Host`) exactly as the TUI does.

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

## Status

Phase 0 (this crate's current state): graphite window, transport bar, live playhead
counter mirrored from the engine, working Play/Pause. See
`docs/superpowers/specs/2026-05-31-daw-gpui-interface-design.md` for the full plan and
`docs/superpowers/plans/2026-05-31-daw-gpui-phase0.md` for the task breakdown.

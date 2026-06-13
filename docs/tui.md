# Terminal UI guide

The ratatui frontend (`cargo run --release`) — tabs, keybindings, and
live-performance features. The desktop GUI shares the same engine and
project files.

## Keybindings

> `^` means `Ctrl`.

### Global

| Key | Action |
|-----|--------|
| `1` – `9` | Switch tab |
| `Space` / `p` | Play / Pause |
| `0` | **Panic** — silence all synth voices immediately |
| `l` | Toggle **live piano mode** (QWERTY → MIDI notes) |
| `+` / `-` | BPM +1 / -1 |
| `[` / `]` | Transpose all notes −1 / +1 semitone |
| `{` / `}` | Swing −5% / +5% |
| `Shift+H` | Cycle **velocity humanization** 0 → 25 → 50 → 75 → 100% |
| `Shift+J` | Toggle **JAM mode** (live hot-reload of the project file) |
| `^N` | Toggle **metronome** on / off |
| `^S` | Save project (prompts for filename if unsaved) |
| `^Shift+S` | Save As — always prompt for filename |
| `^E` | Export mix to WAV (prompts for output filename) |
| `^R` | Start / stop audio recording on selected track |
| `^Q` | Quit (auto-saves) |

### Tracks tab (`1`)

| Key | Action |
|-----|--------|
| `↑` / `↓` | Select track |
| `t` | Toggle track type: Drum ↔ Melodic |
| `m` | Mute / unmute selected track |
| `r` | Rename selected track |
| `a` | Add melodic track |
| `^A` | Add audio track |
| `^D` | Delete selected track |

The help bar shows the current **metronome state** (`ON` / `OFF`) — toggle with `^N`.

### Step Sequencer tab (`2`)

| Key | Action |
|-----|--------|
| `↑` / `↓` | Select row |
| `←` / `→` | Move step cursor |
| `Space` / `Enter` | Toggle step on/off |
| `<` / `>` | Selected step velocity −/+ (graded glyph `· ░ ▒ ▓ █`) |
| `u` / `U` | **Euclidean fill** — spread one more / one fewer pulse evenly across the row |
| `(` / `)` | Pattern **gate** (note length) shorter / longer |
| `,` / `.` | Row pitch −1 / +1 (GM drum name shown inline) |
| `r` | Add row |
| `e` | Extend pattern by 16 steps |
| `w` | Shrink pattern by 16 steps (minimum 16) |

### Piano Roll tab (`3`)

| Key | Action |
|-----|--------|
| `↑` / `↓` | Move pitch cursor |
| `←` / `→` | Move time cursor (120 ticks = one 16th note) |
| `Enter` | Add note at cursor |
| `Del` / `Backspace` | Remove note at cursor |
| `^R` | Toggle **real-time MIDI record** (records while sequencer plays) |

### Mixer tab (`4`)

| Key | Action |
|-----|--------|
| `←` / `→` | Select track |
| `↑` / `↓` | Volume +5% / −5% |
| `p` | Cycle pan: left → center → right |
| `m` | Toggle mute |

### Browser tab (`5`)

| Key | Action |
|-----|--------|
| `←` / `→` | Switch instrument category |
| `↑` / `↓` | Scroll presets within category |
| `Enter` | Load preset into active track |

### FX tab (`6`)

| Key | Action |
|-----|--------|
| `↑` / `↓` | Select parameter |
| `←` / `→` | Decrease / increase value |
| `m` / `x` | Add / remove an LFO modulator on the active track |
| `Tab` | Select next modulator |
| `d` / `s` | Cycle modulator destination / shape |
| `,` / `.` | Modulator rate −/+ (Hz) |
| `<` / `>` | Modulator depth −/+ |

**MIDI / Drum tracks:** Reverb, Chorus, Cutoff, Resonance, LFO Depth — sent as MIDI CC to the synthesizer.

**Modulators (all track types):** a per-track LFO matrix. Each modulator drives a destination (cutoff, resonance, volume, pan) with an LFO (sine / triangle / saw / square) at a given rate and depth, so parameters move continuously over time. Phase is a function of absolute time, so live playback and offline renders stay in sync.

**Audio tracks — three sections:**

| Section | Parameters |
|---------|-----------|
| Effect Chain | Delay Time (0–2000 ms), Delay Feedback, Delay Mix, Cutoff (LP filter), Drive (tanh) |
| 3-Band EQ | Low gain (±12 dB), Mid gain (±12 dB), High gain (±12 dB), Mid frequency (200–8000 Hz) |
| Compressor | Threshold (−60–0 dBFS), Ratio (1–20), Attack (0.5–100 ms), Release (10–1000 ms) |

Hardware knobs and faders (from a connected MIDI controller) are shown with their live CC values in the Hardware Knobs / Hardware Faders sections.

### Arrange tab (`7`)

| Key | Action |
|-----|--------|
| `↑` / `↓` | Select track row |
| `←` / `→` | Move bar cursor |
| `Enter` | Place / cycle pattern at cursor bar |
| `Backspace` | Remove pattern at cursor bar |

### Visualizer tab (`8`)

Real-time audio visualization — no keys needed, updates at 60 fps.

- **Top half — Oscilloscope:** mixed stereo signal rendered as a waveform (cyan). Center line in gray marks zero amplitude.
- **Bottom half — Spectrum Analyzer:** 2048-point Hann-windowed FFT displayed on a log frequency scale (20 Hz – 20 kHz). Bar color encodes amplitude: green (< −18 dBFS) → yellow (< −6 dBFS) → red (≥ −6 dBFS).

## Live Piano Mode

Press `l` from any tab to enter live piano mode. The transport bar shows `[LIVE]`. Press `l` or `Esc` to exit.

```
Sharps:  2  3     5  6  7
Upper:  q  w  e  r  t  y  u   →  C5 D5 E5 F5 G5 A5 B5

Sharps:  s  d     g  h  j
Lower:  z  x  c  v  b  n  m   →  C4 D4 E4 F4 G4 A4 B4
```

Each key press fires a NoteOn immediately (duration ≈ one 16th note at current BPM).

**Step-recording:** when live mode is active on the Piano Roll tab, each key press also writes the note into the pattern at the current cursor position and advances the cursor by one step.

**Real-time MIDI recording:** press `^R` on the Piano Roll tab while the sequencer is playing to record notes from a MIDI keyboard in real time. Notes are committed to the pattern when recording stops.

## Metronome

Press `^N` from any tab to toggle the metronome. When active:

- `[MET]` appears in yellow in the transport bar
- Claves (GM pitch 75, channel 9) click on every beat
- The downbeat of each bar is accented (velocity 100 vs 60)
- State is saved in the project file (`metronome: true/false`)

## Swing

Press `{` / `}` to decrease / increase swing in 5% steps. Swing delays every offbeat 8th note step by up to one-sixth of a beat (triplet ratio at 100%). The current value is shown in the transport bar (`swing 0%`).

## Velocity, Humanization & Articulation

Step patterns store a velocity per step, not just on/off:

- `0` = off, `1` = the row's default velocity, `2`–`127` = an explicit velocity
- In the sequencer, `<` / `>` nudge the selected step; the grid shows intensity as `· ░ ▒ ▓ █`
- **Humanization** (`Shift+H`) adds deterministic per-note velocity variation (±35% at 100%). It's seeded from `(channel, pitch, tick)`, so live playback and rendered exports are identical and reproducible. Stored as `project.humanize` (0.0–1.0).
- **Gate** (`(` / `)`) sets a step pattern's note length as a fraction of one step: `0.5` is the default, `>1.0` overlaps into the next step (legato), `<0.5` is staccato. Stored as `gate` on the step pattern.

## Modulation (LFO matrix)

Each track can carry any number of LFO **modulators** that move a parameter over time — breathing filters, drifting pans, tremolo, slow swells. Edit them in the FX tab (`m` add, `x` remove, `Tab` select, `d`/`s` cycle dest/shape, `,`/`.` rate, `<`/`>` depth).

```json
"modulators": [
  { "dest": "cutoff", "shape": "sine",     "rate_hz": 0.1, "depth": 0.3 },
  { "dest": "pan",    "shape": "triangle", "rate_hz": 0.5, "depth": 0.4, "phase": 0.25 }
]
```

`dest`: `cutoff` · `resonance` · `volume` · `pan`. `shape`: `sine` · `triangle` · `saw` · `square`. The LFO oscillates `±depth` around the parameter's static value; `rate_hz` is its frequency.

## Generative Toolkit

`daw-core::generative` provides pure, dependency-free building blocks:

- `euclidean(pulses, steps, rotation)` — Euclidean rhythms (E(4,16) four-on-the-floor, E(3,8) tresillo, …). In the step sequencer, `u` / `U` fill the selected row with one more / fewer evenly-spread pulse.
- `scale_intervals` / `scale_pitches` / `snap_to_scale` — scale-aware note handling (major, minor, dorian, phrygian, pentatonics, blues, …)
- `chord_intervals` / `chord_pitches` — a chord palette (maj7, min9, dom7, dim, sus, 7♭9, …)
- `arpeggiate(pitches, mode, …)` — turn a chord into an arpeggio (up / down / up-down / random; random is deterministic)

## Live-Code REPL (mini-notation)

The **Code** tab (`9`) is a Strudel/TidalCycles-style live-coding pane. Pick a target track (`↑`/`↓`), type a mini-notation pattern, and press `Enter` — it compiles to a piano-roll pattern and is applied to the running project **on the next bar boundary** (cycle-quantized), so the change lands in time. `Esc` leaves the tab.

```
bd sd hh hh     sequence (equal slots)     ~        rest
[a b]           sub-group (one slot)        a , b    stack (layer)
a*3             repeat in slot              a!3      replicate
a@3 b           weight (3:1 time)           bd(3,8)  euclid  (+rotation: bd(3,8,2))
```

Tokens resolve to MIDI pitches: drum names (`bd sd hh oh cp rs lt mt ht cr rd cb`), note names (`c3 e4 g#3 eb2`, default octave 4), or raw MIDI integers (`60`). The same `daw-core::mini_notation` parser is reusable from code (e.g. by an LLM editing the project in JAM mode).

## Multi-Soundfont

A track's instrument can name its own soundfont, so one project can mix timbres from several `.sf2` files. Omit the field to use the project's main soundfont (existing projects are unaffected).

```json
"instrument": { "type": "soundfont", "preset": 38, "bank": 0, "soundfont": "assets/synths.sf2" }
```

Currently applied by the **offline WAV exporter** (each soundfont gets its own synthesizer, summed); live multi-soundfont playback is in progress.

## JAM Mode (live coding)

Press `Shift+J` to arm JAM mode (`[JAM]` appears in the transport bar). While armed, the app watches the project file and **hot-reloads it whenever it changes on disk** — so you can edit the `.daw.json` by hand, or have an LLM (e.g. Claude) rewrite patterns, chords, effects, or the arrangement, and hear the change within ~300 ms. The app's own saves don't trigger a reload, and a half-written file is tolerated and retried.

## Headless Rendering

Render any project to a stereo WAV without launching the TUI:

```bash
cargo run -p daw-core --example render -- projects/my-song.daw.json out.wav
```

The exporter peak-normalizes the master to −1 dBFS. Two demo tracks are included: `projects/midnight_static.daw.json` (electronic trip-hop) and `projects/neon_pulse.daw.json` (techno).

## Live Audio Recording

1. Press `^A` on the Tracks tab to add an Audio track (`[AU]`)
2. Select it with `↑` / `↓`
3. Press `^R` — the track label turns red and the transport bar shows `[REC]`
4. Play or sing into your microphone / line-in
5. Press `^R` again to stop — the clip is saved to `recordings/<track_id>.wav`
6. Press `Space` to play back — the clip loops alongside your MIDI tracks

Recordings are 32-bit float WAV. If the input and output devices run at different sample rates the clip is resampled automatically.

## Saving & Exporting

| Action | Key | Notes |
|--------|-----|-------|
| Save project | `^S` | Saves to current path; prompts for filename if unsaved |
| Save As | `^Shift+S` | Always prompts for filename |
| Export WAV | `^E` | Prompts for output filename; renders offline at 44.1 kHz |

When the project has an arrangement, the export renders the full timeline; otherwise it renders one cycle of the longest pattern. A 2-second decay tail is added, LFO modulation is applied per block, and the master is peak-normalized to −1 dBFS. Both MIDI synthesis and looping audio clips are included in the mix.

## Project File Format

Projects are plain `.daw.json` files — fully editable by hand or by an LLM.

```json
{
  "version": "0.1",
  "project": {
    "name": "My Song", "bpm": 120.0,
    "key": "C", "scale": "major",
    "swing": 0.0, "metronome": false, "humanize": 0.0
  },
  "tracks": [...],
  "patterns": [...],
  "arrangement": [...]
}
```

Each track carries an `effects` object with all DSP parameters so that saving and loading is lossless.

## Architecture

Cargo workspace with four crates:

- **`daw-core`** — audio engine (cpal + rustysynth), biquad EQ, compressor, offline WAV exporter, BPM sequencer, project format, WAV recorder. Zero TUI/UI dependencies.
- **`daw-io`** — MIDI input/mapping and the `Host` that wires the engine, sequencer, and I/O threads together (shared by both frontends).
- **`daw-tui`** — ratatui terminal frontend, keyboard/MIDI handling, rustfft spectrum analyzer.
- **`daw-gpui`** — native desktop frontend (gpui): transport, arrange timeline with waveform audio clips, mixer, browser, and editor dock.

**Threads at runtime:**

| Thread | Role |
|--------|------|
| TUI / main | Renders at ~60 fps, handles keyboard events |
| Sequencer | BPM clock, pattern → AudioEvent dispatch |
| Audio relay | WAV file loading + sample-rate conversion |
| cpal output callback | Synthesizer + clip mixing, writes vis buffer |
| cpal input callback | Records microphone input |
| midir callback | Forwards MIDI CC / NoteOn to audio thread (zero-latency) |

Sequencer → audio via lock-free `ringbuf` queue. The visualization buffer is an `Arc<Mutex<Vec<f32>>>` written with `try_lock` in the audio callback (non-blocking) and snapshotted by the TUI thread each frame.

## Drive it with an LLM (MCP)

`daw-mcp` is a stdio MCP server bound to one project file. An LLM can read
and edit the project (tracks, drum steps, mini-notation melodies,
arrangement, FX, sends, CLAP plugins, the loop region), render any bar
range offline, and "listen" via numeric analysis: peak/RMS dBFS, clipping,
per-bar loudness, stereo width, and a 7-band spectral balance.

```bash
cargo build -p daw-mcp
# register for Claude Code: .mcp.json in this repo already points at it
target/debug/daw-mcp my-song.daw.json   # or run it from any MCP client
```

Open the same project in the GUI with JAM mode on and every MCP edit
hot-reloads live (each one lands in the GUI's undo history). Renders go to
`renders/` next to the project.

## Testing

```bash
cargo test --workspace

# Audio smoke test (requires speakers and assets/GeneralUser.sf2)
cargo test -p daw-core smoke_plays_note -- --ignored --nocapture
```

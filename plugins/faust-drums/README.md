# West African Drum Kit

A stereo CLAP instrument with six synthesized, velocity-sensitive percussion
voices. The included daw-ai project uses two editable step patterns. Click a
row to program that voice or trigger it with a MIDI note.

| MIDI | Note | Voice |
| ---: | --- | --- |
| 36 | C2 | Djembe bass |
| 37 | C♯2 | Djembe tone |
| 38 | D2 | Djembe slap |
| 39 | D♯2 | Dunun |
| 40 | E2 | Bell |
| 41 | F2 | Shekere |

## Build and play

Run from the daw-ai checkout with Rust and Faust installed. This build was
verified with Faust 2.70.3. Set `FAUST` to use a compiler outside your PATH.

```bash
./plugins/faust-drums/build.sh
./run-gui.sh outputs/faust-drums/demo.daw.json
```

The builder compiles `drums.dsp` to Rust, links it into a CLAP shared library
and packages the plugin beside the demo in `outputs/faust-drums`. The plugin
uses the generated DSP at playback time. The **Level** control is available
under the kit's instrument parameters in **FX**.

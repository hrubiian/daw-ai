# Model-M for daw-ai on Linux

This local CLAP wrapper runs the DSP from the Model-M v4 Flow Music export.
The SHA-256 check binds the wrapper to the DSP version whose ABI and memory
layout were verified. The build embeds the DSP and its parameter metadata.
It uses daw-ai's standard CLAP controls and project state.

From the daw-ai checkout, build with Cargo and Python 3.

```sh
CARGO_BUILD_JOBS=1 python3 plugins/flow-model-m/build.py /path/to/Model-M_Modular_Synth_v4.fmx outputs/model-m/clap/Model-M.clap
./run-gui.sh examples/quickstart.daw.json
```

Select a melodic track in **First Light**, then choose **Model-M Modular
Synth** from **Browse instruments… → CLAP**. Press **Space** to hear the
track's notes or play it from your configured MIDI input. In the track's
**FX** pane, click
**Open editor** to open the original Model-M panel in a new browser window.
The knobs follow edits from daw-ai and work while playback is running.
The scope displays daw-ai's master output. Save the daw-ai project to
keep your changes. The browser connects to a local bridge started by
daw-ai and uses the editor assets beside the `.clap` file.

In other projects, select a melodic track and search for **Model-M** or
**Moog** in **Browse instruments…**. Choose **Model-M Modular Synth** in the
**CLAP** section and click it again to confirm. Builds run from this checkout
find the local bundle automatically. For binaries built elsewhere, add
`outputs/model-m/clap` to `CLAP_PATH` as an absolute path.

The wrapper and editor cover the Model-M v4 export and its 23 parameters.

## KeyLab Essential 61

Select the keyboard's **MID** input in daw-ai and select the Model-M track.
daw-ai also opens the matching **DAW** port for the KeyLab's controls.
In the KeyLab's DAW map, the first eight encoders control cutoff, resonance,
filter envelope amount, LFO rate, LFO-to-cutoff, LFO-to-pitch, noise and
oscillator 2 level. Faders 1–4 control the amp envelope, faders 5–8 control the
filter envelope, and fader 9 controls synth volume.

User maps can send absolute MIDI CC on the MID port. Model-M responds to
CC 74 for cutoff, 71 for resonance, 76 for LFO rate, 77 for LFO-to-cutoff,
73/75/79/72 for the amp envelope, and 7 or 85 for volume.

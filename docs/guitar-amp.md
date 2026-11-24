# Guitar amp (low-latency monitoring)

Plug a guitar into an audio interface and play through an amp and speaker
cabinet inside daw-ai. Added 2026-11-11. Verified by unit tests and silent
smoke tests on a UMC404HD; **not yet confirmed by ear with a guitar**.

## Using it

1. Press **Amp setup…** beside **🎸 Amp** in the transport bar. It opens a
   separate resizable window. Pressing the button again focuses that window.
2. Under **Guitar input**, pick the interface and mono input the guitar is plugged into, then switch
   **Monitoring: on**. **🎸 Amp** in the transport still toggles monitoring and
   shows the measured software latency while on.
3. Use headphones, or keep the guitar away from the speakers: an open input
   feeding speakers can feed back.

The setup window has input selection, refresh, input/output meters and the
controls: Gain, Bass, Mid, Treble, Level, Noise
gate (Shift+click for fine steps), Amp on/bypassed, Cabinet (built-in 4×12
voicing / a loaded impulse response / off), Load IR…, Buffer, and Output.
The guitar input is saved separately from **Settings → Default audio input**.
**Use recording default** restores the link used by older configurations.
Everything is saved in `~/.config/daw-ai/config.json`. Monitoring is never
switched on automatically at startup.

### Recording the amp and cabinet

Each track header has an **In: … ▾** button. Choose the interface, mono input
or stereo pair, and the sound to record. The route is saved with that track
in the project. **Use recording default** restores the global input selection.
Audio input recording requires an **Audio** track (use **+ Audio**).

- **Auto (match guitar input)** prints the amp and cabinet when guitar
  monitoring is active on the same mono input; other inputs record dry.
- **Guitar rack (amp + cab)** always prints the current rack into the take WAV,
  including the selected/blended cabinet IR, even when monitoring is off.
- **Dry input** records the unprocessed interface signal for later processing.

Choose **Guitar rack (amp + cab)** to reliably keep the amplified sound.
Press **Rec** to arm and meter the selected track's input, then **Rec** again
to record. **Stop** while armed releases the input. Input routing is locked
during a take, including loop passes. Rack tone changes also update the
recording processor. Recording uses the same DSP as monitoring, at the capture
stream's own sample rate. The audio track's **Monitor** switch sends its
selected input through track inserts. The take still uses the chosen recording
source. Existing dry recordings stay dry.

### Voice, guitar and MIDI together

Choose the mic's input under **In: …** on its audio track and enable **Monitor**.
Choose a different mono input in **Amp setup… → Guitar input** and enable the
guitar monitor. On one multichannel interface, use the physical jack numbers for
the two mono selections. DAW‑AI rejects a second monitor on a channel already
in use, including a channel included in a stereo pair.

Choose the keyboard port in **Settings → MIDI keyboard**, then select the
instrument track it should play. **Keyboard plays** shows its target. Selecting
an audio track keeps the keyboard on the previous instrument. The desktop app
arms and records one audio track at a time.

Two bundled IR buttons are ready to compare: **Open-back 1×12** (bright/airy)
and **Closed-back 4×12** (tight/darker). Clicking either immediately selects
IR mode and applies it to the running rig. These are original **modeled**
responses, not measured cabinet captures. They work offline from any launch
directory: they are synthesized from a short filter chain in
`crates/daw-io/src/cabinet.rs`, so no IR files ship with the app.
**Load IR…** still loads your own WAV. The selected source persists across
restarts and buffer/output changes. Cabinet cycles through built-in, selected
IR, and off. Closing the setup window leaves monitoring running; use
**Monitoring: off** or **🎸 Amp** to stop it.

## Getting the lowest latency

- **Buffer**: start at 128 frames; go to 64 if it stays clean, up to 256 if
  you hear crackles. The settings panel shows the period the device actually
  granted and a dropout count.
- **Output: the input device's outputs** plays through the interface itself
  rather than the system default output. Use it when the interface is not
  the desktop's default output device.
- **Sample rate**: 48 kHz or higher on the interface. The same buffer in
  frames is shorter in time at a higher rate.
- The desktop audio server (PipeWire) normally runs a 1024-frame buffer
  (about 21 ms each way). The monitor asks for its own small buffer, which
  makes PipeWire shorten its cycle while the amp is on.

What the reported number covers: input period + the safety cushion + the
output device queue. The interface's own converters and USB transfer
(typically 1–3 ms on a class-compliant USB interface) are on top and cannot
be measured from software. The UMC404HD's front-panel "direct monitor" mix is
still the only zero-latency path, but it carries the dry signal, not the amp.

Measured on 2026-11-11 with a UMC404HD through PipeWire at 48 kHz (silent
test, output level zero, the app's own playback stream open at the same
time): the sound server ran a 64-frame cycle; the 128 setting settled at a
160-frame cushion, 7.5 ms, no dropouts over 4 s; the 256 setting gave 8.0 ms,
no dropouts. Not yet confirmed by ear with a guitar.

### Routing

With a sound server (PipeWire/PulseAudio) the interface cannot be opened
directly: the server already holds it for the song's playback. The amp
therefore opens the system default devices and points each stream at the
server's node for the interface picked under Settings → Input. Any entry of
that interface in the list works (`hw:`, `front:`, `iec958:` …); only its
card id is used. "Output: this interface" plays through the same card;
"Output: system default" uses whatever the desktop plays through. Without a
sound server the named ALSA devices are opened as before.

The safety cushion starts at one period and grows by half a period after
each dropout (up to six periods), so a setting that is slightly too tight
for the machine settles after a few clicks instead of crackling for good.

## How it works

- `crates/daw-core/src/audio/amp.rs` — `GuitarRig`: noise gate → high-pass →
  drive pedal → gain → two soft-clip stages (2× oversampled) → tone stack → cabinet → modulation → echo → reverb → level
  → safety clip. All sample-by-sample; a test asserts an impulse comes out on
  the same sample it went in. The IR cabinet is a direct (time-domain)
  convolution of up to 2048 taps, so it adds no block delay either.
- `crates/daw-core/src/audio/monitor.rs` — `MonitorStream`: its own input and
  output streams with a small fixed buffer, joined by a lock-free ring. It is
  separate from the song mixer's stream on purpose: the mixer renders plugins
  and many voices and keeps its comfortable buffer, so the song does not
  crackle because the guitar wants 32-frame periods. If the output stalls,
  the backlog is discarded so the delay returns to one cushion.
- `daw-io` `Host::start_monitor` / `stop_monitor`, settings in
  `UserConfig.guitar`. GUI in `crates/daw-gpui/src/app/guitar.rs`.

## Not done

- No re-amp of an existing recorded take yet.
- One amp voicing. Live monitoring is mono; recording supports mono or stereo
  with independent amp/cabinet state per channel. No tuner.
- On non-Linux hosts the "buffer" value is passed to the system as-is and
  the period relationship described above may differ.

Smoke test (opens the default devices for a few seconds with the level at
zero, prints what the system granted; set `DAW_MONITOR_SOURCE` /
`DAW_MONITOR_SINK` to sound-server node names to test one interface):
`cargo test -p daw-core monitor_smoke -- --ignored --nocapture`

## Building a rack and custom cabinet

**Amp setup…** now opens a preset library beside the live rack. Choose **Rock**
(crunch and blended cabs), **Jazz** (warm clean), or **Metal** (tight high gain).
Presets set the amp and cabinet without changing your interface, input channel,
output routing, or buffer. The library shows **Custom tone** after you edit a
preset. Enter a name under **My presets** and choose **Save current rig**;
saving the same name replaces it. Saved tones persist in the user config.

**Thrash Rhythm** is a fast palm-muted rhythm-tone starting point using the expanded rack:
Boost (gain 30%, tone 64%, level 60%) → amp (gain 74%, bass 54%, mids 24%,
treble 67%, gate 48%, output 35%) → 90% closed-back 4×12 / 10% open-back 1×12,
with 90 Hz low cut and 6.2 kHz high cut. Modulation, echo, and reverb are off
to leave space between fast palm-muted notes.
Start with the bridge pickup; adjust gain/gate for your guitar's output.
Choose the track's **Guitar rack (amp + cab)** recording source to print the
whole tone into new takes. Selecting the preset preserves input/output routing.

Expand **Input & routing** to choose the interface/channel or see meters.
In **Cabinet builder**, choose **Dual IR** and load/select IR **A** and **B**.
The A/B blend runs from A only to B only; select B before increasing its blend.
**B phase** inverts B's polarity, which can change cancellation between the IRs.
Low cut removes bass rumble; high cut reduces fizz. At 20 Hz / 20000 Hz the
respective cut is bypassed. These controls affect Dual IR mode, with the
combined result built off the audio thread into the same single 2048-tap FIR.
Phase cancellation stays quiet; the mixed result is not normalized back up.

**Export cabinet WAV…** saves the current combined cabinet as a mono 48 kHz,
16-bit WAV for other IR loaders. It contains the blended/filtered cabinet,
not the nonlinear amp or noise gate. A preset referencing a custom WAV still
needs that source file after restarting; export a cabinet to make a single
portable WAV. Built-in and Bypass modes leave the recipe available for later.

## CLAP guitar plugin options

### AmpForge live backend

AmpForge is supported as an external CLAP guitar processor, including its native
floating editor. Tested with the official Linux x86_64 v0.5.0 release. Install
the Linux `.clap` from [AmpForge releases](https://github.com/Loursy/AmpForge/releases)
under `~/.clap/` (for example `~/.clap/AmpForge/ampforge_main.clap`). AmpForge's
source and license remain separate from daw-ai; no AmpForge DSP is embedded here.

1. Open **Amp setup…**, choose **Rescan plugins**, then **AmpForge** under
   **Guitar processor**. Switching back to **Built-in rack** retains the cached
   AmpForge tone.
2. Select the guitar's mono input and enable **Monitoring**. Choose
   **Open AmpForge editor…** to use its pedals, amp, presets, cabinet WAV loader,
   and NAM model loader. On Linux the floating editor uses X11/XWayland.
3. AmpForge supplies the complete live processing chain. The built-in amp,
   cabinet, and pedals are bypassed in this mode. Load a cabinet in AmpForge
   when your selected amp/model needs one; its distribution does not bundle IRs
   or NAM captures. The built-in cabinet can be exported as a WAV from the rack and loaded in
   its editor.
4. For audio takes select **Mono** and **Guitar rack (amp + cab)** in the track's
   **In: …** picker. **Auto** also prints AmpForge if monitoring is on when the
   take starts. **Dry input** continues recording dry.

Native editor edits are saved to the user config; **Save AmpForge tone** captures
them immediately, and **My presets → Save current rig** saves a named external
tone. Cabinet/model paths travel in plugin state, so keep those files available.
Tempo-sync effects receive the project's BPM. The live path uses one mono
instance; recording uses a second instance at the input's sample rate, initialized
from the live tone. The editor hides during recording and the recorded tone is
held until the take ends. Wet takes already contain the processor; adding another
AmpForge track insert processes them again. NAM models increase CPU load; a larger buffer can help
monitoring if needed. A missing or unloadable plugin is reported, without silently
substituting a dry take or the built-in amp.

Compatibility checks (the AmpForge tests require the installed external plugin;
the editor check also requires a working desktop display):

```bash
cargo test -p daw-core --test ampforge_live -- --ignored --test-threads=1
cargo test --workspace
```

### Recorded-track inserts

The rack's **CLAP plugins** section scans installed effects and can add one to
the track selected in the main window. This is the existing **track playback**
insert chain: it does not process the separate low-latency guitar monitor.
Choose **Dry input** to record dry guitar, then use the plugin on the recorded track. Plugin
parameters are available in the existing Audio Effects editor; embedded native
plugin editors are not provided here.

Two candidates with documented Linux CLAP builds, checked on 2026-11-11:

| Plugin | Useful for | Upstream |
| --- | --- | --- |
| BYOD (ChowDSP) | Modular guitar distortion and tone-shaping chains | [Project](https://github.com/Chowdhury-DSP/BYOD), [manual](https://github.com/Chowdhury-DSP/BYOD/blob/main/manual/Manual.md) |
| OrbitCab | Amp captures, power-amp stages and dual-IR cabinets | [Project and installation](https://github.com/darwinscat/orbitcab) |

Install the **Linux CLAP** build in `~/.clap` (or `/usr/lib/clap`,
`/usr/local/lib/clap`, or a directory in `CLAP_PATH`), then press **Rescan
installed CLAP effects**. These are compatibility candidates based on upstream
format support; neither binary has been installed or validated in daw-ai in
this change. Use external plugins through the host rather than bundling their
source into the app; their licenses differ from this project's license.


## Live guitar pedals

The rack now includes pedals in the monitor's actual signal path:

`input → gate → drive → amp → cabinet → modulation → echo → reverb → level`

- **Drive**: Off, Boost, Overdrive, Distortion or Fuzz. Drive, Tone and Pedal
  level controls. These are generic modeled flavors rather than branded pedal
  emulations. Boost includes a bass cut before the amp for tighter palm mutes.
- **Modulation**: Off, Flanger, Chorus or Tremolo. Rate (0.05–8 Hz), Depth and
  Mix; Flanger/Chorus also have feedback from −85% to +85%. Negative feedback
  changes the comb-filter character. Tremolo modulates volume.
- **Echo**: Off, Digital, Tape or Slapback. Digital and Tape have Time
  (20–1500 ms), Feedback (0–85%) and Mix (0–70%). Tape adds high-frequency
  damping and gentle delay modulation. Slapback gives one repeat, with Time
  limited to 20–250 ms; feedback is disabled. Time changes crossfade between
  read positions over 20 ms.
- **Reverb**: Off, Room or Hall, with Size/decay, Damping and Mix (0–70%).
  Uses the existing Freeverb network, with shorter room or longer hall decay.

**Pedal combinations** offers Dry, Tight rhythm, Clean chorus, Lead echo and
Jet flange. These buttons change only pedals, leaving amp and cabinet choices
in place. Each pedal's Off mode bypasses it. Mode changes clear that pedal's
stored tail; bypass does not preserve an old echo/reverb for later reactivation.
The final amp Level controls the whole chain including echoes and reverb.

Controls update the running rig without restarting the interface. Delay
buffers are allocated once when the monitor opens; audio processing and
parameter changes do not allocate/free memory. The dry path has no added
block delay; the audible flanger/chorus/echo/reverb delays are intentional.

The entire pedal chain saves with **Save current rig**, and restores with a
factory or user tone. Existing configs and presets with no pedal fields load
with every pedal off. The **Guitar rack** recording source prints this same
pedal chain into the WAV; **Dry input** keeps the unprocessed guitar. **Export
cabinet WAV…** exports only the cabinet IR, excluding pedals and amp distortion.

Suggested starting combinations: **Tight rhythm** for heavy riffs, **Lead
echo** for solos, **Clean chorus** with a clean amp, or **Jet flange** for a
pronounced sweep. These combinations have been checked with generated signals;
final guitar tone and preferred levels still need listening on your interface.

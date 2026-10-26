# Creative effects and plugins

Open **Audio Effects** or the **FX** dock. **Track / Reverb / Echo** switches
between track inserts/sends and the two shared spaces. Bus edits affect every
track sending to that bus. Presets preserve its fader and mute. Shift+click
steppers for smaller adjustments.

## Built-in effects

Every track type exposes insert delay, saturation, low-pass filtering,
three-band EQ, compression and reverb/echo sends. SoundFont instruments
also expose instrument chorus, resonance and vibrato.

Reverb adds 0–250 ms pre-delay and low/high cuts to room size, damping and
width. Small room, wide hall and dark space are presets of the existing
Freeverb network, not separate plate/convolution algorithms.

Echo adds free timing up to 12 seconds or tempo-linked divisions, including
dotted/triplet eighths. Controls include stereo/ping-pong routing, filtering
in the feedback path, width, and 0–20 ms delay modulation at 0.05–8 Hz. Tape
echo is a filtered/modulated delay preset, not a physical tape model. Slapback
uses a short stereo delay. The time readout resolves sync at the current tempo;
choosing Free ms or nudging time retains that time and disables sync.

Older files keep their unsynced ping-pong sound, no modulation/pre-delay, and
bypassed new tone filters. Buffers are allocated at construction; control
changes do not resize them. Delay/pre-delay changes crossfade read positions
over 128 frames. Tone, width and routing changes are not fully smoothed yet.
Expanded spatial settings receive extra full-render decay time, capped at 30
seconds; extreme feedback/long delays can outlast it. Legacy settings keep the
two-second pad. Explicit ranges warm up from song start and stop at the chosen
range boundary.

## CLAP workflow

Install a **native Linux CLAP** build in `~/.clap`, `/usr/lib/clap`,
`/usr/local/lib/clap`, or `CLAP_PATH`. Rescan from the FX page and add an
installed effect to the selected track. Generic parameter controls support
fine increments and default reset, honor discrete parameters, and do not edit
read-only parameters. Plugin state/overrides persist through the existing
host and session undo path.

Native plugin windows, plugin preset browsers, tempo/transport context,
latency compensation, sidechains, output automation and sandboxing remain
unimplemented. Third-party plugin tempo-sync is therefore not qualified:
use its millisecond mode or the built-in synced echo. Generic parameters are
not continuous live readback of plugin meters/internal edits.

CLAP is integrated. VST3 needs a separate host implementation; renaming a
plugin is insufficient. The current VST3 SDK is MIT-licensed, but lifecycle,
transport, latency, audio-port and state handling still need implementation
and tests. Windows VST binaries do not run in this native Linux host.

Potential CLAP candidates include commercial u-he Colour Copy (delay) and
Twangström (spring reverb). Neither has been qualified in this host; no external
plugin was installed. Check the publisher's Linux/CLAP build and audition its
demo before purchase. Primary references:

- [Colour Copy](https://u-he.com/products/colourcopy/)
- [Twangström](https://u-he.com/products/twangstrom/)
- [u-he formats](https://u-he.com/support/faq/compatibility/formats/index.html)
- [VST SDK](https://www.steinberg.net/developers/vstsdk/)

## AI tools

`set_returns` accepts any subset of the expanded settings:

```json
{
  "reverb": {"predelay_ms": 35, "low_cut_hz": 180, "high_cut_hz": 7000},
  "delay": {
    "sync": "eighth_dotted", "mode": "ping_pong", "feedback": 0.42,
    "low_cut_hz": 180, "high_cut_hz": 3500, "width": 0.75,
    "modulation_ms": 2.5, "modulation_hz": 0.32
  }
}
```

`apply_spatial_preset` accepts `small_room`, `wide_hall`, `dark_space`,
`tape_echo`, `ping_pong`, or `slapback`, preserving bus gain/mute.
`rescan_clap_plugins` refreshes an MCP process's catalog.
`get_clap_params(uid)` reports declared IDs, bounds, defaults, discrete/read-only
flags and saved overrides from a separately loaded worker instance. It does
not change the project or read live automation/internal edits. Use those IDs
with `set_clap_param`. Build workspace test plugins before standalone MCP
tests; `scripts/check.sh` already does this.

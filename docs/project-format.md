# The `.daw.json` project format

Expanded spatial returns and their backward-compatible defaults are described
in [Creative effects](creative-effects.md).

Every daw-ai song is one human-readable JSON file — open it in a text
editor, diff it in git, or let an LLM rewrite it (with the GUI's JAM mode
on, edits hot-reload into the running app). All fields are
serde-defaulted: files from older versions always load.

```jsonc
{
  "version": "0.1",
  "project": { "name": "Demo", "bpm": 126.0, "time_signature": [4, 4],
               "key": "B", "scale": "minor", "swing": 0.12,
               "metronome": false, "humanize": 0.0 },

  "tracks": [{
    "id": "t1", "name": "Drums", "type": "drum",   // drum | melodic | audio  ← key is "type" (not "track_type")
    "instrument": { "type": "soundfont", "preset": 0, "bank": 128 },
    //            | { "type": "sampler", "pads": [{ "pitch": 36, "path": "kick.wav", "gain": 1.0 }] }
    //            | { "type": "audio_file", "path": "loop.wav" }
    //            | { "type": "clap", "uid": "c1", "path": "/usr/lib/clap/X.clap",
    //                "id": "org.x.y", "state": "<base64>", "params": { "17": 0.42 } }
    "volume": 0.8,            // 0..2 (1.0 = unity)
    "pan": 0.0, "muted": false,
    "patterns": ["p1"],
    "effects": { "reverb": 0.2, "send_delay": 0.1,        // sends to the shared buses
                 "cutoff": 1.0, "drive": 0.0, "eq_low_gain": 0.0, /* … */ },
    "modulators": [],          // per-track LFOs -> cutoff/resonance/volume/pan
    "clap_fx": []              // CLAP insert chain (same slot shape as the instrument)
  }],

  "patterns": [
    { "type": "step", "id": "p1", "name": "Beat", "steps": 16, "gate": 0.5,
      "rows": [{ "name": "Kick", "pitch": 36, "vel": 100,
                 "steps": [1,0,0,0, 1,0,0,0, 1,0,0,0, 1,0,0,0] }] },
      // steps are 16th notes: 0 = off, 1 = on at row vel, 2..127 = explicit velocity
      // 32 steps = a 2-bar pattern
    { "type": "piano_roll", "id": "p2", "name": "Bassline", "bars": 1,
      "ticks_per_bar": 480,
      "notes": [{ "pitch": 47, "tick": 60, "duration": 50, "vel": 118 }] }
  ],

  "arrangement": [ { "track": "t1", "pattern": "p1", "bar": 0, "len_bars": 4 },
                   { "track": "t2", "pattern": "p2", "bar": 2, "offset_ticks": 240 } ],
  // len_bars stretches a clip; the pattern loops to fill.
  // offset_ticks (default 0) starts a clip off the bar line: 480/bar, so 240 =
  // half a bar, 120 = one beat. Absolute start = bar*480 + offset_ticks.

  "audio_clips": [ { "track": "t3", "path": "recordings/take-p1.wav",
                     "start_sec": 4.0, "offset_sec": 0.012, "len_sec": 8.0, "gain": 1.0 } ],

  "returns": { "reverb": { "gain": 1.0, "room_size": 0.6, "damping": 0.4, "width": 1.0 },
               "delay":  { "gain": 1.0, "time_ms": 310, "feedback": 0.3 } },

  "loop_region": { "start_bar": 0, "end_bar": 4, "enabled": false,
                   "punch_in": false, "punch_out": false }
}
```

Timing: 480 ticks per 4/4 bar, 120 per beat; step-grid steps are always
16th notes (30 ticks). The authoritative schema is the serde model in
`crates/daw-core/src/project/mod.rs`. See [Live collaboration](live-collaboration.md) for validation and persistence.

## Expressive sampler pads

The optional `variants` field adds velocity layers and alternate takes to a pad:

```json
{
  "pitch": 38,
  "path": "samples/snare-base.wav",
  "gain": 1.0,
  "variants": [
    {"path": "samples/snare-soft-1.wav", "min_velocity": 1, "max_velocity": 63},
    {"path": "samples/snare-soft-2.wav", "min_velocity": 1, "max_velocity": 63},
    {"path": "samples/snare-hard-1.wav", "min_velocity": 64, "max_velocity": 127}
  ]
}
```

Different ranges select different attacks/timbres. Identical ranges rotate takes
in their saved order, independently for each layer. Note velocity still controls
loudness. Optional per-variant `gain` (default 1, range 0–2) multiplies pad gain.
Ranges must be disjoint or identical, within 1–127; each pad supports at most 32
variants. Uncovered velocities use the original `path`, which remains required.
Zero-gain triggers do not advance rotation. Panic resets rotation; ordinary note
and mixer edits retain it when the instrument resources are unchanged. Old pads
without variants keep their original behavior. Replacing a base sample clears
its variants; clearing variants alone preserves the base sample.

All sample loading occurs when preparing strips, outside the audio callback.
Voices refer to the selected preloaded take, so another hit cannot change an
already sounding voice. Each strip reserves 128 sampler voices; overload steals
the oldest voice and increments `audio_diagnostics.sampler_voice_steals`.
This is a bounded overload policy, not seamless unlimited polyphony.

Live playback and export share selection. Range renders warm from song start to
preserve rotation history. Missing takes fail export with an explicit error;
asset resolution and render provenance include every alternate take.

Use MCP `set_sampler_variants(track_id, pitch, variants)` to edit a pad through
the validated project/session path. `[]` clears its takes. In the GUI FX tab,
Base replaces the sample; +Soft/+Hard append takes to ranges 1–63 and 64–127.
All changes use undo. Shift+click a step or piano-roll note to increase velocity,
Alt+click to decrease it; step cells show velocity and piano-roll starts show
velocity in their hover help. These controls preserve an existing note's onset
and duration. Step velocity 1 retains its legacy meaning (row-default); explicit
step dynamics use 2–127.

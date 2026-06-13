# The `.daw.json` project format

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
    "id": "t1", "name": "Drums", "track_type": "drum",   // drum | melodic | audio
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

  "arrangement": [ { "track": "t1", "pattern": "p1", "bar": 0, "len_bars": 4 } ],
  // len_bars stretches a clip; the pattern loops to fill

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
`crates/daw-core/src/project/mod.rs`; design rationale lives in
`docs/superpowers/specs/`.

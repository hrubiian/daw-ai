# Assets

Large binaries (`*.sf2`, `*.wav`, `assets/samples/`) are **gitignored** — they're
fetched locally, not committed (same model as the soundfont). Get the free,
redistributable extras with:

```bash
./fetch-assets.sh
```

## What's here

| Path | What | License |
|------|------|---------|
| `GeneralUser.sf2` | Project default GM soundfont. **It's actually "Fluid R3 GM"** (Frank Wen) — renamed. `head -c 2000 GeneralUser.sf2 \| strings \| grep -i fluid` | MIT |
| `samples/wav/*.wav` | Drum one-shots (TR-909, LinnDrum, lo-fi) + vocal clips | mixed / check source |
| `samples/vcsl/` | **VCSL** — Versilian Community Sample Library (after `fetch-assets.sh`) | CC0 |
| `icons/` | App icons | project |

## Adding more — sources

Because this is an open-source repo, only commit (or rely on others fetching)
**redistributable** licenses: **CC0 / public domain / MIT / CC-BY**. Avoid
"royalty-free but no redistribution" packs.

- **Soundfonts (.sf2):** [Musical Artifacts](https://musical-artifacts.com/artifacts?formats=sf2&license=free) ·
  [Polyphone](https://www.polyphone.io/en/soundfonts) · FluidR3 (MIT) · MuseScore General (MIT).
  Use **.sf2** (rustysynth) — not SFZ, and prefer .sf2 over .sf3.
- **WAV samples:** [VCSL](https://github.com/sgossner/VCSL) (CC0) ·
  [Freesound CC0](https://freesound.org/browse/tags/cc0/) ·
  [Discord SFZ GM bank](https://github.com/sfzinstruments/Discord-SFZ-GM-Bank) (CC0 WAVs).

Any sample rate / bit depth works — the loader resamples to the engine rate.

## Wiring into a project (`.daw.json`)

**Drum / one-shot WAVs** → a Sampler track maps pitch → file:
```json
"instrument": { "type": "sampler", "pads": [
  { "pitch": 36, "path": "assets/samples/wav/mykick.wav", "gain": 1.0 },
  { "pitch": 38, "path": "assets/samples/vcsl/.../snare.wav" }
]}
```

**Loops / vocals** → use in-app **File ▸ Import Sample** (or `+WAV`), or add an
`audio_clips` entry.

**Custom soundfont per track:**
```json
"instrument": { "type": "soundfont", "preset": 0, "bank": 0,
                "soundfont": "assets/MyPiano.sf2" }
```
> ⚠️ **Caveat:** the per-track `soundfont` field currently only affects **WAV
> export** — the live engine plays a single soundfont (`assets/GeneralUser.sf2`).
> To change *live* sound for the whole project, replace that file. (A live
> soundfont picker is a possible future enhancement.)

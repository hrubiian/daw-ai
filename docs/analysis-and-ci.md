# Listening, analysis & Music CI — a playbook

These MCP tools attach measurable audio diagnostics to a render,
and let a song be **versioned and regression-tested** like code. This is the
playbook for *which tool when*.

For revision-linked background jobs, cancellation and A/B artifacts, see [live collaboration](live-collaboration.md). These measurements describe signals and arrangements; they do not grade musical quality.

## Symptom → tool

| You want to know… | Tool | Renders? |
|---|---|---|
| Overall levels / clipping / spectral tilt of the whole mix | `listen` | yes |
| **Which tracks** are muddy / **which tracks mask** each other | `analyze_mix` | yes (per-track solo) |
| Does the piece **build** or is it a flat, static texture? | `analyze_arrangement` | yes (one mix render) |
| Does the rhythm **feel human** or robotic/dead-on-grid? | `analyze_groove` | no (symbolic) |
| Every track sitting at a sensible level? | `auto_balance` | yes (per-track solo) |
| **What changed** between two versions? | `diff_project` | no (symbolic) |
| **Did my last edit break the mix?** | `snapshot` / `check` | yes |

## The ears (perceptual analysis)

- **`listen`** — peak/RMS dBFS, clipping, per-bar loudness, stereo width, 7-band
  balance for the whole mix (or a bar range / soloed subset). Your first ear.
- **`analyze_mix`** — solo-renders each track, reports where each one *lives* in
  the spectrum (energy-weighted centroid) and flags bands where ≥2 audible tracks
  **collide** (low band first — low-end masking hurts most). Use when `listen`
  says "muddy" but not *who*.
- **`analyze_arrangement`** — density (tracks active/bar) and dynamic arc as
  sparklines; flags `flat` (tiny loudness range = static texture) and
  `no intro build` (all tracks from bar 1). Use to check the piece has *shape*.
- **`analyze_groove`** — how notes sit against the 16th grid (on-grid % +
  micro-timing spread) plus `swing`/`humanize`/`timing`; flags `mechanical`.
  Off-grid placement *or* swing both count as feel. Fast, no render.

## Music CI (version + test)

The project is one plain `.daw.json` — so a song can be diffed, snapshotted, and
regression-tested.

- **`diff_project other_path`** — a *musical* changelog vs another `.daw.json`
  (a git-stashed copy, an earlier take): added/removed tracks, volume/instrument/
  effect/tuning edits, note-count and step changes, arrangement moves. Not a JSON
  line-diff.
- **`snapshot`** — render now, save an analyzed fingerprint (peak/RMS/clipping/
  stereo/7-band + track & note counts) to `<project>.snapshot.json`. Run when the
  mix is in a good state.
- **`check`** — re-render, compare to the snapshot, report regressions:
  `FAIL` new clipping · `WARN` peak jump / band drift / lost tracks or notes ·
  `INFO` loudness move. Deterministic render ⇒ a re-render with no edits passes.

## A typical loop

```
compose (apply_mini_notation / set_steps / set_notes)
  → listen                     # overall level & tilt
  → analyze_mix                # any masking? who?
  → analyze_arrangement        # does it build, or is it flat?
  → analyze_groove             # human feel, or mechanical?
  → fix (edit) ; auto_balance  # levels
  → snapshot                   # bank a known-good baseline
  … later edits …
  → check                      # did anything regress?
  → diff_project <baseline>    # what exactly changed
```

## Designing sounds (Surge XT)

The musician describes a sound in words, and Claude designs it on Surge XT
(free and open source; the `surge-xt` deb installs it to
`/usr/lib/clap/Surge XT.clap`). With the GUI open in live mode, every
change is heard on the track at once.

| Step | Tool | What it does |
|------|------|--------------|
| 1 | `find_patches` | Search Surge's free patches by name or category for a starting point ("deep tom" → Percussion). |
| 2 | `load_patch` | Put the patch on its **own new track**, named after the sound. Loading onto a named track whose instrument isn't Surge XT needs `replace: true`. |
| 3 | `describe_sound` | Read the sound's musical controls (oscillators, noise, filter, envelopes, LFO, drive, voice) in Surge's own units. |
| 4 | `shape_sound` | Set controls with text like `"600 Hz"` / `"180 ms"` or a number in range. Everything is checked before saving. |
| 5 | `audition_sound` | Play a phrase (hit, note, chord or line) in the song's key: in the GUI in live mode, and always as a WAV. Reports pitch versus the intended note, attack, decay, sustain, brightness and noisiness. |
| — | `sound_history` / `revert_sound` | Step back to any earlier version from this session. |
| — | `print_sound` | Render the sound (a phrase, or the track's part over bars) to `prints/` and place it on a new audio track "<name> (print)". |

Loop: steps 4–5 until the measured traits match the words, then hand over
to the musician, who steers ("darker", "shorter") by ear. The numbers
guide the next tweak; they never establish that a sound is good.

Patches are stored as the plugin's state in the project, so reload,
`listen`, `start_listen` and export reproduce the sound exactly.
Patch files are read only from Surge's patch folders (plus
`$SURGE_PATCH_DIRS`).

## Building kits and adding movement

| Goal | Tool |
|------|------|
| Map a .wav to a pitch (creates the sampler if the track has none) | `set_sampler_pad(track_id, pitch, path, gain?, gate?)` |
| A long sample that should follow note length (808, pad, chop) | `set_sampler_pad(…, gate: true)`, then write durations with `set_notes` |
| Soft/hard layers and round-robin takes | `set_sampler_variants` |
| Filter sweep, tremolo, auto-pan | `set_modulators(track_id, [{dest, shape, rate_hz, depth, phase?}])` |
| Detune a track between the keys | `set_track(track_id, tuning_cents)` |

`project_summary` lists each sampler's pad pitches (`g` marks gated pads) and
the LFO destinations on a track, so notes can be written for the right pitches
without fetching the whole project.


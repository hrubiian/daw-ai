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

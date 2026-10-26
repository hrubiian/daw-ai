# daw-ai adapter

Use this adapter only in a checkout with the existing daw-ai engine. Prefer
installed production software if that is the user's workflow. Check the current
project-format docs and model; the engine evolves independently of this skill.

- Editable sessions are `.daw.json`; track kind is stored as `type`.
- The current engine validates only 4/4 and uses 480 ticks/bar, 120/quarter and
  30/sixteenth. Changing meter metadata or `ticks_per_bar` does not supply a
  compound-meter timeline. For a 6/8 study, write musical measures of 360 ticks
  (60/eighth), then pack the full performance into 480-tick container measures.
  Four musical 6/8 measures fit three engine bars. Keep a musical-bar map and
  export MIDI with a real 6/8 signature. Label the engine session as a packed
  timeline; do not claim native meter support. Avoid per-phrase container clips
  that introduce gaps or unintended pattern repeats.
- Use project-relative paths for original sampler material. GeneralUser.sf2 is
  an installed GM demonstration palette; its presets cannot establish live
  strings, expressive winds or studio production quality.
- Check decoded source levels against a native solo render. At the time of the
  K violin study, the WAV loader divides all integer samples above 16 bits by
  2^31, making 24-bit PCM sources about 48 dB too quiet. Float WAV and 16-bit PCM
  avoid that issue. Verify the current loader before assuming this workaround
  remains necessary; delivery files played in a browser use a different decoder.
- If a live session exists, inspect its revision and use validated edits. For a
  fresh experiment use a new project folder and do not overwrite another song.
- `start_listen` + `job_status` produces revision-linked audio and provenance.
  `analyze_arrangement` and `analyze_groove` are diagnostics. Their flags are not
  musical grades. Avoid an automatic balance pass that makes accompaniment as
  loud as the lead.
- Calibrate sample drums against GM accompaniment with short solo renders. In
  the initial study, similarly modest fader values left the synthesized kick
  about 17.5 dB above the GM synth bass in whole-window RMS. Fader positions are
  not comparable loudness across instrument types. Adjust roles deliberately;
  do not copy the example's final volumes to different assets.

The experiment scripts are `scripts/compose_studies.py` (deterministic example
projects and original sample assets), `scripts/render_studies.py` (MCP renderer),
and `scripts/audio_probe.py` (numpy + ffmpeg analysis). Example invocation:

```sh
python3 /path/to/bonobo-composer/scripts/compose_studies.py --output /path/to/studies
python3 /path/to/bonobo-composer/scripts/render_studies.py --repo /path/to/daw_cli --directory /path/to/studies
python3 /path/to/bonobo-composer/scripts/audio_probe.py /path/to/studies/renders/estuary.wav --output /path/to/studies/measurements.json
```

`compose_studies.py` is a reproducible demonstration, not the skill's mandatory
composition algorithm. Write new musical material for fresh user requests.

`scripts/prepare_comparison.py --experiment /path/to/experiment` prepares gain-only
masters, verified loudness-matched excerpts and an HTML listening form for the
initial folder layout (`references/` and `studies-v2/`). It requires ffmpeg and
ffprobe. It does not fetch reference audio or fill in listener ratings. Use new
experiment folders to preserve prior delivery artifacts. Its clips use fixed
clock windows; audition and change selections before treating them as equivalent
musical sections.

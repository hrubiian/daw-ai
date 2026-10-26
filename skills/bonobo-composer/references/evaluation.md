# Evaluation protocol

## Signals and symbolic checks

- Verify saved sessions load in the actual renderer and all referenced assets
  exist. Preserve the renderer version/hash and settings with each take.
- Check decoded native and delivery audio for finite samples, peaks, overload,
  audible-duration plausibility, and a clean tail. True-peak checks use a proper
  oversampled meter such as ffmpeg ebur128; ordinary sample peaks are not enough.
- Compute signal descriptors on equal-duration passages. Preserve selection
  criteria and timestamps. Report whole-piece dynamics separately; preview fades
  and an original's intro/outro otherwise confound the comparison.
- Check actual rendered event times where timing is suspect. Symbolic offsets
  and metadata do not prove correct playback timing.
- Verify recurrence AND development: note sequences, phrase endings, active
  roles and deliberate breaks. More off-grid notes or more sections do not
  automatically mean a better groove or a better song.
- Track close voice leading and note overlaps in the score as diagnostics. They
  do not replace listening for dissonance, phrasing, breath and low-end masking.

## Listening comparison

Use 20–30 second excerpts with similar musical functions, matched integrated
loudness to within about 0.5 LU where feasible. Avoid limiter-driven loudness
matching. Lower all excerpts to a common level that preserves true-peak headroom.
Match the whole listening set once, then verify the rendered clips; don't let
each clip's player normalize differently.

Provide randomized A/B labels for original takes and keep the key separately.
Reference audio may identify itself; this is label masking, not full blinding.
Have the listener rate each dimension 1–5 and write one concrete observation:

| Dimension | Listen for |
|---|---|
| Groove | Weight, pocket, stable pulse and purposeful syncopation |
| Fluency | Phrase endings, transitions, breaths and continuity |
| Development | Recognizable motif that changes meaning or answer |
| Timbre/depth | Articulation, acoustic detail, foreground/background |
| Mix | Bass definition, masking, stereo/mono coherence, fatigue |
| Overall | Whether the listener wants to keep listening |

Leave ratings null until actually supplied. Compare originals to each reference
individually and explain when meter or genre makes a dimension incomparable.
Retain listener comments verbatim. Do not call technical compliance a quality
score, equate feature closeness with artistry, or claim professional equivalence.

## Does the skill help?

A self-applied demo is a smoke test. A stronger test uses multiple fresh briefs
and seeds with the same model, tools, time budget and assets, randomly assigning
skill enabled/disabled, followed by listener comparison. Never construct an
intentionally weak no-skill control and present the result as causal evidence.
A loop-versus-developed comparison tests a package of arrangement decisions;
it is not such a controlled skill trial. Record which rules changed, what
improved or regressed, and confidence. Revise only from observed failure.

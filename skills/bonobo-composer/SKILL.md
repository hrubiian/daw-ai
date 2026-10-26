---
name: bonobo-composer
description: Analyze Bonobo references and compose original organic trip-hop or electronica using a DAW, with evidence-linked musical rules, editable arrangements, rendered audio, and honest reference comparisons.
---

# Bonobo-informed composition experiment

Produce original music from transferable techniques: an acoustic seed reshaped as
a sample, interlocking rhythmic roles, economical harmony and developing phrases.
Do not substitute an artist label for a musical brief or reuse a reference melody.
This is a practical composition method, not a trained audio model.

Read [references/reference-study.md](references/reference-study.md) when choosing
the reference branch. It distinguishes artist statements, excerpt measurements
and compositional hypotheses, and records the initial listener's palette choice.
Use that choice as context when continuing this experiment. Kiara, Cirrus and Black Sands are separate cases;
never average their tempos, meters or arrangements into one mandatory template.

## Establish evidence

Locate user audio or public previews and record source, edition, duration, hash,
and excerpt offset. Unknown offsets stay unknown. With full tracks, map musical
sections with timestamps and verify downbeats by audition. With excerpts, describe
only the observed window; do not infer full-song section lengths or keys from
album descriptions. Mark observations as heard, measured, reported or inferred.

If audio listening is available, audition rhythm, bass, phrase endings and section
joins. If only numerical tools are available, say so and leave subjective ratings
unfilled. A tool named `listen` may return signal measurements rather than sound
perception. Read [references/evaluation.md](references/evaluation.md) for tests.

## Compose decisions

1. Choose a lane: **broken-beat motif** (Kiara), **interlocking sample rhythm**
   (Cirrus), or **compound-meter acoustic ensemble** (Black Sands). The first
   demo may combine techniques, but name its main lane and what it cannot test.
2. Write a fresh 2–4-bar musical seed before orchestrating. Give it a recognisable
   contour and an answering rest. Make an 8–16-bar passage with a second ending.
   Render this passage before committing to the long arrangement.
3. Let the seed determine harmony. Try reharmonizing one shared note; use close
   inner voices, root motion and inversion to change its meaning. Added 7ths/9ths
   are options, not a requirement for every chord. Keep the sub-register sparse.
4. Build groove by role. Kick establishes weight; backbeat fixes the pocket;
   quieter percussion answers. Try stable backbeat lateness of 5–15 ms and small
   role-specific offsets, then audition. These are starting hypotheses, not
   transcriptions. Do not combine authored swing with unexamined global jitter.
5. Edit an original pluck into attacks and tails. Align selected attacks with
   percussion; let a secondary figure cross the barline or use a different cycle.
   Preserve the main pulse. Vary articulation/velocity with phrase function;
   random changes alone do not establish phrasing.
   For sustained recorded strings and winds, author phrase boundaries and joins:
   carry the sound between intended connected notes, reduce repeated source
   attacks, and preserve bow changes and breath rests. Use source audio with
   enough sustain; do not bridge missing material with silence. Touching or
   overlapping clip rectangles do not establish audible continuity. Keep distinct
   attacks for plucked instruments and percussion; their phrasing needs a
   different treatment. Crossfaded sustains approximate connected playing but
   do not supply recordings of real pitch transitions.
6. Give each 8–16 bars a musical event: a different answer, bass inversion,
   rhythmic displacement, silence or reharmonization. A return should recall the
   seed and change its ending. Adding more tracks alone does not count as melodic
   development. Avoid a fill at every boundary or an obligatory EDM riser/drop.
7. Mix while arranging. Separate bass/kick envelopes; keep sub weight near the
   center. Use a short near space and a darker, filtered long space for depth.
   Let arrangement create room before adding corrective EQ. Acoustic presets are
   demo substitutes for live players, not evidence of live-performance realism.

For the compound-meter lane, count six eighths as two groups of three. Preserve
that phrasing through melody, bass and percussion. Do not paste a 4/4 backbeat
under it. A 3/4 reading is an alternate interpretation to audition, not an alias
for 6/8.

## Tools and iteration

Prefer the user's installed production DAW, instruments and editable session.
If absent, use an available renderer. For daw-ai, read
[references/daw-ai.md](references/daw-ai.md). Never silently replace live edits.
Keep original assets, an editable session, native render and a delivery copy.
Use gain and modest fades for delivery; retain raw diagnostics and processing
settings. Check decoded delivery audio for overshoot after lossy encoding.

Use `scripts/audio_probe.py` for reproducible signal descriptors (numpy and
ffmpeg required). It reports unverified periodicity/change candidates, not key,
meter, instrument identification, human feel or musical quality. Full-file
measurements and equal-duration active excerpts answer different questions.

Render → inspect → audition when possible → revise one coherent musical issue.
Compare against references at equal playback loudness using similar musical
roles, not merely equal clock time. Do not compress a sparse passage to imitate
the loudness of a commercial climax. Prefer subtraction or new articulation when
a development becomes congested. Retain the previous take and why it changed.

Deliver audio, editable project and a concise evidence report. Separate technical
pass/fail, musical hypotheses, and human ratings. A demonstration by the author
shows executability; it does not establish independent skill effectiveness or
parity with Bonobo. Ask for focused listener feedback after the artifacts exist.

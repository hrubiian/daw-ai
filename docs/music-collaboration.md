# Musical collaboration workflow

## Compose, render, revise

1. Turn the brief into a testable musical intent. Describe who carries the
   pulse, who carries the melody, and what changes by the end of the phrase.
   Write an original motif of two to four bars and an answer. Save the
   pitches, chord movement, rests, and phrasing in editable clips.
2. Make an eight- to sixteen-bar passage. Give each part a job. Establish the
   pulse with a few strong anchors and use deliberate accents, note lengths,
   and timing for weaker notes. Check kick and bass with `compare_groove` if
   the intended relationship is unclear. Use a dry render when an audible
   attack might come from an envelope or echo.
3. Render the passage with `start_listen` and keep its job ID. If your MCP
   client passes audio to your model, use `audition_excerpt` to investigate a
   specific moment. A client may display the excerpt to the user without
   passing sound to the model. In that case, use the render as a listening
   artifact and ask the musician what they hear.
4. Form one hypothesis and make an alternative from the same passage. For
   example, give the flute the second half of a sung phrase. Keep tempo,
   source assets, render bars, and unrelated parts stable. Use
   `prepare_audition` with a question the listener can answer. Present both
   clips as A and B and keep the assigned revisions out of the question.
5. Call `record_audition_choice` only after the listener responds. Use their
   choice and wording, including a tie or neither. Keep the preferred
   revision, or refine the specific problem. Try the next agreed alternative
   before expanding the arrangement. Keep revision hashes and audio so an
   edit can be traced back to the listening result.

### Listening tools

`audition_excerpt` takes a completed render ID, `offset_seconds` from the start
of the render and `duration_seconds` up to six seconds. It returns mono PCM WAV
audio. For stereo balance and spatial detail, play the full WAV or a listening
pair. The legacy `listen` tool reports numerical measurements.

`prepare_audition` takes `first`, `second` (completed render IDs), an
`offset_seconds`, a `duration_seconds` from 5 to 30, and a `question`. It
requires FFmpeg. The clips receive identical short fades at their edges. The
louder clip is attenuated and both clips are lowered as needed for headroom.
The tool remeasures the stereo WAVs, requiring a difference of at most 0.5 LU
and true peaks at or below −2 dBTP. Files and source mappings live in the
project's `auditions/` directory. The listener sees randomized A/B labels.

`record_audition_choice` accepts the audition `id`, `preference` (`a`, `b`,
`tie`, or `neither`), and the listener's optional `comment`. It logs the vote
alongside the render IDs and project hashes. Votes help choose revisions in
this project and do not update a model's weights.

## Research and evaluation

YuE2 reported that experts preferred score-first generation over generation
without symbolic planning in a comparison using the same checkpoint. Its
audio rendering stage is a learned model trained for music. In daw-ai we
can apply the score-first idea and retain edits, while the instruments and
performances still determine what the score sounds like.
[YuE2 technical report](https://arxiv.org/abs/2609.33757)

MusicRL reports that human pairwise preferences improved a music generation
model's listener appeal. Music Arena collects anonymous pair choices with
listener comments. Studies of automatic music metrics find weak agreement on
some human judgments. For daw-ai, these results support comparable listening
examples and measured feedback, with beat, loudness and clipping checks used
for specific production problems.
[MusicRL](https://arxiv.org/abs/2402.04229) · [Music Arena](https://arxiv.org/abs/2507.20900) ·
[Music metric preference study](https://arxiv.org/abs/2506.19085)

Music Flamingo demonstrates specialized audio understanding across harmony,
structure, and timbre tasks. A listener model can offer a second opinion if
it receives the sound, but its reliability for judging a performance or mix
needs to be tested on those decisions. Check observed playback before
attributing a timing problem to either the score or the performer.
[Music Flamingo](https://research.nvidia.com/labs/adlr/MF/)

To measure progress toward human work, use fresh briefs across styles and
equivalent instrument and time budgets. Compare the workflow with and without
iteration before comparing it with musician-made references. Randomize pair
order, match listening level, collect judgments from more than one musician,
and track groove, phrasing, development, timbre, and overall preference. A
single local A/B verifies the workflow and supplies one musical observation.

## Performance notes

The musician's recorded idea and listening feedback should guide development.
When starting from scratch, create an editable 8–16-bar performance before
expanding it into a song. Preserve each accepted version and its audio artifact.

A first passage should establish a groove, a melodic intention and space for an
answer. Write dynamics deliberately: strong arrivals, quieter inner voices,
ghost notes and pickups that lead to an accent. Vary articulation with note
length, repetition, register and rests. Intentional timing can be authored in
note ticks; turn global random humanization down when the phrasing is already
specified. Repeating a pattern while adding layers is only one arrangement
technique, not a default substitute for development.

Use the shared session for live edits and undo. Read the current revision before
changing notes so musician edits are retained. Use granular musical edits or a
validated transaction rather than blindly replacing an old project snapshot.
Label accepted versus engine-applied changes accurately. A late agent response
should not be treated as a real-time note scheduler.

Sampler pads can carry soft/hard velocity layers and alternate takes through
set_sampler_variants. Different attacks and articulations are preferable to
turning down one identical sample for every ghost note. Recorded performances
can supply those takes; synthesized variants remain synthesized sounds. The GUI
exposes these choices in FX and supports velocity adjustment on notes/steps.
SoundFont/CLAP instruments still depend on their own sample/articulation quality.

Render an excerpt with start_listen and let the musician audition it. If making
an A/B comparison, identify whether it tests one variable or a complete musical
rewrite, and use comparable delivery gain without flattening the dynamics.
Analysis can identify clipping, gross level imbalance and other technical
problems; it cannot verify feeling or musical quality. Record only the user's
own ratings. Ask for focused feedback on groove, phrasing or timbre, then make
one coherent revision.

Current limits include persisted sustain/expression curves, tempo maps, plugin
transport/latency support and hardware MIDI timing qualification. These should
be addressed through specific musical use cases and playback tests. A framework
rewrite alone does not solve performance or composition quality.

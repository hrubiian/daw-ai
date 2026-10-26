# Musical collaboration workflow

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

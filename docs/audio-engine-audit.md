# Audio engine and expressive collaboration audit

Reviewed 2026-09-23. This is a source review plus automated regression testing, not certification of every audio interface or third-party plugin. The intended product is a musician and an agent editing one live session without losing phrasing, sounding notes, project state, or honest feedback about what reached the engine.

## Changes delivered in this review

| Problem | Change | Verification |
| --- | --- | --- |
| Every accepted edit rebuilt all strips and released sustained notes, including tempo, notes and fader edits. | Retain instrument instances and outstanding note releases when track order, instrument resources, tuning and plugin routing are unchanged. Send changed gain/pan/mute/effects controls to existing strips. Structural changes still rebuild. | Sustained-edit regression checks retained releases, absence of resource reloads, changed controls and a safe rebuild after track reorder. |
| Unchanged sample buffers were reloaded on edits; deleting the last audio clip left its buffer table installed. | Cache resource specifications; send an empty table when cleared. Reconcile active clip voices when clip scheduling or strip structure changes. | Last-clip removal regression. |
| Play/seek at a clip's start could arm it and then trigger it again in the sequencer tick. | Only arm clips that started strictly before the playhead; tick dispatch owns exact starts. | Boundary regression requires exactly one trigger. |
| Offline piano-roll playback ignored legato. | One note-duration function shared by live and export, including notes belonging to the same chord group. | Chord-group duration test and rendered CLAP tone remains audible beyond its short authored duration. |
| CLAP input event storage could grow in the callback; panic replaced and dropped a held-note vector there. | Preallocate 512 input events, bound insertion, remove dynamic held-key tracking, and prioritize a bounded all-key release on note-off overflow. | A sounding CLAP fixture is flooded with events and still becomes silent on release; overflow is counted. |
| Saved-project parameter edits were ignored for an existing CLAP instance. | Diff saved parameter values and deliver changes at the edit timestamp; removing an override restores the declared default. | Parameter-diff/default assertions alongside the working gain-plugin regression. |
| Unused plugin output events were accumulated in a growable callback buffer. | Use the library's void output-event sink, preserving the existing discard behavior without storing those events. | Existing instrument/effect and state regressions. Output MIDI/automation capture remains unsupported. |
| Plugin processing errors were printed from the callback and export could report success with missing plugin sound. | Count failures without callback logging, silence failed blocks, latch processor failure, fail an offline render when a processor fails, and expose plugin failures/overflows in UI and session diagnostics. | Build and existing plugin regressions; a dedicated process-failure fixture remains desirable. |

A retained note completes its previously scheduled release after an ordinary edit. Editing/deleting its future pattern does not retroactively truncate that already sounding note. Mute, pause and panic remain explicit release/silence actions. Changing an instrument, tuning, plugin routing, track count/order, or legacy audio-file resource can still rebuild and interrupt voices; seamless structural graph replacement is not implemented.

## Existing timing work retained

Sequenced events use device-frame timestamps and bounded look-ahead. Callback and offline rendering split at event positions. Transport generations suppress obsolete performance events after pause/seek; project-control acknowledgements survive. Device-buffer tests preserve sampler attacks across 128-, 257- and 1024-frame callbacks. Live and offline note placement share deterministic timing humanization.

These guarantees concern event dispatch. They do not imply that every instrument produces its waveform at the exact same sample: the installed RustySynth renders internally in 64-frame blocks and buffers the unused part. That is up to roughly 1.45 ms of additional control granularity at 44.1 kHz. Changing its block size requires CPU and sound-quality measurements; no unmeasured setting change was made here.

## Remaining gaps, in delivery order

### P1: Performance and collaboration correctness

1. **Prepare structural edits before their musical deadline.** `sequencer/engine.rs::apply_project` and the relay in `daw-io/src/host.rs` still build changed instruments/assets on the event path. A slow SoundFont/plugin load can delay queued musical events. Use a prepared graph transaction, readiness/error response, and scheduled atomic adoption, with reconciliation if the source revision changes during preparation. Test slow loads while drums continue.
2. **Give repeated notes independent identities.** `audio/mixer.rs` forwards pitch-only note-offs; RustySynth releases voices sharing channel/key, while CLAP notes use a wildcard note ID. Overlapping repetitions can cut the later note. Add explicit note lifetimes and backend-specific policies; test repeated piano notes, chord repetitions and sustain before selecting a policy. A reference counter alone would change articulation rather than provide independent releases.
3. **Timestamp live MIDI and persist expression.** `daw-io/src/midi.rs` feeds immediate events; `sequencer/record.rs` records note pitches/durations. Direct input still lands on callback boundaries, and CC64 sustain, expression and pitch curves need a persisted timeline and replay contract. CLAP instrument routing also needs supported MIDI/expression events, not only native note events. Test hardware timestamp mapping, sustain across edits, pedal-up, and panic.
4. **Implement plugin transport and latency compensation.** `audio/clap.rs` currently passes no transport object to process, ignores restart/callback requests, and does not compensate plugin latency. Tempo-synced effects and parallel dry/wet routing cannot be assumed correct. Add transport, lifecycle handoff and reported-latency alignment with purpose-built fixtures. Full state-blob replacement of an already running instance is not reconciled; restoring declared parameter defaults is not a substitute for restoring a saved state blob. Same-UID replacement also needs generation-safe ownership of the old handle until its processor is retired.
5. **Add shared meter/tempo-map support.** The project validator deliberately accepts only 4/4 and a single tempo; pattern timing uses 480 ticks per bar. Jazz waltz and changing meters require a shared tempo/meter map through arranging, looping, recording, export, UI and tools. Preserve old project compatibility; test 3/4 and 5/4 before enabling them in validation.
6. **Smooth live parameter changes.** Faders/pan and filter coefficients change immediately. Add bounded ramps and coefficient/state handling with discontinuity tests; preserve transients and do not change sample playback gain semantics silently.

### P2: Realtime operation and diagnostics

- Sampler and clip voice vectors can still grow in `audio/mixer.rs`; ended clip voices hold reference-counted buffers whose final release may happen in the callback. Use reserved voice pools, documented stealing policies and off-thread retirement. Validate dense sustained chords and long overlapping clips.
- The GUI capture path has reserved scratch storage but oversized buffers can require growth; the legacy recorder uses locking/growing storage. Audit both recording paths and provide bounded overflow reporting.
- Callback segmentation improves event precision but increases plugin process calls. Benchmark dense chord passages, CC streams and plugin chains at 44.1/48/96 kHz and multiple device buffer sizes. Bound work without silently discarding releases.
- Diagnostics are cumulative, process-wide counters. Add per-device/processor attribution, session baselines and a user-visible recovery/reset action; a historical overflow should not be mistaken for a currently failing edit.
- Qualify every backend with a realtime allocation detector and a long-running stress test. Current unit tests and short live checks do not prove hard realtime operation.

### P3: Musical evaluation and human–agent interaction

- Build a reproducible jazz fixture suite: swung ride and hats, velocity-shaped ghost notes, walking bass, dense extended chords, overlapping repeated notes, pedal-held piano, rubato/tempo changes and a jazz waltz once meter support lands.
- Compare live capture to offline output with declared device/plugin latency, not only waveform peaks or successful command acknowledgements. Keep sampler scheduling tests separate from synthesis/backend equivalence.
- Preserve revision-linked renders, previews and shared undo; label accepted, prepared, scheduled, applied and failed states distinctly. A saved project is not proof that a change was heard.
- Add in-app A/B audition and explicit musician ratings. Timing correctness is measurable; an attractive jazz feel, timbre and arrangement still require listening and musical judgment.

## Qualification gate

Before describing this as a dependable DAW engine, require: no unbounded callback allocation in the supported path; release-safe overload behavior; timestamped MIDI and pedal/expression replay; plugin transport/latency tests; prepared structural swaps; recording latency calibration; supported-meter regressions; and a sustained hardware stress run with attributable underrun/late-event data.

Run `bash scripts/check.sh` for the checked-in build, protocol, workspace and Clippy checks. The required-assets CI job also sets `DAW_REQUIRE_TEST_ASSETS=1`. The new CLAP export regression requires the workspace's test-plugin libraries, which the check script builds first.

## Expressive sampler follow-up — 2026-10-13

Sampler pads now support validated velocity ranges and deterministic alternate
sample takes. Selection happens in the shared mixer for sequenced/live input
and offline export. Resources are loaded outside the callback; sounding voices
retain their take by index. Sampler voice storage is preallocated and bounded to
128 per strip, with observable oldest-voice stealing on overload. This addresses
the sampler voice-vector growth item above; clip voices and recording-path
allocation concerns remain. Project asset resolution and fingerprints include
all takes, and invalid ranges do not mutate a valid pad/project.

The GUI exposes soft/hard take loading and per-note/step velocity adjustment;
MCP exposes validated set_sampler_variants. Tests check timbral selection,
rotation across different velocities, overlapping hits, fallback, panic reset,
bounded voice storage, legacy serialization, asset identity, invalid edits,
within-render sample amplitudes, range/full equivalence and failed take loading.

This improves the instrument's expressive range. It does not establish natural
musical phrasing, solve expression/sustain/tempo-map gaps, or make synthetic
samples sound like recorded acoustic performances. Those remain separate work.

Validation: the full repository check completed with 262 passing tests and four
explicitly ignored tests, including 180 core, 44 MCP and 19 GUI tests. Build,
wire-protocol discovery and Clippy completed (existing warnings). No hardware
latency certification or subjective musical rating is claimed.


## Click-free mixing, working LFOs and note-length samples — 2026-11-11

| Problem | Change | Verification |
| --- | --- | --- |
| Fader, pan and mute changes jumped in one sample (a click on every live edit, and on every agent mix move during playback); mute was a hard cut. | Each strip glides its left/right gains to the fader/pan/mute target (about 15 ms). A new strip starts at its fader with no fade-in; a fully faded muted strip is skipped as before. | `fader_mute_and_pan_changes_glide_instead_of_stepping` bounds the per-sample step and checks the exact settled value. |
| Cutoff LFOs did nothing: they were sent as CC74, which the SoundFont synth ignores, and modulators were skipped entirely on sampler and audio tracks (CLAP tracks dropped them silently). | LFO output is a mixer event (`AudioEvent::Modulate`). Cutoff drives the strip's insert low-pass on every strip kind; volume and pan replace the fader/pan on synth-less strips. SoundFont strips keep their CC7/CC10 response. The filter coefficient glides (about 30 ms). | `cutoff_lfo_sweeps_the_insert_filter…`, `lfo_modulation_reaches_synthless_strips_and_clears`, and the render test `cutoff_lfo_moves_a_sampler_track_in_a_render`. |
| Removing a modulator left the strip at the LFO's last value until a rebuild. | The sequencer clears a destination when its modulator disappears; SoundFont strips return to GM defaults (volume 100, pan centre). | Clear path covered by the two mixer tests above. |
| Live LFOs updated every 8th note, so anything faster than a slow breath stair-stepped and differed from the export. | Updated every 32nd note; the mixer's glide smooths between values. | Source review; export still updates per 1024-frame block. |
| Sampler pads ignored note-off, so long sounds needed one file per length. | Optional per-pad `gate`: note-off releases the oldest held voice of that pitch over about 30 ms. Default stays one-shot. Live and export share it. | `gated_pad_releases_on_note_off_and_one_shots_ignore_it`, `gated_pad_follows_the_note_length_in_a_render…`. |

This closes P1 item 6 (smooth live parameter changes) for fader, pan, mute and
filter cutoff. EQ, compressor, drive and delay-time edits still apply at once.
A volume LFO on a SoundFont track still multiplies the fader through the
synth's CC7 curve, unlike the other track types; unifying that would change
the level of existing songs, so it was left alone. Not done here: note
identities for overlapping SoundFont/CLAP repeats, timestamped MIDI input,
plugin transport and latency compensation, and tempo/meter maps.

Validation: `cargo test --workspace --offline` passed. No hardware listening
test was run; the changes are verified by sample-level assertions only.

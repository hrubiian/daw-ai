# Desktop UI review

Scope: the GPUI frontend in `crates/daw-gpui`, reviewed against its source and a
screenshot of the running app.

## Findings and changes

| Finding | Change in this pass |
| --- | --- |
| A 680 px track header competes with the timeline, especially at the default 1280 px window width. | Two rows of controls in a 460 px header reclaim 220 px horizontally. Rows are 72 px tall to accommodate both control groups. |
| Track headers omit the 8 px loop lane above the timeline ruler. | A shared loop-lane height is included in the header spacer and vertical viewport calculation. Track and clip rows also explicitly resist flex shrinking. |
| Scrolling right progressively hides clips while later ruler bars remain visible. | Headers, ruler and lanes now use stationary clipping viewports with explicitly sized, absolutely positioned scrolling documents. Moving a relative flex container also moved its nested clip mask; the initial width-only change did not resolve the reported scrolling failure. |
| Secondary text is difficult to read against dark panels. | Raised secondary text brightness while retaining the graphite palette and track colors. |
| Abbreviated buttons and raw volume/pan values require prior knowledge. | Added hover help, percent volume, Center/L/R pan labels, readable add/import labels, and full-name tooltips. |
| Dense fixed-height toolbars can place controls outside a narrow window. | Added horizontal scrolling and prevented pill labels from wrapping. |
| Search with no matches produces an unexplained blank area. | Added a no-results message with search examples and the existing Esc reset action. |
| Idle status area provides no navigation guidance. | Added timeline scroll and zoom hints when there is no status message. |
| GUI README still described the initial transport-only prototype. | Updated the feature overview and interaction guide. |

## Recommended next work

1. **Keyboard workflow and focus.** The root GUI handler currently provides undo
   and redo, but does not provide the TUI's Space transport or Ctrl+S save
   shortcuts. Add GUI actions and keybindings with explicit input contexts so
   typing in browser search, rename or code fields cannot trigger global edits.
2. **Adaptive dock layout.** The browser, track headers and mixer still have fixed
   or manually resized widths. Add collapsible side docks and a minimum timeline
   width. Scrollable toolbars are an interim improvement; grouped menus would
   reduce the need for horizontal scrolling.
3. **Instrument browsing.** Category, preset and search result lists use clipped
   containers. Give long lists bounded scrollable regions, keyboard selection,
   and a visible indication of which track will receive the instrument.
4. **Project state and feedback.** Add a dirty-project indicator and consistent
   save/export progress and error presentation. Current status messages share a
   single narrow text area.
5. **Component boundaries.** Separate transport, arrangement, browser, editor and
   theme code from the large `app.rs` so interaction rules and sizing conventions
   can be maintained consistently.

## Validation

`cargo check -p daw-gpui --offline` passed during implementation. Final validation
also checks the crate's test targets with `cargo check -p daw-gpui --tests --offline`.
The scrolling regression test uses GPUI’s headless test window to inspect paint
bounds and masks for 18 tracks and 40-bar clips across horizontal/vertical
scrolls, viewport sizes and zoom levels. Run it with
`cargo test -p daw-gpui --lib --offline` (10 tests passed). It does not replace native desktop
visual acceptance. Existing unused-variable/dead-code warnings remain outside
this pass.

Native desktop UI automation is unavailable in this session. Run `./run-gui.sh`
for visual acceptance; the following checks remain manual:

- At 1280×800 and a narrower window, reach the last transport and song-toolbar
  controls by horizontal scrolling; confirm labels remain on one line.
- With multiple tracks, compare header and clip boundaries, then resize a track,
  scroll vertically and resize the editor dock. Rows should remain aligned.
- Verify mute/solo, volume/pan, track type, pattern creation, remove/undo and
  double-click rename after the controls' relocation.
- Hover abbreviated controls and long track/instrument names.
- Search for an unmatched term, clear with Esc, and apply a matching preset.
- Check normal and full arrangement modes with the mixer open and closed.
- Open a song with 40-bar clips and scroll to bar 17, then bar 30.
  Its clips should continue through bar 40, with no artificial cutoff
  around bar 20 and no disappearance when scrolling farther. Repeat at different
  zoom levels and after narrowing the window; verify audio clips as well.

The narrower headers trade some vertical track density for timeline space.
Very narrow windows with both side docks open still need the adaptive layout
work described above.

The local desktop shortcut launches `target/debug/daw-ai` directly. Source edits
and `cargo check` do not update that executable; build with
`cargo build -p daw-ai --offline` and restart the app to use changes.


## Follow-up review — 2026-10-13

Reviewed the current frontend source and exercised the complete GPUI view in
headless tests. No current native desktop screenshot or hardware acceptance is
claimed. The previous source/screenshot review above remains historical.

Major issues addressed:

- Reserve timeline width when docks are open, collapse docks when they cannot
  fit, and cap editor height after a window resize. Add a Browser toggle.
- Replace clipped browser, mixer, sequencer, FX and pattern lists with bounded
  scroll containers. Settings now stays within the workspace and scrolls.
- Correct piano-roll resolution from quarter-note to sixteenth-note columns;
  display and edit notes between grid lines without snapping their onset.
  Index note spans once and build only nearby horizontal columns.
- Restore root focus after rename or workspace clicks, accept ordinary typing
  with or without key_char, and prevent modifier shortcuts becoming text.
  Cell buttons consume clicks before enclosing track rows can change selection.
- Clamp arrangement horizontal scrolling to the actual viewport, including
  after zoom/resize, and support horizontal trackpad deltas.
- Route the custom titlebar and compositor close request through unsaved-change
  confirmation. Display dirty state; Save & close waits for a successful save.
  New resets stale file revision and audition state; unnamed Save opens Save As.
- Run audio/MIDI device enumeration and WAV waveform reads off the UI thread.
  Failed waveform loads report errors and do not repeatedly reopen the file.
- Account for the missing-soundfont banner when positioning Audio Effects.

Added a device-free Host fixture behind the test-support feature, allowing the
actual GUI to render without audio/MIDI devices or worker threads. Integration
coverage exercises all dock tabs, File/Settings, Audio Effects, and the close
modal at 800×480, 1280×800 and 1920×1080, plus rename/transport focus, parent-row
click isolation, off-grid note deletion and failed-save preservation. These
checks complement the existing scrolling and overlay regressions.

Manual acceptance remains required for compositor drag/resize, native file
pickers, very large plugin catalogs, screen-reader/IME behavior and hardware
recording. The hand-built single-line text inputs still lack full selection,
clipboard and IME integration; this review improves their existing key handling
without claiming a complete text-editing component.

Validation: `bash scripts/check.sh` passed with 253 tests and four explicitly
ignored tests, including 19 GUI tests, MCP protocol checks, workspace build and
Clippy (existing warnings). The Unix-socket test required execution outside the
socket-restricted sandbox. The debug desktop executable was rebuilt. Changes
remain uncommitted for review.

## Dedicated Mixing workspace — 2026-10-14

Added Mixing beside Sequencer, Piano Roll, FX and Code. The transport's Mixing
button opens the same full workspace, including from full arrangement mode.
The arrangement/editor/browser remain available by switching tabs; the mixer
uses the workspace height instead of the narrow right dock. Navigation changes
UI state only and does not reload the song or reset transport.

Readable channel strips include track/instrument names with full tooltips,
volume in dB, pan, mute/solo, reverb/echo sends, live post-fader meters and an
Effects button. Controls use the existing revision-checked edit path and undo.
Levels support buttons or wheel adjustment, with Shift for fine increments.
Effects opens the existing occluding overlay for that channel without leaving
Mixing. Shared Reverb/Echo gains, mute and effects controls are included.
The master shows the actual post-limiter peak; there is no invented master-gain
control. Auto balance retains the existing background analysis and stale-result
checks; both mixer layouts share its launch method.

The master stays visible. At widths >=1100 px the returns are pinned as well;
in narrower windows they scroll with the track strips. Explicit channel-document
width plus independent horizontal/vertical scrolling makes the last channel
reachable in large projects and short windows. Tab navigation scrolls if needed.

Device-free GPUI tests exercise volume/send/return edits, undo, selection
isolation, effects navigation and playback/pattern preservation. A 23-track
fixture reaches and edits the final channel and echo return through repeated
800x480, 1280x800 and 1920x1080 resizing. The existing full-view regression now
also covers Mixing. Native desktop visual acceptance is still needed; these
checks render and click the real components in GPUI's headless window.

After rebuilding, an already running desktop window needs a restart. Save
unsaved music before closing it; source changes do not update an existing process.


## Bug pass — 2026-11-11

Found by reading the frontend source; fixed and covered by headless tests
where a test could reach them. No native desktop acceptance was run.

| Bug | Fix |
| --- | --- |
| In a live session every edit from an agent reset the selected track (and the open pattern) to track 1. | Selection is kept when its track still exists and repaired otherwise (`HeadlessState::reconcile_selection`). The same repair runs after undo/redo and JAM reloads, which could leave a stale selection, a solo of a deleted track, or a rename box for a removed item. |
| One rejected GUI edit wrote into the shared engine-error slot. The status line then showed that error forever, hiding "saved"/"exported" messages, and agents were told the engine had failed. | GUI submit errors go to the status line only. A real engine failure is shown as a prefix beside the current status rather than replacing it. |
| File → New and File → Load discarded unsaved work without asking. | Both go through the unsaved-changes dialog (Save / Discard / Cancel). Ctrl+N and Ctrl+O added. |
| After Load, Ctrl+Z put the previous song under the new file's path; Ctrl+S then overwrote the wrong file. | Opening a file starts a fresh undo history. Layout, zoom, accent and JAM survive opening a file. |
| A new project was marked "unsaved" before any edit. | It is clean until edited. |
| JAM mode: while the file on disk conflicted with unsaved edits, the poll returned early every 16 ms, so export results and recording stopped updating and the window repainted constantly. A half-written or invalid file was re-read 60 times a second with no message. | Each external write is reported once; the rest of the poll keeps running. An unreadable file says why and the sounding project is kept until the next write. |
| A recorded audio take landed on whichever track was selected when it finished. | It lands on the track it was started on; if that track was removed the file is kept and the status says so. |
| Importing an unreadable WAV still added an empty audio track and an undo step. | The file is checked first. |
| After adding or removing a track, a lit Solo button no longer matched what was heard. | The solo mask is re-applied after every project push. |
| During playback the playhead ran off the right edge of the timeline. | The timeline pages to follow it; a playhead you scrolled away from is left alone. Home returns to the start. |
| −4/+4 steps at the limit, and removing a missing track, added empty undo steps. | Undo is recorded only when something changed. |

Also added: a per-pad Gate toggle (one-shot / note length) in the FX tab's
sampler section.


## Frame cost on large songs — 2026-11-11

Measured with the ignored benchmark `frame_cost_on_a_large_song` (six tracks,
each one 100-bar clip of 700 notes, 1280×800, playing) and located with gdb
stack sampling of that benchmark. `perf` is unavailable on this machine
(`perf_event_paranoid` = 4) and gdb cannot attach to an already running app
(`ptrace_scope` = 1), so the samples come from the headless test window, not
the desktop app.

| View | Before (dev build) | After (dev build) |
| --- | --- | --- |
| Arrangement + step dock | 76 ms/frame | 9 ms/frame |
| Piano roll | 266 ms/frame | 16 ms/frame |
| Mixing | 42 ms/frame | 3 ms/frame |

- Nearly all samples were in the UI toolkit's layout and paint code, compiled
  without optimization. The dev profile now builds dependencies at
  `opt-level = 3` and workspace crates at `opt-level = 1` (line-level debug
  info kept). This is most of the gain and also speeds up the audio thread.
- The piano roll built every pitch row (about 50) for the visible columns;
  it now builds only the rows in the scrolled viewport, with spacers. Cell ids
  are integers instead of formatted strings.
- A song-long clip spent its 256 preview marks on bars far off screen; marks
  are now limited to the visible part of the clip.
- The title pill serialized and hashed the whole project on every frame to
  show "unsaved"; that check now runs at most four times a second.

Run the benchmark with
`cargo test -p daw-gpui frame_cost -- --ignored --nocapture`.
The first build after this change recompiles all dependencies (a few minutes).

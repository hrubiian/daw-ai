# Desktop UI and mixing

Select **Mixer** in the transport to open the channel strips between the
arrangement and the editor. On shorter windows, select a bottom tab to return
to the editor. Drag the divider above a panel to resize it when both are shown.
**Arrange ⛶** expands the timeline. Switching views preserves playback.

Each channel shows its name, instrument, level in dB, pan, mute/solo, reverb and
echo sends, and a live peak meter. Use +/− or the wheel over a level bar; Shift
makes fine changes. Effects… opens the selected channel's effects overlay.
Shared Reverb/Echo returns expose level, mute and effect settings. The master
shows the actual post-limiter output meter, without an unsupported gain control.
Auto balance runs the existing background analysis and rejects stale results.

The master stays visible. Wide windows also pin the returns; on narrower
screens they scroll with the tracks. Scroll sideways for additional channels
and vertically in short windows. Full names remain available as tooltips.

Hover over **Browse instruments…** in the header or press **Ctrl+K** to search.
The transport, patterns and editors scroll when they overflow. Native
window controls support minimize/maximize/close and resizing. Unsaved work is
protected by a close prompt and conflict-checked saves. Effects/menu overlays
consume pointer events instead of activating covered track controls.

See [Creative effects](creative-effects.md) for inserts, spatial controls and
CLAP limitations, and [Live collaboration](live-collaboration.md) for shared
undo, revision conflicts, diagnostics and recoverable saving.

Run `cargo test -p daw-gpui --lib --locked` for device-free render/interaction
regressions. Coverage includes control isolation, undo, effects navigation,
last-channel access after scrolling/resizing, and playback/pattern preservation.
Hardware playback depends on the selected device and compositor.
After updating the binary, save your work and restart an existing GUI process.

## Laptop keys in the Piano Roll

Open an instrument pattern in **Piano Roll** and click **Laptop keys** or press
`l`. The keys `z x c v b n m` play C4 through B4 and `q w e r t y u` play C5
through B5. The sharps are `s d g h j` and `2 3 5 6 7`.

Each press previews a note and enters it at the highlighted sixteenth-note step.
Click a grid column or use **←** and **→** to move the entry cursor. Press `l`
or **Esc** to return to mouse editing.

## Track comments

Right-click a track header and choose **Add comment** to leave a note about the
whole track. Right-click a clip or an empty spot in the arrangement to pin the
comment to the bar under the pointer. Each location holds one comment. Click the
speech bubble to edit or delete it. **Enter** saves and **Shift+Enter** adds a
line break.

Comments are saved in the project. A connected MCP agent sees them in
`project_summary`. Ask it to read the comments when you want it to act on them.

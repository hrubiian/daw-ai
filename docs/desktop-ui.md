# Desktop UI and mixing

Select **Mixing** beside Sequencer, Piano Roll, FX and Code, or use the Mixing
button in the transport. The console fills the workspace. Switching tabs does
not reload the song or reset playback.

Each channel shows its name, instrument, level in dB, pan, mute/solo, reverb and
echo sends, and a live peak meter. Use +/− or the wheel over a level bar; Shift
makes fine changes. Effects… opens the selected channel's effects overlay.
Shared Reverb/Echo returns expose level, mute and effect settings. The master
shows the actual post-limiter output meter, without an unsupported gain control.
Auto balance runs the existing background analysis and rejects stale results.

The master stays visible. Wide windows also pin the returns; on narrower
screens they scroll with the tracks. Scroll sideways for additional channels
and vertically in short windows. Full names remain available as tooltips.

The browser, transport, patterns and editors scroll when they overflow. Native
window controls support minimize/maximize/close and resizing. Unsaved work is
protected by a close prompt and conflict-checked saves. Effects/menu overlays
consume pointer events instead of activating covered track controls.

See [Creative effects](creative-effects.md) for inserts, spatial controls and
CLAP limitations, and [Live collaboration](live-collaboration.md) for shared
undo, revision conflicts, diagnostics and recoverable saving.

Run `cargo test -p daw-gpui --lib --locked` for device-free render/interaction
regressions. Coverage includes control isolation, undo, effects navigation,
last-channel access after scrolling/resizing, and playback/pattern preservation.
Hardware playback and native compositor behavior still need desktop acceptance.
After updating the binary, save your work and restart an existing GUI process.

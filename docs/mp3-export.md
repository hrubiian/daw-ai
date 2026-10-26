# MP3 export

Choose **File → Export MP3** in the desktop app. The complete song is rendered
through the shared mixer, respecting track mutes and retaining instrument
inserts and reverb/echo tails, then encoded as a stereo **320 kbps MP3**. The app stays usable
while exporting. Choosing either Export action again requests cancellation.

The result is saved under the project's `renders/<unique-id>/export.mp3`; the
status bar shows the full path. A project snapshot and a report containing source
hashes, format, bitrate and gain policy are saved alongside it. Unsaved projects
use the app's working directory as the base. **Export WAV** remains available.

In the terminal interface, press **Ctrl+E** and enter a filename ending in `.mp3`.
A `.wav` extension or no extension selects WAV, as before.

For an export without opening either interface:

```sh
target/debug/daw-ai song.daw.json --export-mp3 "Song title.mp3"
```

Project and destination paths resolve against the launch directory. Audio
devices are not initialized for this command. Relative sample paths resolve
using the usual project asset rules.

MP3 export requires **FFmpeg with libmp3lame** on the app's PATH. Availability is
checked before rendering, and a missing encoder produces an actionable error.
WAV export does not require FFmpeg.

The renderer uses the existing peak normalization, followed by at most a
constant attenuation to a -2 dBFS PCM peak ceiling for MP3 encoding headroom.
No additional compressor or limiter is applied by the encoder wrapper. This is
not loudness normalization or a guarantee of decoded true peak for every song.
MP3 is lossy; keep the project or WAV when retaining an editable master.

Encoding stages files in a unique directory beside the destination. Only a
completed, nonempty MP3 is renamed into place. A failed or cancelled encode
retains any existing destination and removes its staging files. Cancellation
also stops/reaps the encoder process; the render's existing timeout applies.

Encoder tests decode the produced MP3 to verify stereo separation, duration,
quiet effect-tail retention and gain, and cover cancellation, invalid audio,
missing encoders and staging cleanup. The desktop interaction test selects the
MP3 action and cancels it through the existing WAV action.

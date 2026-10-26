# Reference study — 18 October 2026

## Evidence boundaries

The initial experiment uses Apple public album previews of Kiara (track
416292278), Cirrus (611171963), and Black Sands (416292289), each approximately
29.98 seconds decoded. Their album positions are unknown. No full-track audition
or subjective audio evaluation was performed. Exact keys, section maps and
instrument identities were not estimated from these previews.

Primary sources and public album pages:

- [Ninja Tune: Black Sands](https://ninjatune.net/release/bonobo/black-sands)
- [Bonobo's Tape Notes session and transcript](https://tapenotes.co.uk/project/tn144-bonobo)
- [NPR interview](https://www.wunc.org/npr-music/2013-04-05/bonobo-challenging-musics-borders-finding-a-new-frontier)
- [Artist interview: source sound and mixing](https://www.musicradar.com/news/tech/bonobo-on-making-music-alone-and-the-power-of-software-607467)
- [Kiara album track](https://music.apple.com/us/album/kiara/416292276?i=416292278)
- [Cirrus album track](https://music.apple.com/us/album/cirrus/611171778?i=611171963)
- [Black Sands album track](https://music.apple.com/us/album/black-sands/416292276?i=416292289)

## Reported production evidence

Ninja Tune describes Kiara as combining a string refrain, strong beat, deep
synths and cut vocals. Its description of the title track emphasizes a melancholy
waltz and swelling horns. These are label descriptions, not our listening notes.

In Tape Notes (01:09–01:11), Green explains that Cirrus joins recorded kalimba
edits to the transients of a sampled drum break, then adds a strong synth bass.
For Black Sands (01:27–01:32), he describes a nylon-guitar 6/8 seed, upright bass,
mandolin, piano and a melody later played by clarinet; recorded takes are looped
as a collage. He also describes supplementing MIDI strings with live violin.
Elsewhere in the episode (00:12), he discusses manually placing swing and using
different loop lengths. That passage concerns Expander, so it is evidence of a
broader working technique, not a transcription of the three selected tracks.

The NPR interview reports that decaying thumb-piano notes contributed to Cirrus.
In MusicRadar, Green describes mixing during composition and getting recorded
instruments right at the source.

## Measured excerpt observations

One run of the bundled probe, decoded to 22,050 Hz stereo:

| Preview | RMS dBFS | Crest dB | Power centroid Hz | Strongest periodicity candidate |
|---|---:|---:|---:|---:|
| Kiara | -11.80 | 11.98 | 357 | 97.5 BPM |
| Cirrus | -13.49 | 13.69 | 318 | 120.2 BPM |
| Black Sands | -13.97 | 13.49 | 338 | 178.2 BPM; 87.6 also strong |

These are compressed preview measurements. Centroid is power weighted and
dominated by bass; it is not a timbre rating. Periodicity is onset-flux
autocorrelation within 60–180 BPM, with octave/metrical ambiguity. In particular,
the Black Sands candidates do not establish a 4/4 pulse. AAC decoding can exceed
0 dBFS without proving clipping in the source master. Do not prescribe these
values as universal targets or average the three tracks.

## Compositional hypotheses to test

| Case | Decision to transfer | Useful test | Likely failure |
|---|---|---|---|
| Kiara | A clear small motif above a strong broken-beat foundation | Can the motif be recalled after a contrasting answer? | Constant four-bar loop with decorative strings |
| Cirrus | Timbre and rhythm generated from the same plucked material | Does muting the secondary figure reveal a stable groove? | Unrelated arpeggios occupying every subdivision |
| Black Sands | Compound-meter acoustic phrases assembled into a developing ensemble | Do bass and melody articulate two groups of three? | A swung 4/4 drum pattern relabeled 6/8 |

These proposed decisions need audition. The initial F-minor main piece uses the
first two ideas; the separate compound-meter study exercises the third. Neither
attempts to recreate an original recording. More complete source audio is needed
to validate whole-track arrangement rules.

## Listener feedback from the palette follow-up

On 18 October 2026, the user compared the same eight-bar score with (A) the
original sources, (B) recorded VCSL drums/kalimba/vibraphone and Surge XT bass,
and (C) the B palette with Surge XT delays, darker reverb and gentle saturation.
Active playback loudness was matched within 0.02 LU. The user's exact feedback
was: "C sounds much better".

For continuing this user's experiment, retain C as the preferred production
baseline. Its editable session is `projects/bonobo_sound_palette/C.daw.json` in
the original daw-ai checkout, with source/effect settings and comparison evidence
in the neighboring report. Preserve the reference audio and baseline take when
making further changes. These project files are local artifacts and may not
exist in another environment; inspect availability before using them.

This supports a preference for the complete C result under this comparison.
It does not isolate which effect helped, establish B over A, quantify groove or
fluency, or demonstrate resemblance or parity with Bonobo. No numerical rating
was supplied. Do not turn this listener's choice into a universal requirement
for these plugins or these exact settings.

The same listener later specified that C had better space, instrument tone,
drums, echo and overall sound, but called the bass fuzzy and unclear and the
kalimba too loud and pure, asking for decoration and more instruments. When
continuing this user's piece, preserve the liked production features while
testing clearer bass articulation, a quieter/decorated motif and selective
supporting parts. Save candidate revisions separately: these requests do not
mean the listener has accepted a new revision. The exact form text is retained
in the original checkout's `projects/bonobo_sound_palette/listener-feedback.json`.

The listener accepted the subsequent clearer-bass, quieter-kalimba revision D
with "yes, much better, add violin and flute". D is the preferred baseline when
continuing this piece. The requested E candidate adds sampled violin and flute
phrases while retaining D's existing eight tracks; E is not yet accepted. Read
the local experiment's latest feedback before assuming an earlier preference
still applies.

The listener then asked to make the piece sound like the title track, Black
Sands. Candidate F, `projects/bonobo_sound_palette/F.daw.json` (Tideglass), changes
the composition's foundation: original nylon-guitar picking in 6/8, recorded
upright bass, a clarinet theme, violin and flute answers, and late horn swells.
It is a 96-second original with a separate 6/8 MIDI score; the renderer packs
48 musical measures into 36 fixed 4/4 container bars. It uses a recorded FreePats
classical-guitar soundfont and VSCO Community Edition acoustic samples, all CC0.
The electronic bass and kalimba hook are absent from this candidate. This tests
a different musical branch, rather than treating more effects as the solution
to every reference mismatch. F has not been accepted or rated by the listener.

When the requested reference changes from an electronic, broken-beat passage to
the acoustic title track, reassess meter, instrumental roles, melodic phrasing
and development before inheriting the previous arrangement. Preserve the user's
accepted take separately. Neither a different palette nor correct meter alone
establishes resemblance or musical quality.

The listener subsequently rejected F's violin as "creepy". Do not infer that
the acoustic direction or other instruments were rejected. Candidate G tests
softer recorded violin-section sustains, lower chord-aware phrasing, restrained
upper mids and less reverb; H tests recorded pizzicato with the same revised
score. Both preserve all other F parts. Neither has been accepted yet.
Inspection also found an unintended F-sharp in F's violin answer over D-flat:
fixed two-semitone subtraction had produced an unplanned chromatic tone. Check
generated answers against the intended chord and scale, while permitting
deliberately authored chromatic notes. Sampled vibrato, articulation and phrase
joins need evaluation before adding more ambience to address a timbre complaint.

The listener next identified detached note playback and requested applying the
phrase-connection approach to all recorded instruments, including clarinet.
Candidate J (`projects/bonobo_sound_palette/J.daw.json`) extends the authored
connections across clarinet, flute, bowed violin and three horn voices while
retaining phrase rests, note pitches/onsets and the plucked/percussion parts.
Joins and source-attack offsets are instrument-specific. This is a pre-rendered
crossfaded-sustain approximation with retained source/score metadata, not a new
native legato editor or recordings of real interval transitions. The request
authorizes the continuation; it is not a numerical quality score or acceptance
of J. The local listening page is `ensemble-phrasing.html`.

The listener then requested a more natural violin timbre. K replaces J's
section recordings with SSO Solo Violin 2 non-vibrato recordings, one for each
of the eight sounding pitches, with only small measured tuning corrections.
Stereo source detail and connected phrasing are retained; all other instruments
are unchanged. This source has its own Sampling Plus license, retained locally,
and must not be described as CC0. Naturalness has not been listener-verified.
Compare `solo-violin.html` and inspect `revision-K.json` for the actual changes.


The listener next requested room/hall reflections around the recorded instruments.
L tests two measured-response convolution returns while preserving K's notes,
phrasing, source recordings, direct-track faders, pans and EQ. A short room and
quieter, darker concert hall receive instrument-specific post-fader sends; bass
and percussion receive less than bowed strings and winds. Existing algorithmic
reverb is disabled to avoid stacking incompatible spaces. Remove the direct
arrival from parallel convolution returns, retain a little predelay for clear
attacks, and leave enough output length for the decay. Judge the result at
matched loudness and compare solo exposed phrases as well as the ensemble.

This trial uses FFmpeg afir, with frozen wet audio tracks in the native project;
it does not add a live convolution plugin. Note/timing edits require regenerating
those returns. The listening page `room-hall.html` also allows independent room
and hall amounts. EchoThief's original impulse responses are licensed for creating
convolved derivative audio, not relicensed as CC0 or bundled into this skill.
The captured responses are mono; added channel offsets must not be described as
recorded stereo ambience. L's spatial realism is still awaiting listener review.

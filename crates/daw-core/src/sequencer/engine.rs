use crate::audio::input::{TRANSPORT_COUNT_IN, TRANSPORT_ROLLING, TRANSPORT_STOPPED};
use crate::audio::mixer::{StripSpec, STRIP_UTILITY};
use crate::audio::AudioEvent;
use crate::project::{EffectChain, Instrument, LfoShape, ModDest, Modulator, Pattern, Project, Returns, TrackType};
use crossbeam_channel::{Receiver, Sender};
use std::collections::BTreeMap;
use std::sync::{Arc, atomic::{AtomicU32, AtomicU8, Ordering}};
use std::time::{Duration, Instant};

pub const TICKS_PER_BAR: u32 = 480;
pub const BEATS_PER_BAR: u32 = 4;
pub const TICKS_PER_BEAT: u32 = TICKS_PER_BAR / BEATS_PER_BAR; // 120

pub enum SequencerCommand {
    Play,
    Pause,
    /// Jump the playhead to an absolute tick (wraps into song length). Used for
    /// transport Stop (→ 0) and click-to-seek on the timeline.
    Seek(u32),
    SetBpm(f64),
    SetSwing(f32),
    LoadProject(Project),
    /// Like LoadProject but keeps the current playhead position (live edit).
    UpdateLive(Project),
    SetTrackVolume  { track_id: String, volume: f32 },
    SetTrackMute    { track_id: String, muted: bool },
    SetTrackPan     { track_id: String, pan: f32 },
    SetTrackEffects { track_id: String, effects: EffectChain },
    /// Reconfigure the shared return buses (reverb/delay) live.
    SetReturns(Returns),
    /// Update the transport loop/punch region live (no project reload).
    SetLoopRegion(crate::project::LoopRegion),
    /// End the recording session: close the take gate, stop punch gating.
    RecordStop,
    /// Queue a parameter change on a live CLAP plugin instance.
    SetClapParam { uid: String, param_id: u32, value: f64 },
    PreviewNote     { track_id: String, pitch: u8, velocity: u8 },
    PreviewNoteOff  { track_id: String, pitch: u8 },
    RawCC           { channel: u8, cc: u8, value: u8 },
    SetMetronome(bool),
    /// Begin a recording take: count in `count_in_bars` bars of clicks with the
    /// playhead held, then roll. Already playing → punch in immediately.
    RecordStart { count_in_bars: u8 },
    Panic,
    Quit,
}

/// Tick offset of `step` within its pattern. Steps are always 16th notes
/// (30 ticks); offbeat 8ths (step % 4 == 2) are delayed by up to
/// TICKS_PER_BEAT/6 ticks, giving a triplet swing feel at swing = 1.0.
pub fn swung_step_tick(step: u32, swing: f32) -> u32 {
    const TICKS_PER_STEP: u32 = TICKS_PER_BAR / 16; // 30
    let base = step * TICKS_PER_STEP;
    if step % 4 == 2 {
        let max_delay = TICKS_PER_BEAT / 6; // 20 ticks → triplet ratio
        base + (max_delay as f32 * swing.clamp(0.0, 1.0)) as u32
    } else {
        base
    }
}

/// Apply deterministic velocity humanization. The jitter is a pure function of
/// `(channel, pitch, tick)`, so live playback and offline export agree and
/// renders are reproducible. At `humanize = 1.0`, velocity varies by ±35%.
/// `humanize <= 0` returns the input unchanged. Result is clamped to `1..=127`.
pub fn humanize_velocity(vel: u8, channel: u8, pitch: u8, tick: u32, humanize: f32) -> u8 {
    let h = humanize.clamp(0.0, 1.0);
    if h <= 0.0 || vel == 0 {
        return vel;
    }
    // Mix the inputs, then a couple of xorshift/multiply rounds for avalanche.
    let mut x = tick
        .wrapping_mul(2_654_435_761)
        ^ (pitch as u32).wrapping_mul(40_503)
        ^ (channel as u32).wrapping_mul(2_246_822_519);
    x ^= x >> 15;
    x = x.wrapping_mul(2_246_822_519);
    x ^= x >> 13;
    let unit = (x as f32 / u32::MAX as f32) * 2.0 - 1.0; // [-1, 1]
    let factor = 1.0 + unit * h * 0.35;
    (vel as f32 * factor).round().clamp(1.0, 127.0) as u8
}

/// Evaluate an LFO of `shape` at normalized `phase`, returning `[-1, 1]`.
pub fn lfo_value(shape: LfoShape, phase: f32) -> f32 {
    let p = phase.rem_euclid(1.0);
    match shape {
        LfoShape::Sine     => (p * std::f32::consts::TAU).sin(),
        LfoShape::Triangle => 1.0 - 4.0 * (p - 0.5).abs(),
        LfoShape::Saw      => 2.0 * p - 1.0,
        LfoShape::Square   => if p < 0.5 { 1.0 } else { -1.0 },
    }
}

/// Modulated parameter value in `0..=1` for modulator `m` at time `t_secs`,
/// oscillating by `±depth` around the static `base01` value.
pub fn modulated_value(m: &Modulator, base01: f32, t_secs: f32) -> f32 {
    let phase = m.rate_hz * t_secs + m.phase;
    (base01 + m.depth * lfo_value(m.shape, phase)).clamp(0.0, 1.0)
}

pub fn pattern_length_ticks(pattern: &Pattern) -> u32 {
    match pattern {
        // Steps are 16th notes: length derives from the count (32 steps =
        // 2 bars, 4 steps = a 1-beat loop). The serialized `bars` field is
        // back-compat only.
        Pattern::Step(p)      => (p.steps as u32).max(1) * (TICKS_PER_BAR / 16),
        Pattern::PianoRoll(p) => p.ticks_per_bar * p.bars as u32,
    }
}

/// Convert seconds to absolute ticks at `bpm` (rounded).
/// ticks/sec = (bpm / 60) * TICKS_PER_BEAT.
pub fn sec_to_tick(sec: f64, bpm: f64) -> u32 {
    let ticks_per_sec = (bpm / 60.0) * TICKS_PER_BEAT as f64;
    (sec.max(0.0) * ticks_per_sec).round() as u32
}

// One arranged audio clip, resolved to ticks for triggering. Track volume,
// pan, and mute are applied by the clip's strip in the mixer, so the job only
// carries the clip's own gain.
struct AudioClipJob {
    clip_id:    u32,
    strip:      u8,
    start_tick: u32,
    end_tick:   u32,
    offset_sec: f32,
    len_sec:    f32,
    gain:       f32,
}

// One placed block in the arrangement timeline.
struct SlotState {
    start_tick: u32,
    end_tick:   u32,
    loop_pat:   bool,   // true = pattern loops within the slot (pattern-mode auto-fill)
    pattern:    Pattern,
}

struct TrackState {
    /// Mixer strip index (= track index in the project).
    strip:    u8,
    slots:    Vec<SlotState>,
    muted:    bool,
    is_audio: bool,
    /// True when this track triggers sampler pads instead of a synth.
    is_sampler: bool,
    /// (CC number, static base value 0..1, modulator) for each LFO on this track.
    mod_jobs: Vec<(u8, f32, Modulator)>,
}

/// Static base value (0..1) a modulator oscillates around, read from the track.
pub fn mod_base01(dest: ModDest, track: &crate::project::Track) -> f32 {
    match dest {
        ModDest::Cutoff    => track.effects.cutoff,
        ModDest::Resonance => track.effects.resonance,
        ModDest::Volume    => track.volume,
        ModDest::Pan       => (track.pan + 1.0) * 0.5,
    }
}

pub struct Sequencer {
    cmd_rx:        Receiver<SequencerCommand>,
    audio_tx:      Sender<AudioEvent>,
    bpm:           f64,
    swing:         f32,
    humanize:      f32,
    playing:       bool,
    /// A live edit waiting to be applied on the next bar boundary (Strudel-style
    /// cycle-quantized eval). `None` when nothing is queued.
    pending_update: Option<Project>,
    tick:          u32,
    tracks:        Vec<(String, TrackState)>,
    schedule:      BTreeMap<u32, Vec<AudioEvent>>,
    pub playback_tick: Arc<AtomicU32>,
    metronome:     bool,
    audio_jobs:    Vec<AudioClipJob>,
    /// Count-in ticks remaining before the transport rolls (0 = not counting in).
    count_in:      u32,
    /// Stopped / CountIn / Rolling — shared with the capture callback and UI.
    pub transport_state: Arc<AtomicU8>,
    /// Playhead tick at the moment the transport last started rolling
    /// (restamped at every take-gate open: "tick where the current take began").
    pub roll_start_tick: Arc<AtomicU32>,
    /// Transport loop/punch region, mirrored from the project.
    loop_region: crate::project::LoopRegion,
    /// Monotonic take gate: odd = recording window open. The capture
    /// callback pushes samples and the writer splits WAV takes on it.
    pub take_gate: Arc<AtomicU32>,
    /// True between RecordStart and RecordStop/pause (punch gating active).
    recording: bool,
}

impl Sequencer {
    pub fn new(cmd_rx: Receiver<SequencerCommand>, audio_tx: Sender<AudioEvent>) -> Self {
        Sequencer {
            cmd_rx,
            audio_tx,
            bpm: 120.0,
            swing: 0.0,
            humanize: 0.0,
            playing: false,
            pending_update: None,
            tick: 0,
            tracks: Vec::new(),
            schedule: BTreeMap::new(),
            playback_tick: Arc::new(AtomicU32::new(0)),
            metronome: false,
            audio_jobs: Vec::new(),
            count_in: 0,
            transport_state: Arc::new(AtomicU8::new(TRANSPORT_STOPPED)),
            roll_start_tick: Arc::new(AtomicU32::new(0)),
            loop_region: crate::project::LoopRegion::default(),
            take_gate: Arc::new(AtomicU32::new(0)),
            recording: false,
        }
    }

    fn gate_is_open(&self) -> bool {
        self.take_gate.load(Ordering::Relaxed) % 2 == 1
    }

    fn open_gate(&mut self) {
        if !self.gate_is_open() {
            self.roll_start_tick.store(self.tick, Ordering::Release);
            self.take_gate.fetch_add(1, Ordering::Release);
        }
    }

    fn close_gate(&mut self) {
        if self.gate_is_open() {
            self.take_gate.fetch_add(1, Ordering::Release);
        }
    }

    /// Desired gate state at `tick` while armed + rolling.
    fn punch_wants_open(&self, tick: u32) -> bool {
        let (s, e) = self.loop_region.ticks();
        let after_in = !self.loop_region.punch_in || tick >= s;
        let before_out = !self.loop_region.punch_out || tick < e;
        after_in && before_out
    }

    /// Reconcile the gate with the punch window (armed + rolling only).
    fn update_gate(&mut self) {
        if !self.recording || !self.playing {
            return;
        }
        if self.punch_wants_open(self.tick) {
            self.open_gate()
        } else {
            self.close_gate()
        }
    }

    fn tick_duration(&self) -> Duration {
        let secs = 60.0 / self.bpm / TICKS_PER_BEAT as f64;
        Duration::from_secs_f64(secs)
    }


    fn song_length_ticks(&self) -> u32 {
        let pat = self.tracks.iter()
            .flat_map(|(_, ts)| ts.slots.iter().map(|s| s.end_tick))
            .max()
            .unwrap_or(0);
        let audio = self.audio_jobs.iter().map(|j| j.end_tick).max().unwrap_or(0);
        pat.max(audio).max(TICKS_PER_BAR)
    }

    /// Flip a track's live mute. Event gating in `fire_tick` uses `ts.muted`;
    /// the strip mute covers everything already sounding (synth voices and
    /// audio-clip voices alike) and keeps positions advancing for clean unmute.
    fn set_track_mute(&mut self, track_id: &str, muted: bool) {
        let strip = self.tracks.iter_mut()
            .find(|(id, _)| id == track_id)
            .map(|(_, ts)| { ts.muted = muted; ts.strip });
        if let Some(strip) = strip {
            let _ = self.audio_tx.try_send(AudioEvent::SetStripMute { strip, muted });
        }
    }

    /// Clear all audio-clip voices, then (re)start every clip that spans the
    /// current playhead from its correct in-clip offset. This is what makes
    /// playback produce audio when starting *inside* a clip — e.g. after the
    /// user clicks a chunk partway through the song and presses Play, the vocal
    /// clip plays from that point rather than only from its very beginning.
    /// Muted clips are still triggered; the engine silences them via its mute set.
    fn arm_audio_clips_at_playhead(&self) {
        let _ = self.audio_tx.try_send(AudioEvent::StopAudioClips);
        let spt = 60.0 / self.bpm / TICKS_PER_BEAT as f64; // seconds per tick
        for job in &self.audio_jobs {
            if job.start_tick <= self.tick && self.tick < job.end_tick {
                let into_sec = (self.tick - job.start_tick) as f32 * spt as f32;
                let _ = self.audio_tx.try_send(AudioEvent::TriggerAudioClip {
                    strip: job.strip,
                    clip_id: job.clip_id,
                    offset_sec: job.offset_sec + into_sec,
                    len_sec: (job.len_sec - into_sec).max(0.0),
                    gain: job.gain,
                });
            }
        }
    }

    /// Move the playhead to `tick` (wrapped into the song length) and drop any
    /// scheduled note-offs so the new position starts clean.
    fn seek(&mut self, tick: u32) {
        let max = self.song_length_ticks().max(1);
        self.tick = tick % max;
        self.playback_tick.store(self.tick, Ordering::Relaxed);
        self.schedule.clear();
        // Scrubbing *during* playback re-arms clips from the new offset. When the
        // transport is stopped/paused (e.g. Stop sends Pause then Seek(0)), just
        // clear voices — Play will re-arm them from the playhead. (Without this
        // guard a clip covering the seek point would resurrect after Stop.)
        if self.playing {
            self.arm_audio_clips_at_playhead();
        } else {
            let _ = self.audio_tx.try_send(AudioEvent::StopAudioClips);
        }
    }

    /// Transition into rolling playback: mark the state, restart the preview
    /// clip, and arm arranged audio clips at the playhead (mirrors Play).
    fn start_roll(&mut self) {
        self.transport_state.store(TRANSPORT_ROLLING, Ordering::Release);
        let _ = self.audio_tx.try_send(AudioEvent::PlayClip);
        self.arm_audio_clips_at_playhead();
        // Legacy (no punch toggles): the gate opens right at the roll.
        self.update_gate();
    }

    /// Handle `RecordStart`: hold the playhead and click for `count_in_bars`
    /// bars, then roll. If already playing this is a punch-in: just mark the
    /// roll point and keep rolling.
    fn record_start(&mut self, count_in_bars: u8) {
        self.recording = true;
        self.roll_start_tick.store(self.tick, Ordering::Release);
        if self.playing {
            self.transport_state.store(TRANSPORT_ROLLING, Ordering::Release);
            self.update_gate();
            return;
        }
        self.count_in = count_in_bars as u32 * TICKS_PER_BAR;
        self.playing = true;
        if self.count_in == 0 {
            self.start_roll();
        } else {
            self.transport_state.store(TRANSPORT_COUNT_IN, Ordering::Release);
        }
    }

    /// Pause playback (also cancels a count-in).
    fn do_pause(&mut self) {
        self.close_gate();
        self.recording = false;
        self.playing = false;
        self.count_in = 0;
        self.transport_state.store(TRANSPORT_STOPPED, Ordering::Release);
        let _ = self.audio_tx.try_send(AudioEvent::StopClip);
        let _ = self.audio_tx.try_send(AudioEvent::StopAudioClips);
    }

    /// One count-in tick: click on beats (accented on the bar), playhead held.
    /// Count-in clicks always sound, regardless of the metronome toggle.
    fn fire_count_in_tick(&mut self) {
        if self.count_in % TICKS_PER_BEAT == 0 {
            let vel = if self.count_in % TICKS_PER_BAR == 0 { 110u8 } else { 80u8 };
            let _ = self.audio_tx.try_send(AudioEvent::NoteOff { strip: STRIP_UTILITY, pitch: 75 });
            let _ = self.audio_tx.try_send(AudioEvent::NoteOn { strip: STRIP_UTILITY, pitch: 75, velocity: vel });
        }
        self.count_in -= 1;
        if self.count_in == 0 {
            let _ = self.audio_tx.try_send(AudioEvent::NoteOff { strip: STRIP_UTILITY, pitch: 75 });
        }
    }

    /// One playing-state step: count-in tick or a normal sequencer tick.
    fn advance(&mut self) {
        if self.count_in > 0 {
            self.fire_count_in_tick();
            if self.count_in == 0 {
                self.start_roll();
            }
        } else {
            self.fire_tick();
        }
    }

    fn fire_tick(&mut self) {
        // 0. On a bar boundary, apply any queued live edit (cycle-quantized swap)
        //    before generating this bar's events, so it starts on the downbeat.
        if self.tick % TICKS_PER_BAR == 0 {
            if let Some(p) = self.pending_update.take() {
                self.apply_project(p, false);
            }
        }

        // 1. Drain scheduled NoteOffs whose tick has arrived
        let keys: Vec<u32> = self.schedule.range(..=self.tick).map(|(k, _)| *k).collect();
        for k in keys {
            if let Some(events) = self.schedule.remove(&k) {
                for ev in events {
                    let _ = self.audio_tx.try_send(ev);
                }
            }
        }

        // 2. Collect NoteOns/NoteOffs for current tick (immutable read phase)
        let mut note_ons:  Vec<AudioEvent>          = Vec::new();
        let mut note_offs: Vec<(u32, AudioEvent)>   = Vec::new();

        for (_, ts) in &self.tracks {
            if ts.muted { continue; }
            let strip = ts.strip;

            // Find the slot that covers the current tick
            let slot = ts.slots.iter().find(|s| {
                s.start_tick <= self.tick && self.tick < s.end_tick
            });
            let Some(slot) = slot else { continue };

            let local_tick  = self.tick - slot.start_tick;
            let pat_len     = pattern_length_ticks(&slot.pattern);
            let pat_tick    = if slot.loop_pat { local_tick % pat_len } else { local_tick };

            if pat_tick >= pat_len { continue; }

            match &slot.pattern {
                Pattern::Step(sp) => {
                    let step_count    = sp.steps as u32;
                    let ticks_per_step = TICKS_PER_BAR / 16; // steps are 16ths
                    let swing         = self.swing;
                    for step in 0..step_count {
                        if swung_step_tick(step, swing) == pat_tick {
                            let idx = step as usize;
                            for row in &sp.rows {
                                if let Some(velocity) = row.step_velocity(idx) {
                                    let velocity = humanize_velocity(velocity, strip, row.pitch, self.tick, self.humanize);
                                    if ts.is_sampler {
                                        note_ons.push(AudioEvent::TriggerSample { strip, pitch: row.pitch, gain: velocity as f32 / 127.0 });
                                    } else {
                                        note_ons.push(AudioEvent::NoteOn { strip, pitch: row.pitch, velocity });
                                        let off_tick = self.tick + sp.step_note_ticks(ticks_per_step);
                                        note_offs.push((off_tick, AudioEvent::NoteOff { strip, pitch: row.pitch }));
                                    }
                                }
                            }
                        }
                    }
                }
                Pattern::PianoRoll(pr) => {
                    for note in &pr.notes {
                        if note.tick == pat_tick {
                            let velocity = humanize_velocity(note.vel, strip, note.pitch, self.tick, self.humanize);
                            if ts.is_sampler {
                                note_ons.push(AudioEvent::TriggerSample { strip, pitch: note.pitch, gain: velocity as f32 / 127.0 });
                            } else {
                                note_ons.push(AudioEvent::NoteOn { strip, pitch: note.pitch, velocity });
                                let off_tick = self.tick + note.duration;
                                note_offs.push((off_tick, AudioEvent::NoteOff { strip, pitch: note.pitch }));
                            }
                        }
                    }
                }
            }
        }

        // Apply collected events (mutable write phase — no borrow conflict)
        for ev in note_ons {
            let _ = self.audio_tx.try_send(ev);
        }
        for (off_tick, ev) in note_offs {
            self.schedule.entry(off_tick).or_default().push(ev);
        }

        // Fire arranged audio clips whose start tick is the current tick.
        for job in &self.audio_jobs {
            if job.start_tick == self.tick {
                let _ = self.audio_tx.try_send(AudioEvent::TriggerAudioClip {
                    strip: job.strip,
                    clip_id: job.clip_id,
                    offset_sec: job.offset_sec,
                    len_sec: job.len_sec,
                    gain: job.gain,
                });
            }
        }

        // LFO modulation: re-send modulated CCs on a coarse interval (~every
        // 8th note) so live playback breathes like the export, without flooding.
        if self.tick % (TICKS_PER_BEAT / 2) == 0 {
            let spt = 60.0 / self.bpm / TICKS_PER_BEAT as f64; // seconds per tick
            let t = (self.tick as f64 * spt) as f32;
            let mut ccs: Vec<AudioEvent> = Vec::new();
            for (_, ts) in &self.tracks {
                for (cc, base01, m) in &ts.mod_jobs {
                    let v = modulated_value(m, *base01, t);
                    ccs.push(AudioEvent::RawCC {
                        strip:   ts.strip,
                        cc:      *cc,
                        value:   (v * 127.0).round().clamp(0.0, 127.0) as u8,
                    });
                }
            }
            for ev in ccs { let _ = self.audio_tx.try_send(ev); }
        }

        // Metronome click (utility synth, pitch 75 = Claves)
        if self.metronome && self.tick % TICKS_PER_BEAT == 0 {
            let vel = if self.tick % TICKS_PER_BAR == 0 { 100u8 } else { 60u8 };
            let _ = self.audio_tx.try_send(AudioEvent::NoteOn { strip: STRIP_UTILITY, pitch: 75, velocity: vel });
            let off = self.tick + TICKS_PER_BEAT / 4;
            self.schedule.entry(off).or_default().push(AudioEvent::NoteOff { strip: STRIP_UTILITY, pitch: 75 });
        }

        // 3. Advance tick: loop-region wrap takes precedence over the song
        //    boundary. Region wrap = seek semantics (schedule dropped,
        //    straddling clips re-armed) + a take split while recording.
        let (ls, le) = self.loop_region.ticks();
        let next = self.tick + 1;
        if self.loop_region.enabled && next >= le && self.tick < le {
            // Fire pending NoteOffs before seek() drops the schedule, so
            // notes never hang across a loop pass (same as the song wrap).
            let pending: Vec<(u8, u8)> = self.schedule.values()
                .flat_map(|evs| evs.iter())
                .filter_map(|ev| match ev {
                    AudioEvent::NoteOff { strip, pitch } => Some((*strip, *pitch)),
                    _ => None,
                })
                .collect();
            for (strip, pitch) in pending {
                let _ = self.audio_tx.try_send(AudioEvent::NoteOff { strip, pitch });
            }
            let was_open = self.gate_is_open();
            if was_open {
                self.close_gate();
            }
            self.seek(ls);
            if was_open {
                self.open_gate(); // restamps roll_start_tick for the new pass
            }
            self.update_gate();
            return;
        }
        if self.loop_region.enabled && next < le {
            // While the loop is on, the region defines the playback span:
            // run toward its end even past the song's content length
            // (silence beyond the content, then cycle).
            self.tick = next;
            self.playback_tick.store(self.tick, Ordering::Relaxed);
            self.update_gate();
            return;
        }
        let max = self.song_length_ticks();
        self.tick = next % max;
        self.playback_tick.store(self.tick, Ordering::Relaxed);
        self.update_gate();
        if self.tick == 0 {
            // Flush pending NoteOffs to prevent stuck notes on loop
            let pending: Vec<(u8, u8)> = self.schedule.values()
                .flat_map(|evs| evs.iter())
                .filter_map(|ev| match ev {
                    AudioEvent::NoteOff { strip, pitch } => Some((*strip, *pitch)),
                    _ => None,
                })
                .collect();
            self.schedule.clear();
            for (strip, pitch) in pending {
                let _ = self.audio_tx.try_send(AudioEvent::NoteOff { strip, pitch });
            }
            let _ = self.audio_tx.try_send(AudioEvent::StopAudioClips);
        }
    }

    /// Load a project and restart playback from the top.
    fn load_project(&mut self, project: Project) {
        self.apply_project(project, true);
    }

    /// Rebuild from `project`. With `reset_tick = false` the playhead keeps its
    /// current position (a live swap), so editing a pattern continues the groove
    /// instead of jumping back to bar 1; currently-ringing notes are silenced so
    /// the now-removed note-offs can't leave anything hanging.
    fn apply_project(&mut self, project: Project, reset_tick: bool) {
        if reset_tick {
            self.schedule.clear();
            self.tick = 0;
            self.playback_tick.store(0, Ordering::Relaxed);
        } else {
            // Live swap: send the note-offs that were scheduled so the rebuild
            // (which drops the schedule) can't leave a note hanging — without
            // silencing the whole synth on every edit.
            for (_, evs) in std::mem::take(&mut self.schedule) {
                for ev in evs { let _ = self.audio_tx.try_send(ev); }
            }
        }
        self.bpm       = project.project.bpm.clamp(20.0, 300.0);
        self.swing     = project.project.swing;
        self.humanize  = project.project.humanize;
        self.metronome = project.project.metronome;
        self.loop_region = project.loop_region.clone();
        self.tracks.clear();

        // ── Mixer strips: one per track, built off-thread by the relay ────────
        let _ = self.audio_tx.try_send(AudioEvent::LoadStrips {
            specs: StripSpec::from_project(&project),
        });
        let _ = self.audio_tx.try_send(AudioEvent::SetReturns {
            returns: project.returns.clone(),
        });
        // CLAP instances: the relay diffs these against what's live, so this
        // is cheap when nothing plugin-related changed.
        let _ = self.audio_tx.try_send(AudioEvent::LoadClapInstances {
            specs: crate::audio::clap::slot_specs(&project),
        });

        self.audio_jobs.clear();
        let mut clip_specs: Vec<(u32, String)> = Vec::new();
        for (ci, clip) in project.audio_clips.iter().enumerate() {
            let strip = project.tracks.iter().position(|t| t.id == clip.track).unwrap_or(0) as u8;
            let start_tick = sec_to_tick(clip.start_sec, self.bpm);
            let end_tick   = sec_to_tick(clip.start_sec + clip.len_sec, self.bpm);
            let clip_id = ci as u32;
            self.audio_jobs.push(AudioClipJob {
                clip_id, strip, start_tick, end_tick,
                offset_sec: clip.offset_sec as f32,
                len_sec: clip.len_sec as f32,
                gain: clip.gain,
            });
            clip_specs.push((clip_id, clip.path.clone()));
        }
        if !clip_specs.is_empty() {
            let _ = self.audio_tx.try_send(AudioEvent::LoadAudioClips { specs: clip_specs });
        }

        // Compute the longest first-pattern length across all tracks (used for auto-loop mode)
        let auto_loop_len: u32 = project.tracks.iter()
            .flat_map(|t| t.patterns.first())
            .filter_map(|pid| project.patterns.iter().find(|p| p.id() == pid))
            .map(|p| pattern_length_ticks(p))
            .max()
            .unwrap_or(TICKS_PER_BAR);

        // Determine arrangement mode
        let has_arrangement = !project.arrangement.is_empty();

        for (i, track) in project.tracks.iter().enumerate() {
            let strip = i as u8;
            // Audio tracks: legacy preview clip (TUI path), skip MIDI scheduling.
            // Program/volume/pan/effects for all strips are baked into the
            // StripSpec build above; nothing per-track to send here.
            if track.track_type == TrackType::Audio {
                if let Instrument::AudioFile { path } = &track.instrument {
                    if !path.is_empty() {
                        let _ = self.audio_tx.try_send(AudioEvent::LoadClipFromFile {
                            path: path.clone(),
                            volume: track.volume,
                        });
                        let fx = &track.effects;
                        let _ = self.audio_tx.try_send(AudioEvent::SetClipEffects {
                            delay_time_ms:  fx.delay_time_ms,
                            delay_feedback: fx.delay_feedback,
                            delay_mix:      fx.delay_mix,
                            cutoff:         fx.cutoff,
                            drive:          fx.drive,
                            eq_low_gain:    fx.eq_low_gain,
                            eq_mid_gain:    fx.eq_mid_gain,
                            eq_high_gain:   fx.eq_high_gain,
                            eq_mid_freq:    fx.eq_mid_freq,
                            comp_threshold: fx.comp_threshold,
                            comp_ratio:     fx.comp_ratio,
                            comp_attack_ms: fx.comp_attack_ms,
                            comp_release_ms: fx.comp_release_ms,
                        });
                    }
                }
                self.tracks.push((track.id.clone(), TrackState {
                    strip,
                    slots:    Vec::new(),
                    muted:    track.muted,
                    is_audio: true,
                    is_sampler: false,
                    mod_jobs: Vec::new(), // clip modulation handled by the exporter
                }));
                continue;
            }

            let is_sampler = matches!(track.instrument, Instrument::Sampler { .. });

            let slots: Vec<SlotState> = if has_arrangement {
                // Build slots from explicit arrangement entries for this track
                let mut v: Vec<SlotState> = project.arrangement.iter()
                    .filter(|s| s.track == track.id)
                    .filter_map(|s| {
                        let pat = project.patterns.iter().find(|p| p.id() == s.pattern)?.clone();
                        let start = s.bar * TICKS_PER_BAR;
                        let (end, loop_pat) = match s.len_bars {
                            Some(n) => (start + n.max(1) * TICKS_PER_BAR, true), // loop to fill
                            None => (start + pattern_length_ticks(&pat), false), // natural, unchanged
                        };
                        Some(SlotState { start_tick: start, end_tick: end, loop_pat, pattern: pat })
                    })
                    .collect();
                v.sort_by_key(|s| s.start_tick);
                v
            } else {
                // Auto-loop: first pattern fills the entire song cycle
                track.patterns.first()
                    .and_then(|pid| project.patterns.iter().find(|p| p.id() == pid).cloned())
                    .map(|pat| vec![SlotState {
                        start_tick: 0,
                        end_tick:   auto_loop_len,
                        loop_pat:   true,
                        pattern:    pat,
                    }])
                    .unwrap_or_default()
            };

            // Sampler tracks have no synth for modulators to drive.
            let mod_jobs = if is_sampler {
                Vec::new()
            } else {
                track.modulators.iter()
                    .map(|m| (m.dest.cc(), mod_base01(m.dest, track), m.clone()))
                    .collect()
            };

            self.tracks.push((track.id.clone(), TrackState {
                strip,
                slots,
                muted:    track.muted,
                is_audio: false,
                is_sampler,
                mod_jobs,
            }));
        }

        // Live swap: keep the playhead, but clamp it to the (possibly new) length.
        if !reset_tick {
            let max = self.song_length_ticks().max(1);
            self.tick %= max;
            self.playback_tick.store(self.tick, Ordering::Relaxed);
        }
    }

    pub fn run(mut self) {
        let mut next_tick = Instant::now();
        loop {
            while let Ok(cmd) = self.cmd_rx.try_recv() {
                match cmd {
                    SequencerCommand::Play  => {
                        self.playing = true;
                        self.roll_start_tick.store(self.tick, Ordering::Release);
                        self.transport_state.store(TRANSPORT_ROLLING, Ordering::Release);
                        let _ = self.audio_tx.try_send(AudioEvent::PlayClip);
                        // Start any audio clip spanning the playhead from the right
                        // offset, so pressing Play mid-clip plays audio from there
                        // (not silence, and not only from the clip's start).
                        self.arm_audio_clips_at_playhead();
                    }
                    SequencerCommand::Pause => {
                        self.do_pause();
                    }
                    SequencerCommand::RecordStart { count_in_bars } => {
                        self.record_start(count_in_bars);
                        next_tick = Instant::now();
                    }
                    SequencerCommand::Seek(tick) => {
                        self.seek(tick);
                        let _ = self.audio_tx.try_send(AudioEvent::Panic);
                        next_tick = Instant::now();
                    }
                    SequencerCommand::SetBpm(bpm) => {
                        self.bpm = bpm;
                        next_tick = Instant::now();
                    }
                    SequencerCommand::SetSwing(sw) => {
                        self.swing = sw.clamp(0.0, 1.0);
                    }
                    SequencerCommand::LoadProject(p) => {
                        self.pending_update = None;
                        self.load_project(p);
                    }
                    SequencerCommand::UpdateLive(p)  => {
                        // While playing, defer to the next bar so the swap lands
                        // on the downbeat (latest queued edit wins). When paused,
                        // apply immediately.
                        if self.playing {
                            self.pending_update = Some(p);
                        } else {
                            self.apply_project(p, false);
                        }
                    }
                    SequencerCommand::SetTrackVolume { track_id, volume } => {
                        if let Some((_, ts)) = self.tracks.iter().find(|(id, _)| id == &track_id) {
                            let _ = self.audio_tx.try_send(AudioEvent::SetStripGain { strip: ts.strip, gain: volume });
                        }
                    }
                    SequencerCommand::SetTrackMute { track_id, muted } => {
                        self.set_track_mute(&track_id, muted);
                    }
                    SequencerCommand::SetTrackPan { track_id, pan } => {
                        if let Some((_, ts)) = self.tracks.iter().find(|(id, _)| id == &track_id) {
                            let _ = self.audio_tx.try_send(AudioEvent::SetStripPan { strip: ts.strip, pan });
                        }
                    }
                    SequencerCommand::SetTrackEffects { track_id, effects } => {
                        if let Some((_, ts)) = self.tracks.iter().find(|(id, _)| id == &track_id) {
                            // Legacy preview-clip chain (TUI audio-file path).
                            if ts.is_audio {
                                let _ = self.audio_tx.try_send(AudioEvent::SetClipEffects {
                                    delay_time_ms:   effects.delay_time_ms,
                                    delay_feedback:  effects.delay_feedback,
                                    delay_mix:       effects.delay_mix,
                                    cutoff:          effects.cutoff,
                                    drive:           effects.drive,
                                    eq_low_gain:     effects.eq_low_gain,
                                    eq_mid_gain:     effects.eq_mid_gain,
                                    eq_high_gain:    effects.eq_high_gain,
                                    eq_mid_freq:     effects.eq_mid_freq,
                                    comp_threshold:  effects.comp_threshold,
                                    comp_ratio:      effects.comp_ratio,
                                    comp_attack_ms:  effects.comp_attack_ms,
                                    comp_release_ms: effects.comp_release_ms,
                                });
                            }
                            let _ = self.audio_tx.try_send(AudioEvent::SetStripEffects {
                                strip: ts.strip,
                                effects,
                            });
                        }
                    }
                    SequencerCommand::SetReturns(returns) => {
                        let _ = self.audio_tx.try_send(AudioEvent::SetReturns { returns });
                    }
                    SequencerCommand::SetClapParam { uid, param_id, value } => {
                        let _ = self.audio_tx.try_send(AudioEvent::SetClapParam { uid, param_id, value });
                    }
                    SequencerCommand::SetLoopRegion(r) => {
                        self.loop_region = r;
                    }
                    SequencerCommand::RecordStop => {
                        self.close_gate();
                        self.recording = false;
                    }
                    SequencerCommand::PreviewNote { track_id, pitch, velocity } => {
                        if let Some((_, ts)) = self.tracks.iter().find(|(id, _)| id == &track_id) {
                            let _ = self.audio_tx.try_send(AudioEvent::NoteOn {
                                strip: ts.strip, pitch, velocity,
                            });
                        }
                    }
                    SequencerCommand::PreviewNoteOff { track_id, pitch } => {
                        if let Some((_, ts)) = self.tracks.iter().find(|(id, _)| id == &track_id) {
                            let _ = self.audio_tx.try_send(AudioEvent::NoteOff {
                                strip: ts.strip, pitch,
                            });
                        }
                    }
                    SequencerCommand::RawCC { channel, cc, value } => {
                        // The command's `channel` field now carries a strip index.
                        let _ = self.audio_tx.try_send(AudioEvent::RawCC { strip: channel, cc, value });
                    }
                    SequencerCommand::SetMetronome(enabled) => {
                        self.metronome = enabled;
                    }
                    SequencerCommand::Panic => {
                        self.schedule.clear();
                        let _ = self.audio_tx.try_send(AudioEvent::Panic);
                    }
                    SequencerCommand::Quit => { return; }
                }
            }

            if self.playing {
                self.advance();
                next_tick += self.tick_duration();
                let now = Instant::now();
                if now < next_tick {
                    std::thread::sleep(next_tick - now);
                }
            } else {
                std::thread::sleep(Duration::from_millis(10));
                next_tick = Instant::now();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;
    use crossbeam_channel::unbounded;

    fn make_sequencer() -> (Sequencer, crossbeam_channel::Sender<SequencerCommand>,
                            crossbeam_channel::Receiver<AudioEvent>) {
        let (cmd_tx, cmd_rx) = unbounded::<SequencerCommand>();
        let (audio_tx, audio_rx) = unbounded::<AudioEvent>();
        let seq = Sequencer::new(cmd_rx, audio_tx);
        (seq, cmd_tx, audio_rx)
    }

    use crate::audio::input::{TRANSPORT_COUNT_IN, TRANSPORT_ROLLING, TRANSPORT_STOPPED};

    #[test]
    fn record_start_counts_in_then_rolls() {
        let (mut seq, _, audio_rx) = make_sequencer();
        seq.load_project(Project::default_project());
        let _ = audio_rx.try_iter().count();

        seq.record_start(1);
        assert!(seq.playing);
        assert_eq!(seq.transport_state.load(Ordering::Relaxed), TRANSPORT_COUNT_IN);
        assert_eq!(seq.roll_start_tick.load(Ordering::Relaxed), 0);

        // One bar of count-in: 4 clicks (ch 9 / pitch 75), no pattern notes.
        for _ in 0..TICKS_PER_BAR {
            seq.advance();
        }
        let evs: Vec<_> = audio_rx.try_iter().collect();
        let clicks = evs.iter().filter(|e| matches!(e,
            AudioEvent::NoteOn { strip: STRIP_UTILITY, pitch: 75, .. })).count();
        assert_eq!(clicks, 4);
        assert!(
            !evs.iter().any(|e| matches!(e, AudioEvent::NoteOn { pitch: 36, .. })),
            "no pattern notes may fire during count-in"
        );
        assert_eq!(seq.transport_state.load(Ordering::Relaxed), TRANSPORT_ROLLING);

        // The next tick is the downbeat: the default project's kick fires.
        seq.advance();
        let evs: Vec<_> = audio_rx.try_iter().collect();
        assert!(evs.iter().any(|e| matches!(e, AudioEvent::NoteOn { pitch: 36, .. })));
    }

    #[test]
    fn record_start_zero_bars_rolls_immediately() {
        let (mut seq, _, _) = make_sequencer();
        seq.load_project(Project::default_project());
        seq.record_start(0);
        assert!(seq.playing);
        assert_eq!(seq.transport_state.load(Ordering::Relaxed), TRANSPORT_ROLLING);
    }

    #[test]
    fn record_start_while_playing_is_punch_in() {
        let (mut seq, _, _) = make_sequencer();
        seq.load_project(Project::default_project());
        seq.playing = true;
        seq.transport_state.store(TRANSPORT_ROLLING, Ordering::Relaxed);
        seq.tick = 100;
        seq.record_start(2);
        // No count-in when already rolling; the roll tick is the punch point.
        assert_eq!(seq.count_in, 0);
        assert_eq!(seq.roll_start_tick.load(Ordering::Relaxed), 100);
        assert_eq!(seq.transport_state.load(Ordering::Relaxed), TRANSPORT_ROLLING);
    }

    #[test]
    fn pause_cancels_count_in() {
        let (mut seq, _, _) = make_sequencer();
        seq.load_project(Project::default_project());
        seq.record_start(1);
        seq.do_pause();
        assert!(!seq.playing);
        assert_eq!(seq.count_in, 0);
        assert_eq!(seq.transport_state.load(Ordering::Relaxed), TRANSPORT_STOPPED);
    }

    #[test]
    fn tick_duration_at_120_bpm() {
        let (seq, _, _) = make_sequencer();
        let d = seq.tick_duration();
        let expected = Duration::from_secs_f64(60.0 / 120.0 / 120.0);
        assert!((d.as_secs_f64() - expected.as_secs_f64()).abs() < 1e-9);
    }

    #[test]
    fn load_project_sets_bpm() {
        let (mut seq, _, _) = make_sequencer();
        let mut proj = Project::default_project();
        proj.project.bpm = 140.0;
        seq.load_project(proj);
        assert_eq!(seq.bpm, 140.0);
    }

    #[test]
    fn humanize_velocity_is_deterministic_and_bounded() {
        // humanize = 0 -> unchanged
        assert_eq!(humanize_velocity(90, 0, 60, 0, 0.0), 90);
        // off (vel 0) stays 0 regardless
        assert_eq!(humanize_velocity(0, 0, 60, 0, 1.0), 0);
        // deterministic for identical inputs
        let a = humanize_velocity(90, 2, 64, 480, 0.5);
        let b = humanize_velocity(90, 2, 64, 480, 0.5);
        assert_eq!(a, b);
        // within ±35% and clamped to 1..=127 across many ticks
        for tick in 0..2000u32 {
            let v = humanize_velocity(100, 9, 36, tick, 1.0);
            assert!((1..=127).contains(&v));
            assert!(v >= 65 && v <= 127, "tick {tick} -> {v} out of ±35% band");
        }
        // different ticks produce variation (not a constant)
        let set: std::collections::HashSet<u8> =
            (0..32u32).map(|t| humanize_velocity(100, 0, 60, t, 1.0)).collect();
        assert!(set.len() > 1, "humanization produced no variation");
    }

    #[test]
    fn lfo_and_modulation_bounds() {
        // lfo stays within [-1, 1] for every shape across a full cycle
        for shape in [LfoShape::Sine, LfoShape::Triangle, LfoShape::Saw, LfoShape::Square] {
            for i in 0..=100 {
                let v = lfo_value(shape, i as f32 / 100.0);
                assert!((-1.0..=1.0).contains(&v), "{shape:?} out of range: {v}");
            }
        }
        // sine zero-crossings and triangle extremes at known phases
        assert!(lfo_value(LfoShape::Sine, 0.0).abs() < 1e-6);
        assert!((lfo_value(LfoShape::Triangle, 0.5) - 1.0).abs() < 1e-6);
        // modulated value oscillates around base and is clamped to 0..1
        let m = Modulator { dest: ModDest::Cutoff, shape: LfoShape::Sine, rate_hz: 1.0, depth: 0.4, phase: 0.0 };
        for step in 0..200 {
            let t = step as f32 / 100.0;
            let v = modulated_value(&m, 0.6, t);
            assert!((0.0..=1.0).contains(&v));
        }
        // depth 0 -> constant at base
        let m0 = Modulator { dest: ModDest::Cutoff, shape: LfoShape::Sine, rate_hz: 2.0, depth: 0.0, phase: 0.3 };
        assert!((modulated_value(&m0, 0.5, 1.234) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn update_live_keeps_playhead_load_resets_it() {
        let (mut seq, _, _) = make_sequencer();
        seq.load_project(Project::default_project());
        assert_eq!(seq.tick, 0);

        // Move the playhead, then a live update should keep it (within bounds).
        seq.tick = 200;
        seq.apply_project(Project::default_project(), false);
        assert_eq!(seq.tick, 200, "UpdateLive must not restart the playhead");

        // A full load restarts from the top.
        seq.tick = 200;
        seq.load_project(Project::default_project());
        assert_eq!(seq.tick, 0, "LoadProject should reset the playhead");
    }

    #[test]
    fn queued_update_applies_only_on_bar_boundary() {
        let (mut seq, _, _) = make_sequencer();
        seq.load_project(Project::default_project());
        let mut edited = Project::default_project();
        edited.project.bpm = 155.0;
        seq.pending_update = Some(edited);

        // Mid-bar: the queued edit must NOT apply yet.
        seq.tick = 100;
        seq.fire_tick();
        assert_eq!(seq.bpm, 120.0);
        assert!(seq.pending_update.is_some());

        // On the bar boundary (tick % 480 == 0): it applies.
        seq.tick = 0;
        seq.fire_tick();
        assert_eq!(seq.bpm, 155.0);
        assert!(seq.pending_update.is_none());
    }

    #[test]
    fn fire_tick_emits_kick_on_step_zero() {
        let (mut seq, _, audio_rx) = make_sequencer();
        seq.load_project(Project::default_project());
        seq.tick = 0;
        seq.fire_tick();
        let events: Vec<_> = audio_rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(
            e, AudioEvent::NoteOn { pitch: 36, .. }
        )));
    }

    #[test]
    fn arrangement_slot_honors_len_bars() {
        let (mut seq, _, _) = make_sequencer();
        let mut p = crate::project::Project::default_project();
        p.arrangement.iter_mut().find(|s| s.track == "t1").unwrap().len_bars = Some(3);
        seq.load_project(p);
        let ts = &seq.tracks.iter().find(|(id, _)| id == "t1").unwrap().1;
        let slot = ts.slots.iter().find(|s| s.start_tick == 0).expect("a slot at bar 0");
        assert_eq!(slot.end_tick, 3 * TICKS_PER_BAR);
        assert!(slot.loop_pat, "a length-extended clip loops to fill");
    }

    #[test]
    fn arrangement_natural_length_unchanged() {
        let (mut seq, _, _) = make_sequencer();
        let p = crate::project::Project::default_project(); // no len_bars
        seq.load_project(p);
        let ts = &seq.tracks.iter().find(|(id, _)| id == "t1").unwrap().1;
        let slot = ts.slots.iter().find(|s| s.start_tick == 0).unwrap();
        assert_eq!(slot.end_tick, TICKS_PER_BAR); // p1 is a 1-bar step pattern
        assert!(!slot.loop_pat);
    }

    #[test]
    fn sec_to_tick_uses_bpm() {
        // 120 bpm: ticks/sec = bpm/60 * TICKS_PER_BEAT = 2 * 120 = 240.
        assert_eq!(sec_to_tick(0.0, 120.0), 0);
        assert_eq!(sec_to_tick(1.0, 120.0), 240);
        assert_eq!(sec_to_tick(0.5, 120.0), 120);
    }

    #[test]
    fn seek_sets_tick_and_clears_schedule() {
        let (mut seq, _, _) = make_sequencer();
        seq.load_project(Project::default_project());
        // prime a pending note-off so clearing is observable
        seq.schedule.insert(100, vec![AudioEvent::NoteOff { strip: 1, pitch: 60 }]);
        // default project's longest slot is the 2-bar bass pattern → 960 ticks.
        seq.seek(481);
        assert_eq!(seq.tick, 481);
        assert_eq!(seq.playback_tick.load(std::sync::atomic::Ordering::Relaxed), 481);
        assert!(seq.schedule.is_empty(), "seek must clear the note-off schedule");
        // seeking past song length wraps into range
        seq.seek(10_000);
        assert!(seq.tick < seq.song_length_ticks());
    }

    /// Default project plus one audio track ("vox") and a clip on it at tick 0.
    fn project_with_audio_clip(track_muted: bool) -> Project {
        use crate::project::{AudioClip, Instrument, Track, TrackType};
        let mut p = Project::default_project();
        p.tracks.push(Track {
            id: "vox".into(),
            name: "Vocals".into(),
            track_type: TrackType::Audio,
            instrument: Instrument::AudioFile { path: String::new() },
            volume: 1.0,
            pan: 0.0,
            muted: track_muted,
            patterns: vec![],
            effects: Default::default(),
            modulators: vec![],
            clap_fx: vec![],
        });
        p.audio_clips.push(AudioClip {
            track: "vox".into(),
            path: "nonexistent.wav".into(),
            start_sec: 0.0,
            offset_sec: 0.0,
            len_sec: 4.0,
            gain: 1.0,
        });
        p
    }

    #[test]
    fn paused_seek_does_not_retrigger_audio_clips() {
        // Regression: Stop = Pause + Seek(0). A paused seek must not re-arm a clip
        // covering tick 0, or the vocals resurrect right after Stop cleared them.
        let (mut seq, _, audio_rx) = make_sequencer();
        seq.load_project(project_with_audio_clip(false));
        let _ = audio_rx.try_iter().count(); // drain load-time events

        seq.playing = false;
        seq.seek(0);
        let evs: Vec<_> = audio_rx.try_iter().collect();
        assert!(
            !evs.iter().any(|e| matches!(e, AudioEvent::TriggerAudioClip { .. })),
            "a paused seek must not trigger audio clips"
        );

        // Positive control: scrubbing *during* playback still re-arms the clip.
        seq.playing = true;
        seq.seek(0);
        let evs: Vec<_> = audio_rx.try_iter().collect();
        assert!(
            evs.iter().any(|e| matches!(e, AudioEvent::TriggerAudioClip { clip_id: 0, .. })),
            "scrubbing during playback should re-arm the clip"
        );
    }

    #[test]
    fn long_note_off_fires_across_bars() {
        let (mut seq, _, audio_rx) = make_sequencer();
        let mut p = Project::default_project();
        p.set_note_duration("p2", 36, 0, 600); // ends 120 ticks into bar 2
        p.resize_clip("t2", 0, 2); // bar 2 exists on the bass track
        seq.load_project(p);
        let _ = audio_rx.try_iter().count();
        seq.playing = true;
        for _ in 0..600 {
            seq.advance(); // fires ticks 0..=599
        }
        let early_off = audio_rx
            .try_iter()
            .any(|e| matches!(e, AudioEvent::NoteOff { strip: 1, pitch: 36 }));
        assert!(!early_off, "note must still be held before tick 600");
        seq.advance(); // fires tick 600: the scheduled off
        let off = audio_rx
            .try_iter()
            .any(|e| matches!(e, AudioEvent::NoteOff { strip: 1, pitch: 36 }));
        assert!(off, "2-bar note's off fires in bar 2");
    }

    #[test]
    fn steps_are_sixteenths_and_extend_length() {
        // swung tick math: fixed 30 ticks/step, offbeat = step % 4 == 2.
        assert_eq!(swung_step_tick(0, 0.0), 0);
        assert_eq!(swung_step_tick(2, 0.0), 60);
        assert_eq!(swung_step_tick(2, 1.0), 60 + TICKS_PER_BEAT / 6);
        assert_eq!(swung_step_tick(16, 0.0), TICKS_PER_BAR); // bar 2 exists now
        assert_eq!(swung_step_tick(18, 1.0), TICKS_PER_BAR + 60 + TICKS_PER_BEAT / 6);

        // Pattern length derives from steps: 32 steps = 2 bars, 4 steps = 1 beat.
        let mut p = Project::default_project();
        p.resize_steps("p1", 16); // 32 steps
        let pat = p.patterns.iter().find(|x| x.id() == "p1").unwrap();
        assert_eq!(pattern_length_ticks(pat), 2 * TICKS_PER_BAR);
        assert_eq!(pat.bars(), 2);
        p.resize_steps("p1", -28); // 4 steps
        let pat = p.patterns.iter().find(|x| x.id() == "p1").unwrap();
        assert_eq!(pattern_length_ticks(pat), 4 * 30);
        assert_eq!(pat.bars(), 1);
    }

    #[test]
    fn thirtytwo_step_pattern_fires_in_second_bar() {
        // A hit on step 16 sounds at the start of bar 2 — not double-time in bar 1.
        let (mut seq, _, audio_rx) = make_sequencer();
        let mut p = Project::default_project();
        p.resize_steps("p1", 16); // 32 steps
        if let Pattern::Step(sp) = p.patterns.iter_mut().find(|x| x.id() == "p1").unwrap() {
            for r in &mut sp.rows {
                for s in r.steps.iter_mut() {
                    *s = 0;
                }
            }
            sp.rows[0].steps[16] = 1; // kick on the downbeat of bar 2
        }
        seq.load_project(p);
        let _ = audio_rx.try_iter().count();
        seq.playing = true;
        for _ in 0..TICKS_PER_BAR {
            seq.advance();
        }
        // Strip 0 = the drum track under test (the default bass also plays
        // pitch 36 in bar 1 — don't let it pollute the assertion).
        let bar1: Vec<_> = audio_rx
            .try_iter()
            .filter(|e| matches!(e, AudioEvent::NoteOn { strip: 0, pitch: 36, .. }
                                  | AudioEvent::TriggerSample { strip: 0, pitch: 36, .. }))
            .collect();
        assert!(bar1.is_empty(), "step 16 must not fire in bar 1: {bar1:?}");
        seq.advance();
        let hit = audio_rx.try_iter().any(|e| matches!(e,
            AudioEvent::NoteOn { strip: 0, pitch: 36, .. }
            | AudioEvent::TriggerSample { strip: 0, pitch: 36, .. }));
        assert!(hit, "step 16 fires at bar 2 start");
    }

    #[test]
    fn loop_region_wraps_playhead() {
        let (mut seq, _, audio_rx) = make_sequencer();
        let mut p = Project::default_project();
        p.set_loop_region(1, 2);
        p.loop_region.enabled = true;
        seq.load_project(p);
        let _ = audio_rx.try_iter().count();
        seq.playing = true;
        seq.seek(2 * TICKS_PER_BAR - 1); // one tick before the region end
        let _ = audio_rx.try_iter().count();
        seq.advance(); // advances + wraps
        assert_eq!(seq.tick, TICKS_PER_BAR, "wrap lands on region start, got {}", seq.tick);
        let evs: Vec<_> = audio_rx.try_iter().collect();
        // seek() during playback re-arms clips; the default project has no
        // audio clips, but the wrap must at least keep running (no panic) —
        // assert the playhead mirror moved.
        assert_eq!(seq.playback_tick.load(Ordering::Relaxed), TICKS_PER_BAR);
        let _ = evs;
    }

    #[test]
    fn loop_region_beyond_song_length_still_cycles() {
        // A 1-bar song with a region over bars 2-3: the playhead must run
        // past the content into the region and cycle there, not wrap at the
        // song boundary.
        let (mut seq, _, _) = make_sequencer();
        let mut p = Project::default_project();
        p.set_loop_region(1, 3);
        p.loop_region.enabled = true;
        seq.load_project(p);
        seq.playing = true;
        let song_len = seq.song_length_ticks();
        // Walk well past both the song length and one region wrap.
        for _ in 0..(3 * TICKS_PER_BAR + 10) {
            seq.advance();
        }
        let (ls, le) = (TICKS_PER_BAR, 3 * TICKS_PER_BAR);
        assert!(seq.tick >= ls && seq.tick < le,
            "playhead cycles inside the region, got {} (song len {song_len})", seq.tick);
    }

    #[test]
    fn take_gate_legacy_opens_on_roll_and_closes_on_pause() {
        let (mut seq, _, _) = make_sequencer();
        seq.load_project(Project::default_project()); // region disabled
        assert_eq!(seq.take_gate.load(Ordering::Relaxed), 0);
        seq.record_start(0); // no count-in -> rolls immediately
        assert_eq!(seq.take_gate.load(Ordering::Relaxed) % 2, 1, "gate open at roll");
        seq.do_pause();
        assert_eq!(seq.take_gate.load(Ordering::Relaxed) % 2, 0, "pause closes gate");
    }

    #[test]
    fn take_gate_punch_bounds_and_loop_passes() {
        let (mut seq, _, _) = make_sequencer();
        let mut p = Project::default_project();
        p.set_loop_region(1, 2);
        p.loop_region.enabled = true;
        p.loop_region.punch_in = true;
        p.loop_region.punch_out = true;
        seq.load_project(p);
        seq.record_start(0); // rolling from tick 0, before the region
        assert_eq!(seq.take_gate.load(Ordering::Relaxed) % 2, 0,
            "punch_in keeps the gate closed before the region");
        // Walk to the region start: gate opens at punch-in.
        while seq.tick < TICKS_PER_BAR {
            seq.advance();
        }
        assert_eq!(seq.take_gate.load(Ordering::Relaxed) % 2, 1, "opens at punch-in");
        assert_eq!(seq.roll_start_tick.load(Ordering::Relaxed), TICKS_PER_BAR);
        // Walk through one wrap: pass split = close + open (gate +2).
        let g_open = seq.take_gate.load(Ordering::Relaxed);
        loop {
            seq.advance();
            if seq.tick == TICKS_PER_BAR {
                break; // wrapped back to the region start
            }
        }
        let g_after = seq.take_gate.load(Ordering::Relaxed);
        assert_eq!(g_after, g_open + 2, "wrap splits the take (close+open)");
        assert_eq!(g_after % 2, 1, "still recording on pass 2");
        assert_eq!(seq.roll_start_tick.load(Ordering::Relaxed), TICKS_PER_BAR,
            "roll tick restamped at the pass start");
        seq.do_pause();
        assert_eq!(seq.take_gate.load(Ordering::Relaxed) % 2, 0);
    }

    #[test]
    fn apply_project_emits_clap_specs() {
        let (mut seq, _, audio_rx) = make_sequencer();
        let mut p = Project::default_project();
        let t0 = p.tracks[0].id.clone();
        p.add_clap_fx(&t0, "/x.clap", "test.gain").unwrap();
        seq.load_project(p);
        let evs: Vec<_> = audio_rx.try_iter().collect();
        assert!(evs.iter().any(|e| matches!(e,
            AudioEvent::LoadClapInstances { specs } if specs.len() == 1)),
            "apply_project must push CLAP slot specs");
    }

    #[test]
    fn load_project_emits_return_config() {
        let (mut seq, _, audio_rx) = make_sequencer();
        seq.load_project(Project::default_project());
        let evs: Vec<_> = audio_rx.try_iter().collect();
        assert!(evs.iter().any(|e| matches!(e,
            AudioEvent::SetReturns { returns } if returns.delay.time_ms == 300)),
            "apply_project must push the return-bus config");
    }

    #[test]
    fn mute_propagates_to_strips() {
        let (mut seq, _, audio_rx) = make_sequencer();
        // A track muted in the project file starts its strip muted (via the
        // StripSpec build; the spec is checked here, the strip math in mixer tests).
        seq.load_project(project_with_audio_clip(true));
        let evs: Vec<_> = audio_rx.try_iter().collect();
        let spec_muted = evs.iter().any(|e| matches!(e,
            AudioEvent::LoadStrips { specs } if specs.last().is_some_and(|s| s.muted)));
        assert!(spec_muted, "loading a muted audio track must mute its strip spec");

        // Live toggle off, then back on, reaches the strip each time.
        // "vox" is the last track (index 2 in the default project + 1 audio track).
        seq.set_track_mute("vox", false);
        let evs: Vec<_> = audio_rx.try_iter().collect();
        assert!(evs.iter().any(|e| matches!(e,
            AudioEvent::SetStripMute { muted: false, .. })));

        seq.set_track_mute("vox", true);
        let evs: Vec<_> = audio_rx.try_iter().collect();
        assert!(evs.iter().any(|e| matches!(e,
            AudioEvent::SetStripMute { muted: true, .. })));
    }

    #[test]
    fn play_starts_audio_clip_from_playhead_offset() {
        // Clicking a chunk mid-song seeks while paused, then Play must start the
        // audio clip from the matching in-clip offset — not from its start, and
        // not silent. Clip spans 0..4s (offset 0); default bpm 120 → 240 ticks/s.
        let (mut seq, _, audio_rx) = make_sequencer();
        seq.load_project(project_with_audio_clip(false));
        let _ = audio_rx.try_iter().count(); // drain load events

        // Paused seek to 1s in (tick 240): no audio yet.
        seq.playing = false;
        seq.seek(240);
        let evs: Vec<_> = audio_rx.try_iter().collect();
        assert!(
            !evs.iter().any(|e| matches!(e, AudioEvent::TriggerAudioClip { .. })),
            "paused seek must stay silent"
        );

        // Play arms the spanning clip from offset ≈ 1.0s (what the Play cmd does).
        seq.playing = true;
        seq.arm_audio_clips_at_playhead();
        let evs: Vec<_> = audio_rx.try_iter().collect();
        let off = evs.iter().find_map(|e| match e {
            AudioEvent::TriggerAudioClip { clip_id: 0, offset_sec, .. } => Some(*offset_sec),
            _ => None,
        }).expect("Play should trigger the clip spanning the playhead");
        assert!((off - 1.0).abs() < 0.05, "expected ~1.0s in-clip offset, got {off}");
    }
}

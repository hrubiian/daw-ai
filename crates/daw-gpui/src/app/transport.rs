use super::*;

#[cfg(test)]
pub(super) fn playhead_time(tick: u32, bpm: f64) -> String {
    playhead_time_with_map(tick, bpm, &[])
}

pub(super) fn playhead_time_with_map(
    tick: u32,
    bpm: f64,
    tempo_changes: &[daw_core::project::TempoPoint],
) -> String {
    let seconds = daw_core::musical_time::tick_to_seconds(bpm, tempo_changes, tick);
    let millis = (seconds * 1000.).round() as u64;
    let hours = millis / 3_600_000;
    let minutes = millis / 60_000 % 60;
    let seconds = millis / 1000 % 60;
    let millis = millis % 1000;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
    } else {
        format!("{minutes:02}:{seconds:02}.{millis:03}")
    }
}

impl DawView {
    pub(super) fn toggle_play(&mut self) {
        self.playing = !self.playing;
        // Pausing while armed ends the take (a frozen playhead can't record).
        if !self.playing && self.midi_recording {
            self.end_midi_record();
        }
        // Pausing also punches out an active audio take.
        if !self.playing {
            use daw_core::audio::input::RecordPhase;
            if self.audio_take_active || matches!(self.audio_rec.phase, RecordPhase::AwaitingRoll | RecordPhase::Recording) {
                self.stop_audio_take();
            }
        }
        let cmd = if self.playing {
            SequencerCommand::Play
        } else {
            SequencerCommand::Pause
        };
        if let Some(session) = &self.host.session {
            if let Err(e) = session.transport(if self.playing { daw_core::session::Command::Play } else { daw_core::session::Command::Pause }) {
                self.playing = session.playing();
                self.st.set_status(e.to_string());
            }
        } else { let _ = self.send_command(cmd); }
    }

    pub(super) fn project_name(&self) -> SharedString {
        SharedString::from(self.st.project.project.name.clone())
    }

    /// True stop: pause and return the playhead to the start.
    pub(super) fn stop(&mut self) {
        if self.midi_recording {
            self.end_midi_record();
        }
        {
            use daw_core::audio::input::RecordPhase;
            if self.audio_take_active {
                self.stop_audio_take();
            } else if !self.audio_take_finishing {
                match self.audio_rec.phase {
                    RecordPhase::AwaitingRoll | RecordPhase::Recording => self.stop_audio_take(),
                    // Stop while armed-but-idle disarms (closes the input stream).
                    RecordPhase::Armed => {
                        self.audio_rec.disarm();
                        self.host.disarm_input();
                        self.armed_track = None;
                        self.armed_input = None;
                        self.st.set_status("record disarmed");
                    }
                    RecordPhase::Idle => {}
                }
            }
        }
        self.playing = false;
        if let Some(session) = &self.host.session {
            if let Err(e) = session.transport(daw_core::session::Command::Stop) { self.st.set_status(e.to_string()); }
        } else {
            let _ = self.send_command(SequencerCommand::Pause);
            let _ = self.send_command(SequencerCommand::Seek(0));
        }
    }

    /// The open piano-roll pattern as `(index in project.patterns, length in ticks)`,
    /// or `None` when no piano-roll pattern is open (e.g. a Step pattern is selected).
    pub(super) fn open_pr_pattern(&self) -> Option<(usize, u32)> {
        let pid = self.st.selected_pattern_id()?;
        let idx = self.st.project.patterns.iter().position(|p| p.id() == pid)?;
        match &self.st.project.patterns[idx] {
            daw_core::project::Pattern::PianoRoll(pr) => Some((idx, pr.ticks_per_bar * pr.bars as u32)),
            _ => None,
        }
    }

    /// Insert a recorded note into the open pattern (dedup on pitch+tick, keep
    /// sorted), then push the live project to the engine.
    pub(super) fn insert_recorded_note(&mut self, idx: usize, note: daw_core::project::Note) {
        // One label for the whole take: coalescing folds a run of notes into
        // a single undo step.
        self.undo.record("record MIDI", &self.st.project);
        if let daw_core::project::Pattern::PianoRoll(pr) = &mut self.st.project.patterns[idx] {
            if !pr.notes.iter().any(|n| n.pitch == note.pitch && n.tick == note.tick) {
                pr.notes.push(note);
                pr.notes.sort_by_key(|n| (n.tick, n.pitch));
            }
        }
        self.push_live();
    }

    /// Flush still-held notes and disarm. Safe to call when not recording.
    pub(super) fn end_midi_record(&mut self) {
        if !self.midi_recording {
            return;
        }
        self.recorder.punch_window = None;
        let ph = self.st.playhead();
        if let Some((idx, len)) = self.open_pr_pattern() {
            for note in self.recorder.stop(ph, len) {
                self.insert_recorded_note(idx, note);
            }
        }
        self.midi_recording = false;
        self.st.set_status("recording stopped");
    }

    /// Toggle real-time MIDI recording into the open piano-roll pattern. Arming
    /// auto-starts the transport and clears the pattern (replace mode).
    pub(super) fn toggle_midi_record(&mut self) {
        if self.midi_recording {
            self.end_midi_record();
            return;
        }
        let Some((idx, _len)) = self.open_pr_pattern() else {
            self.st.set_status("open a piano-roll pattern to record");
            return;
        };
        if !self.playing {
            if let Some(session) = &self.host.session {
                if let Err(e) = session.transport(daw_core::session::Command::Play) {
                    self.st.set_status(e.to_string());
                    return;
                }
            } else {
                let _ = self.send_command(SequencerCommand::Play);
            }
            self.playing = true;
        }
        self.undo.record("record MIDI", &self.st.project);
        let lr = self.st.project.loop_region.clone();
        if !lr.enabled {
            // Replace mode (legacy). With the loop region enabled we overdub:
            // each pass's notes layer into the pattern, nothing is cleared.
            if let daw_core::project::Pattern::PianoRoll(pr) = &mut self.st.project.patterns[idx] {
                pr.notes.clear();
            }
        }
        self.recorder.punch_window = if lr.punch_in || lr.punch_out {
            let (s, e) = lr.ticks_in(self.st.project.ticks_per_bar());
            Some((
                if lr.punch_in { s } else { 0 },
                if lr.punch_out { e } else { u32::MAX },
            ))
        } else {
            None
        };
        self.push_live();
        let ph = self.st.playhead();
        self.recorder.start(ph);
        self.piano_step_input = None;
        self.midi_recording = true;
        self.st.set_status("recording MIDI…");
    }

    /// Point live MIDI input at the selected track's mixer strip (the shared
    /// atomic carries a strip index = track index). Keep the previous musical
    /// track by ID when an audio track is selected or tracks move.
    pub(super) fn sync_midi_strip(&mut self) {
        use daw_core::{audio::MAX_STRIPS, project::TrackType};
        let playable = self.st.project.tracks.iter().take(MAX_STRIPS).enumerate()
            .filter(|(_, track)| track.track_type != TrackType::Audio);
        let target = playable.clone().find(|(_, track)| Some(&track.id) == self.st.selected_track.as_ref())
            .or_else(|| playable.clone().find(|(_, track)| Some(&track.id) == self.midi_track.as_ref()))
            .or_else(|| playable.into_iter().next());
        self.midi_track = target.map(|(_, track)| track.id.clone());
        // MAX_STRIPS is outside the mixer and distinct from the utility strip.
        self.host.midi_channel.store(target.map_or(MAX_STRIPS, |(index, _)| index) as u8, Ordering::Relaxed);
    }

    pub(super) fn selected_track_is_audio(&self) -> bool {
        self.st
            .selected_track
            .as_deref()
            .and_then(|id| self.st.project.tracks.iter().find(|t| t.id == id))
            .map(|t| t.track_type == daw_core::project::TrackType::Audio)
            .unwrap_or(false)
    }

    /// Rec pill: audio track → audio take flow; otherwise the MIDI-record flow.
    pub(super) fn toggle_record(&mut self) {
        use daw_core::audio::input::RecordPhase;
        if self.audio_take_active { self.stop_audio_take(); return; }
        if self.audio_take_finishing { self.st.set_status("finishing take…"); return; }
        if !self.selected_track_is_audio() {
            self.toggle_midi_record();
            return;
        }
        match self.audio_rec.phase {
            RecordPhase::Idle => self.arm_audio_record(),
            RecordPhase::Armed => self.start_audio_take(),
            RecordPhase::AwaitingRoll | RecordPhase::Recording => self.stop_audio_take(),
        }
    }

    /// First Rec press on an audio track: open the input stream, meter live.
    pub(super) fn arm_audio_record(&mut self) {
        let Some(track_id) = self.st.selected_track.clone() else { return };
        self.arm_audio_track(&track_id);
    }

    pub(super) fn arm_audio_track(&mut self, track_id: &str) {
        let Some(input) = self.effective_track_input(track_id) else { return };
        let mut cfg = self.config.clone();
        cfg.input_device = input.device.clone();
        cfg.input_channels = input.channels;
        match self.host.arm_input(&cfg, input.source) {
            Ok(cs) => {
                cs.reset_clip_latch();
                let name = cs.device_name.clone();
                let guitar = self.host.capture_uses_guitar(input.source);
                self.audio_rec.arm();
                self.armed_track = Some(track_id.to_owned());
                self.armed_input = Some(input);
                self.capture_guitar = guitar;
                self.st.set_status(format!("armed: {name} · {} — Rec to start, Stop to disarm", if guitar { if cfg.guitar.ampforge_enabled { "AmpForge" } else { "amp + cabinet" } } else { "dry input" }));
            }
            Err(e) => self.st.set_status(format!("input unavailable: {e}")),
        }
    }

    /// Re-open the capture stream after a device/channel change, never mid-take.
    pub(super) fn rearm_capture_if_armed(&mut self) {
        use daw_core::audio::input::RecordPhase;
        if !self.audio_take_active && !self.audio_take_finishing && self.host.capture.is_some() && self.audio_rec.phase == RecordPhase::Armed {
            let Some(track) = self.armed_track.clone() else { return };
            let monitoring = self.track_monitoring(&track);
            self.host.disarm_input();
            self.audio_rec.disarm();
            self.arm_audio_track(&track);
            if monitoring && self.host.capture.is_some() {
                self.enable_track_monitor(&track);
            }
        }
    }

    /// Second Rec press: queue the take and start the count-in (none if the
    /// transport is already rolling — punch-in).
    pub(super) fn start_audio_take(&mut self) {
        let Some(track_id) = self.st.selected_track.clone() else { return };
        // The route must belong to the track receiving this take.
        if self.armed_track.as_deref() != Some(track_id.as_str())
            || self.armed_input != self.effective_track_input(&track_id)
            || self.host.capture.is_none()
        {
            self.host.disarm_input();
            self.audio_rec.disarm();
            self.arm_audio_record();
            if self.audio_rec.phase != daw_core::audio::input::RecordPhase::Armed { return; }
        }
        if let Some(input) = self.effective_track_input(&track_id) {
            self.capture_guitar = self.host.capture_uses_guitar(input.source);
        }
        if let Err(e) = self.sync_capture_tone() {
            self.st.set_status(format!("cannot record guitar rack: {e}"));
            return;
        }
        let safe: String = track_id
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let stamp = daw_core::audio::input::utc_timestamp(now);
        // Session base: the writer appends "-p<N>.wav" per loop pass.
        let base = std::path::PathBuf::from(format!("recordings/{safe}-{stamp}"));
        if let Some(parent) = base.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Some(cs) = self.host.capture.as_ref() else { return };
        if self.config.guitar.ampforge_enabled {
            if let Some(plugin) = self.host.monitor.as_mut().and_then(|m| m.plugin.as_mut()) { plugin.hide_editor(); }
        }
        let bars = if self.playing { 0 } else { self.config.count_in_bars };
        if let Some(session) = &self.host.session {
            if let Err(e) = session.start_recording(bars, || cs.begin_session(base)) {
                self.st.set_status(e.to_string());
                return;
            }
        } else {
            cs.begin_session(base);
            let _ = self.send_command(SequencerCommand::RecordStart { count_in_bars: bars });
        }
        self.rec_track = Some(track_id.clone());
        self.audio_rec.begin();
        self.audio_take_active = true;
        self.playing = true;
        self.st.set_status(if bars > 0 {
            format!("count-in: {bars} bar(s)…")
        } else {
            "recording…".to_string()
        });
    }

    /// Punch out: finalize the take; the transport keeps rolling. The clip is
    /// placed when the TakeResult arrives in `poll_recording`.
    pub(super) fn stop_audio_take(&mut self) {
        self.audio_take_active = false;
        self.audio_take_finishing = true;
        if let Some(cs) = self.host.capture.as_ref() {
            cs.end_session();
        }
        let _ = self.send_command(SequencerCommand::RecordStop);
        self.st.set_status("finishing take…");
    }

    /// Per-frame: latch the roll tick, collect finished takes into clips.
    /// Returns true while the meter should repaint (capture stream open).
    pub(super) fn poll_recording(&mut self) -> bool {
        use daw_core::audio::input::{RecordPhase, TRANSPORT_ROLLING};
        // Acquire completion before draining results: a finished writer has
        // published every take, so reopening cannot discard an unread WAV.
        let writer_finished = self.host.capture.as_ref()
            .is_none_or(|c| !c.shared.session_active.load(Ordering::Acquire));
        if self.audio_rec.phase == RecordPhase::AwaitingRoll
            && self.host.transport_state.load(Ordering::Relaxed) == TRANSPORT_ROLLING
        {
            let tick = self.host.roll_start_tick.load(Ordering::Relaxed);
            self.audio_rec.rolled(tick);
            self.st.set_status("recording…");
        }

        let results: Vec<daw_core::audio::input::TakeResult> = self
            .host
            .capture
            .as_ref()
            .map(|c| c.take_rx.try_iter().collect())
            .unwrap_or_default();
        for take in results {
            if take.dropped_frames > 0 {
                self.st.set_status(format!("warning: {} input frames dropped", take.dropped_frames));
            }
            let placement =
                self.audio_rec.finish(take.start_tick, self.st.bpm(), self.config.latency_trim_ms, take.duration_sec);
            let Some((start_sec, offset_sec, len_sec)) = placement else {
                let _ = std::fs::remove_file(&take.path);
                self.st.set_status("take too short — discarded");
                continue;
            };
            // The take belongs to the track it was started on, even if the
            // selection moved while it recorded (or that track is gone).
            let target = self.rec_track.clone().or_else(|| self.st.selected_track.clone())
                .filter(|id| self.st.project.tracks.iter().any(|t| &t.id == id
                    && t.track_type == daw_core::project::TrackType::Audio));
            let Some(track_id) = target else {
                self.st.set_status(format!("take kept on disk (its track was removed): {}", take.path.display()));
                continue;
            };
            let path_str = take.path.to_string_lossy().into_owned();
            // Undo removes the clip from the project; the WAV stays on disk.
            self.undo.record("record take", &self.st.project);
            self.st.project.audio_clips.push(daw_core::project::AudioClip {
                track: track_id.clone(),
                path: path_str.clone(),
                start_sec,
                offset_sec,
                len_sec,
                gain: 1.0,
            });
            // Loop passes stack: silence the previous pass's clip (it stays
            // in the project for manual comping — nothing is destroyed).
            if let Some(prev) = self.audio_rec.last_clip.take() {
                self.st.project.set_audio_clip_gain(prev, 0.0);
            }
            self.audio_rec.last_clip = Some(self.st.project.audio_clips.len() - 1);
            self.audio_rec.pass += 1;
            // (pump section 0c caches the waveform peaks for the new path)
            self.push_live();
            if self.audio_rec.pass > 1 {
                self.st.set_status(format!("pass {} saved: {path_str}", self.audio_rec.pass));
            } else {
                self.st.set_status(format!("take saved: {path_str}"));
            }
        }
        if self.audio_take_finishing && writer_finished {
            self.audio_take_finishing = false;
            self.audio_rec.arm(); // Also recovers a count-in canceled before any frames arrived.
        }
        self.host.capture.is_some()
    }

    /// A small input-level meter, shown while the capture stream is open.
    pub(super) fn input_meter(&self) -> Option<gpui::Div> {
        let theme = self.theme();
        let (peak, clipped) = self.host.capture.as_ref()?.meter();
        let bars = 12usize;
        let lit = (peak.min(1.0) * bars as f32).round() as usize;
        let cells = (0..bars).map(move |i| {
            let on = i < lit;
            let color = if i >= bars - 2 { RECORD } else if i >= bars - 5 { METER_WARN } else { 0x6ee87a };
            div().w(px(4.)).h(px(12.)).rounded_sm().bg(rgb(if on { color } else { theme.panel_alt }))
        });
        Some(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(div().text_color(rgb(if clipped { RECORD } else { theme.text_dim })).child(if clipped { "CLIP" } else { "IN" }))
                .children(cells),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::playhead_time;

    #[test]
    fn playhead_time_displays_milliseconds_at_the_current_tempo() {
        assert_eq!(playhead_time(0, 92.), "00:00.000");
        assert_eq!(playhead_time(480, 92.), "00:02.609");
        assert_eq!(playhead_time(14_399, 120.), "00:59.996");
        assert_eq!(playhead_time(14_400, 120.), "01:00.000");
        assert_eq!(playhead_time(864_000, 120.), "01:00:00.000");
    }
}

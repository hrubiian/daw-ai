use super::*;

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
            if matches!(self.audio_rec.phase, RecordPhase::AwaitingRoll | RecordPhase::Recording) {
                self.stop_audio_take();
            }
        }
        let cmd = if self.playing {
            SequencerCommand::Play
        } else {
            SequencerCommand::Pause
        };
        if let Some(session) = &self.host.session {
            if let Err(e) = session.transport(if self.playing { daw_core::session::Command::Play } else { daw_core::session::Command::Pause }) { self.st.set_status(e.to_string()); }
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
            match self.audio_rec.phase {
                RecordPhase::AwaitingRoll | RecordPhase::Recording => self.stop_audio_take(),
                // Stop while armed-but-idle disarms (closes the input stream).
                RecordPhase::Armed => {
                    self.audio_rec.disarm();
                    self.host.disarm_input();
                    self.st.set_status("record disarmed");
                }
                RecordPhase::Idle => {}
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
            self.playing = true;
            let _ = self.send_command(SequencerCommand::Play);
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
            let (s, e) = lr.ticks();
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
        self.midi_recording = true;
        self.st.set_status("recording MIDI…");
    }

    /// Point live MIDI input at the selected track's mixer strip (the shared
    /// atomic carries a strip index = track index). Audio tracks keep the
    /// previous target — there's nothing for a keyboard to play there.
    pub(super) fn sync_midi_strip(&self) {
        if let Some(idx) = self.st.selected_track.as_deref()
            .and_then(|id| self.st.project.tracks.iter().position(|t| t.id == id))
        {
            let is_audio = self.st.project.tracks[idx].track_type == daw_core::project::TrackType::Audio;
            if !is_audio {
                self.host.midi_channel.store(idx as u8, Ordering::Relaxed);
            }
        }
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
        match self.host.arm_input(&self.config) {
            Ok(cs) => {
                cs.reset_clip_latch();
                let name = cs.device_name.clone();
                self.audio_rec.arm();
                self.st.set_status(format!("armed: {name} — Rec to start, Stop to disarm"));
            }
            Err(e) => self.st.set_status(format!("input unavailable: {e}")),
        }
    }

    /// Re-open the capture stream after a device/channel change (armed only —
    /// never mid-take).
    pub(super) fn reopen_capture_if_armed(&mut self) {
        use daw_core::audio::input::RecordPhase;
        if self.host.capture.is_some() && self.audio_rec.phase == RecordPhase::Armed {
            self.host.disarm_input();
            self.arm_audio_record();
        }
    }

    /// Second Rec press: queue the take and start the count-in (none if the
    /// transport is already rolling — punch-in).
    pub(super) fn start_audio_take(&mut self) {
        let Some(track_id) = self.st.selected_track.clone() else { return };
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
        cs.begin_session(base);
        self.audio_rec.begin();
        let bars = if self.playing { 0 } else { self.config.count_in_bars };
        let _ = self.send_command(SequencerCommand::RecordStart { count_in_bars: bars });
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
            let Some(track_id) = self.st.selected_track.clone() else { continue };
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
        self.host.capture.is_some()
    }

    /// A small input-level meter, shown while the capture stream is open.
    pub(super) fn input_meter(&self) -> Option<gpui::Div> {
        let (peak, clipped) = self.host.capture.as_ref()?.meter();
        let bars = 12usize;
        let lit = (peak.min(1.0) * bars as f32).round() as usize;
        let cells = (0..bars).map(move |i| {
            let on = i < lit;
            let color = if i >= bars - 2 { RECORD } else if i >= bars - 5 { METER_WARN } else { 0x6ee87a };
            div().w(px(4.)).h(px(12.)).rounded_sm().bg(rgb(if on { color } else { PANEL_ALT }))
        });
        Some(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(div().text_color(rgb(if clipped { RECORD } else { TEXT_DIM })).child(if clipped { "CLIP" } else { "IN" }))
                .children(cells),
        )
    }
}

use super::*;
use daw_core::audio::input::{InputChannels, RecordPhase};
use daw_core::project::{RecordingSource, Track, TrackInput, TrackType};

impl DawView {
    fn input_device_info(&self, name: &str) -> Option<&daw_core::audio::input::InputDeviceInfo> {
        self.input_devices.iter().find(|device| device.name == name).or_else(|| {
            let card = daw_io::host::alsa_card_id(name)?;
            self.input_devices.iter().find(|device| daw_io::host::alsa_card_id(&device.name) == Some(card))
        })
    }

    pub(super) fn input_device_label<'a>(&'a self, name: Option<&'a str>) -> &'a str {
        let Some(name) = name else { return "System default"; };
        self.input_device_info(name).map_or(name, |device| device.label.as_str())
    }

    pub(super) fn input_channel_count(&self, name: Option<&str>) -> u16 {
        self.input_device_info(name.unwrap_or("default")).map_or(2, |device| device.channels).max(1)
    }

    pub(super) fn effective_track_input(&self, id: &str) -> Option<TrackInput> {
        let track = self.st.project.tracks.iter().find(|t| t.id == id)?;
        Some(track.input.clone().unwrap_or_else(|| TrackInput {
            device: self.config.input_device.clone(),
            channels: self.config.input_channels,
            source: RecordingSource::Auto,
        }))
    }

    pub(super) fn recording_route_locked(&self) -> bool {
        self.audio_take_active
            || self.audio_take_finishing
            || matches!(
                self.audio_rec.phase,
                RecordPhase::AwaitingRoll | RecordPhase::Recording
            )
    }

    pub(super) fn refresh_auto_capture(&mut self) {
        if !self.recording_route_locked() && self.host.capture.is_some() {
            if let Some(input) = self
                .armed_track
                .as_deref()
                .and_then(|id| self.effective_track_input(id))
            {
                self.capture_guitar = self.host.capture_uses_guitar(input.source);
                if let Err(e) = self.sync_capture_tone() {
                    self.st
                        .set_status(format!("recording tone unavailable: {e}"));
                }
            }
        }
    }

    /// Both monitor and recorder use GuitarRig, with the IR built at each
    /// stream's native rate. Existing takes already contain their printed tone.
    pub(super) fn sync_capture_tone(&mut self) -> anyhow::Result<()> {
        if self.capture_guitar && self.config.guitar.ampforge_enabled {
            if self.audio_take_active || self.audio_take_finishing { return Ok(()); }
            self.save_ampforge_state()?;
            let slot = self.config.guitar.active_ampforge()?.cloned().ok_or_else(|| anyhow::anyhow!("AmpForge is not configured"))?;
            let bpm = self.st.bpm();
            if let Some(capture) = &mut self.host.capture { capture.set_plugin(&slot)?; capture.pump_plugin(bpm); }
            return Ok(());
        }
        if let Some(capture) = &mut self.host.capture {
            let ir = if self.capture_guitar
                && self.config.guitar.amp.cabinet == daw_core::audio::amp::Cabinet::Ir
            {
                daw_io::cabinet::build_ir(&self.config.guitar, capture.sample_rate)?
            } else {
                Vec::new()
            };
            capture.set_guitar(self.capture_guitar.then_some(self.config.guitar.amp), &ir);
        }
        Ok(())
    }

    fn set_track_input(&mut self, id: &str, input: Option<TrackInput>) {
        if self.recording_route_locked() {
            self.st
                .set_status("finish the current take before changing recording inputs");
            return;
        }
        let Some(index) = self.st.project.tracks.iter().position(|t| t.id == id) else {
            return;
        };
        if self.st.project.tracks[index].input == input {
            return;
        }
        self.undo.record("track input", &self.st.project);
        self.st.project.tracks[index].input = input;
        if self.armed_track.as_deref() == Some(id) {
            self.rearm_capture_if_armed();
        }
        self.push_live();
    }

    pub(super) fn set_default_input(&mut self, device: Option<String>, channels: InputChannels) {
        if self.config.input_device == device && self.config.input_channels == channels {
            return;
        }
        if self.recording_route_locked() {
            self.st.set_status("finish the current take before changing recording inputs");
            return;
        }
        let previous_device = std::mem::replace(&mut self.config.input_device, device);
        let previous_channels = std::mem::replace(&mut self.config.input_channels, channels);
        if self.config.guitar.input.is_none() && self.host.monitor.is_some() {
            let _ = self.save_ampforge_state();
            if !self.start_monitor() {
                self.config.input_device = previous_device;
                self.config.input_channels = previous_channels;
                if self.host.monitor.is_none() {
                    let error = self.st.status.clone();
                    self.start_monitor();
                    self.st.set_status(error);
                }
                return;
            }
            self.refresh_auto_capture();
        }
        let _ = self.config.save();
        if self.armed_track.as_deref().and_then(|id| self.effective_track_input(id)) != self.armed_input {
            self.rearm_capture_if_armed();
        }
    }

    pub(super) fn track_monitoring(&self, track_id: &str) -> bool {
        self.armed_track.as_deref() == Some(track_id)
            && self.host.capture.as_ref().is_some_and(|capture| capture.is_monitoring())
    }

    pub(super) fn validate_track_monitor(&self) {
        if let Some(track_id) = &self.armed_track {
            if self.track_monitoring(track_id)
                && (self.effective_track_input(track_id) != self.armed_input
                    || !self.st.project.tracks.iter().any(|track| track.id == *track_id && track.track_type == TrackType::Audio))
            {
                self.host.disable_track_monitor();
            }
        }
    }

    pub(super) fn enable_track_monitor(&mut self, track_id: &str) {
        match self.host.enable_track_monitor(track_id) {
            Ok(()) => self.st.set_status("Monitoring input through track FX · Rec to record"),
            Err(error) => self.st.set_status(format!("Input monitor: {error}")),
        }
    }

    fn toggle_track_monitor(&mut self, track_id: &str) {
        if self.track_monitoring(track_id) {
            self.host.disable_track_monitor();
            self.st.set_status("Input monitoring off");
            return;
        }
        if self.host.sample_rate.load(Ordering::Acquire) == 0 {
            self.st.set_status("Audio output is unavailable");
            return;
        }
        let use_armed_input = self.armed_track.as_deref() == Some(track_id)
            && self.armed_input == self.effective_track_input(track_id)
            && self.host.capture.is_some();
        if !use_armed_input {
            if self.recording_route_locked() {
                self.st.set_status("Finish the current take before monitoring another input");
                return;
            }
            self.host.disarm_input();
            self.audio_rec.disarm();
            self.armed_track = None;
            self.armed_input = None;
            self.arm_audio_track(track_id);
        }
        if self.host.capture.is_some() {
            self.st.select_track(track_id);
            self.sync_midi_strip();
            self.enable_track_monitor(track_id);
        }
    }

    pub(super) fn track_monitor_button(&self, track: &Track, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme();
        let id = track.id.clone();
        theme.cell_btn_with_help(
            format!("track-monitor-{id}"),
            "Monitor",
            self.track_monitoring(&id),
            self.accent(),
            "Hear the live input through this track's effects".into(),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.toggle_track_monitor(&id);
            cx.stop_propagation();
            cx.notify();
        }))
    }

    pub(super) fn track_input_button(
        &self,
        track: &Track,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme();
        let route = self
            .effective_track_input(&track.id)
            .expect("project track");
        let device = self.input_device_label(route.device.as_deref());
        let channel = channel_label(route.channels);
        let source = source_label(route.source);
        let label = format!("In: {channel} ▾");
        let tip = SharedString::from(format!("Track input: {device} · {channel} · {source}"));
        theme.cell_btn_with_help(
            format!("track-input-{}", track.id),
            label,
            track.input.is_some(),
            self.accent(),
            tip,
        )
        .on_click(cx.listener({
            let id = track.id.clone();
            move |this, _, _, cx| {
                this.input_track = Some(id.clone());
                this.st.open_menu = None;
                this.scan_input_devices();
                cx.stop_propagation();
                cx.notify();
            }
        }))
    }

    pub(super) fn track_input_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let theme = self.theme();
        let id = self.input_track.as_ref()?;
        let track = self.st.project.tracks.iter().find(|t| &t.id == id)?;
        let route = self.effective_track_input(id)?;
        let max_channels = self.input_channel_count(route.device.as_deref());
        let locked = self.recording_route_locked();
        let mut panel = overlay_surface()
            .id("track-input-panel")
            .w(px(560.))
            .max_w(gpui::relative(0.95))
            .max_h(gpui::relative(0.85))
            .overflow_y_scroll()
            .p_4()
            .rounded_md()
            .bg(rgb(theme.panel))
            .text_color(rgb(theme.text))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(format!("Track input · {}", track.name)),
                    )
                    .child(
                        theme.cell_btn("track-input-close", "Close", false, theme.panel_alt).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.input_track = None;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(div().text_color(rgb(theme.text_dim)).child(format!(
                "{} · {} · {}",
                route.device.as_deref().unwrap_or("System default"),
                channel_label(route.channels),
                source_label(route.source)
            )));
        if track.track_type != daw_core::project::TrackType::Audio {
            panel = panel.child(div().text_color(rgb(theme.text_dim)).child("Audio input is used when this track's type is Audio. MIDI recording uses the keyboard input."));
        }
        if locked {
            panel = panel.child(
                div()
                    .text_color(rgb(theme.record_text))
                    .child("Finish the current take before changing inputs."),
            );
        }
        panel = panel
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        theme.cell_btn(
                            "track-input-inherit",
                            "Use recording default",
                            track.input.is_none(),
                            self.accent(),
                        )
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| {
                                this.set_track_input(&id, None);
                                cx.notify();
                            }
                        })),
                    )
                    .child(
                        theme.cell_btn(
                            "track-input-refresh",
                            if self.input_scan_running {
                                "Scanning…"
                            } else {
                                "Refresh devices"
                            },
                            false,
                            theme.panel_alt,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.scan_input_devices();
                            cx.notify();
                        })),
                    ),
            )
            .child(theme.section_title("Input device"));
        let devices = std::iter::once((None, "System default".to_string())).chain(
            self.input_devices.iter().filter(|d| d.name != "default").map(|d| {
                (
                    Some(d.name.clone()),
                    format!("{} · {} inputs", d.label, d.channels),
                )
            }),
        );
        panel = panel.child(
            div()
                .id("track-input-devices")
                .max_h(px(180.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1()
                .children(devices.enumerate().map(|(i, (device, label))| {
                    let id = id.clone();
                    let mut next = route.clone();
                    // A newly selected device starts on input 1. Never silently
                    // clamp a saved channel when reopening a disconnected route.
                    if next.device != device {
                        next.device = device.clone();
                        next.channels = InputChannels::Mono(0);
                    }
                    theme.cell_btn(
                        format!("track-input-device-{i}"),
                        label,
                        route.device == device,
                        self.accent(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_track_input(&id, Some(next.clone()));
                        cx.notify();
                    }))
                })),
        );
        let channels = audio_channel_choices(max_channels);
        panel = panel.child(theme.section_title("Input channel"))
            .child(div().flex().flex_wrap().gap_1().children(channels.enumerate().map(|(i, channel)| {
                let id = id.clone();
                let mut next = route.clone();
                next.channels = channel;
                theme.cell_btn(format!("track-input-channel-{i}"), channel_label(channel), route.channels == channel, self.accent())
                    .on_click(cx.listener(move |this, _, _, cx| { this.set_track_input(&id, Some(next.clone())); cx.notify(); }))
            })))
            .child(theme.section_title("Record sound"))
            .child(div().flex().flex_wrap().gap_1().children([RecordingSource::Auto, RecordingSource::Guitar, RecordingSource::Dry].into_iter().enumerate().map(|(i, source)| {
                let id = id.clone();
                let mut next = route.clone();
                next.source = source;
                theme.cell_btn(format!("track-input-source-{i}"), source_label(source), route.source == source, self.accent())
                    .on_click(cx.listener(move |this, _, _, cx| { this.set_track_input(&id, Some(next.clone())); cx.notify(); }))
            })))
            .child(div().text_color(rgb(theme.text_dim)).child("Dry saves the input as heard at the interface. Guitar rack prints the amp and cabinet. Auto uses the rack for the same mono input as the active guitar monitor. Monitor plays this track’s inserts live."));
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    overlay_surface()
                        .id("track-input-scrim")
                        .absolute()
                        .inset_0()
                        .bg(gpui::rgba(0x00000088))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.input_track = None;
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
                .child(panel.relative())
                .into_any_element(),
        )
    }
}

pub(super) fn audio_channel_choices(count: u16) -> impl Iterator<Item = InputChannels> {
    (0..count).map(InputChannels::Mono).chain(
        (0..count.saturating_sub(1)).step_by(2).map(|channel| InputChannels::Stereo(channel, channel + 1)),
    )
}

pub(super) fn channel_label(channels: InputChannels) -> String {
    match channels {
        InputChannels::Mono(c) => format!("Mono {}", u32::from(c) + 1),
        InputChannels::Stereo(l, r) => {
            format!("Stereo {} / {}", u32::from(l) + 1, u32::from(r) + 1)
        }
    }
}

fn source_label(source: RecordingSource) -> &'static str {
    match source {
        RecordingSource::Auto => "Auto (match guitar input)",
        RecordingSource::Guitar => "Guitar rack (amp + cab)",
        RecordingSource::Dry => "Dry input",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn keyboard_target_stays_on_the_instrument_while_editing_audio(cx: &mut gpui::TestAppContext) {
        let (host, _commands, _audio) = Host::headless();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut project = Project::default_project();
            project.add_track(TrackType::Audio);
            DawView::new(host, project, None, String::new(), cx)
        });
        view.update(cx, |view, cx| {
            view.st.select_track("t2");
            view.sync_midi_strip();
            view.st.select_track("t3");
            view.sync_midi_strip();
            assert_eq!(view.midi_track.as_deref(), Some("t2"));
            assert_eq!(view.host.midi_channel.load(Ordering::Relaxed), 1);
            view.st.project.tracks.swap(0, 1);
            view.sync_midi_strip();
            assert_eq!(view.host.midi_channel.load(Ordering::Relaxed), 0);
            cx.notify();
        });
    }

    #[gpui::test]
    fn track_input_picker_saves_independent_routes_and_blocks_mid_take_changes(
        cx: &mut gpui::TestAppContext,
    ) {
        let (host, _commands, _audio) = Host::headless();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut project = Project::default_project();
            project.add_track(TrackType::Audio);
            let mut view = DawView::new(host, project, None, String::new(), cx);
            view.config = Default::default();
            view.sf2_missing = false;
            view.input_scan_running = true; // The test supplies devices without touching audio hardware.
            view.input_devices = vec![daw_core::audio::input::InputDeviceInfo {
                name: "Guitar interface".into(),
                label: "Guitar interface".into(),
                channels: 4,
                sample_rate: 48_000,
            }];
            view
        });
        cx.simulate_resize(size(px(1280.), px(800.)));
        cx.run_until_parked();
        let selected = view.read_with(cx, |v, _| v.st.selected_track.clone());
        assert!(cx.debug_bounds("track-monitor-t3").is_some());
        assert!(cx.debug_bounds("track-monitor-t2").is_none());
        for selector in ["track-input-t1", "track-input-t2"] {
            let button = cx
                .debug_bounds(selector)
                .expect("every track has an input selector");
            assert!(button.size.width > px(40.));
        }
        let input_button = cx.debug_bounds("track-input-t2").unwrap();
        cx.simulate_click(input_button.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        view.read_with(cx, |v, _| {
            assert_eq!(v.input_track.as_deref(), Some("t2"));
            assert_eq!(
                v.st.selected_track, selected,
                "input picker must not trigger track selection beneath it"
            );
            assert!(v.host.capture.is_none());
        });
        for button in [
            "track-input-device-1",
            "track-input-channel-1",
            "track-input-source-1",
        ] {
            let bounds = cx.debug_bounds(button).expect("input choice is visible");
            cx.simulate_click(bounds.center(), gpui::Modifiers::default());
            cx.run_until_parked();
        }
        view.update(cx, |v, _| {
            let route = v.st.project.tracks[1].input.clone().unwrap();
            assert_eq!(route.device.as_deref(), Some("Guitar interface"));
            assert_eq!(route.channels, InputChannels::Mono(1));
            assert_eq!(route.source, RecordingSource::Guitar);
            assert!(v.st.project.tracks[0].input.is_none());
            let json = serde_json::to_value(&v.st.project).unwrap();
            assert!(
                json["tracks"][0].get("input").is_none(),
                "old tracks retain the existing format"
            );
            let loaded: Project = serde_json::from_value(json).unwrap();
            assert_eq!(loaded.tracks[1].input, Some(route.clone()));
            assert!(loaded.tracks[0].input.is_none());
            v.audio_take_active = true;
            v.audio_rec.phase = RecordPhase::Armed; // A completed loop pass can leave this phase armed.
            v.set_track_input("t2", None);
            assert_eq!(v.st.project.tracks[1].input, Some(route));
            v.audio_take_active = false;
            v.audio_take_finishing = true;
            v.set_track_input("t2", None);
            assert!(
                v.st.project.tracks[1].input.is_some(),
                "pending WAVs must be collected before rearming"
            );
            v.audio_rec.phase = RecordPhase::AwaitingRoll;
            v.poll_recording(); // Canceled count-in: no capture and no TakeResult.
            assert!(!v.audio_take_finishing);
            assert_eq!(v.audio_rec.phase, RecordPhase::Armed);
        });
        cx.simulate_keystrokes("escape");
        view.read_with(cx, |v, _| assert!(v.input_track.is_none()));
    }

    #[test]
    fn auto_prints_the_rack_when_monitoring_and_explicit_sources_override_it() {
        assert!(RecordingSource::Auto.uses_guitar(true));
        assert!(!RecordingSource::Auto.uses_guitar(false));
        assert!(RecordingSource::Guitar.uses_guitar(false));
        assert!(!RecordingSource::Dry.uses_guitar(true));
    }
}

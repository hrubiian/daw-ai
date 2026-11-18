use super::*;
use daw_core::audio::input::{InputChannels, RecordPhase};
use daw_core::project::{RecordingSource, Track, TrackInput};

impl DawView {
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
                self.capture_guitar = input.source.uses_guitar(self.host.monitor.is_some());
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
        self.undo.record("track input", &self.st.project);
        self.st.project.tracks[index].input = input;
        self.push_live();
        if self.armed_track.as_deref() == Some(id) {
            self.rearm_capture_if_armed();
        }
    }

    pub(super) fn track_input_button(
        &self,
        track: &Track,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let route = self
            .effective_track_input(&track.id)
            .expect("project track");
        let device = route.device.as_deref().unwrap_or("System default");
        let channel = channel_label(route.channels);
        let source = source_label(route.source);
        let label = format!("In: {} ▾", trunc(device, 12));
        let tip = SharedString::from(format!("Recording input: {device} · {channel} · {source}"));
        cell_btn_with_help(
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
                this.scan_guitar_inputs();
                cx.stop_propagation();
                cx.notify();
            }
        }))
    }

    pub(super) fn track_input_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let id = self.input_track.as_ref()?;
        let track = self.st.project.tracks.iter().find(|t| &t.id == id)?;
        let route = self.effective_track_input(id)?;
        let max_channels = self
            .input_devices
            .iter()
            .find(|d| route.device.as_deref() == Some(d.name.as_str()))
            .map(|d| d.channels)
            .unwrap_or(2)
            .max(1);
        let locked = self.recording_route_locked();
        let mut panel = overlay_surface()
            .id("track-input-panel")
            .w(px(560.))
            .max_w(gpui::relative(0.95))
            .max_h(gpui::relative(0.85))
            .overflow_y_scroll()
            .p_4()
            .rounded_md()
            .bg(rgb(PANEL))
            .text_color(rgb(TEXT))
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
                            .child(format!("Recording input · {}", track.name)),
                    )
                    .child(
                        cell_btn("track-input-close", "Close", false, PANEL_ALT).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.input_track = None;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(div().text_color(rgb(TEXT_DIM)).child(format!(
                "{} · {} · {}",
                route.device.as_deref().unwrap_or("System default"),
                channel_label(route.channels),
                source_label(route.source)
            )));
        if track.track_type != daw_core::project::TrackType::Audio {
            panel = panel.child(div().text_color(rgb(TEXT_DIM)).child("Audio input is used when this track's type is Audio. MIDI recording uses the keyboard input."));
        }
        if locked {
            panel = panel.child(
                div()
                    .text_color(rgb(RECORD))
                    .child("Finish the current take before changing inputs."),
            );
        }
        panel = panel
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        cell_btn(
                            "track-input-inherit",
                            "Use Settings input",
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
                        cell_btn(
                            "track-input-refresh",
                            if self.input_scan_running {
                                "Scanning…"
                            } else {
                                "Refresh devices"
                            },
                            false,
                            PANEL_ALT,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.scan_guitar_inputs();
                            cx.notify();
                        })),
                    ),
            )
            .child(section_title("Input device"));
        let devices = std::iter::once((None, "System default".to_string())).chain(
            self.input_devices.iter().map(|d| {
                (
                    Some(d.name.clone()),
                    format!("{} · {} channels", d.name, d.channels),
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
                    next.device = device.clone();
                    // A newly selected device starts on input 1. Never silently
                    // clamp a saved channel when reopening a disconnected route.
                    next.channels = InputChannels::Mono(0);
                    cell_btn(
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
        let channels = (0..max_channels).map(InputChannels::Mono).chain(
            (0..max_channels.saturating_sub(1))
                .step_by(2)
                .map(|c| InputChannels::Stereo(c, c + 1)),
        );
        panel = panel.child(section_title("Input channel"))
            .child(div().flex().flex_wrap().gap_1().children(channels.enumerate().map(|(i, channel)| {
                let id = id.clone();
                let mut next = route.clone();
                next.channels = channel;
                cell_btn(format!("track-input-channel-{i}"), channel_label(channel), route.channels == channel, self.accent())
                    .on_click(cx.listener(move |this, _, _, cx| { this.set_track_input(&id, Some(next.clone())); cx.notify(); }))
            })))
            .child(section_title("Record sound"))
            .child(div().flex().flex_wrap().gap_1().children([RecordingSource::Auto, RecordingSource::Guitar, RecordingSource::Dry].into_iter().enumerate().map(|(i, source)| {
                let id = id.clone();
                let mut next = route.clone();
                next.source = source;
                cell_btn(format!("track-input-source-{i}"), source_label(source), route.source == source, self.accent())
                    .on_click(cx.listener(move |this, _, _, cx| { this.set_track_input(&id, Some(next.clone())); cx.notify(); }))
            })))
            .child(div().text_color(rgb(TEXT_DIM)).child("Guitar rack prints the current amp and cabinet into the WAV, even with monitoring off. Auto uses the rack if monitoring is on when armed. Dry saves the unprocessed input. Track inserts apply on playback."));
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

fn channel_label(channels: InputChannels) -> String {
    match channels {
        InputChannels::Mono(c) => format!("Mono {}", u32::from(c) + 1),
        InputChannels::Stereo(l, r) => {
            format!("Stereo {} / {}", u32::from(l) + 1, u32::from(r) + 1)
        }
    }
}

fn source_label(source: RecordingSource) -> &'static str {
    match source {
        RecordingSource::Auto => "Auto (follow amp)",
        RecordingSource::Guitar => "Guitar rack (amp + cab)",
        RecordingSource::Dry => "Dry input",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn track_input_picker_saves_independent_routes_and_blocks_mid_take_changes(
        cx: &mut gpui::TestAppContext,
    ) {
        let (host, _commands, _audio) = Host::headless();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = DawView::new(host, Project::default_project(), None, String::new(), cx);
            view.config = Default::default();
            view.sf2_missing = false;
            view.input_scan_running = true; // The test supplies devices without touching audio hardware.
            view.input_devices = vec![daw_core::audio::input::InputDeviceInfo {
                name: "Guitar interface".into(),
                channels: 4,
                sample_rate: 48_000,
            }];
            view
        });
        cx.simulate_resize(size(px(1280.), px(800.)));
        cx.run_until_parked();
        let selected = view.read_with(cx, |v, _| v.st.selected_track.clone());
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

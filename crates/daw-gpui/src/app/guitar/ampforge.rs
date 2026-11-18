use super::*;
use daw_core::audio::live_plugin::AMPFORGE_ID;

impl DawView {
    pub(in crate::app) fn save_ampforge_state(&mut self) -> anyhow::Result<()> {
        if let Some(plugin) = self.host.monitor.as_mut().and_then(|m| m.plugin.as_mut()) {
            let snapshot = plugin.snapshot();
            if self.config.guitar.ampforge.as_ref() != Some(&snapshot) {
                self.config.guitar.ampforge = Some(snapshot);
                self.config.save()?;
            }
        }
        Ok(())
    }

    pub(in crate::app) fn pump_ampforge(&mut self) -> bool {
        let bpm = self.st.bpm();
        if let Some(capture) = &mut self.host.capture {
            capture.pump_plugin(bpm);
        }
        let active =
            if let Some(plugin) = self.host.monitor.as_mut().and_then(|m| m.plugin.as_mut()) {
                plugin.set_tempo(bpm);
                let _ = plugin.pump();
                true
            } else {
                false
            };
        // Some plugins do not mark state-only file changes dirty. Polling
        // snapshots also catches replaced cabinet/NAM paths; unchanged states
        // never write the config file.
        if active && self.ampforge_save_at.is_none() {
            self.ampforge_save_at = Some(std::time::Instant::now());
        }
        if self
            .ampforge_save_at
            .is_some_and(|t| t.elapsed() >= std::time::Duration::from_millis(750))
        {
            self.ampforge_save_at = None;
            let before = self.config.guitar.ampforge.clone();
            if let Err(e) = self.save_ampforge_state() {
                self.st
                    .set_status(format!("could not save AmpForge tone: {e}"));
                return true;
            }
            return before != self.config.guitar.ampforge;
        }
        false
    }

    fn select_guitar_backend(&mut self, ampforge: bool) {
        if self.recording_route_locked() {
            self.st
                .set_status("finish the current take before changing guitar backends");
            return;
        }
        if let Err(e) = self.save_ampforge_state() {
            self.st.set_status(e.to_string());
            return;
        }
        let mut candidate = self.config.guitar.clone();
        if ampforge
            && candidate
                .ampforge
                .as_ref()
                .is_none_or(|s| !std::path::Path::new(&s.path).is_file())
        {
            let Some(plugin) = self.st.clap_plugins.iter().find(|p| p.id == AMPFORGE_ID) else {
                self.start_clap_scan();
                self.st
                    .set_status("scanning for AmpForge; select it again when the scan finishes");
                return;
            };
            if let Some(slot) = &mut candidate.ampforge {
                slot.path = plugin.path.clone();
            } else {
                candidate.ampforge = Some(daw_core::project::ClapSlot {
                    uid: "guitar-ampforge".into(),
                    path: plugin.path.clone(),
                    id: plugin.id.clone(),
                    state: None,
                    params: Default::default(),
                });
            }
        }
        candidate.ampforge_enabled = ampforge;
        self.apply_guitar_config(
            candidate,
            if ampforge {
                "AmpForge selected; enable monitoring and open its editor"
            } else {
                "built-in guitar rack selected"
            }
            .into(),
        );
    }

    pub(super) fn guitar_backend(&self, cx: &mut Context<Self>) -> gpui::Div {
        let selected = self.config.guitar.ampforge_enabled;
        div().flex().flex_col().gap_2().p_3().bg(rgb(PANEL))
            .child(section_title("Guitar processor"))
            .child(div().flex().flex_wrap().gap_2()
                .child(cell_btn("guitar-backend-builtin", "Built-in rack", !selected, self.accent())
                    .on_click(cx.listener(|this, _, _, cx| { this.select_guitar_backend(false); cx.notify(); })))
                .child(cell_btn("guitar-backend-ampforge", "AmpForge", selected, self.accent())
                    .on_click(cx.listener(|this, _, _, cx| { this.select_guitar_backend(true); cx.notify(); })))
                .child(cell_btn("ampforge-rescan", "Rescan plugins", false, PANEL_ALT)
                    .on_click(cx.listener(|this, _, _, cx| { this.start_clap_scan(); cx.notify(); }))))
            .child(div().text_color(rgb(TEXT_DIM)).child("AmpForge uses its own amp, cabinet and pedals for live monitoring and new guitar-rack takes."))
    }

    pub(super) fn ampforge_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let on = self.host.monitor.is_some();
        let choices = daw_core::audio::monitor::BUFFER_CHOICES;
        let index = choices
            .iter()
            .position(|n| *n == self.config.guitar.buffer_frames)
            .unwrap_or(1);
        let previous = choices[index.saturating_sub(1)];
        let next = choices[(index + 1).min(choices.len() - 1)];
        let same_output = self.config.guitar.output_same_as_input;
        let meter = self
            .host
            .monitor
            .as_ref()
            .map(|m| {
                let stats = m.stats();
                format!(
                    "Input {:.0}% · output {:.0}% · ~{:.1} ms{}",
                    stats.in_peak * 100.,
                    stats.out_peak * 100.,
                    stats.latency_ms(),
                    if stats.in_clipped {
                        " · INPUT CLIP"
                    } else {
                        ""
                    }
                )
            })
            .unwrap_or_else(|| "Monitoring is off".into());
        div().flex().flex_col().gap_2().p_3().bg(rgb(PANEL))
            .child(section_title("AmpForge · live guitar"))
            .child(div().flex().flex_wrap().gap_2()
                .child(cell_btn("amp-monitor", if on { "Monitoring: on" } else { "Monitoring: off" }, on, self.accent())
                    .on_click(cx.listener(|this, _, _, cx| { this.toggle_monitor(); cx.notify(); })))
                .child(cell_btn("ampforge-editor", "Open AmpForge editor…", false, PANEL_ALT)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.recording_route_locked() {
                            this.st.set_status("finish the AmpForge take before editing its tone");
                        } else if let Some(plugin) = this.host.monitor.as_mut().and_then(|m| m.plugin.as_mut()) {
                            if let Err(e) = plugin.show_editor() { this.st.set_status(format!("AmpForge editor: {e}")); }
                        } else { this.st.set_status("enable monitoring first, then open the AmpForge editor"); }
                        cx.notify();
                    })))
                .child(cell_btn("ampforge-save", "Save AmpForge tone", false, PANEL_ALT)
                    .on_click(cx.listener(|this, _, _, cx| {
                        match this.save_ampforge_state() {
                            Ok(()) => this.st.set_status("AmpForge tone saved; use My presets to name this rig"),
                            Err(e) => this.st.set_status(e.to_string()),
                        }
                        cx.notify();
                    }))))
            .child(div().flex().flex_wrap().items_center().gap_2()
                .child(div().child(format!("Buffer: {} frames", self.config.guitar.buffer_frames)))
                .child(cell_btn("ampforge-buffer-down", "−", false, PANEL_ALT).on_click(cx.listener(move |this, _, _, cx| {
                    this.edit_monitor_device(|g| g.buffer_frames = previous); cx.notify();
                })))
                .child(cell_btn("ampforge-buffer-up", "+", false, PANEL_ALT).on_click(cx.listener(move |this, _, _, cx| {
                    this.edit_monitor_device(|g| g.buffer_frames = next); cx.notify();
                })))
                .child(cell_btn("ampforge-output", if same_output { "Output: this interface" } else { "Output: system default" }, same_output, self.accent())
                    .on_click(cx.listener(|this, _, _, cx| { this.edit_monitor_device(|g| g.output_same_as_input = !g.output_same_as_input); cx.notify(); }))))
            .child(div().text_color(rgb(TEXT_DIM)).child(meter))
            .child(div().text_color(rgb(TEXT_DIM)).child("Use AmpForge's editor for its pedals, cabinet WAVs, NAM models and presets. Cabinet/model files must remain at their saved paths."))
            .child(div().text_color(rgb(TEXT_DIM)).child("Recording: select Mono input and Guitar rack in the track input picker. The editor hides when a take starts; its tone is held for the take. Stop recording before editing."))
            .child(cell_btn("ampforge-project", "AmpForge downloads & source", false, PANEL_ALT)
                .on_click(|_, _, cx| cx.open_url("https://github.com/Loursy/AmpForge")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn backend_selection_exposes_ampforge_controls_and_locks_during_a_take(
        cx: &mut gpui::TestAppContext,
    ) {
        let (host, _commands, _audio) = Host::headless();
        let (owner, main) = cx.add_window_view(|_, cx| {
            let mut view = DawView::new(host, Project::default_project(), None, String::new(), cx);
            view.input_scan_running = true;
            view.config = Default::default();
            view.st.clap_plugins = vec![daw_core::audio::clap::ClapPluginDesc {
                path: "/plugins/ampforge.clap".into(),
                id: AMPFORGE_ID.into(),
                name: "AmpForge".into(),
                vendor: "AmpForge".into(),
                is_instrument: false,
                is_effect: true,
            }];
            view
        });
        owner.update(main, |v, cx| v.open_guitar_window(cx));
        main.run_until_parked();
        let handle = owner.read_with(main, |v, _| v.guitar_window.unwrap());
        let mut setup = gpui::VisualTestContext::from_window(handle.into(), main);
        setup.simulate_resize(size(px(1080.), px(1000.)));
        setup.run_until_parked();
        let button = setup.debug_bounds("guitar-backend-ampforge").unwrap();
        setup.simulate_click(button.center(), gpui::Modifiers::default());
        setup.run_until_parked();
        assert!(setup.debug_bounds("ampforge-editor").is_some());
        assert!(setup.debug_bounds("ampforge-buffer-up").is_some());
        // GPUI 0.2 retains old debug selectors across frames, so absence of
        // an old built-in selector cannot test conditional visibility here.
        owner.update(&mut setup, |v, _| {
            assert!(v.config.guitar.ampforge_enabled);
            assert!(
                v.host.monitor.is_none(),
                "choosing a backend must not open an input"
            );
            v.audio_take_active = true;
            v.select_guitar_backend(false);
            assert!(
                v.config.guitar.ampforge_enabled,
                "active take keeps its backend"
            );
            v.audio_take_active = false;
            v.select_guitar_backend(false);
            assert!(!v.config.guitar.ampforge_enabled);
            assert!(v.config.guitar.ampforge.is_some());
        });
    }
}

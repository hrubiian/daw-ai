//! Guitar amp: low-latency input monitoring with an amp and cabinet.

use super::*;
use daw_core::audio::amp::{AmpSettings, Cabinet};
mod rack;
mod pedals;
mod ampforge;

impl DawView {
    pub(super) fn open_guitar_window(&mut self, cx: &mut Context<Self>) {
        self.scan_guitar_inputs();
        let owner = cx.entity();
        // open_window renders synchronously. Defer until the transport's
        // DawView update has finished so the new view can read its owner.
        cx.defer(move |cx| {
            let existing = owner.read(cx).guitar_window;
            if let Some(handle) = existing {
                if handle.update(cx, |_, w, _| w.activate_window()).is_ok() {
                    return;
                }
            }
            let bounds = Bounds::centered(None, size(px(1080.), px(820.)), cx);
            let result = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("daw-ai · Guitar amp".into()),
                        ..Default::default()
                    }),
                    window_decorations: Some(gpui::WindowDecorations::Client),
                    window_min_size: Some(size(px(900.), px(560.))),
                    app_id: Some("daw-ai".into()),
                    ..Default::default()
                },
                |w, cx| cx.new(|cx| GuitarWindow::new(owner.clone(), w, cx)),
            );
            owner.update(cx, |owner, cx| {
                match result {
                    Ok(handle) => owner.guitar_window = Some(handle),
                    Err(e) => owner
                        .st
                        .set_status(format!("cannot open guitar amp window: {e}")),
                }
                cx.notify();
            });
        });
    }

    pub(super) fn scan_guitar_inputs(&mut self) {
        if self.input_scan_running {
            return;
        }
        self.input_scan_running = true;
        let result = self.input_scan.clone();
        std::thread::spawn(move || {
            let devices = daw_core::audio::input::list_input_devices();
            if let Ok(mut slot) = result.lock() {
                *slot = Some((devices, daw_io::list_midi_ports()));
            }
        });
    }

    pub(super) fn amp_setup_button(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        cell_btn("amp-setup", "Amp setup…", false, PANEL_ALT)
            .debug_selector(|| "amp-setup".into())
            .on_click(cx.listener(|this, _, _, cx| {
                this.st.open_menu = None;
                this.open_guitar_window(cx);
                cx.notify();
            }))
    }

    /// Switch the live guitar monitor on or off.
    pub(super) fn toggle_monitor(&mut self) {
        if self.host.monitor.is_some() {
            let _ = self.save_ampforge_state();
            self.host.stop_monitor();
            self.refresh_auto_capture();
            self.st.set_status("guitar amp off");
            return;
        }
        self.start_monitor();
        self.refresh_auto_capture();
    }

    /// (Re)open the monitor streams with the current settings.
    pub(super) fn start_monitor(&mut self) {
        match self.host.start_monitor(&self.config, self.st.bpm()) {
            Ok(m) => {
                let short = |name: &str| {
                    trunc(
                        name.trim_start_matches("alsa_input.")
                            .trim_start_matches("alsa_output."),
                        28,
                    )
                };
                let text = format!(
                    "guitar amp on: {} → {} — use headphones; speakers near a guitar feed back",
                    short(&m.input_name),
                    short(&m.output_name)
                );
                self.st.set_status(text);
            }
            Err(e) => self.st.set_status(format!("guitar amp unavailable: {e}")),
        }
    }

    /// Change an amp control: save it and send it to the running rig.
    pub(super) fn edit_amp(&mut self, f: impl FnOnce(&mut AmpSettings)) {
        f(&mut self.config.guitar.amp);
        let _ = self.config.save();
        if let Some(m) = &self.host.monitor {
            m.set(self.config.guitar.amp);
        }
        if self.capture_guitar && !self.config.guitar.ampforge_enabled {
            if let Some(capture) = &self.host.capture { capture.set_guitar_settings(self.config.guitar.amp); }
        }
    }

    /// Change a device-level setting (buffer, output): save and reopen.
    pub(super) fn edit_monitor_device(
        &mut self,
        f: impl FnOnce(&mut daw_io::config::GuitarConfig),
    ) {
        if self.recording_route_locked() && self.config.guitar.ampforge_enabled {
            self.st.set_status("finish the AmpForge take before changing its device settings"); return;
        }
        let _ = self.save_ampforge_state();
        f(&mut self.config.guitar);
        let _ = self.config.save();
        if self.host.monitor.is_some() {
            self.start_monitor();
        }
    }

    pub(super) fn pick_cabinet_ir(&mut self) {
        self.pick_cabinet_ir_for(false);
    }

    fn pick_cabinet_ir_for(&mut self, secondary: bool) {
        let slot = self.ir_pick.clone();
        std::thread::spawn(move || {
            if let Some(f) = rfd::FileDialog::new()
                .add_filter("Impulse response (WAV)", &["wav"])
                .pick_file()
            {
                *slot.lock().unwrap() = Some((secondary, f));
            }
        });
    }

    /// Drain a picked IR file: check it loads, remember it, use it.
    pub(super) fn poll_cabinet_ir(&mut self) -> bool {
        let Some((secondary, path)) = self.ir_pick.lock().ok().and_then(|mut s| s.take()) else {
            return false;
        };
        if secondary {
            self.select_cabinet_slot(&path.to_string_lossy(), true);
        } else {
            self.select_cabinet_ir(&path.to_string_lossy());
        }
        true
    }

    fn select_cabinet_ir(&mut self, source: &str) {
        self.select_cabinet_slot(source, false);
    }

    fn select_cabinet_slot(&mut self, source: &str, secondary: bool) {
        let mut candidate = self.config.guitar.clone();
        if secondary {
            candidate.cabinet.secondary_ir_path = Some(source.into());
        } else {
            candidate.ir_path = Some(source.into());
        }
        candidate
            .ir_path
            .get_or_insert_with(|| daw_io::cabinet::BUNDLED_IRS[0].id.into());
        candidate.amp.cabinet = Cabinet::Ir;
        // Validate even an inactive slot, before saving the path.
        let rate = self
            .host
            .monitor
            .as_ref()
            .map(|m| m.sample_rate())
            .unwrap_or(48000);
        if let Err(e) = daw_io::cabinet::load_ir(source, rate) {
            self.st.set_status(format!("cabinet IR not loaded: {e}"));
            return;
        }
        self.apply_guitar_config(
            candidate,
            format!(
                "cabinet IR {}: {}",
                if secondary { "B" } else { "A" },
                cabinet_name(source)
            ),
        );
    }

    /// Validate the whole cabinet before atomically changing the live tone.
    fn apply_guitar_config(&mut self, candidate: daw_io::config::GuitarConfig, status: String) {
        if self.recording_route_locked() && (self.config.guitar.ampforge_enabled || candidate.ampforge_enabled) {
            self.st.set_status("finish the AmpForge take before changing its tone"); return;
        }
        if let Err(e) = candidate.active_ampforge() { self.st.set_status(e.to_string()); return; }
        let restart = self.config.guitar.ampforge_enabled != candidate.ampforge_enabled
            || (candidate.ampforge_enabled && self.config.guitar.ampforge != candidate.ampforge);
        let rate = self
            .host
            .monitor
            .as_ref()
            .map(|m| m.sample_rate())
            .unwrap_or(48000);
        let ir = if !candidate.ampforge_enabled && candidate.amp.cabinet == Cabinet::Ir {
            daw_io::cabinet::build_ir(&candidate, rate)
        } else {
            Ok(Vec::new())
        };
        match ir {
            Ok(ir) => {
                self.config.guitar = candidate;
                let _ = self.config.save();
                if restart && self.host.monitor.is_some() {
                    self.start_monitor();
                    if self.host.monitor.is_none() { return; } // Keep the backend's load error visible.
                }
                else if !self.config.guitar.ampforge_enabled {
                    if let Some(m) = &self.host.monitor {
                        m.set_ir(ir);
                        m.set(self.config.guitar.amp);
                    }
                }
                if let Err(e) = self.sync_capture_tone() {
                    self.st.set_status(format!("recording tone unavailable: {e}"));
                    return;
                }
                self.st.set_status(status);
            }
            Err(e) => self.st.set_status(format!("cabinet IR not loaded: {e}")),
        }
    }

    /// One-line live readout for the transport pill, in half-millisecond
    /// steps so the button is not repainted for every tiny fluctuation.
    pub(super) fn monitor_readout(&self) -> Option<String> {
        let s = self.host.monitor.as_ref()?.stats();
        Some(if s.in_period == 0 || s.out_period == 0 {
            "starting…".to_string()
        } else {
            format!("{:.1} ms", (s.latency_ms() * 2.0).round() / 2.0)
        })
    }

    /// Transport button: shows the measured latency while on.
    pub(super) fn amp_pill(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let on = self.host.monitor.is_some();
        let label = match self.monitor_readout() {
            Some(r) => format!("🎸 Amp {r}"),
            None => "🎸 Amp".to_string(),
        };
        pill(
            label,
            if on { self.accent() } else { PANEL_ALT },
            if on { BG } else { TEXT },
        )
        .id("toggle-amp")
        .debug_selector(|| "toggle-amp".into())
        .cursor_pointer()
        .tooltip(|_, cx| {
            cx.new(|_| TextTooltip {
                text:
                    "Toggle guitar monitoring · use Amp setup… for input, tone and cabinet controls"
                        .into(),
            })
            .into()
        })
        .on_click(cx.listener(|this, _, _, cx| {
            this.toggle_monitor();
            cx.notify();
        }))
    }

    /// The "Guitar amp" block of the Settings panel.
    pub(super) fn guitar_settings(&self, cx: &mut Context<Self>) -> gpui::Div {
        if self.config.guitar.ampforge_enabled { return self.ampforge_controls(cx); }
        let accent = self.accent();
        let g = self.config.guitar.clone();
        let on = self.host.monitor.is_some();
        let knob = |id: &'static str,
                    name: &'static str,
                    value: f32,
                    get: fn(&mut AmpSettings) -> &mut f32,
                    cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .child(div().w(px(64.)).text_color(rgb(TEXT_DIM)).child(name))
                .child(
                    cell_btn(format!("amp-{id}-down"), "−", false, PANEL_ALT).on_click(
                        cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                            let step = if e.modifiers().shift { 0.01 } else { 0.05 };
                            this.edit_amp(|a| {
                                let v = get(a);
                                *v = (*v - step).clamp(0.0, 1.0);
                            });
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    div()
                        .w(px(44.))
                        .text_color(rgb(TEXT))
                        .child(format!("{:.0}", value * 100.0)),
                )
                .child(
                    cell_btn(format!("amp-{id}-up"), "+", false, PANEL_ALT).on_click(cx.listener(
                        move |this, e: &gpui::ClickEvent, _, cx| {
                            let step = if e.modifiers().shift { 0.01 } else { 0.05 };
                            this.edit_amp(|a| {
                                let v = get(a);
                                *v = (*v + step).clamp(0.0, 1.0);
                            });
                            cx.notify();
                        },
                    )),
                )
        };
        let status = match self.host.monitor.as_ref().map(|m| m.stats()) {
            Some(s) if s.in_period > 0 && s.out_period > 0 => format!(
                "{:.1} ms in software + your interface's converters · {} Hz · period {} · {} dropouts (the cushion grows after each, then holds){}",
                s.latency_ms(), s.sample_rate, s.in_period, s.underruns,
                if s.in_clipped { " · INPUT CLIPPING: turn the interface gain down" } else { "" }),
            Some(_) => "starting…".to_string(),
            None => "Monitoring off. Choose an input, then switch Monitoring on.".to_string(),
        };
        let cab_label = match g.amp.cabinet {
            Cabinet::Off => "Cabinet: off".to_string(),
            Cabinet::BuiltIn => "Cabinet: built-in 4×12".to_string(),
            Cabinet::Ir => format!(
                "Cabinet: {}",
                g.ir_path
                    .as_deref()
                    .map(cabinet_name)
                    .unwrap_or_else(|| "(none loaded — built-in used)".into())
            ),
        };
        let buffer_idx = daw_core::audio::monitor::BUFFER_CHOICES
            .iter()
            .position(|b| *b >= g.buffer_frames)
            .unwrap_or(1);
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .child(
                        cell_btn(
                            "amp-monitor",
                            if on {
                                "Monitoring: on"
                            } else {
                                "Monitoring: off"
                            },
                            on,
                            accent,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_monitor();
                            cx.notify();
                        })),
                    )
                    .child(
                        cell_btn(
                            "amp-on",
                            if g.amp.amp_on {
                                "Amp: on"
                            } else {
                                "Amp: bypassed"
                            },
                            g.amp.amp_on,
                            accent,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.edit_amp(|a| a.amp_on = !a.amp_on);
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(rgb(if on { TEXT } else { TEXT_DIM }))
                    .child(status),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .child(div().w(px(64.)).text_color(rgb(TEXT_DIM)).child("Buffer"))
                    .child(
                        cell_btn("amp-buf-down", "−", false, PANEL_ALT).on_click(cx.listener(
                            move |this, _, _, cx| {
                                let choices = daw_core::audio::monitor::BUFFER_CHOICES;
                                this.edit_monitor_device(|g| {
                                    g.buffer_frames = choices[buffer_idx.saturating_sub(1)]
                                });
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        div()
                            .w(px(84.))
                            .text_color(rgb(TEXT))
                            .child(format!("{} frames", g.buffer_frames)),
                    )
                    .child(
                        cell_btn("amp-buf-up", "+", false, PANEL_ALT).on_click(cx.listener(
                            move |this, _, _, cx| {
                                let choices = daw_core::audio::monitor::BUFFER_CHOICES;
                                this.edit_monitor_device(|g| {
                                    g.buffer_frames =
                                        choices[(buffer_idx + 1).min(choices.len() - 1)]
                                });
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        div()
                            .text_color(rgb(TEXT_DIM))
                            .child("smaller = less delay; raise it if you hear crackles"),
                    ),
            )
            .child(
                div().flex().items_center().gap_2().px_3().py_1().child(
                    cell_btn(
                        "amp-output",
                        if g.output_same_as_input {
                            "Output: this interface"
                        } else {
                            "Output: system default"
                        },
                        g.output_same_as_input,
                        accent,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.edit_monitor_device(|g| {
                            g.output_same_as_input = !g.output_same_as_input
                        });
                        cx.notify();
                    })),
                ),
            )
            .child(knob("gain", "Gain", g.amp.gain, |a| &mut a.gain, cx))
            .child(knob("bass", "Bass", g.amp.bass, |a| &mut a.bass, cx))
            .child(knob("mid", "Mid", g.amp.mid, |a| &mut a.mid, cx))
            .child(knob(
                "treble",
                "Treble",
                g.amp.treble,
                |a| &mut a.treble,
                cx,
            ))
            .child(knob("level", "Level", g.amp.level, |a| &mut a.level, cx))
            .child(knob("gate", "Noise gate", g.amp.gate, |a| &mut a.gate, cx))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(rgb(TEXT_DIM))
                    .child("Shift + click for fine steps · settings save automatically"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .child(
                        cell_btn("amp-cab", cab_label, g.amp.cabinet != Cabinet::Off, accent)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let mode = match this.config.guitar.amp.cabinet {
                                    Cabinet::BuiltIn => Cabinet::Ir,
                                    Cabinet::Ir => Cabinet::Off,
                                    Cabinet::Off => Cabinet::BuiltIn,
                                };
                                this.set_cabinet_mode(mode);
                                cx.notify();
                            })),
                    )
                    .child(
                        cell_btn("amp-ir", "Load IR…", false, PANEL_ALT).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.pick_cabinet_ir();
                                cx.notify();
                            },
                        )),
                    ),
            )
    }
}

fn cabinet_name(source: &str) -> String {
    daw_io::cabinet::bundled_ir(source)
        .map(|ir| ir.name.to_string())
        .unwrap_or_else(|| {
            std::path::Path::new(source)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| source.into())
        })
}

/// A second view of the same host/config; it never creates another audio rig.
pub(super) struct GuitarWindow {
    owner: gpui::WeakEntity<DawView>,
    _subscriptions: Vec<gpui::Subscription>,
    preset_name: String,
    preset_focus: FocusHandle,
    input_open: bool,
    plugins_open: bool,
}

impl GuitarWindow {
    fn new(owner: gpui::Entity<DawView>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&owner, |_, _, cx| cx.notify()),
            cx.observe_release_in(&owner, window, |_, _, w, _| w.remove_window()),
        ];
        // Refresh meters/dropouts even when latency stays constant. Only this
        // small window repaints, rather than the arrangement on every sample.
        cx.spawn(async move |weak, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(100))
                .await;
            if weak
                .update(cx, |this, cx| {
                    if this
                        .owner
                        .upgrade()
                        .is_some_and(|owner| owner.read(cx).host.monitor.is_some())
                    {
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
        Self {
            owner: owner.downgrade(),
            _subscriptions: subscriptions,
            preset_name: "My rig".into(),
            preset_focus: cx.focus_handle(),
            input_open: false,
            plugins_open: false,
        }
    }
}

impl GuitarWindow {
    fn preset_editor(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .py_2()
            .child(
                div()
                    .text_color(rgb(TEXT_DIM))
                    .child("Preset name (saving the same name replaces it)"),
            )
            .child(
                div()
                    .id("guitar-preset-name")
                    .track_focus(&self.preset_focus)
                    .p_2()
                    .bg(rgb(BG))
                    .border_1()
                    .border_color(rgb(TEXT_DIM))
                    .child(format!("{}{}", self.preset_name, "▏"))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, w, cx| {
                            this.preset_focus.focus(w);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                        cx.stop_propagation();
                        let modifiers = &ev.keystroke.modifiers;
                        if (modifiers.control || modifiers.platform) && ev.keystroke.key == "a" {
                            this.preset_name.clear();
                            cx.notify();
                            return;
                        }
                        if modifiers.control || modifiers.platform || modifiers.alt {
                            return;
                        }
                        match ev.keystroke.key.as_str() {
                            "backspace" => {
                                this.preset_name.pop();
                            }
                            "enter" => {
                                let name = this.preset_name.clone();
                                let _ = this.owner.update(cx, |owner, cx| {
                                    owner.save_guitar_preset(&name);
                                    cx.notify();
                                });
                                w.blur();
                            }
                            "escape" => w.blur(),
                            "space" => this.preset_name.push(' '),
                            _ => {
                                if this.preset_name.chars().count() < 64 {
                                    if let Some(ch) = &ev.keystroke.key_char {
                                        this.preset_name.push_str(ch);
                                    }
                                }
                            }
                        }
                        cx.notify();
                    })),
            )
            .child(
                cell_btn("guitar-save-preset", "Save current rig", false, ACCENT).on_click(
                    cx.listener(|this, _, _, cx| {
                        let name = this.preset_name.clone();
                        let _ = this.owner.update(cx, |owner, cx| {
                            owner.save_guitar_preset(&name);
                            cx.notify();
                        });
                    }),
                ),
            )
    }
}

impl Render for GuitarWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input_open = self.input_open;
        let plugins_open = self.plugins_open;
        let input_toggle = cell_btn(
            "guitar-input-toggle",
            if input_open {
                "Input & routing ▾"
            } else {
                "Input & routing ▸"
            },
            false,
            PANEL_ALT,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.input_open = !this.input_open;
            cx.notify();
        }));
        let plugins_toggle = cell_btn(
            "guitar-plugins-toggle",
            if plugins_open {
                "CLAP plugins ▾"
            } else {
                "CLAP plugins ▸"
            },
            false,
            PANEL_ALT,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.plugins_open = !this.plugins_open;
            cx.notify();
        }));
        let editor = self.preset_editor(cx);
        let (library, rack, status) = self
            .owner
            .update(cx, |owner, cx| {
                let rack = div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(input_toggle)
                    .when(input_open, |d| d.child(owner.guitar_input_settings(cx)))
                    .child(owner.guitar_backend(cx))
                    .when(owner.config.guitar.ampforge_enabled, |d| d.child(owner.ampforge_controls(cx)))
                    .when(!owner.config.guitar.ampforge_enabled, |d| d
                    .child(owner.pedal_starting_points(cx))
                    .child(owner.drive_pedals(cx))
                    .child(
                        div()
                            .bg(rgb(PANEL))
                            .py_2()
                            .child(section_title("Amp · gate · output"))
                            .child(owner.guitar_settings(cx)),
                    )
                    .child(owner.cabinet_builder(cx))
                    .child(owner.modulation_pedal(cx))
                    .child(owner.echo_pedal(cx))
                    .child(owner.reverb_pedal(cx)))
                    .child(plugins_toggle)
                    .when(plugins_open, |d| d.child(owner.guitar_plugins(cx)));
                (owner.guitar_library(cx), rack, owner.st.status.clone())
            })
            .unwrap_or_else(|_| (div(), div(), "Session closed".into()));
        let content = div()
            .id("guitar-window")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .text_sm()
            .child(
                div()
                    .p_3()
                    .bg(rgb(PANEL))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().text_xl().child("Guitar rack"))
                    .child(
                        div()
                            .text_color(rgb(TEXT_DIM))
                            .child("Live monitoring · editable tones · recording"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("guitar-library")
                            .w(px(250.))
                            .flex_shrink_0()
                            .overflow_y_scroll()
                            .p_3()
                            .bg(rgb(PANEL))
                            .child(library)
                            .child(editor),
                    )
                    .child(
                        div()
                            .id("guitar-rack-scroll")
                            .flex_1()
                            .min_w_0()
                            .overflow_y_scroll()
                            .p_3()
                            .child(rack),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .bg(rgb(PANEL))
                    .text_color(rgb(TEXT_DIM))
                    .child(status),
            );
        window_frame::frame(content, window, |_, w, _| w.remove_window())
    }
}

impl DawView {
    fn guitar_input_settings(&self, cx: &mut Context<Self>) -> gpui::Div {
        use daw_core::audio::input::InputChannels;
        let channel = match self.config.input_channels {
            InputChannels::Mono(c) | InputChannels::Stereo(c, _) => c,
        };
        let max_channels = self
            .input_devices
            .iter()
            .find(|d| self.config.input_device.as_deref() == Some(d.name.as_str()))
            .map(|d| d.channels)
            .unwrap_or(2)
            .max(1);
        let meter = self
            .host
            .monitor
            .as_ref()
            .map(|m| {
                let s = m.stats();
                let db = |v: f32| {
                    if v > 0.00001 {
                        format!("{:.0} dBFS", 20.0 * v.log10())
                    } else {
                        "−∞ dBFS".into()
                    }
                };
                format!("Input {} · Output {}", db(s.in_peak), db(s.out_peak))
            })
            .unwrap_or_else(|| "Input/output meters appear while monitoring".into());
        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .bg(rgb(PANEL))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child("Audio input"))
                    .child(
                        cell_btn(
                            "amp-input-refresh",
                            if self.input_scan_running {
                                "Scanning…"
                            } else {
                                "Refresh"
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
            .child(div().text_color(rgb(TEXT_DIM)).child(format!(
                    "Selected: {}",
                    self.config
                        .input_device
                        .as_deref()
                        .unwrap_or("System default")
                )))
            .child(
                cell_btn(
                    "amp-input-default",
                    "System default",
                    self.config.input_device.is_none(),
                    self.accent(),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.config.input_device = None;
                    this.config.input_channels = InputChannels::Mono(0);
                    let _ = this.config.save();
                    this.reopen_capture_if_armed();
                    cx.notify();
                })),
            )
            .child(
                div()
                    .id("amp-input-list")
                    .max_h(px(144.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(self.input_devices.iter().enumerate().map(|(i, device)| {
                        let name = device.name.clone();
                        cell_btn(
                            format!("amp-input-{i}"),
                            format!("{} ({} ch)", name, device.channels),
                            self.config.input_device.as_deref() == Some(name.as_str()),
                            self.accent(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.config.input_device = Some(name.clone());
                            this.config.input_channels = InputChannels::Mono(0);
                            let _ = this.config.save();
                            this.reopen_capture_if_armed();
                            cx.notify();
                        }))
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .child(
                        cell_btn("amp-input-down", "−", false, PANEL_ALT).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.config.input_channels =
                                    InputChannels::Mono(channel.saturating_sub(1));
                                let _ = this.config.save();
                                this.reopen_capture_if_armed();
                                cx.notify();
                            },
                        )),
                    )
                    .child(format!("Guitar: input {} (mono)", channel + 1))
                    .child(
                        cell_btn("amp-input-up", "+", false, PANEL_ALT).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.config.input_channels = InputChannels::Mono(
                                    channel.saturating_add(1).min(max_channels - 1),
                                );
                                let _ = this.config.save();
                                this.reopen_capture_if_armed();
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(div().text_color(rgb(TEXT_DIM)).child(meter))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn guitar_setup_is_one_shared_reopenable_window(cx: &mut gpui::TestAppContext) {
        let (host, _commands, _audio) = Host::headless();
        let (owner, main) = cx.add_window_view(|_, cx| {
            let mut view = DawView::new(host, Project::default_project(), None, String::new(), cx);
            view.input_scan_running = true; // no real devices in this UI test
            view.config = Default::default();
            view
        });
        main.run_until_parked();
        let button = main
            .debug_bounds("amp-setup")
            .expect("transport opens setup");
        main.simulate_click(button.center(), gpui::Modifiers::default());
        main.run_until_parked();
        let handle = owner.read_with(main, |v, _| {
            assert!(
                v.host.monitor.is_none(),
                "opening setup must not open the input"
            );
            v.guitar_window.unwrap()
        });
        let mut setup = gpui::VisualTestContext::from_window(handle.into(), main);
        setup.simulate_resize(size(px(1080.), px(2600.)));
        setup.run_until_parked();
        for id in [
            "amp-monitor",
            "amp-gain-up",
            "amp-ir",
            "cab-A-builtin:open-back-1x12",
            "cab-B-builtin:closed-back-4x12",
            "guitar-preset-Rock",
            "guitar-preset-Jazz",
            "guitar-preset-Metal",
            "guitar-preset-Thrash Rhythm",
            "pedal-preset-LeadEcho",
            "pedal-drive-Fuzz",
            "pedal-modulation-Flanger",
            "pedal-modulation-Chorus",
            "pedal-mod-rate-up",
            "pedal-echo-Tape",
            "pedal-echo-time-up",
            "pedal-reverb-Hall",
            "pedal-reverb-mix-up",
        ] {
            assert!(
                setup.debug_bounds(id).is_some(),
                "missing setup control {id}"
            );
        }
        assert_eq!(setup.windows().len(), 2);
        let thrash = setup.debug_bounds("guitar-preset-Thrash Rhythm").unwrap();
        setup.simulate_click(thrash.center(), gpui::Modifiers::default());
        setup.run_until_parked();
        owner.read_with(&setup, |v, _| {
            let preset = daw_io::guitar_presets::factory_presets().into_iter()
                .find(|p| p.name == "Thrash Rhythm").unwrap();
            assert!(preset.matches(&v.config.guitar), "selecting the preset loads the complete pedal/cabinet rig");
            assert!(v.host.monitor.is_none(), "choosing a tone must not enable monitoring");
        });
        setup.simulate_resize(size(px(900.), px(560.)));
        setup.run_until_parked();
        setup.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(px(700.), px(300.)),
            delta: gpui::ScrollDelta::Lines(gpui::point(0., -150.)),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        setup.run_until_parked();
        let reverb = setup.debug_bounds("pedal-reverb-mix-up").expect("reverb remains reachable by scrolling");
        assert!(reverb.origin.y >= px(0.) && reverb.bottom() <= px(560.));
        owner.update(&mut setup, |v, cx| {
            v.open_guitar_window(cx);
            v.config.guitar.amp.gain = 0.8;
            v.config.guitar.ir_path = Some(daw_io::cabinet::BUNDLED_IRS[1].id.into());
            v.config.guitar.amp.cabinet = Cabinet::Ir;
            cx.notify();
        });
        setup.run_until_parked();
        assert_eq!(
            setup.windows().len(),
            2,
            "reopening focuses the existing window"
        );
        handle
            .update(&mut setup, |v, _, cx| {
                v.owner
                    .update(cx, |owner, _| assert_eq!(owner.config.guitar.amp.gain, 0.8))
                    .unwrap();
            })
            .unwrap();
        // A load failure must preserve the chosen IR and bypass state.
        owner.update(&mut setup, |v, _| {
            let before = v.config.guitar.clone();
            v.select_cabinet_ir("/nonexistent/guitar-cab.wav");
            assert_eq!(v.config.guitar, before);
            assert!(v.st.status.starts_with("cabinet IR not loaded:"));
        });
        handle
            .update(&mut setup, |_, w, _| w.remove_window())
            .unwrap();
        setup.run_until_parked();
        assert_eq!(setup.windows().len(), 1);
        owner.update(main, |v, cx| v.open_guitar_window(cx));
        main.run_until_parked();
        assert_eq!(main.windows().len(), 2, "closed setup can be opened again");
        owner.read_with(main, |v, _| {
            assert_eq!(
                v.config.guitar.ir_path.as_deref(),
                Some(daw_io::cabinet::BUNDLED_IRS[1].id)
            );
            assert!(v.host.monitor.is_none());
        });
    }
}

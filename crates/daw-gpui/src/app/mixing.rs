use super::*;
use daw_core::project::{EffectParam, ReturnParam};

const STRIP_W: f32 = 136.;
const STRIP_H: f32 = 480.;
const LEVEL_H: f32 = 130.;

#[derive(Clone)]
enum MixLevel {
    Track(String),
    Return(usize),
}

fn db_label(gain: f32) -> String {
    if gain <= 0.00001 {
        "−∞ dB".into()
    } else {
        format!("{:+.1} dB", 20. * gain.log10())
    }
}

fn level_meter(peak: f32, clipped: bool, height: f32) -> gpui::Div {
    let fraction = if peak <= 0.00001 {
        0.
    } else {
        ((20. * peak.log10() + 60.) / 60.).clamp(0., 1.)
    };
    let color = if clipped {
        RECORD
    } else if peak >= 0.7 {
        METER_WARN
    } else {
        0x6ee87a
    };
    div()
        .w(px(10.))
        .h(px(height))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .justify_end()
        .bg(rgb(BG))
        .rounded_sm()
        .overflow_hidden()
        .child(div().w_full().h(px(height * fraction)).bg(rgb(color)))
}

fn channel_surface(color: u32, selected: bool) -> gpui::Div {
    div()
        .w(px(STRIP_W))
        .h(px(STRIP_H))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .bg(rgb(if selected { PANEL_ALT } else { PANEL }))
        .border_t_4()
        .border_color(rgb(color))
}

impl DawView {
    pub(super) fn start_mix_balance(&mut self) {
        if self.balance_running {
            return;
        }
        self.balance_running = true;
        let hash =
            daw_core::project::format::project_revision(&self.st.project).unwrap_or_default();
        let project = self
            .host
            .session
            .as_ref()
            .map(|s| s.runtime_project(&self.st.project))
            .unwrap_or_else(|| self.st.project.clone());
        let sf2 = PathBuf::from(&self.sf2_path);
        let slot = self.balance_result.clone();
        self.st.set_status("Analyzing track levels…");
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                daw_core::audio::balance::auto_balance(&project, &sf2)
            }))
            .map_err(|_| "balance worker panicked".to_string())
            .and_then(|r| r.map_err(|e| e.to_string()));
            if let Ok(mut slot) = slot.lock() {
                *slot = Some((hash, result));
            }
        });
    }

    fn mix_level_nudge(&mut self, target: &MixLevel, delta: f32) {
        match target {
            MixLevel::Track(id) => self.nudge_volume(id, delta),
            MixLevel::Return(i) => self.nudge_return(
                if *i == 0 {
                    ReturnParam::RevGain
                } else {
                    ReturnParam::DlyGain
                },
                delta,
            ),
        }
    }

    fn mix_level_control(
        &self,
        target: MixLevel,
        gain: f32,
        max: f32,
        peak: f32,
        clipped: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let id = match &target {
            MixLevel::Track(id) => id.clone(),
            MixLevel::Return(i) => format!("return-{i}"),
        };
        let (down, up, wheel) = (target.clone(), target.clone(), target);
        let selector = format!("mix-fader-{id}");
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .child(div().text_color(rgb(TEXT_DIM)).child("Level"))
            .child(
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .cursor_pointer()
                    .flex()
                    .items_end()
                    .gap_2()
                    .tooltip(|_, cx| {
                        cx.new(|_| TextTooltip {
                            text: "Wheel to adjust level · Shift for fine changes".into(),
                        })
                        .into()
                    })
                    .on_scroll_wheel(cx.listener(move |this, e: &gpui::ScrollWheelEvent, _, cx| {
                        cx.stop_propagation();
                        let dy = f32::from(e.delta.pixel_delta(px(16.)).y);
                        if dy != 0. {
                            this.mix_level_nudge(
                                &wheel,
                                dy.signum() * if e.modifiers.shift { 0.005 } else { 0.025 },
                            );
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .w(px(24.))
                            .h(px(LEVEL_H))
                            .relative()
                            .flex()
                            .flex_col()
                            .justify_end()
                            .rounded_sm()
                            .bg(rgb(BG))
                            .overflow_hidden()
                            .child(
                                div()
                                    .w_full()
                                    .h(px(LEVEL_H * (gain / max).clamp(0., 1.)))
                                    .bg(rgb(self.accent())),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .bottom(px(LEVEL_H / max))
                                    .w_full()
                                    .h(px(1.))
                                    .bg(rgb(TEXT)),
                            ),
                    )
                    .child(level_meter(peak, clipped, LEVEL_H)),
            )
            .child(div().text_color(rgb(TEXT)).child(db_label(gain)))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        cell_btn(format!("mix-level-down-{id}"), "−", false, PANEL_ALT).on_click(
                            cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                                this.mix_level_nudge(
                                    &down,
                                    if e.modifiers().shift { -0.005 } else { -0.05 },
                                );
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        cell_btn(format!("mix-level-up-{id}"), "+", false, PANEL_ALT).on_click(
                            cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                                this.mix_level_nudge(
                                    &up,
                                    if e.modifiers().shift { 0.005 } else { 0.05 },
                                );
                                cx.notify();
                            }),
                        ),
                    ),
            )
    }

    fn mix_send_control(
        &self,
        id: &str,
        param: EffectParam,
        label: &str,
        value: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let (down, up) = (id.to_string(), id.to_string());
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(11.))
                    .text_color(rgb(TEXT_DIM))
                    .child(format!("{label} {:.0}%", value * 100.)),
            )
            .child(
                cell_btn(format!("mix-send-down-{label}-{id}"), "−", false, PANEL_ALT).on_click(
                    cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                        this.undo.record("mix send", &this.st.project);
                        this.st.project.nudge_effect(
                            &down,
                            param,
                            if e.modifiers().shift { -0.005 } else { -0.025 },
                        );
                        this.push_effects(&down);
                        cx.notify();
                    }),
                ),
            )
            .child(
                cell_btn(format!("mix-send-up-{label}-{id}"), "+", false, PANEL_ALT).on_click(
                    cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                        this.undo.record("mix send", &this.st.project);
                        this.st.project.nudge_effect(
                            &up,
                            param,
                            if e.modifiers().shift { 0.005 } else { 0.025 },
                        );
                        this.push_effects(&up);
                        cx.notify();
                    }),
                ),
            )
    }

    fn mix_track_strip(
        &self,
        index: usize,
        peak: f32,
        clipped: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let t = &self.st.project.tracks[index];
        let (select, mute, solo, left, right, fx) = (
            t.id.clone(),
            t.id.clone(),
            t.id.clone(),
            t.id.clone(),
            t.id.clone(),
            t.id.clone(),
        );
        let name: SharedString = t.name.clone().into();
        let instrument: SharedString = instrument_label(t).into();
        let selected =
            self.st.selected_track.as_deref() == Some(&t.id) && self.st.selected_return.is_none();
        let selector = format!("mix-channel-{}", t.id);
        channel_surface(track_color(index), selected)
            .child(
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .h(px(38.))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .cursor_pointer()
                    .text_color(rgb(TEXT))
                    .child(t.name.clone())
                    .tooltip(move |_, cx| cx.new(|_| TextTooltip { text: name.clone() }).into())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.st.select_track(&select);
                        this.st.selected_return = None;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id(SharedString::from(format!("mix-instrument-{}", t.id)))
                    .h(px(18.))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(11.))
                    .text_color(rgb(TEXT_DIM))
                    .child(instrument_label(t))
                    .tooltip(move |_, cx| {
                        cx.new(|_| TextTooltip {
                            text: instrument.clone(),
                        })
                        .into()
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        cell_btn(format!("mix-mute-{}", t.id), "Mute", t.muted, RECORD).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.toggle_mute(&mute);
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        cell_btn(
                            format!("mix-solo-{}", t.id),
                            "Solo",
                            self.st.soloed.contains(&t.id),
                            self.accent(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_solo(&solo);
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        cell_btn(format!("mix-pan-left-{}", t.id), "‹", false, PANEL_ALT).on_click(
                            cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                                this.nudge_pan(
                                    &left,
                                    if e.modifiers().shift { -0.01 } else { -0.1 },
                                );
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .text_color(rgb(TEXT_DIM))
                            .child(pan_label(t.pan)),
                    )
                    .child(
                        cell_btn(format!("mix-pan-right-{}", t.id), "›", false, PANEL_ALT)
                            .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                                this.nudge_pan(
                                    &right,
                                    if e.modifiers().shift { 0.01 } else { 0.1 },
                                );
                                cx.notify();
                            })),
                    ),
            )
            .child(self.mix_send_control(
                &t.id,
                EffectParam::Reverb,
                "Reverb",
                t.effects.reverb,
                cx,
            ))
            .child(self.mix_send_control(
                &t.id,
                EffectParam::SendDelay,
                "Echo",
                t.effects.send_delay,
                cx,
            ))
            .child(self.mix_level_control(
                MixLevel::Track(t.id.clone()),
                t.volume,
                2.,
                peak,
                clipped,
                cx,
            ))
            .child(div().flex_1())
            .child(
                cell_btn(format!("mix-fx-{}", t.id), "Effects…", false, PANEL_ALT).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.st.select_track(&fx);
                        this.st.selected_return = None;
                        this.st.fx_window_open = true;
                        cx.notify();
                    }),
                ),
            )
    }

    fn mix_return_strip(&self, index: usize, cx: &mut Context<Self>) -> gpui::Div {
        let (label, gain, muted) = if index == 0 {
            (
                "Reverb",
                self.st.project.returns.reverb.gain,
                self.st.project.returns.reverb.muted,
            )
        } else {
            (
                "Echo",
                self.st.project.returns.delay.gain,
                self.st.project.returns.delay.muted,
            )
        };
        let (peak, clipped) = self.host.return_meters[index].read();
        channel_surface(self.accent(), self.st.selected_return == Some(index))
            .child(
                div()
                    .h(px(38.))
                    .flex_shrink_0()
                    .text_color(rgb(self.accent()))
                    .child(label),
            )
            .child(
                div()
                    .h(px(18.))
                    .text_size(px(11.))
                    .text_color(rgb(TEXT_DIM))
                    .child("Shared return"),
            )
            .child(
                cell_btn(format!("mix-return-mute-{index}"), "Mute", muted, RECORD).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.toggle_return_mute(index);
                        cx.notify();
                    }),
                ),
            )
            .child(div().h(px(96.)).flex_shrink_0())
            .child(self.mix_level_control(MixLevel::Return(index), gain, 1.25, peak, clipped, cx))
            .child(div().flex_1())
            .child(
                cell_btn(
                    format!("mix-return-fx-{index}"),
                    "Effects…",
                    false,
                    PANEL_ALT,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.st.selected_return = Some(index);
                    this.st.fx_window_open = true;
                    cx.notify();
                })),
            )
    }

    fn mix_master_strip(&self) -> gpui::Div {
        let (peak, clipped) = self.host.master_meter.read();
        channel_surface(TEXT_DIM, false)
            .child(div().h(px(38.)).text_color(rgb(TEXT)).child("Master"))
            .child(
                div()
                    .h(px(18.))
                    .text_size(px(11.))
                    .text_color(rgb(TEXT_DIM))
                    .child("Output meter"),
            )
            .child(div().h(px(96.)).flex_shrink_0())
            .child(
                div()
                    .flex()
                    .justify_center()
                    .child(level_meter(peak, clipped, LEVEL_H + 70.)),
            )
            .child(
                div()
                    .text_color(rgb(if clipped { RECORD } else { TEXT }))
                    .child(db_label(peak)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(TEXT_DIM))
                    .child("Post limiter"),
            )
    }

    pub(super) fn mixing_workspace(
        &self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let pinned_returns = width >= 1100.;
        let peaks: Vec<_> = self
            .host
            .strip_meters
            .lock()
            .map(|v| v.iter().map(|m| m.read()).collect())
            .unwrap_or_default();
        let count = self.st.project.tracks.len() + if pinned_returns { 0 } else { 2 };
        let mut channels = div()
            .w(px(count as f32 * (STRIP_W + 8.) + 16.))
            .h(px(STRIP_H))
            .flex()
            .gap_2()
            .px_2()
            .children(self.st.project.tracks.iter().enumerate().map(|(i, _)| {
                let (peak, clipped) = peaks.get(i).copied().unwrap_or((0., false));
                self.mix_track_strip(i, peak, clipped, cx)
            }));
        if !pinned_returns {
            channels = channels
                .child(self.mix_return_strip(0, cx))
                .child(self.mix_return_strip(1, cx));
        }
        let outputs = div()
            .flex()
            .gap_2()
            .flex_shrink_0()
            .children(pinned_returns.then(|| self.mix_return_strip(0, cx)))
            .children(pinned_returns.then(|| self.mix_return_strip(1, cx)))
            .child(self.mix_master_strip());
        div()
            .id("mixing-workspace")
            .debug_selector(|| "mixing-workspace".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .flex_shrink_0()
                    .child(
                        div()
                            .text_color(rgb(TEXT))
                            .child(format!("Mixing · {} tracks", self.st.project.tracks.len())),
                    )
                    .child(
                        cell_btn(
                            "mix-balance",
                            if self.balance_running {
                                "Balancing…"
                            } else {
                                "Auto balance"
                            },
                            false,
                            PANEL_ALT,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start_mix_balance();
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(TEXT_DIM))
                            .child("Shift: fine adjustment · Scroll sideways for more channels"),
                    ),
            )
            .child(
                div()
                    .id("mix-console-scroll")
                    .track_scroll(&self.mix_vertical_scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .h(px(STRIP_H + 16.))
                            .w_full()
                            .flex()
                            .gap_2()
                            .py_2()
                            .pr_2()
                            .child(
                                div()
                                    .id("mix-channel-scroll")
                                    .track_scroll(&self.mix_scroll)
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(STRIP_H))
                                    .overflow_x_scroll()
                                    .child(channels),
                            )
                            .child(outputs),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn mixing_tab_controls_only_the_target_and_keeps_transport_and_patterns(
        cx: &mut gpui::TestAppContext,
    ) {
        let (host, commands, _audio) = Host::headless();
        let project = Project::default_project();
        let first = project.tracks[0].id.clone();
        let second = project.tracks[1].id.clone();
        let patterns = serde_json::to_value(&project.patterns).unwrap();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut v = DawView::new(host, project, None, String::new(), cx);
            v.sf2_missing = false;
            v.playing = true;
            v
        });
        cx.simulate_resize(size(px(1280.), px(800.)));
        cx.run_until_parked();
        let button = cx
            .debug_bounds("dock-tab-Mixing")
            .expect("Mixing tab is visible");
        cx.simulate_click(button.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("mixing-workspace").is_some());
        let before = view.read_with(cx, |v, _| v.st.project.tracks[1].volume);
        let button = cx.debug_bounds("mix-level-up-t2").unwrap();
        cx.simulate_click(button.center(), gpui::Modifiers::default());
        view.read_with(cx, |v, _| {
            assert!((v.st.project.tracks[1].volume - before - 0.05).abs() < 0.0001);
            assert_eq!(v.st.selected_track.as_deref(), Some(first.as_str()));
            assert!(v.playing);
            assert_eq!(
                serde_json::to_value(&v.st.project.patterns).unwrap(),
                patterns
            );
        });
        assert!(commands.try_iter().any(
            |c| matches!(c, SequencerCommand::SetTrackVolume { track_id, .. } if track_id == second)
        ));
        // One click is one undoable edit.
        view.update(cx, |v, cx| {
            v.undo();
            cx.notify();
        });
        view.read_with(cx, |v, _| assert_eq!(v.st.project.tracks[1].volume, before));
        cx.run_until_parked();
        let button = cx.debug_bounds("mix-send-up-Reverb-t2").unwrap();
        let send = view.read_with(cx, |v, _| v.st.project.tracks[1].effects.reverb);
        cx.simulate_click(button.center(), gpui::Modifiers::default());
        view.read_with(cx, |v, _| {
            assert!((v.st.project.tracks[1].effects.reverb - send - 0.025).abs() < 0.0001);
            assert_eq!(v.st.selected_track.as_deref(), Some(first.as_str()));
        });
        cx.run_until_parked();
        let button = cx.debug_bounds("mix-level-up-return-1").unwrap();
        let gain = view.read_with(cx, |v, _| v.st.project.returns.delay.gain);
        cx.simulate_click(button.center(), gpui::Modifiers::default());
        view.read_with(cx, |v, _| {
            assert!((v.st.project.returns.delay.gain - gain - 0.05).abs() < 0.0001);
            assert_eq!(v.st.active_dock, DockTab::Mixing);
            assert!(!v.st.fx_window_open);
        });
        cx.run_until_parked();
        let button = cx.debug_bounds("mix-fx-t2").unwrap();
        cx.simulate_click(button.center(), gpui::Modifiers::default());
        view.read_with(cx, |v, _| {
            assert!(v.st.fx_window_open);
            assert_eq!(v.st.active_dock, DockTab::Mixing);
            assert_eq!(v.st.selected_track.as_deref(), Some(second.as_str()));
            assert!(v.st.selected_return.is_none());
        });
        view.update(cx, |v, cx| {
            v.st.fx_window_open = false;
            cx.notify();
        });
        cx.run_until_parked();
        let button = cx.debug_bounds("dock-tab-Piano Roll").unwrap();
        cx.simulate_click(button.center(), gpui::Modifiers::default());
        view.read_with(cx, |v, _| {
            assert_eq!(v.st.active_dock, DockTab::PianoRoll);
            assert!(v.playing);
            assert_eq!(
                serde_json::to_value(&v.st.project.patterns).unwrap(),
                patterns
            );
        });
    }

    #[gpui::test]
    fn mixing_scroll_reaches_last_track_and_returns_after_resize(cx: &mut gpui::TestAppContext) {
        let (host, _commands, _audio) = Host::headless();
        let mut project = Project::default_project();
        let original = project.tracks[0].clone();
        for i in 0..20 {
            let mut t = original.clone();
            t.id = format!("mix-extra-{i}");
            t.name = format!("A long named channel {i}");
            t.patterns.clear();
            project.tracks.push(t);
        }
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut v = DawView::new(host, project, None, String::new(), cx);
            v.sf2_missing = false;
            v.st.active_dock = DockTab::Mixing;
            v.st.browser_open = true;
            v
        });
        for (width, height) in [(800., 480.), (1280., 800.), (1920., 1080.), (800., 480.)] {
            cx.simulate_resize(size(px(width), px(height)));
            view.update(cx, |v, cx| {
                v.mix_scroll.set_offset(gpui::point(px(-10000.), px(0.)));
                v.mix_vertical_scroll
                    .set_offset(gpui::point(px(0.), px(-10000.)));
                cx.notify();
            });
            cx.run_until_parked();
            let button = cx
                .debug_bounds("mix-level-up-mix-extra-19")
                .expect("last channel paints after scrolling");
            assert!(button.center().x > px(0.) && button.center().x < px(width));
            assert!(button.center().y > px(0.) && button.center().y < px(height));
            let before = view.read_with(cx, |v, _| v.st.project.tracks.last().unwrap().volume);
            cx.simulate_click(button.center(), gpui::Modifiers::default());
            view.read_with(cx, |v, _| {
                assert!(v.st.project.tracks.last().unwrap().volume > before)
            });
            view.update(cx, |v, cx| {
                v.mix_vertical_scroll
                    .set_offset(gpui::point(px(0.), px(0.)));
                cx.notify();
            });
            cx.run_until_parked();
            let button = cx
                .debug_bounds("mix-return-mute-1")
                .expect("echo return reachable");
            assert!(button.center().x < px(width));
            assert!(button.center().y > px(0.) && button.center().y < px(height));
            let before = view.read_with(cx, |v, _| v.st.project.returns.delay.muted);
            cx.simulate_click(button.center(), gpui::Modifiers::default());
            view.read_with(cx, |v, _| {
                assert_ne!(v.st.project.returns.delay.muted, before);
                assert_eq!(v.st.active_dock, DockTab::Mixing);
                assert!(!v.st.fx_window_open);
            });
        }
    }
}

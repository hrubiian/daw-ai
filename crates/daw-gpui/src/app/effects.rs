use super::*;
use daw_core::project::{EchoDivision, EchoMode, ReturnParam, SpatialPreset};

impl DawView {
    pub(super) fn project_groove(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let accent = self.accent();
        let project = &self.st.project.project;

        let stepper = |label, down_id, up_id, value, adjust: fn(&mut Self, f32), cx: &mut Context<Self>| {
            header_group()
                .child(div().child(label))
                .child(theme.cell_btn(down_id, "−", false, theme.panel_alt)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        adjust(this, -0.05);
                        cx.notify();
                    })))
                .child(div().min_w(px(32.)).text_center().child(format!("{:.0}%", value * 100.0)))
                .child(theme.cell_btn(up_id, "+", false, theme.panel_alt)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        adjust(this, 0.05);
                        cx.notify();
                    })))
        };

        div()
            .debug_selector(|| "fx-project-groove".into())
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_1()
            .p_2()
            .bg(rgb(theme.panel))
            .text_color(rgb(theme.text))
            .text_size(px(12.))
            .line_height(px(16.))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(div().text_color(rgb(theme.text_dim)).child("Project groove"))
            .child(div().flex().flex_wrap().items_center().gap_2()
                .child(stepper("Swing", "sw-down", "sw-up", project.swing, Self::adjust_swing, cx))
                .child(theme.cell_btn("humanize", format!("Humanize {:.0}%", project.humanize * 100.0),
                    project.humanize > 0.0, accent)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cycle_humanize();
                        cx.notify();
                    })))
                .child(stepper("Timing", "tm-down", "tm-up", project.timing, Self::adjust_timing, cx)))
    }

    fn effects_navigation(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        div().flex().flex_wrap().gap_1().flex_shrink_0().children(
            [(None, "Track"), (Some(0), "Reverb"), (Some(1), "Echo")]
                .into_iter()
                .map(|(target, label)| {
                    theme.cell_btn(
                        format!("fx-scope-{label}"),
                        label,
                        self.st.selected_return == target,
                        self.accent(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.st.selected_return = target;
                        this.fx_library.open = false;
                        cx.notify();
                    }))
                }),
        )
    }
    fn spatial_presets(&self, reverb: bool, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let presets = if reverb {
            SpatialPreset::REVERB
        } else {
            SpatialPreset::ECHO
        };
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(presets.into_iter().map(|preset| {
                theme.cell_btn(
                    format!("space-preset-{}", preset.label()),
                    preset.label(),
                    false,
                    theme.panel_alt,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.undo.record("space preset", &this.st.project);
                    this.st.project.apply_spatial_preset(preset);
                    this.push_returns();
                    cx.notify();
                }))
            }))
    }
    fn echo_sync_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let sync = self.st.project.returns.delay.sync;
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(
                        theme.cell_btn("echo-free", "Free ms", sync.is_none(), self.accent()).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.undo.record("echo free time", &this.st.project);
                                this.st.project.returns.delay.time_ms = this
                                    .st
                                    .project
                                    .returns
                                    .delay
                                    .effective_time_ms(this.st.project.project.bpm);
                                this.st.project.returns.delay.sync = None;
                                this.push_returns();
                                cx.notify();
                            }),
                        ),
                    )
                    .children(EchoDivision::ALL.into_iter().map(|division| {
                        theme.cell_btn(
                            format!("echo-sync-{}", division.label()),
                            division.label(),
                            sync == Some(division),
                            self.accent(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.undo.record("echo tempo sync", &this.st.project);
                            this.st.project.returns.delay.sync = Some(division);
                            this.push_returns();
                            cx.notify();
                        }))
                    })),
            )
            .child(
                div().flex().gap_1().children(
                    [
                        (EchoMode::PingPong, "Ping-pong"),
                        (EchoMode::Stereo, "Stereo"),
                    ]
                    .into_iter()
                    .map(|(mode, label)| {
                        theme.cell_btn(
                            format!("echo-mode-{label}"),
                            label,
                            self.st.project.returns.delay.mode == mode,
                            self.accent(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.undo.record("echo mode", &this.st.project);
                            this.st.project.returns.delay.mode = mode;
                            this.push_returns();
                            cx.notify();
                        }))
                    }),
                ),
            )
    }
    pub(super) fn start_clap_scan(&mut self) {
        if self.clap_scanning.swap(true, Ordering::AcqRel) {
            return;
        }
        self.st.set_status("Scanning installed plugins…");
        let result = self.clap_scan.clone();
        let scanning = self.clap_scanning.clone();
        std::thread::spawn(move || {
            let found = daw_core::audio::clap::discover();
            if let Ok(mut slot) = result.lock() {
                *slot = Some(found);
            }
            scanning.store(false, Ordering::Release);
        });
    }
    /// Bus-parameter editor for the selected return (FX dock).
    pub(super) fn return_editor(&self, ri: usize, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let (title, params): (&str, &[ReturnParam]) = if ri == 0 {
            ("Reverb Return", &ReturnParam::REVERB)
        } else {
            ("Delay Return", &ReturnParam::DELAY)
        };
        let muted = if ri == 0 {
            self.st.project.returns.reverb.muted
        } else {
            self.st.project.returns.delay.muted
        };
        let rows: Vec<_> = params
            .iter()
            .copied()
            .map(|p| {
                let val = if p == ReturnParam::DlyTimeMs {
                    self.st
                        .project
                        .returns
                        .delay
                        .effective_time_ms(self.st.project.project.bpm) as f32
                } else {
                    p.value(&self.st.project.returns)
                };
                let d: f32 = match p {
                    ReturnParam::DlyTimeMs => 10.,
                    ReturnParam::RevPredelay => 5.,
                    ReturnParam::RevLowCut | ReturnParam::DlyLowCut => 50.,
                    ReturnParam::RevHighCut | ReturnParam::DlyHighCut => 500.,
                    ReturnParam::DlyModDepth => 0.5,
                    _ => 0.05,
                };
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(26.))
                    .flex_shrink_0()
                    .child(
                        div()
                            .w(px(104.))
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .text_color(rgb(theme.text_dim))
                            .child(p.label()),
                    )
                    .child(
                        theme.cell_btn(format!("rt-{ri}-{}", p.label()), "−", false, theme.panel_alt).on_click(
                            cx.listener(move |this, e: &gpui::ClickEvent, _w, cx| {
                                let step = if e.modifiers().shift {
                                    if matches!(
                                        p,
                                        ReturnParam::DlyTimeMs | ReturnParam::RevPredelay
                                    ) {
                                        (d / 10.).max(1.)
                                    } else {
                                        d / 10.
                                    }
                                } else {
                                    d
                                };
                                this.nudge_return(p, -step);
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        div()
                            .w(px(64.))
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .text_color(rgb(theme.text))
                            .child(format!("{val:.2}")),
                    )
                    .child(
                        theme.cell_btn(format!("rt+{ri}-{}", p.label()), "+", false, theme.panel_alt).on_click(
                            cx.listener(move |this, e: &gpui::ClickEvent, _w, cx| {
                                let step = if e.modifiers().shift {
                                    if matches!(
                                        p,
                                        ReturnParam::DlyTimeMs | ReturnParam::RevPredelay
                                    ) {
                                        (d / 10.).max(1.)
                                    } else {
                                        d / 10.
                                    }
                                } else {
                                    d
                                };
                                this.nudge_return(p, step);
                                cx.notify();
                            }),
                        ),
                    )
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .overflow_hidden()
            .child(self.effects_navigation(cx))
            .child(div().text_color(rgb(theme.text)).child(title))
            .child(
                div().text_color(rgb(theme.text_dim)).child(
                    "Shared space: shapes all sending tracks. Shift+click for fine control.",
                ),
            )
            .child(self.spatial_presets(ri == 0, cx))
            .children((ri == 1).then(|| self.echo_sync_controls(cx)))
            .children(rows)
            .child(
                theme.cell_btn(format!("rtm{ri}"), "Mute", muted, RECORD).on_click(cx.listener(
                    move |this, _e, _w, cx| {
                        this.toggle_return_mute(ri);
                        cx.notify();
                    },
                )),
            )
    }

    /// Effect-chain editor for the selected track (type-appropriate params).
    pub(super) fn fx_editor(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        use daw_core::project::EffectParam;
        if let Some(ri) = self.st.selected_return {
            return self.return_editor(ri, cx);
        }
        let Some(tid) = self.st.selected_track.clone() else {
            return div()
                .flex()
                .flex_col()
                .child(self.effects_navigation(cx))
                .child(theme.dock_hint("Select a track to edit its effects."));
        };
        let Some(track) = self.st.project.tracks.iter().find(|t| t.id == tid) else {
            return theme.dock_hint("Select a track to edit its effects.");
        };
        use EffectParam::*;
        let mut groups: Vec<(&str, &[EffectParam])> = vec![
            ("EQ & Filter", &[Cutoff, EqLowGain, EqMidGain, EqHighGain, EqMidFreq]),
            ("Dynamics & Drive", &[CompThreshold, CompRatio, CompAttackMs, CompReleaseMs, Drive]),
            ("Delay & Sends", &[DelayTimeMs, DelayFeedback, DelayMix, Reverb, SendDelay]),
        ];
        if matches!(track.instrument, daw_core::project::Instrument::Soundfont { .. }) {
            groups.push(("Modulation", &[Chorus, Resonance, LfoDepth]));
        } else if !track.modulators.is_empty() {
            groups.push(("Modulation", &[]));
        }
        let step = |p: EffectParam| -> f32 {
            match p {
                EffectParam::DelayTimeMs => 10.0,
                EffectParam::EqMidFreq => 100.0,
                EffectParam::EqLowGain
                | EffectParam::EqMidGain
                | EffectParam::EqHighGain
                | EffectParam::CompThreshold
                | EffectParam::CompRatio
                | EffectParam::CompAttackMs
                | EffectParam::CompReleaseMs => 1.0,
                _ => 0.05,
            }
        };

        let controls = groups.into_iter().map(|(group, params)| {
            let open = self.fx_controls_group == Some(group);
            let mut panel = div().flex().flex_col().gap_1().child(
                theme.cell_btn(format!("fx-controls-{group}"),
                    format!("{}  {group}", if open { "▾" } else { "▸" }), open, self.accent())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.fx_controls_group = (!open).then_some(group);
                        cx.notify();
                    })),
            );
            if open {
                panel = panel.children(params.iter().copied().map(|p| {
                    let val = p.value(&track.effects);
                    let (tdec, tinc) = (tid.clone(), tid.clone());
                    let d = step(p);
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(26.))
                        .flex_shrink_0()
                        .child(
                            div()
                                .w(px(104.))
                                .flex_shrink_0()
                                .whitespace_nowrap()
                                .text_color(rgb(theme.text_dim))
                                .child(p.label()),
                        )
                        .child(
                            theme.cell_btn(format!("fx-{}", p.label()), "−", false, theme.panel_alt).on_click(
                                cx.listener(move |this, e: &gpui::ClickEvent, _w, cx| {
                                    this.undo.record("effects", &this.st.project);
                                    this.st.project.nudge_effect(
                                        &tdec,
                                        p,
                                        if e.modifiers().shift { -d / 10. } else { -d },
                                    );
                                    this.push_effects(&tdec);
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            div()
                                .w(px(64.))
                                .flex_shrink_0()
                                .whitespace_nowrap()
                                .text_color(rgb(theme.text))
                                .child(format!("{val:.2}")),
                        )
                        .child(
                            theme.cell_btn(format!("fx+{}", p.label()), "+", false, theme.panel_alt).on_click(
                                cx.listener(move |this, e: &gpui::ClickEvent, _w, cx| {
                                    this.undo.record("effects", &this.st.project);
                                    this.st.project.nudge_effect(
                                        &tinc,
                                        p,
                                        if e.modifiers().shift { d / 10. } else { d },
                                    );
                                    this.push_effects(&tinc);
                                    cx.notify();
                                }),
                            ),
                        )
                }));
                if group == "Modulation" {
                    panel = panel.children(track.modulators.iter().map(|modulator| {
                        let base = daw_core::sequencer::engine::mod_base01(modulator.dest, track);
                        let value = daw_core::sequencer::engine::modulated_value(
                            modulator, base, self.st.playhead_seconds() as f32,
                        );
                        div().flex().items_center().justify_between().gap_2().px_2()
                            .text_color(rgb(theme.text_dim)).child(modulator.dest.label())
                            .child(format!("{:.0}%", value * 100.0))
                    }));
                }
            }
            panel
        }).collect::<Vec<_>>();

        let insert_header = div().flex().items_center().justify_between().gap_2().px_1().py_1()
            .child(div().text_color(rgb(theme.text)).child("Track inserts"))
            .child(theme.cell_btn("fx-show-library", "+ Add effect", true, self.accent())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.show_effect_library(window);
                    cx.notify();
                })));

        let mut panel = div().flex().flex_col().gap_2().p_2()
            .child(self.effects_navigation(cx))
            .child(insert_header)
            .children(track.clap_fx.is_empty().then(|| theme.dock_hint("No insert plugins")))
            .children(track.clap_fx.iter().map(|slot| self.clap_slot_section(slot, true, cx)))
            .child(theme.section_title("Built-in controls"))
            .children(controls);

        if let daw_core::project::Instrument::Clap(slot) = &track.instrument {
            panel = panel.child(theme.section_title("Instrument"))
                .child(self.clap_slot_section(slot, false, cx));
        }
        panel.children(self.sampler_controls(track, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn project_groove_moves_to_effects_and_updates_the_engine(cx: &mut gpui::TestAppContext) {
        let (host, commands, _audio) = Host::headless();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = DawView::new(host, Project::default_project(), None, String::new(), cx);
            view.sf2_missing = false;
            view
        });
        cx.simulate_resize(size(px(800.), px(480.)));
        cx.run_until_parked();
        for id in ["sw-up", "humanize", "tm-up"] {
            assert!(cx.debug_bounds(id).is_none());
        }

        let effects = cx.debug_bounds("menu-audio-effects").unwrap();
        cx.simulate_click(effects.center(), gpui::Modifiers::default());
        let workspace = cx.debug_bounds("fx-project-groove").unwrap();
        for id in ["sw-up", "humanize", "tm-up"] {
            let button = cx.debug_bounds(id).unwrap().center();
            assert!(workspace.contains(&button));
            cx.simulate_click(button, gpui::Modifiers::default());
        }
        view.read_with(cx, |view, _| {
            let project = &view.st.project.project;
            assert_eq!((project.swing, project.humanize, project.timing), (0.05, 0.25, 0.05));
        });
        let commands: Vec<_> = commands.try_iter().collect();
        assert!(commands.iter().any(|cmd| matches!(cmd, SequencerCommand::SetSwing(v) if *v == 0.05)));
        assert!(commands.iter().any(|cmd| matches!(cmd, SequencerCommand::UpdateLive(p) if p.project.humanize == 0.25)));
        assert!(commands.iter().any(|cmd| matches!(cmd, SequencerCommand::SetTiming(v) if *v == 0.05)));

        cx.simulate_click(effects.center(), gpui::Modifiers::default());
        let fx_tab = cx.debug_bounds("dock-tab-FX").unwrap();
        cx.simulate_click(fx_tab.center(), gpui::Modifiers::default());
        let swing_down = cx.debug_bounds("sw-down").unwrap();
        assert!(swing_down.top() >= fx_tab.bottom());
        cx.simulate_click(swing_down.center(), gpui::Modifiers::default());
        view.read_with(cx, |view, _| assert_eq!(view.st.project.project.swing, 0.));
    }

    #[gpui::test]
    fn audio_track_can_insert_a_flow_effect(cx: &mut gpui::TestAppContext) {
        let (host, _commands, _audio) = Host::headless();
        host.sample_rate.store(48_000, Ordering::Relaxed);
        let stereo: Vec<_> = (0..16_384).map(|i| {
            let t = i as f32 * std::f32::consts::TAU / 48_000.0;
            0.12 * ((t * 261.626).sin() + (t * 329.628).sin() + (t * 391.995).sin())
        }).collect();
        host.vis_buf.lock().unwrap().push(&stereo, Some(&stereo));
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut project = Project::default_project();
            let track = project.add_track(daw_core::project::TrackType::Audio);
            let mut view = DawView::new(host, project, None, String::new(), cx);
            view.sf2_missing = false;
            view.st.fx_window_open = true;
            view.st.selected_track = Some(track);
            view.st
                .clap_plugins
                .push(daw_core::audio::clap::ClapPluginDesc {
                    path: "/tmp/test-effect.fmx".into(),
                    id: "local.daw-ai.fmx.test-effect".into(),
                    name: "Test Vocal Strip".into(),
                    vendor: "Flow Music".into(),
                    features: Vec::new(),
                    is_instrument: false,
                    is_effect: true,
                });
            view
        });
        cx.simulate_resize(size(px(800.), px(700.)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("output-analyzer").is_some());
        assert!(cx.debug_bounds("output-harmony").is_some());
        assert!(cx.debug_bounds("fx-add-plugin-local.daw-ai.fmx.test-effect").is_none());
        let library = cx.debug_bounds("fx-show-library").unwrap();
        cx.simulate_click(library.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("fx-import-plugin").is_some());
        assert!(cx.debug_bounds("fx-add-plugin-local.daw-ai.fmx.test-effect").is_none());
        let category = cx.debug_bounds("fx-category-Pitch & Vocals").unwrap();
        cx.simulate_click(category.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("fx-add-plugin-local.daw-ai.fmx.test-effect").is_some());
        let search = cx.debug_bounds("fx-library-search").unwrap();
        cx.simulate_click(search.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| assert!(view.read(cx).fx_library.focus.is_focused(window)));
        cx.simulate_keystrokes("n o m a t c h");
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert_eq!(view.fx_library.query, "nomatch"));
        assert!(cx.debug_bounds("fx-no-matches").is_some());
        let clear = cx.debug_bounds("fx-clear-search").unwrap();
        cx.simulate_click(clear.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.simulate_keystrokes("v o c a l");
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert_eq!(view.fx_library.query, "vocal"));
        let add = cx
            .debug_bounds("fx-add-plugin-local.daw-ai.fmx.test-effect")
            .unwrap();
        cx.simulate_click(add.center(), gpui::Modifiers::default());
        view.read_with(cx, |view, _| {
            let track = view.st.project.tracks.last().unwrap();
            assert_eq!(track.track_type, daw_core::project::TrackType::Audio);
            assert_eq!(track.clap_fx[0].id, "local.daw-ai.fmx.test-effect");
            assert!(!view.fx_library.open);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("editor-c1").is_some());
    }

    #[gpui::test]
    fn spatial_controls_change_the_bus_without_clicking_tracks(cx: &mut gpui::TestAppContext) {
        let (host, commands, _audio) = Host::headless();
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut v = DawView::new(host, Project::default_project(), None, String::new(), cx);
            v.sf2_missing = false;
            v.st.fx_window_open = true;
            v.st.selected_return = Some(1);
            v
        });
        cx.simulate_resize(size(px(800.), px(700.)));
        cx.run_until_parked();
        let selected = view.read_with(cx, |v, _| v.st.selected_track.clone());
        let patterns = view.read_with(cx, |v, _| {
            serde_json::to_value(&v.st.project.patterns).unwrap()
        });
        let bounds = cx
            .debug_bounds("space-preset-Tape echo")
            .expect("echo preset visible");
        cx.simulate_click(bounds.center(), gpui::Modifiers::default());
        view.read_with(cx, |v, _| {
            assert_eq!(
                v.st.project.returns.delay.sync,
                Some(EchoDivision::EighthDotted)
            );
            assert_eq!(v.st.project.returns.delay.modulation_ms, 2.5);
            assert_eq!(v.st.selected_track, selected);
            assert_eq!(
                serde_json::to_value(&v.st.project.patterns).unwrap(),
                patterns
            );
        });
        assert!(commands
            .try_iter()
            .any(|c| matches!(c, SequencerCommand::SetReturns(_))));
        cx.run_until_parked();
        let bounds = cx
            .debug_bounds("echo-sync-1/4")
            .expect("sync choice visible");
        cx.simulate_click(bounds.center(), gpui::Modifiers::default());
        view.read_with(cx, |v, _| {
            assert_eq!(v.st.project.returns.delay.sync, Some(EchoDivision::Quarter))
        });
        cx.run_until_parked();
        let bounds = cx.debug_bounds("fx-scope-Track").unwrap();
        cx.simulate_click(bounds.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("fx+Drive").is_none());
        let dynamics = cx.debug_bounds("fx-controls-Dynamics & Drive").unwrap();
        cx.simulate_click(dynamics.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("fx+Drive").is_some(),
            "saturation exposed on a musical track"
        );
        assert!(
            cx.debug_bounds("fx+Cmp Rat").is_some(),
            "compressor exposed on a musical track"
        );
        let bounds = cx.debug_bounds("fx-scope-Reverb").unwrap();
        cx.simulate_click(bounds.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        let bounds = cx.debug_bounds("space-preset-Wide hall").unwrap();
        cx.simulate_click(bounds.center(), gpui::Modifiers::default());
        view.read_with(cx, |v, _| {
            assert_eq!(v.st.project.returns.reverb.predelay_ms, 35);
            assert_eq!(v.st.selected_track, selected);
        });
        for (w, h) in [(800., 480.), (1920., 1080.)] {
            cx.simulate_resize(size(px(w), px(h)));
            cx.run_until_parked();
            for ri in [0, 1] {
                view.update(cx, |v, cx| {
                    v.st.selected_return = Some(ri);
                    cx.notify();
                });
                cx.run_until_parked();
            }
        }
    }
}

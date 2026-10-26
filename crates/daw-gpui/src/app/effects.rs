use super::*;
use daw_core::project::{EchoDivision, EchoMode, ReturnParam, SpatialPreset};

impl DawView {
    fn effects_navigation(&self, cx: &mut Context<Self>) -> gpui::Div {
        div().flex().flex_wrap().gap_1().flex_shrink_0().children(
            [(None, "Track"), (Some(0), "Reverb"), (Some(1), "Echo")]
                .into_iter()
                .map(|(target, label)| {
                    cell_btn(
                        format!("fx-scope-{label}"),
                        label,
                        self.st.selected_return == target,
                        self.accent(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.st.selected_return = target;
                        cx.notify();
                    }))
                }),
        )
    }
    fn spatial_presets(&self, reverb: bool, cx: &mut Context<Self>) -> gpui::Div {
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
                cell_btn(
                    format!("space-preset-{}", preset.label()),
                    preset.label(),
                    false,
                    PANEL_ALT,
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
                        cell_btn("echo-free", "Free ms", sync.is_none(), self.accent()).on_click(
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
                        cell_btn(
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
                        cell_btn(
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
        self.st.set_status("Scanning installed CLAP plugins…");
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
    fn effects_plugin_picker(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut panel = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(section_title("CLAP insert effects"))
            .child(
                cell_btn(
                    "fx-rescan-clap",
                    if self.clap_scanning.load(Ordering::Acquire) {
                        "Scanning…"
                    } else {
                        "Rescan CLAP effects"
                    },
                    false,
                    PANEL_ALT,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.start_clap_scan();
                    cx.notify();
                })),
            );
        let effects: Vec<_> = self
            .st
            .clap_plugins
            .iter()
            .filter(|p| p.is_effect)
            .collect();
        if effects.is_empty() {
            panel = panel.child(
                div()
                    .text_color(rgb(TEXT_DIM))
                    .child("Install a Linux CLAP effect, then rescan."),
            );
        }
        for desc in effects {
            let path = desc.path.clone();
            let id = desc.id.clone();
            panel = panel.child(
                cell_btn(
                    format!("fx-add-plugin-{}", desc.id),
                    format!("+ {}", desc.name),
                    false,
                    PANEL_ALT,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.add_clap_fx(path.clone(), id.clone());
                    cx.notify();
                })),
            );
        }
        panel
    }
    /// Bus-parameter editor for the selected return (FX dock).
    pub(super) fn return_editor(&self, ri: usize, cx: &mut Context<Self>) -> gpui::Div {
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
                            .text_color(rgb(TEXT_DIM))
                            .child(p.label()),
                    )
                    .child(
                        cell_btn(format!("rt-{ri}-{}", p.label()), "−", false, PANEL_ALT).on_click(
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
                            .text_color(rgb(TEXT))
                            .child(format!("{val:.2}")),
                    )
                    .child(
                        cell_btn(format!("rt+{ri}-{}", p.label()), "+", false, PANEL_ALT).on_click(
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
            .child(div().text_color(rgb(TEXT)).child(title))
            .child(
                div().text_color(rgb(TEXT_DIM)).child(
                    "Shared space: shapes all sending tracks. Shift+click for fine control.",
                ),
            )
            .child(self.spatial_presets(ri == 0, cx))
            .children((ri == 1).then(|| self.echo_sync_controls(cx)))
            .children(rows)
            .child(
                cell_btn(format!("rtm{ri}"), "Mute", muted, RECORD).on_click(cx.listener(
                    move |this, _e, _w, cx| {
                        this.toggle_return_mute(ri);
                        cx.notify();
                    },
                )),
            )
    }

    /// Effect-chain editor for the selected track (type-appropriate params).
    pub(super) fn fx_editor(&self, cx: &mut Context<Self>) -> gpui::Div {
        use daw_core::project::EffectParam;
        if let Some(ri) = self.st.selected_return {
            return self.return_editor(ri, cx);
        }
        let Some(tid) = self.st.selected_track.clone() else {
            return div()
                .flex()
                .flex_col()
                .child(self.effects_navigation(cx))
                .child(dock_hint("Select a track to edit its effects."));
        };
        let Some(track) = self.st.project.tracks.iter().find(|t| t.id == tid) else {
            return dock_hint("Select a track to edit its effects.");
        };
        // DSP inserts run on every strip; expose them for samplers and synths.
        let mut params = EffectParam::AUDIO.to_vec();
        if matches!(
            track.instrument,
            daw_core::project::Instrument::Soundfont { .. }
        ) {
            params.extend([
                EffectParam::Chorus,
                EffectParam::Resonance,
                EffectParam::LfoDepth,
            ]);
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

        let rows: Vec<_> = params
            .iter()
            .copied()
            .map(|p| {
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
                            .text_color(rgb(TEXT_DIM))
                            .child(p.label()),
                    )
                    .child(
                        cell_btn(format!("fx-{}", p.label()), "−", false, PANEL_ALT).on_click(
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
                            .text_color(rgb(TEXT))
                            .child(format!("{val:.2}")),
                    )
                    .child(
                        cell_btn(format!("fx+{}", p.label()), "+", false, PANEL_ALT).on_click(
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
            })
            .collect();

        // CLAP sections: the instrument (if any) above the insert chain.
        let mut clap_sections: Vec<gpui::Div> = Vec::new();
        if let daw_core::project::Instrument::Clap(slot) = &track.instrument {
            clap_sections.push(self.clap_slot_section(&slot.clone(), false, cx));
        }
        for slot in track.clap_fx.clone() {
            clap_sections.push(self.clap_slot_section(&slot, true, cx));
        }

        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .child(self.effects_navigation(cx))
            .child(section_title("Track inserts & sends"))
            .child(
                div().text_color(rgb(TEXT_DIM)).child(
                    "Shift+click for fine control. Open Reverb / Echo to shape shared spaces.",
                ),
            )
            .children(rows)
            .children(clap_sections)
            .child(self.effects_plugin_picker(cx))
            .children(self.sampler_controls(track, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

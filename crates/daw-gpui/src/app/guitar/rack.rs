//! Preset library and dual-IR cabinet construction for the live guitar rack.
use super::*;
use daw_io::{
    cabinet::CabinetRecipe,
    guitar_presets::{factory_presets, GuitarPreset},
};

impl DawView {
    pub(super) fn set_cabinet_mode(&mut self, mode: Cabinet) {
        let mut candidate = self.config.guitar.clone();
        candidate.amp.cabinet = mode;
        if mode == Cabinet::Ir && candidate.ir_path.is_none() {
            candidate.ir_path = Some(daw_io::cabinet::BUNDLED_IRS[0].id.into());
        }
        self.apply_guitar_config(candidate, "cabinet mode updated".into());
    }

    fn apply_guitar_preset(&mut self, preset: &GuitarPreset) {
        let mut candidate = self.config.guitar.clone();
        preset.apply(&mut candidate);
        self.apply_guitar_config(candidate, format!("guitar preset: {}", preset.name));
    }

    pub(super) fn save_guitar_preset(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.st.set_status("enter a preset name first");
            return;
        }
        if let Err(e) = self.save_ampforge_state() { self.st.set_status(e.to_string()); return; }
        let preset = GuitarPreset::capture(name.chars().take(64).collect(), &self.config.guitar);
        if let Some(index) = self
            .config
            .guitar_presets
            .iter()
            .position(|p| p.name == preset.name)
        {
            self.config.guitar_presets[index] = preset;
        } else {
            self.config.guitar_presets.push(preset);
        }
        match self.config.save() {
            Ok(()) => self.st.set_status(format!("saved guitar preset: {name}")),
            Err(e) => self
                .st
                .set_status(format!("could not save guitar presets: {e}")),
        }
    }

    fn edit_cabinet_recipe(&mut self, edit: impl FnOnce(&mut CabinetRecipe)) {
        let mut candidate = self.config.guitar.clone();
        candidate
            .ir_path
            .get_or_insert_with(|| daw_io::cabinet::BUNDLED_IRS[0].id.into());
        candidate.amp.cabinet = Cabinet::Ir;
        edit(&mut candidate.cabinet);
        self.apply_guitar_config(candidate, "custom cabinet updated".into());
    }

    pub(super) fn guitar_library(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let accent = self.accent();
        let mut library = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(theme.section_title("Built-in tone presets"))
            .child(
                div()
                    .text_color(rgb(theme.text_dim))
                    .child("Choose a built-in tone, or save an AmpForge rig under My presets."),
            );
        for (preset, description) in factory_presets().into_iter().zip([
            "Crunch · forward mids · blended cabs",
            "Clean · warm · open-back",
            "High gain · tight lows · closed-back",
            "Tight boost · scooped rhythm · 90/10 dual cab",
        ]) {
            let selected = preset.matches(&self.config.guitar);
            library = library.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .py_2()
                    .child(
                        theme.cell_btn(
                            format!("guitar-preset-{}", preset.name),
                            preset.name.clone(),
                            selected,
                            accent,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.apply_guitar_preset(&preset);
                            cx.notify();
                        })),
                    )
                    .child(div().text_color(rgb(theme.text_dim)).child(description)),
            );
        }
        let current = factory_presets()
            .iter()
            .chain(self.config.guitar_presets.iter())
            .find(|p| p.matches(&self.config.guitar))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Custom tone".into());
        library = library
            .child(
                div()
                    .py_2()
                    .text_color(rgb(accent))
                    .child(format!("Current: {current}")),
            )
            .child(theme.section_title("My presets"));
        if self.config.guitar_presets.is_empty() {
            library = library.child(
                div()
                    .text_color(rgb(theme.text_dim))
                    .child("Name and save your rig below."),
            );
        }
        for (index, preset) in self.config.guitar_presets.iter().cloned().enumerate() {
            let selected = preset.matches(&self.config.guitar);
            library = library.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        theme.cell_btn(
                            format!("guitar-user-{index}"),
                            trunc(&preset.name, 22),
                            selected,
                            accent,
                        )
                        .flex_1()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.apply_guitar_preset(&preset);
                            cx.notify();
                        })),
                    )
                    .child(
                        theme.cell_btn(format!("guitar-delete-{index}"), "×", false, theme.panel_alt).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.config.guitar_presets.remove(index);
                                if let Err(e) = this.config.save() {
                                    this.st
                                        .set_status(format!("could not save guitar presets: {e}"));
                                }
                                cx.notify();
                            }),
                        ),
                    ),
            );
        }
        library
    }

    pub(super) fn cabinet_builder(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let g = &self.config.guitar;
        let accent = self.accent();
        let mut panel = div().flex().flex_col().gap_2().p_3().bg(rgb(theme.panel))
            .child(theme.section_title("Cabinet builder"))
            .child(div().flex().gap_2()
                .children([(Cabinet::BuiltIn, "Built-in"), (Cabinet::Ir, "Dual IR"), (Cabinet::Off, "Bypass")].into_iter().map(|(mode, label)| {
                    theme.cell_btn(format!("cab-mode-{mode:?}"), label, g.amp.cabinet == mode, accent)
                        .on_click(cx.listener(move |this, _, _, cx| { this.set_cabinet_mode(mode); cx.notify(); }))
                })))
            .child(div().text_color(rgb(theme.text_dim)).child("Blend two speaker/mic IRs into your own cabinet. Cuts and blend affect Dual IR mode."));
        for secondary in [false, true] {
            let slot = if secondary { "B" } else { "A" };
            let source = if secondary {
                g.cabinet.secondary_ir_path.as_deref()
            } else {
                g.ir_path.as_deref()
            };
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .bg(rgb(theme.bg))
                    .child(div().child(format!(
                            "IR {slot}: {}",
                            source
                                .map(cabinet_name)
                                .unwrap_or_else(|| "not selected".into())
                        )))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .children(daw_io::cabinet::BUNDLED_IRS.iter().map(|preset| {
                                theme.cell_btn(
                                    format!("cab-{slot}-{}", preset.id),
                                    preset.name,
                                    source == Some(preset.id),
                                    accent,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.select_cabinet_slot(preset.id, secondary);
                                        cx.notify();
                                    },
                                ))
                            }))
                            .child(
                                theme.cell_btn(format!("cab-load-{slot}"), "Load WAV…", false, theme.panel_alt)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.pick_cabinet_ir_for(secondary);
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }
        let stepper = |id: &'static str,
                       label: String,
                       value: f32,
                       get: fn(&mut CabinetRecipe) -> &mut f32,
                       min: f32,
                       max: f32,
                       step: f32,
                       cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().child(label))
                .child(
                    theme.cell_btn(format!("cab-{id}-down"), "−", false, theme.panel_alt).on_click(
                        cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                            let step = if e.modifiers().shift {
                                step / 5.0
                            } else {
                                step
                            };
                            this.edit_cabinet_recipe(|r| *get(r) = (value - step).clamp(min, max));
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    theme.cell_btn(format!("cab-{id}-up"), "+", false, theme.panel_alt).on_click(cx.listener(
                        move |this, e: &gpui::ClickEvent, _, cx| {
                            let step = if e.modifiers().shift {
                                step / 5.0
                            } else {
                                step
                            };
                            this.edit_cabinet_recipe(|r| *get(r) = (value + step).clamp(min, max));
                            cx.notify();
                        },
                    )),
                )
        };
        panel
            .child(stepper(
                "blend",
                format!(
                    "Blend: A {:.0}% / B {:.0}%",
                    (1.0 - g.cabinet.blend) * 100.0,
                    g.cabinet.blend * 100.0
                ),
                g.cabinet.blend,
                |r| &mut r.blend,
                0.0,
                1.0,
                0.05,
                cx,
            ))
            .child(
                theme.cell_btn(
                    "cab-invert-b",
                    if g.cabinet.invert_b {
                        "B phase: inverted"
                    } else {
                        "B phase: normal"
                    },
                    g.cabinet.invert_b,
                    accent,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.edit_cabinet_recipe(|r| r.invert_b = !r.invert_b);
                    cx.notify();
                })),
            )
            .child(stepper(
                "low-cut",
                format!(
                    "Low cut: {:.0} Hz{}",
                    g.cabinet.low_cut_hz,
                    if g.cabinet.low_cut_hz <= 20.0 {
                        " (off)"
                    } else {
                        ""
                    }
                ),
                g.cabinet.low_cut_hz,
                |r| &mut r.low_cut_hz,
                20.0,
                400.0,
                10.0,
                cx,
            ))
            .child(stepper(
                "high-cut",
                format!(
                    "High cut: {:.0} Hz{}",
                    g.cabinet.high_cut_hz,
                    if g.cabinet.high_cut_hz >= 20000.0 {
                        " (off)"
                    } else {
                        ""
                    }
                ),
                g.cabinet.high_cut_hz,
                |r| &mut r.high_cut_hz,
                1000.0,
                20000.0,
                250.0,
                cx,
            ))
            .child(
                theme.cell_btn("cab-export", "Export cabinet WAV…", false, theme.panel_alt).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.export_guitar_cabinet();
                        cx.notify();
                    }),
                ),
            )
    }

    fn export_guitar_cabinet(&mut self) {
        if self.config.guitar.amp.cabinet != Cabinet::Ir {
            self.st.set_status("select Dual IR to export a cabinet");
            return;
        }
        let ir = match daw_io::cabinet::build_ir(&self.config.guitar, 48000) {
            Ok(ir) => ir,
            Err(e) => {
                self.st.set_status(format!("cannot export cabinet: {e}"));
                return;
            }
        };
        let result = self.cabinet_export_result.clone();
        std::thread::spawn(move || {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Cabinet IR", &["wav"])
                .set_file_name("my-cabinet.wav")
                .save_file()
            else {
                return;
            };
            let message = match daw_io::cabinet::write_ir(&path, &ir, 48000) {
                Ok(()) => format!("cabinet exported: {}", path.display()),
                Err(e) => format!("cabinet export failed: {e}"),
            };
            if let Ok(mut slot) = result.lock() {
                *slot = Some(message);
            }
        });
    }

    pub(super) fn guitar_plugins(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let target = self
            .st
            .selected_track
            .as_deref()
            .and_then(|id| self.st.project.tracks.iter().find(|t| t.id == id));
        let mut panel = div().flex().flex_col().gap_2().p_3().bg(rgb(theme.panel))
            .child(theme.section_title("CLAP guitar plugins"))
            .child(div().text_color(rgb(theme.text_dim)).child("Track inserts process recorded playback. Live monitoring uses the built-in rack above."))
            .child(div().child(format!("Insert target: {}", target.map(|t| t.name.as_str()).unwrap_or("select a track in the main window"))))
            .child(theme.cell_btn("guitar-scan-clap", if self.clap_scanning.load(Ordering::Acquire) { "Scanning…" } else { "Rescan installed CLAP effects" }, false, theme.panel_alt)
                .on_click(cx.listener(|this, _, _, cx| { this.start_clap_scan(); cx.notify(); })));
        let mut installed = div()
            .id("guitar-installed-plugins")
            .max_h(px(240.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1();
        for plugin in self.st.clap_plugins.iter().filter(|p| p.is_effect) {
            let path = plugin.path.clone();
            let id = plugin.id.clone();
            installed = installed.child(
                theme.cell_btn(
                    format!("guitar-clap-{}", plugin.id),
                    format!("+ {} · {}", plugin.name, plugin.vendor),
                    false,
                    theme.panel_alt,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.add_clap_fx(path.clone(), id.clone());
                    cx.notify();
                })),
            );
        }
        panel = panel.child(installed);
        if !self.st.clap_plugins.iter().any(|p| p.is_effect) {
            panel =
                panel.child(div().text_color(rgb(theme.text_dim)).child(
                    "No CLAP effects found. Install a Linux .clap in ~/.clap, then rescan.",
                ));
        }
        panel
            .child(
                div()
                    .text_color(rgb(theme.text_dim))
                    .child("Explore compatible formats (not yet tested in daw-ai):"),
            )
            .children(
                [
                    (
                        "BYOD · modular guitar distortion",
                        "https://github.com/Chowdhury-DSP/BYOD",
                    ),
                    (
                        "OrbitCab · amp captures and dual IR cabinet",
                        "https://github.com/darwinscat/orbitcab",
                    ),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (label, url))| {
                    theme.cell_btn(
                        format!("guitar-plugin-link-{index}"),
                        label,
                        false,
                        theme.panel_alt,
                    )
                    .on_click(move |_, _, cx| cx.open_url(url))
                }),
            )
    }
}

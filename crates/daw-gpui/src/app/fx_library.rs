use super::*;
use daw_core::audio::clap::ClapPluginDesc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Category {
    Dynamics,
    Eq,
    Space,
    Vocal,
    Modulation,
    Drive,
    Multi,
    Utility,
    Other,
}

impl Category {
    const ALL: [Self; 9] = [
        Self::Dynamics,
        Self::Eq,
        Self::Space,
        Self::Vocal,
        Self::Modulation,
        Self::Drive,
        Self::Multi,
        Self::Utility,
        Self::Other,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Dynamics => "Dynamics",
            Self::Eq => "EQ & Filters",
            Self::Space => "Reverb & Delay",
            Self::Vocal => "Pitch & Vocals",
            Self::Modulation => "Modulation",
            Self::Drive => "Drive & Amp",
            Self::Multi => "Multi-effects",
            Self::Utility => "Utility & Analysis",
            Self::Other => "Other effects",
        }
    }

    fn from_feature(feature: &str) -> Option<Self> {
        Some(match feature {
            "compressor" | "limiter" | "expander" | "gate" | "transient-shaper" => Self::Dynamics,
            "filter" | "equalizer" => Self::Eq,
            "reverb" | "delay" => Self::Space,
            "pitch-correction" | "pitch-shifter" | "de-esser" | "vocal" => Self::Vocal,
            "chorus" | "flanger" | "phaser" | "tremolo" | "frequency-shifter" => Self::Modulation,
            "distortion" | "saturation" | "amplifier" => Self::Drive,
            "multi-effects" => Self::Multi,
            "analyzer" | "utility" | "restoration" => Self::Utility,
            _ => return None,
        })
    }

    fn for_plugin(plugin: &ClapPluginDesc) -> Self {
        let name = plugin.name.to_lowercase();
        let has = |choices: &[&str]| choices.iter().any(|word| name.contains(word));
        if has(&["vocal", "pitch correction", "de-ess", "de ess", "deess"]) {
            return Self::Vocal;
        }
        // Prefer a specific functional tag. Many plugins also declare utility.
        if let Some(category) = plugin
            .features
            .iter()
            .filter_map(|f| Self::from_feature(f))
            .find(|category| *category != Self::Utility)
        {
            return category;
        }
        if has(&["reverb", "delay", "echo"]) {
            Self::Space
        } else if has(&["compress", "limiter", "expander", "gate", "transient"]) {
            Self::Dynamics
        } else if name
            .split(|ch: char| !ch.is_alphanumeric())
            .any(|word| word == "eq")
            || has(&["equaliz", "equalis", "filter", "crossover"])
        {
            Self::Eq
        } else if has(&["chorus", "flanger", "phaser", "tremolo", "modulation"]) {
            Self::Modulation
        } else if has(&[
            "ampforge",
            "amp sim",
            "saturat",
            "distort",
            "overdrive",
            "cabinet",
        ]) {
            Self::Drive
        } else if plugin
            .features
            .iter()
            .any(|f| Self::from_feature(f) == Some(Self::Utility))
            || has(&[
                "analyz",
                "analys",
                "meter",
                "gain",
                "stereo width",
                "phase invert",
            ])
        {
            Self::Utility
        } else {
            Self::Other
        }
    }
}

pub(super) struct EffectLibrary {
    pub(super) open: bool,
    category: Option<Category>,
    pub(super) query: String,
    pub(super) focus: FocusHandle,
    scroll: gpui::ScrollHandle,
}

impl EffectLibrary {
    pub(super) fn new(cx: &mut Context<DawView>) -> Self {
        Self {
            open: false,
            category: None,
            query: String::new(),
            focus: cx.focus_handle(),
            scroll: gpui::ScrollHandle::new(),
        }
    }

    fn reset_scroll(&self) {
        self.scroll.set_offset(gpui::point(px(0.), px(0.)));
    }

    pub(super) fn reveal(&mut self, name: &str) {
        self.query = name.to_owned();
        self.reset_scroll();
    }
}

impl DawView {
    pub(super) fn show_effect_library(&mut self, window: &mut Window) {
        self.fx_library.open = true;
        self.fx_library.category = None;
        self.fx_library.query.clear();
        self.fx_library.reset_scroll();
        self.fx_library.focus.focus(window);
    }

    pub(super) fn close_effect_library(&mut self, window: &mut Window) {
        self.fx_library.open = false;
        self.root_focus.focus(window);
    }

    pub(super) fn effects_panel(&self, cx: &mut Context<Self>, workspace: bool) -> gpui::Div {
        let library_open = self.fx_library.open
            && workspace == self.st.fx_window_open
            && self.st.selected_return.is_none()
            && self.st.selected_track.is_some();
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .child(if library_open {
                self.effect_library(cx)
            } else {
                div()
                    .id(if workspace {
                        "fx-chain-scroll"
                    } else {
                        "dock-fx-scroll"
                    })
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.project_groove(cx))
                    .child(self.fx_editor(cx))
                    .into_any_element()
            })
    }

    fn effect_library(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme();
        let plugins: Vec<_> = self
            .st
            .clap_plugins
            .iter()
            .filter(|plugin| plugin.is_effect)
            .map(|plugin| (plugin, Category::for_plugin(plugin)))
            .collect();
        let query = self.fx_library.query.to_lowercase();
        let filters: Vec<_> = query.split_whitespace().collect();
        let searching = !filters.is_empty();
        let mut body = div().flex().flex_col().gap_1();
        if plugins.is_empty() {
            body = body.child(theme.dock_hint(
                "Import a Flow Music effect (.fmx) or install a CLAP effect.",
            ));
        } else if searching {
            let matches: Vec<_> = plugins
                .into_iter()
                .filter(|(plugin, category)| {
                    let metadata = format!(
                        "{} {} {} {}",
                        plugin.name,
                        plugin.vendor,
                        category.label(),
                        plugin.features.join(" ")
                    )
                    .to_lowercase();
                    filters.iter().all(|word| metadata.contains(word))
                })
                .collect();
            if matches.is_empty() {
                body = body.child(
                    theme.dock_hint("No matching effects").debug_selector(|| "fx-no-matches".into()),
                );
            }
            body = body.children(
                matches
                    .into_iter()
                    .map(|(plugin, category)| self.effect_library_row(plugin, Some(category), cx)),
            );
        } else {
            for category in Category::ALL {
                let effects: Vec<_> = plugins
                    .iter()
                    .filter(|(_, group)| *group == category)
                    .map(|(plugin, _)| *plugin)
                    .collect();
                if effects.is_empty() {
                    continue;
                }
                let open = self.fx_library.category == Some(category);
                body = body.child(
                    theme.cell_btn(
                        format!("fx-category-{}", category.label()),
                        format!(
                            "{}  {}  ·  {}",
                            if open { "▾" } else { "▸" },
                            category.label(),
                            effects.len()
                        ),
                        open,
                        self.accent(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.fx_library.category = (!open).then_some(category);
                        cx.notify();
                    })),
                );
                if open {
                    body = body.children(
                        effects
                            .into_iter()
                            .map(|plugin| self.effect_library_row(plugin, None, cx)),
                    );
                }
            }
        }

        let field = div()
            .id("fx-library-search")
            .debug_selector(|| "fx-library-search".into())
            .key_context("EffectSearch")
            .track_focus(&self.fx_library.focus)
            .focus(|style| style.bg(rgb(theme.panel_alt)))
            .flex_1()
            .min_w_0()
            .px_2()
            .py_1()
            .truncate()
            .bg(rgb(theme.bg))
            .text_color(rgb(if self.fx_library.query.is_empty() {
                theme.text_dim
            } else {
                theme.text
            }))
            .child(if self.fx_library.query.is_empty() {
                "Search effects…".to_owned()
            } else {
                self.fx_library.query.clone()
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.fx_library.focus.focus(window);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                cx.stop_propagation();
                let key = &event.keystroke;
                if key.modifiers.control || key.modifiers.platform || key.modifiers.alt {
                    return;
                }
                match key.key.as_str() {
                    "space" => this.fx_library.query.push(' '),
                    "backspace" => {
                        this.fx_library.query.pop();
                    }
                    _ => {
                        if let Some(text) = &key.key_char {
                            this.fx_library.query.push_str(text);
                        } else if key.key.chars().count() == 1 {
                            this.fx_library.query.push_str(&key.key);
                        } else {
                            return;
                        }
                    }
                }
                this.fx_library.reset_scroll();
                cx.notify();
            }));

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .flex_wrap()
                    .gap_1()
                    .child(
                        theme.cell_btn("fx-library-back", "‹ Inserts", false, theme.panel_alt).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.close_effect_library(window);
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        theme.cell_btn(
                            "fx-import-plugin",
                            if self.plugin_import.is_some() {
                                "Importing…"
                            } else {
                                "Import plugin…"
                            },
                            false,
                            theme.panel_alt,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.import_plugin_dialog();
                            cx.notify();
                        })),
                    )
                    .child(
                        theme.cell_btn(
                            "fx-rescan-clap",
                            if self.clap_scanning.load(Ordering::Acquire) {
                                "Scanning…"
                            } else {
                                "Rescan"
                            },
                            false,
                            theme.panel_alt,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start_clap_scan();
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .overflow_hidden()
                    .child(field)
                    .children((!self.fx_library.query.is_empty()).then(|| {
                        theme.cell_btn("fx-clear-search", "Clear", false, theme.panel_alt).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.fx_library.query.clear();
                                this.fx_library.reset_scroll();
                                this.fx_library.focus.focus(window);
                                cx.notify();
                            }),
                        )
                    })),
            )
            .child(
                div()
                    .id("fx-library-results")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.fx_library.scroll)
                    .child(body),
            )
            .into_any_element()
    }

    fn effect_library_row(
        &self,
        plugin: &ClapPluginDesc,
        category: Option<Category>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = self.theme();
        let path = plugin.path.clone();
        let id = plugin.id.clone();
        let name = plugin.name.clone();
        let vendor = &plugin.vendor;
        let details = match category {
            Some(category) if !vendor.is_empty() => format!("{} · {vendor}", category.label()),
            Some(category) => category.label().into(),
            None => vendor.clone(),
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(theme.bg))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().child(name.clone()))
                    .children((!details.is_empty()).then(|| {
                        div()
                            .truncate()
                            .text_size(px(10.))
                            .text_color(rgb(theme.text_dim))
                            .child(details)
                    })),
            )
            .child(
                theme.cell_btn_with_help(
                    format!("fx-add-plugin-{id}"),
                    "Add",
                    false,
                    theme.panel_alt,
                    format!("Insert {name}").into(),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.add_clap_fx(path.clone(), id.clone()) {
                        this.st.set_status(format!("Added {name}"));
                        this.close_effect_library(window);
                    }
                    cx.notify();
                })),
            )
    }
}

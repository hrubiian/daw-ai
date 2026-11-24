use super::*;
use daw_core::audio::clap::ClapPluginDesc;
use std::sync::mpsc::{self, TryRecvError};

pub(super) type PluginImportResult = Option<Result<ClapPluginDesc, String>>;

impl DawView {
    pub(super) fn import_plugin_dialog(&mut self) {
        if self.plugin_import.is_some() {
            return;
        }
        let (send, receive) = mpsc::channel();
        self.plugin_import = Some(receive);
        std::thread::spawn(move || {
            let imported = rfd::FileDialog::new()
                .add_filter("Flow Music plugins", &["fmx"])
                .pick_file()
                .map(|path| {
                    daw_io::flow::import_plugin(&path).map_err(|error| format!("{error:#}"))
                });
            let _ = send.send(imported);
        });
    }

    pub(super) fn pump_plugin_import(&mut self) -> bool {
        let imported = match self.plugin_import.as_ref().map(|result| result.try_recv()) {
            Some(Ok(imported)) => imported,
            Some(Err(TryRecvError::Disconnected)) => Some(Err("The import worker stopped".into())),
            _ => return false,
        };
        self.plugin_import = None;
        match imported {
            Some(Ok(plugin)) => {
                self.st.set_status(format!("Imported {}", plugin.name));
                if self.fx_library.open && plugin.is_effect {
                    self.fx_library.reveal(&plugin.name);
                }
                self.st.browser_search = plugin.name.clone();
                self.browser_scroll.set_offset(gpui::point(px(0.), px(0.)));
                self.st.clap_plugins.retain(|existing| existing.id != plugin.id);
                self.st.clap_plugins.push(plugin);
                self.st.clap_plugins.sort_by(|a, b| a.name.cmp(&b.name));
            }
            Some(Err(error)) => self.st.set_status(format!("Plugin import failed: {error}")),
            None => {}
        }
        true
    }

    pub(super) fn focus_instrument_browser(&mut self, window: &mut Window) {
        if self.close_confirmation || self.input_track.is_some() {
            return;
        }
        self.st.open_menu = None;
        self.st.fx_window_open = false;
        self.st.browser_open = true;
        self.search_focus.focus(window);
    }

    pub(super) fn dismiss_instrument_browser(&mut self, window: &mut Window) {
        self.st.browser_open = false;
        if self.search_focus.is_focused(window) {
            self.root_focus.focus(window);
        }
    }

    pub(super) fn instrument_browser(&self, cx: &mut Context<Self>, window: &Window) -> gpui::Div {
        let theme = self.theme();
        let window_width = f32::from(window.viewport_size().width);
        let content_height = window_frame::content_height(window);
        let width = layout::instrument_browser_width(window_width);
        let available = self.st.open_menu.is_none()
            && !self.st.fx_window_open
            && !self.close_confirmation
            && self.input_track.is_none();
        let open = available && self.st.browser_open;
        let focused = self.search_focus.is_focused(window);
        let empty = self.st.browser_search.is_empty();
        let field = div()
            .id("browser-search")
            .debug_selector(|| "browser-search".into())
            .key_context("BrowserSearch")
            .track_focus(&self.search_focus)
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .gap_2()
            .px_2()
            .cursor_pointer()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.focus_instrument_browser(window);
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                cx.stop_propagation();
                let key = &event.keystroke;
                if key.modifiers.control || key.modifiers.platform || key.modifiers.alt {
                    return;
                }
                match key.key.as_str() {
                    "space" => this.st.browser_search.push(' '),
                    "backspace" => {
                        this.st.browser_search.pop();
                    }
                    _ => {
                        if let Some(text) = &key.key_char {
                            this.st.browser_search.push_str(text);
                        } else if key.key.chars().count() == 1 {
                            this.st.browser_search.push_str(&key.key);
                        } else {
                            return;
                        }
                    }
                }
                this.browser_scroll.set_offset(gpui::point(px(0.), px(0.)));
                cx.notify();
            }))
            .child(div().text_color(rgb(theme.text_dim)).child("⌕"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(rgb(if empty { theme.text_dim } else { theme.text }))
                    .child(match (empty, focused) {
                        (true, false) => SharedString::from("Browse instruments…"),
                        (true, true) => SharedString::from("▎ Browse instruments…"),
                        (false, true) => SharedString::from(format!("{}▎", self.st.browser_search)),
                        (false, false) => SharedString::from(self.st.browser_search.clone()),
                    }),
            )
            .children(empty.then(|| {
                div()
                    .flex_shrink_0()
                    .text_size(px(10.))
                    .text_color(rgb(theme.text_dim))
                    .child("Ctrl+K")
            }));

        let header = div().flex().h(px(HEADER_H)).items_center().child(
            div()
                .w_full()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded_md()
                .border_1()
                .border_color(rgb(if open { self.accent() } else { theme.border }))
                .bg(rgb(theme.panel_alt))
                .text_size(px(13.))
                .child(field)
                .children((!empty).then(|| {
                    theme.cell_btn("browser-clear", "×", false, theme.panel_alt)
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.st.browser_search.clear();
                            this.browser_scroll.set_offset(gpui::point(px(0.), px(0.)));
                            this.focus_instrument_browser(window);
                            cx.notify();
                        }))
                })),
        );

        let group = overlay_surface()
            .id("instrument-browser")
            .absolute()
            .left(px((window_width - width) * 0.5))
            .top_0()
            .w(px(width))
            .flex()
            .flex_col()
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                let visible = available && (*hovered || this.search_focus.is_focused(window));
                if visible != this.st.browser_open {
                    this.st.browser_open = visible;
                    cx.notify();
                }
            }))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if this.st.browser_open {
                    this.dismiss_instrument_browser(window);
                    cx.notify();
                }
            }))
            .child(header)
            .children(open.then(|| {
                self.instrument_results(cx, (content_height - HEADER_H - 12.).clamp(150., 490.))
            }));
        div().absolute().inset_0().child(group)
    }

    fn instrument_results(&self, cx: &mut Context<Self>, height: f32) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme();
        let selected = self
            .st
            .selected_track
            .as_ref()
            .and_then(|id| self.st.project.tracks.iter().find(|track| &track.id == id));
        let target = selected
            .map(|track| format!("For: {}", track.name))
            .unwrap_or_else(|| "Select a track to load an instrument".into());
        let audio_track = selected.is_some_and(|track| track.track_type == daw_core::project::TrackType::Audio);
        let active_preset = selected.and_then(|track| match &track.instrument {
            daw_core::project::Instrument::Soundfont {
                preset, bank: 0, ..
            } => Some(*preset),
            _ => None,
        });
        let query = self.st.browser_search.trim().to_lowercase();
        let category = self.st.browser_category.min(GM_CATEGORIES.len() - 1);
        let mut results = div().flex().flex_col().p_2();
        if !audio_track && query.is_empty() {
            let chips = GM_CATEGORIES.iter().enumerate().map(|(index, (name, _))| {
                theme.cell_btn(
                    format!("cat-{index}"),
                    *name,
                    category == index,
                    self.accent(),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.st.set_browser_category(index);
                    cx.notify();
                }))
            });
            results = results.child(div().flex().flex_wrap().gap_1().pb_2().children(chips));
        }

        let mut presets = Vec::new();
        for (index, (name, entries)) in GM_CATEGORIES.iter().enumerate().filter(|_| !audio_track) {
            if query.is_empty() && index != category {
                continue;
            }
            for (preset, program) in *entries {
                let label = if query.is_empty() {
                    format!("{preset} ({program})")
                } else {
                    format!("{preset} · {name}")
                };
                if query_matches(&query, &format!("{preset} {name} {program}")) {
                    presets.push(self.preset_row(
                        cx,
                        label,
                        *program,
                        active_preset == Some(*program),
                    ));
                }
            }
        }

        let mut plugins = Vec::new();
        let mut flow_plugins = Vec::new();
        for plugin in &self.st.clap_plugins {
            if audio_track && !plugin.is_effect {
                continue;
            }
            let aliases = if plugin.id == model_m_midi::PLUGIN_ID {
                "moog"
            } else {
                ""
            };
            let search_label = format!("{} {} {} {aliases}", plugin.name, plugin.vendor, plugin.id);
            if !query_matches(&query, &search_label) {
                continue;
            }
            let plugin = plugin.clone();
            let load_instrument = plugin.is_instrument && !audio_track;
            let tag = if load_instrument {
                "Instrument"
            } else {
                "FX"
            };
            let label = format!("{} · {tag}", plugin.name);
            let key = format!("clap-{}-{}", plugin.path, plugin.id);
            let selector = key.clone();
            let rows = if plugin.id.starts_with(daw_core::audio::flow::ID_PREFIX) {
                &mut flow_plugins
            } else {
                &mut plugins
            };
            rows.push(
                div()
                    .id(SharedString::from(key))
                    .debug_selector(move || selector.clone())
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .truncate()
                    .text_color(rgb(theme.text))
                    .hover(|s| s.bg(rgb(theme.panel_alt)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if load_instrument {
                            this.set_clap_instrument(plugin.path.clone(), plugin.id.clone());
                        } else {
                            this.add_clap_fx(plugin.path.clone(), plugin.id.clone());
                        }
                        cx.notify();
                    })),
            );
        }

        if presets.is_empty() && plugins.is_empty() && flow_plugins.is_empty() {
            results = results.child(
                div()
                    .p_2()
                    .text_color(rgb(theme.text_dim))
                    .child("No matching instruments or effects."),
            );
        }
        if !presets.is_empty() {
            results = results
                .child(theme.section_title("SoundFont presets"))
                .children(presets);
        }
        if !plugins.is_empty() {
            results = results.child(theme.section_title("CLAP")).children(plugins);
        }
        if !flow_plugins.is_empty() {
            results = results.child(theme.section_title("Flow Music")).children(flow_plugins);
        }

        div()
            .id("instrument-results")
            .debug_selector(|| "instrument-results".into())
            .max_h(px(height))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.panel))
            .shadow_md()
            .text_size(px(13.))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div().flex_1().min_w_0().text_color(rgb(theme.text_dim)).truncate().child(target),
                    )
                    .child(
                        theme.cell_btn(
                            "import-plugin",
                            if self.plugin_import.is_some() { "Importing…" } else { "Import plugin…" },
                            false,
                            theme.panel_alt,
                        )
                        .debug_selector(|| "import-plugin".into())
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.focus_instrument_browser(window);
                            this.import_plugin_dialog();
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .id("instrument-results-scroll")
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.browser_scroll)
                    .child(results),
            )
    }

    fn preset_row(
        &self,
        cx: &mut Context<Self>,
        label: String,
        program: u8,
        active: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme();
        div()
            .id(SharedString::from(format!("preset-{program}")))
            .debug_selector(move || format!("preset-{program}"))
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .truncate()
            .bg(rgb(if active { self.accent() } else { theme.panel }))
            .text_color(rgb(if active { theme.bg } else { theme.text }))
            .when(!active, |row| row.hover(|s| s.bg(rgb(theme.panel_alt))))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(track) = this.st.selected_track.clone() {
                    this.undo.record("preset", &this.st.project);
                    if this.st.project.set_track_preset(&track, program) {
                        this.push_live();
                    }
                }
                cx.notify();
            }))
    }
}

fn query_matches(query: &str, label: &str) -> bool {
    let label = label.to_lowercase();
    query.split_whitespace().all(|word| label.contains(word))
}

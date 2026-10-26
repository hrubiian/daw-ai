use super::*;

impl DawView {
    pub(super) fn browser(&self, cx: &mut Context<Self>, width: f32) -> gpui::Div {
        let q = self.st.browser_search.clone();
        let accent = self.accent();
        // The preset currently plugged into the selected track — highlighted in
        // the lists below so you can see (and confirm) what's loaded.
        let active_preset: Option<u8> = self.st.selected_track.as_ref()
            .and_then(|tid| self.st.project.tracks.iter().find(|t| &t.id == tid))
            .and_then(|t| match &t.instrument {
                // Only melodic-bank presets correspond to the browser list.
                daw_core::project::Instrument::Soundfont { preset, bank: 0, .. } => Some(*preset),
                _ => None,
            });

        // Search input — mirrors the REPL pattern.
        let search_field = div()
            .id("browser-search").flex_shrink_0()
            .key_context("BrowserSearch")
            .track_focus(&self.search_focus)
            .px_2()
            .py_1()
            .m_1()
            .rounded_md()
            .bg(rgb(PANEL_ALT))
            .text_color(rgb(TEXT))
            .cursor_pointer()
            .child(if q.is_empty() {
                SharedString::from("search instruments…")
            } else {
                SharedString::from(format!("{}▎", q))
            })
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _e, window, cx| {
                cx.stop_propagation();
                this.search_focus.focus(window);
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
                cx.stop_propagation();
                if ev.keystroke.modifiers.control || ev.keystroke.modifiers.platform || ev.keystroke.modifiers.alt { return; }
                let key = ev.keystroke.key.as_str();
                match key {
                    "space" => { this.st.browser_search.push(' '); }
                    "backspace" => { this.st.browser_search.pop(); }
                    "escape"    => { this.st.browser_search.clear(); }
                    _ => {
                        // Prefer key_char (shifted chars/punctuation), but fall back
                        // to the key name — with IME disabled (the XWayland launch),
                        // key_char is None for plain letters, so without this the
                        // field captured nothing. Mirrors the Code REPL input.
                        if let Some(s) = &ev.keystroke.key_char {
                            this.st.browser_search.push_str(s);
                        } else if key.chars().count() == 1 {
                            this.st.browser_search.push_str(key);
                        }
                    }
                }
                cx.notify();
            }));

        let cat = self.st.browser_category.min(GM_CATEGORIES.len() - 1);

        let content = div().id("browser-content").flex_1().min_h_0().overflow_y_scroll();
        let target = self.st.selected_track.as_ref().and_then(|id| self.st.project.tracks.iter().find(|t| &t.id == id))
            .map(|t| format!("For: {}", t.name)).unwrap_or_else(|| "Select a track to load an instrument".into());
        let frame = div()
            .w(px(width))
            .min_h_0()
            .overflow_hidden()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(section_title("Browser"))
            .child(search_field)
            .child(div().flex_shrink_0().px_2().text_size(px(12.)).text_color(rgb(TEXT_DIM)).child(target));
        let mut panel = div().flex().flex_col();

        if q.is_empty() {
            // Normal view: category list + preset list.
            let cats = GM_CATEGORIES.iter().enumerate().map(|(i, (name, _))| {
                let is_sel = i == cat;
                div()
                    .id(SharedString::from(format!("cat-{i}")))
                    .px_2().py_1().rounded_md().cursor_pointer().whitespace_nowrap()
                    .bg(rgb(if is_sel { PANEL_ALT } else { PANEL }))
                    .text_color(rgb(if is_sel { TEXT } else { TEXT_DIM }))
                    .child(*name)
                    .on_click(cx.listener(move |this, _e, _w, cx| { this.st.set_browser_category(i); cx.notify(); }))
            });

            let presets = GM_CATEGORIES[cat].1.iter().map(|(name, prog)| {
                let prog = *prog;
                let is_active = active_preset == Some(prog);
                div()
                    .id(SharedString::from(format!("preset-{prog}")))
                    .px_2().py_1().rounded_md().cursor_pointer().whitespace_nowrap()
                    .bg(rgb(if is_active { accent } else { PANEL }))
                    .text_color(rgb(if is_active { BG } else { TEXT }))
                    .child(format!("{name} ({prog})"))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        if let Some(tid) = this.st.selected_track.clone() {
                            this.undo.record("preset", &this.st.project);
                            if this.st.project.set_track_preset(&tid, prog) {
                                this.push_live();
                            }
                        }
                        cx.notify();
                    }))
            });

            panel = panel
                .child(div().flex().flex_col().gap_1().p_1().overflow_hidden().children(cats))
                .child(section_title("Presets"))
                .child(div().flex().flex_col().gap_1().p_1().overflow_hidden().children(presets));
        } else {
            // Filtered view: flat list of matching presets across all categories.
            let needle = q.to_lowercase();
            let matches = GM_CATEGORIES.iter().flat_map(|(cat_name, presets)| {
                presets.iter().map(move |(name, prog)| (*cat_name, *name, *prog))
            })
            // Match the preset name OR its category, so a category word like
            // "bass"/"piano"/"guitar" finds its presets (which are named
            // "Acoustic"/"Finger"/… and don't contain the category word).
            .filter(|(cat_name, name, _)| {
                name.to_lowercase().contains(&needle) || cat_name.to_lowercase().contains(&needle)
            })
            .map(|(cat_name, name, prog)| {
                let is_active = active_preset == Some(prog);
                div()
                    .id(SharedString::from(format!("hit-{prog}-{name}")))
                    .px_2().py_1().rounded_md().cursor_pointer()
                    .bg(rgb(if is_active { accent } else { PANEL }))
                    .text_color(rgb(if is_active { BG } else { TEXT }))
                    .child(format!("{name}  ·  {cat_name}"))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        if let Some(tid) = this.st.selected_track.clone() {
                            this.undo.record("preset", &this.st.project);
                            if this.st.project.set_track_preset(&tid, prog) { this.push_live(); }
                        }
                        cx.notify();
                    }))
            }).collect::<Vec<_>>();

            let no_matches = matches.is_empty();
            panel = panel
                .child(section_title("Matches"))
                .child(div().flex().flex_col().gap_1().p_1().overflow_hidden()
                    .children(matches)
                    .children(no_matches.then(|| div().p_2().text_color(rgb(TEXT_DIM))
                        .child("No instruments found. Try a category such as piano or bass. Press Esc to clear."))));
        }

        // CLAP plugins (background discovery scan). Instruments retarget the
        // selected track; effects append to its insert chain.
        if !self.st.clap_plugins.is_empty() {
            let rows = self.st.clap_plugins.iter().cloned().map(|d| {
                let tag = if d.is_instrument { "[I]" } else { "[FX]" };
                let label = format!("{tag} {}", d.name);
                let key = format!("clap-{}-{}", d.path, d.id);
                div()
                    .id(SharedString::from(key))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(PANEL))
                    .text_color(rgb(TEXT))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(label)
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        if d.is_instrument {
                            this.set_clap_instrument(d.path.clone(), d.id.clone());
                        } else {
                            this.add_clap_fx(d.path.clone(), d.id.clone());
                        }
                        cx.notify();
                    }))
            });
            panel = panel
                .child(section_title("CLAP"))
                .child(div().flex().flex_col().gap_1().p_1().overflow_hidden().children(rows));
        }

        frame.child(content.child(panel))
    }
}

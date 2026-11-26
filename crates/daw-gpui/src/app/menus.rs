use super::*;

impl DawView {
    pub(super) fn song_toolbar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();

        let tracks = header_group()
            .child(theme.cell_btn("add-mel", "+ MIDI", false, theme.panel_alt).on_click(cx.listener(|this, _e, _w, cx| { this.add_track(daw_core::project::TrackType::Melodic); cx.notify(); })))
            .child(theme.cell_btn("add-drum", "+ Drum", false, theme.panel_alt).on_click(cx.listener(|this, _e, _w, cx| { this.add_track(daw_core::project::TrackType::Drum); cx.notify(); })))
            .child(theme.cell_btn("add-aud", "+ Audio", false, theme.panel_alt).on_click(cx.listener(|this, _e, _w, cx| { this.add_track(daw_core::project::TrackType::Audio); cx.notify(); })))
            // Always available: the pump auto-creates an Audio track when the
            // selection isn't one, so +WAV works from anywhere.
            .child(
                theme.cell_btn("add-wav", "Import WAV", false, theme.panel_alt)
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        let slot = this.audio_clip_result.clone();
                        std::thread::spawn(move || {
                            if let Some(f) = rfd::FileDialog::new()
                                .add_filter("WAV", &["wav"])
                                .pick_file()
                            {
                                *slot.lock().unwrap() = Some(f);
                            }
                        });
                        cx.notify();
                    }))
            );
        let timeline = header_group()
            .child(div().text_color(rgb(theme.text_dim)).child("Timeline"))
            .child(theme.cell_btn("arr-start", "⏮", false, theme.panel_alt)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.scroll_timeline_to(0., window);
                    cx.notify();
                })))
            .child(theme.cell_btn("arr-left", "◀", false, theme.panel_alt)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.scroll_timeline_to(this.st.arrange_scroll_x - 4. * this.bar_w(), window);
                    cx.notify();
                })))
            .child(theme.cell_btn("arr-right", "▶", false, theme.panel_alt)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.scroll_timeline_to(this.st.arrange_scroll_x + 4. * this.bar_w(), window);
                    cx.notify();
                })))
            .child(theme.cell_btn("arr-end", "⏭", false, theme.panel_alt)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.scroll_timeline_to(this.arrangement_end_offset(window), window);
                    cx.notify();
                })))
            .child(theme.cell_btn("arr-fit", "⤢", false, theme.panel_alt)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.st.arrange_zoom = 1.;
                    this.scroll_timeline_to(0., window);
                    cx.notify();
                })))
            .child(theme.cell_btn_with_help(
                "volume-automation", "Volume automation", self.volume_automation_open, self.accent(),
                "Click to add a point. Drag to move it. Right-click to remove it.".into(),
            ).on_click(cx.listener(|this, _, _, cx| {
                this.volume_automation_open = !this.volume_automation_open;
                cx.notify();
            })));
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .h(px(TOOLS_H))
            .flex_shrink_0()
            .bg(rgb(theme.panel))
            .border_b_1()
            .border_color(rgb(theme.border))
            .text_size(px(11.))
            .line_height(px(16.))
            .text_color(rgb(theme.text_dim))
            .child(
                div()
                    .id("song-toolbar")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .whitespace_nowrap()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(tracks)
                    .child(theme.header_divider())
                    .child(timeline),
            )
            .child(header_group()
                .children(self.input_meter())
                .child(self.setup_button(TopMenu::Microphone, cx))
                .child(self.setup_button(TopMenu::Midi, cx))
                .child(self.amp_pill(cx))
                .child(self.amp_setup_button(cx)))
            .child(theme.cell_btn("panic", "Panic", false, RECORD).on_click(
                cx.listener(|this, _, _, cx| {
                    this.panic_engine();
                    cx.notify();
                }),
            ))
    }

    fn setup_panel_top(&self) -> f32 {
        ARRANGE_HEADER_H + if self.sf2_missing { SOUNDFONT_BANNER_H } else { 0. }
    }

    fn setup_button(&self, menu: TopMenu, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let (id, label, icon) = match menu {
            TopMenu::Microphone => ("mic-setup", "Microphone setup", SetupIcon::Microphone),
            TopMenu::Midi => ("midi-setup", "MIDI keyboard setup", SetupIcon::Keyboard),
            _ => unreachable!(),
        };
        let theme = self.theme();
        let accent = self.accent();
        let active = self.st.open_menu == Some(menu);
        let color = if active { theme::contrast_text(accent) } else { theme.text };
        theme.control(id.into(), active, accent, label.into())
            .child(icon.render(color))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.st.browser_open = false;
                this.root_focus.focus(window);
                if this.st.open_menu == Some(menu) {
                    this.st.open_menu = None;
                } else {
                    this.st.open_menu = Some(menu);
                    this.scan_input_devices();
                }
                cx.notify();
            }))
    }

    pub(super) fn key_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let key = format!("{} {}", self.st.project.project.key, self.st.project.project.scale);
        header_group()
            // Tonality / transpose
            .child(theme.cell_btn("key-down", "−", false, theme.panel_alt).on_click(cx.listener(|this, _e, _w, cx| { this.transpose(-1); cx.notify(); })))
            .child(div().min_w(px(72.)).text_color(rgb(theme.text)).child(key))
            .child(theme.cell_btn("key-up", "+", false, theme.panel_alt).on_click(cx.listener(|this, _e, _w, cx| { this.transpose(1); cx.notify(); })))
    }

    pub(super) fn cycle_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let accent = self.accent();
        let metro = self.st.project.project.metronome;
        header_group()
            // Metronome toggle
            .child(
                theme.cell_btn("metro", "Metro", metro, accent)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.toggle_metronome(); cx.notify(); }))
            )
            // Loop region + punch toggles (one region, three switches).
            .child(
                theme.cell_btn("loop", "⟳ Loop", self.st.project.loop_region.enabled, accent)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.toggle_loop(); cx.notify(); }))
            )
            .child(
                theme.cell_btn("punch-in", "⇥In", self.st.project.loop_region.punch_in, RECORD)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.toggle_punch_in(); cx.notify(); }))
            )
            .child(
                theme.cell_btn("punch-out", "Out⇤", self.st.project.loop_region.punch_out, RECORD)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.toggle_punch_out(); cx.notify(); }))
            )
    }

    /// Leave the center of the header for the instrument search.
    pub(super) fn menu_bar(&self, cx: &mut Context<Self>, width: f32) -> gpui::Div {
        let theme = self.theme();
        const FILE_W: f32 = 54.;
        const SETTINGS_W: f32 = 86.;
        let left_room = (width - layout::instrument_browser_width(width)) * 0.5;
        let wordmark_width = (left_room - FILE_W - SETTINGS_W - 8.).max(60.);
        let file_open = self.st.open_menu == Some(TopMenu::File);
        let settings_open = self.st.open_menu == Some(TopMenu::Settings);
        let agent_session_open = self.st.open_menu == Some(TopMenu::AgentSession);
        let fx_open = self.st.fx_window_open;
        let accent = self.accent();
        div()
            .flex()
            .items_center()
            .h(px(HEADER_H))
            .flex_shrink_0()
            .bg(rgb(theme.panel))
            .border_b_1()
            .border_color(rgb(theme.border))
            .text_size(px(12.))
            .text_color(rgb(theme.text))
            // App wordmark (left of the menus).
            .child(
                div()
                    .px_3()
                    .max_w(px(wordmark_width))
                    .h_full()
                    .flex()
                    .items_center()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(self.accent()))
                    .child(div().min_w_0().truncate().child(if width >= 1360. {
                        concat!("daw-ai ", env!("CARGO_PKG_VERSION"), " · ", env!("DAW_BUILD_REVISION"))
                    } else if width >= 990. {
                        concat!("daw-ai ", env!("CARGO_PKG_VERSION"))
                    } else {
                        "daw-ai"
                    })),
            )
            .child(
                div()
                    .id("menu-file")
                    .w(px(FILE_W)).flex_shrink_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(rgb(if file_open { theme.panel_alt } else { theme.panel }))
                    .text_color(rgb(theme.text))
                    .child("File ▾")
                    .on_click(cx.listener(|this, _e, w, cx| {
                        this.st.browser_open = false;
                        this.root_focus.focus(w);
                        let m = TopMenu::File;
                        this.st.open_menu = if this.st.open_menu == Some(m) { None } else { Some(m) };
                        cx.notify();
                    }))
            )
            .child(
                div()
                    .id("menu-settings")
                    .debug_selector(|| "menu-settings".into())
                    .w(px(SETTINGS_W)).flex_shrink_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(rgb(if settings_open { theme.panel_alt } else { theme.panel }))
                    .text_color(rgb(theme.text))
                    .child("Settings ▾")
                    .on_click(cx.listener(|this, _e, w, cx| {
                        this.st.browser_open = false;
                        this.root_focus.focus(w);
                        let m = TopMenu::Settings;
                        let opening = this.st.open_menu != Some(m);
                        if opening {
                            this.scan_input_devices();
                        }
                        this.st.open_menu = if opening { Some(m) } else { None };
                        cx.notify();
                    }))
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("menu-audio-effects")
                    .debug_selector(|| "menu-audio-effects".into())
                    .w(px(105.)).flex_shrink_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(rgb(if fx_open { theme.panel_alt } else { theme.panel }))
                    .text_color(rgb(if fx_open { accent } else { theme.text }))
                    .child("Audio Effects")
                    .on_click(cx.listener(|this, _e, w, cx| {
                        this.st.browser_open = false;
                        this.root_focus.focus(w);
                        this.st.fx_window_open = !this.st.fx_window_open;
                        this.fx_library.open = false;
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
            .child(
                div()
                    .id("menu-agent-session")
                    .w(px(121.)).flex_shrink_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(rgb(if agent_session_open { theme.panel_alt } else { theme.panel }))
                    .child("Agent Session ▾")
                    .on_click(cx.listener(|this, _, w, cx| {
                        this.st.browser_open = false;
                        this.root_focus.focus(w);
                        let menu = TopMenu::AgentSession;
                        this.st.open_menu = if this.st.open_menu == Some(menu) { None } else { Some(menu) };
                        cx.notify();
                    })),
            )
    }

    /// Overlay: full-window scrim (closes on outside-click) + the open dropdown panel.
    /// Returns `None` when no menu is open.
    pub(super) fn menu_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let open = self.st.open_menu?;
        let panel = match open {
            TopMenu::File => self.file_menu(cx).into_any_element(),
            TopMenu::Settings | TopMenu::Microphone | TopMenu::Midi => {
                self.settings_panel(cx).into_any_element()
            }
            TopMenu::AgentSession => self.agent_session_panel(cx).into_any_element(),
        };
        let overlay = div()
            .absolute()
            .inset_0()
            .child(
                // Start below the active menu's toolbar so its toggle stays clickable.
                overlay_surface()
                    .id("menu-scrim")
                    .absolute()
                    .top(px(if matches!(open, TopMenu::Microphone | TopMenu::Midi) {
                        self.setup_panel_top()
                    } else {
                        HEADER_H
                    }))
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.st.open_menu = None;
                        cx.stop_propagation();
                        cx.notify();
                    }))
            )
            .child(panel);
        Some(overlay.into_any_element())
    }

    fn agent_session_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let panel = div()
            .absolute()
            .top(px(HEADER_H))
            .right(px(8.))
            .max_w(px(420.))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(theme.panel))
            .border_1()
            .border_color(rgb(theme.border))
            .text_color(rgb(theme.text))
            .child(self.project_name());
        let Some(session) = &self.host.session else {
            return panel.child("Live session unavailable");
        };
        panel
            .child(div().text_color(rgb(theme.text_dim)).child(self.live_status.clone().unwrap_or_else(|| session.status_line())))
            .child(div().text_color(rgb(theme.text_dim)).child("Add this configuration once. Your agent connects automatically when one daw-ai session is open."))
            .child(theme.cell_btn("copy-agent-config", "Copy MCP configuration", false, theme.panel_alt)
                .on_click(cx.listener(|this, _, _, cx| {
                    let result = (|| -> anyhow::Result<String> {
                        let binary = std::env::current_exe()?.with_file_name("daw-mcp");
                        anyhow::ensure!(binary.is_file(), "Build daw-mcp in the same Cargo profile as daw-ai before connecting.");
                        Ok(serde_json::to_string_pretty(&serde_json::json!({
                            "mcpServers": {"daw": {"command": binary, "args": ["--live"]}}
                        }))?)
                    })();
                    match result {
                        Ok(config) => {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(config));
                            this.st.set_status("MCP configuration copied");
                            this.st.open_menu = None;
                        }
                        Err(error) => {
                            this.st.set_status(error.to_string());
                            this.st.open_menu = None;
                        }
                    }
                    cx.notify();
                })))
    }

    /// Overlay panel for Audio Effects workspace.
    /// Returns `None` when `fx_window_open` is false.
    /// Covers the body area below menu (24) + transport (48) + toolbar (30) + status (18) = 120px.
    pub(super) fn audio_effects_overlay(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let theme = self.theme();
        if !self.st.fx_window_open { return None; }

        let track_name = self.st.selected_track.as_ref()
            .and_then(|tid| self.st.project.tracks.iter().find(|t| &t.id == tid))
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "no track".to_string());

        let scope=match self.st.selected_return {Some(0)=>"Shared reverb",Some(_)=>"Shared echo",None=>&track_name};
        let title = format!("Audio Effects \u{2014} {scope}");

        let header = div()
            .flex()
            .items_center()
            .h(px(32.))
            .px_3()
            .bg(rgb(theme.panel))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(theme.text))
                    .child(title)
            )
            .child(
                div()
                    .id("fx-close")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(theme.panel_alt))
                    .text_color(rgb(theme.text_dim))
                    .child("\u{2715}")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.st.fx_window_open = false;
                        this.fx_library.open = false;
                        cx.notify();
                    }))
            );

        let left_col = div()
            .w(gpui::relative(0.44))
            .min_w(px(264.))
            .max_w(px(392.))
            .flex_shrink_0()
            .bg(rgb(theme.panel))
            .border_r_1()
            .border_color(rgb(theme.border))
            .overflow_hidden()
            .child(self.effects_panel(cx, true));

        self.output_analyzer.poll(&self.host);
        let right_col = div()
            .id("fx-right-scroll")
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .p_2()
            .child(self.output_analyzer.panel(
                self.host.sample_rate.load(Ordering::Relaxed) > 0,
                self.config.light_theme,
            ));

        let body = div()
            .flex()
            .flex_1()
            .min_h(px(0.))
            .child(left_col)
            .child(right_col);

        let panel = overlay_surface()
            .id("audio-effects-workspace")
            .absolute()
            .left(px(0.))
            .top(px(ARRANGE_HEADER_H + if self.sf2_missing { SOUNDFONT_BANNER_H } else { 0. }))
            .right(px(0.))
            .bottom(px(0.))
            .bg(rgb(theme.bg))
            .flex()
            .flex_col()
            .child(header)
            .child(body);

        Some(panel.into_any_element())
    }

    /// File dropdown with project actions and audio exports.
    pub(super) fn file_menu(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        // Each row is built individually to attach distinct on_click closures.
        overlay_surface()
            .absolute()
            .left(px(0.))
            .top(px(HEADER_H))
            .min_w(px(160.))
            .flex()
            .flex_col()
            .bg(rgb(theme.panel_alt))
            .border_1()
            .border_color(rgb(theme.border))
            .rounded_md()
            .shadow_md()
            .overflow_hidden()
            .child(
                div().id("fm-new").px_3().py_1().cursor_pointer().text_color(rgb(theme.text))
                    .bg(rgb(theme.panel_alt)).whitespace_nowrap().child("New")
                    .on_click(cx.listener(|this, _e, w, cx| {
                        this.st.open_menu = None;
                        this.request_replace(ConfirmThen::New, w);
                        cx.notify();
                    }))
            )
            .child(
                div().id("fm-load").px_3().py_1().cursor_pointer().text_color(rgb(theme.text))
                    .bg(rgb(theme.panel_alt)).whitespace_nowrap().child("Open…")
                    .on_click(cx.listener(|this, _e, w, cx| {
                        this.st.open_menu = None;
                        this.request_replace(ConfirmThen::Open, w);
                        cx.notify();
                    }))
            )
            .child({
                let label = self.undo.undo_label().map(|l| format!("Undo {l}")).unwrap_or_else(|| "Undo".into());
                let fg = if self.undo.can_undo() { theme.text } else { theme.text_dim };
                div().id("fm-undo").px_3().py_1().cursor_pointer().text_color(rgb(fg))
                    .bg(rgb(theme.panel_alt)).whitespace_nowrap().child(label)
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.undo();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            })
            .child({
                let label = self.undo.redo_label().map(|l| format!("Redo {l}")).unwrap_or_else(|| "Redo".into());
                let fg = if self.undo.can_redo() { theme.text } else { theme.text_dim };
                div().id("fm-redo").px_3().py_1().cursor_pointer().text_color(rgb(fg))
                    .bg(rgb(theme.panel_alt)).whitespace_nowrap().child(label)
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.redo();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            })
            .child(
                div().id("fm-save").px_3().py_1().cursor_pointer().text_color(rgb(theme.text))
                    .bg(rgb(theme.panel_alt)).whitespace_nowrap().child("Save")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.save_project();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
            .child(
                div().id("fm-saveas").px_3().py_1().cursor_pointer().text_color(rgb(theme.text))
                    .bg(rgb(theme.panel_alt)).whitespace_nowrap().child("Save As…")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.save_as_dialog();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
            .child(
                div().id("fm-export").debug_selector(|| "fm-export".into()).px_3().py_1().cursor_pointer().text_color(rgb(theme.text))
                    .bg(rgb(theme.panel_alt)).whitespace_nowrap().child("Export WAV")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.export_audio(daw_core::audio::ExportFormat::Wav);
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
            .child(
                div().id("fm-export-mp3").debug_selector(|| "fm-export-mp3".into()).px_3().py_1().cursor_pointer().text_color(rgb(theme.text))
                    .bg(rgb(theme.panel_alt)).whitespace_nowrap().child("Export MP3")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.export_audio(daw_core::audio::ExportFormat::Mp3);
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
            .child(
                div().id("fm-import").px_3().py_1().cursor_pointer().text_color(rgb(theme.text))
                    .bg(rgb(theme.panel_alt)).whitespace_nowrap().child("Import Sample")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.import_sample_dialog();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
    }

    /// Settings dropdown panel for devices, recording defaults, and appearance.
    pub(super) fn settings_panel(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme();
        let menu = self.st.open_menu.unwrap_or(TopMenu::Settings);
        let dedicated = matches!(menu, TopMenu::Microphone | TopMenu::Midi);
        let sr = self.host.sample_rate.load(Ordering::Relaxed);
        overlay_surface().id(match menu {
                TopMenu::Microphone => "microphone-panel",
                TopMenu::Midi => "midi-panel",
                _ => "settings-panel",
            })
            .absolute()
            .when(dedicated, |d| d.right(px(8.)).top(px(self.setup_panel_top())))
            .when(!dedicated, |d| d.left(px(80.)).top(px(HEADER_H)))
            .bottom(px(8.))
            .w(px(320.))
            .flex()
            .flex_col()
            .bg(rgb(theme.panel_alt))
            .text_color(rgb(theme.text))
            .border_1()
            .border_color(rgb(theme.border))
            .rounded_md()
            .shadow_md()
            .overflow_y_scroll()
            .when(dedicated, |d| d.child(
                div().flex().items_center().justify_between().gap_2().p_2().flex_shrink_0()
                    .child(theme.cell_btn("refresh-setup", "Refresh devices", false, theme.panel_alt)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.scan_input_devices();
                            cx.notify();
                        })))
                    .child(theme.cell_btn("close-setup", "Close", false, theme.panel_alt)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.st.open_menu = None;
                            cx.notify();
                        }))),
            ))
            .when(!dedicated, |d| d.child(self.appearance_settings(cx)))
            .children(self.input_scan_running.then(|| {
                div().px_3().py_1().text_color(rgb(theme.text_dim)).child("Scanning devices…")
            }))
            .when(menu != TopMenu::Microphone, |d| d.child(self.midi_settings(cx)))
            .when(menu != TopMenu::Midi, |d| d.child(self.audio_input_settings(cx)))
            .when(!dedicated, |d| {
                d.child(theme.section_title("Audio"))
                    .child(div().px_3().py_1().text_color(rgb(theme.text_dim))
                        .child(if sr > 0 { format!("Sample rate: {sr} Hz") } else { "Sample rate: —".into() }))
                    .child(div().px_3().py_1().text_color(rgb(theme.text_dim))
                        .child(format!("Soundfont: {}", self.sf2_path)))
            })
    }

    fn appearance_settings(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let accent = self.accent();
        let swatch_row = div()
            .flex()
            .gap_2()
            .px_3()
            .py_2()
            .children(theme.accents.iter().enumerate().map(|(i, &c)| {
                let selected = self.st.accent_idx == i;
                div()
                    .id(SharedString::from(format!("acc-{i}")))
                    .w(px(20.))
                    .h(px(20.))
                    .rounded_md()
                    .bg(rgb(c))
                    .cursor_pointer()
                    .border_2()
                    .border_color(rgb(if selected { theme.text } else { theme.panel }))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.st.accent_idx = i;
                        cx.notify();
                    }))
            }));

        div().flex().flex_col().flex_shrink_0()
            .child(theme.section_title("Appearance"))
            .child(div().flex().gap_2().px_3().pt_2().children(
                [("theme-dark", "Dark", false), ("theme-light", "White", true)]
                    .into_iter()
                    .map(|(id, label, light)| {
                        theme.cell_btn(id, label, self.config.light_theme == light, accent)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.config.light_theme != light {
                                    this.config.light_theme = light;
                                    if let Err(error) = this.config.save() {
                                        this.st.set_status(format!("Could not save theme: {error}"));
                                    }
                                    cx.notify();
                                }
                            }))
                    }),
            ))
            .child(swatch_row)
    }

    fn midi_settings(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let midi_ports = self.midi_ports.clone();
        let cur_device = self.host.midi_device.clone();
        let cur_device_display = cur_device.clone().unwrap_or_else(|| "none".into());
        let midi_target = self.st.project.tracks.iter()
            .find(|track| Some(track.id.as_str()) == self.midi_track.as_deref())
            .map(|track| track.name.as_str())
            .unwrap_or("No instrument track");
        let accent = self.accent();

        let port_rows = midi_ports.into_iter().enumerate().map(|(i, name)| {
            let cur = Some(&name) == cur_device.as_ref();
            let fg = if cur { accent } else { theme.text };
            div()
                .id(SharedString::from(format!("midip-{i}")))
                .debug_selector(move || format!("midip-{i}"))
                .flex_shrink_0()
                .px_2()
                .py_1()
                .cursor_pointer()
                .bg(rgb(theme.panel))
                .text_color(rgb(fg))
                .whitespace_nowrap()
                .child(name.clone())
                .on_click(cx.listener(move |this, _e, _w, cx| {
                    if this.host.set_midi_port(i) {
                        this.host.enable_keylab_controls();
                    } else {
                        this.st.set_status("Could not open that MIDI port.");
                    }
                    cx.notify();
                }))
        });

        div().flex().flex_col().flex_shrink_0()
            .child(theme.section_title("MIDI keyboard"))
            .child(
                div().px_3().py_1().text_color(rgb(theme.text_dim))
                    .child(format!("Device: {}", cur_device_display))
            )
            .children(port_rows)
            .children((self.midi_ports.is_empty() && !self.input_scan_running).then(|| {
                div().px_3().py_1().text_color(rgb(theme.text_dim)).child("No MIDI inputs found")
            }))
            .children(self.host.keylab_controls_connected().then(|| {
                div().px_3().py_1().text_color(rgb(theme.text_dim)).child("KeyLab DAW controls connected")
            }))
            // Live MIDI routes to the selected track's strip automatically
            // (no manual channel pick since the per-track mixer landed).
            .child(
                div().px_3().py_1().text_color(rgb(theme.text_dim))
                    .child(format!("Keyboard plays: {midi_target}"))
            )
    }

    fn audio_input_settings(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let accent = self.accent();
        div().flex().flex_col().flex_shrink_0()
            .child(theme.section_title("Default audio input"))
            .child(div().px_3().text_color(rgb(theme.text_dim)).child("Audio tracks can choose their own input under In: …"))
            .children({
                let devices = std::iter::once((None, "System default".to_string())).chain(
                    self.input_devices.iter()
                        .filter(|device| device.name != "default")
                        .map(|device| (Some(device.name.clone()), format!("{} · {} inputs", device.label, device.channels))),
                );
                devices.enumerate().map(|(index, (device, label))| {
                    let selected = self.config.input_device == device;
                    theme.cell_btn(format!("in-dev-{index}"), label, selected, accent)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.config.input_device != device {
                                this.set_default_input(device.clone(), daw_core::audio::input::InputChannels::Mono(0));
                            }
                            cx.notify();
                        }))
                }).collect::<Vec<_>>()
            })
            .child({
                let max_channels = self.input_channel_count(self.config.input_device.as_deref());
                div().flex().flex_wrap().gap_1().px_2().py_1()
                    .children(super::inputs::audio_channel_choices(max_channels).enumerate().map(|(index, channels)| {
                        theme.cell_btn(format!("in-channel-{index}"), super::inputs::channel_label(channels),
                            self.config.input_channels == channels, accent)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_default_input(this.config.input_device.clone(), channels);
                                cx.notify();
                            }))
                    }))
            })
            .child(
                div().flex().items_center().gap_2().px_3().py_1()
                    .child(div().text_color(rgb(theme.text_dim)).child("Count-in:"))
                    .child(div().id("ci-down").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(theme.panel)).text_color(rgb(theme.text)).child("−")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            this.config.count_in_bars = this.config.count_in_bars.saturating_sub(1);
                            let _ = this.config.save();
                            cx.notify();
                        })))
                    .child(div().min_w(px(56.)).text_color(rgb(theme.text)).child(format!("{} bars", self.config.count_in_bars)))
                    .child(div().id("ci-up").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(theme.panel)).text_color(rgb(theme.text)).child("+")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            this.config.count_in_bars = (this.config.count_in_bars + 1).min(4);
                            let _ = this.config.save();
                            cx.notify();
                        })))
            )
            .child(
                div().flex().items_center().gap_2().px_3().py_1()
                    .child(div().text_color(rgb(theme.text_dim)).child("Latency trim:"))
                    .child(div().id("lt-down").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(theme.panel)).text_color(rgb(theme.text)).child("−")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            this.config.latency_trim_ms = (this.config.latency_trim_ms - 2.0).max(0.0);
                            let _ = this.config.save();
                            cx.notify();
                        })))
                    .child(div().min_w(px(64.)).text_color(rgb(theme.text)).child(format!("{:.0} ms", self.config.latency_trim_ms)))
                    .child(div().id("lt-up").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(theme.panel)).text_color(rgb(theme.text)).child("+")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            this.config.latency_trim_ms = (this.config.latency_trim_ms + 2.0).min(200.0);
                            let _ = this.config.save();
                            cx.notify();
                        })))
            )
    }

    pub(super) fn close_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let theme = self.theme();
        if !self.close_confirmation { return None; }
        let (title, discard, save) = match self.confirm_then {
            ConfirmThen::Close => ("Save changes before closing?", "Discard & close", "Save & close"),
            ConfirmThen::New => ("Save changes before starting a new project?", "Discard & start new", "Save & start new"),
            ConfirmThen::Open => ("Save changes before opening another project?", "Discard & open", "Save & open"),
        };
        Some(overlay_surface().id("close-confirmation").absolute().inset_0()
            .flex().items_center().justify_center().bg(gpui::rgba(0x00000099))
            .child(overlay_surface().w(px(480.)).p_4().rounded_md().bg(rgb(theme.panel)).text_color(rgb(theme.text))
                .child(div().text_lg().child(title))
                .child(div().py_2().child(self.project_name()))
                .child(div().py_2().text_color(rgb(theme.text_dim)).child(self.st.status.clone()))
                .child(div().flex().gap_2()
                    .child(theme.cell_btn("close-cancel", "Cancel", false, theme.panel_alt).on_click(cx.listener(|this, _, _, cx| {
                        this.close_confirmation = false; this.close_after_save = false;
                        this.confirm_then = ConfirmThen::Close; cx.notify();
                    })))
                    .child(theme.cell_btn("close-discard", discard, false, RECORD).on_click(cx.listener(|this, _, w, cx| {
                        match this.confirm_then {
                            ConfirmThen::Close => w.remove_window(),
                            then => {
                                this.close_confirmation = false;
                                this.close_after_save = false;
                                this.confirm_then = ConfirmThen::Close;
                                this.replace_project(then);
                                cx.notify();
                            }
                        }
                    })))
                    .child(theme.cell_btn("close-save", save, true, self.accent()).on_click(cx.listener(|this, _, _, cx| {
                        this.close_after_save = true;
                        this.save_project();
                        cx.notify();
                    })))))
            .into_any_element())
    }
}

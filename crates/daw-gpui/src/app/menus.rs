use super::*;

impl DawView {
    pub(super) fn song_toolbar(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let accent = self.accent();
        let key = format!("{} {}", self.st.project.project.key, self.st.project.project.scale);
        let swing = self.st.project.project.swing;
        let humanize = self.st.project.project.humanize;
        let timing = self.st.project.project.timing;
        let metro = self.st.project.project.metronome;
        div()
            .id("song-toolbar")
            .overflow_x_scroll()
            .whitespace_nowrap()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .h(px(30.))
            .flex_shrink_0()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .text_color(rgb(TEXT_DIM))
            // Tonality / transpose
            .child(cell_btn("key-down", "−", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.transpose(-1); cx.notify(); })))
            .child(div().min_w(px(72.)).text_color(rgb(TEXT)).child(key))
            .child(cell_btn("key-up", "+", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.transpose(1); cx.notify(); })))
            // Swing
            .child(div().child("Swing"))
            .child(cell_btn("sw-down", "−", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.adjust_swing(-0.05); cx.notify(); })))
            .child(div().min_w(px(40.)).text_color(rgb(TEXT)).child(format!("{:.0}%", swing * 100.0)))
            .child(cell_btn("sw-up", "+", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.adjust_swing(0.05); cx.notify(); })))
            // Humanize (click cycles)
            .child(
                cell_btn("humanize", format!("Humanize {:.0}%", humanize * 100.0), humanize > 0.0, accent)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.cycle_humanize(); cx.notify(); }))
            )
            // Timing humanization (note placement looseness)
            .child(div().child("Timing"))
            .child(cell_btn("tm-down", "−", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.adjust_timing(-0.05); cx.notify(); })))
            .child(div().min_w(px(40.)).text_color(rgb(if timing > 0.0 { accent } else { TEXT })).child(format!("{:.0}%", timing * 100.0)))
            .child(cell_btn("tm-up", "+", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.adjust_timing(0.05); cx.notify(); })))
            // Metronome toggle
            .child(
                cell_btn("metro", "Metro", metro, accent)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.toggle_metronome(); cx.notify(); }))
            )
            // Loop region + punch toggles (one region, three switches).
            .child(
                cell_btn("loop", "⟳ Loop", self.st.project.loop_region.enabled, accent)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.toggle_loop(); cx.notify(); }))
            )
            .child(
                cell_btn("punch-in", "⇥In", self.st.project.loop_region.punch_in, RECORD)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.toggle_punch_in(); cx.notify(); }))
            )
            .child(
                cell_btn("punch-out", "Out⇤", self.st.project.loop_region.punch_out, RECORD)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.toggle_punch_out(); cx.notify(); }))
            )
            // Panic
            .child(
                cell_btn("panic", "Panic", false, RECORD)
                    .on_click(cx.listener(|this, _e, _w, cx| { this.panic_engine(); cx.notify(); }))
            )
            // Arrange scroll controls (work regardless of mouse-wheel behaviour).
            .child(div().flex_1())
            .child(div().text_color(rgb(TEXT_DIM)).child("Arr:"))
            .child(cell_btn("arr-left", "◀", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| {
                let step = 4.0 * this.bar_w();
                this.st.arrange_scroll_x = (this.st.arrange_scroll_x - step).max(0.0);
                cx.notify();
            })))
            .child(cell_btn("arr-right", "▶", false, PANEL_ALT).on_click(cx.listener(|this, _e, w, cx| {
                let step = 4.0 * this.bar_w();
                let content_w = this.st.project.song_bars() as f32 * this.bar_w();
                let max_off = (content_w - this.timeline_width(w)).max(0.0);
                this.st.arrange_scroll_x = (this.st.arrange_scroll_x + step).clamp(0.0, max_off);
                cx.notify();
            })))
            .child(cell_btn("arr-fit", "⤢", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| {
                this.st.arrange_zoom = 1.0;
                this.st.arrange_scroll_x = 0.0;
                cx.notify();
            })))
            // Track add buttons
            .child(div().text_color(rgb(TEXT_DIM)).child("Track:"))
            .child(cell_btn("add-mel", "+ MIDI", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.add_track(daw_core::project::TrackType::Melodic); cx.notify(); })))
            .child(cell_btn("add-drum", "+Drum", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.add_track(daw_core::project::TrackType::Drum); cx.notify(); })))
            .child(cell_btn("add-aud", "+ Audio", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| { this.add_track(daw_core::project::TrackType::Audio); cx.notify(); })))
            // Always available: the pump auto-creates an Audio track when the
            // selection isn't one, so +WAV works from anywhere.
            .child(
                cell_btn("add-wav", "Import WAV", false, PANEL_ALT)
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
            )
    }

    /// Slim top row with File / Settings buttons.
    pub(super) fn menu_bar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let file_open = self.st.open_menu == Some(TopMenu::File);
        let settings_open = self.st.open_menu == Some(TopMenu::Settings);
        let fx_open = self.st.fx_window_open;
        let accent = self.accent();
        div()
            .flex()
            .items_center()
            .h(px(24.))
            .flex_shrink_0()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .text_color(rgb(TEXT))
            // App wordmark (left of the menus).
            .child(
                div()
                    .px_3()
                    .h_full()
                    .flex()
                    .items_center()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(self.accent()))
                    .child(concat!("daw-ai ", env!("CARGO_PKG_VERSION"), " · ", env!("DAW_BUILD_REVISION"))),
            )
            .child(
                div()
                    .id("menu-file")
                    .px_3()
                    .h_full()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .bg(rgb(if file_open { PANEL_ALT } else { PANEL }))
                    .text_color(rgb(TEXT))
                    .child("File ▾")
                    .on_click(cx.listener(|this, _e, w, cx| {
                        this.root_focus.focus(w);
                        let m = TopMenu::File;
                        this.st.open_menu = if this.st.open_menu == Some(m) { None } else { Some(m) };
                        cx.notify();
                    }))
            )
            .child(
                div()
                    .id("menu-settings")
                    .px_3()
                    .h_full()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .bg(rgb(if settings_open { PANEL_ALT } else { PANEL }))
                    .text_color(rgb(TEXT))
                    .child("Settings ▾")
                    .on_click(cx.listener(|this, _e, w, cx| {
                        this.root_focus.focus(w);
                        let m = TopMenu::Settings;
                        let opening = this.st.open_menu != Some(m);
                        if opening {
                            // Device enumeration is too slow for render(); do it once on open.
                            if !this.input_scan_running {
                                this.input_scan_running = true;
                                let result = this.input_scan.clone();
                                std::thread::spawn(move || {
                                    let devices = daw_core::audio::input::list_input_devices();
                                    if let Ok(mut slot) = result.lock() { *slot = Some((devices, daw_io::list_midi_ports())); }
                                });
                            }
                        }
                        this.st.open_menu = if opening { Some(m) } else { None };
                        cx.notify();
                    }))
            )
            .child(
                div()
                    .id("menu-audio-effects")
                    .px_3()
                    .h_full()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .bg(rgb(if fx_open { PANEL_ALT } else { PANEL }))
                    .text_color(rgb(if fx_open { accent } else { TEXT }))
                    .child("Audio Effects")
                    .on_click(cx.listener(|this, _e, w, cx| {
                        this.root_focus.focus(w);
                        this.st.fx_window_open = !this.st.fx_window_open;
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
    }

    /// Overlay: full-window scrim (closes on outside-click) + the open dropdown panel.
    /// Returns `None` when no menu is open.
    pub(super) fn menu_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let open = self.st.open_menu?;
        let panel = match open {
            TopMenu::File => self.file_menu(cx).into_any_element(),
            TopMenu::Settings => self.settings_panel(cx).into_any_element(),
        };
        let overlay = div()
            .absolute()
            .inset_0()
            .child(
                // scrim: covers the window *below* the 24px menu bar so an
                // outside click closes the dropdown. It must NOT overlap the menu
                // buttons, or a second click on File would hit the scrim (close)
                // AND the button beneath (re-open), leaving the menu stuck open.
                overlay_surface()
                    .id("menu-scrim")
                    .absolute()
                    .top(px(24.))
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

    /// Overlay panel for Audio Effects workspace.
    /// Returns `None` when `fx_window_open` is false.
    /// Covers the body area below menu (24) + transport (48) + toolbar (30) + status (18) = 120px.
    pub(super) fn audio_effects_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.st.fx_window_open { return None; }

        let accent = self.accent();

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
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(TEXT))
                    .child(title)
            )
            .child(
                div()
                    .id("fx-close")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(PANEL_ALT))
                    .text_color(rgb(TEXT_DIM))
                    .child("\u{2715}")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.st.fx_window_open = false;
                        cx.notify();
                    }))
            );

        // ── Left column: Effect Chain editor ────────────────────────────────
        let left_col = div()
            .w(px(360.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .overflow_hidden()
            .child(section_title("Effect Chain"))
            .child(
                div()
                    .id("fx-chain-scroll")
                    .flex_1()
                    .overflow_scroll()
                    .child(self.fx_editor(cx))
            );

        // ── Right column: Visualizations ────────────────────────────────────
        // Snapshot vis buffer once and share across all scopes.
        let samples: Vec<f32> = self.host.vis_buf.lock().ok()
            .map(|b| b.clone())
            .unwrap_or_default();
        let sr = self.host.sample_rate.load(std::sync::atomic::Ordering::Relaxed).max(44100);

        // Box height (px) for each scope.
        const H: f32 = 90.0;

        // ── Oscilloscope ─────────────────────────────────────────────────────
        let cols = 160usize;
        let osc_bars = if samples.is_empty() {
            vec![0.0f32; cols]
        } else {
            let n = samples.len();
            (0..cols).map(|c| {
                let a = c * n / cols;
                let b = ((c + 1) * n / cols).max(a + 1).min(n);
                samples[a..b].iter().map(|s| s.abs()).fold(0.0f32, f32::max)
            }).collect()
        };
        let osc_children: Vec<_> = osc_bars.iter().map(|&mag| {
            let h = (mag * H).clamp(1.0, H);
            div()
                .w(px(2.))
                .h(px(h))
                .bg(rgb(accent))
        }).collect();
        let oscilloscope_box = div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(section_title("Master output · oscilloscope"))
            .child(
                div()
                    .h(px(H))
                    .flex()
                    .items_center()
                    .px_2()
                    .gap_px()
                    .overflow_hidden()
                    .children(osc_children)
            );

        // ── Spectrum ─────────────────────────────────────────────────────────
        let spec_bars = daw_core::dsp::spectrum(&samples, sr, 64, 20.0, 20000.0, -60.0);
        let spec_children: Vec<_> = spec_bars.iter().map(|&level| {
            let h = (level * H).clamp(1.0, H);
            let color = if level >= 0.85 { RECORD } else if level >= 0.6 { METER_WARN } else { accent };
            div()
                .flex_1()
                .h(px(h))
                .bg(rgb(color))
        }).collect();
        let spectrum_box = div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(section_title("Master output · spectrum"))
            .child(
                div()
                    .h(px(H))
                    .flex()
                    .items_end()
                    .px_2()
                    .gap_px()
                    .overflow_hidden()
                    .children(spec_children)
            );

        // ── Filter / LFO ─────────────────────────────────────────────────────
        // Read selected track data for filter cutoff and modulators.
        let (cutoff01, modulators) = self.st.selected_track.as_ref()
            .and_then(|tid| self.st.project.tracks.iter().find(|t| &t.id == tid))
            .map(|t| (t.effects.cutoff, t.modulators.clone()))
            .unwrap_or((1.0, vec![]));

        // LP response curve: 64 log-spaced frequency columns.
        // Dim the accent by averaging its R/G/B channels with BG.
        let dim_accent = {
            let ar = ((accent >> 16) & 0xff) / 2 + ((BG >> 16) & 0xff) / 2;
            let ag = ((accent >> 8) & 0xff) / 2 + ((BG >> 8) & 0xff) / 2;
            let ab = (accent & 0xff) / 2 + (BG & 0xff) / 2;
            (ar << 16) | (ag << 8) | ab
        };
        let f_min = 20.0f32;
        let f_max = (sr as f32 / 2.0).min(20000.0);
        let lp_children: Vec<_> = (0..64usize).map(|i| {
            let f = f_min * (f_max / f_min).powf(i as f32 / 64.0);
            let mag = lp_magnitude(cutoff01, f, sr);
            let h = (mag * H).clamp(1.0, H);
            div()
                .flex_1()
                .h(px(h))
                .bg(rgb(dim_accent))
        }).collect();

        // LFO modulator bars.
        let t_secs = self.st.playhead_seconds() as f32;
        let lfo_section: gpui::Div = if modulators.is_empty() {
            div()
                .px_2()
                .py_1()
                .text_color(rgb(TEXT_DIM))
                .child("no LFOs")
        } else {
            let lfo_bars: Vec<_> = modulators.iter().map(|m| {
                let base = daw_core::sequencer::engine::mod_base01(m.dest, {
                    // Re-borrow the track for mod_base01
                    self.st.project.tracks.iter()
                        .find(|t| self.st.selected_track.as_deref() == Some(&t.id))
                        .unwrap()
                });
                let val = daw_core::sequencer::engine::modulated_value(m, base, t_secs);
                let bar_h = (val * (H - 4.0)).clamp(2.0, H - 4.0);
                let label = m.dest.label();
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .child(
                        div()
                            .w(px(14.))
                            .h(px(bar_h))
                            .bg(rgb(accent))
                    )
                    .child(
                        div()
                            .text_color(rgb(TEXT_DIM))
                            .child(label)
                    )
            }).collect();
            div()
                .flex()
                .items_end()
                .gap_2()
                .px_2()
                .h(px(H))
                .children(lfo_bars)
        };

        let filter_lfo_box = div()
            .flex()
            .flex_col()
            .child(section_title("Filter / LFO"))
            .child(
                div()
                    .h(px(H))
                    .flex()
                    .items_end()
                    .px_2()
                    .gap_px()
                    .overflow_hidden()
                    .children(lp_children)
            )
            .child(lfo_section);

        let right_col = div()
            .id("fx-right-scroll")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_scroll()
            .child(oscilloscope_box)
            .child(spectrum_box)
            .child(filter_lfo_box);

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
            .top(px(120. + if self.sf2_missing { 22. } else { 0. }))
            .right(px(0.))
            .bottom(px(0.))
            .bg(rgb(BG))
            .flex()
            .flex_col()
            .child(header)
            .child(body);

        Some(panel.into_any_element())
    }

    /// File dropdown with project actions and audio exports.
    pub(super) fn file_menu(&self, cx: &mut Context<Self>) -> gpui::Div {
        // Each row is built individually to attach distinct on_click closures.
        overlay_surface()
            .absolute()
            .left(px(0.))
            .top(px(24.))
            .min_w(px(160.))
            .flex()
            .flex_col()
            .bg(rgb(PANEL_ALT))
            .border_1()
            .border_color(rgb(BORDER))
            .rounded_md()
            .shadow_md()
            .overflow_hidden()
            .child(
                div().id("fm-new").px_3().py_1().cursor_pointer().text_color(rgb(TEXT))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child("New")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.undo.record("new project", &this.st.project);
                        this.st.project = daw_core::project::Project::default_project();
                        this.st.project_path = None;
                        this.st.file_revision = None;
                        this.st.saved_hash.clear();
                        this.st.soloed.clear();
                        this.st.selected_return = None;
                        this.st.arrange_scroll_x = 0.;
                        this.st.arrange_scroll_y = 0.;
                        if let Some(id) = this.st.project.tracks.first().map(|t| t.id.clone()) {
                            this.st.select_track(&id);
                        }
                        this.sync_midi_strip();
                        this.push_live();
                        this.st.open_menu = None;
                        this.st.set_status("new project (unsaved — Save As to name it)");
                        cx.notify();
                    }))
            )
            .child(
                div().id("fm-load").px_3().py_1().cursor_pointer().text_color(rgb(TEXT))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child("Load")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.open_dialog();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
            .child({
                let label = self.undo.undo_label().map(|l| format!("Undo {l}")).unwrap_or_else(|| "Undo".into());
                let fg = if self.undo.can_undo() { TEXT } else { TEXT_DIM };
                div().id("fm-undo").px_3().py_1().cursor_pointer().text_color(rgb(fg))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child(label)
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.undo();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            })
            .child({
                let label = self.undo.redo_label().map(|l| format!("Redo {l}")).unwrap_or_else(|| "Redo".into());
                let fg = if self.undo.can_redo() { TEXT } else { TEXT_DIM };
                div().id("fm-redo").px_3().py_1().cursor_pointer().text_color(rgb(fg))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child(label)
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.redo();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            })
            .child(
                div().id("fm-save").px_3().py_1().cursor_pointer().text_color(rgb(TEXT))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child("Save")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.save_project();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
            .child(
                div().id("fm-saveas").px_3().py_1().cursor_pointer().text_color(rgb(TEXT))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child("Save As…")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.save_as_dialog();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
            .child(
                div().id("fm-export").debug_selector(|| "fm-export".into()).px_3().py_1().cursor_pointer().text_color(rgb(TEXT))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child("Export WAV")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.export_audio(daw_core::audio::ExportFormat::Wav);
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
            .child(
                div().id("fm-export-mp3").debug_selector(|| "fm-export-mp3".into()).px_3().py_1().cursor_pointer().text_color(rgb(TEXT))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child("Export MP3")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.export_audio(daw_core::audio::ExportFormat::Mp3);
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
            .child(
                div().id("fm-import").px_3().py_1().cursor_pointer().text_color(rgb(TEXT))
                    .bg(rgb(PANEL_ALT)).whitespace_nowrap().child("Import Sample")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.import_sample_dialog();
                        this.st.open_menu = None;
                        cx.notify();
                    }))
            )
    }

    /// Settings dropdown panel — MIDI, Audio (read-only), Appearance.
    pub(super) fn settings_panel(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let midi_ports = self.midi_ports.clone();
        let cur_device = self.host.midi_device.clone();
        let cur_device_display = cur_device.clone().unwrap_or_else(|| "none".into());
        let midi_ch = self.host.midi_channel.load(std::sync::atomic::Ordering::Relaxed);
        let sr = self.host.sample_rate.load(std::sync::atomic::Ordering::Relaxed);
        let accent = self.accent();

        let port_rows = midi_ports.into_iter().enumerate().map(|(i, name)| {
            let cur = Some(&name) == cur_device.as_ref();
            let fg = if cur { accent } else { TEXT };
            div()
                .id(SharedString::from(format!("midip-{i}")))
                .px_2()
                .py_1()
                .cursor_pointer()
                .bg(rgb(PANEL))
                .text_color(rgb(fg))
                .whitespace_nowrap()
                .child(name.clone())
                .on_click(cx.listener(move |this, _e, _w, cx| {
                    this.host.set_midi_port(i);
                    cx.notify();
                }))
        });

        let swatch_row = div()
            .flex()
            .gap_2()
            .px_3()
            .py_2()
            .children(ACCENTS.iter().enumerate().map(|(i, &c)| {
                let selected = self.st.accent_idx == i;
                div()
                    .id(SharedString::from(format!("acc-{i}")))
                    .w(px(20.))
                    .h(px(20.))
                    .rounded_md()
                    .bg(rgb(c))
                    .cursor_pointer()
                    .border_2()
                    .border_color(rgb(if selected { TEXT } else { PANEL }))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.st.accent_idx = i;
                        cx.notify();
                    }))
            }));

        overlay_surface().id("settings-panel")
            .absolute()
            .left(px(80.))
            .top(px(24.)).bottom(px(8.))
            .w(px(320.))
            .flex()
            .flex_col()
            .bg(rgb(PANEL_ALT))
            .border_1()
            .border_color(rgb(BORDER))
            .rounded_md()
            .shadow_md()
            .overflow_y_scroll()
            // MIDI section
            .children(self.input_scan_running.then(|| div().px_3().py_1().text_color(rgb(TEXT_DIM)).child("Scanning audio inputs…")))
            .child(section_title("MIDI"))
            .child(
                div().px_3().py_1().text_color(rgb(TEXT_DIM))
                    .child(format!("Device: {}", cur_device_display))
            )
            .children(port_rows)
            // Live MIDI routes to the selected track's strip automatically
            // (no manual channel pick since the per-track mixer landed).
            .child(
                div().px_3().py_1().text_color(rgb(TEXT_DIM))
                    .child(format!("Plays strip: {} (selected track)", midi_ch))
            )
            // Audio section
            .child(section_title("Audio"))
            .child(
                div().px_3().py_1().text_color(rgb(TEXT_DIM))
                    .child(if sr > 0 { format!("Sample rate: {sr} Hz") } else { "Sample rate: —".into() })
            )
            .child(
                div().px_3().py_1().text_color(rgb(TEXT_DIM))
                    .child(format!("Soundfont: {}", self.sf2_path))
            )
            // Input (recording) section
            .child(section_title("Input"))
            .children({
                let rows: Vec<_> = self.input_devices.iter().enumerate().map(|(i, d)| {
                    let selected = self.config.input_device.as_deref() == Some(d.name.as_str());
                    let name = d.name.clone();
                    div()
                        .id(SharedString::from(format!("in-dev-{i}")))
                        .px_3().py_1().cursor_pointer()
                        .text_color(rgb(if selected { accent } else { TEXT_DIM }))
                        .whitespace_nowrap()
                        .child(format!("{} {} ({} ch)", if selected { "●" } else { "○" }, trunc(&name, 26), d.channels))
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            this.config.input_device = Some(name.clone());
                            let _ = this.config.save();
                            this.reopen_capture_if_armed();
                            cx.notify();
                        }))
                }).collect();
                rows
            })
            .child({
                use daw_core::audio::input::InputChannels;
                let label = match self.config.input_channels {
                    InputChannels::Mono(c) => format!("Mono — input {}", c + 1),
                    InputChannels::Stereo(l, r) => format!("Stereo — inputs {}+{}", l + 1, r + 1),
                };
                let max_ch = self.input_devices.iter()
                    .find(|d| self.config.input_device.as_deref() == Some(d.name.as_str()))
                    .map(|d| d.channels).unwrap_or(2);
                div().flex().items_center().gap_2().px_3().py_1()
                    .child(div().text_color(rgb(TEXT_DIM)).child("Channel:"))
                    .child(div().id("in-ch-down").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(PANEL)).text_color(rgb(TEXT)).child("−")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            use daw_core::audio::input::InputChannels;
                            this.config.input_channels = match this.config.input_channels {
                                InputChannels::Mono(c) => InputChannels::Mono(c.saturating_sub(1)),
                                InputChannels::Stereo(l, _) => {
                                    let l = l.saturating_sub(1);
                                    InputChannels::Stereo(l, l + 1)
                                }
                            };
                            let _ = this.config.save();
                            this.reopen_capture_if_armed();
                            cx.notify();
                        })))
                    .child(div().min_w(px(140.)).text_color(rgb(TEXT)).child(label))
                    .child(div().id("in-ch-up").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(PANEL)).text_color(rgb(TEXT)).child("+")
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            use daw_core::audio::input::InputChannels;
                            let max = max_ch.max(1) - 1;
                            this.config.input_channels = match this.config.input_channels {
                                InputChannels::Mono(c) => InputChannels::Mono((c + 1).min(max)),
                                InputChannels::Stereo(l, _) => {
                                    let l = (l + 1).min(max.saturating_sub(1));
                                    InputChannels::Stereo(l, l + 1)
                                }
                            };
                            let _ = this.config.save();
                            this.reopen_capture_if_armed();
                            cx.notify();
                        })))
                    .child(div().id("in-ch-mode").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(PANEL)).text_color(rgb(TEXT)).child("mono/stereo")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            use daw_core::audio::input::InputChannels;
                            this.config.input_channels = match this.config.input_channels {
                                InputChannels::Mono(c) => InputChannels::Stereo(c, c + 1),
                                InputChannels::Stereo(l, _) => InputChannels::Mono(l),
                            };
                            let _ = this.config.save();
                            this.reopen_capture_if_armed();
                            cx.notify();
                        })))
            })
            .child(
                div().flex().items_center().gap_2().px_3().py_1()
                    .child(div().text_color(rgb(TEXT_DIM)).child("Count-in:"))
                    .child(div().id("ci-down").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(PANEL)).text_color(rgb(TEXT)).child("−")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            this.config.count_in_bars = this.config.count_in_bars.saturating_sub(1);
                            let _ = this.config.save();
                            cx.notify();
                        })))
                    .child(div().min_w(px(56.)).text_color(rgb(TEXT)).child(format!("{} bars", self.config.count_in_bars)))
                    .child(div().id("ci-up").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(PANEL)).text_color(rgb(TEXT)).child("+")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            this.config.count_in_bars = (this.config.count_in_bars + 1).min(4);
                            let _ = this.config.save();
                            cx.notify();
                        })))
            )
            .child(
                div().flex().items_center().gap_2().px_3().py_1()
                    .child(div().text_color(rgb(TEXT_DIM)).child("Latency trim:"))
                    .child(div().id("lt-down").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(PANEL)).text_color(rgb(TEXT)).child("−")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            this.config.latency_trim_ms = (this.config.latency_trim_ms - 2.0).max(0.0);
                            let _ = this.config.save();
                            cx.notify();
                        })))
                    .child(div().min_w(px(64.)).text_color(rgb(TEXT)).child(format!("{:.0} ms", self.config.latency_trim_ms)))
                    .child(div().id("lt-up").px_2().py_1().rounded_md().cursor_pointer()
                        .bg(rgb(PANEL)).text_color(rgb(TEXT)).child("+")
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            this.config.latency_trim_ms = (this.config.latency_trim_ms + 2.0).min(200.0);
                            let _ = this.config.save();
                            cx.notify();
                        })))
            )
            // Appearance section
            .child(section_title("Appearance"))
            .child(swatch_row)
    }
}

impl DawView {
    pub(super) fn close_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.close_confirmation { return None; }
        Some(overlay_surface().id("close-confirmation").absolute().inset_0()
            .flex().items_center().justify_center().bg(gpui::rgba(0x00000099))
            .child(overlay_surface().w(px(440.)).p_4().rounded_md().bg(rgb(PANEL)).text_color(rgb(TEXT))
                .child(div().text_lg().child("Save changes before closing?"))
                .child(div().py_2().child(self.project_name()))
                .child(div().py_2().text_color(rgb(TEXT_DIM)).child(self.st.status.clone()))
                .child(div().flex().gap_2()
                    .child(cell_btn("close-cancel", "Cancel", false, PANEL_ALT).on_click(cx.listener(|this, _, _, cx| {
                        this.close_confirmation = false; this.close_after_save = false; cx.notify();
                    })))
                    .child(cell_btn("close-discard", "Discard & close", false, RECORD).on_click(cx.listener(|_, _, w, _| w.remove_window())))
                    .child(cell_btn("close-save", "Save & close", true, self.accent()).on_click(cx.listener(|this, _, _, cx| {
                        this.close_after_save = true;
                        this.save_project();
                        cx.notify();
                    })))))
            .into_any_element())
    }
}

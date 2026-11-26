use super::*;

pub(super) const EDITOR_TABS_H: f32 = 32.;
const PIANO_SCROLL_INSET: f32 = 8.;
const TICKS_PER_COL: u32 = daw_core::musical_time::TICKS_PER_BEAT / 4;

#[derive(Clone)]
struct PianoTimeDrag;

impl DawView {
    pub(super) fn editor_tabs(
        &self,
        compact_mixer: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme();
        let active = self.st.active_dock;
        div()
            .id("workspace-tabs")
            .debug_selector(|| "editor-tabs".into())
            .h(px(EDITOR_TABS_H))
            .flex_shrink_0()
            .overflow_x_scroll()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .text_size(px(12.))
            .line_height(px(16.))
            .bg(rgb(theme.panel))
            .border_b_1()
            .border_color(rgb(theme.border))
            .children(DockTab::ALL.into_iter().map(|tab| {
                let selected = tab == active && !compact_mixer;
                div()
                    .id(SharedString::from(tab.label()))
                    .debug_selector(move || format!("dock-tab-{}", tab.label()))
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(if selected { theme.panel_alt } else { theme.panel }))
                    .text_color(rgb(if selected { theme.text } else { theme.text_dim }))
                    .child(tab.label())
                    .on_click(cx.listener(move |this, _ev, _w, cx| {
                        this.st.active_dock = tab;
                        if tab != DockTab::PianoRoll {
                            this.piano_step_input = None;
                        }
                        if compact_mixer {
                            this.st.mixer_open = false;
                        }
                        this.st.arrange_full = false;
                        cx.notify();
                    }))
            }))
    }

    pub(super) fn editor_dock(&mut self, height: f32, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let active = self.st.active_dock;
        let tabs = self.editor_tabs(false, cx);

        let picker = self.pattern_picker(cx);
        let body = match active {
            DockTab::Sequencer => self.sequencer_editor(cx),
            DockTab::PianoRoll => self.piano_roll_editor(cx),
            DockTab::Fx => self.effects_panel(cx, false),
            DockTab::Code => self.code_editor(cx),
        };

        div()
            .debug_selector(|| "editor-dock".into())
            .h(px(height))
            .flex()
            .flex_col()
            .bg(rgb(theme.bg))
            .border_t_1()
            .border_color(rgb(theme.border))
            .flex_shrink_0()
            .min_h_0()
            .child(tabs)
            .child(picker)
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
    }

    pub(super) fn sampler_controls(&self, track: &daw_core::project::Track, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let theme = self.theme();
        let daw_core::project::Instrument::Sampler { pads } = &track.instrument else { return None };
        Some(div().flex().flex_col().gap_1().child(theme.section_title("Sampler dynamics · soft 1–63 / hard 64–127"))
            .children(pads.iter().map(|pad| {
                let pitch = pad.pitch;
                let (base, soft, hard, reset) = (track.id.clone(), track.id.clone(), track.id.clone(), track.id.clone());
                let (gate, gate_tid) = (pad.gate, track.id.clone());
                div().flex().items_center().gap_1().flex_shrink_0()
                    .child(div().w(px(150.)).text_color(rgb(theme.text_dim)).child(format!("{} · {} takes", note_name(pitch), pad.variants.len())))
                    .child(theme.cell_btn(format!("take-base-{pitch}"), "Base", false, theme.panel_alt).on_click(cx.listener(move |this, _, _, _| this.load_pad_dialog(base.clone(), pitch))))
                    .child(theme.cell_btn(format!("take-soft-{pitch}"), "+ Soft", false, theme.panel_alt).on_click(cx.listener(move |this, _, _, _| this.load_pad_variant_dialog(soft.clone(), pitch, 1, 63))))
                    .child(theme.cell_btn(format!("take-hard-{pitch}"), "+ Hard", false, theme.panel_alt).on_click(cx.listener(move |this, _, _, _| this.load_pad_variant_dialog(hard.clone(), pitch, 64, 127))))
                    .child(theme.cell_btn(format!("pad-gate-{pitch}"), if gate { "Gate: note length" } else { "Gate: one-shot" }, gate, self.accent())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let before = this.st.project.clone();
                            if this.st.project.set_sampler_gate(&gate_tid, pitch, !gate).is_ok() {
                                this.undo.record("sampler gate", &before); this.push_live(); cx.notify();
                            }
                        })))
                    .child(theme.cell_btn(format!("take-reset-{pitch}"), "Clear takes", false, theme.panel_alt).on_click(cx.listener(move |this, _, _, cx| {
                        let before = this.st.project.clone();
                        if this.st.project.set_sampler_variants(&reset, pitch, Vec::new()).is_ok() {
                            this.undo.record("clear sampler takes", &before); this.push_live(); cx.notify();
                        }
                    })))
            })))
    }

    /// Step-grid editor for the selected pattern (when it's a step pattern).
    pub(super) fn sequencer_editor(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let accent = self.accent();
        let Some(pid) = self.st.selected_pattern_id() else {
            return theme.dock_hint("Select a track with a step pattern.");
        };
        let pat = self.st.project.patterns.iter().find(|p| p.id() == pid);
        let Some(daw_core::project::Pattern::Step(sp)) = pat else {
            return theme.dock_hint("This pattern isn't a step pattern — use the Piano Roll tab.");
        };

        let steps = sp.steps as usize;
        let gate = sp.gate;
        // Steps are 16th notes (30 ticks); the play column wraps over the
        // whole pattern length (steps × 30).
        let ticks_per_step = daw_core::sequencer::TICKS_PER_BEAT / 4;
        let pat_len = (steps as u32 * ticks_per_step).max(1);
        let play_step = ((self.st.playhead() % pat_len) / ticks_per_step) as usize;

        // Collect per-row data into owned values to avoid holding a borrow on sp
        // while building the children (which need cx).
        let row_data: Vec<(String, u8, Vec<u8>)> = sp.rows.iter().map(|row| {
            let name = row.name.clone();
            let pitch = row.pitch;
            let step_flags: Vec<u8> = (0..steps).map(|si| row.step_velocity(si).unwrap_or(0)).collect();
            (name, pitch, step_flags)
        }).collect();

        // Build toolbar — captures clones of pid, not sp.
        let pid_t = pid.clone();
        let toolbar = {
            let (p1, p2, p3, p4, p5) = (pid.clone(), pid.clone(), pid.clone(), pid.clone(), pid.clone());
            div().flex().items_center().gap_2().pb_1().text_color(rgb(theme.text_dim))
                .child(div().child(format!("{} steps", steps)))
                .child(theme.cell_btn("seq-steps-down", "−4", false, theme.panel_alt).on_click(cx.listener(move |this, _e, _w, cx| { let before = this.st.project.clone(); if this.st.project.resize_steps(&p1, -4) { this.undo.record("steps", &before); this.push_live(); } cx.notify(); })))
                .child(theme.cell_btn("seq-steps-up", "+4", false, theme.panel_alt).on_click(cx.listener(move |this, _e, _w, cx| { let before = this.st.project.clone(); if this.st.project.resize_steps(&p2, 4) { this.undo.record("steps", &before); this.push_live(); } cx.notify(); })))
                .child(theme.cell_btn("seq-addrow", "+ Row", false, theme.panel_alt).on_click(cx.listener(move |this, _e, _w, cx| { this.undo.record("add row", &this.st.project); this.st.project.add_step_row(&p3); this.push_live(); cx.notify(); })))
                .child(div().child(format!("gate {:.2}", gate)))
                .child(theme.cell_btn("seq-gate-down", "−", false, theme.panel_alt).on_click(cx.listener(move |this, _e, _w, cx| {
                    let g = (this.cur_gate(&p4) - 0.05).max(0.1); this.undo.record("gate", &this.st.project); this.st.project.set_gate(&p4, g); this.push_live(); cx.notify();
                })))
                .child(theme.cell_btn("seq-gate-up", "+", false, theme.panel_alt).on_click(cx.listener(move |this, _e, _w, cx| {
                    let g = (this.cur_gate(&p5) + 0.05).min(2.0); this.undo.record("gate", &this.st.project); this.st.project.set_gate(&p5, g); this.push_live(); cx.notify();
                })))
        };
        let _ = pid_t;

        // When the selected track is a Sampler, expose a per-row "load sample"
        // button (pitch -> wav path) so a kit can be built entirely in the GUI.
        let (sampler_tid, pad_paths): (Option<String>, std::collections::HashMap<u8, String>) = {
            let track = self.st.selected_track.as_ref()
                .and_then(|tid| self.st.project.tracks.iter().find(|t| &t.id == tid));
            match track.map(|t| (t.id.clone(), &t.instrument)) {
                Some((id, daw_core::project::Instrument::Sampler { pads })) =>
                    (Some(id), pads.iter().map(|p| (p.pitch, p.path.clone())).collect()),
                _ => (None, std::collections::HashMap::new()),
            }
        };

        let rows = row_data.into_iter().enumerate().map(|(ri, (row_name, row_pitch, step_flags))| {
            let pitch_tag = format!("p{row_pitch}");
            let label = if row_name == pitch_tag {
                pitch_tag.clone()
            } else {
                format!("{} {}", trunc(&row_name, 8), pitch_tag)
            };
            // Sampler-pad load button for this row's pitch (sampler tracks only).
            let pad_btn = sampler_tid.clone().map(|tid| {
                let loaded = pad_paths.get(&row_pitch).cloned();
                let lbl = loaded.as_deref()
                    .and_then(|p| std::path::Path::new(p).file_name())
                    .map(|s| trunc(&s.to_string_lossy(), 12))
                    .unwrap_or_else(|| "+ wav".to_string());
                theme.cell_btn(format!("pad-{ri}"), lbl, loaded.is_some(), accent)
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.load_pad_dialog(tid.clone(), row_pitch);
                        cx.notify();
                    }))
            });
            let cells: Vec<_> = step_flags.iter().enumerate().map(|(si, &velocity)| {
                let on = velocity > 0;
                let is_play_col = si == play_step;
                let beat = si % 4 == 0;
                // Off cells are clearly visible; beat starts read brighter.
                let bg = if on { accent }
                         else if is_play_col { theme.step_playing }
                         else if beat { theme.step_beat }
                         else { theme.panel_alt };
                let pid_c = pid.clone();
                div()
                    .id(SharedString::from(format!("step-{pid}-{ri}-{si}")))
                    .w(px(STEP_W))
                    .h(px(STEP_W))
                    .flex_shrink_0()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(bg))
                    .opacity(if on { 0.4 + 0.6 * velocity as f32 / 127. } else { 1. })
                    .text_color(rgb(if on { theme::contrast_text(accent) } else { theme.text_dim })).text_size(px(10.))
                    .flex().items_center().justify_center()
                    .child(if on { velocity.to_string() } else { String::new() })
                    .tooltip(move |_, cx| cx.new(|_| TextTooltip { theme, text: SharedString::from(format!("Velocity {velocity} · Click: toggle · Shift+click: louder · Alt+click: softer")) }).into())
                    .border_1()
                    .border_color(rgb(theme.border))
                    // Drum-machine grouping: extra gap before each 4-step block.
                    .when(beat && si > 0, |d| d.ml(px(6.)))
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _w, cx| {
                        this.undo.record("step dynamics", &this.st.project);
                        let modifiers = e.modifiers();
                        if modifiers.shift {
                            this.st.project.set_step_velocity(&pid_c, ri, si, if velocity == 0 { 96 } else { velocity.saturating_add(16).min(127) });
                        } else if modifiers.alt {
                            this.st.project.set_step_velocity(&pid_c, ri, si, if velocity == 0 { 32 } else { velocity.saturating_sub(16).max(2) });
                        } else { this.st.project.toggle_step(&pid_c, ri, si); }
                        this.push_live();
                        cx.notify();
                    }))
            }).collect();
            div()
                .flex()
                .items_center()
                .gap_1()
                .h(px(STEP_W + 4.0)).flex_shrink_0()
                .child(if self.st.renaming == Some(RenameTarget::Row(pid.clone(), ri)) {
                    div().w(px(96.)).flex_shrink_0().child(self.rename_editor(cx)).into_any_element()
                } else {
                    let (pid_r, name_r) = (pid.clone(), row_name.clone());
                    div()
                        .id(SharedString::from(format!("rowlbl-{ri}")))
                        .w(px(96.))
                        .flex_shrink_0()
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .text_color(rgb(theme.text_dim))
                        .child(label)
                        .on_click(cx.listener(move |this, e: &gpui::ClickEvent, w, cx| {
                            if e.click_count() >= 2 {
                                this.start_rename(RenameTarget::Row(pid_r.clone(), ri), name_r.clone());
                                this.rename_focus.clone().focus(w);
                                cx.notify();
                            }
                        }))
                        .into_any_element()
                })
                .children(pad_btn)
                .child(theme.cell_btn(format!("rp-down-{ri}"), "−", false, theme.panel_alt).on_click(cx.listener({
                    let pid = pid.clone();
                    move |this, _e, _w, cx| { this.undo.record("row pitch", &this.st.project); this.st.project.nudge_row_pitch(&pid, ri, -1); this.push_live(); cx.notify(); }
                })))
                .child(theme.cell_btn(format!("rp-up-{ri}"), "+", false, theme.panel_alt).on_click(cx.listener({
                    let pid = pid.clone();
                    move |this, _e, _w, cx| { this.undo.record("row pitch", &this.st.project); this.st.project.nudge_row_pitch(&pid, ri, 1); this.push_live(); cx.notify(); }
                })))
                .children(cells)
        });

        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(toolbar.flex_shrink_0())
            .child(div().id("seq-scroll").flex_1().min_h_0().overflow_scroll().child(div().flex().flex_col().gap_1().p_2().children(rows)))
    }

    /// Read the current gate value for a step pattern by id (avoids stale captures).
    pub(super) fn cur_gate(&self, pid: &str) -> f32 {
        self.st.project.patterns.iter().find_map(|p| match p {
            daw_core::project::Pattern::Step(sp) if sp.id == pid => Some(sp.gate),
            _ => None,
        }).unwrap_or(0.5)
    }

    fn toggle_piano_step_input(&mut self) -> bool {
        let Some((_, len)) = self.open_pr_pattern() else { return false };
        let Some(pid) = self.st.selected_pattern_id() else { return false };
        if self.midi_recording
            || self.selected_track_is_audio()
            || !self.st.track_pattern_ids().contains(&pid)
        {
            return false;
        }
        self.piano_step_input = if self.piano_step_tick(&pid).is_some() {
            None
        } else {
            let first_col = (-f32::from(self.piano_scroll.offset().x) / PR_COL_W) as u32;
            Some((pid, first_col.min(len.saturating_sub(1) / TICKS_PER_COL) * TICKS_PER_COL))
        };
        true
    }

    fn piano_step_tick(&self, pid: &str) -> Option<u32> {
        self.piano_step_input.as_ref().filter(|(id, _)| id == pid).map(|(_, tick)| *tick)
    }

    fn place_piano_step_cursor(&mut self, pid: &str, col: u32) -> bool {
        let Some((id, tick)) = &mut self.piano_step_input else { return false };
        if id != pid { return false; }
        *tick = col * TICKS_PER_COL;
        true
    }

    pub(super) fn handle_piano_step_key(&mut self, event: &KeyDownEvent) -> bool {
        let stroke = &event.keystroke;
        if stroke.modifiers.modified() { return false; }
        if stroke.key == "l" {
            return !event.is_held && self.toggle_piano_step_input();
        }
        let Some(pid) = self.st.selected_pattern_id() else { return false };
        let Some(tick) = self.piano_step_tick(&pid) else { return false };
        if stroke.key == "escape" {
            self.piano_step_input = None;
            return true;
        }
        let Some((idx, len)) = self.open_pr_pattern() else { return false };
        let tick = tick.min(len);
        let next_tick = match stroke.key.as_str() {
            "left" => tick.saturating_sub(1) / TICKS_PER_COL * TICKS_PER_COL,
            "right" => (tick + TICKS_PER_COL).min(len),
            key => {
                let mut chars = key.chars();
                let Some(key) = chars.next().filter(|_| chars.next().is_none()) else { return false };
                let Some(pitch) = daw_io::midi::computer_key_pitch(key) else { return false };
                if event.is_held { return true; }
                let Some(track_id) = self.st.selected_track.clone() else { return false };
                if tick < len {
                    let daw_core::project::Pattern::PianoRoll(pattern) = &self.st.project.patterns[idx] else { return false };
                    if !pattern.notes.iter().any(|note| note.pitch == pitch && note.tick == tick) {
                        let duration = TICKS_PER_COL.min(len - tick);
                        self.edit("piano step input", |project| { project.toggle_note(&pid, pitch, tick, duration); });
                    }
                }
                let bpm = self.st.project.project.bpm;
                self.audition_piano_note(track_id, pitch, 80, (60_000. / bpm / 4.).round() as u32);
                (tick + TICKS_PER_COL).min(len)
            }
        };
        self.piano_step_input = Some((pid, next_tick));
        self.scroll_to_piano_step(next_tick.min(len.saturating_sub(1)) / TICKS_PER_COL);
        true
    }

    fn scroll_to_piano_step(&self, col: u32) {
        let offset = self.piano_scroll.offset();
        let width = f32::from(self.piano_scroll.bounds().size.width) - PR_KEYBOARD_GUTTER - 2. * PIANO_SCROLL_INSET;
        let left = -f32::from(offset.x);
        let x = col as f32 * PR_COL_W;
        let wanted = left.clamp(x + PR_COL_W - width.max(PR_COL_W), x);
        self.piano_scroll.set_offset(gpui::point(
            px(-wanted).clamp(-self.piano_scroll.max_offset().width, px(0.)), offset.y,
        ));
    }

    fn audition_piano_note(&self, track_id: String, pitch: u8, velocity: u8, dur_ms: u32) {
        let note = daw_core::audition::AuditionNote { pitch, velocity, start_ms: 0, dur_ms };
        let _ = self.send_command(SequencerCommand::Audition { track_id, notes: vec![note] });
    }

    /// Pitch×time grid editor for the selected pattern (when it's piano-roll).
    pub(super) fn piano_roll_editor(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let accent = self.accent();
        let Some(pid) = self.st.selected_pattern_id() else {
            return theme.dock_hint("Select a track with a piano-roll pattern.");
        };
        let pat = self.st.project.patterns.iter().find(|p| p.id() == pid);
        let Some(daw_core::project::Pattern::PianoRoll(pr)) = pat else {
            return theme.dock_hint("This pattern isn't a piano-roll pattern — use the Sequencer tab.");
        };

        let pat_len = pr.ticks_per_bar * pr.bars.max(1) as u32;
        let cols = pat_len.div_ceil(TICKS_PER_COL).max(1) as usize;
        let grid_width = cols as f32 * PR_COL_W;
        let document_width = PR_KEYBOARD_GUTTER + grid_width;

        // Pitch window: span the pattern's notes with generous padding so notes
        // can be added above/below; the grid scrolls vertically through it.
        // Default ~C3..C6 when empty; always at least ~3 octaves tall.
        let (mut lo, mut hi) = (48u8, 84u8);
        if let (Some(min), Some(max)) =
            (pr.notes.iter().map(|n| n.pitch).min(), pr.notes.iter().map(|n| n.pitch).max())
        {
            lo = min.saturating_sub(8);
            hi = (max as u16 + 8).min(127) as u8;
        }
        if hi.saturating_sub(lo) < 36 {
            hi = lo.saturating_add(36).min(127);
            lo = hi.saturating_sub(36);
        }
        let row_h = PR_ROW_H + 1.0;
        let view_h = f32::from(self.piano_scroll.bounds().size.height);
        if self.piano_scrolled_pattern.as_deref() != Some(pid.as_str()) {
            let pitch = pr.notes.iter().min_by_key(|note| note.tick).map_or(60, |note| note.pitch);
            let view_h = if view_h > 1. { view_h } else { (self.st.dock_h - 100.).max(row_h) };
            let offset = (f32::from(hi - pitch.clamp(lo, hi)) * row_h - view_h * 0.4).max(0.);
            self.piano_scroll.set_offset(gpui::point(px(0.), px(-offset)));
            self.piano_scroll_grab = None;
            self.piano_scrolled_pattern = Some(pid.clone());
        }
        let scroll_x = -f32::from(self.piano_scroll.offset().x);
        let view_width = f32::from(self.piano_scroll.bounds().size.width);
        let visible_cols = piano_visible_columns(scroll_x, if view_width > 1. { view_width } else { 1280. }, cols);
        let cells = std::rc::Rc::new(piano_cells(
            &pr.notes, TICKS_PER_COL, visible_cols.start as u32..visible_cols.end as u32,
        ));
        let pressed_keys = self.piano_pressed_keys();
        let track = self.st.selected_track.as_deref();
        let play_col = ((self.st.playhead() % pat_len.max(1)) / TICKS_PER_COL) as usize;
        let step_tick = self.piano_step_tick(&pid);
        let step_col = step_tick.filter(|&tick| tick < pat_len).map(|tick| (tick / TICKS_PER_COL) as usize);

        // Slim toolbar: pattern length in bars (notes auto-grow it too).
        let pr_bars = pr.bars;
        let bar_width = (pr.ticks_per_bar as f32 * PR_COL_W / TICKS_PER_COL as f32).max(1.);
        let first_bar = (scroll_x / bar_width).floor() as u32 + 1;
        let visible_width = (view_width - PR_KEYBOARD_GUTTER - 2. * PIANO_SCROLL_INSET).max(0.);
        let last_bar = (((scroll_x + visible_width) / bar_width).ceil() as u32)
            .clamp(first_bar.min(u32::from(pr_bars)), u32::from(pr_bars));
        let legato_on = pr.legato;
        let (pb1, pb2, pbl) = (pid.clone(), pid.clone(), pid.clone());
        let toolbar = div().flex().items_center().gap_2().pb_1().px_2().text_color(rgb(theme.text_dim))
            .child(div().child(format!("Bars {}–{last_bar} / {pr_bars}", first_bar.min(last_bar))))
            .child(theme.cell_btn("pr-bars-down", "−Bar", false, theme.panel_alt).on_click(cx.listener(move |this, _e, _w, cx| {
                this.undo.record("pr bars", &this.st.project);
                this.st.project.resize_pr_bars(&pb1, -1);
                this.push_live();
                cx.notify();
            })))
            .child(theme.cell_btn("pr-bars-up", "+Bar", false, theme.panel_alt).on_click(cx.listener(move |this, _e, _w, cx| {
                this.undo.record("pr bars", &this.st.project);
                this.st.project.resize_pr_bars(&pb2, 1);
                this.push_live();
                cx.notify();
            })))
            .child(theme.cell_btn("pr-legato", "Legato", legato_on, accent).on_click(cx.listener(move |this, _e, _w, cx| {
                this.undo.record("legato", &this.st.project);
                this.st.project.set_legato(&pbl, !legato_on);
                this.push_live();
                cx.notify();
            })))
            .child(theme.cell_btn_with_help(
                "pr-computer-piano", "Laptop keys", step_tick.is_some(), accent,
                "L toggles laptop-key step input. Click the grid or use ←/→ to move. Esc exits.".into(),
            ).on_click(cx.listener(|this, _, window, cx| {
                if this.toggle_piano_step_input() {
                    this.root_focus.focus(window);
                    cx.notify();
                }
            })))
            .children(step_tick.map(|tick| {
                if tick >= pat_len { "End of pattern".into() }
                else { format!("Step {}", tick / TICKS_PER_COL + 1) }
            }));

        // Only the pitch rows inside the scrolled viewport are built (plus a
        // margin); spacers keep the document height, so scrolling is unchanged.
        // Building all ~50 rows × visible columns cost most of a frame.
        let total_rows = (hi - lo) as usize + 1;
        let (first_row, row_count) = piano_visible_rows(-f32::from(self.piano_scroll.offset().y), view_h, row_h, total_rows);
        let spacer_above = div().h(px(first_row as f32 * row_h)).flex_shrink_0();
        let spacer_below = div().h(px((total_rows - first_row - row_count) as f32 * row_h)).flex_shrink_0();
        // High pitch at top down to low.
        let rows = (lo..=hi).rev().skip(first_row).take(row_count).map(|pitch| {
            let is_black = black_key(pitch);
            let row_bg = if is_black { theme.bg } else { theme.panel };
            let background_cells = cells.clone();
            let columns = visible_cols.clone();
            let background = gpui::canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    for col in columns.clone() {
                        if background_cells.contains_key(&(pitch, col as u32)) {
                            continue;
                        }
                        let origin = bounds.origin + gpui::point(px(col as f32 * PR_COL_W + 1.), px(1.));
                        let rect = Bounds::new(origin, size(px(PR_COL_W - 2.), px(PR_ROW_H - 2.)));
                        let color = if Some(col) == step_col { theme.step_playing }
                            else if col == play_col { theme.panel_alt } else { row_bg };
                        window.paint_quad(gpui::fill(rect, rgb(color)));
                    }
                },
            ).absolute().inset_0();

            let mut note_cells = Vec::new();
            for col in visible_cols.clone() {
                let Some(info) = cells.get(&(pitch, col as u32)).copied() else { continue; };
                let is_start = info.start.is_some();
                let note = div()
                    .id(("piano-cell", pitch as usize * 1_000_000 + col))
                    .absolute().left(px(col as f32 * PR_COL_W)).top_0()
                    .w(px(PR_COL_W)).h(px(PR_ROW_H))
                    .bg(rgb(if is_start { accent } else { 0x9a6a2a }))
                    .border_1().border_color(rgb(theme.bg))
                    .opacity(if is_start { 0.4 + 0.6 * info.velocity as f32 / 127. } else { 1. })
                    .cursor_pointer()
                    .tooltip(move |_, cx| cx.new(|_| TextTooltip {
                        theme,
                        text: format!("{} · velocity {} · Shift+click a note start: louder · Alt+click: softer", note_name(pitch), info.velocity).into(),
                    }).into());
                let Some(note_tick) = info.start else {
                    note_cells.push(note);
                    continue;
                };
                let pattern = pid.clone();
                note_cells.push(note
                    .on_drag(NoteStretch { pid: pid.clone(), pitch, tick: note_tick }, |_, _, _, cx| cx.new(|_| EmptyDrag))
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        cx.stop_propagation();
                        let modifiers = event.modifiers();
                        if !modifiers.modified() && this.place_piano_step_cursor(&pattern, col as u32) {
                            cx.notify();
                            return;
                        }
                        this.undo.record("note dynamics", &this.st.project);
                        if modifiers.shift {
                            this.st.project.set_note_velocity(&pattern, pitch, note_tick, info.velocity.saturating_add(16).min(127));
                        } else if modifiers.alt {
                            this.st.project.set_note_velocity(&pattern, pitch, note_tick, info.velocity.saturating_sub(16).max(1));
                        } else {
                            this.st.project.toggle_note(&pattern, pitch, note_tick, TICKS_PER_COL);
                        }
                        this.push_live();
                        cx.notify();
                    })));
            }

            let pattern = pid.clone();
            let row_cells = cells.clone();
            let drop_pattern = pid.clone();
            let grid = div()
                .id(("piano-grid", usize::from(pitch)))
                .relative().ml(px(PR_KEYBOARD_GUTTER)).w(px(grid_width)).h(px(PR_ROW_H)).flex_shrink_0()
                .cursor_pointer()
                .tooltip(move |_, cx| cx.new(|_| TextTooltip {
                    theme,
                    text: format!("{} · Click to add a note", note_name(pitch)).into(),
                }).into())
                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                    let Some(col) = this.piano_column_at(event.position().x, cols) else { return; };
                    if !event.modifiers().modified() && this.place_piano_step_cursor(&pattern, col) {
                        cx.notify();
                        return;
                    }
                    if row_cells.contains_key(&(pitch, col)) { return; }
                    this.undo.record("toggle note", &this.st.project);
                    this.st.project.toggle_note(&pattern, pitch, col * TICKS_PER_COL, TICKS_PER_COL);
                    this.push_live();
                    cx.notify();
                }))
                .on_drop(cx.listener(move |this, drag: &NoteStretch, window, cx| {
                    if drag.pid != drop_pattern || drag.pitch != pitch { return; }
                    let Some(col) = this.piano_column_at(window.mouse_position().x, cols) else { return; };
                    let tick = col * TICKS_PER_COL;
                    if tick < drag.tick { return; }
                    this.undo.record("stretch note", &this.st.project);
                    this.st.project.set_note_duration(&drag.pid, pitch, drag.tick, tick + TICKS_PER_COL - drag.tick);
                    this.push_live();
                    cx.notify();
                }))
                .child(background)
                .children(note_cells);
            div()
                .id(("piano-row", usize::from(pitch)))
                .relative()
                .flex()
                .items_center()
                .h(px(row_h)).flex_shrink_0()
                .child(grid)
                .child(self.piano_keyboard_row(cx, pitch, pressed_keys, track, scroll_x))
        });

        // Keep the viewport clipped while the scroll handle translates the
        // document. Handle wheel input here so Shift can select the time axis.
        // Wrapped in a plain div so the return type stays `gpui::Div` (a `.id()`
        // chain is `Stateful<Div>`) — matching the other editor_dock match arms.
        div().size_full().min_h_0().flex().flex_col().child(toolbar.flex_shrink_0()).child(
            div()
                .id(SharedString::from(format!("pr-scroll-{pid}")))
                .debug_selector(|| "piano-scroll-viewport".into())
                .track_scroll(&self.piano_scroll)
                .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                    let delta = event.delta.pixel_delta(px(40.));
                    let horizontal = if event.shift && delta.y.abs() > delta.x.abs() {
                        delta.y
                    } else {
                        delta.x
                    };
                    let vertical = if event.shift { px(0.) } else { delta.y };
                    let offset = this.piano_scroll.offset();
                    let max = this.piano_scroll.max_offset();
                    let next = gpui::point(
                        (offset.x + horizontal).clamp(-max.width, px(0.)),
                        (offset.y + vertical).clamp(-max.height, px(0.)),
                    );
                    if next != offset {
                        this.piano_scroll.set_offset(next);
                        cx.notify();
                    }
                    cx.stop_propagation();
                }))
                .flex_1().min_h_0().min_w_0()
                .overflow_hidden()
                .flex()
                .flex_col()
                .p_2()
                .child(div().w(px(document_width)).flex_shrink_0().flex().flex_col()
                    .child(spacer_above).children(rows).child(spacer_below)),
        ).child(self.piano_time_scrollbar(cx))
    }

    fn piano_time_scrollbar(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme();
        use gpui::MouseButton::Left;

        let handle = self.piano_scroll.clone();
        let accent = self.accent();
        div().id("piano-time-scrollbar")
            .debug_selector(|| "piano-time-scrollbar".into())
            .mx(px(PIANO_SCROLL_INSET)).my_1().h(px(14.)).flex_shrink_0()
            .rounded_md().bg(rgb(theme.panel)).cursor_pointer()
            .tooltip(move |_, cx| cx.new(|_| TextTooltip {
                theme,
                text: "Drag or Shift+wheel to scroll through bars".into(),
            }).into())
            .child(gpui::canvas(|_, _, _| (), move |bounds, _, window, _| {
                let width = f32::from(bounds.size.width);
                let max = f32::from(handle.max_offset().width);
                if max <= 0. || width <= 0. { return; }
                let thumb = scrollbar_thumb(width, max);
                let fraction = (-f32::from(handle.offset().x) / max).clamp(0., 1.);
                let origin = bounds.origin + gpui::point(px(fraction * (width - thumb)), px(3.));
                window.paint_quad(gpui::fill(
                    Bounds::new(origin, size(px(thumb), px(8.))),
                    rgb(accent),
                ));
            }).size_full())
            .on_mouse_down(Left, cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                let max = f32::from(this.piano_scroll.max_offset().width);
                let width = f32::from(this.piano_scroll.bounds().size.width) - 2. * PIANO_SCROLL_INSET;
                if max <= 0. || width <= 0. { return; }
                let thumb = scrollbar_thumb(width, max);
                let offset = -f32::from(this.piano_scroll.offset().x);
                let start = (offset / max).clamp(0., 1.) * (width - thumb);
                let x = this.piano_scroll_local_x(event.position.x);
                let within_thumb = x - start;
                this.piano_scroll_grab = Some(if (0.0..=thumb).contains(&within_thumb) {
                    within_thumb
                } else {
                    thumb * 0.5
                });
                this.drag_piano_time(event.position.x);
                cx.notify();
            }))
            .on_drag(PianoTimeDrag, |_, _, _, cx| cx.new(|_| EmptyDrag))
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<PianoTimeDrag>, _, cx| {
                this.drag_piano_time(event.event.position.x);
                cx.notify();
            }))
            .on_mouse_up(Left, cx.listener(|this, _, _, _| this.piano_scroll_grab = None))
            .on_mouse_up_out(Left, cx.listener(|this, _, _, _| this.piano_scroll_grab = None))
    }

    fn piano_scroll_local_x(&self, x: gpui::Pixels) -> f32 {
        f32::from(x - self.piano_scroll.bounds().left()) - PIANO_SCROLL_INSET
    }

    fn drag_piano_time(&self, x: gpui::Pixels) {
        let Some(grab) = self.piano_scroll_grab else { return; };
        let width = f32::from(self.piano_scroll.bounds().size.width) - 2. * PIANO_SCROLL_INSET;
        let max = f32::from(self.piano_scroll.max_offset().width);
        let travel = width - scrollbar_thumb(width, max);
        if travel > 0. && max > 0. {
            let offset = (self.piano_scroll_local_x(x) - grab) / travel;
            self.piano_scroll.set_offset(gpui::point(
                px(-offset.clamp(0., 1.) * max), self.piano_scroll.offset().y,
            ));
        }
    }

    fn piano_column_at(&self, pointer_x: gpui::Pixels, columns: usize) -> Option<u32> {
        let origin = self.piano_scroll.bounds().left()
            + px(PIANO_SCROLL_INSET + PR_KEYBOARD_GUTTER)
            + self.piano_scroll.offset().x;
        let position = f32::from(pointer_x - origin) / PR_COL_W;
        (position >= 0. && position < columns as f32).then_some(position as u32)
    }

    pub(super) fn piano_pressed_keys(&self) -> [u64; 2] {
        let Some(index) = self.st.selected_track.as_ref()
            .and_then(|id| self.st.project.tracks.iter().position(|track| &track.id == id))
        else { return [0; 2] };
        self.host.strip_meters.lock().ok()
            .and_then(|meters| meters.get(index).map(|meter| meter.pressed_keys()))
            .unwrap_or_default()
    }

    fn piano_keyboard_row(
        &self,
        cx: &Context<Self>,
        pitch: u8,
        keys: [u64; 2],
        track: Option<&str>,
        scroll_x: f32,
    ) -> gpui::Div {
        let row = div().absolute().left(px(scroll_x)).top_0().w(px(PR_KEY_W))
            .h_full().block_mouse_except_scroll().bg(rgb(self.theme().panel));
        let row_h = PR_ROW_H + 1.;
        if black_key(pitch) {
            let pressed = key_pressed(keys, pitch);
            row.child(div().absolute().inset_0().flex().flex_col()
                .child(self.piano_key(cx, pitch + 1, keys, track, false).h(px(row_h * 0.5))
                    .border_b_1().border_color(rgb(0xb3ab9e)))
                .child(self.piano_key(cx, pitch - 1, keys, track, false).h(px(row_h * 0.5))))
                .child(self.piano_key(cx, pitch, keys, track, true)
                    .absolute().left_0().top_0().h(px(PR_ROW_H))
                    .w(px(if pressed { 54. } else { 59. }))
                    .rounded_sm().border_b_1().border_color(rgb(0x101013)))
        } else {
            row.child(self.piano_key(cx, pitch, keys, track, true).h_full()
                .when(pitch == 0 || !black_key(pitch - 1), |key| {
                    key.border_b_1().border_color(rgb(0xb3ab9e))
                }))
        }
    }

    fn piano_key(
        &self,
        cx: &Context<Self>,
        pitch: u8,
        keys: [u64; 2],
        track: Option<&str>,
        principal: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme();
        let pressed = key_pressed(keys, pitch);
        let is_black = black_key(pitch);
        let face = if pressed { self.accent() } else if is_black { 0x26262b } else { 0xf1ede5 };
        let track = track.map(str::to_owned);
        div()
            .id(("piano-key", usize::from(pitch)))
            .when(principal, |key| key.debug_selector(move || format!("piano-key-{pitch}")))
            .block_mouse_except_scroll()
            .w_full()
            .flex().items_center().justify_end().px_2()
            .bg(rgb(face)).text_color(rgb(if pressed { theme::contrast_text(face) } else if is_black { 0xd6d6db } else { 0x272326 }))
            .text_size(px(10.)).cursor_pointer()
            .active(|style| style.opacity(0.86))
            .children((principal && (pitch.is_multiple_of(12) || pressed)).then(|| note_name(pitch)))
            .tooltip(move |_, cx| cx.new(|_| TextTooltip { theme, text: note_name(pitch).into() }).into())
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if let Some(track_id) = &track {
                    this.audition_piano_note(track_id.clone(), pitch, 88, 240);
                }
            }))
    }

    pub(super) fn code_editor(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
        let buf = self.st.repl_buf.clone();
        let status = self.st.status.clone();

        // The inner stateful div carries the id (required for click events),
        // focus tracking, key_context, and key/click handlers.
        let inner = div()
            .id("repl")
            .key_context("ReplInput")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _e: &gpui::ClickEvent, window, cx| {
                cx.stop_propagation();
                this.focus.focus(window);
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
                cx.stop_propagation();
                if ev.keystroke.modifiers.control || ev.keystroke.modifiers.platform || ev.keystroke.modifiers.alt { return; }
                let key = ev.keystroke.key.as_str();
                match key {
                    "enter" => {
                        if let Some(tid) = this.st.selected_track.clone() {
                            let src = this.st.repl_buf.clone();
                            this.undo.record("REPL eval", &this.st.project);
                            match this.st.project.apply_mini_notation(&tid, &src) {
                                Ok(n) => {
                                    this.push_live();
                                    this.st.set_status(format!("✓ {n} notes → {tid}"));
                                }
                                Err(e) => this.st.set_status(format!("✗ {e}")),
                            }
                        }
                    }
                    "space" => this.st.repl_push(' '),
                    "backspace" => this.st.repl_backspace(),
                    "escape" => this.st.repl_clear(),
                    _ => {
                        // Prefer key_char: includes shifted characters and punctuation.
                        if let Some(ch_str) = &ev.keystroke.key_char {
                            for c in ch_str.chars() {
                                this.st.repl_push(c);
                            }
                        } else if key.chars().count() == 1 {
                            if let Some(c) = key.chars().next() {
                                this.st.repl_push(c);
                            }
                        }
                    }
                }
                cx.notify();
            }))
            .child(
                div()
                    .text_color(rgb(theme.text_dim))
                    .child("type mini-notation (e.g. `c4 e4 g4`), Enter to eval"),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(theme.panel))
                    .text_color(rgb(theme.text))
                    .min_h(px(24.))
                    .child(if buf.is_empty() {
                        SharedString::from("▎")
                    } else {
                        SharedString::from(format!("{buf}▎"))
                    }),
            )
            .child(
                div()
                    .text_color(rgb(theme.text_dim))
                    .child(SharedString::from(status)),
            );

        // Outer plain Div (no .id()) — required for the return type to be `gpui::Div`.
        div().flex_1().child(inner)
    }
}

#[derive(Clone, Copy, Default)]
struct PianoCell {
    start: Option<u32>,
    velocity: u8,
    held: bool,
}

// Index notes once instead of scanning every note for every pitch/time cell.
// Preserve the authored tick for notes between grid lines when editing them.
fn piano_cells(
    notes: &[daw_core::project::Note],
    ticks: u32,
    columns: std::ops::Range<u32>,
) -> std::collections::HashMap<(u8, u32), PianoCell> {
    let mut cells = std::collections::HashMap::<(u8, u32), PianoCell>::new();
    for note in notes {
        let first = note.tick / ticks;
        let end = note.tick.saturating_add(note.duration).div_ceil(ticks);
        if columns.contains(&first) {
            let cell = cells.entry((note.pitch, first)).or_default();
            if cell.start.is_none_or(|previous| note.tick < previous) {
                cell.start = Some(note.tick);
                cell.velocity = note.vel;
            }
        }
        for col in (first + 1).max(columns.start)..end.min(columns.end) {
            let cell = cells.entry((note.pitch, col)).or_default();
            if !cell.held && cell.start.is_none() { cell.velocity = note.vel; }
            cell.held = true;
        }
    }
    cells
}


fn piano_visible_columns(offset: f32, width: f32, total: usize) -> std::ops::Range<usize> {
    let count = ((width - PR_KEYBOARD_GUTTER).max(PR_COL_W) / PR_COL_W).ceil() as usize + 3;
    let first = ((offset.max(0.) / PR_COL_W) as usize).saturating_sub(1).min(total.saturating_sub(count));
    let end = (first + count).min(total);
    first..end
}

fn black_key(pitch: u8) -> bool {
    matches!(pitch % 12, 1 | 3 | 6 | 8 | 10)
}

fn key_pressed(keys: [u64; 2], pitch: u8) -> bool {
    keys[usize::from(pitch) / 64] & (1 << (pitch % 64)) != 0
}

/// (first row, row count) to build for a viewport `view_h` tall scrolled by
/// `offset` px, with a few rows of margin. Before the first layout the
/// viewport height is unknown (0): build a generous screenful.
fn piano_visible_rows(offset: f32, view_h: f32, row_h: f32, total: usize) -> (usize, usize) {
    let view_h = if view_h > 1.0 { view_h } else { 900.0 };
    let margin = 2;
    let first = ((offset.max(0.0) / row_h) as usize).saturating_sub(margin).min(total.saturating_sub(1));
    let count = ((view_h / row_h).ceil() as usize + 2 * margin).min(total - first);
    (first, count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn piano_roll_builds_only_the_rows_in_view() {
        // 13 px rows, 49 pitches, a 160 px viewport scrolled 200 px down.
        let (first, count) = piano_visible_rows(200., 160., 13., 49);
        assert_eq!(first, 13, "15 rows are scrolled off; 2 kept as margin");
        assert!(count < 25 && first + count >= 15 + 13, "covers the view: {count}");
        // Bottom of the document never runs past the last row.
        let (first, count) = piano_visible_rows(10_000., 160., 13., 49);
        assert_eq!(first + count, 49);
        assert!(count >= 1);
        // Unknown viewport (first frame): everything a tall window could show.
        assert_eq!(piano_visible_rows(0., 0., 13., 49), (0, 49));
    }
    #[test]
    fn piano_roll_preserves_short_and_off_grid_notes() {
        let notes = [
            daw_core::project::Note { pitch: 60, tick: 62, duration: 10, vel: 80 },
            daw_core::project::Note { pitch: 60, tick: 92, duration: 80, vel: 70 },
            daw_core::project::Note { pitch: 64, tick: 0, duration: 30, vel: 90 },
        ];
        let cells = piano_cells(&notes, 30, 0..8);
        assert_eq!(cells[&(60, 2)].start, Some(62));
        assert_eq!(cells[&(60, 3)].start, Some(92));
        assert!(cells[&(60, 5)].held);
        assert!(!cells.contains_key(&(60, 6)));
        assert!(!cells.contains_key(&(64, 1)));
        let scrolled = piano_cells(&notes, 30, 4..5);
        assert_eq!(scrolled.len(), 1);
        assert!(scrolled[&(60, 4)].held);
    }
}

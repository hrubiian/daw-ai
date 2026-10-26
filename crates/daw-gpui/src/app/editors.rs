use super::*;

impl DawView {
    pub(super) fn editor_tabs(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let active = self.st.active_dock;
        div()
            .id("workspace-tabs")
            .flex_shrink_0()
            .overflow_x_scroll()
            .flex()
            .gap_1()
            .px_2()
            .py_1()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .children(DockTab::ALL.into_iter().map(|tab| {
                let selected = tab == active;
                div()
                    .id(SharedString::from(tab.label()))
                    .debug_selector(move || format!("dock-tab-{}", tab.label()))
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(if selected { PANEL_ALT } else { PANEL }))
                    .text_color(rgb(if selected { TEXT } else { TEXT_DIM }))
                    .child(tab.label())
                    .on_click(cx.listener(move |this, _ev, _w, cx| {
                        this.st.active_dock = tab;
                        this.st.arrange_full = false;
                        cx.notify();
                    }))
            }))
    }

    pub(super) fn editor_dock(&self, cx: &mut Context<Self>) -> gpui::Div {
        let active = self.st.active_dock;
        let tabs = self.editor_tabs(cx);

        let picker = self.pattern_picker(cx);
        let body = match active {
            DockTab::Sequencer => self.sequencer_editor(cx),
            DockTab::PianoRoll => self.piano_roll_editor(cx),
            DockTab::Mixing => dock_hint("Open the Mixing workspace."),
            DockTab::Fx => div().size_full().child(div().id("dock-fx-scroll").size_full().overflow_scroll().child(self.fx_editor(cx))),
            DockTab::Code => self.code_editor(cx),
        };

        div()
            .h(px(self.st.dock_h))
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .border_t_1()
            .border_color(rgb(BORDER))
            .flex_shrink_0()
            .min_h_0()
            .child(tabs)
            .child(picker)
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
    }

    pub(super) fn sampler_controls(&self, track: &daw_core::project::Track, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let daw_core::project::Instrument::Sampler { pads } = &track.instrument else { return None };
        Some(div().flex().flex_col().gap_1().child(section_title("Sampler dynamics · soft 1–63 / hard 64–127"))
            .children(pads.iter().map(|pad| {
                let pitch = pad.pitch;
                let (base, soft, hard, reset) = (track.id.clone(), track.id.clone(), track.id.clone(), track.id.clone());
                div().flex().items_center().gap_1().flex_shrink_0()
                    .child(div().w(px(150.)).text_color(rgb(TEXT_DIM)).child(format!("{} · {} takes", note_name(pitch), pad.variants.len())))
                    .child(cell_btn(format!("take-base-{pitch}"), "Base", false, PANEL_ALT).on_click(cx.listener(move |this, _, _, _| this.load_pad_dialog(base.clone(), pitch))))
                    .child(cell_btn(format!("take-soft-{pitch}"), "+ Soft", false, PANEL_ALT).on_click(cx.listener(move |this, _, _, _| this.load_pad_variant_dialog(soft.clone(), pitch, 1, 63))))
                    .child(cell_btn(format!("take-hard-{pitch}"), "+ Hard", false, PANEL_ALT).on_click(cx.listener(move |this, _, _, _| this.load_pad_variant_dialog(hard.clone(), pitch, 64, 127))))
                    .child(cell_btn(format!("take-reset-{pitch}"), "Clear takes", false, PANEL_ALT).on_click(cx.listener(move |this, _, _, cx| {
                        let before = this.st.project.clone();
                        if this.st.project.set_sampler_variants(&reset, pitch, Vec::new()).is_ok() {
                            this.undo.record("clear sampler takes", &before); this.push_live(); cx.notify();
                        }
                    })))
            })))
    }

    /// Step-grid editor for the selected pattern (when it's a step pattern).
    pub(super) fn sequencer_editor(&self, cx: &mut Context<Self>) -> gpui::Div {
        let accent = self.accent();
        let Some(pid) = self.st.selected_pattern_id() else {
            return dock_hint("Select a track with a step pattern.");
        };
        let pat = self.st.project.patterns.iter().find(|p| p.id() == pid);
        let Some(daw_core::project::Pattern::Step(sp)) = pat else {
            return dock_hint("This pattern isn't a step pattern — use the Piano Roll tab.");
        };

        let steps = sp.steps as usize;
        let gate = sp.gate;
        // Steps are 16th notes (30 ticks); the play column wraps over the
        // whole pattern length (steps × 30).
        let ticks_per_step = daw_core::sequencer::TICKS_PER_BAR / 16;
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
            div().flex().items_center().gap_2().pb_1().text_color(rgb(TEXT_DIM))
                .child(div().child(format!("{} steps", steps)))
                .child(cell_btn("seq-steps-down", "−4", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.undo.record("steps", &this.st.project); this.st.project.resize_steps(&p1, -4); this.push_live(); cx.notify(); })))
                .child(cell_btn("seq-steps-up", "+4", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.undo.record("steps", &this.st.project); this.st.project.resize_steps(&p2, 4); this.push_live(); cx.notify(); })))
                .child(cell_btn("seq-addrow", "+ Row", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.undo.record("add row", &this.st.project); this.st.project.add_step_row(&p3); this.push_live(); cx.notify(); })))
                .child(div().child(format!("gate {:.2}", gate)))
                .child(cell_btn("seq-gate-down", "−", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| {
                    let g = (this.cur_gate(&p4) - 0.05).max(0.1); this.undo.record("gate", &this.st.project); this.st.project.set_gate(&p4, g); this.push_live(); cx.notify();
                })))
                .child(cell_btn("seq-gate-up", "+", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| {
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
                cell_btn(format!("pad-{ri}"), lbl, loaded.is_some(), accent)
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
                         else if is_play_col { 0x4a4a4a }
                         else if beat { 0x383838 }
                         else { PANEL_ALT };
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
                    .text_color(rgb(if on { BG } else { TEXT_DIM })).text_size(px(10.))
                    .flex().items_center().justify_center()
                    .child(if on { velocity.to_string() } else { String::new() })
                    .tooltip(move |_, cx| cx.new(|_| TextTooltip { text: SharedString::from(format!("Velocity {velocity} · Click: toggle · Shift+click: louder · Alt+click: softer")) }).into())
                    .border_1()
                    .border_color(rgb(BORDER))
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
                        .text_color(rgb(TEXT_DIM))
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
                .child(cell_btn(format!("rp-down-{ri}"), "−", false, PANEL_ALT).on_click(cx.listener({
                    let pid = pid.clone();
                    move |this, _e, _w, cx| { this.undo.record("row pitch", &this.st.project); this.st.project.nudge_row_pitch(&pid, ri, -1); this.push_live(); cx.notify(); }
                })))
                .child(cell_btn(format!("rp-up-{ri}"), "+", false, PANEL_ALT).on_click(cx.listener({
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

    /// Pitch×time grid editor for the selected pattern (when it's piano-roll).
    pub(super) fn piano_roll_editor(&self, cx: &mut Context<Self>) -> gpui::Div {
        let accent = self.accent();
        const TICKS_PER_COL: u32 = 30; // Sixteenth notes in supported 4/4 (480/bar).
        let Some(pid) = self.st.selected_pattern_id() else {
            return dock_hint("Select a track with a piano-roll pattern.");
        };
        let pat = self.st.project.patterns.iter().find(|p| p.id() == pid);
        let Some(daw_core::project::Pattern::PianoRoll(pr)) = pat else {
            return dock_hint("This pattern isn't a piano-roll pattern — use the Sequencer tab.");
        };

        let pat_len = pr.ticks_per_bar * pr.bars.max(1) as u32;
        let cols = pat_len.div_ceil(TICKS_PER_COL).max(1) as usize;
        let cells = piano_cells(&pr.notes, TICKS_PER_COL);
        let visible_cols = piano_visible_columns(-f32::from(self.piano_scroll.offset().x),
            f32::from(self.piano_scroll.bounds().size.width).max(1280.), cols);
        let grid_width = cols as f32 * PR_COL_W;
        let document_width = 44. + grid_width;

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
        let play_col = ((self.st.playhead() % pat_len.max(1)) / TICKS_PER_COL) as usize;

        // Slim toolbar: pattern length in bars (notes auto-grow it too).
        let pr_bars = pr.bars;
        let legato_on = pr.legato;
        let (pb1, pb2, pbl) = (pid.clone(), pid.clone(), pid.clone());
        let toolbar = div().flex().items_center().gap_2().pb_1().px_2().text_color(rgb(TEXT_DIM))
            .child(div().child(format!("{pr_bars} bars")))
            .child(cell_btn("pr-bars-down", "−Bar", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| {
                this.undo.record("pr bars", &this.st.project);
                this.st.project.resize_pr_bars(&pb1, -1);
                this.push_live();
                cx.notify();
            })))
            .child(cell_btn("pr-bars-up", "+Bar", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| {
                this.undo.record("pr bars", &this.st.project);
                this.st.project.resize_pr_bars(&pb2, 1);
                this.push_live();
                cx.notify();
            })))
            .child(cell_btn("pr-legato", "Legato", legato_on, accent).on_click(cx.listener(move |this, _e, _w, cx| {
                this.undo.record("legato", &this.st.project);
                this.st.project.set_legato(&pbl, !legato_on);
                this.push_live();
                cx.notify();
            })));

        // High pitch at top down to low.
        let rows = (lo..=hi).rev().map(|pitch| {
            let is_black = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
            let label = note_name(pitch);
            let cells: Vec<_> = visible_cols.clone().map(|c| {
                let tick = c as u32 * TICKS_PER_COL;
                // Span rendering: starts are full accent, continuations dimmer.
                let info = cells.get(&(pitch, c as u32)).copied().unwrap_or_default();
                let is_start = info.start.is_some();
                let is_cont = info.held;
                let note_tick = info.start.unwrap_or(tick);
                let is_play_col = c == play_col;
                let bg = if is_start { accent }
                    else if is_cont { 0x9a6a2a } // dimmed accent: held note body
                    else if is_play_col { PANEL_ALT }
                    else if is_black { BG }
                    else { PANEL };
                let pid_c = pid.clone();
                let cell = div()
                    .id(SharedString::from(format!("nt-{pid}-{pitch}-{c}")))
                    .absolute().left(px(c as f32 * PR_COL_W)).top_0()
                    .w(px(PR_COL_W))
                    .h(px(PR_ROW_H))
                    .opacity(if is_start { 0.4 + 0.6 * info.velocity as f32 / 127. } else { 1. })
                    .tooltip(move |_, cx| cx.new(|_| TextTooltip { text: SharedString::from(if is_start || is_cont { format!("{} · velocity {} · Shift+click a note start: louder · Alt+click: softer", note_name(pitch), info.velocity) } else { format!("{} · Click to add a note", note_name(pitch)) }) }).into())
                    .flex_shrink_0()
                    .cursor_pointer()
                    .bg(rgb(bg))
                    .border_1()
                    .border_color(rgb(BG))
                    // Drop target: stretch a dragged note (same row) to end here.
                    .on_drop(cx.listener({
                        let pid_d = pid.clone();
                        move |this, d: &NoteStretch, _w, cx| {
                            if d.pid != pid_d || d.pitch != pitch || tick < d.tick {
                                return;
                            }
                            let dur = tick + TICKS_PER_COL - d.tick;
                            this.undo.record("stretch note", &this.st.project);
                            this.st.project.set_note_duration(&d.pid, d.pitch, d.tick, dur);
                            this.push_live();
                            cx.notify();
                        }
                    }));
                if is_start {
                    // Start cells: drag to stretch; click still toggles off.
                    cell.on_drag(
                        NoteStretch { pid: pid.clone(), pitch, tick: note_tick },
                        |_, _, _, cx| cx.new(|_| EmptyDrag),
                    )
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _w, cx| {
                        this.undo.record("note dynamics", &this.st.project);
                        let m = e.modifiers();
                        if m.shift { this.st.project.set_note_velocity(&pid_c, pitch, note_tick, info.velocity.saturating_add(16).min(127)); }
                        else if m.alt { this.st.project.set_note_velocity(&pid_c, pitch, note_tick, info.velocity.saturating_sub(16).max(1)); }
                        else { this.st.project.toggle_note(&pid_c, pitch, note_tick, TICKS_PER_COL); }
                        this.push_live(); cx.notify();
                    }))
                    .into_any_element()
                } else if is_cont {
                    // Continuation cells are drop targets only.
                    cell.into_any_element()
                } else {
                    cell.on_click(cx.listener(move |this, _e, _w, cx| {
                        this.undo.record("toggle note", &this.st.project);
                        this.st.project.toggle_note(&pid_c, pitch, note_tick, TICKS_PER_COL);
                        this.push_live();
                        cx.notify();
                    }))
                    .into_any_element()
                }
            }).collect();
            div()
                .flex()
                .items_center()
                .gap_1()
                .h(px(PR_ROW_H + 1.0)).flex_shrink_0()
                .child(div().w(px(40.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(TEXT_DIM)).child(label))
                .child(div().relative().w(px(grid_width)).h(px(PR_ROW_H)).flex_shrink_0().children(cells))
        });

        // Fill the dock body and scroll the (taller) pitch grid vertically; long
        // patterns also scroll horizontally. The `.id` persists the scroll offset.
        // Wrapped in a plain div so the return type stays `gpui::Div` (a `.id()`
        // chain is `Stateful<Div>`) — matching the other editor_dock match arms.
        div().size_full().min_h_0().flex().flex_col().child(toolbar.flex_shrink_0()).child(
            div()
                .id(SharedString::from(format!("pr-scroll-{pid}")))
                .track_scroll(&self.piano_scroll)
                .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
                .flex_1().min_h_0()
                .overflow_scroll()
                .flex()
                .flex_col()
                .p_2()
                .child(div().w(px(document_width)).flex_shrink_0().flex().flex_col().children(rows)),
        )
    }

    /// Right-dock mixer: one vertical channel strip per track.
    pub(super) fn mixer(&self, cx: &mut Context<Self>, width: f32) -> gpui::Div {
        let accent = self.accent();
        let sel = self.st.selected_track.clone();
        // Live per-strip peaks from the engine mixer (index = track index).
        let peaks: Vec<(f32, bool)> = self
            .host
            .strip_meters
            .lock()
            .map(|v| v.iter().map(|m| m.read()).collect())
            .unwrap_or_default();
        let strips = self.st.project.tracks.iter().enumerate().map(|(ti, t)| {
            let (peak, clipped) = peaks.get(ti).copied().unwrap_or((0.0, false));
            let id = t.id.clone();
            let is_sel = sel.as_deref() == Some(id.as_str());
            let vol = t.volume;
            let pan = t.pan;
            let muted = t.muted;
            let soloed = self.st.soloed.contains(&id);
            let name = trunc(&t.name, 6);

            let fader = div()
                .w(px(14.))
                .h(px(96.))
                .relative()
                .flex()
                .flex_col()
                .justify_end()
                .bg(rgb(PANEL_ALT))
                .rounded_md()
                .overflow_hidden()
                .child(div().w(px(14.)).h(px((96.0 * vol / 2.0).clamp(0.0, 96.0))).bg(rgb(accent)))
                // Unity notch: vol = 1.0 sits at half height.
                .child(div().absolute().left_0().bottom(px(48.)).w(px(14.)).h(px(1.)).bg(rgb(TEXT_DIM)));

            // Post-fader peak meter beside the fader; red latch when clipped.
            let meter_color = if clipped { RECORD } else if peak > 0.7 { METER_WARN } else { 0x6ee87a };
            let meter = div()
                .w(px(6.))
                .h(px(96.))
                .flex()
                .flex_col()
                .justify_end()
                .bg(rgb(PANEL_ALT))
                .rounded_sm()
                .overflow_hidden()
                .child(div().w(px(6.)).h(px((96.0 * peak.min(1.0)).max(0.0))).bg(rgb(meter_color)));
            let fader = div().flex().gap_1().items_end().child(fader).child(meter);

            let (idu, idd, idm, ids, idpl, idpr) =
                (id.clone(), id.clone(), id.clone(), id.clone(), id.clone(), id.clone());

            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .px_1()
                .py_2()
                .w(px(64.))
                .flex_shrink_0()
                .bg(rgb(if is_sel { PANEL_ALT } else { PANEL }))
                .child(div().whitespace_nowrap().overflow_hidden().text_color(rgb(TEXT)).child(name))
                .child(cell_btn(format!("mv+{id}"), "+", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.nudge_volume(&idu, 0.05); cx.notify(); })))
                .child(fader)
                .child(cell_btn(format!("mv-{id}"), "−", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.nudge_volume(&idd, -0.05); cx.notify(); })))
                .child(div().text_color(rgb(TEXT_DIM)).child(format!("{:.0}", vol * 100.0)))
                .child(
                    div().flex().gap_1()
                        .child(cell_btn(format!("mpl{id}"), "◄", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.nudge_pan(&idpl, -0.1); cx.notify(); })))
                        .child(cell_btn(format!("mpr{id}"), "►", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.nudge_pan(&idpr, 0.1); cx.notify(); }))),
                )
                .child(div().text_color(rgb(TEXT_DIM)).child(format!("p{pan:+.1}")))
                .child(
                    div().flex().gap_1()
                        .child(cell_btn(format!("mm{id}"), "M", muted, RECORD).on_click(cx.listener(move |this, _e, _w, cx| { this.toggle_mute(&idm); cx.notify(); })))
                        .child(cell_btn(format!("ms{id}"), "S", soloed, accent).on_click(cx.listener(move |this, _e, _w, cx| { this.toggle_solo(&ids); cx.notify(); }))),
                )
        });

        // Return strips (RVB / DLY): fader = bus gain, live meter, mute,
        // click → bus params in the FX dock.
        let ret_peaks: [(f32, bool); 2] = [
            self.host.return_meters[0].read(),
            self.host.return_meters[1].read(),
        ];
        let returns = (0..2usize).map(|i| {
            let (label, gain, muted) = match i {
                0 => ("RVB", self.st.project.returns.reverb.gain, self.st.project.returns.reverb.muted),
                _ => ("DLY", self.st.project.returns.delay.gain, self.st.project.returns.delay.muted),
            };
            let (peak, clipped) = ret_peaks[i];
            let is_sel = self.st.selected_return == Some(i);
            let gain_param = if i == 0 {
                daw_core::project::ReturnParam::RevGain
            } else {
                daw_core::project::ReturnParam::DlyGain
            };
            let meter_color = if clipped { RECORD } else if peak > 0.7 { METER_WARN } else { 0x6ee87a };

            let fader = div()
                .w(px(14.))
                .h(px(96.))
                .flex()
                .flex_col()
                .justify_end()
                .bg(rgb(PANEL_ALT))
                .rounded_md()
                .overflow_hidden()
                .child(div().w(px(14.)).h(px((96.0 * gain / 1.25).clamp(0.0, 96.0))).bg(rgb(accent)));
            let meter = div()
                .w(px(6.))
                .h(px(96.))
                .flex()
                .flex_col()
                .justify_end()
                .bg(rgb(PANEL_ALT))
                .rounded_sm()
                .overflow_hidden()
                .child(div().w(px(6.)).h(px((96.0 * peak.min(1.0)).max(0.0))).bg(rgb(meter_color)));

            div()
                .id(SharedString::from(format!("return-{i}")))
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .px_1()
                .py_2()
                .w(px(64.))
                .flex_shrink_0()
                .cursor_pointer()
                .bg(rgb(if is_sel { PANEL_ALT } else { BG }))
                .border_t_2()
                .border_color(rgb(accent))
                .on_click(cx.listener(move |this, _e, _w, cx| {
                    this.st.selected_return = Some(i);
                    this.st.active_dock = DockTab::Fx;
                    cx.notify();
                }))
                .child(div().whitespace_nowrap().text_color(rgb(accent)).child(label))
                .child(cell_btn(format!("rg+{i}"), "+", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.nudge_return(gain_param, 0.05); cx.notify(); })))
                .child(div().flex().gap_1().items_end().child(fader).child(meter))
                .child(cell_btn(format!("rg-{i}"), "−", false, PANEL_ALT).on_click(cx.listener(move |this, _e, _w, cx| { this.nudge_return(gain_param, -0.05); cx.notify(); })))
                .child(div().text_color(rgb(TEXT_DIM)).child(format!("{:.0}", gain * 100.0)))
                .child(cell_btn(format!("rm{i}"), "M", muted, RECORD).on_click(cx.listener(move |this, _e, _w, cx| { this.toggle_return_mute(i); cx.notify(); })))
        });

        div()
            .w(px(width))
            .min_h_0()
            .overflow_hidden()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_l_1()
            .border_color(rgb(BORDER))
            .child(
                div().flex().items_center().justify_between().pr_1()
                    .child(section_title("Mixer"))
                    .child(cell_btn("balance", "⚖", false, PANEL_ALT).on_click(cx.listener(|this, _e, _w, cx| {
                        this.start_mix_balance();
                        cx.notify();
                    })))
            )
            .child(div().id("mixer-scroll").flex_1().min_h_0().overflow_scroll().child(div().flex().gap_1().p_1().children(strips).children(returns)))
    }

    /// Single-line mini-notation REPL for the selected track.
    pub(super) fn code_editor(&self, cx: &mut Context<Self>) -> gpui::Div {
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
                    .text_color(rgb(TEXT_DIM))
                    .child("type mini-notation (e.g. `c4 e4 g4`), Enter to eval"),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(PANEL))
                    .text_color(rgb(TEXT))
                    .min_h(px(24.))
                    .child(if buf.is_empty() {
                        SharedString::from("▎")
                    } else {
                        SharedString::from(format!("{buf}▎"))
                    }),
            )
            .child(
                div()
                    .text_color(rgb(TEXT_DIM))
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
fn piano_cells(notes: &[daw_core::project::Note], ticks: u32) -> std::collections::HashMap<(u8, u32), PianoCell> {
    let mut cells = std::collections::HashMap::<(u8, u32), PianoCell>::new();
    for note in notes {
        let first = note.tick / ticks;
        let end = note.tick.saturating_add(note.duration).div_ceil(ticks);
        let cell = cells.entry((note.pitch, first)).or_default();
        if cell.start.is_none_or(|previous| note.tick < previous) {
            cell.start = Some(note.tick); cell.velocity = note.vel;
        }
        for col in first + 1..end {
            let cell = cells.entry((note.pitch, col)).or_default();
            if !cell.held && cell.start.is_none() { cell.velocity = note.vel; }
            cell.held = true;
        }
    }
    cells
}


fn piano_visible_columns(offset: f32, width: f32, total: usize) -> std::ops::Range<usize> {
    let count = (width / PR_COL_W).ceil() as usize + 3;
    let first = (((offset - 44.).max(0.) / PR_COL_W) as usize).saturating_sub(1).min(total.saturating_sub(count));
    let end = (first + count).min(total);
    first..end
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn piano_roll_preserves_short_and_off_grid_notes() {
        let notes = [
            daw_core::project::Note { pitch: 60, tick: 62, duration: 10, vel: 80 },
            daw_core::project::Note { pitch: 60, tick: 92, duration: 80, vel: 70 },
            daw_core::project::Note { pitch: 64, tick: 0, duration: 30, vel: 90 },
        ];
        let cells = piano_cells(&notes, 30);
        assert_eq!(cells[&(60, 2)].start, Some(62));
        assert_eq!(cells[&(60, 3)].start, Some(92));
        assert!(cells[&(60, 5)].held);
        assert!(!cells.contains_key(&(60, 6)));
        assert!(!cells.contains_key(&(64, 1)));
    }
}

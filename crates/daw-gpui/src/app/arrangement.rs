use super::*;

const PLAYHEAD_GREEN: u32 = 0x4ade80;

impl DawView {
    pub(super) fn track_h(&self, id: &str) -> f32 {
        self.st.track_heights.get(id).copied().unwrap_or(TRACK_H)
    }

    /// Effective per-bar pixel width at the current zoom (clamped).
    pub(super) fn bar_w(&self) -> f32 {
        (BAR_W * self.st.arrange_zoom).clamp(MIN_BAR_W, MAX_BAR_W)
    }

    /// Total pixel height of all track rows stacked (the scrollable content).
    pub(super) fn tracks_content_h(&self) -> f32 {
        self.st.project.tracks.iter().map(|t| self.track_h(&t.id)).sum()
    }

    /// Visible height of the track-rows viewport (below the ruler), derived from
    /// the window height minus the fixed chrome: menu(24) + optional sf2 banner
    /// + transport(48) + song toolbar(30) + status(18), then the editor dock
    /// (splitter 5 + dock_h, absent in full-arrange mode), then the ruler.
    pub(super) fn tracks_vis_h(&self, window_h: f32) -> f32 {
        let banner = if self.sf2_missing { 22.0 } else { 0.0 };
        let chrome = 24.0 + banner + 48.0 + 30.0 + 18.0;
        let dock = if self.st.arrange_full { 0.0 } else { 5.0 + self.st.dock_h };
        (window_h - chrome - dock - RULER_H - LOOP_LANE_H).max(40.0)
    }

    /// Largest valid vertical scroll offset: content taller than the viewport,
    /// else 0 (everything fits, no scrolling).
    pub(super) fn tracks_max_scroll_y(&self, window_h: f32) -> f32 {
        (self.tracks_content_h() - self.tracks_vis_h(window_h)).max(0.0)
    }

    /// The track header column: one row per track with select / mute / solo /
    /// volume / pan steppers / instrument label.
    pub(super) fn track_headers(&self, cx: &mut Context<Self>) -> gpui::Div {
        let accent = self.accent();
        let selected = self.st.selected_track.clone();
        let rows: Vec<_> = self.st.project.tracks.iter().enumerate().map(|(ti, t)| {
            let id = t.id.clone();
            let is_sel = selected.as_deref() == Some(id.as_str());
            let muted = t.muted;
            let soloed = self.st.soloed.contains(&id);
            let vol = t.volume;
            let pan = t.pan;
            let name = t.name.clone();
            let badge = track_type_badge(t);
            let instr = instrument_label(t);
            // The plugged instrument shows in the accent color so it's an
            // obvious, always-visible confirmation of what's loaded.
            let instr_color = match &t.instrument {
                daw_core::project::Instrument::Soundfont { bank: 0, .. } => accent,
                _ => TEXT_DIM,
            };
            let tcolor = track_color(ti);
            let th = self.track_h(&id);

            div()
                .id(SharedString::from(format!("trk-{id}")))
                .relative()
                .flex()
                .items_center()
                .gap_1()
                .h(px(th))
                .flex_shrink_0()
                .bg(rgb(if is_sel { PANEL_ALT } else { PANEL }))
                .border_b_1()
                .border_color(rgb(BORDER))
                .cursor_pointer()
                .on_click(cx.listener({
                    let id = id.clone();
                    move |this, _e, _w, cx| {
                        this.st.select_track(&id);
                        this.sync_midi_strip();
                        cx.notify();
                    }
                }))
                // Ableton-style per-track color stripe at the left edge.
                .child(div().w(px(4.)).h(px(th)).flex_shrink_0().bg(rgb(tcolor)))
                .child(
                    div().flex_1().min_w(px(0.)).flex().flex_col().gap_1().pr_2()
                        .child(div().flex().items_center().gap_1()
                            .child(div().pl_1().w(px(40.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(tcolor)).child(badge))
                            .child(if self.st.renaming == Some(RenameTarget::Track(id.clone())) {
                                div().flex_1().min_w(px(60.)).child(self.rename_editor(cx)).into_any_element()
                            } else {
                                let (idr, namer) = (id.clone(), name.clone());
                                div()
                                    .id(SharedString::from(format!("trkname-{id}")))
                                    .tooltip({
                                        let text = SharedString::from(format!("{name} · Double-click to rename"));
                                        move |_w, cx| cx.new(|_| TextTooltip { text: text.clone() }).into()
                                    })
                                    .flex_1()
                                    .min_w(px(60.))
                                    .truncate()
                                    .text_color(rgb(TEXT))
                                    .child(name.clone())
                                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, w, cx| {
                                        if e.click_count() >= 2 {
                                            this.start_rename(RenameTarget::Track(idr.clone()), namer.clone());
                                            this.rename_focus.clone().focus(w);
                                            cx.notify();
                                        }
                                    }))
                                    .into_any_element()
                            })
                            .child(cell_btn(format!("m-{id}"), "M", muted, RECORD).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| {
                                    this.toggle_mute(&id);
                                    cx.notify();
                                }
                            })))
                            .child(cell_btn(format!("s-{id}"), "S", soloed, accent).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| {
                                    this.toggle_solo(&id);
                                    cx.notify();
                                }
                            })))
                            .child(cell_btn(format!("tt-{id}"), "T", false, PANEL_ALT).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| { this.cycle_track_type(&id); cx.notify(); }
                            })))
                            .child(cell_btn(format!("tp-{id}"), "+P", false, PANEL_ALT).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| { this.add_pattern(&id); cx.notify(); }
                            })))
                            .child(cell_btn(format!("tx-{id}"), "✕", false, RECORD).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| { this.remove_track(&id); cx.notify(); }
                            })))
                        )
                        .child(div().flex().items_center().gap_1().pl_1().text_size(px(12.))
                            .child(cell_btn(format!("vd-{id}"), "−", false, PANEL_ALT).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| {
                                    this.nudge_volume(&id, -0.05);
                                    cx.notify();
                                }
                            })))
                            .child(div().w(px(46.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(TEXT_DIM)).child(format!("{:.0}%", vol * 100.0)))
                            .child(cell_btn(format!("vu-{id}"), "+", false, PANEL_ALT).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| {
                                    this.nudge_volume(&id, 0.05);
                                    cx.notify();
                                }
                            })))
                            .child(cell_btn(format!("pd-{id}"), "−", false, PANEL_ALT).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| {
                                    this.nudge_pan(&id, -0.1);
                                    cx.notify();
                                }
                            })))
                            .child(div().w(px(46.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(TEXT_DIM)).child(pan_label(pan)))
                            .child(cell_btn(format!("pu-{id}"), "+", false, PANEL_ALT).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| {
                                    this.nudge_pan(&id, 0.1);
                                    cx.notify();
                                }
                            })))
                            .child(div().id(SharedString::from(format!("instrument-{id}")))
                                .tooltip({
                                    let text = SharedString::from(instr.clone());
                                    move |_w, cx| cx.new(|_| TextTooltip { text: text.clone() }).into()
                                })
                                .flex_1().min_w(px(0.)).truncate().text_color(rgb(instr_color)).child(instr))
                        )
                )
                // Bottom-edge resize handle: dragging it adjusts this track's height.
                .child(
                    div()
                        .id(SharedString::from(format!("rzh-{id}")))
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(px(4.))
                        .cursor_pointer()
                        .bg(rgb(BORDER))
                        .on_drag(TrackResize { track: id.clone() }, |_, _, _, cx| cx.new(|_| EmptyDrag))
                        .on_drag_move(cx.listener({
                            let id = id.clone();
                            move |this, e: &DragMoveEvent<TrackResize>, _w, cx| {
                                // Only the track actually being dragged resizes (the move
                                // event fires for every TrackResize handler).
                                let dragged = { e.drag(cx).track.clone() };
                                if dragged != id { return; }
                                // Make this track's bottom follow the cursor: new_h =
                                // cur_h + (mouse_y - handle_top_y). The handle re-renders
                                // at the new bottom each frame, so it converges.
                                let handle_y = f32::from(e.bounds.origin.y);
                                let mouse_y = f32::from(e.event.position.y);
                                let cur = this.track_h(&id);
                                let new = (cur + (mouse_y - handle_y)).clamp(TRACK_H, 220.0);
                                this.st.track_heights.insert(id.clone(), new);
                                cx.notify();
                            }
                        })),
                )
        }).collect();

        let scroll_y = self.st.arrange_scroll_y;
        div()
            .flex()
            .flex_col()
            .w(px(TRACK_HEADER_W))
            .flex_shrink_0()
            .overflow_hidden()
            .bg(rgb(PANEL))
            // Top spacer aligns header rows with the timeline lanes (below the ruler);
            // it stays pinned while the rows below scroll.
            .child(div().h(px(RULER_H + LOOP_LANE_H)).flex_shrink_0()
                .px_2().flex().items_center().text_size(px(12.)).text_color(rgb(TEXT_DIM))
                .border_b_1().border_color(rgb(BORDER)).child("TRACKS  ·  double-click a name to rename"))
            // Scrollable rows: a bounded viewport that clips, with the row stack
            // shifted up by `arrange_scroll_y`. Vertical wheel drives the shared
            // offset (the arrange lanes read the same value, staying in sync).
            .child(
                track_viewport()
                    .id("track-rows")
                    .on_scroll_wheel(cx.listener(|this, e: &gpui::ScrollWheelEvent, w, cx| {
                        let step = match e.delta {
                            gpui::ScrollDelta::Lines(p) => p.y * 48.0,
                            gpui::ScrollDelta::Pixels(p) => f32::from(p.y),
                        };
                        if step == 0.0 { return; }
                        let max_y = this.tracks_max_scroll_y(window_frame::content_height(w));
                        this.st.arrange_scroll_y = (this.st.arrange_scroll_y - step).clamp(0.0, max_y);
                        cx.notify();
                    }))
                    .child(
                        scroll_document(TRACK_HEADER_W, self.tracks_content_h(), 0.0, scroll_y)
                            .children(rows),
                    ),
            )
    }

    /// The arrangement timeline: a clickable bar ruler, one lane per track with
    /// clip blocks from the project's arrangement, and a continuous green playhead.
    pub(super) fn timeline(&self, cx: &mut Context<Self>, vis_w: f32) -> gpui::Div {
        let accent = self.accent();
        let bars = self.st.project.song_bars();
        let cur_bar = self.st.current_bar();
        let bps = self.st.project.project.time_signature[0].max(1) as f64;
        let bar_seconds = (60.0 / self.st.project.project.bpm) * bps;
        let bw = self.bar_w();
        let pps = (bw as f64 / bar_seconds) as f32; // pixels per second
        let scroll_x = self.st.arrange_scroll_x;
        let playhead_x = self.st.playhead() as f32
            / daw_core::sequencer::TICKS_PER_BAR as f32 * bw - scroll_x;
        let document_w = (bars as f32 * bw).max(vis_w);

        // Virtualize: only build the bars within the visible scroll window (plus a
        // small margin), so render cost is O(visible bars), not O(song length).
        // `vis_w` (window width) is a conservative over-estimate of the timeline
        // region — it renders a few extra bars rather than ever too few.
        let first_bar = ((scroll_x / bw).floor() as u32).min(bars);
        let vis_count = (vis_w / bw).ceil() as u32 + 2;
        let last_bar = (first_bar + vis_count).min(bars);

        // Loop-region row: a thin clickable lane above the ruler. Click left
        // of the region to move its start, right of it to extend the end,
        // inside it to shrink to that bar (toolbar ⟳ enables looping).
        let lr = self.st.project.loop_region.clone();
        let region_row = div()
            .flex()
            .h(px(LOOP_LANE_H))
            .flex_shrink_0()
            .bg(rgb(PANEL))
            .child(div().w(px(first_bar as f32 * bw)).flex_shrink_0())
            .children((first_bar..last_bar).map(|b| {
                let inside = b >= lr.start_bar && b < lr.end_bar;
                let bg = if inside && lr.enabled {
                    accent
                } else if inside {
                    0x666666
                } else {
                    PANEL_ALT
                };
                div()
                    .id(SharedString::from(format!("loopbar-{b}")))
                    .w(px(bw))
                    .h_full()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .border_r_1()
                    .border_color(rgb(PANEL))
                    .bg(rgb(bg))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.click_loop_bar(b);
                        cx.notify();
                    }))
            }));

        // Ruler: clickable bar-number cells that seek.
        let ruler = div()
            .flex()
            .h(px(RULER_H))
            .flex_shrink_0()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            // leading spacer for the bars scrolled off the left
            .child(div().w(px(first_bar as f32 * bw)).flex_shrink_0())
            .children((first_bar..last_bar).map(|b| {
                div()
                    .id(SharedString::from(format!("bar-{b}")))
                    .w(px(bw))
                    .flex_shrink_0()
                    .flex()
                    .items_end()
                    .cursor_pointer()
                    .border_r_1()
                    .border_color(rgb(BORDER))
                    .child(div().px_1().text_color(rgb(if b == cur_bar { accent } else { TEXT_DIM })).child(format!("{}", b + 1)))
                    .children((0..3).map(move |_| div().w(px(1.)).h(px(5.)).bg(rgb(PANEL_ALT)).ml(px(bw / 4.0 - 1.0))))
                    .on_click(cx.listener(move |this, _e, _w, cx| { this.seek_to_bar(b); cx.notify(); }))
            }));

        let tpb = daw_core::sequencer::TICKS_PER_BAR as f32;
        // One lane per track; clip blocks span their bars (in document flow), empty
        // bars are grid cells. A clip's sub-bar `offset_ticks` is shown as a small
        // left margin so off-grid starts are visible; the engine plays the true
        // offset regardless.
        let lanes = self.st.project.tracks.iter().enumerate().map(|(ti, t)| {
            let color = clip_color(ti);
            let th = self.track_h(&t.id);
            // (bar, offset_ticks, span_bars, pattern_name, pattern_id)
            let blocks: Vec<(u32, u32, u32, String, String)> = self
                .st
                .project
                .arrangement
                .iter()
                .filter(|s| s.track == t.id)
                .filter_map(|s| {
                    let pat = self.st.project.patterns.iter().find(|p| p.id() == s.pattern)?;
                    let span = s.len_bars.unwrap_or_else(|| pat.bars().max(1)).max(1);
                    Some((s.bar, s.offset_ticks, span, pat.name().to_string(), s.pattern.clone()))
                })
                .collect();

            let tid = t.id.clone();
            // Start at the first visible bar, but back up to the start of any clip
            // that straddles it so a clip extending into view still renders.
            let start_bar = blocks.iter()
                .find(|(s, _, span, _, _)| *s <= first_bar && first_bar < *s + *span)
                .map(|(s, _, _, _, _)| *s)
                .unwrap_or(first_bar);
            let mut cells: Vec<gpui::AnyElement> = Vec::new();
            if start_bar > 0 {
                cells.push(div().w(px(start_bar as f32 * bw)).flex_shrink_0().into_any_element());
            }
            let mut bar = start_bar;
            while bar < last_bar {
                if let Some((_start, coff, span, name, pid)) = blocks.iter().find(|(s, _, _, _, _)| *s == bar).cloned() {
                    let tid_c = tid.clone();
                    let tid_x = tid.clone();
                    let tid_cyc = tid.clone();
                    let bx = bar;
                    let drag = ClipDrag { track: tid.clone(), bar, name: name.clone(), color };
                    let drop_tid = tid.clone();
                    let drop_bar = bar;
                    let off_px = coff as f32 / tpb * bw;
                    let off_label = if coff != 0 { format!(" · +{coff}t") } else { String::new() };
                    cells.push(
                        div()
                            .id(SharedString::from(format!("clip-{tid}-{bar}")))
                            .ml(px(off_px))
                            .w(px(bw * span as f32))
                            .flex_shrink_0()
                            .h(px(th - 8.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(color))
                            .text_color(rgb(BG))
                            .px_2()
                            .overflow_hidden()
                            .relative()
                            // Mini content preview (absolute underlay; the
                            // name + buttons row paints on top).
                            .child({
                                let marks = self
                                    .st
                                    .project
                                    .patterns
                                    .iter()
                                    .find(|p| p.id() == pid)
                                    .map(|p| clip_preview_marks(p, bw * span as f32, th - 8.0, span * daw_core::sequencer::TICKS_PER_BAR))
                                    .unwrap_or_default();
                                div().absolute().inset_0().children(marks.into_iter().map(|(x, y, w, h)| {
                                    div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).rounded_sm().bg(rgb(BG))
                                }))
                            })
                            // Hover: full pattern name even on 1-bar slivers.
                            .tooltip({
                                let tip: SharedString = format!(
                                    "{name} · {pid} · {span} bar{}{off_label}",
                                    if span == 1 { "" } else { "s" }
                                )
                                .into();
                                move |_w, cx| cx.new(|_| TextTooltip { text: tip.clone() }).into()
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(name),
                            )
                            // Cycle this clip to the track's next pattern, in place.
                            .child(
                                div()
                                    .id(SharedString::from(format!("clipcyc-{tid}-{bar}")))
                                    .px_1()
                                    .text_color(rgb(BG))
                                    .cursor_pointer()
                                    .child("⟳")
                                    .on_click(cx.listener(move |this, _e, _w, cx| {
                                        this.undo.record("pattern", &this.st.project);
                                        this.st.project.cycle_clip_pattern(&tid_cyc, bx);
                                        this.push_live();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("clipx-{tid}-{bar}")))
                                    .px_1()
                                    .text_color(rgb(BG))
                                    .cursor_pointer()
                                    .child("✕")
                                    .on_click(cx.listener(move |this, _e, _w, cx| {
                                        this.undo.record("remove clip", &this.st.project);
                                        this.st.project.remove_clip(&tid_x, bx);
                                        this.push_live();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("rzc-{tid}-{bar}")))
                                    .w(px(6.))
                                    .h(px(th - 8.0))
                                    .flex_shrink_0()
                                    .cursor_pointer()
                                    .bg(rgb(BG))
                                    .on_drag(ResizeClip { track: tid.clone(), bar }, |_, _, _, cx| cx.new(|_| EmptyDrag))
                                    .on_drag_move(cx.listener({
                                        let tr = tid.clone();
                                        let bw = bw;
                                        move |this, e: &DragMoveEvent<ResizeClip>, _w, cx| {
                                            // Only the clip actually being dragged should resize:
                                            // on_drag_move fires for every ResizeClip handler.
                                            let (dt, db) = { let p = e.drag(cx); (p.track.clone(), p.bar) };
                                            if dt != tr || db != bar { return; }
                                            // handle's left edge ≈ clip's right edge; how far the
                                            // cursor is past it (in bars) is the length delta. The
                                            // handle re-renders at the new edge each step, so this
                                            // converges to the cursor without feedback.
                                            let handle_x = f32::from(e.bounds.origin.x);
                                            let mouse_x = f32::from(e.event.position.x);
                                            let delta = ((mouse_x - handle_x) / bw).round() as i32;
                                            if delta != 0 {
                                                if let Some(cur) = this.st.project.clip_len_bars(&tr, bar) {
                                                    let new = (cur as i32 + delta).max(1) as u32;
                                                    this.undo.record("resize clip", &this.st.project);
                                                    if this.st.project.resize_clip(&tr, bar, new) {
                                                        this.push_live();
                                                        cx.notify();
                                                    }
                                                }
                                            }
                                        }
                                    })),
                            )
                            .on_click(cx.listener(move |this, _e, _w, cx| {
                                this.st.select_track(&tid_c);
                                this.sync_midi_strip();
                                this.st.select_pattern(&pid);
                                // Set the playback start to this clip's first bar so
                                // Play begins from here (matches DAW arrange behavior).
                                this.seek_to_bar(bx);
                                cx.notify();
                            }))
                            .on_drag(drag, |d, _off, _win, cx| {
                                cx.new(|_| ClipDragPreview { name: d.name.clone(), color: d.color })
                            })
                            .on_drop(cx.listener(move |this, d: &ClipDrag, _w, cx| {
                                this.undo.record("move clip", &this.st.project);
                                if this.st.project.move_clip(&d.track, d.bar, &drop_tid, drop_bar, 0) {
                                    this.push_live();
                                }
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                    bar += span.max(1);
                } else {
                    let here = bar == cur_bar;
                    let tid_c = tid.clone();
                    let drop_tid = tid.clone();
                    let b = bar;
                    cells.push(
                        div()
                            .id(SharedString::from(format!("cell-{tid}-{bar}")))
                            .w(px(bw))
                            .flex_shrink_0()
                            .h(px(th))
                            .cursor_pointer()
                            .border_r_1()
                            .border_color(rgb(PANEL))
                            .bg(rgb(if here { PANEL_ALT } else { BG }))
                            .on_click(cx.listener(move |this, _e, _w, cx| {
                                this.place_in_arrange(&tid_c, b);
                                cx.notify();
                            }))
                            .on_drop(cx.listener(move |this, d: &ClipDrag, _w, cx| {
                                this.undo.record("move clip", &this.st.project);
                                if this.st.project.move_clip(&d.track, d.bar, &drop_tid, b, 0) {
                                    this.push_live();
                                }
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                    bar += 1;
                }
            }

            let audio_blocks: Vec<gpui::AnyElement> = self.st.project.audio_clips.iter().enumerate()
                .filter(|(_, c)| c.track == t.id)
                // skip clips entirely outside the visible scroll window
                .filter(|(_, c)| {
                    let left = (c.start_sec as f32) * pps;
                    let right = left + ((c.len_sec as f32) * pps).max(2.0);
                    right >= scroll_x && left <= scroll_x + vis_w
                })
                .map(|(idx, c)| {
                    let left  = (c.start_sec as f32) * pps;
                    let width = ((c.len_sec as f32) * pps).max(2.0);
                    let h = th - 8.0;
                    let dur = self.st.peaks_dur_cache.get(&c.path).copied()
                        .unwrap_or(c.offset_sec + c.len_sec);
                    let start_frac = if dur > 0.0 { c.offset_sec / dur } else { 0.0 };
                    let end_frac   = if dur > 0.0 { (c.offset_sec + c.len_sec) / dur } else { 1.0 };
                    let cols = ((width / 2.0) as usize).clamp(1, 400);
                    let mags = match self.st.peaks_cache.get(&c.path) {
                        Some(pk) => waveform_columns(pk, start_frac, end_frac, cols),
                        None => Vec::new(),
                    };
                    let name = std::path::Path::new(&c.path).file_name()
                        .map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    let start_sec = c.start_sec;
                    div()
                        .id(SharedString::from(format!("aclip-{idx}")))
                        .absolute()
                        .left(px(left))
                        .top(px(4.))
                        .w(px(width))
                        .h(px(h))
                        .rounded_md()
                        .bg(rgb(PANEL_ALT))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .overflow_hidden()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            // Start playback from exactly where this audio clip begins.
                            this.seek_to_sec(start_sec);
                            cx.notify();
                        }))
                        .on_drag(AudioMove { idx }, |_, _, _, cx| cx.new(|_| EmptyDrag))
                        .on_drag_move(cx.listener({
                            move |this, e: &DragMoveEvent<AudioMove>, _w, cx| {
                                // Only the dragged clip moves (fires for every AudioMove handler).
                                if e.drag(cx).idx != idx { return; }
                                // Block's current left (px) tracks start_sec; move it so its
                                // left follows the cursor. Converges as the block re-renders.
                                let block_x = f32::from(e.bounds.origin.x);
                                let mouse_x = f32::from(e.event.position.x);
                                let delta_sec = (mouse_x - block_x) / pps;
                                if delta_sec.abs() > 0.001 {
                                    let cur = this.st.project.audio_clips.get(idx).map(|c| c.start_sec).unwrap_or(0.0);
                                    this.undo.record("move audio clip", &this.st.project);
                                    this.st.project.move_audio_clip(idx, cur + delta_sec as f64);
                                    this.push_live();
                                    cx.notify();
                                }
                            }
                        }))
                        .child(
                            div().absolute().inset_0().flex().items_center()
                                .children(mags.into_iter().map(|m|
                                    div().w(px(1.)).h(px((m * h).max(1.0))).mr(px(1.)).bg(rgb(accent))
                                ))
                        )
                        .child(div().absolute().top_0().left_0().px_1().text_color(rgb(TEXT)).child(name))
                        .child(
                            div().id(SharedString::from(format!("aclipx-{idx}")))
                                .absolute().top_0().right_0().px_1().cursor_pointer()
                                .text_color(rgb(TEXT)).child("✕")
                                .on_click(cx.listener(move |this, _e, _w, cx| {
                                    this.undo.record("remove audio clip", &this.st.project);
                                    this.st.project.remove_audio_clip(idx);
                                    this.push_live();
                                    cx.notify();
                                }))
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("acl-{idx}")))
                                .absolute().top_0().left_0().bottom_0()
                                .w(px(6.))
                                .cursor_pointer()
                                .bg(rgb(BORDER))
                                .on_drag(AudioCropL { idx }, |_, _, _, cx| cx.new(|_| EmptyDrag))
                                .on_drag_move(cx.listener(move |this, e: &DragMoveEvent<AudioCropL>, _w, cx| {
                                    if e.drag(cx).idx != idx { return; }
                                    // handle's left ≈ clip's left edge; cursor delta (px→sec) trims the front.
                                    let handle_x = f32::from(e.bounds.origin.x);
                                    let mouse_x = f32::from(e.event.position.x);
                                    let delta_sec = (mouse_x - handle_x) / pps;
                                    if delta_sec.abs() > 0.001 {
                                        this.undo.record("crop clip", &this.st.project);
                                        this.st.project.crop_audio_clip_start(idx, delta_sec as f64);
                                        this.push_live();
                                        cx.notify();
                                    }
                                }))
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("acr-{idx}")))
                                .absolute().top_0().right_0().bottom_0()
                                .w(px(6.))
                                .cursor_pointer()
                                .bg(rgb(BORDER))
                                .on_drag(AudioCropR { idx }, |_, _, _, cx| cx.new(|_| EmptyDrag))
                                .on_drag_move(cx.listener(move |this, e: &DragMoveEvent<AudioCropR>, _w, cx| {
                                    if e.drag(cx).idx != idx { return; }
                                    // handle sits at clip's right edge; (mouse - handle) is the length delta.
                                    // Read len_sec into a local first to avoid borrow conflict.
                                    let handle_x = f32::from(e.bounds.origin.x);
                                    let mouse_x = f32::from(e.event.position.x);
                                    let delta_sec = (mouse_x - handle_x) / pps;
                                    let cur_len = this.st.project.audio_clips.get(idx).map(|c| c.len_sec).unwrap_or(0.01);
                                    let new_len = (cur_len + delta_sec as f64).max(0.01);
                                    if delta_sec.abs() > 0.001 {
                                        this.undo.record("crop clip", &this.st.project);
                                        this.st.project.crop_audio_clip_end(idx, new_len);
                                        this.push_live();
                                        cx.notify();
                                    }
                                }))
                        )
                        .into_any_element()
                }).collect();

            div()
                .relative()
                .flex()
                .items_center()
                .h(px(th))
                .flex_shrink_0()
                .border_b_1()
                .border_color(rgb(BG))
                .children(cells)
                .children(audio_blocks)
        });

        // Manual horizontal scroll: we shift the content left by `scroll_x` (computed
        // above) inside an overflow-hidden viewport. We do NOT use gpui's
        // overflow_x_scroll/track_scroll here because a plain mouse wheel emits only
        // vertical deltas (which an x-only scroll ignores) and because the scroll
        // element overwrites a handle's offset from its own input state each frame —
        // so set_offset had no lasting effect.
        let scroll_y = self.st.arrange_scroll_y;
        div()
            .flex_1()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .min_w(px(0.))
            .child(
                div()
                    .id("arrange-scroll")
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(|this, e: &gpui::ScrollWheelEvent, w, cx| {
                        // Ctrl+wheel = zoom; Shift+wheel = vertical track scroll;
                        // any other wheel = horizontal scroll.
                        let dy = match e.delta {
                            gpui::ScrollDelta::Lines(p)  => p.y,
                            gpui::ScrollDelta::Pixels(p) => f32::from(p.y),
                        };
                        let dx = match e.delta {
                            gpui::ScrollDelta::Lines(p) => p.x * 48.,
                            gpui::ScrollDelta::Pixels(p) => f32::from(p.x),
                        };
                        if dy == 0.0 && dx == 0.0 { return; }
                        let content_w = this.st.project.song_bars() as f32 * this.bar_w();

                        if e.shift && !e.control {
                            let step = match e.delta {
                                gpui::ScrollDelta::Lines(p) => p.y * 48.0,
                                gpui::ScrollDelta::Pixels(p) => f32::from(p.y),
                            };
                            let max_y = this.tracks_max_scroll_y(window_frame::content_height(w));
                            this.st.arrange_scroll_y = (this.st.arrange_scroll_y - step).clamp(0.0, max_y);
                            cx.notify();
                            return;
                        }

                        if e.control {
                            let old_bw = this.bar_w();
                            let factor = if dy > 0.0 { 1.1 } else { 1.0 / 1.1 };
                            this.st.arrange_zoom = (this.st.arrange_zoom * factor)
                                .clamp(MIN_BAR_W / BAR_W, MAX_BAR_W / BAR_W);
                            let new_bw = this.bar_w();
                            // Keep the left-edge bar roughly fixed as zoom changes.
                            this.st.arrange_scroll_x =
                                (this.st.arrange_scroll_x * new_bw / old_bw).max(0.0);
                            cx.notify();
                            return;
                        }

                        // Wheel down (dy < 0) advances toward later bars. Lines→px per notch.
                        let step = match e.delta {
                            gpui::ScrollDelta::Lines(p)  => p.y * 48.0,
                            gpui::ScrollDelta::Pixels(p) => f32::from(p.y),
                        };
                        let max_off = (content_w - this.timeline_width(w)).max(0.0);
                        this.st.arrange_scroll_x =
                            (this.st.arrange_scroll_x - if dx != 0. { dx } else { step }).clamp(0.0, max_off);
                        cx.notify();
                    }))
                    // Keep every clipping viewport stationary. Only the absolute
                    // document inside it moves; flex sizing must not resize a
                    // scrolling document or translate its ancestor's clip mask.
                    .child(
                        div().relative().h(px(RULER_H + LOOP_LANE_H))
                            .flex_shrink_0().overflow_hidden()
                            .child(scroll_document(document_w, RULER_H + LOOP_LANE_H, scroll_x, 0.0)
                                .child(region_row).child(ruler)),
                    )
                    .child(
                        track_viewport()
                            .child(scroll_document(document_w, self.tracks_content_h(), scroll_x, scroll_y)
                                .children(lanes)),
                    )
                    // Paint above the clips in viewport coordinates: horizontal
                    // scrolling and zoom affect x, while vertical track scrolling
                    // leaves the line spanning the entire visible track area.
                    // No occlusion or handlers: clips and ruler remain clickable.
                    .child(
                        div().id("arrangement-playhead")
                            .absolute().left(px(playhead_x))
                            .top(px(LOOP_LANE_H)).bottom_0().w(px(2.))
                            .bg(rgb(PLAYHEAD_GREEN))
                            .child(div().absolute().top_0().left(px(-3.))
                                .w(px(8.)).h(px(5.)).bg(rgb(PLAYHEAD_GREEN))),
                    ),
            )
    }
}

use super::*;
use super::comments::CommentLocation;

struct TimelineScrollDrag;

impl DawView {
    /// One drag event on audio clip `idx` at cursor x `mouse_x`: snapshot
    /// for undo when the drag starts, place the clip from its anchor (see
    /// `HeadlessState::drag_audio_clip`) and push the edit live.
    fn drag_audio_clip(&mut self, idx: usize, kind: AudioDragKind, mouse_x: f32, pps: f32, cx: &mut Context<Self>) {
        if !self.st.is_audio_drag(idx, kind) {
            let label = if kind == AudioDragKind::Move { "move audio clip" } else { "crop clip" };
            self.undo.record(label, &self.st.project);
        }
        if self.st.drag_audio_clip(idx, kind, mouse_x, pps) {
            self.push_live();
            cx.notify();
        }
    }

    pub(super) fn track_h(&self, id: &str) -> f32 {
        self.st.track_heights.get(id).copied().unwrap_or(TRACK_H)
    }

    /// Effective per-bar pixel width at the current zoom (clamped).
    pub(super) fn bar_w(&self) -> f32 {
        (BAR_W * self.st.arrange_zoom).clamp(MIN_BAR_W, MAX_BAR_W)
    }

    fn timeline_bar_at_x(&self, x: gpui::Pixels) -> f64 {
        let timeline_x = f64::from(f32::from(x) - TRACK_HEADER_W) + f64::from(self.st.arrange_scroll_x);
        timeline_x / f64::from(self.bar_w())
    }

    fn seek_at_timeline_x(&mut self, x: gpui::Pixels) {
        let tick = (self.timeline_bar_at_x(x) * f64::from(self.st.project.ticks_per_bar())).round();
        self.seek_to_tick(tick as u32);
    }

    /// Total pixel height of all track rows stacked (the scrollable content).
    pub(super) fn tracks_content_h(&self) -> f32 {
        self.st.project.tracks.iter().map(|t| self.track_h(&t.id)).sum()
    }

    /// Visible height of the track lanes below the ruler.
    pub(super) fn tracks_vis_h(&self, window_h: f32) -> f32 {
        let banner = if self.sf2_missing { SOUNDFONT_BANNER_H } else { 0. };
        let layout = layout::PanelLayout::new(window_h, &self.st, self.sf2_missing);
        (window_h - ARRANGE_CHROME_H - banner - layout.bottom_height()).max(0.)
    }

    /// Largest valid vertical scroll offset: content taller than the viewport,
    /// else 0 (everything fits, no scrolling).
    pub(super) fn tracks_max_scroll_y(&self, window_h: f32) -> f32 {
        (self.tracks_content_h() - self.tracks_vis_h(window_h)).max(0.0)
    }

    fn timeline_bars(&self, width: f32) -> u32 {
        let viewport_bars = (width / self.bar_w()).ceil() as u32;
        let comment_end = self.st.project.tracks.iter().flat_map(|track| &track.comments)
            .filter_map(|comment| comment.bar).max().map_or(0, |bar| bar + 1);
        self.st.project.song_bars()
            .max(comment_end)
            .max(self.st.project.loop_region.end_bar)
            .max(viewport_bars)
            .saturating_add(8)
    }

    pub(super) fn timeline_max_scroll(&self, width: f32) -> f32 {
        (self.timeline_bars(width) as f32 * self.bar_w() - width).max(0.)
    }

    pub(super) fn arrangement_end_offset(&self, window: &Window) -> f32 {
        (self.st.project.song_bars() as f32 * self.bar_w() - self.timeline_width(window)).max(0.)
    }

    pub(super) fn scroll_timeline_to(&mut self, offset: f32, window: &Window) {
        self.st.arrange_scroll_x = offset.clamp(0., self.timeline_max_scroll(self.timeline_width(window)));
        // Explicit navigation can leave the playhead offscreen during playback.
        let x = self.st.playhead() as f32 / self.st.project.ticks_per_bar() as f32 * self.bar_w();
        self.playhead_was_visible = (self.st.arrange_scroll_x..=self.st.arrange_scroll_x + self.timeline_width(window)).contains(&x);
    }

    fn scroll_tracks(&mut self, delta: f32, window: &Window) {
        let limit = self.tracks_max_scroll_y(window_frame::content_height(window));
        self.st.arrange_scroll_y = (self.st.arrange_scroll_y - delta).clamp(0., limit);
    }

    fn timeline_wheel(&mut self, event: &gpui::ScrollWheelEvent, window: &Window, over_ruler: bool) {
        let delta = event.delta.pixel_delta(px(48.));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        if event.control {
            if dy == 0. { return; }
            let mouse_x = (f32::from(event.position.x) - TRACK_HEADER_W)
                .clamp(0., self.timeline_width(window));
            let bar = (self.st.arrange_scroll_x + mouse_x) / self.bar_w();
            let factor = if dy > 0. { 1.1 } else { 1. / 1.1 };
            self.st.arrange_zoom = (self.st.arrange_zoom * factor)
                .clamp(MIN_BAR_W / BAR_W, MAX_BAR_W / BAR_W);
            self.scroll_timeline_to(bar * self.bar_w() - mouse_x, window);
        } else if event.shift || over_ruler {
            let horizontal = if dx.abs() >= dy.abs() { dx } else { dy };
            self.scroll_timeline_to(self.st.arrange_scroll_x - horizontal, window);
        } else {
            self.scroll_tracks(dy, window);
            if dx != 0. {
                self.scroll_timeline_to(self.st.arrange_scroll_x - dx, window);
            }
        }
    }

    fn drag_timeline_scrollbar(&mut self, x: f32, window: &Window) {
        let Some(grab) = self.timeline_scroll_grab else { return; };
        let width = self.timeline_width(window);
        let max = self.timeline_max_scroll(width);
        let travel = width - scrollbar_thumb(width, max);
        if travel > 0. {
            self.scroll_timeline_to((x - grab) / travel * max, window);
        }
    }

    fn timeline_scrollbar(&self, width: f32, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        use gpui::MouseButton::Left;

        let theme = self.theme();
        let max = self.timeline_max_scroll(width);
        let thumb = scrollbar_thumb(width, max);
        let left = if max > 0. { self.st.arrange_scroll_x / max * (width - thumb) } else { 0. };
        div()
            .id("timeline-scrollbar")
            .debug_selector(|| "timeline-scrollbar".into())
            .relative()
            .h(px(ARRANGE_SCROLLBAR_H))
            .flex_shrink_0()
            .bg(rgb(theme.panel))
            .border_t_1()
            .border_color(rgb(theme.border))
            .cursor_pointer()
            .tooltip(move |_, cx| cx.new(|_| TextTooltip {
                theme,
                text: "Drag or Shift+scroll to move through bars".into(),
            }).into())
            .child(div().debug_selector(|| "timeline-scrollbar-thumb".into())
                .absolute().top(px(3.)).left(px(left)).w(px(thumb)).h(px(9.))
                .rounded_full().bg(rgb(theme.text_dim)))
            .on_mouse_down(Left, cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                let x = f32::from(event.position.x) - TRACK_HEADER_W;
                let within_thumb = x - left;
                this.timeline_scroll_grab = Some(if (0.0..=thumb).contains(&within_thumb) {
                    within_thumb
                } else {
                    thumb / 2.
                });
                this.drag_timeline_scrollbar(x, window);
                cx.notify();
            }))
            .on_drag(TimelineScrollDrag, |_, _, _, cx| cx.new(|_| EmptyDrag))
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<TimelineScrollDrag>, window, cx| {
                this.drag_timeline_scrollbar(f32::from(event.event.position.x - event.bounds.left()), window);
                cx.notify();
            }))
            .on_mouse_up(Left, cx.listener(|this, _, _, _| this.timeline_scroll_grab = None))
            .on_mouse_up_out(Left, cx.listener(|this, _, _, _| this.timeline_scroll_grab = None))
    }

    /// The track header column: one row per track with select / mute / solo /
    /// volume / pan steppers / instrument label.
    pub(super) fn track_headers(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme();
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
                daw_core::project::Instrument::Soundfont { bank: 0, .. }
                | daw_core::project::Instrument::Sfz { .. } => accent,
                _ => theme.text_dim,
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
                .bg(rgb(if is_sel { theme.panel_alt } else { theme.panel }))
                .border_b_1()
                .border_color(rgb(theme.border))
                .cursor_pointer()
                .on_mouse_down(gpui::MouseButton::Right, cx.listener({
                    let id = id.clone();
                    move |this, event: &gpui::MouseDownEvent, _, cx| {
                        this.open_comment_menu(CommentLocation { track_id: id.clone(), bar: None }, event.position);
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
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
                                        move |_w, cx| cx.new(|_| TextTooltip { theme, text: text.clone() }).into()
                                    })
                                    .flex_1()
                                    .min_w(px(60.))
                                    .truncate()
                                    .text_color(rgb(theme.text))
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
                            .children(t.comment(None).map(|text| self.comment_pin(
                                CommentLocation { track_id: id.clone(), bar: None }, text, 22., cx,
                            )))
                            .child(theme.cell_btn(format!("m-{id}"), "M", muted, RECORD).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| {
                                    this.toggle_mute(&id);
                                    cx.notify();
                                }
                            })))
                            .child(theme.cell_btn(format!("s-{id}"), "S", soloed, accent).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| {
                                    this.toggle_solo(&id);
                                    cx.notify();
                                }
                            })))
                            .child(theme.cell_btn(format!("tt-{id}"), "T", false, theme.panel_alt).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| { this.cycle_track_type(&id); cx.notify(); }
                            })))
                            .child(theme.cell_btn(format!("tp-{id}"), "+P", false, theme.panel_alt).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| { this.add_pattern(&id); cx.notify(); }
                            })))
                            .child(theme.cell_btn(format!("tx-{id}"), "✕", false, RECORD).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| { this.remove_track(&id); cx.notify(); }
                            })))
                        )
                        .child(div().flex().items_center().gap_1().pl_1().text_size(px(12.))
                            .child(theme.cell_btn(format!("vd-{id}"), "−", false, theme.panel_alt).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| {
                                    this.nudge_volume(&id, -0.05);
                                    cx.notify();
                                }
                            })))
                            .child(div().w(px(46.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(theme.text_dim)).child(format!("{:.0}%", vol * 100.0)))
                            .child(theme.cell_btn(format!("vu-{id}"), "+", false, theme.panel_alt).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| {
                                    this.nudge_volume(&id, 0.05);
                                    cx.notify();
                                }
                            })))
                            .child(theme.cell_btn(format!("pd-{id}"), "−", false, theme.panel_alt).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| {
                                    this.nudge_pan(&id, -0.1);
                                    cx.notify();
                                }
                            })))
                            .child(div().w(px(46.)).flex_shrink_0().whitespace_nowrap().text_color(rgb(theme.text_dim)).child(pan_label(pan)))
                            .child(theme.cell_btn(format!("pu-{id}"), "+", false, theme.panel_alt).on_click(cx.listener({
                                let id = id.clone();
                                move |this, _e, _w, cx| {
                                    this.nudge_pan(&id, 0.1);
                                    cx.notify();
                                }
                            })))
                            .child(self.track_input_button(t, cx))
                            .children((t.track_type == daw_core::project::TrackType::Audio).then(|| self.track_monitor_button(t, cx)))
                            .child(div().id(SharedString::from(format!("instrument-{id}")))
                                .tooltip({
                                    let text = SharedString::from(instr.clone());
                                    move |_w, cx| cx.new(|_| TextTooltip { theme, text: text.clone() }).into()
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
                        .bg(rgb(theme.border))
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
            .bg(rgb(theme.panel))
            // Top spacer aligns header rows with the timeline lanes (below the ruler);
            // it stays pinned while the rows below scroll.
            .child(div().h(px(RULER_H + LOOP_LANE_H)).flex_shrink_0()
                .px_2().flex().items_center().text_size(px(12.)).text_color(rgb(theme.text_dim))
                .border_b_1().border_color(rgb(theme.border)).child("TRACKS  ·  double-click a name to rename"))
            // Scrollable rows: a bounded viewport that clips, with the row stack
            // shifted up by `arrange_scroll_y`. Vertical wheel drives the shared
            // offset (the arrange lanes read the same value, staying in sync).
            .child(
                track_viewport()
                    .id("track-rows")
                    .on_scroll_wheel(cx.listener(|this, e: &gpui::ScrollWheelEvent, w, cx| {
                        this.scroll_tracks(f32::from(e.delta.pixel_delta(px(48.)).y), w);
                        cx.notify();
                    }))
                    .child(
                        scroll_document(TRACK_HEADER_W, self.tracks_content_h(), 0.0, scroll_y)
                            .children(rows),
                    ),
            )
            .child(div().h(px(ARRANGE_SCROLLBAR_H)).flex_shrink_0()
                .border_t_1().border_color(rgb(theme.border)))
    }

    /// The arrangement timeline: a clickable bar ruler, one lane per track with
    /// clip blocks from the project's arrangement, and a continuous green playhead.
    pub(super) fn timeline(&self, cx: &mut Context<Self>, vis_w: f32) -> gpui::Div {
        let theme = self.theme();
        let accent = self.accent();
        let bars = self.timeline_bars(vis_w);
        let cur_bar = self.st.current_bar();
        let tpb_u32 = self.st.project.ticks_per_bar();
        let bpb = self.st.project.beats_per_bar();
        let meter_beat_ticks = self.st.project.ticks_per_meter_beat();
        let spb_steps = self.st.project.steps_per_bar();
        let bar_seconds = self.st.project.seconds_per_bar();
        let bw = self.bar_w();
        let pps = (bw as f64 / bar_seconds) as f32; // pixels per second
        let scroll_x = self.st.arrange_scroll_x;
        let playhead_x = self.st.playhead() as f32
            / tpb_u32 as f32 * bw - scroll_x;
        let document_w = (bars as f32 * bw).max(vis_w);

        // Virtualize: only build the bars within the visible scroll window (plus a
        // small margin), so render cost is O(visible bars), not O(song length).
        let first_bar = ((scroll_x / bw).floor() as u32).min(bars);
        let vis_count = (vis_w / bw).ceil() as u32 + 2;
        let last_bar = first_bar.saturating_add(vis_count).min(bars);

        // Loop-region row: a thin clickable lane above the ruler. Click left
        // of the region to move its start, right of it to extend the end,
        // inside it to shrink to that bar (toolbar ⟳ enables looping).
        let lr = self.st.project.loop_region.clone();
        let region_row = div()
            .flex()
            .h(px(LOOP_LANE_H))
            .flex_shrink_0()
            .bg(rgb(theme.panel))
            .child(div().w(px(first_bar as f32 * bw)).flex_shrink_0())
            .children((first_bar..last_bar).map(|b| {
                let inside = b >= lr.start_bar && b < lr.end_bar;
                let bg = if inside && lr.enabled {
                    accent
                } else if inside {
                    0x666666
                } else {
                    theme.panel_alt
                };
                div()
                    .id(SharedString::from(format!("loopbar-{b}")))
                    .w(px(bw))
                    .h_full()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .border_r_1()
                    .border_color(rgb(theme.panel))
                    .bg(rgb(bg))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.click_loop_bar(b);
                        cx.notify();
                    }))
            }));

        // Hide labels first and then ticks as the available space per beat shrinks.
        let beat_w = bw / bpb as f32;
        let active_beat = (self.st.playhead() % tpb_u32) / meter_beat_ticks;
        let label_stride = if bw >= 32. { 1 } else if bw >= 16. { 2 } else { 4 };
        let ruler = div()
            .flex()
            .h(px(RULER_H))
            .flex_shrink_0()
            .bg(rgb(theme.panel))
            .border_b_1()
            .border_color(rgb(theme.border))
            // leading spacer for the bars scrolled off the left
            .child(div().w(px(first_bar as f32 * bw)).flex_shrink_0())
            .children((first_bar..last_bar).map(|b| {
                div()
                    .id(SharedString::from(format!("bar-{b}")))
                    .w(px(bw))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .cursor_pointer()
                    .border_r_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(if b == cur_bar { theme.panel_alt } else { theme.panel }))
                    .child(div().h(px(17.)).px_1().text_size(px(11.)).line_height(px(16.))
                        .text_color(rgb(if b == cur_bar { accent } else { theme.text }))
                        .children(b.is_multiple_of(label_stride).then(|| (b + 1).to_string())))
                    .children((beat_w >= 4.).then(|| {
                        div().flex().border_t_1().border_color(rgb(theme.border)).children(
                            (0..bpb).map(|beat| {
                                let active = b == cur_bar && beat == active_beat;
                                div().w(px(beat_w)).h(px(14.)).flex_shrink_0()
                                    .border_l_1().border_color(rgb(if active { accent } else { theme.border }))
                                    .px_1().text_size(px(9.)).line_height(px(13.))
                                    .debug_selector(move || format!("timeline-beat-{b}-{beat}"))
                                    .text_color(rgb(if active { accent } else { theme.text_dim }))
                                    .children((beat_w >= 13.).then(|| (beat + 1).to_string()))
                            }),
                        )
                    }))
                    .on_click(cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                        this.seek_at_timeline_x(event.position().x);
                        cx.notify();
                    }))
            }));

        let tpb = tpb_u32 as f32;
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
                    let span = s.len_bars.unwrap_or_else(|| pat.bars_in(spb_steps).max(1)).max(1);
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
                            .debug_selector(|| format!("clip-{tid}-{bar}"))
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
                            .text_color(rgb(ON_CLIP))
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
                                    .map(|p| {
                                        // Clip-local x of the visible timeline window.
                                        let clip_x = bar as f32 * bw + off_px;
                                        clip_preview_marks_in(p, bw * span as f32, th - 8.0, span * tpb_u32,
                                            scroll_x - clip_x - 16.0, scroll_x - clip_x + vis_w + 16.0)
                                    })
                                    .unwrap_or_default();
                                div().absolute().inset_0().children(marks.into_iter().map(|(x, y, w, h)| {
                                    div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).rounded_sm().bg(rgb(theme.bg))
                                }))
                            })
                            // Hover: full pattern name even on 1-bar slivers.
                            .tooltip({
                                let tip: SharedString = format!(
                                    "{name} · {pid} · {span} bar{}{off_label}",
                                    if span == 1 { "" } else { "s" }
                                )
                                .into();
                                move |_w, cx| cx.new(|_| TextTooltip { theme, text: tip.clone() }).into()
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
                                    .text_color(rgb(ON_CLIP))
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
                                    .text_color(rgb(ON_CLIP))
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
                                    .bg(rgb(theme.bg))
                                    .on_drag(ResizeClip { track: tid.clone(), bar }, |_, _, _, cx| cx.new(|_| EmptyDrag))
                                    .on_drag_move(cx.listener({
                                        let tr = tid.clone();
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
                            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                this.st.select_track(&tid_c);
                                this.sync_midi_strip();
                                this.st.select_pattern(&pid);
                                this.seek_at_timeline_x(event.position().x);
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
                            .border_color(rgb(if (b + 1).is_multiple_of(4) { theme.border } else { theme.panel }))
                            .bg(rgb(if here { theme.panel_alt } else { theme.bg }))
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
                    div()
                        .id(SharedString::from(format!("aclip-{idx}")))
                        .debug_selector(|| format!("aclip-{idx}"))
                        .absolute()
                        .left(px(left))
                        .top(px(4.))
                        .w(px(width))
                        .h(px(h))
                        .rounded_md()
                        .bg(rgb(theme.panel_alt))
                        .border_1()
                        .border_color(rgb(theme.border))
                        .overflow_hidden()
                        .cursor_pointer()
                        // Every press starts a fresh drag anchor (the edge
                        // handles' presses bubble here too).
                        .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, _| this.st.end_audio_drag()))
                        .on_click(cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                            this.seek_at_timeline_x(event.position().x);
                            cx.notify();
                        }))
                        .on_drag(AudioMove { idx }, |_, _, _, cx| cx.new(|_| EmptyDrag))
                        .on_drag_move(cx.listener(move |this, e: &DragMoveEvent<AudioMove>, _w, cx| {
                            // Only the dragged clip moves (fires for every AudioMove handler).
                            if e.drag(cx).idx != idx { return; }
                            this.drag_audio_clip(idx, AudioDragKind::Move, f32::from(e.event.position.x), pps, cx);
                        }))
                        .child(
                            div().absolute().inset_0().flex().items_center()
                                .children(mags.into_iter().map(|m|
                                    div().w(px(1.)).h(px((m * h).max(1.0))).mr(px(1.)).bg(rgb(accent))
                                ))
                        )
                        .child(div().absolute().top_0().left_0().px_1().text_color(rgb(theme.text)).child(name))
                        .child(
                            div().id(SharedString::from(format!("aclipx-{idx}")))
                                .absolute().top_0().right_0().px_1().cursor_pointer()
                                .text_color(rgb(theme.text)).child("✕")
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
                                .bg(rgb(theme.border))
                                .on_drag(AudioCropL { idx }, |_, _, _, cx| cx.new(|_| EmptyDrag))
                                .on_drag_move(cx.listener(move |this, e: &DragMoveEvent<AudioCropL>, _w, cx| {
                                    if e.drag(cx).idx != idx { return; }
                                    this.drag_audio_clip(idx, AudioDragKind::CropStart, f32::from(e.event.position.x), pps, cx);
                                }))
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("acr-{idx}")))
                                .absolute().top_0().right_0().bottom_0()
                                .w(px(6.))
                                .cursor_pointer()
                                .bg(rgb(theme.border))
                                .on_drag(AudioCropR { idx }, |_, _, _, cx| cx.new(|_| EmptyDrag))
                                .on_drag_move(cx.listener(move |this, e: &DragMoveEvent<AudioCropR>, _w, cx| {
                                    if e.drag(cx).idx != idx { return; }
                                    this.drag_audio_clip(idx, AudioDragKind::CropEnd, f32::from(e.event.position.x), pps, cx);
                                }))
                        )
                        .into_any_element()
                }).collect();

            div()
                .id(SharedString::from(format!("track-lane-{tid}")))
                .relative()
                .flex()
                .items_center()
                .h(px(th))
                .flex_shrink_0()
                .border_b_1()
                .border_color(rgb(theme.border))
                .on_mouse_down(gpui::MouseButton::Right, cx.listener({
                    let tid = tid.clone();
                    move |this, event: &gpui::MouseDownEvent, _, cx| {
                        let bar = this.timeline_bar_at_x(event.position.x).floor() as u32;
                        this.open_comment_menu(CommentLocation { track_id: tid.clone(), bar: Some(bar) }, event.position);
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .children(cells)
                .children(audio_blocks)
                .children(self.volume_automation_open.then(|| self.volume_lane(t, vis_w, cx)))
                .children(t.comments.iter().filter_map(|comment| {
                    let bar = comment.bar.filter(|bar| (first_bar..last_bar).contains(bar))?;
                    let pin_width = bw.min(22.);
                    Some(div().absolute().left(px((bar + 1) as f32 * bw - pin_width)).bottom_1()
                        .child(self.comment_pin(
                            CommentLocation { track_id: tid.clone(), bar: Some(bar) },
                            &comment.text, pin_width, cx,
                        )))
                }))
        });

        // Shift the document within stationary viewports so the ruler, lanes
        // and headers stay aligned during horizontal and vertical scrolling.
        let scroll_y = self.st.arrange_scroll_y;
        div()
            .flex_1()
            .flex()
            .flex_col()
            .bg(rgb(theme.bg))
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
                    .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, window, cx| {
                        this.timeline_wheel(event, window, false);
                        cx.notify();
                        cx.stop_propagation();
                    }))
                    .child(
                        div().id("timeline-ruler").relative().h(px(RULER_H + LOOP_LANE_H))
                            .flex_shrink_0().overflow_hidden()
                            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, window, cx| {
                                this.timeline_wheel(event, window, true);
                                cx.notify();
                                cx.stop_propagation();
                            }))
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
                            .bg(rgb(theme.playhead))
                            .child(div().absolute().top_0().left(px(-3.))
                                .w(px(8.)).h(px(5.)).bg(rgb(theme.playhead))),
                    ),
            )
            .child(self.timeline_scrollbar(vis_w, cx))
    }
}

use super::*;
use daw_core::project::{Track, VolumePoint, MAX_VOLUME_POINTS};
use gpui::{Bounds, ClickEvent, MouseButton, Pixels, Point};
use std::{cell::Cell, rc::Rc};

const INSET: f32 = 7.0;

#[derive(Clone)]
struct VolumeDrag {
    track_id: String,
    point: VolumePoint,
}

fn gain_y(gain: f32, height: f32) -> f32 {
    INSET + (1.0 - gain * 0.5) * (height - 2.0 * INSET).max(1.0)
}

fn point_at(
    position: Point<Pixels>,
    bounds: Bounds<Pixels>,
    scroll: f32,
    px_per_tick: f32,
) -> VolumePoint {
    let x = f32::from(position.x - bounds.origin.x);
    let y = f32::from(position.y - bounds.origin.y);
    let height = f32::from(bounds.size.height);
    VolumePoint {
        tick: ((f64::from(x) + f64::from(scroll)) / f64::from(px_per_tick)).round() as u32,
        gain: 2.0 * (1.0 - ((y - INSET) / (height - 2.0 * INSET).max(1.0)).clamp(0.0, 1.0)),
    }
}

impl DawView {
    pub(super) fn volume_lane(
        &self,
        track: &Track,
        width: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme();
        let accent = self.accent();
        let scroll = self.st.arrange_scroll_x;
        let px_per_tick = self.bar_w() / self.st.project.ticks_per_bar() as f32;
        let track_id = &track.id;
        let height = self.track_h(track_id);
        let start_tick = (scroll / px_per_tick).floor() as u32;
        let end_tick = ((scroll + width) / px_per_tick).ceil() as u32;
        let x_for_tick = move |tick| (f64::from(tick) * f64::from(px_per_tick) - f64::from(scroll)) as f32;
        let points = &track.volume_automation;
        let first = points.partition_point(|point| point.tick < start_tick);
        let last = points.partition_point(|point| point.tick <= end_tick);
        let visible = &points[first..last];
        let path_point = |x, gain| gpui::point(px(x), px(gain_y(gain, height)));
        let start = path_point(x_for_tick(start_tick), VolumePoint::segment_at(points, start_tick).0);
        let mut curves = Vec::new();
        let mut tick = start_tick;
        while tick < end_tick {
            let Some(segment) = VolumePoint::curve_at(points, tick) else { break; };
            let end = segment.end.tick.min(end_tick);
            let [_, a, b, gain] = segment.bezier_gains(tick, end);
            let x = x_for_tick(tick);
            let right = x_for_tick(end);
            let third = (right - x) / 3.0;
            curves.push([
                path_point(right, gain),
                path_point(x + third, a),
                path_point(right - third, b),
            ]);
            tick = end;
        }
        let held_end = (tick < end_tick).then(|| {
            path_point(x_for_tick(end_tick), VolumePoint::segment_at(points, tick).0)
        });

        let bounds = Rc::new(Cell::new(Bounds::default()));
        let painted = bounds.clone();
        let click_bounds = bounds.clone();
        let click_track = track_id.clone();
        let drop_track = track_id.clone();
        let selector = format!("volume-lane-{track_id}");
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .absolute()
            .left(px(scroll))
            .top_0()
            .bottom_0()
            .w(px(width))
            .occlude()
            .bg(gpui::rgba((theme.bg << 8) | 0xa0))
            .cursor(gpui::CursorStyle::Crosshair)
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                let point = point_at(event.position(), click_bounds.get(), scroll, px_per_tick);
                this.edit_volume_point(&click_track, None, Some(point), cx);
                cx.stop_propagation();
            }))
            .on_drop(cx.listener(move |this, drag: &VolumeDrag, window, cx| {
                if drag.track_id == drop_track {
                    let point =
                        point_at(window.mouse_position(), bounds.get(), scroll, px_per_tick);
                    this.edit_volume_point(&drop_track, Some(drag.point), Some(point), cx);
                }
            }))
            .child(
                gpui::canvas(
                    move |bounds, _, _| painted.set(bounds),
                    move |bounds, _, window, _| {
                        let mut path = gpui::PathBuilder::stroke(px(1.5));
                        path.move_to(bounds.origin + start);
                        for &[to, a, b] in &curves {
                            path.cubic_bezier_to(bounds.origin + to, bounds.origin + a, bounds.origin + b);
                        }
                        if let Some(end) = held_end {
                            path.line_to(bounds.origin + end);
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, rgb(accent));
                        }
                    },
                )
                .size_full(),
            )
            .children(visible.iter().copied().map(|point| {
                let drag = VolumeDrag {
                    track_id: track_id.clone(),
                    point,
                };
                let remove_track = track_id.clone();
                let hint: SharedString = format!("{:.0}%", point.gain * 100.0).into();
                div()
                    .id(SharedString::from(format!("volume-point-{track_id}-{}", point.tick)))
                    .absolute()
                    .left(px(x_for_tick(point.tick) - 5.0))
                    .top(px(gain_y(point.gain, height) - 5.0))
                    .size(px(10.0))
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(theme.bg))
                    .bg(rgb(accent))
                    .cursor_pointer()
                    .tooltip(move |_, cx| {
                        cx.new(|_| TextTooltip {
                            theme,
                            text: hint.clone(),
                        })
                        .into()
                    })
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, _, cx| {
                            this.edit_volume_point(&remove_track, Some(point), None, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_drag(drag, |_, _, _, cx| cx.new(|_| EmptyDrag))
            }))
    }

    fn edit_volume_point(
        &mut self,
        track_id: &str,
        original: Option<VolumePoint>,
        mut replacement: Option<VolumePoint>,
        cx: &mut Context<Self>,
    ) {
        let Some(track) = self
            .st
            .project
            .tracks
            .iter()
            .find(|track| track.id == track_id)
        else {
            return;
        };
        let mut points = track.volume_automation.clone();
        if let Some(original) = original {
            let Ok(index) = points.binary_search_by_key(&original.tick, |point| point.tick) else {
                return;
            };
            if points[index] != original {
                return;
            }
            if let Some(point) = &mut replacement {
                let min = index
                    .checked_sub(1)
                    .map_or(0, |previous| points[previous].tick + 1);
                let max = points.get(index + 1).map_or(u32::MAX, |next| next.tick - 1);
                point.tick = point.tick.clamp(min, max);
            }
            points.remove(index);
        }
        if let Some(point) = replacement {
            match points.binary_search_by_key(&point.tick, |p| p.tick) {
                Ok(index) => points[index] = point,
                Err(index) if points.len() < MAX_VOLUME_POINTS => points.insert(index, point),
                Err(_) => {
                    self.st.set_status(format!(
                        "Volume automation supports up to {MAX_VOLUME_POINTS} points per track"
                    ));
                    cx.notify();
                    return;
                }
            }
        }
        if points != track.volume_automation {
            self.undo.record("volume automation", &self.st.project);
            match self.st.project.set_volume_automation(track_id, points) {
                Ok(()) => self.push_live(),
                Err(error) => self.st.set_status(error),
            }
            cx.notify();
        }
    }
}

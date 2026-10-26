//! Desktop chrome for compositors that require client-side decorations.
use super::*;
use gpui::{CursorStyle, Decorations, MouseButton, ResizeEdge};

const TITLE_HEIGHT: f32 = 32.0;
const RESIZE_BORDER: f32 = 5.0;

fn client_frame(window: &Window) -> bool {
    matches!(window.window_decorations(), Decorations::Client { .. })
}

pub(super) fn content_height(window: &Window) -> f32 {
    f32::from(window.viewport_size().height)
        - if client_frame(window) {
            TITLE_HEIGHT
        } else {
            0.0
        }
}

pub(super) fn frame(
    content: impl IntoElement,
    window: &Window,
    close: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    if !client_frame(window) {
        return div().size_full().child(content);
    }
    client_frame_content(content, window, close)
}

fn client_frame_content(
    content: impl IntoElement,
    window: &Window,
    close: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    let title = div()
        .id("window-title-drag")
        .flex_1()
        .h_full()
        .px_3()
        .flex()
        .items_center()
        .child("daw-ai")
        .on_mouse_down(MouseButton::Left, |event, window, cx| {
            cx.stop_propagation();
            if event.click_count == 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        })
        .on_mouse_down(MouseButton::Right, |event, window, cx| {
            cx.stop_propagation();
            window.show_window_menu(event.position);
        });
    let button = |id: &'static str, label: &'static str, hover_color: u32| {
        div()
            .id(id)
            .w(px(44.))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(hover_color)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(label)
    };
    let titlebar = div()
        .h(px(TITLE_HEIGHT))
        .flex_shrink_0()
        .flex()
        .bg(rgb(PANEL))
        .text_color(rgb(TEXT))
        .child(title)
        .child(
            button("window-minimize", "−", PANEL_ALT).on_click(|_, w, cx| {
                cx.stop_propagation();
                w.minimize_window();
            }),
        )
        .child(
            button(
                "window-maximize",
                if window.is_maximized() { "❐" } else { "□" },
                PANEL_ALT,
            )
            .on_click(|_, w, cx| {
                cx.stop_propagation();
                w.zoom_window();
            }),
        )
        .child(button("window-close", "×", RECORD).on_click(close));
    let mut root = div()
        .size_full()
        .relative()
        .flex()
        .flex_col()
        .child(titlebar)
        // Keep menus positioned relative to the DAW workspace, below the titlebar.
        .child(div().relative().flex_1().min_h_0().w_full().child(content));
    if !window.is_maximized() && !window.is_fullscreen() {
        for (index, edge) in [
            ResizeEdge::Top,
            ResizeEdge::Bottom,
            ResizeEdge::Left,
            ResizeEdge::Right,
            ResizeEdge::TopLeft,
            ResizeEdge::TopRight,
            ResizeEdge::BottomLeft,
            ResizeEdge::BottomRight,
        ]
        .into_iter()
        .enumerate()
        {
            let top = matches!(
                edge,
                ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight
            );
            let bottom = matches!(
                edge,
                ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight
            );
            let left = matches!(
                edge,
                ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft
            );
            let right = matches!(
                edge,
                ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight
            );
            let cursor = match edge {
                ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
                ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
                _ => CursorStyle::ResizeUpRightDownLeft,
            };
            root = root.child(
                div()
                    .id(("window-resize", index))
                    .absolute()
                    .occlude()
                    .cursor(cursor)
                    .when(top, |d| d.top_0())
                    .when(bottom, |d| d.bottom_0())
                    .when(left, |d| d.left_0())
                    .when(right, |d| d.right_0())
                    .when(!left && !right, |d| {
                        d.left(px(RESIZE_BORDER)).right(px(RESIZE_BORDER))
                    })
                    .when(!top && !bottom, |d| {
                        d.top(px(RESIZE_BORDER)).bottom(px(RESIZE_BORDER))
                    })
                    .when(top || bottom, |d| d.h(px(RESIZE_BORDER)))
                    .when(left || right, |d| d.w(px(RESIZE_BORDER)))
                    .on_mouse_down(MouseButton::Left, move |_, w, cx| {
                        cx.stop_propagation();
                        w.start_window_resize(edge);
                    }),
            );
        }
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FrameProbe;

    impl Render for FrameProbe {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            // Test windows report server decorations. Exercise the actual client
            // frame explicitly so Wayland-only startup panics cannot hide there.
            client_frame_content(div().size_full().child("Workspace"), window, |_, w, _| {
                w.remove_window()
            })
        }
    }

    #[gpui::test]
    fn client_titlebar_renders_at_different_window_sizes(cx: &mut gpui::TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| FrameProbe);
        for (width, height) in [(800., 480.), (1280., 800.), (1920., 1080.)] {
            cx.simulate_resize(size(px(width), px(height)));
            cx.run_until_parked();
        }
    }
}

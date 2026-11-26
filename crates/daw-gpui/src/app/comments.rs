use super::*;
use daw_core::project::MAX_TRACK_COMMENT_CHARS;
use gpui::{MouseButton, Pixels, Point};

#[derive(Clone)]
pub(super) struct CommentLocation {
    pub track_id: String,
    pub bar: Option<u32>,
}

pub(super) struct CommentDraft {
    location: CommentLocation,
    original: String,
    chars: Vec<char>,
    caret: usize,
    select_all: bool,
    error: Option<String>,
}

impl CommentDraft {
    fn insert(&mut self, text: &str) {
        if self.select_all {
            self.chars.clear();
            self.caret = 0;
            self.select_all = false;
        }
        let available = MAX_TRACK_COMMENT_CHARS.saturating_sub(self.chars.len());
        let inserted: Vec<char> = text
            .chars()
            .filter(|ch| !ch.is_control() || *ch == '\n')
            .take(available)
            .collect();
        let count = inserted.len();
        self.chars.splice(self.caret..self.caret, inserted);
        self.caret += count;
    }

    fn backspace(&mut self) {
        if self.select_all {
            self.insert("");
        } else if self.caret > 0 {
            self.caret -= 1;
            self.chars.remove(self.caret);
        }
    }

    fn delete(&mut self) {
        if self.select_all {
            self.insert("");
        } else if self.caret < self.chars.len() {
            self.chars.remove(self.caret);
        }
    }
}

impl DawView {
    pub(super) fn open_comment_menu(&mut self, location: CommentLocation, point: Point<Pixels>) {
        self.comment_menu = Some((location, point));
    }

    fn start_comment(&mut self, location: CommentLocation, window: &mut Window) {
        let Some(track) = self
            .st
            .project
            .tracks
            .iter()
            .find(|track| track.id == location.track_id)
        else {
            self.dismiss_comment(window);
            return;
        };
        let original = track.comment(location.bar).unwrap_or_default().to_owned();
        let chars: Vec<char> = original.chars().collect();
        self.comment_menu = None;
        self.comment_draft = Some(CommentDraft {
            location,
            original,
            caret: chars.len(),
            chars,
            select_all: false,
            error: None,
        });
        self.comment_focus.focus(window);
    }

    pub(super) fn dismiss_comment(&mut self, window: &mut Window) {
        self.comment_draft = None;
        self.comment_menu = None;
        self.root_focus.focus(window);
    }

    fn commit_comment(&mut self, remove: bool, window: &mut Window) {
        let Some(draft) = &mut self.comment_draft else {
            return;
        };
        let CommentLocation { track_id, bar } = &draft.location;
        let Some(index) = self
            .st
            .project
            .tracks
            .iter()
            .position(|track| &track.id == track_id)
        else {
            draft.error = Some("This track was removed.".into());
            return;
        };
        let track = &self.st.project.tracks[index];
        if track.comment(*bar).unwrap_or_default() != draft.original {
            draft.error = Some("This comment changed. Reopen it to see the latest text.".into());
            return;
        }
        let text = if remove {
            String::new()
        } else {
            draft.chars.iter().collect::<String>()
        };
        if text.trim() != draft.original {
            let mut updated = self.st.project.clone();
            if let Err(error) = updated.tracks[index].set_comment(*bar, &text) {
                draft.error = Some(error.to_string());
                return;
            }
            if let Some(session) = &self.host.session {
                if let Err(error) = session.submit(&updated) {
                    draft.error = Some(error.to_string());
                    return;
                }
            }
            let label = if text.trim().is_empty() {
                "remove track comment"
            } else {
                "save track comment"
            };
            self.undo.record(label, &self.st.project);
            self.st.project = updated;
        }
        self.dismiss_comment(window);
    }

    pub(super) fn comment_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = &mut self.comment_draft else {
            return;
        };
        let key = &event.keystroke;
        if key.modifiers.control || key.modifiers.platform {
            match key.key.as_str() {
                "a" => draft.select_all = true,
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        draft.insert(&text.replace("\r\n", "\n"));
                    }
                }
                _ => {}
            }
            return;
        }
        match key.key.as_str() {
            "escape" => self.dismiss_comment(window),
            "enter" if key.modifiers.shift => draft.insert("\n"),
            "enter" => self.commit_comment(false, window),
            "backspace" => draft.backspace(),
            "delete" => draft.delete(),
            "left" | "right" | "home" | "end" => {
                draft.caret = match key.key.as_str() {
                    "left" => {
                        if draft.select_all {
                            0
                        } else {
                            draft.caret.saturating_sub(1)
                        }
                    }
                    "right" => {
                        if draft.select_all {
                            draft.chars.len()
                        } else {
                            (draft.caret + 1).min(draft.chars.len())
                        }
                    }
                    "home" => 0,
                    _ => draft.chars.len(),
                };
                draft.select_all = false;
            }
            "space" => draft.insert(" "),
            _ => {
                if let Some(text) = &key.key_char {
                    draft.insert(text);
                } else if key.key.chars().count() == 1 && !key.modifiers.alt {
                    draft.insert(&key.key);
                }
            }
        }
    }

    pub(super) fn comment_pin(
        &self,
        location: CommentLocation,
        text: &str,
        width: f32,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme();
        let id = format!(
            "comment-pin-{}-{}",
            location.track_id,
            location
                .bar
                .map_or_else(|| "track".into(), |bar| bar.to_string())
        );
        let text = SharedString::from(trunc(text, 90));
        div()
            .id(SharedString::from(id.clone()))
            .debug_selector(move || id.clone())
            .w(px(width))
            .h(px(18.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .rounded_sm()
            .bg(rgb(theme.panel))
            .text_color(rgb(self.accent()))
            .text_size(px(width.min(13.)))
            .cursor_pointer()
            .child("💬")
            .tooltip(move |_w, cx| {
                cx.new(|_| TextTooltip {
                    theme,
                    text: text.clone(),
                })
                .into()
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.start_comment(location.clone(), window);
                cx.stop_propagation();
                cx.notify();
            }))
    }

    pub(super) fn comment_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let theme = self.theme();
        if let Some((location, point)) = &self.comment_menu {
            let location = location.clone();
            let editing = self
                .st
                .project
                .tracks
                .iter()
                .find(|track| track.id == location.track_id)
                .is_some_and(|track| track.comment(location.bar).is_some());
            let content_h = window_frame::content_height(window);
            let title_h = f32::from(window.viewport_size().height) - content_h;
            let left = f32::from(point.x)
                .clamp(0., (f32::from(window.viewport_size().width) - 164.).max(0.));
            let top = (f32::from(point.y) - title_h).clamp(0., (content_h - 42.).max(0.));
            return Some(
                div()
                    .absolute()
                    .inset_0()
                    .child(
                        overlay_surface()
                            .id("comment-menu-scrim")
                            .absolute()
                            .inset_0()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dismiss_comment(window);
                                cx.notify();
                            })),
                    )
                    .child(
                        overlay_surface()
                            .absolute()
                            .left(px(left))
                            .top(px(top))
                            .w(px(164.))
                            .p_1()
                            .bg(rgb(theme.panel))
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(theme.border))
                            .child(
                                theme
                                    .cell_btn(
                                        "comment-menu-item",
                                        if editing {
                                            "Edit comment"
                                        } else {
                                            "Add comment"
                                        },
                                        false,
                                        self.accent(),
                                    )
                                    .debug_selector(|| "comment-menu-item".into())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.start_comment(location.clone(), window);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .into_any_element(),
            );
        }

        let draft = self.comment_draft.as_ref()?;
        let track = self
            .st
            .project
            .tracks
            .iter()
            .find(|track| track.id == draft.location.track_id);
        let name = track.map_or("Track", |track| track.name.as_str());
        let title = draft.location.bar.map_or_else(
            || name.to_owned(),
            |bar| format!("{name} · Bar {}", bar + 1),
        );
        let display = if draft.select_all {
            draft.chars.iter().collect::<String>()
        } else {
            draft.chars[..draft.caret]
                .iter()
                .copied()
                .chain(std::iter::once('▏'))
                .chain(draft.chars[draft.caret..].iter().copied())
                .collect::<String>()
        };
        Some(
            overlay_surface()
                .id("track-comment-editor")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000099))
                .child(
                    overlay_surface()
                        .w(px(510.))
                        .max_w_full()
                        .p_4()
                        .rounded_md()
                        .bg(rgb(theme.panel))
                        .text_color(rgb(theme.text))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_lg().child(title))
                        .child(
                            div()
                                .id("comment-text")
                                .track_focus(&self.comment_focus)
                                .h(px(122.))
                                .overflow_y_scroll()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(self.accent()))
                                .p_2()
                                .bg(rgb(if draft.select_all {
                                    theme.panel_alt
                                } else {
                                    theme.bg
                                }))
                                .child(display)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        this.comment_focus.focus(window);
                                        cx.stop_propagation();
                                    }),
                                ),
                        )
                        .children(
                            draft
                                .error
                                .clone()
                                .map(|error| div().text_color(rgb(theme.record_text)).child(error)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    theme
                                        .cell_btn(
                                            "comment-save",
                                            "Save comment",
                                            true,
                                            self.accent(),
                                        )
                                        .debug_selector(|| "comment-save".into())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.commit_comment(false, window);
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    theme
                                        .cell_btn(
                                            "comment-cancel",
                                            "Cancel",
                                            false,
                                            theme.panel_alt,
                                        )
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.dismiss_comment(window);
                                            cx.notify();
                                        })),
                                )
                                .children((!draft.original.is_empty()).then(|| {
                                    theme
                                        .cell_btn("comment-remove", "Delete comment", false, RECORD)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.commit_comment(true, window);
                                            cx.notify();
                                        }))
                                }))
                                .child(div().flex_1())
                                .child(div().text_color(rgb(theme.text_dim)).child(format!(
                                    "{}/{}",
                                    draft.chars.len(),
                                    MAX_TRACK_COMMENT_CHARS
                                ))),
                        ),
                )
                .into_any_element(),
        )
    }
}

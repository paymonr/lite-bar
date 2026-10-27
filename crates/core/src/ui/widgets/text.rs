//! Single-line text field.

use super::super::theme;
use super::super::{
    tab_key, Clipboard, Cmd, CursorKind, Id, Pt, Rect, TextEdit, TextStyle, Ui, UiKey,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextFieldEvent {
    None,
    /// The text was edited this frame.
    Changed,
    /// Enter or Tab was pressed or the field lost focus.
    Committed,
    /// Esc: the text went back to what it was when the field got focus, and the field
    /// lost focus.
    Cancelled,
}

/// Stands in when the host gave the `Ui` no clipboard.
struct NoClipboard;

impl Clipboard for NoClipboard {
    fn get(&mut self) -> Option<String> {
        None
    }
    fn set(&mut self, _: &str) {}
}

/// Byte offset of the caret in `edit.text`.
fn caret_byte(edit: &TextEdit) -> usize {
    edit.text
        .char_indices()
        .nth(edit.caret)
        .map_or(edit.text.len(), |(b, _)| b)
}

impl Ui<'_> {
    /// A full-width text field (or `next_width`). Clicking focuses it; typed text and
    /// editing keys then go to `edit`. A press anywhere else blurs it and commits.
    pub fn text_field(&mut self, id: Id, edit: &mut TextEdit, placeholder: &str) -> TextFieldEvent {
        let rect = self.alloc(None, theme::CONTROL_HEIGHT);
        self.text_input(id, rect, edit, placeholder, TextStyle::BODY)
    }

    /// Char index in `text` whose left edge is nearest to `x` (relative to the text start).
    fn caret_at(&self, text: &str, x: f32, style: TextStyle) -> usize {
        let mut best = (0, x.abs());
        for (i, (b, c)) in text.char_indices().enumerate() {
            let w = self.measure.measure(&text[..b + c.len_utf8()], style).0;
            let d = (w - x).abs();
            if d < best.1 {
                best = (i + 1, d);
            }
        }
        best.0
    }

    /// The text field behind `text_field` and the color picker's hex box; `rect` is in
    /// screen space.
    pub(super) fn text_input(
        &mut self,
        id: Id,
        rect: Rect,
        edit: &mut TextEdit,
        placeholder: &str,
        style: TextStyle,
    ) -> TextFieldEvent {
        self.focusables.push(id);
        let hot = self.hover(id, rect);
        if hot {
            self.cursor = CursorKind::IBeam;
        }
        let text_x = rect.x + theme::FIELD_PAD_X;
        let mut ev = TextFieldEvent::None;
        if self.input.pressed {
            if hot {
                if self.state.focused != Some(id) {
                    self.state.focus_revert = Some(edit.text.clone());
                }
                self.state.focused = Some(id);
                self.state.focus_fresh = false;
                edit.all_selected = false;
                edit.last_input_ms = self.input.now_ms;
                if let Some(p) = self.input.pointer {
                    edit.caret = self.caret_at(&edit.text, p.x - text_x + edit.scroll, style);
                }
            } else if self.focus_start == Some(id) {
                if self.state.focused == Some(id) {
                    self.state.focused = None;
                }
                ev = TextFieldEvent::Committed;
            }
        }
        if self.state.focused == Some(id) {
            self.focus_seen = true;
            if std::mem::take(&mut self.state.focus_fresh) {
                self.state.focus_revert = Some(edit.text.clone());
            }
            let before = edit.text.clone();
            if !self.input.text.is_empty() || !self.input.keys.is_empty() {
                edit.last_input_ms = self.input.now_ms;
            }
            let mut none = NoClipboard;
            let cb: &mut dyn Clipboard = match self.clipboard.as_deref_mut() {
                Some(c) => c,
                None => &mut none,
            };
            for c in &self.input.text {
                edit.insert(*c);
            }
            for k in &self.input.keys {
                if k.key == UiKey::Enter {
                    ev = TextFieldEvent::Committed;
                    break;
                }
                if let Some(forward) = tab_key(k) {
                    self.tab = Some(forward);
                    ev = TextFieldEvent::Committed;
                    break;
                }
                if k.key == UiKey::Esc {
                    if let Some(text) = self.state.focus_revert.take() {
                        *edit = TextEdit::new(&text);
                    }
                    ev = TextFieldEvent::Cancelled;
                    break;
                }
                edit.apply(k, cb);
            }
            if matches!(ev, TextFieldEvent::Committed | TextFieldEvent::Cancelled) {
                self.state.focused = None;
                self.state.focus_revert = None;
            } else if edit.text != before {
                ev = TextFieldEvent::Changed;
            }
        }
        let focused = self.state.focused == Some(id);
        self.caret_seen |= focused;
        self.scroll_to_caret(rect, edit, style, focused);
        self.draw_text_input(rect, edit, placeholder, style, focused);
        ev
    }

    /// Scrolls a focused field just enough to show the caret; an unfocused one shows the start.
    fn scroll_to_caret(&self, rect: Rect, edit: &mut TextEdit, style: TextStyle, focused: bool) {
        if !focused {
            edit.scroll = 0.0;
            return;
        }
        let room = (rect.w - 2.0 * theme::FIELD_PAD_X).max(0.0);
        let caret_x = self
            .measure
            .measure(&edit.text[..caret_byte(edit)], style)
            .0;
        let text_w = self.measure.measure(&edit.text, style).0;
        let mut scroll = edit.scroll;
        if caret_x - scroll > room {
            scroll = caret_x - room;
        }
        if caret_x < scroll {
            scroll = caret_x;
        }
        edit.scroll = scroll.min((text_w - room).max(0.0)).max(0.0);
    }

    fn draw_text_input(
        &mut self,
        rect: Rect,
        edit: &TextEdit,
        placeholder: &str,
        style: TextStyle,
        focused: bool,
    ) {
        self.cmds.push(Cmd::Fill {
            rect,
            color: theme::BG,
            radius: theme::FIELD_RADIUS,
        });
        let text_x = rect.x + theme::FIELD_PAD_X - edit.scroll;
        let line_h = self.measure.measure("Ag", style).1;
        let text_y = rect.y + (rect.h - line_h) / 2.0;
        let pos = Pt {
            x: text_x,
            y: text_y,
        };
        self.clip(rect.shrink(1.0), |ui| {
            if edit.text.is_empty() {
                ui.cmds.push(Cmd::Text {
                    pos,
                    text: placeholder.to_string(),
                    style,
                    color: theme::PLACEHOLDER,
                });
            } else {
                if focused && edit.all_selected {
                    let w = ui.measure.measure(&edit.text, style).0;
                    ui.cmds.push(Cmd::Fill {
                        rect: Rect::new(text_x, text_y, w, line_h),
                        color: theme::SELECTION,
                        radius: 0.0,
                    });
                }
                ui.cmds.push(Cmd::Text {
                    pos,
                    text: edit.text.clone(),
                    style,
                    color: theme::TEXT,
                });
            }
            let since = ui.input.now_ms.saturating_sub(edit.last_input_ms);
            let blink_on = (since / theme::CARET_BLINK_MS).is_multiple_of(2);
            if focused && blink_on {
                let x = text_x + ui.measure.measure(&edit.text[..caret_byte(edit)], style).0;
                ui.cmds.push(Cmd::Line {
                    from: Pt { x, y: text_y },
                    to: Pt {
                        x,
                        y: text_y + line_h,
                    },
                    color: theme::TEXT,
                    width: 1.0,
                });
            }
        });
        self.cmds.push(Cmd::Stroke {
            rect,
            color: if focused {
                theme::ACCENT
            } else {
                theme::BORDER
            },
            radius: theme::FIELD_RADIUS,
            width: 1.0,
        });
    }
}

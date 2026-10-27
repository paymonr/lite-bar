//! Hotkey capture field.

use super::super::theme::{self, Rgba};
use super::super::{tab_key, Cmd, HotkeyCapture, HotkeyFieldEvent, Id, Pt, Rect, TextStyle, Ui};
use crate::hotkey::Hotkey;

const KBD_STYLE: TextStyle = TextStyle {
    size: 12.0,
    bold: false,
    mono: true,
};
const SEP_GAP: f32 = 4.0;
const WAITING_TEXT: &str = "Press the new keys…";

/// What the field shows.
enum Content<'a> {
    Keys(Vec<String>),
    Message(&'a str, Rgba),
}

fn key_names(h: &Hotkey) -> Vec<String> {
    let mut parts: Vec<String> = [
        (h.ctrl, "Ctrl"),
        (h.alt, "Alt"),
        (h.shift, "Shift"),
        (h.win, "Win"),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .map(|(_, n)| n.to_string())
    .collect();
    parts.push(h.key.to_string());
    parts
}

fn content<'a>(current: &Hotkey, capture: &'a HotkeyCapture) -> Content<'a> {
    match capture {
        HotkeyCapture::Idle => Content::Keys(key_names(current)),
        HotkeyCapture::Waiting => Content::Message(WAITING_TEXT, theme::MUTED),
        HotkeyCapture::Error(msg) | HotkeyCapture::Retry(msg) => {
            Content::Message(msg, theme::ERROR)
        }
    }
}

impl Ui<'_> {
    /// Width `hotkey_field` takes for `current` in this capture state, for laying it
    /// out (e.g. right-aligned) before building it.
    pub fn hotkey_field_width(&self, current: &Hotkey, capture: &HotkeyCapture) -> f32 {
        self.content_width(&content(current, capture))
    }

    fn content_width(&self, content: &Content) -> f32 {
        let inner_w = match content {
            Content::Keys(parts) => {
                let caps: f32 = parts
                    .iter()
                    .map(|p| self.measure.measure(p, KBD_STYLE).0 + 2.0 * theme::KBD_PAD_X)
                    .sum();
                let plus = self.measure.measure("+", TextStyle::BODY).0 + 2.0 * SEP_GAP;
                caps + plus * (parts.len() - 1) as f32
            }
            Content::Message(m, _) => self.measure.measure(m, TextStyle::BODY).0,
        };
        inner_w + 2.0 * theme::KEY_PAD_X
    }

    /// Shows `current` as key caps. Clicking starts capture; the next combination is
    /// proposed, Esc or a press elsewhere cancels. An invalid combination shows why and
    /// keeps listening; `HotkeyCapture::Error` shows its
    /// message until clicked again.
    pub fn hotkey_field(
        &mut self,
        id: Id,
        current: &Hotkey,
        capture: &mut HotkeyCapture,
    ) -> HotkeyFieldEvent {
        let content = content(current, capture);
        let rect = self.alloc(Some(self.content_width(&content)), theme::CONTROL_HEIGHT);
        let border = match &*capture {
            HotkeyCapture::Idle => theme::BORDER,
            HotkeyCapture::Waiting => theme::ACCENT,
            HotkeyCapture::Error(_) | HotkeyCapture::Retry(_) => theme::ERROR,
        };
        self.draw_hotkey(rect, &content, border);

        self.focusables.push(id);
        let (hot, clicked) = self.interact(id, rect);
        let mut ev = HotkeyFieldEvent::None;
        if self.state.focused == Some(id) {
            self.focus_seen = true;
        }
        let tabbed_in =
            self.state.focused == Some(id) && std::mem::take(&mut self.state.focus_fresh);
        if tabbed_in && !capture.is_listening() {
            capture.start();
            ev = HotkeyFieldEvent::Started;
        } else if capture.is_listening() {
            if (self.input.pressed && !hot) || self.input.deactivated {
                *capture = HotkeyCapture::Idle;
                ev = HotkeyFieldEvent::Cancelled;
            } else {
                for k in &self.input.keys {
                    if let Some(forward) = tab_key(k) {
                        // Tab moves on, like from a text field.
                        self.tab = Some(forward);
                        *capture = HotkeyCapture::Idle;
                        ev = HotkeyFieldEvent::Cancelled;
                        break;
                    }
                    ev = capture.feed(k);
                    if ev != HotkeyFieldEvent::None {
                        break;
                    }
                }
            }
        } else if clicked {
            capture.start();
            self.state.focused = Some(id);
            self.focus_seen = true;
            ev = HotkeyFieldEvent::Started;
        }
        self.capturing_keys |= capture.is_listening();
        if !capture.is_listening() && self.state.focused == Some(id) {
            self.state.focused = None;
        }
        ev
    }

    fn draw_hotkey(&mut self, rect: Rect, content: &Content, border: Rgba) {
        self.cmds.push(Cmd::Fill {
            rect,
            color: theme::BG,
            radius: theme::KEY_RADIUS,
        });
        self.cmds.push(Cmd::Stroke {
            rect,
            color: border,
            radius: theme::KEY_RADIUS,
            width: 1.0,
        });
        let inner = Rect::new(
            rect.x + theme::KEY_PAD_X,
            rect.y,
            rect.w - 2.0 * theme::KEY_PAD_X,
            rect.h,
        );
        match content {
            Content::Message(m, color) => {
                let h = self.measure.measure(m, TextStyle::BODY).1;
                self.cmds.push(Cmd::Text {
                    pos: Pt {
                        x: inner.x,
                        y: inner.y + (inner.h - h) / 2.0,
                    },
                    text: m.to_string(),
                    style: TextStyle::BODY,
                    color: *color,
                });
            }
            Content::Keys(parts) => {
                let mut x = inner.x;
                let cap_y = rect.y + (rect.h - theme::KBD_H) / 2.0;
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        let (pw, _) = self.measure.measure("+", TextStyle::BODY);
                        let plus = Rect::new(x, rect.y, pw + 2.0 * SEP_GAP, rect.h);
                        self.text_centered(plus, "+", TextStyle::BODY, theme::MUTED);
                        x = plus.right();
                    }
                    let cw = self.measure.measure(part, KBD_STYLE).0 + 2.0 * theme::KBD_PAD_X;
                    let cap = Rect::new(x, cap_y, cw, theme::KBD_H);
                    self.cmds.push(Cmd::Fill {
                        rect: cap,
                        color: theme::BUTTON_BG,
                        radius: theme::BUTTON_RADIUS,
                    });
                    self.cmds.push(Cmd::Stroke {
                        rect: cap,
                        color: theme::BUTTON_BORDER,
                        radius: theme::BUTTON_RADIUS,
                        width: 1.0,
                    });
                    self.text_centered(cap, part, KBD_STYLE, theme::TEXT);
                    x = cap.right();
                }
            }
        }
    }
}

//! Default chip and two-click confirm button.

use super::super::theme::{self, Rgba};
use super::super::{Cmd, Id, TextStyle, Ui};

const CHIP_STYLE: TextStyle = TextStyle {
    size: 11.0,
    bold: false,
    mono: false,
};

fn dim(c: Rgba) -> Rgba {
    Rgba(c.0, c.1, c.2, theme::DIM_ALPHA)
}

impl Ui<'_> {
    /// A small pill such as "Default 18px". Muted and inert when `at_default`.
    pub fn default_chip(&mut self, id: Id, label: &str, at_default: bool) -> bool {
        let tw = self.measure.measure(label, CHIP_STYLE).0;
        let rect = self.alloc(Some(tw + 2.0 * theme::CHIP_PAD_X), theme::CHIP_H);
        let (hot, clicked) = if at_default {
            (false, false)
        } else {
            self.interact(id, rect)
        };
        let (text, border) = if at_default {
            (dim(theme::MUTED), dim(theme::BORDER))
        } else if hot {
            (theme::TEXT, theme::ACCENT)
        } else {
            (theme::MUTED, theme::BORDER)
        };
        let radius = theme::CHIP_H / 2.0;
        self.cmds.push(Cmd::Fill {
            rect,
            color: theme::BG,
            radius,
        });
        self.cmds.push(Cmd::Stroke {
            rect,
            color: border,
            radius,
            width: 1.0,
        });
        self.text_centered(rect, label, CHIP_STYLE, text);
        clicked
    }

    /// The first click arms it and shows `confirm_text` in the accent style; a second
    /// click within 3 s returns true. Otherwise it disarms. Time is `Input::now_ms`.
    pub fn confirm_button(&mut self, id: Id, text: &str, confirm_text: &str) -> bool {
        let now = self.input.now_ms;
        if let Some(&at) = self.state.armed.get(&id) {
            if now.saturating_sub(at) >= theme::CONFIRM_MS {
                self.state.armed.remove(&id);
            }
        }
        let look = |armed: bool| {
            if armed {
                (confirm_text, TextStyle::BOLD)
            } else {
                (text, TextStyle::BODY)
            }
        };
        let armed = self.state.armed.contains_key(&id);
        let (label, style) = look(armed);
        let tw = self.measure.measure(label, style).0;
        let rect = self.alloc(Some(tw + 2.0 * theme::BUTTON_PAD_X), theme::CONTROL_HEIGHT);
        let (hot, clicked) = self.interact(id, rect);
        if clicked {
            if armed {
                self.state.armed.remove(&id);
            } else {
                self.state.armed.insert(id, now);
            }
        }
        // Drawn in the new state so the confirm text shows on the arming click.
        let armed_at = self.state.armed.get(&id).copied();
        if let Some(at) = armed_at {
            // Redraw once at the deadline so it is seen to disarm.
            self.redraw_at(at + theme::CONFIRM_MS);
        }
        let armed_now = armed_at.is_some();
        let (label, style) = look(armed_now);
        let (bg, border, fg) = if armed_now {
            (theme::ACCENT, theme::ACCENT, theme::ON_ACCENT)
        } else if hot {
            (theme::BUTTON_BG_HOT, theme::BUTTON_BORDER, theme::TEXT)
        } else {
            (theme::BUTTON_BG, theme::BUTTON_BORDER, theme::TEXT)
        };
        self.cmds.push(Cmd::Fill {
            rect,
            color: bg,
            radius: theme::BUTTON_RADIUS,
        });
        self.cmds.push(Cmd::Stroke {
            rect,
            color: border,
            radius: theme::BUTTON_RADIUS,
            width: 1.0,
        });
        self.text_centered(rect, label, style, fg);
        clicked && armed
    }
}

//! Basic widgets: label, button, toggle, slider, segmented.
//!
//! Every widget returns `true` when its value changed or it was clicked.

use std::ops::RangeInclusive;

use super::theme::{self, Rgba};
use super::{Cmd, CursorKind, Id, Pt, Rect, TextStyle, Ui};

impl Ui<'_> {
    /// Marks `rect` hot when the pointer is over it, and active when pressed on it.
    /// Returns whether the pointer is over `rect`.
    fn hover(&mut self, id: Id, rect: Rect) -> bool {
        let over = self.input.pointer.is_some_and(|p| rect.contains(p));
        if over {
            self.state.hot = Some(id);
            self.cursor = CursorKind::Hand;
            if self.input.pressed {
                self.state.active = Some(id);
            }
        }
        over
    }

    /// Click semantics: released while still over the widget that was pressed.
    fn clicked(&mut self, id: Id, rect: Rect) -> bool {
        let over = self.hover(id, rect);
        over && self.input.released && self.state.active == Some(id)
    }

    fn text_centered(&mut self, rect: Rect, text: &str, style: TextStyle, color: Rgba) {
        let (w, h) = self.measure.measure(text, style);
        let pos = Pt {
            x: rect.x + (rect.w - w) / 2.0,
            y: rect.y + (rect.h - h) / 2.0,
        };
        self.cmds.push(Cmd::Text {
            pos,
            text: text.to_string(),
            style,
            color,
        });
    }

    pub fn label(&mut self, text: &str, style: TextStyle, color: Rgba) {
        let (w, h) = self.measure.measure(text, style);
        let rect = self.alloc(Some(w), h);
        let pos = Pt {
            x: rect.x,
            y: rect.y,
        };
        self.cmds.push(Cmd::Text {
            pos,
            text: text.to_string(),
            style,
            color,
        });
    }

    pub fn button(&mut self, id: Id, text: &str) -> bool {
        let (tw, _) = self.measure.measure(text, TextStyle::BODY);
        let rect = self.alloc(Some(tw + 2.0 * theme::BUTTON_PAD_X), theme::CONTROL_HEIGHT);
        let clicked = self.clicked(id, rect);
        let bg = if self.state.active == Some(id) && self.state.hot == Some(id) {
            theme::BUTTON_BG_ACTIVE
        } else if self.state.hot == Some(id) {
            theme::BUTTON_BG_HOT
        } else {
            theme::BUTTON_BG
        };
        self.cmds.push(Cmd::Fill {
            rect,
            color: bg,
            radius: theme::BUTTON_RADIUS,
        });
        self.cmds.push(Cmd::Stroke {
            rect,
            color: theme::BUTTON_BORDER,
            radius: theme::BUTTON_RADIUS,
            width: 1.0,
        });
        self.text_centered(rect, text, TextStyle::BODY, theme::TEXT);
        clicked
    }

    pub fn toggle(&mut self, id: Id, value: &mut bool) -> bool {
        let rect = self.alloc(Some(theme::TOGGLE_W), theme::TOGGLE_H);
        let clicked = self.clicked(id, rect);
        if clicked {
            *value = !*value;
        }
        let track = if *value { theme::ACCENT } else { theme::BORDER };
        self.cmds.push(Cmd::Fill {
            rect,
            color: track,
            radius: theme::TOGGLE_H / 2.0,
        });
        let knob = 14.0;
        let inset = (theme::TOGGLE_H - knob) / 2.0;
        let kx = if *value {
            rect.right() - inset - knob
        } else {
            rect.x + inset
        };
        self.cmds.push(Cmd::Fill {
            rect: Rect::new(kx, rect.y + inset, knob, knob),
            color: theme::TEXT,
            radius: knob / 2.0,
        });
        clicked
    }

    pub fn slider(&mut self, id: Id, value: &mut i64, range: RangeInclusive<i64>) -> bool {
        let (lo, hi) = (*range.start(), *range.end());
        let rect = self.alloc(None, theme::SLIDER_H);
        let half = theme::SLIDER_THUMB / 2.0;
        let (x0, x1) = (rect.x + half, (rect.right() - half).max(rect.x + half));
        if self.hover(id, rect) && self.input.pressed {
            self.state.drag_origin = self.input.pointer.map(|p| p.x);
        }
        let mut changed = false;
        if self.state.active == Some(id) {
            if let Some(p) = self.input.pointer {
                let t = if x1 > x0 {
                    ((p.x - x0) / (x1 - x0)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let new = lo + (t * (hi - lo) as f32).round() as i64;
                if new != *value {
                    *value = new;
                    changed = true;
                }
            }
        }
        let t = if hi > lo {
            (*value - lo) as f32 / (hi - lo) as f32
        } else {
            0.0
        };
        let tx = x0 + t.clamp(0.0, 1.0) * (x1 - x0);
        let cy = rect.y + rect.h / 2.0;
        let track_h = theme::SLIDER_TRACK_H;
        let track = Rect::new(rect.x, cy - track_h / 2.0, rect.w, track_h);
        self.cmds.push(Cmd::Fill {
            rect: track,
            color: theme::BORDER,
            radius: track_h / 2.0,
        });
        let filled = Rect::new(rect.x, track.y, tx - rect.x, track_h);
        self.cmds.push(Cmd::Fill {
            rect: filled,
            color: theme::ACCENT,
            radius: track_h / 2.0,
        });
        self.cmds.push(Cmd::Fill {
            rect: Rect::new(
                tx - half,
                cy - half,
                theme::SLIDER_THUMB,
                theme::SLIDER_THUMB,
            ),
            color: theme::TEXT,
            radius: half,
        });
        changed
    }

    pub fn segmented(&mut self, id: Id, selected: &mut usize, options: &[&str]) -> bool {
        let widths: Vec<f32> = options
            .iter()
            .map(|o| self.measure.measure(o, TextStyle::BODY).0 + 2.0 * theme::SEGMENT_PAD_X)
            .collect();
        let rect = self.alloc(Some(widths.iter().sum()), theme::CONTROL_HEIGHT);
        self.cmds.push(Cmd::Fill {
            rect,
            color: theme::BG,
            radius: theme::SEGMENT_RADIUS,
        });
        let mut changed = false;
        let mut x = rect.x;
        for (i, (opt, w)) in options.iter().zip(&widths).enumerate() {
            let cell = Rect::new(x, rect.y, *w, rect.h);
            x += w;
            if self.clicked(super::id_with(id, i), cell) && *selected != i {
                *selected = i;
                changed = true;
            }
            if *selected == i {
                let inner = cell.shrink(1.0);
                self.cmds.push(Cmd::Fill {
                    rect: inner,
                    color: theme::ACCENT,
                    radius: theme::SEGMENT_RADIUS - 1.0,
                });
                self.text_centered(cell, opt, TextStyle::BOLD, theme::ON_ACCENT);
            } else {
                self.text_centered(cell, opt, TextStyle::BODY, theme::SEGMENT_TEXT);
            }
        }
        self.cmds.push(Cmd::Stroke {
            rect,
            color: theme::BORDER,
            radius: theme::SEGMENT_RADIUS,
            width: 1.0,
        });
        changed
    }
}

//! Navigation: sidebar and scroll area.

use super::super::theme::{self, Rgba};
use super::super::{id_with, Cmd, Id, Pt, Rect, TextStyle, Ui};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SidebarItem<'a> {
    pub label: &'a str,
    /// A sub-item under the previous top-level item.
    pub indent: bool,
    pub enabled: bool,
    /// Shown after the label, e.g. "soon".
    pub note: Option<&'a str>,
}

const SUB_STYLE: TextStyle = TextStyle {
    size: 12.0,
    bold: false,
    mono: false,
};
const NOTE_STYLE: TextStyle = TextStyle {
    size: 11.0,
    bold: false,
    mono: false,
};
const THUMB_W: f32 = 4.0;
const THUMB_MIN_H: f32 = 20.0;

impl<'a> Ui<'a> {
    /// A vertical list of items; clicking an enabled one selects it. A selected
    /// sub-item also highlights the top-level item above it.
    pub fn sidebar(&mut self, id: Id, items: &[SidebarItem], selected: &mut usize) -> bool {
        let height = |it: &SidebarItem| {
            if it.indent {
                theme::NAV_SUB_H
            } else {
                theme::NAV_H
            }
        };
        let total: f32 = items.iter().map(|it| height(it) + theme::NAV_SPACING).sum();
        let area = self.alloc(None, (total - theme::NAV_SPACING).max(0.0));
        let parent_on = match items.get(*selected) {
            Some(it) if it.indent => items[..*selected].iter().rposition(|p| !p.indent),
            _ => Some(*selected),
        };
        let mut changed = false;
        let mut y = area.y;
        for (i, it) in items.iter().enumerate() {
            let rect = Rect::new(area.x, y, area.w, height(it));
            y = rect.bottom() + theme::NAV_SPACING;
            let hot = if it.enabled {
                let (hot, clicked) = self.interact(id_with(id, i), rect);
                if clicked && *selected != i {
                    *selected = i;
                    changed = true;
                }
                hot
            } else {
                false
            };
            let on = if it.indent {
                *selected == i
            } else {
                parent_on == Some(i)
            };
            self.draw_nav_item(rect, it, on, hot);
        }
        changed
    }

    fn draw_nav_item(&mut self, rect: Rect, it: &SidebarItem, on: bool, hot: bool) {
        let top_on = on && !it.indent;
        if top_on || (hot && !on) {
            self.cmds.push(Cmd::Fill {
                rect,
                color: theme::SURFACE,
                radius: theme::NAV_RADIUS,
            });
        }
        if top_on {
            self.cmds.push(Cmd::Fill {
                rect: Rect::new(rect.x, rect.y, 3.0, rect.h),
                color: theme::ACCENT,
                radius: 0.0,
            });
        }
        let (style, pad, color) = if it.indent {
            let c = if on { theme::TEXT } else { theme::MUTED };
            (SUB_STYLE, theme::NAV_SUB_PAD_X, c)
        } else {
            let c = if top_on {
                theme::SIDEBAR_SELECTED_TEXT
            } else {
                theme::SEGMENT_TEXT
            };
            (TextStyle::BODY, theme::NAV_PAD_X, c)
        };
        let color = if it.enabled {
            color
        } else {
            Rgba(color.0, color.1, color.2, theme::DIM_ALPHA)
        };
        let (w, h) = self.measure.measure(it.label, style);
        let x = rect.x + pad;
        self.cmds.push(Cmd::Text {
            pos: Pt {
                x,
                y: rect.y + (rect.h - h) / 2.0,
            },
            text: it.label.to_string(),
            style,
            color,
        });
        if let Some(note) = it.note {
            let nh = self.measure.measure(note, NOTE_STYLE).1;
            self.cmds.push(Cmd::Text {
                pos: Pt {
                    x: x + w + 4.0,
                    y: rect.y + (rect.h - nh) / 2.0,
                },
                text: note.to_string(),
                style: NOTE_STYLE,
                color: theme::PLACEHOLDER,
            });
        }
    }

    /// A full-width viewport `height` tall showing `f`'s content (`content_height` tall)
    /// scrolled by the wheel, 40 DIPs per notch, and clipped to the viewport. Only the
    /// innermost scroll area under the pointer takes the wheel.
    pub fn scroll_area(
        &mut self,
        id: Id,
        height: f32,
        content_height: f32,
        f: impl FnOnce(&mut Ui<'a>),
    ) {
        let local = self.alloc_local(None, height);
        let screen = self.shifted(local);
        let max = (content_height - height).max(0.0);
        let mut scroll = self.state.scroll.get(&id).copied().unwrap_or(0.0);
        if self.state.scroll_hot == Some(id) {
            scroll -= self.input.wheel * theme::SCROLL_STEP;
        }
        let scroll = scroll.clamp(0.0, max);
        self.state.scroll.insert(id, scroll);
        if self.pointer_hits(screen) {
            self.state.scroll_hot_next = Some(id);
        }
        self.clip(screen, |ui| {
            ui.region(local, |ui| ui.offset(0.0, -scroll, f));
        });
        if max > 0.0 {
            let thumb_h = (height * height / content_height)
                .max(THUMB_MIN_H)
                .min(height);
            let y = screen.y + scroll / max * (height - thumb_h);
            self.cmds.push(Cmd::Fill {
                rect: Rect::new(screen.right() - THUMB_W - 2.0, y, THUMB_W, thumb_h),
                color: theme::BORDER,
                radius: THUMB_W / 2.0,
            });
        }
    }

    /// Sets a scroll area's offset; it is clamped when the area is next built.
    pub fn set_scroll(&mut self, id: Id, y: f32) {
        self.state.scroll.insert(id, y);
    }
}

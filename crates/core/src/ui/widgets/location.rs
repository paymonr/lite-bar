//! Overlay location picker: a 16:9 screen outline with eight position buttons.

use super::super::theme;
use super::super::{id_with, Cmd, Id, Rect, Ui};
use crate::settings::Location;

/// Each location with its button's column and row in the 3x3 grid.
const CELLS: [(Location, usize, usize); 8] = [
    (Location::TopLeft, 0, 0),
    (Location::TopCenter, 1, 0),
    (Location::TopRight, 2, 0),
    (Location::CenterLeft, 0, 1),
    (Location::CenterRight, 2, 1),
    (Location::BottomLeft, 0, 2),
    (Location::BottomCenter, 1, 2),
    (Location::BottomRight, 2, 2),
];

impl Ui<'_> {
    /// A 160x90 screen outline with a small button at each of the eight positions;
    /// the selected one is accent-filled.
    pub fn location_picker(&mut self, id: Id, value: &mut Location) -> bool {
        let screen = self.alloc(Some(theme::SCREEN_W), theme::SCREEN_H);
        self.cmds.push(Cmd::Fill {
            rect: screen,
            color: theme::BG,
            radius: theme::SCREEN_RADIUS,
        });
        self.cmds.push(Cmd::Stroke {
            rect: screen,
            color: theme::BORDER,
            radius: theme::SCREEN_RADIUS,
            width: 2.0,
        });
        let b = theme::LOC_BUTTON;
        let inset = theme::LOC_INSET;
        let step_x = (screen.w - 2.0 * inset - b) / 2.0;
        let step_y = (screen.h - 2.0 * inset - b) / 2.0;
        let mut changed = false;
        for (i, (loc, col, row)) in CELLS.iter().enumerate() {
            let rect = Rect::new(
                screen.x + inset + *col as f32 * step_x,
                screen.y + inset + *row as f32 * step_y,
                b,
                b,
            );
            let (hot, clicked) = self.interact(id_with(id, i), rect);
            if clicked && *value != *loc {
                *value = *loc;
                changed = true;
            }
            if *value == *loc {
                self.cmds.push(Cmd::Fill {
                    rect: rect.shrink(-3.0),
                    color: theme::ACCENT_HALO,
                    radius: 6.0,
                });
            }
            let color = if *value == *loc {
                theme::ACCENT
            } else if hot {
                theme::BUTTON_BORDER
            } else {
                theme::BORDER
            };
            self.cmds.push(Cmd::Fill {
                rect,
                color,
                radius: 3.0,
            });
        }
        changed
    }
}

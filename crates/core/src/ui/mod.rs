//! Immediate-mode UI toolkit. Each frame the host builds an `Input`, calls a page
//! function with a `Ui`, and renders the `Frame` of draw commands from `finish`.

mod draw;
mod geom;
mod input;
pub mod theme;
mod widgets;

#[cfg(test)]
mod tests;

pub use draw::{Cmd, CursorKind, Frame, TextMeasure, TextStyle};
pub use geom::{Pt, Rect};
pub use input::{Id, Input, KeyPress, UiKey, UiState};
pub use theme::Rgba;

/// Hashes a name to a widget id (FNV-1a, 64 bit).
pub fn id(name: &str) -> Id {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Derives a child id, for list rows and sub-widgets.
pub fn id_with(base: Id, index: usize) -> Id {
    (base ^ (index as u64).wrapping_add(0x9e37_79b9_7f4a_7c15)).wrapping_mul(0x0000_0100_0000_01b3)
}

struct RowCursor {
    x: f32,
    y: f32,
    h: f32,
}

pub struct Ui<'a> {
    measure: &'a dyn TextMeasure,
    input: &'a Input,
    state: &'a mut UiState,
    cmds: Vec<Cmd>,
    cursor: CursorKind,
    bounds: Rect,
    y: f32,
    row: Option<RowCursor>,
}

impl<'a> Ui<'a> {
    pub fn new(
        measure: &'a dyn TextMeasure,
        input: &'a Input,
        state: &'a mut UiState,
        area: Rect,
    ) -> Ui<'a> {
        state.hot = None;
        Ui {
            measure,
            input,
            state,
            cmds: Vec::new(),
            cursor: CursorKind::Arrow,
            bounds: area,
            y: area.y,
            row: None,
        }
    }

    pub fn id(&self, name: &str) -> Id {
        id(name)
    }

    pub fn id_with(&self, base: Id, index: usize) -> Id {
        id_with(base, index)
    }

    /// Adds vertical space, or horizontal space inside a `row`.
    pub fn space(&mut self, px: f32) {
        match &mut self.row {
            Some(r) => r.x += px,
            None => self.y += px,
        }
    }

    /// Lays out the widgets built by `f` left to right in a strip of `height`,
    /// vertically centred, then moves the cursor below it.
    pub fn row(&mut self, height: f32, f: impl FnOnce(&mut Ui<'a>)) {
        self.row = Some(RowCursor {
            x: self.bounds.x,
            y: self.y,
            h: height,
        });
        f(self);
        self.row = None;
        self.y += height + theme::GAP;
    }

    /// Reserves a rect for the next widget. `w` of `None` fills the remaining width.
    fn alloc(&mut self, w: Option<f32>, h: f32) -> Rect {
        match &mut self.row {
            Some(r) => {
                let w = w.unwrap_or((self.bounds.right() - r.x).max(0.0));
                let rect = Rect::new(r.x, r.y + (r.h - h) / 2.0, w, h);
                r.x += w + theme::GAP;
                rect
            }
            None => {
                let rect = Rect::new(self.bounds.x, self.y, w.unwrap_or(self.bounds.w), h);
                self.y += h + theme::GAP;
                rect
            }
        }
    }

    pub fn finish(self) -> Frame {
        if self.input.released {
            self.state.active = None;
        }
        Frame {
            cmds: self.cmds,
            wants_caret_blink: self.state.focused.is_some(),
            cursor: self.cursor,
        }
    }
}

//! Immediate-mode UI toolkit. Each frame the host builds an `Input`, calls a page
//! function with a `Ui`, and renders the `Frame` of draw commands from `finish`.

mod color;
mod draw;
mod geom;
mod hotkey_capture;
mod input;
mod text_edit;
pub mod theme;
mod widgets;

#[cfg(test)]
mod tests;

pub use color::{wheel_pixels, Hsv};
pub use draw::{Cmd, CursorKind, Frame, TextMeasure, TextStyle};
pub use geom::{Pt, Rect};
pub use hotkey_capture::{HotkeyCapture, HotkeyFieldEvent};
pub use input::{Id, Input, KeyPress, UiKey, UiState, WheelCache};
pub use text_edit::{Clipboard, TextEdit};
pub use theme::Rgba;
pub use widgets::{ColorPickerEvent, SidebarItem, TextFieldEvent, PRESET_COLORS};

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

/// Id used by `Ui::block` so overlays can claim the pointer.
const BLOCKER: Id = 0;

/// One level of layout. Rows place items left to right and centre them vertically;
/// otherwise items stack top to bottom. A gap goes between items, not after the last.
struct Layout {
    bounds: Rect,
    x: f32,
    y: f32,
    row: bool,
    gap_pending: bool,
}

impl Layout {
    fn new(bounds: Rect, row: bool) -> Layout {
        Layout {
            bounds,
            x: bounds.x,
            y: bounds.y,
            row,
            gap_pending: false,
        }
    }
}

pub struct Ui<'a> {
    measure: &'a dyn TextMeasure,
    input: &'a Input,
    state: &'a mut UiState,
    cmds: Vec<Cmd>,
    cursor: CursorKind,
    layouts: Vec<Layout>,
    next_w: Option<f32>,
    /// Content offset (for scrolling) applied to every allocated rect and `block`.
    offset: Pt,
    /// Screen-space clip rects; each entry is already intersected with its parent.
    clips: Vec<Rect>,
    /// DIP to pixel scale of the target, so generated images match the screen. Default 1.
    pub scale: f32,
    /// Used by text fields for copy and paste; without one, paste does nothing.
    pub clipboard: Option<&'a mut dyn text_edit::Clipboard>,
    /// `state.focused` when the frame started.
    focus_start: Option<Id>,
    /// Whether the focused text field was built this frame.
    focus_seen: bool,
    /// Earliest `now_ms` at which a widget needs a frame without input.
    redraw_at: Option<u64>,
}

impl<'a> Ui<'a> {
    /// Starts a frame. One `Ui` per frame; use `row`, `column`, `region` and `clip`
    /// to lay out several areas inside it.
    pub fn new(
        measure: &'a dyn TextMeasure,
        input: &'a Input,
        state: &'a mut UiState,
        area: Rect,
    ) -> Ui<'a> {
        state.hot_next = None;
        state.scroll_hot_next = None;
        let focus_start = state.focused;
        Ui {
            measure,
            input,
            state,
            cmds: Vec::new(),
            cursor: CursorKind::Arrow,
            layouts: vec![Layout::new(area, false)],
            next_w: None,
            offset: Pt::default(),
            clips: Vec::new(),
            scale: 1.0,
            clipboard: None,
            focus_start,
            focus_seen: false,
            redraw_at: None,
        }
    }

    fn top(&mut self) -> &mut Layout {
        self.layouts
            .last_mut()
            .expect("layout stack is never empty")
    }

    /// Adds `px` of space after the previous item, replacing the usual gap.
    pub fn space(&mut self, px: f32) {
        let l = self.top();
        if l.row {
            l.x += px;
        } else {
            l.y += px;
        }
        l.gap_pending = false;
    }

    /// The next widget (or row/column) gets exactly this width.
    pub fn next_width(&mut self, w: f32) {
        self.next_w = Some(w);
    }

    /// Reserves a rect in layout space. `w` of `None` fills the remaining width.
    fn alloc_local(&mut self, w: Option<f32>, h: f32) -> Rect {
        let nw = self.next_w.take();
        let l = self.top();
        if l.gap_pending {
            if l.row {
                l.x += theme::GAP;
            } else {
                l.y += theme::GAP;
            }
        }
        l.gap_pending = true;
        if l.row {
            let w = nw.or(w).unwrap_or((l.bounds.right() - l.x).max(0.0));
            let rect = Rect::new(l.x, l.bounds.y + (l.bounds.h - h) / 2.0, w, h);
            l.x += w;
            rect
        } else {
            let w = nw.or(w).unwrap_or(l.bounds.w);
            let rect = Rect::new(l.bounds.x, l.y, w, h);
            l.y += h;
            rect
        }
    }

    fn shifted(&self, r: Rect) -> Rect {
        Rect::new(r.x + self.offset.x, r.y + self.offset.y, r.w, r.h)
    }

    /// Reserves a rect for the next widget, in screen space (offset applied).
    fn alloc(&mut self, w: Option<f32>, h: f32) -> Rect {
        let r = self.alloc_local(w, h);
        self.shifted(r)
    }

    /// Width the next widget would get if it asked to fill: `next_width` if set,
    /// otherwise the rest of the row or the column's width.
    fn avail_width(&self) -> f32 {
        if let Some(w) = self.next_w {
            return w;
        }
        let l = self.layouts.last().expect("layout stack is never empty");
        if l.row {
            let gap = if l.gap_pending { theme::GAP } else { 0.0 };
            (l.bounds.right() - l.x - gap).max(0.0)
        } else {
            l.bounds.w
        }
    }

    fn with_layout(&mut self, bounds: Rect, row: bool, f: impl FnOnce(&mut Ui<'a>)) {
        // A pending width was meant for a widget in the outer layout.
        self.next_w = None;
        self.layouts.push(Layout::new(bounds, row));
        f(self);
        self.layouts.pop();
    }

    /// Lays out the widgets built by `f` left to right in a strip of `height`,
    /// vertically centred. Rows nest.
    pub fn row(&mut self, height: f32, f: impl FnOnce(&mut Ui<'a>)) {
        let rect = self.alloc_local(None, height);
        self.with_layout(rect, true, f);
    }

    /// A vertical stack inside a row, as tall as the row. Give it a width with
    /// `next_width`, or it fills the rest of the row.
    pub fn column(&mut self, f: impl FnOnce(&mut Ui<'a>)) {
        let l = self.top();
        let h = if l.row {
            l.bounds.h
        } else {
            (l.bounds.bottom() - l.y).max(0.0)
        };
        let rect = self.alloc_local(None, h);
        self.with_layout(rect, false, f);
    }

    /// Runs `f` with vertical layout confined to `rect` (layout space). The outer
    /// cursor does not move.
    pub fn region(&mut self, rect: Rect, f: impl FnOnce(&mut Ui<'a>)) {
        self.with_layout(rect, false, f);
    }

    /// Same as `region`; reads better when pinning something to a fixed position.
    pub fn place(&mut self, rect: Rect, f: impl FnOnce(&mut Ui<'a>)) {
        self.region(rect, f);
    }

    /// Pins a row of widgets of total `width` to the right edge of the current row
    /// (or of a new control-height strip when in a column). Does not consume row space.
    pub fn right_aligned(&mut self, width: f32, f: impl FnOnce(&mut Ui<'a>)) {
        self.next_w = None;
        let l = self.top();
        let strip = if l.row {
            l.bounds
        } else {
            self.alloc_local(None, theme::CONTROL_HEIGHT)
        };
        let rect = Rect::new(strip.right() - width, strip.y, width, strip.h);
        self.with_layout(rect, true, f);
    }

    /// Shifts everything laid out inside `f` by (dx, dy), for scrolling content.
    /// Hit tests move with the drawing.
    pub fn offset(&mut self, dx: f32, dy: f32, f: impl FnOnce(&mut Ui<'a>)) {
        self.offset.x += dx;
        self.offset.y += dy;
        f(self);
        self.offset.x -= dx;
        self.offset.y -= dy;
    }

    /// Clips drawing inside `f` to `rect` (screen space) and stops widgets outside it
    /// from reacting to the pointer.
    pub fn clip(&mut self, rect: Rect, f: impl FnOnce(&mut Ui<'a>)) {
        let eff = self.clips.last().map_or(rect, |c| c.intersect(rect));
        self.cmds.push(Cmd::ClipPush(rect));
        self.clips.push(eff);
        f(self);
        self.clips.pop();
        self.cmds.push(Cmd::ClipPop);
    }

    /// Claims the pointer over `rect` (layout space) for anything registered before
    /// it this frame, so overlays and popovers hide widgets beneath from hover and clicks.
    pub fn block(&mut self, rect: Rect) {
        let r = self.shifted(rect);
        if self.pointer_hits(r) {
            self.state.hot_next = Some(BLOCKER);
        }
    }

    /// Whether the pointer is over `rect` and inside the current clip.
    fn pointer_hits(&self, rect: Rect) -> bool {
        self.input
            .pointer
            .is_some_and(|p| rect.contains(p) && self.clips.last().is_none_or(|c| c.contains(p)))
    }

    /// Asks the host for a frame at `now_ms` time `at`, even without input.
    fn redraw_at(&mut self, at: u64) {
        self.redraw_at = Some(self.redraw_at.map_or(at, |r| r.min(at)));
    }

    pub fn finish(self) -> Frame {
        self.state.hot = self.state.hot_next.take();
        self.state.scroll_hot = self.state.scroll_hot_next.take();
        if !self.focus_seen {
            // The focused field was not built (e.g. the page changed), so nothing can
            // type into it or blink its caret.
            self.state.focused = None;
        }
        if self.input.released {
            self.state.active = None;
        }
        Frame {
            cmds: self.cmds,
            wants_caret_blink: self.state.focused.is_some(),
            cursor: self.cursor,
            redraw_after_ms: self.redraw_at,
        }
    }
}

//! Geometry in DIPs (device-independent pixels); the host multiplies by the DPI scale.

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// Half-open: includes the left/top edge, excludes the right/bottom edge.
    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    /// Insets every side by `d` (width and height never go negative).
    pub fn shrink(&self, d: f32) -> Rect {
        Rect {
            x: self.x + d,
            y: self.y + d,
            w: (self.w - 2.0 * d).max(0.0),
            h: (self.h - 2.0 * d).max(0.0),
        }
    }

    /// Splits off a left part of width `w` (clamped to the rect); returns (left, rest).
    pub fn split_left(&self, w: f32) -> (Rect, Rect) {
        let w = w.clamp(0.0, self.w);
        (
            Rect::new(self.x, self.y, w, self.h),
            Rect::new(self.x + w, self.y, self.w - w, self.h),
        )
    }
}

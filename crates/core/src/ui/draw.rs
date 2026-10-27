//! Draw commands and text measuring. The host renders `Cmd`s with Direct2D.

use std::rc::Rc;

use super::geom::{Pt, Rect};
use super::theme::Rgba;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TextStyle {
    pub size: f32,
    pub bold: bool,
    pub mono: bool,
}

impl TextStyle {
    pub const BODY: TextStyle = TextStyle {
        size: 13.0,
        bold: false,
        mono: false,
    };
    pub const BOLD: TextStyle = TextStyle {
        size: 13.0,
        bold: true,
        mono: false,
    };
}

pub trait TextMeasure {
    /// Returns (width, height) in DIPs.
    fn measure(&self, text: &str, style: TextStyle) -> (f32, f32);
}

#[derive(Clone, PartialEq, Debug)]
pub enum Cmd {
    Fill {
        rect: Rect,
        color: Rgba,
        radius: f32,
    },
    Stroke {
        rect: Rect,
        color: Rgba,
        radius: f32,
        width: f32,
    },
    /// `pos` is the top-left of the text box.
    Text {
        pos: Pt,
        text: String,
        style: TextStyle,
        color: Rgba,
    },
    Line {
        from: Pt,
        to: Pt,
        color: Rgba,
        width: f32,
    },
    /// A 1 DIP dashed outline.
    Dashed {
        rect: Rect,
        color: Rgba,
        radius: f32,
    },
    /// RGBA pixels with premultiplied alpha, stretched to `rect`. The host may cache
    /// the uploaded bitmap per `Rc` while it stays alive.
    Image {
        rect: Rect,
        width_px: u32,
        height_px: u32,
        rgba: Rc<Vec<u8>>,
    },
    ClipPush(Rect),
    ClipPop,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CursorKind {
    #[default]
    Arrow,
    Hand,
    IBeam,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Frame {
    pub cmds: Vec<Cmd>,
    pub wants_caret_blink: bool,
    pub cursor: CursorKind,
    /// An `Input::now_ms` time at which the host should run one more frame even
    /// without input, e.g. to show an armed confirm button disarming.
    pub redraw_after_ms: Option<u64>,
}

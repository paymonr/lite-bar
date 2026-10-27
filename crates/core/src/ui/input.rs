//! Per-frame input and the state that persists across frames.

use std::collections::HashMap;

use super::geom::Pt;

pub type Id = u64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiKey {
    Tab,
    Enter,
    Esc,
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    A,
    C,
    V,
    /// Any other virtual-key code.
    Other(u16),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeyPress {
    pub key: UiKey,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Input {
    pub pointer: Option<Pt>,
    pub down: bool,
    /// Went down this frame.
    pub pressed: bool,
    /// Went up this frame.
    pub released: bool,
    pub wheel: f32,
    pub keys: Vec<KeyPress>,
    pub text: Vec<char>,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct UiState {
    /// Topmost widget under the pointer in the previous frame; used for visuals and clicks.
    pub hot: Option<Id>,
    /// Topmost widget under the pointer so far this frame; becomes `hot` at `finish`.
    pub hot_next: Option<Id>,
    pub active: Option<Id>,
    pub focused: Option<Id>,
    pub scroll: HashMap<Id, f32>,
}

//! Per-frame input and the state that persists across frames.

use std::collections::HashMap;
use std::rc::Rc;

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
    /// The right button went up this frame.
    pub right_released: bool,
    /// Wheel notches this frame; positive scrolls up (away from the user).
    pub wheel: f32,
    pub keys: Vec<KeyPress>,
    pub text: Vec<char>,
    /// Monotonic clock in milliseconds, for the caret blink and timed widgets.
    pub now_ms: u64,
}

/// The color wheel's last generated pixels, keyed by size and quantised brightness.
#[derive(Clone, PartialEq, Debug)]
pub struct WheelCache {
    pub diameter_px: u32,
    /// Brightness in 1/256 steps.
    pub v_step: u16,
    pub rgba: Rc<Vec<u8>>,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct UiState {
    /// Topmost widget under the pointer in the previous frame; used for visuals and clicks.
    pub hot: Option<Id>,
    /// Topmost widget under the pointer so far this frame; becomes `hot` at `finish`.
    pub hot_next: Option<Id>,
    pub active: Option<Id>,
    /// The focused text field; typed text and keys go only to it.
    pub focused: Option<Id>,
    pub scroll: HashMap<Id, f32>,
    /// Innermost scroll area under the pointer in the previous frame; only it takes the wheel.
    pub scroll_hot: Option<Id>,
    pub scroll_hot_next: Option<Id>,
    /// Armed confirm buttons and when they were armed (`Input::now_ms`).
    pub armed: HashMap<Id, u64>,
    pub wheel_cache: Option<WheelCache>,
}

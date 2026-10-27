//! Colors and sizes. Palette values come from the approved mockup.

use crate::color::Rgb;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

impl From<Rgb> for Rgba {
    fn from(c: Rgb) -> Rgba {
        Rgba(c.r, c.g, c.b, 255)
    }
}

pub const BG: Rgba = Rgba(0x1C, 0x1E, 0x24, 255);
pub const SURFACE: Rgba = Rgba(0x26, 0x29, 0x31, 255);
pub const BORDER: Rgba = Rgba(0x3A, 0x3E, 0x48, 255);
pub const TEXT: Rgba = Rgba(0xE6, 0xE6, 0xE6, 255);
pub const MUTED: Rgba = Rgba(0x9A, 0xA3, 0xAD, 255);
pub const ACCENT: Rgba = Rgba(0x5F, 0xA8, 0xFF, 255);

pub const BUTTON_BG: Rgba = Rgba(0x30, 0x34, 0x3D, 255);
pub const BUTTON_BG_HOT: Rgba = Rgba(0x3A, 0x3F, 0x4A, 255);
pub const BUTTON_BG_ACTIVE: Rgba = Rgba(0x28, 0x2B, 0x33, 255);
pub const BUTTON_BORDER: Rgba = Rgba(0x44, 0x4A, 0x55, 255);
/// Text on top of the accent color.
pub const ON_ACCENT: Rgba = Rgba(0x0D, 0x14, 0x20, 255);
pub const SEGMENT_TEXT: Rgba = Rgba(0xCF, 0xD5, 0xDC, 255);

pub const BUTTON_RADIUS: f32 = 4.0;
pub const SEGMENT_RADIUS: f32 = 6.0;
pub const CONTROL_HEIGHT: f32 = 28.0;
pub const BUTTON_PAD_X: f32 = 12.0;
pub const SEGMENT_PAD_X: f32 = 14.0;
pub const TOGGLE_W: f32 = 38.0;
pub const TOGGLE_H: f32 = 20.0;
pub const TOGGLE_KNOB: f32 = 14.0;
pub const SLIDER_H: f32 = 20.0;
pub const SLIDER_TRACK_H: f32 = 4.0;
pub const SLIDER_THUMB: f32 = 14.0;
/// Vertical and horizontal spacing between laid-out widgets.
pub const GAP: f32 = 8.0;

pub const PLACEHOLDER: Rgba = Rgba(0x5D, 0x64, 0x6E, 255);
pub const SELECTION: Rgba = Rgba(0x5F, 0xA8, 0xFF, 0x60);
pub const ERROR: Rgba = Rgba(0xFF, 0x55, 0x55, 255);
/// Accent halo around the selected location button.
pub const ACCENT_HALO: Rgba = Rgba(0x5F, 0xA8, 0xFF, 0x40);
pub const SIDEBAR_SELECTED_TEXT: Rgba = Rgba(0xFF, 0xFF, 0xFF, 255);
/// Muted look (45% opacity) for inactive chips and disabled sidebar items.
pub const DIM_ALPHA: u8 = 115;

pub const FIELD_RADIUS: f32 = 5.0;
pub const FIELD_PAD_X: f32 = 8.0;
pub const SWATCH: f32 = 18.0;
pub const SWATCH_GAP: f32 = 6.0;
pub const SWATCH_RADIUS: f32 = 4.0;
pub const HEX_W: f32 = 76.0;
pub const HEX_H: f32 = 24.0;
pub const SCREEN_W: f32 = 160.0;
pub const SCREEN_H: f32 = 90.0;
pub const SCREEN_RADIUS: f32 = 6.0;
pub const LOC_BUTTON: f32 = 14.0;
pub const LOC_INSET: f32 = 8.0;
pub const SCROLL_STEP: f32 = 40.0;
pub const NAV_H: f32 = 32.0;
pub const NAV_SUB_H: f32 = 26.0;
pub const NAV_SPACING: f32 = 2.0;
pub const NAV_RADIUS: f32 = 6.0;
pub const NAV_PAD_X: f32 = 10.0;
pub const NAV_SUB_PAD_X: f32 = 30.0;
pub const CHIP_H: f32 = 18.0;
pub const CHIP_PAD_X: f32 = 8.0;
pub const KEY_RADIUS: f32 = 6.0;
pub const KEY_PAD_X: f32 = 8.0;
pub const KBD_H: f32 = 18.0;
pub const KBD_PAD_X: f32 = 6.0;
pub const CONFIRM_MS: u64 = 3000;
pub const CARET_BLINK_MS: u64 = 500;

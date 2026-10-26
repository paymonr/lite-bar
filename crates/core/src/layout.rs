//! Overlay placement and FPS text formatting. Pure logic, no Windows types.

use crate::settings::Location;

/// Overlay margin from the screen corner at 100% DPI. The app multiplies it by the DPI scale.
pub const BASE_MARGIN_PX: i32 = 10;

/// Screen rectangle in virtual-desktop coordinates. `right` and `bottom` are exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub w: i32,
    pub h: i32,
}

/// Top-left corner of a `size` box placed `margin` px inside `location` of `area`.
/// Centered axes ignore the margin and use integer division.
pub fn overlay_origin(location: Location, area: Rect, size: Size, margin: i32) -> (i32, i32) {
    let left = area.left + margin;
    let top = area.top + margin;
    let right = area.right - margin - size.w;
    let bottom = area.bottom - margin - size.h;
    let center_x = area.left + (area.right - area.left - size.w) / 2;
    let center_y = area.top + (area.bottom - area.top - size.h) / 2;
    match location {
        Location::TopLeft => (left, top),
        Location::TopCenter => (center_x, top),
        Location::TopRight => (right, top),
        Location::CenterLeft => (left, center_y),
        Location::CenterRight => (right, center_y),
        Location::BottomLeft => (left, bottom),
        Location::BottomCenter => (center_x, bottom),
        Location::BottomRight => (right, bottom),
    }
}

/// FPS as display text: rounded to the nearest integer, clamped to 0..=9999.
/// `None` (no presenting app) is an em dash; NaN displays as "0".
pub fn fps_text(fps: Option<f64>) -> String {
    match fps {
        None => "—".to_string(),
        Some(v) if v.is_nan() => "0".to_string(),
        Some(v) => (v.round().clamp(0.0, 9999.0) as u32).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        left: 1920,
        top: 0,
        right: 3840,
        bottom: 1080,
    }; // second monitor
    const SZ: Size = Size { w: 60, h: 30 };

    #[test]
    fn corners() {
        assert_eq!(overlay_origin(Location::TopLeft, AREA, SZ, 10), (1930, 10));
        assert_eq!(overlay_origin(Location::TopRight, AREA, SZ, 10), (3770, 10));
        assert_eq!(
            overlay_origin(Location::BottomLeft, AREA, SZ, 10),
            (1930, 1040)
        );
        assert_eq!(
            overlay_origin(Location::BottomRight, AREA, SZ, 10),
            (3770, 1040)
        );
    }

    #[test]
    fn centered_locations() {
        assert_eq!(
            overlay_origin(Location::TopCenter, AREA, SZ, 10),
            (2850, 10)
        );
        assert_eq!(
            overlay_origin(Location::CenterLeft, AREA, SZ, 10),
            (1930, 525)
        );
        assert_eq!(
            overlay_origin(Location::CenterRight, AREA, SZ, 10),
            (3770, 525)
        );
        assert_eq!(
            overlay_origin(Location::BottomCenter, AREA, SZ, 10),
            (2850, 1040)
        );
    }

    #[test]
    fn negative_coordinates_monitor_left_of_primary() {
        let a = Rect {
            left: -2560,
            top: -200,
            right: 0,
            bottom: 1240,
        };
        assert_eq!(overlay_origin(Location::TopLeft, a, SZ, 10), (-2550, -190));
    }

    #[test]
    fn fps_text_formats() {
        assert_eq!(fps_text(Some(143.6)), "144");
        assert_eq!(fps_text(Some(0.4)), "0");
        assert_eq!(fps_text(None), "—");
        assert_eq!(fps_text(Some(123456.0)), "9999");
    }

    #[test]
    fn fps_text_rounds_half_away_from_zero_and_handles_invalid_input() {
        assert_eq!(fps_text(Some(2.5)), "3");
        assert_eq!(fps_text(Some(-0.4)), "0");
        assert_eq!(fps_text(Some(-30.0)), "0");
        assert_eq!(fps_text(Some(f64::NAN)), "0");
        assert_eq!(fps_text(Some(f64::INFINITY)), "9999");
        assert_eq!(fps_text(Some(9999.4)), "9999");
    }
}

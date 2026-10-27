use super::*;
use crate::settings::Location;

#[test]
fn location_buttons_set_matching_location() {
    // 160x90 screen at (0, 0); 14 DIP buttons at x 8/73/138 and y 8/38/68.
    let cases = [
        (Location::TopLeft, 15.0, 15.0),
        (Location::TopCenter, 80.0, 15.0),
        (Location::TopRight, 145.0, 15.0),
        (Location::CenterLeft, 15.0, 45.0),
        (Location::CenterRight, 145.0, 45.0),
        (Location::BottomLeft, 15.0, 75.0),
        (Location::BottomCenter, 80.0, 75.0),
        (Location::BottomRight, 145.0, 75.0),
    ];
    for (want, x, y) in cases {
        let mut st = UiState::default();
        let mut loc = if want == Location::TopLeft {
            Location::BottomRight
        } else {
            Location::TopLeft
        };
        assert!(
            click_at(&mut st, x, y, |ui| ui.location_picker(id("loc"), &mut loc)),
            "{want:?}"
        );
        assert_eq!(loc, want);
    }
}

#[test]
fn location_hit_targets_are_larger_than_the_buttons() {
    // Top-left is drawn at 8..22; its target reaches 5 DIPs further, to 3..27.
    for (x, y) in [(4.0, 4.0), (26.0, 26.0)] {
        let mut st = UiState::default();
        let mut loc = Location::BottomRight;
        assert!(
            click_at(&mut st, x, y, |ui| ui.location_picker(id("loc"), &mut loc)),
            "({x}, {y})"
        );
        assert_eq!(loc, Location::TopLeft);
    }
    // The gap between targets (27..33 in y) selects nothing.
    let mut st = UiState::default();
    let mut loc = Location::BottomRight;
    assert!(!click_at(&mut st, 15.0, 30.0, |ui| ui
        .location_picker(id("loc"), &mut loc)));
}

#[test]
fn clicking_the_selected_location_is_not_a_change() {
    let mut st = UiState::default();
    let mut loc = Location::TopLeft;
    assert!(!click_at(&mut st, 15.0, 15.0, |ui| ui
        .location_picker(id("loc"), &mut loc)));
}

#[test]
fn selected_location_is_accent_filled() {
    let mut st = UiState::default();
    let mut loc = Location::CenterRight;
    let f = frame(&mut st, &Input::default(), |ui| {
        ui.location_picker(id("loc"), &mut loc);
    });
    let accent: Vec<Rect> = fills(&f)
        .into_iter()
        .filter(|(_, c)| *c == theme::ACCENT)
        .map(|(r, _)| r)
        .collect();
    assert_eq!(accent, vec![Rect::new(138.0, 38.0, 14.0, 14.0)]);
}

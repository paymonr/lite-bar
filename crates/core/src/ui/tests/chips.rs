use super::*;

#[test]
fn default_chip_inactive_at_default() {
    // "Default 18px" is 84 wide, so the chip is 100 x 18 at (0, 0).
    let mut st = UiState::default();
    assert!(!click_at(&mut st, 10.0, 9.0, |ui| ui.default_chip(
        id("d"),
        "Default 18px",
        true
    )));
    let f = frame(&mut st, &hover(10.0, 9.0), |ui| {
        ui.default_chip(id("d"), "Default 18px", true);
    });
    assert_eq!(f.cursor, CursorKind::Arrow);
    assert!(click_at(&mut st, 10.0, 9.0, |ui| ui.default_chip(
        id("d"),
        "Default 18px",
        false
    )));
}

fn click_confirm_at(st: &mut UiState, now_ms: u64) -> (bool, Frame) {
    let build =
        |ui: &mut Ui| ui.confirm_button(id("r"), "Restore defaults", "Click again to confirm");
    for input in [hover(10.0, 10.0), press(10.0, 10.0)] {
        frame(st, &Input { now_ms, ..input }, |ui| {
            build(ui);
        });
    }
    let mut clicked = false;
    let f = frame(
        st,
        &Input {
            now_ms,
            ..release(10.0, 10.0)
        },
        |ui| clicked = build(ui),
    );
    (clicked, f)
}

#[test]
fn confirm_button_needs_second_click_within_3s() {
    let mut st = UiState::default();
    frame(&mut st, &hover(10.0, 10.0), |ui| {
        ui.confirm_button(id("r"), "Restore defaults", "Click again to confirm");
    });
    let (clicked, f) = click_confirm_at(&mut st, 1_000);
    assert!(!clicked);
    assert!(texts(&f).contains(&"Click again to confirm".to_string()));
    assert!(fills(&f).iter().any(|(_, c)| *c == theme::ACCENT));
    assert!(click_confirm_at(&mut st, 2_500).0);
    // After confirming it is back to normal.
    let f = frame(&mut st, &Input::default(), |ui| {
        ui.confirm_button(id("r"), "Restore defaults", "Click again to confirm");
    });
    assert!(texts(&f).contains(&"Restore defaults".to_string()));
    // A second click after 3 s just arms it again.
    assert!(!click_confirm_at(&mut st, 10_000).0);
    assert!(!click_confirm_at(&mut st, 13_000).0);
    assert!(click_confirm_at(&mut st, 13_500).0);
}

#[test]
fn armed_confirm_button_asks_for_a_redraw_at_its_deadline() {
    let mut st = UiState::default();
    frame(&mut st, &hover(10.0, 10.0), |ui| {
        ui.confirm_button(id("r"), "Restore defaults", "Click again to confirm");
    });
    let (_, f) = click_confirm_at(&mut st, 1_000);
    assert_eq!(f.redraw_after_ms, Some(4_000));
    let (confirmed, f) = click_confirm_at(&mut st, 2_000);
    assert!(confirmed);
    assert_eq!(f.redraw_after_ms, None);
}

#[test]
fn confirm_button_disarms_after_3s_without_a_click() {
    let mut st = UiState::default();
    frame(&mut st, &hover(10.0, 10.0), |ui| {
        ui.confirm_button(id("r"), "Restore defaults", "Click again to confirm");
    });
    click_confirm_at(&mut st, 1_000);
    let f = frame(
        &mut st,
        &Input {
            now_ms: 4_000,
            ..Input::default()
        },
        |ui| {
            ui.confirm_button(id("r"), "Restore defaults", "Click again to confirm");
        },
    );
    assert!(texts(&f).contains(&"Restore defaults".to_string()));
}

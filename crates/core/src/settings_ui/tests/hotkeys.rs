use super::*;

#[test]
fn hotkey_capture_emits_proposal() {
    let mut p = Page::new(TALL);
    p.click_text("Ctrl");
    let a = p.keys(vec![KeyPress {
        ctrl: true,
        alt: true,
        ..key(UiKey::Other(0x4B))
    }]);
    assert_eq!(
        a,
        vec![UiAction::ProposeHotkey(
            Hotkey::new(true, true, false, false, Key::Letter('K')).unwrap()
        )]
    );
}

#[test]
fn esc_cancels_hotkey_capture() {
    let mut p = Page::new(TALL);
    p.click_text("Ctrl");
    let a = p.keys(vec![key(UiKey::Esc)]);
    assert_eq!(a, vec![UiAction::CancelHotkeyCapture]);
}

#[test]
fn hotkey_status_shows_under_the_field() {
    let mut p = Page::new(TALL);
    p.status = Some("Saved".to_string());
    p.idle();
    let field_y = find_text(p.frame(), "Ctrl").unwrap().0.y;
    let (pos, _, color) = find_text(p.frame(), "Saved").unwrap();
    assert_eq!(color, theme::MUTED);
    assert!(pos.y > field_y);
    p.status = Some("Already used by another app".to_string());
    p.idle();
    let (_, _, color) = find_text(p.frame(), "Already used by another app").unwrap();
    assert_eq!(color, Rgba(0xFF, 0x8A, 0x80, 255));
}

#[test]
fn starting_capture_asks_the_app_to_free_the_hotkey() {
    let mut p = Page::new(TALL);
    assert_eq!(p.click_text("Ctrl"), vec![UiAction::StartHotkeyCapture]);
}

#[test]
fn hotkey_restore_failure_shows_as_a_problem() {
    let mut p = Page::new(TALL);
    let msg = "Couldn't restore Ctrl+Alt+Shift+L — another app took it";
    p.status = Some(msg.to_string());
    p.idle();
    let (_, _, color) = find_text(p.frame(), msg).unwrap();
    assert_eq!(color, Rgba(0xFF, 0x8A, 0x80, 255));
}

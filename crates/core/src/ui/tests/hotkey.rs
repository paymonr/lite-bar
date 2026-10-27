use super::*;
use crate::hotkey::Hotkey;

fn current() -> Hotkey {
    "Ctrl+Alt+Shift+L".parse().unwrap()
}

fn run(st: &mut UiState, input: &Input, cap: &mut HotkeyCapture) -> (HotkeyFieldEvent, Frame) {
    let mut ev = HotkeyFieldEvent::None;
    let cur = current();
    let f = frame(st, input, |ui| ev = ui.hotkey_field(id("hk"), &cur, cap));
    (ev, f)
}

fn waiting() -> (UiState, HotkeyCapture) {
    let mut st = UiState::default();
    let mut cap = HotkeyCapture::Idle;
    for input in [hover(10.0, 14.0), hover(10.0, 14.0), press(10.0, 14.0)] {
        run(&mut st, &input, &mut cap);
    }
    run(&mut st, &release(10.0, 14.0), &mut cap);
    assert_eq!(cap, HotkeyCapture::Waiting);
    (st, cap)
}

fn kp(vk: u16, ctrl: bool, alt: bool, shift: bool) -> KeyPress {
    KeyPress {
        key: UiKey::Other(vk),
        ctrl,
        alt,
        shift,
        win: false,
    }
}

#[test]
fn hotkey_field_shows_the_current_keys() {
    let mut st = UiState::default();
    let (_, f) = run(&mut st, &Input::default(), &mut HotkeyCapture::Idle);
    let t = texts(&f);
    for part in ["Ctrl", "Alt", "Shift", "L"] {
        assert!(t.contains(&part.to_string()), "{t:?}");
    }
}

#[test]
fn ctrl_alt_k_is_proposed() {
    let (mut st, mut cap) = waiting();
    let (ev, _) = run(
        &mut st,
        &with_keys(vec![kp(0x4B, true, true, false)]),
        &mut cap,
    );
    assert_eq!(
        ev,
        HotkeyFieldEvent::Proposed("Ctrl+Alt+K".parse().unwrap())
    );
    assert_eq!(cap, HotkeyCapture::Idle);
}

#[test]
fn modifiers_alone_keep_waiting_then_shift_a_is_invalid() {
    let (mut st, mut cap) = waiting();
    let (ev, _) = run(
        &mut st,
        &with_keys(vec![kp(0x10, false, false, true)]),
        &mut cap,
    );
    assert_eq!(ev, HotkeyFieldEvent::None);
    let (ev, _) = run(
        &mut st,
        &with_keys(vec![kp(0x41, false, false, true)]),
        &mut cap,
    );
    assert!(matches!(ev, HotkeyFieldEvent::Invalid(_)));
    assert!(matches!(cap, HotkeyCapture::Retry(_)));
}

#[test]
fn field_keeps_listening_after_invalid_and_shows_why() {
    let (mut st, mut cap) = waiting();
    run(
        &mut st,
        &with_keys(vec![kp(0x41, false, false, true)]),
        &mut cap,
    );
    let (_, f) = run(&mut st, &Input::default(), &mut cap);
    assert!(texts(&f)
        .iter()
        .any(|t| t == crate::ui::hotkey_capture::NEEDS_MODIFIER));
    let (ev, _) = run(
        &mut st,
        &with_keys(vec![kp(0x4B, true, true, false)]),
        &mut cap,
    );
    assert_eq!(
        ev,
        HotkeyFieldEvent::Proposed("Ctrl+Alt+K".parse().unwrap())
    );
}

#[test]
fn pressing_elsewhere_cancels_a_retry() {
    let (mut st, mut cap) = waiting();
    run(
        &mut st,
        &with_keys(vec![kp(0x41, false, false, true)]),
        &mut cap,
    );
    let (ev, _) = run(&mut st, &press(100.0, 150.0), &mut cap);
    assert_eq!(ev, HotkeyFieldEvent::Cancelled);
    assert_eq!(cap, HotkeyCapture::Idle);
}

#[test]
fn esc_cancels() {
    let (mut st, mut cap) = waiting();
    let (ev, _) = run(&mut st, &with_keys(vec![key(UiKey::Esc)]), &mut cap);
    assert_eq!(ev, HotkeyFieldEvent::Cancelled);
    assert_eq!(cap, HotkeyCapture::Idle);
}

#[test]
fn pressing_elsewhere_cancels() {
    let (mut st, mut cap) = waiting();
    let (ev, _) = run(&mut st, &press(100.0, 150.0), &mut cap);
    assert_eq!(ev, HotkeyFieldEvent::Cancelled);
    assert_eq!(cap, HotkeyCapture::Idle);
}

#[test]
fn keys_are_ignored_while_idle() {
    let mut st = UiState::default();
    let mut cap = HotkeyCapture::Idle;
    let (ev, _) = run(
        &mut st,
        &with_keys(vec![kp(0x4B, true, true, false)]),
        &mut cap,
    );
    assert_eq!(ev, HotkeyFieldEvent::None);
}

#[test]
fn rejected_shows_the_error_and_click_retries() {
    let mut st = UiState::default();
    let mut cap = HotkeyCapture::rejected("Already used by another app");
    let (_, f) = run(&mut st, &Input::default(), &mut cap);
    assert!(texts(&f).contains(&"Already used by another app".to_string()));
    for input in [hover(10.0, 14.0), hover(10.0, 14.0), press(10.0, 14.0)] {
        run(&mut st, &input, &mut cap);
    }
    run(&mut st, &release(10.0, 14.0), &mut cap);
    assert_eq!(cap, HotkeyCapture::Waiting);
}

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

#[test]
fn hotkey_field_width_matches_the_drawn_field() {
    let mut st = UiState::default();
    for mut cap in [
        HotkeyCapture::Idle,
        HotkeyCapture::Waiting,
        HotkeyCapture::Error("nope".into()),
    ] {
        let mut w = 0.0;
        let cur = current();
        let f = frame(&mut st, &Input::default(), |ui| {
            w = ui.hotkey_field_width(&cur, &cap);
            ui.hotkey_field(id("hk"), &cur, &mut cap);
        });
        let drawn = f
            .cmds
            .iter()
            .find_map(|c| match c {
                Cmd::Fill { rect, .. } => Some(rect.w),
                _ => None,
            })
            .unwrap();
        assert_eq!(w, drawn, "{cap:?}");
    }
}

#[test]
fn click_reports_capture_start_and_frame_says_capturing() {
    let mut st = UiState::default();
    let mut cap = HotkeyCapture::Idle;
    for input in [hover(10.0, 14.0), hover(10.0, 14.0), press(10.0, 14.0)] {
        let (ev, f) = run(&mut st, &input, &mut cap);
        assert_eq!(ev, HotkeyFieldEvent::None);
        assert!(!f.capturing_keys);
    }
    let (ev, f) = run(&mut st, &release(10.0, 14.0), &mut cap);
    assert_eq!(ev, HotkeyFieldEvent::Started);
    assert!(f.capturing_keys);
    let (ev, f) = run(&mut st, &Input::default(), &mut cap);
    assert_eq!(ev, HotkeyFieldEvent::None, "reported once");
    assert!(f.capturing_keys);
    let (_, f) = run(&mut st, &with_keys(vec![key(UiKey::Esc)]), &mut cap);
    assert!(!f.capturing_keys);
}

#[test]
fn window_deactivation_cancels_capture() {
    let (mut st, mut cap) = waiting();
    let away = Input {
        deactivated: true,
        ..Input::default()
    };
    let (ev, _) = run(&mut st, &away, &mut cap);
    assert_eq!(ev, HotkeyFieldEvent::Cancelled);
    assert_eq!(cap, HotkeyCapture::Idle);
}

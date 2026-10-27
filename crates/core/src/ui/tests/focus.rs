//! Tab / Shift+Tab focus order and Esc in text fields.

use super::*;
use crate::hotkey::Hotkey;

/// Two text fields (a at y 0..28, b at y 36..64) and a hotkey field under them.
struct Form {
    st: UiState,
    a: TextEdit,
    b: TextEdit,
    cap: HotkeyCapture,
}

#[derive(Debug, PartialEq)]
struct Events {
    a: TextFieldEvent,
    b: TextFieldEvent,
    hk: HotkeyFieldEvent,
}

impl Form {
    fn new() -> Form {
        Form {
            st: UiState::default(),
            a: TextEdit::new("one"),
            b: TextEdit::new("two"),
            cap: HotkeyCapture::Idle,
        }
    }

    fn run(&mut self, input: &Input) -> Events {
        let cur: Hotkey = "Ctrl+Alt+Shift+L".parse().unwrap();
        let mut ev = Events {
            a: TextFieldEvent::None,
            b: TextFieldEvent::None,
            hk: HotkeyFieldEvent::None,
        };
        let (a, b, cap) = (&mut self.a, &mut self.b, &mut self.cap);
        frame(&mut self.st, input, |ui| {
            ev.a = ui.text_field(id("a"), a, "");
            ev.b = ui.text_field(id("b"), b, "");
            ev.hk = ui.hotkey_field(id("hk"), &cur, cap);
        });
        ev
    }

    fn click_a(&mut self) {
        for input in [hover(100.0, 10.0), press(100.0, 10.0), release(100.0, 10.0)] {
            self.run(&input);
        }
        assert_eq!(self.st.focused, Some(id("a")));
    }
}

fn tab(shift: bool) -> Input {
    with_keys(vec![KeyPress {
        shift,
        ..key(UiKey::Tab)
    }])
}

#[test]
fn tab_moves_focus_forward_and_commits() {
    let mut f = Form::new();
    f.click_a();
    f.run(&typing("!"));
    let ev = f.run(&tab(false));
    assert_eq!(ev.a, TextFieldEvent::Committed);
    assert_eq!(f.a.text, "one!");
    assert_eq!(f.st.focused, Some(id("b")));
    // b takes typing now.
    f.run(&typing("?"));
    assert_eq!((f.a.text.as_str(), f.b.text.as_str()), ("one!", "two?"));
    // On to the hotkey field, which starts listening.
    let ev = f.run(&tab(false));
    assert_eq!(ev.b, TextFieldEvent::Committed);
    assert_eq!(f.st.focused, Some(id("hk")));
    f.run(&Input::default());
    assert!(f.cap.is_listening(), "{:?}", f.cap);
    // Tab from the hotkey field cancels capture and wraps to the first field.
    let ev = f.run(&tab(false));
    assert_eq!(ev.hk, HotkeyFieldEvent::Cancelled);
    assert_eq!(f.cap, HotkeyCapture::Idle);
    assert_eq!(f.st.focused, Some(id("a")));
}

#[test]
fn shift_tab_moves_back() {
    let mut f = Form::new();
    f.click_a();
    let ev = f.run(&tab(true));
    assert_eq!(ev.a, TextFieldEvent::Committed);
    assert_eq!(f.st.focused, Some(id("hk")), "wraps to the last");
    f.run(&Input::default());
    assert!(f.cap.is_listening());
    let ev = f.run(&tab(true));
    assert_eq!(ev.hk, HotkeyFieldEvent::Cancelled);
    assert_eq!(f.st.focused, Some(id("b")));
}

#[test]
fn tab_with_nothing_focused_focuses_the_first() {
    let mut f = Form::new();
    f.run(&Input::default());
    f.run(&tab(false));
    assert_eq!(f.st.focused, Some(id("a")));
}

#[test]
fn esc_reverts_and_blurs_text_field() {
    let mut f = Form::new();
    f.click_a();
    f.run(&typing("xyz"));
    assert_eq!(f.a.text, "onexyz");
    let ev = f.run(&with_keys(vec![key(UiKey::Esc)]));
    assert_ne!(ev.a, TextFieldEvent::Committed);
    assert_eq!(f.a.text, "one");
    assert_eq!(f.st.focused, None);
}

#[test]
fn esc_reverts_a_field_focused_by_tab() {
    let mut f = Form::new();
    f.click_a();
    f.run(&tab(false));
    f.run(&typing("xyz"));
    f.run(&with_keys(vec![key(UiKey::Esc)]));
    assert_eq!(f.b.text, "two");
    assert_eq!(f.st.focused, None);
}

#[test]
fn a_focused_hotkey_field_does_not_blink() {
    let mut f = Form::new();
    f.click_a();
    f.run(&tab(true));
    let cur: Hotkey = "Ctrl+Alt+Shift+L".parse().unwrap();
    let (a, b, cap) = (&mut f.a, &mut f.b, &mut f.cap);
    let fr = frame(&mut f.st, &Input::default(), |ui| {
        ui.text_field(id("a"), a, "");
        ui.text_field(id("b"), b, "");
        ui.hotkey_field(id("hk"), &cur, cap);
    });
    assert!(cap.is_listening());
    assert!(!fr.wants_caret_blink);
}

#[test]
fn tab_into_the_hotkey_field_reports_capture_start() {
    let mut f = Form::new();
    f.click_a();
    f.run(&tab(true));
    let ev = f.run(&Input::default());
    assert_eq!(ev.hk, HotkeyFieldEvent::Started);
}

#[test]
fn tab_scrolls_the_focused_field_into_view() {
    let mut st = UiState::default();
    let (mut a, mut b) = (TextEdit::new("a"), TextEdit::new("b"));
    let mut run = |st: &mut UiState, input: &Input| {
        frame(st, input, |ui| {
            ui.scroll_area(id("s"), 60.0, 400.0, |ui| {
                ui.text_field(id("a"), &mut a, "");
                ui.space(300.0);
                ui.text_field(id("b"), &mut b, "");
            });
        })
    };
    for input in [hover(100.0, 10.0), press(100.0, 10.0), release(100.0, 10.0)] {
        run(&mut st, &input);
    }
    assert_eq!(st.focused, Some(id("a")));
    run(&mut st, &tab(false));
    assert_eq!(st.focused, Some(id("b")));
    let f = run(&mut st, &Input::default());
    // b sits at 28 + 300 = 328..356; just enough to show its bottom in the 60 tall view.
    assert_eq!(st.scroll[&id("s")], 356.0 - 60.0);
    assert!(f.redraw_after_ms.is_some(), "redrawn at the new offset");
    let f = run(&mut st, &Input::default());
    let b_y = f
        .cmds
        .iter()
        .find_map(|c| match c {
            Cmd::Text { text, pos, .. } if text == "b" => Some(pos.y),
            _ => None,
        })
        .unwrap();
    assert!(
        (0.0..60.0).contains(&b_y),
        "b drawn inside the view at {b_y}"
    );
    run(&mut st, &tab(true));
    run(&mut st, &Input::default());
    assert_eq!(st.scroll[&id("s")], 0.0, "back up to a");
}

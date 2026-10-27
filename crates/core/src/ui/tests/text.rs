use super::*;

struct FakeClipboard(Option<String>);
impl Clipboard for FakeClipboard {
    fn get(&mut self) -> Option<String> {
        self.0.clone()
    }
    fn set(&mut self, s: &str) {
        self.0 = Some(s.to_string());
    }
}

/// One full-width field at (0, 0, 214, 28); text starts at x = 8.
fn field(st: &mut UiState, input: &Input, edit: &mut TextEdit) -> (TextFieldEvent, Frame) {
    let mut ev = TextFieldEvent::None;
    let f = frame(st, input, |ui| ev = ui.text_field(id("f"), edit, "Name"));
    (ev, f)
}

fn focused_field(text: &str) -> (UiState, TextEdit) {
    let mut st = UiState::default();
    let mut e = TextEdit::new(text);
    field(&mut st, &hover(100.0, 10.0), &mut e);
    field(&mut st, &press(100.0, 10.0), &mut e);
    field(&mut st, &release(100.0, 10.0), &mut e);
    assert_eq!(st.focused, Some(id("f")));
    (st, e)
}

#[test]
fn text_field_click_focuses_and_shows_ibeam() {
    let mut st = UiState::default();
    let mut e = TextEdit::default();
    field(&mut st, &hover(100.0, 10.0), &mut e);
    let (_, f) = field(&mut st, &hover(100.0, 10.0), &mut e);
    assert_eq!(f.cursor, CursorKind::IBeam);
    assert!(!f.wants_caret_blink);
    let (_, f) = field(&mut st, &press(100.0, 10.0), &mut e);
    assert_eq!(st.focused, Some(id("f")));
    assert!(f.wants_caret_blink);
}

#[test]
fn text_field_shows_placeholder_when_empty() {
    let mut st = UiState::default();
    let (_, f) = field(&mut st, &Input::default(), &mut TextEdit::default());
    assert_eq!(texts(&f), vec!["Name".to_string()]);
}

#[test]
fn typing_goes_to_the_focused_field() {
    let (mut st, mut e) = focused_field("ab");
    let (ev, _) = field(&mut st, &typing("c"), &mut e);
    assert_eq!(ev, TextFieldEvent::Changed);
    assert_eq!(e.text, "abc");
    let (ev, _) = field(&mut st, &with_keys(vec![key(UiKey::Backspace)]), &mut e);
    assert_eq!(ev, TextFieldEvent::Changed);
    assert_eq!(e.text, "ab");
}

#[test]
fn unfocused_field_ignores_typing() {
    let mut st = UiState::default();
    let mut e = TextEdit::new("ab");
    let (ev, _) = field(&mut st, &typing("c"), &mut e);
    assert_eq!(ev, TextFieldEvent::None);
    assert_eq!(e.text, "ab");
}

#[test]
fn only_the_focused_of_two_fields_gets_text() {
    let mut st = UiState::default();
    let (mut a, mut b) = (TextEdit::default(), TextEdit::default());
    let mut run = |st: &mut UiState, input: &Input| {
        frame(st, input, |ui| {
            ui.text_field(id("a"), &mut a, "");
            ui.text_field(id("b"), &mut b, "");
        });
    };
    // b is the second field, at y 36..64.
    run(&mut st, &hover(10.0, 50.0));
    run(&mut st, &press(10.0, 50.0));
    run(&mut st, &typing("x"));
    assert_eq!((a.text.as_str(), b.text.as_str()), ("", "x"));
}

#[test]
fn enter_commits_and_blurs() {
    let (mut st, mut e) = focused_field("ab");
    let (ev, f) = field(&mut st, &with_keys(vec![key(UiKey::Enter)]), &mut e);
    assert_eq!(ev, TextFieldEvent::Committed);
    assert_eq!(st.focused, None);
    assert!(!f.wants_caret_blink);
}

#[test]
fn pressing_elsewhere_blurs_and_commits() {
    let (mut st, mut e) = focused_field("ab");
    let (ev, f) = field(&mut st, &press(100.0, 150.0), &mut e);
    assert_eq!(ev, TextFieldEvent::Committed);
    assert_eq!(st.focused, None);
    assert!(!f.wants_caret_blink);
}

#[test]
fn pressing_an_earlier_field_still_commits_the_focused_one() {
    let mut st = UiState::default();
    let (mut a, mut b) = (TextEdit::default(), TextEdit::default());
    let mut run = |st: &mut UiState, input: &Input| {
        let mut evs = (TextFieldEvent::None, TextFieldEvent::None);
        frame(st, input, |ui| {
            evs.0 = ui.text_field(id("a"), &mut a, "");
            evs.1 = ui.text_field(id("b"), &mut b, "");
        });
        evs
    };
    run(&mut st, &hover(10.0, 50.0));
    run(&mut st, &press(10.0, 50.0));
    run(&mut st, &release(10.0, 50.0));
    assert_eq!(st.focused, Some(id("b")));
    run(&mut st, &hover(10.0, 10.0));
    let evs = run(&mut st, &press(10.0, 10.0));
    assert_eq!(evs, (TextFieldEvent::None, TextFieldEvent::Committed));
    assert_eq!(st.focused, Some(id("a")));
}

#[test]
fn focus_is_dropped_when_the_field_is_not_built() {
    let (mut st, _) = focused_field("ab");
    let f = frame(&mut st, &Input::default(), |_| {});
    assert_eq!(st.focused, None);
    assert!(!f.wants_caret_blink);
}

#[test]
fn click_places_the_caret() {
    let mut st = UiState::default();
    let mut e = TextEdit::new("abcd");
    // Between "b" and "c": x = 8 + 2 * 7, nudged right.
    field(&mut st, &hover(24.0, 10.0), &mut e);
    field(&mut st, &press(24.0, 10.0), &mut e);
    assert_eq!(e.caret, 2);
}

#[test]
fn paste_uses_the_ui_clipboard() {
    let (mut st, mut e) = focused_field("a");
    let mut cb = FakeClipboard(Some("bc".to_string()));
    let input = with_keys(vec![KeyPress {
        ctrl: true,
        ..key(UiKey::V)
    }]);
    let mut ui = Ui::new(&Fake, &input, &mut st, Rect::new(0.0, 0.0, 214.0, 200.0));
    ui.clipboard = Some(&mut cb);
    let ev = ui.text_field(id("f"), &mut e, "");
    ui.finish();
    assert_eq!(ev, TextFieldEvent::Changed);
    assert_eq!(e.text, "abc");
}

#[test]
fn caret_blinks_with_now_ms() {
    let (mut st, mut e) = focused_field("ab");
    let lines = |f: &Frame| {
        f.cmds
            .iter()
            .filter(|c| matches!(c, Cmd::Line { .. }))
            .count()
    };
    let on = Input {
        now_ms: 1000,
        ..Input::default()
    };
    let off = Input {
        now_ms: 1500,
        ..Input::default()
    };
    assert_eq!(lines(&field(&mut st, &on, &mut e).1), 1);
    assert_eq!(lines(&field(&mut st, &off, &mut e).1), 0);
}

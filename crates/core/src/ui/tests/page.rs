//! Building blocks for whole pages: free-form painting, layout queries and cards.

use super::*;

const AREA: Rect = Rect {
    x: 10.0,
    y: 20.0,
    w: 300.0,
    h: 400.0,
};

#[test]
fn available_is_the_rest_of_the_column_after_the_pending_gap() {
    let mut st = UiState::default();
    let mut seen = Vec::new();
    frame_in(&mut st, &Input::default(), AREA, |ui| {
        seen.push(ui.available());
        ui.label("Hi", TextStyle::BODY, theme::TEXT);
        seen.push(ui.available());
        ui.row(30.0, |ui| {
            ui.next_width(50.0);
            ui.column(|_| {});
            seen.push(ui.available());
        });
    });
    assert_eq!(seen[0], AREA);
    // Label is 13 tall; the next item goes after a gap of 8.
    assert_eq!(seen[1], Rect::new(10.0, 41.0, 300.0, 379.0));
    // In a row: after the 50-wide column and a gap.
    assert_eq!(seen[2], Rect::new(68.0, 41.0, 242.0, 30.0));
}

#[test]
fn paint_moves_with_the_content_offset() {
    let mut st = UiState::default();
    let fill = |x: f32, y: f32| Cmd::Fill {
        rect: Rect::new(x, y, 5.0, 5.0),
        color: theme::TEXT,
        radius: 0.0,
    };
    let text = Cmd::Text {
        pos: Pt { x: 1.0, y: 2.0 },
        text: "a".into(),
        style: TextStyle::BODY,
        color: theme::TEXT,
    };
    let f = frame(&mut st, &Input::default(), |ui| {
        ui.paint(fill(1.0, 2.0));
        ui.offset(10.0, -5.0, |ui| {
            ui.paint(fill(1.0, 2.0));
            ui.paint(text.clone());
        });
    });
    assert_eq!(f.cmds[0], fill(1.0, 2.0));
    assert_eq!(f.cmds[1], fill(11.0, -3.0));
    assert_eq!(
        f.cmds[2],
        Cmd::Text {
            pos: Pt { x: 11.0, y: -3.0 },
            text: "a".into(),
            style: TextStyle::BODY,
            color: theme::TEXT,
        }
    );
}

#[test]
fn text_measure_is_the_frames_measure() {
    let mut st = UiState::default();
    let mut got = (0.0, 0.0);
    frame(&mut st, &Input::default(), |ui| {
        got = ui.text_measure().measure("abc", TextStyle::BODY)
    });
    assert_eq!(got, (21.0, 13.0));
}

#[test]
fn is_focused_reports_the_focused_field() {
    let mut st = UiState::default();
    let mut edit = TextEdit::default();
    let mut focused = (true, true);
    let mut run = |st: &mut UiState, input: &Input| {
        frame(st, input, |ui| {
            focused.0 = ui.is_focused(id("f"));
            ui.text_field(id("f"), &mut edit, "");
            focused.1 = ui.is_focused(id("f"));
        });
        focused
    };
    assert_eq!(run(&mut st, &hover(5.0, 5.0)), (false, false));
    assert_eq!(run(&mut st, &press(5.0, 5.0)), (false, true));
    assert_eq!(run(&mut st, &Input::default()), (true, true));
}

#[test]
fn request_redraw_asks_for_a_frame_now() {
    let mut st = UiState::default();
    let input = Input {
        now_ms: 1234,
        ..Input::default()
    };
    let f = frame(&mut st, &input, |ui| ui.request_redraw());
    assert_eq!(f.redraw_after_ms, Some(1234));
    let f = frame(&mut st, &input, |_| {});
    assert_eq!(f.redraw_after_ms, None);
}

#[test]
fn card_wraps_its_content_with_padding_behind_it() {
    let mut st = UiState::default();
    let mut after = Rect::default();
    let f = frame_in(&mut st, &Input::default(), AREA, |ui| {
        ui.card(|ui| {
            ui.label("Hi", TextStyle::BODY, theme::TEXT);
            ui.label("There", TextStyle::BODY, theme::TEXT);
        });
        after = ui.available();
    });
    // Content: 13 + 8 + 13 = 34, plus 12 above and below.
    let card = Rect::new(10.0, 20.0, 300.0, 58.0);
    assert_eq!(
        f.cmds[0],
        Cmd::Fill {
            rect: card,
            color: theme::SURFACE,
            radius: theme::CARD_RADIUS
        }
    );
    assert_eq!(
        f.cmds[1],
        Cmd::Stroke {
            rect: card,
            color: theme::BORDER,
            radius: theme::CARD_RADIUS,
            width: 1.0
        }
    );
    match &f.cmds[2] {
        Cmd::Text { pos, .. } => assert_eq!(*pos, Pt { x: 26.0, y: 32.0 }),
        c => panic!("expected the first label, got {c:?}"),
    }
    assert_eq!(after.y, card.bottom() + theme::GAP);
}

#[test]
fn card_content_fills_the_inner_width() {
    let mut st = UiState::default();
    let mut inner = Rect::default();
    frame_in(&mut st, &Input::default(), AREA, |ui| {
        ui.card(|ui| inner = ui.available());
    });
    assert_eq!(inner.x, 26.0);
    assert_eq!(inner.y, 32.0);
    assert_eq!(inner.w, 268.0);
}

#[test]
fn color_picker_size_matches_the_widget() {
    // 12 presets + 2 saved + the hex box on one line: 14 × 24 + 76 = 412 wide, 24 tall.
    assert_eq!(color_picker_size(2, 800.0), (412.0, 24.0));
    // Narrower than that wraps the hex box to a second line.
    assert_eq!(color_picker_size(2, 400.0), (330.0, 54.0));
}

#[test]
#[should_panic(expected = "use Ui::clip")]
fn paint_rejects_raw_clips() {
    let mut st = UiState::default();
    frame(&mut st, &Input::default(), |ui| {
        ui.paint(Cmd::ClipPush(Rect::new(0.0, 0.0, 5.0, 5.0)))
    });
}

#[test]
fn accent_button_clicks_and_uses_the_accent_look() {
    let mut st = UiState::default();
    let run = |st: &mut UiState, input: &Input| {
        let mut clicked = false;
        let f = frame(st, input, |ui| clicked = ui.button_accent(id("a"), "Go"));
        (clicked, f)
    };
    // Hot comes from the previous frame.
    run(&mut st, &hover(5.0, 5.0));
    let (_, f) = run(&mut st, &hover(5.0, 5.0));
    assert_eq!(
        f.cmds[0],
        Cmd::Fill {
            rect: Rect::new(0.0, 0.0, 38.0, 28.0),
            color: theme::ACCENT_HOT,
            radius: theme::BUTTON_RADIUS
        }
    );
    match &f.cmds[2] {
        Cmd::Text { style, color, .. } => {
            assert_eq!(*style, TextStyle::BOLD);
            assert_eq!(*color, theme::ON_ACCENT);
        }
        c => panic!("expected the label, got {c:?}"),
    }
    run(&mut st, &press(5.0, 5.0));
    assert!(run(&mut st, &release(5.0, 5.0)).0);
    let (_, f) = run(&mut st, &Input::default());
    assert!(matches!(f.cmds[0], Cmd::Fill { color, .. } if color == theme::ACCENT));
}

#[test]
fn frames_list_their_hit_targets_for_tests() {
    let mut st = UiState::default();
    let f = frame(&mut st, &Input::default(), |ui| {
        ui.button(id("a"), "OK");
        ui.clip(Rect::new(0.0, 0.0, 100.0, 40.0), |ui| {
            ui.button(id("b"), "Hidden");
        });
    });
    // The second button is clipped to the strip it still shows in.
    assert_eq!(
        f.hits,
        vec![
            Rect::new(0.0, 0.0, 38.0, 28.0),
            Rect::new(0.0, 36.0, 66.0, 4.0)
        ]
    );
}

#[test]
fn link_is_accent_text_that_clicks() {
    let mut st = UiState::default();
    let run = |st: &mut UiState, input: &Input| {
        let mut clicked = false;
        let f = frame(st, input, |ui| clicked = ui.link(id("l"), "Open"));
        (clicked, f)
    };
    let (_, f) = run(&mut st, &Input::default());
    // No button box: just the text, in the accent color, centred in a control-high hit area.
    assert!(!f
        .cmds
        .iter()
        .any(|c| matches!(c, Cmd::Fill { .. } | Cmd::Stroke { .. })));
    assert_eq!(
        f.cmds[0],
        Cmd::Text {
            pos: Pt {
                x: 0.0,
                y: (theme::CONTROL_HEIGHT - 13.0) / 2.0
            },
            text: "Open".into(),
            style: TextStyle::BODY,
            color: theme::ACCENT,
        }
    );
    run(&mut st, &hover(5.0, 5.0));
    let (_, f) = run(&mut st, &hover(5.0, 5.0));
    // Hovered: the hot accent and an underline.
    assert!(f
        .cmds
        .iter()
        .any(|c| matches!(c, Cmd::Text { color, .. } if *color == theme::ACCENT_HOT)));
    assert!(f
        .cmds
        .iter()
        .any(|c| matches!(c, Cmd::Fill { rect, .. } if rect.h == 1.0 && rect.w == 28.0)));
    run(&mut st, &press(5.0, 5.0));
    assert!(run(&mut st, &release(5.0, 5.0)).0);
}

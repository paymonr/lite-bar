mod chips;
mod color;
mod hotkey;
mod location;
mod nav;
mod text;

use super::theme::Rgba;
use super::*;

struct Fake;
impl TextMeasure for Fake {
    fn measure(&self, text: &str, style: TextStyle) -> (f32, f32) {
        (7.0 * text.chars().count() as f32, style.size)
    }
    fn baseline(&self, style: TextStyle) -> f32 {
        0.8 * style.size
    }
}

fn at(x: f32, y: f32) -> Option<Pt> {
    Some(Pt { x, y })
}

fn press(x: f32, y: f32) -> Input {
    Input {
        pointer: at(x, y),
        down: true,
        pressed: true,
        ..Input::default()
    }
}

fn hold(x: f32, y: f32) -> Input {
    Input {
        pointer: at(x, y),
        down: true,
        ..Input::default()
    }
}

fn release(x: f32, y: f32) -> Input {
    Input {
        pointer: at(x, y),
        released: true,
        ..Input::default()
    }
}

fn hover(x: f32, y: f32) -> Input {
    Input {
        pointer: at(x, y),
        ..Input::default()
    }
}

fn frame(state: &mut UiState, input: &Input, build: impl FnOnce(&mut Ui)) -> Frame {
    frame_in(state, input, Rect::new(0.0, 0.0, 214.0, 200.0), build)
}

fn frame_in(state: &mut UiState, input: &Input, area: Rect, build: impl FnOnce(&mut Ui)) -> Frame {
    let mut ui = Ui::new(&Fake, input, state, area);
    build(&mut ui);
    ui.finish()
}

fn key(key: UiKey) -> KeyPress {
    KeyPress {
        key,
        ctrl: false,
        alt: false,
        shift: false,
        win: false,
    }
}

fn with_keys(keys: Vec<KeyPress>) -> Input {
    Input {
        keys,
        ..Input::default()
    }
}

fn typing(s: &str) -> Input {
    Input {
        text: s.chars().collect(),
        ..Input::default()
    }
}

fn texts(f: &Frame) -> Vec<String> {
    f.cmds
        .iter()
        .filter_map(|c| match c {
            Cmd::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// "OK" button sits at (0,0) and is 38x28.
fn ok_button(state: &mut UiState, input: &Input) -> bool {
    let mut clicked = false;
    frame(state, input, |ui| clicked = ui.button(id("ok"), "OK"));
    clicked
}

#[test]
fn button_clicks_on_release_inside_only() {
    let mut st = UiState::default();
    ok_button(&mut st, &hover(10.0, 10.0));
    assert!(!ok_button(&mut st, &press(10.0, 10.0)));
    assert_eq!(st.active, Some(id("ok")));
    assert!(ok_button(&mut st, &release(10.0, 10.0)));
    assert_eq!(st.active, None);
}

#[test]
fn button_press_then_release_outside_is_not_a_click() {
    let mut st = UiState::default();
    ok_button(&mut st, &hover(10.0, 10.0));
    ok_button(&mut st, &press(10.0, 10.0));
    assert!(!ok_button(&mut st, &release(150.0, 90.0)));
    assert_eq!(st.active, None);
}

#[test]
fn toggle_flips_on_click() {
    let mut st = UiState::default();
    let mut value = false;
    let run = |st: &mut UiState, input: &Input, value: &mut bool| {
        let mut changed = false;
        frame(st, input, |ui| changed = ui.toggle(id("t"), value));
        changed
    };
    run(&mut st, &hover(5.0, 5.0), &mut value);
    assert!(!run(&mut st, &press(5.0, 5.0), &mut value));
    assert!(!value);
    assert!(run(&mut st, &release(5.0, 5.0), &mut value));
    assert!(value);
}

fn slider(st: &mut UiState, input: &Input, v: &mut i64) -> bool {
    let mut changed = false;
    frame(st, input, |ui| changed = ui.slider(id("s"), v, 0..=100));
    changed
}

#[test]
fn slider_sets_value_from_pointer_x() {
    // Track spans x = 7..207 (200 wide), so x = 107 is the midpoint.
    let mut st = UiState::default();
    let mut v = 0;
    slider(&mut st, &hover(107.0, 10.0), &mut v);
    assert!(slider(&mut st, &press(107.0, 10.0), &mut v));
    assert_eq!(v, 50);
    assert!(slider(&mut st, &hold(57.0, 10.0), &mut v));
    assert_eq!(v, 25);
    assert!(!slider(&mut st, &hold(57.0, 10.0), &mut v));
}

#[test]
fn slider_clamps_and_ends_drag_outside() {
    let mut st = UiState::default();
    let mut v = 0;
    slider(&mut st, &hover(107.0, 10.0), &mut v);
    slider(&mut st, &press(107.0, 10.0), &mut v);
    // Pointer far outside the slider, vertically and horizontally.
    assert!(slider(&mut st, &hold(900.0, 150.0), &mut v));
    assert_eq!(v, 100);
    slider(&mut st, &hold(-50.0, 150.0), &mut v);
    assert_eq!(v, 0);
    // Pointer gone entirely: value is left alone, drag continues.
    slider(
        &mut st,
        &Input {
            down: true,
            ..Input::default()
        },
        &mut v,
    );
    assert_eq!(v, 0);
    assert_eq!(st.active, Some(id("s")));
    // Release outside the track ends the drag.
    slider(&mut st, &release(900.0, 150.0), &mut v);
    assert_eq!(v, 100);
    assert_eq!(st.active, None);
    // Later movement no longer changes the value.
    assert!(!slider(&mut st, &hover(57.0, 10.0), &mut v));
    assert_eq!(v, 100);
}

#[test]
fn segmented_selects_clicked_option() {
    // Option widths: Off 49, Inline 70, Stacked 77 (7/char + 28).
    let mut st = UiState::default();
    let mut sel = 0usize;
    let run = |st: &mut UiState, input: &Input, sel: &mut usize| {
        let mut changed = false;
        frame(st, input, |ui| {
            changed = ui.segmented(id("seg"), sel, &["Off", "Inline", "Stacked"])
        });
        changed
    };
    run(&mut st, &hover(180.0, 10.0), &mut sel);
    assert!(!run(&mut st, &press(180.0, 10.0), &mut sel));
    assert!(run(&mut st, &release(180.0, 10.0), &mut sel));
    assert_eq!(sel, 2);
    // Releasing on the already selected option is not a change.
    run(&mut st, &hover(180.0, 10.0), &mut sel);
    run(&mut st, &press(180.0, 10.0), &mut sel);
    assert!(!run(&mut st, &release(180.0, 10.0), &mut sel));
    run(&mut st, &hover(59.0, 10.0), &mut sel);
    run(&mut st, &press(59.0, 10.0), &mut sel);
    assert!(run(&mut st, &release(59.0, 10.0), &mut sel));
    assert_eq!(sel, 1);
}

#[test]
fn hover_sets_hand_cursor() {
    let mut st = UiState::default();
    let first = frame(&mut st, &hover(10.0, 10.0), |ui| {
        ui.button(id("ok"), "OK");
    });
    // Hot comes from the previous frame, so the first frame has no hand yet.
    assert_eq!(first.cursor, CursorKind::Arrow);
    assert_eq!(st.hot, Some(id("ok")));
    let over = frame(&mut st, &hover(10.0, 10.0), |ui| {
        ui.button(id("ok"), "OK");
    });
    assert_eq!(over.cursor, CursorKind::Hand);
    let away = frame(&mut st, &hover(150.0, 90.0), |ui| {
        ui.button(id("ok"), "OK");
    });
    assert_eq!(away.cursor, CursorKind::Arrow);
    assert_eq!(st.hot, None);
}

#[test]
fn widgets_emit_expected_cmds() {
    let mut st = UiState::default();
    let f = frame(&mut st, &Input::default(), |ui| {
        ui.button(id("ok"), "OK");
    });
    let fill = f.cmds.iter().find_map(|c| match c {
        Cmd::Fill { rect, radius, .. } => Some((*rect, *radius)),
        _ => None,
    });
    let (rect, radius) = fill.expect("button fill");
    assert_eq!(radius, 4.0);
    assert_eq!(rect, Rect::new(0.0, 0.0, 38.0, 28.0));
    let text = f.cmds.iter().find_map(|c| match c {
        Cmd::Text { pos, text, .. } => Some((*pos, text.clone())),
        _ => None,
    });
    let (pos, text) = text.expect("button text");
    assert_eq!(text, "OK");
    // Centred: (38-14)/2 and (28-13)/2.
    assert_eq!(pos, Pt { x: 12.0, y: 7.5 });
    assert!(!f.wants_caret_blink);
}

#[test]
fn row_places_widgets_side_by_side_and_below() {
    let mut st = UiState::default();
    let f = frame(&mut st, &Input::default(), |ui| {
        ui.row(28.0, |ui| {
            ui.button(id("a"), "OK");
            ui.button(id("b"), "OK");
        });
        ui.button(id("c"), "OK");
    });
    let rects: Vec<Rect> = f
        .cmds
        .iter()
        .filter_map(|c| match c {
            Cmd::Fill { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect();
    assert_eq!(rects[0].x, 0.0);
    assert_eq!(rects[1].x, 38.0 + 8.0);
    assert_eq!(rects[2], Rect::new(0.0, 36.0, 38.0, 28.0));
}

#[test]
fn ids_are_stable_and_distinct() {
    assert_eq!(id("opacity"), id("opacity"));
    assert_ne!(id("opacity"), id("size"));
    assert_ne!(id_with(id("row"), 0), id_with(id("row"), 1));
}

#[test]
fn rect_helpers() {
    let r = Rect::new(10.0, 20.0, 100.0, 40.0);
    assert!(r.contains(Pt { x: 10.0, y: 20.0 }));
    assert!(!r.contains(Pt { x: 110.0, y: 30.0 }));
    assert_eq!(r.shrink(5.0), Rect::new(15.0, 25.0, 90.0, 30.0));
    assert_eq!(
        r.intersect(Rect::new(0.0, 0.0, 50.0, 30.0)),
        Rect::new(10.0, 20.0, 40.0, 10.0)
    );
    assert_eq!(r.intersect(Rect::new(500.0, 0.0, 5.0, 5.0)).w, 0.0);
    let (l, rest) = r.split_left(30.0);
    assert_eq!(l, Rect::new(10.0, 20.0, 30.0, 40.0));
    assert_eq!(rest, Rect::new(40.0, 20.0, 70.0, 40.0));
}

fn fills(f: &Frame) -> Vec<(Rect, Rgba)> {
    f.cmds
        .iter()
        .filter_map(|c| match c {
            Cmd::Fill { rect, color, .. } => Some((*rect, *color)),
            _ => None,
        })
        .collect()
}

/// Hover, press, release at (x, y) over one frame builder; returns whether the last frame clicked.
fn click_at(st: &mut UiState, x: f32, y: f32, mut build: impl FnMut(&mut Ui) -> bool) -> bool {
    for input in [hover(x, y), hover(x, y), press(x, y)] {
        frame(st, &input, |ui| {
            build(ui);
        });
    }
    let mut clicked = false;
    frame(st, &release(x, y), |ui| clicked = build(ui));
    clicked
}

#[test]
fn next_width_sets_exact_width_for_one_widget() {
    let mut st = UiState::default();
    let f = frame(&mut st, &Input::default(), |ui| {
        ui.next_width(100.0);
        ui.button(id("a"), "OK");
        ui.button(id("b"), "OK");
    });
    let r = fills(&f);
    assert_eq!(r[0].0.w, 100.0);
    assert_eq!(r[1].0.w, 38.0);
}

#[test]
fn place_confines_widgets_to_rect() {
    let mut st = UiState::default();
    let f = frame(&mut st, &Input::default(), |ui| {
        ui.place(Rect::new(50.0, 60.0, 100.0, 40.0), |ui| {
            ui.button(id("a"), "OK");
            ui.button(id("b"), "OK");
        });
        ui.button(id("c"), "OK");
    });
    let r = fills(&f);
    assert_eq!(r[0].0, Rect::new(50.0, 60.0, 38.0, 28.0));
    assert_eq!(r[1].0, Rect::new(50.0, 96.0, 38.0, 28.0));
    // The outer cursor is unaffected by what was placed.
    assert_eq!(r[2].0, Rect::new(0.0, 0.0, 38.0, 28.0));
}

#[test]
fn right_aligned_hugs_the_right_edge_of_the_row() {
    let mut st = UiState::default();
    let f = frame(&mut st, &Input::default(), |ui| {
        ui.row(28.0, |ui| {
            ui.button(id("a"), "OK");
            ui.right_aligned(80.0, |ui| {
                ui.next_width(80.0);
                ui.button(id("b"), "OK");
            });
        });
    });
    let r = fills(&f);
    assert_eq!(r[0].0.x, 0.0);
    assert_eq!(r[1].0, Rect::new(214.0 - 80.0, 0.0, 80.0, 28.0));
}

#[test]
fn nested_row_inside_a_row() {
    let mut st = UiState::default();
    let f = frame(&mut st, &Input::default(), |ui| {
        ui.row(40.0, |ui| {
            ui.button(id("a"), "OK");
            ui.row(20.0, |ui| {
                ui.button(id("b"), "OK");
                ui.button(id("c"), "OK");
            });
        });
    });
    let r = fills(&f);
    assert_eq!(r[0].0.x, 0.0);
    assert_eq!(r[1].0.x, 38.0 + 8.0);
    assert_eq!(r[2].0.x, 38.0 + 8.0 + 38.0 + 8.0);
    // Inner row is centred in the outer one (y 10..30); buttons are centred in it.
    assert_eq!(r[1].0.y, 10.0 + (20.0 - 28.0) / 2.0);
}

#[test]
fn space_in_a_row_replaces_the_gap() {
    let mut st = UiState::default();
    let f = frame(&mut st, &Input::default(), |ui| {
        ui.row(28.0, |ui| {
            ui.button(id("a"), "OK");
            ui.space(20.0);
            ui.button(id("b"), "OK");
        });
    });
    assert_eq!(fills(&f)[1].0.x, 38.0 + 20.0);
}

#[test]
fn columns_in_a_row_sit_side_by_side() {
    let mut st = UiState::default();
    let f = frame(&mut st, &Input::default(), |ui| {
        ui.row(100.0, |ui| {
            ui.next_width(60.0);
            ui.column(|ui| {
                ui.button(id("a"), "OK");
                ui.button(id("b"), "OK");
            });
            ui.column(|ui| {
                ui.button(id("c"), "OK");
            });
        });
    });
    let r = fills(&f);
    assert_eq!(r[0].0, Rect::new(0.0, 0.0, 38.0, 28.0));
    assert_eq!(r[1].0, Rect::new(0.0, 36.0, 38.0, 28.0));
    assert_eq!(r[2].0, Rect::new(68.0, 0.0, 38.0, 28.0));
}

#[test]
fn clip_emits_commands_and_blocks_hover_outside() {
    let mut st = UiState::default();
    let build = |ui: &mut Ui| {
        let mut c = false;
        ui.clip(Rect::new(0.0, 0.0, 100.0, 10.0), |ui| {
            c = ui.button(id("ok"), "OK")
        });
        c
    };
    let f = frame(&mut st, &Input::default(), |ui| {
        build(ui);
    });
    assert!(matches!(f.cmds.first(), Some(Cmd::ClipPush(_))));
    assert!(matches!(f.cmds.last(), Some(Cmd::ClipPop)));
    // Pointer over the button but outside the clip rect: not hot, not clickable.
    assert!(!click_at(&mut st, 10.0, 20.0, build));
    assert_eq!(st.hot, None);
    // Inside the clip it works.
    assert!(click_at(&mut st, 10.0, 5.0, build));
}

#[test]
fn offset_shifts_rect_and_hit_test_together() {
    let mut st = UiState::default();
    let build = |ui: &mut Ui| {
        let mut c = false;
        ui.offset(0.0, 50.0, |ui| c = ui.button(id("ok"), "OK"));
        c
    };
    let f = frame(&mut st, &Input::default(), |ui| {
        build(ui);
    });
    assert_eq!(fills(&f)[0].0, Rect::new(0.0, 50.0, 38.0, 28.0));
    assert!(!click_at(&mut st, 10.0, 10.0, build));
    assert!(click_at(&mut st, 10.0, 60.0, build));
}

#[test]
fn block_stops_widgets_beneath_reacting() {
    let build = |ui: &mut Ui, blocked: bool| {
        let c = ui.button(id("ok"), "OK");
        if blocked {
            ui.block(Rect::new(0.0, 0.0, 100.0, 100.0));
        }
        c
    };
    // Control: without the blocker the second hover frame draws hot.
    let mut st = UiState::default();
    let mut last = None;
    for _ in 0..2 {
        last = Some(frame(&mut st, &hover(10.0, 10.0), |ui| {
            build(ui, false);
        }));
    }
    assert_eq!(fills(last.as_ref().unwrap())[0].1, theme::BUTTON_BG_HOT);
    // With a later blocker the button is neither hot nor clickable.
    let mut st = UiState::default();
    for _ in 0..2 {
        last = Some(frame(&mut st, &hover(10.0, 10.0), |ui| {
            build(ui, true);
        }));
    }
    assert_eq!(fills(last.as_ref().unwrap())[0].1, theme::BUTTON_BG);
    assert!(!click_at(&mut st, 10.0, 10.0, |ui| build(ui, true)));
}

#[test]
fn segmented_shows_hover_feedback() {
    let mut st = UiState::default();
    let mut last = None;
    for _ in 0..2 {
        let mut sel = 0usize;
        last = Some(frame(&mut st, &hover(59.0, 10.0), |ui| {
            ui.segmented(id("seg"), &mut sel, &["Off", "Inline"]);
        }));
    }
    assert!(fills(last.as_ref().unwrap())
        .iter()
        .any(|f| f.1 == theme::BUTTON_BG_HOT));
}

#[test]
fn next_width_is_cleared_by_a_new_layout() {
    let mut st = UiState::default();
    let f = frame(&mut st, &Input::default(), |ui| {
        ui.next_width(100.0);
        ui.region(Rect::new(0.0, 50.0, 200.0, 50.0), |ui| {
            ui.button(id("a"), "OK");
        });
    });
    assert_eq!(fills(&f)[0].0.w, 38.0);
}

use super::*;

struct Fake;
impl TextMeasure for Fake {
    fn measure(&self, text: &str, style: TextStyle) -> (f32, f32) {
        (7.0 * text.chars().count() as f32, style.size)
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
    let mut ui = Ui::new(&Fake, input, state, Rect::new(0.0, 0.0, 214.0, 200.0));
    build(&mut ui);
    ui.finish()
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
    assert!(!ok_button(&mut st, &press(10.0, 10.0)));
    assert_eq!(st.active, Some(id("ok")));
    assert!(ok_button(&mut st, &release(10.0, 10.0)));
    assert_eq!(st.active, None);
}

#[test]
fn button_press_then_release_outside_is_not_a_click() {
    let mut st = UiState::default();
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
    assert!(!run(&mut st, &press(180.0, 10.0), &mut sel));
    assert!(run(&mut st, &release(180.0, 10.0), &mut sel));
    assert_eq!(sel, 2);
    // Releasing on the already selected option is not a change.
    run(&mut st, &press(180.0, 10.0), &mut sel);
    assert!(!run(&mut st, &release(180.0, 10.0), &mut sel));
    run(&mut st, &press(59.0, 10.0), &mut sel);
    assert!(run(&mut st, &release(59.0, 10.0), &mut sel));
    assert_eq!(sel, 1);
}

#[test]
fn hover_sets_hand_cursor() {
    let mut st = UiState::default();
    let over = frame(&mut st, &hover(10.0, 10.0), |ui| {
        ui.button(id("ok"), "OK");
    });
    assert_eq!(over.cursor, CursorKind::Hand);
    assert_eq!(st.hot, Some(id("ok")));
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
    let (l, rest) = r.split_left(30.0);
    assert_eq!(l, Rect::new(10.0, 20.0, 30.0, 40.0));
    assert_eq!(rest, Rect::new(40.0, 20.0, 70.0, 40.0));
}

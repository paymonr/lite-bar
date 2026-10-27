use super::*;
use crate::color::Rgb;

fn rgb(hex: &str) -> Rgb {
    Rgb::parse_hex(hex).unwrap()
}

const WIDE: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 800.0,
    h: 400.0,
};

struct Picker {
    st: UiState,
    value: Rgb,
    saved: Vec<Rgb>,
    hex: TextEdit,
}

impl Picker {
    fn new(value: &str) -> Picker {
        Picker {
            st: UiState::default(),
            value: rgb(value),
            saved: vec![rgb("#123456"), rgb("#654321")],
            hex: TextEdit::default(),
        }
    }

    fn run(&mut self, input: &Input) -> (ColorPickerEvent, Frame) {
        let mut ev = ColorPickerEvent::None;
        let Picker {
            st,
            value,
            saved,
            hex,
        } = self;
        let f = frame_in(st, input, WIDE, |ui| {
            ev = ui.color_picker(id("c"), value, saved, hex)
        });
        (ev, f)
    }

    /// Hover, hover, press, release; returns the last frame's event.
    fn click(&mut self, x: f32, y: f32) -> ColorPickerEvent {
        self.run(&hover(x, y));
        self.run(&hover(x, y));
        self.run(&press(x, y));
        self.run(&release(x, y)).0
    }
}

/// Swatch `i` spans x = 24 i .. 24 i + 18, y = 3 .. 21; the hex field follows the swatches.
fn swatch_x(i: usize) -> f32 {
    24.0 * i as f32 + 9.0
}

#[test]
fn swatch_click_sets_the_color() {
    let mut p = Picker::new("#FFFFFF");
    assert_eq!(p.click(swatch_x(3), 12.0), ColorPickerEvent::Changed);
    assert_eq!(p.value, rgb("#FF5555"));
    // The saved swatches come after the presets.
    assert_eq!(p.click(swatch_x(13), 12.0), ColorPickerEvent::Changed);
    assert_eq!(p.value, rgb("#654321"));
    // Clicking the current color is not a change.
    assert_eq!(p.click(swatch_x(13), 12.0), ColorPickerEvent::None);
}

#[test]
fn hex_field_follows_the_value() {
    let mut p = Picker::new("#5fa8ff");
    p.run(&Input::default());
    assert_eq!(p.hex.text, "#5FA8FF");
}

fn type_into_hex(p: &mut Picker, s: &str) -> ColorPickerEvent {
    let x = swatch_x(14) + 20.0;
    p.click(x, 12.0);
    assert!(p.st.focused.is_some());
    p.run(&with_keys(vec![KeyPress {
        ctrl: true,
        ..key(UiKey::A)
    }]));
    p.run(&typing(s));
    p.run(&with_keys(vec![key(UiKey::Enter)])).0
}

#[test]
fn valid_hex_commits() {
    let mut p = Picker::new("#FFFFFF");
    assert_eq!(type_into_hex(&mut p, "#00ff80"), ColorPickerEvent::Changed);
    assert_eq!(p.value, rgb("#00FF80"));
    p.run(&Input::default());
    assert_eq!(p.hex.text, "#00FF80");
}

#[test]
fn invalid_hex_reverts() {
    let mut p = Picker::new("#FFFFFF");
    assert_eq!(type_into_hex(&mut p, "#00ff8"), ColorPickerEvent::None);
    assert_eq!(p.value, rgb("#FFFFFF"));
    p.run(&Input::default());
    assert_eq!(p.hex.text, "#FFFFFF");
}

#[test]
fn right_click_saved_swatch_requests_removal() {
    let mut p = Picker::new("#FFFFFF");
    p.run(&hover(swatch_x(13), 12.0));
    let right = Input {
        right_released: true,
        ..hover(swatch_x(13), 12.0)
    };
    assert_eq!(p.run(&right).0, ColorPickerEvent::RemoveSaved(1));
    // Presets cannot be removed.
    p.run(&hover(swatch_x(2), 12.0));
    let right = Input {
        right_released: true,
        ..hover(swatch_x(2), 12.0)
    };
    assert_eq!(p.run(&right).0, ColorPickerEvent::None);
}

#[test]
fn saved_swatches_have_dashed_borders_and_selection_is_outlined() {
    let mut p = Picker::new("#FF5555");
    let (_, f) = p.run(&Input::default());
    let dashed = f
        .cmds
        .iter()
        .filter(|c| matches!(c, Cmd::Dashed { .. }))
        .count();
    assert_eq!(dashed, 2);
    let outline = f.cmds.iter().any(|c| {
        matches!(c, Cmd::Stroke { rect, color, width, .. }
            if *color == theme::ACCENT && *width == 2.0 && rect.x < 72.0 && rect.right() > 90.0)
    });
    assert!(outline);
}

#[test]
fn color_picker_wraps_to_the_row_width() {
    // 214 wide fits 9 swatches per line; swatch 9 starts the second line at y 30.
    let mut st = UiState::default();
    let (mut v, mut hex) = (rgb("#FFFFFF"), TextEdit::default());
    let f = frame(&mut st, &Input::default(), |ui| {
        ui.color_picker(id("c"), &mut v, &[], &mut hex);
    });
    let swatches: Vec<Rect> = fills(&f)
        .into_iter()
        .filter(|(r, _)| r.w == 18.0)
        .map(|(r, _)| r)
        .collect();
    assert_eq!(swatches.len(), 12);
    assert_eq!(swatches[8], Rect::new(192.0, 3.0, 18.0, 18.0));
    assert_eq!(swatches[9], Rect::new(0.0, 33.0, 18.0, 18.0));
}

// --- wheel: 100 DIPs at (0, 0), centre (50, 50), radius 50 ---

fn wheel(st: &mut UiState, input: &Input, hsv: &mut Hsv) -> (bool, Frame) {
    let mut changed = false;
    let f = frame(st, input, |ui| {
        changed = ui.color_wheel(id("w"), hsv, 100.0)
    });
    (changed, f)
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn wheel_click_sets_hue_and_saturation() {
    let mut st = UiState::default();
    let mut hsv = Hsv {
        h: 0.0,
        s: 0.0,
        v: 1.0,
    };
    wheel(&mut st, &hover(75.0, 50.0), &mut hsv);
    assert!(wheel(&mut st, &press(75.0, 50.0), &mut hsv).0);
    assert!(close(hsv.h, 90.0) && close(hsv.s, 0.5), "{hsv:?}");
    assert!(wheel(&mut st, &hold(50.0, 25.0), &mut hsv).0);
    assert!(close(hsv.h, 0.0) && close(hsv.s, 0.5), "{hsv:?}");
    assert_eq!(hsv.v, 1.0);
}

#[test]
fn wheel_drag_clamps_to_rim() {
    let mut st = UiState::default();
    let mut hsv = Hsv {
        h: 0.0,
        s: 0.0,
        v: 1.0,
    };
    wheel(&mut st, &hover(50.0, 50.0), &mut hsv);
    wheel(&mut st, &press(50.0, 50.0), &mut hsv);
    wheel(&mut st, &hold(250.0, 50.0), &mut hsv);
    assert!(close(hsv.h, 90.0) && close(hsv.s, 1.0), "{hsv:?}");
    wheel(&mut st, &hold(50.0, 250.0), &mut hsv);
    assert!(close(hsv.h, 180.0) && close(hsv.s, 1.0), "{hsv:?}");
    wheel(&mut st, &release(50.0, 250.0), &mut hsv);
    assert!(!wheel(&mut st, &hover(50.0, 50.0), &mut hsv).0);
}

#[test]
fn wheel_ignores_presses_outside_the_disc() {
    let mut st = UiState::default();
    let mut hsv = Hsv {
        h: 10.0,
        s: 0.2,
        v: 1.0,
    };
    wheel(&mut st, &hover(3.0, 3.0), &mut hsv);
    assert!(!wheel(&mut st, &press(3.0, 3.0), &mut hsv).0);
    assert_eq!(st.active, None);
}

fn image(f: &Frame) -> (Rect, u32, std::rc::Rc<Vec<u8>>) {
    f.cmds
        .iter()
        .find_map(|c| match c {
            Cmd::Image {
                rect,
                width_px,
                height_px,
                rgba,
            } => {
                assert_eq!(width_px, height_px);
                Some((*rect, *width_px, rgba.clone()))
            }
            _ => None,
        })
        .expect("wheel image")
}

#[test]
fn wheel_image_uses_scale_and_is_cached() {
    let mut st = UiState::default();
    let mut hsv = Hsv {
        h: 0.0,
        s: 0.0,
        v: 1.0,
    };
    let input = Input::default();
    let run = |st: &mut UiState, hsv: &mut Hsv| {
        let mut ui = Ui::new(&Fake, &input, st, Rect::new(0.0, 0.0, 214.0, 200.0));
        ui.scale = 1.5;
        ui.color_wheel(id("w"), hsv, 100.0);
        image(&ui.finish())
    };
    let (rect, px, a) = run(&mut st, &mut hsv);
    assert_eq!(rect, Rect::new(0.0, 0.0, 100.0, 100.0));
    assert_eq!(px, 150);
    assert_eq!(a.len(), 150 * 150 * 4);
    let (_, _, b) = run(&mut st, &mut hsv);
    assert!(std::rc::Rc::ptr_eq(&a, &b));
    // Below the 1/256 quantisation step: still cached.
    hsv.v = 1.0 - 0.001;
    let (_, _, c) = run(&mut st, &mut hsv);
    assert!(std::rc::Rc::ptr_eq(&a, &c));
    hsv.v = 0.5;
    let (_, _, d) = run(&mut st, &mut hsv);
    assert!(!std::rc::Rc::ptr_eq(&a, &d));
}

#[test]
fn wheel_marker_sits_at_the_current_point() {
    let mut st = UiState::default();
    let mut hsv = Hsv {
        h: 90.0,
        s: 0.5,
        v: 1.0,
    };
    let (_, f) = wheel(&mut st, &Input::default(), &mut hsv);
    let marker = f.cmds.iter().any(|c| {
        matches!(c, Cmd::Stroke { rect, .. }
            if close(rect.x + rect.w / 2.0, 75.0) && close(rect.y + rect.h / 2.0, 50.0))
    });
    assert!(marker);
}

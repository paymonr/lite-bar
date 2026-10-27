use super::*;

// --- scroll area: 214 x 100 viewport at (0, 0), 300 DIPs of content ---

fn wheel_at(x: f32, y: f32, notches: f32) -> Input {
    Input {
        wheel: notches,
        ..hover(x, y)
    }
}

fn scrolled(st: &mut UiState, input: &Input) -> Frame {
    frame(st, input, |ui| {
        ui.scroll_area(id("sa"), 100.0, 300.0, |ui| {
            ui.button(id("ok"), "OK");
        })
    })
}

fn first_fill_y(f: &Frame) -> f32 {
    fills(f)[0].0.y
}

#[test]
fn scroll_wheel_scrolls_and_clamps() {
    let mut st = UiState::default();
    scrolled(&mut st, &hover(10.0, 50.0));
    let f = scrolled(&mut st, &wheel_at(10.0, 50.0, -1.0));
    assert_eq!(st.scroll[&id("sa")], 40.0);
    assert_eq!(first_fill_y(&f), -40.0);
    scrolled(&mut st, &wheel_at(10.0, 50.0, -20.0));
    assert_eq!(st.scroll[&id("sa")], 200.0);
    scrolled(&mut st, &wheel_at(10.0, 50.0, 20.0));
    assert_eq!(st.scroll[&id("sa")], 0.0);
}

#[test]
fn scroll_ignores_the_wheel_outside_the_area() {
    let mut st = UiState::default();
    scrolled(&mut st, &hover(10.0, 150.0));
    scrolled(&mut st, &wheel_at(10.0, 150.0, -1.0));
    assert_eq!(st.scroll.get(&id("sa")).copied().unwrap_or(0.0), 0.0);
}

#[test]
fn short_content_does_not_scroll() {
    let mut st = UiState::default();
    let run = |st: &mut UiState, input: &Input| {
        frame(st, input, |ui| {
            ui.scroll_area(id("sa"), 100.0, 60.0, |_| {})
        })
    };
    run(&mut st, &hover(10.0, 50.0));
    run(&mut st, &wheel_at(10.0, 50.0, -1.0));
    assert_eq!(st.scroll[&id("sa")], 0.0);
}

#[test]
fn scroll_content_is_clipped() {
    let mut st = UiState::default();
    let f = scrolled(&mut st, &Input::default());
    assert_eq!(f.cmds[0], Cmd::ClipPush(Rect::new(0.0, 0.0, 214.0, 100.0)));
    assert!(f.cmds.contains(&Cmd::ClipPop));
    // A button scrolled out of the viewport does not react.
    st.scroll.insert(id("sa"), 50.0);
    let build = |ui: &mut Ui| {
        let mut c = false;
        ui.scroll_area(id("sa"), 100.0, 300.0, |ui| {
            ui.space(120.0);
            c = ui.button(id("ok"), "OK");
        });
        c
    };
    // Content y 120..148 is at screen 70..98 inside; below the viewport at 100+ it is not.
    assert!(click_at(&mut st, 10.0, 80.0, build));
    st.scroll.insert(id("sa"), 0.0);
    assert!(!click_at(&mut st, 10.0, 130.0, build));
}

#[test]
fn set_scroll_is_clamped_when_built() {
    let mut st = UiState::default();
    frame(&mut st, &Input::default(), |ui| {
        ui.set_scroll(id("sa"), 1000.0);
        ui.scroll_area(id("sa"), 100.0, 300.0, |_| {});
    });
    assert_eq!(st.scroll[&id("sa")], 200.0);
}

#[test]
fn scroll_area_inside_an_offset_region_is_not_shifted_twice() {
    let mut st = UiState::default();
    st.scroll.insert(id("sa"), 40.0);
    let f = frame(&mut st, &Input::default(), |ui| {
        ui.offset(0.0, 30.0, |ui| {
            ui.scroll_area(id("sa"), 100.0, 300.0, |ui| {
                ui.button(id("ok"), "OK");
            })
        })
    });
    assert_eq!(f.cmds[0], Cmd::ClipPush(Rect::new(0.0, 30.0, 214.0, 100.0)));
    assert_eq!(first_fill_y(&f), 30.0 - 40.0);
}

#[test]
fn nested_scroll_areas_only_the_one_under_the_pointer_scrolls() {
    let mut st = UiState::default();
    let run = |st: &mut UiState, input: &Input| {
        frame(st, input, |ui| {
            ui.scroll_area(id("outer"), 200.0, 600.0, |ui| {
                ui.scroll_area(id("inner"), 100.0, 300.0, |_| {});
            })
        })
    };
    run(&mut st, &hover(10.0, 50.0));
    run(&mut st, &wheel_at(10.0, 50.0, -1.0));
    assert_eq!(st.scroll[&id("inner")], 40.0);
    assert_eq!(st.scroll[&id("outer")], 0.0);
    run(&mut st, &hover(10.0, 150.0));
    run(&mut st, &wheel_at(10.0, 150.0, -1.0));
    assert_eq!(st.scroll[&id("inner")], 40.0);
    assert_eq!(st.scroll[&id("outer")], 40.0);
}

// --- sidebar: rows 32 high (indented 26) with 2 between ---

const ITEMS: [SidebarItem; 4] = [
    SidebarItem {
        label: "Settings",
        indent: false,
        enabled: true,
        note: None,
    },
    SidebarItem {
        label: "Overlay",
        indent: true,
        enabled: true,
        note: None,
    },
    SidebarItem {
        label: "Appearance",
        indent: true,
        enabled: true,
        note: None,
    },
    SidebarItem {
        label: "Sound mixer",
        indent: false,
        enabled: false,
        note: Some("soon"),
    },
];

#[test]
fn sidebar_selects_enabled_items() {
    let mut st = UiState::default();
    let mut sel = 1usize;
    // Appearance spans y 62..88.
    assert!(click_at(&mut st, 20.0, 70.0, |ui| ui.sidebar(
        id("nav"),
        &ITEMS,
        &mut sel
    )));
    assert_eq!(sel, 2);
    assert!(!click_at(&mut st, 20.0, 70.0, |ui| ui.sidebar(
        id("nav"),
        &ITEMS,
        &mut sel
    )));
    assert!(click_at(&mut st, 20.0, 10.0, |ui| ui.sidebar(
        id("nav"),
        &ITEMS,
        &mut sel
    )));
    assert_eq!(sel, 0);
}

#[test]
fn disabled_sidebar_items_are_not_selectable() {
    let mut st = UiState::default();
    let mut sel = 1usize;
    // Sound mixer spans y 90..122.
    assert!(!click_at(&mut st, 20.0, 100.0, |ui| ui.sidebar(
        id("nav"),
        &ITEMS,
        &mut sel
    )));
    assert_eq!(sel, 1);
    let f = frame(&mut st, &hover(20.0, 100.0), |ui| {
        ui.sidebar(id("nav"), &ITEMS, &mut sel);
    });
    assert_eq!(f.cursor, CursorKind::Arrow);
    let t = texts(&f);
    assert!(t.contains(&"Sound mixer".to_string()) && t.contains(&"soon".to_string()));
}

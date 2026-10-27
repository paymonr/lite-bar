use super::*;

#[test]
fn default_chip_resets_counter_size() {
    let mut p = Page::new(TALL);
    p.settings.overlay.counter_size = 40;
    p.idle();
    let a = p.click_text("Default 18px");
    assert_eq!(a, vec![UiAction::Change(SettingsChange::CounterSize(18))]);
    p.idle();
    assert_eq!(find_text(p.frame(), "18px").unwrap().2, theme::SEGMENT_TEXT);
}

#[test]
fn default_chips_reset_text_size_and_opacity() {
    let mut p = Page::new(TALL);
    p.settings.overlay.text_size = 20;
    p.settings.overlay.background_opacity = 90;
    p.idle();
    let mut a = p.click_text("Default 11px");
    a.extend(p.click_text("Default 55%"));
    assert_eq!(
        changes(&a),
        vec![
            SettingsChange::TextSize(11),
            SettingsChange::BackgroundOpacity(55)
        ]
    );
}

#[test]
fn dragging_opacity_emits_changes() {
    let mut p = Page::new(TALL);
    let track = track_near(p.frame(), "Background opacity");
    let y = track.y + 1.0;
    let at = |t: f32| Pt {
        x: track.x + 7.0 + t * (track.w - 14.0),
        y,
    };
    let mut a = p.run(pointer(at(0.55)));
    a.extend(p.run(Input {
        pointer: Some(at(0.2)),
        down: true,
        pressed: true,
        ..Input::default()
    }));
    a.extend(p.run(Input {
        pointer: Some(at(0.8)),
        down: true,
        ..Input::default()
    }));
    a.extend(p.run(Input {
        pointer: Some(at(0.8)),
        released: true,
        ..Input::default()
    }));
    assert_eq!(
        changes(&a),
        vec![
            SettingsChange::BackgroundOpacity(20),
            SettingsChange::BackgroundOpacity(80)
        ]
    );
    assert!(find_text(p.frame(), "80%").is_some());
}

#[test]
fn swatch_click_emits_number_color() {
    let mut p = Page::new(TALL);
    let red = Rgba::from(PRESET_COLORS[3]);
    // Rows are Number, Text, Background: the first red swatch is the Number row's.
    let sw = fills(p.frame())
        .into_iter()
        .find(|(r, c, _)| r.w == theme::SWATCH && *c == red)
        .unwrap()
        .0;
    let a = p.click(center(sw));
    assert_eq!(
        a,
        vec![UiAction::Change(SettingsChange::NumberColor(
            PRESET_COLORS[3]
        ))]
    );
}

#[test]
fn saved_colors_show_in_every_row() {
    let mut p = Page::new(TALL);
    p.settings.saved_colors = vec![rgb("#123456"), rgb("#654321")];
    p.idle();
    assert_eq!(dashed(p.frame()).len(), 6);
}

#[test]
fn right_click_on_a_saved_color_removes_it() {
    let mut p = Page::new(TALL);
    p.settings.saved_colors = vec![rgb("#123456"), rgb("#654321")];
    p.idle();
    // The Background row's second saved swatch.
    let sw = center(dashed(p.frame())[5]);
    p.run(pointer(sw));
    p.run(pointer(sw));
    let a = p.run(Input {
        pointer: Some(sw),
        right_released: true,
        ..Input::default()
    });
    assert_eq!(
        a,
        vec![UiAction::Change(SettingsChange::SavedColors(vec![rgb(
            "#123456"
        )]))]
    );
}

#[test]
fn mixer_use_for_text_emits_label_color() {
    let mut p = Page::new(TALL);
    p.view.mixer = Hsv {
        h: 0.0,
        s: 1.0,
        v: 1.0,
    };
    p.idle();
    let a = p.click_text("Use for text");
    assert_eq!(
        a,
        vec![UiAction::Change(SettingsChange::LabelColor(rgb("#FF0000")))]
    );
    let a = p.click_text("Use for number");
    assert_eq!(
        a,
        vec![UiAction::Change(SettingsChange::NumberColor(rgb(
            "#FF0000"
        )))]
    );
    let a = p.click_text("Use for background");
    assert_eq!(
        a,
        vec![UiAction::Change(SettingsChange::BackgroundColor(rgb(
            "#FF0000"
        )))]
    );
}

#[test]
fn mixer_shows_its_color_in_the_chip_and_hex() {
    let mut p = Page::new(TALL);
    p.view.mixer = Hsv {
        h: 120.0,
        s: 1.0,
        v: 1.0,
    };
    p.idle();
    assert!(find_text(p.frame(), "#00FF00").is_some());
    let green = Rgba(0, 255, 0, 255);
    assert!(fills(p.frame())
        .iter()
        .any(|(r, c, _)| *c == green && r.w == 56.0 && r.h == 40.0));
}

#[test]
fn mixer_hex_entry_sets_the_mixer() {
    let mut p = Page::new(TALL);
    // The Text row's hex also shows #FFFFFF; the mixer's comes last.
    click_field(&mut p, "#FFFFFF");
    p.keys(vec![
        KeyPress {
            ctrl: true,
            ..key(UiKey::A)
        },
        key(UiKey::Backspace),
    ]);
    p.typing("#0000FF");
    assert_eq!(p.keys(vec![key(UiKey::Enter)]), vec![]);
    assert_eq!(p.view.mixer.to_rgb(), rgb("#0000FF"));
}

#[test]
fn brightness_slider_sets_mixer_value() {
    let mut p = Page::new(TALL);
    let track = track_near(p.frame(), "Brightness");
    let mid = Pt {
        x: track.x + track.w / 2.0,
        y: track.y + 1.0,
    };
    p.click(mid);
    assert!((p.view.mixer.v - 0.5).abs() < 0.011);
}

#[test]
fn dragging_the_wheel_sets_the_mixer_hue() {
    let mut p = Page::new(TALL);
    let wheel = p
        .frame()
        .cmds
        .iter()
        .find_map(|c| match c {
            Cmd::Image { rect, .. } if rect.w == 170.0 => Some(*rect),
            _ => None,
        })
        .unwrap();
    // Right edge: hue 90, full saturation.
    p.click(Pt {
        x: wheel.right() - 2.0,
        y: wheel.y + 85.0,
    });
    assert!((p.view.mixer.h - 90.0).abs() < 1.0);
    assert!(p.view.mixer.s > 0.95);
}

#[test]
fn save_to_my_colors_appends_and_caps_at_16() {
    let mut p = Page::new(TALL);
    p.view.mixer = Hsv {
        h: 0.0,
        s: 1.0,
        v: 1.0,
    };
    p.idle();
    let a = p.click_text("+ Save to my colors");
    assert_eq!(
        a,
        vec![UiAction::Change(SettingsChange::SavedColors(vec![rgb(
            "#FF0000"
        )]))]
    );

    let full: Vec<Rgb> = (0..MAX_SAVED_COLORS as u8)
        .map(|i| Rgb { r: i, g: i, b: 1 })
        .collect();
    p.settings.saved_colors = full.clone();
    p.idle();
    let a = p.click_text("+ Save to my colors");
    let mut expected = full[1..].to_vec();
    expected.push(rgb("#FF0000"));
    assert_eq!(
        a,
        vec![UiAction::Change(SettingsChange::SavedColors(expected))]
    );
    assert_eq!(p.settings.saved_colors.len(), MAX_SAVED_COLORS);
}

#[test]
fn save_to_my_colors_skips_a_duplicate() {
    let mut p = Page::new(TALL);
    p.view.mixer = Hsv {
        h: 0.0,
        s: 1.0,
        v: 1.0,
    };
    p.settings.saved_colors = vec![rgb("#FF0000")];
    p.idle();
    assert_eq!(p.click_text("+ Save to my colors"), vec![]);
}

#[test]
fn use_for_number_is_the_accent_button() {
    let p = Page::new(TALL);
    let accent_under = |p: &Page, text: &str| {
        let at = text_center(p.frame(), text);
        fills(p.frame())
            .iter()
            .any(|(r, c, _)| *c == theme::ACCENT && r.contains(at))
    };
    assert!(accent_under(&p, "Use for number"));
    assert!(!accent_under(&p, "Use for text"));
}

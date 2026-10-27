use super::*;
use crate::color::Rgb;
use crate::hotkey::Key;
use crate::settings::{Location, StatsLayout, MAX_SAVED_COLORS};
use crate::settings_edit::apply_change;
use crate::ui::{id, theme, Cmd, Frame, Input, KeyPress, Pt, Rect, Rgba, TextMeasure, TextStyle};
use crate::ui::{UiKey, UiState, PRESET_COLORS};

struct Fake;
impl TextMeasure for Fake {
    fn measure(&self, text: &str, style: TextStyle) -> (f32, f32) {
        (7.0 * text.chars().count() as f32, style.size)
    }
    fn baseline(&self, style: TextStyle) -> f32 {
        0.8 * style.size
    }
}

/// Tall enough that the whole page shows without scrolling.
const TALL: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 900.0,
    h: 3000.0,
};
/// The mockup's window body.
const MOCKUP: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 900.0,
    h: 640.0,
};

fn rgb(hex: &str) -> Rgb {
    Rgb::parse_hex(hex).unwrap()
}

fn game(exe: &str, stored: &str, name: Option<&str>) -> GameRow {
    GameRow {
        exe_path: exe.to_string(),
        stored_name: stored.to_string(),
        override_name: name.map(str::to_string),
    }
}

/// The settings page plus a host that applies every `Change` like the app does.
struct Page {
    st: UiState,
    view: SettingsView,
    settings: Settings,
    games: Vec<GameRow>,
    status: Option<String>,
    area: Rect,
    now: u64,
    last: Option<Frame>,
}

impl Page {
    fn new(area: Rect) -> Page {
        let mut p = Page {
            st: UiState::default(),
            view: SettingsView::default(),
            settings: Settings::default(),
            games: vec![
                game("javaw.exe", "Minecraft", None),
                game("eldenring.exe", "ELDEN RING", Some("Elden Ring")),
            ],
            status: None,
            area,
            now: 0,
            last: None,
        };
        // Two idle frames so the layout settles (content height, heading offsets).
        p.idle();
        p.idle();
        p
    }

    fn run(&mut self, input: Input) -> Vec<UiAction> {
        let input = Input {
            now_ms: self.now,
            ..input
        };
        let mut ui = Ui::new(&Fake, &input, &mut self.st, self.area);
        let actions = settings_frame(
            &mut ui,
            &mut self.view,
            &self.settings,
            &self.games,
            self.status.as_deref(),
        );
        self.last = Some(ui.finish());
        for a in &actions {
            if let UiAction::Change(c) = a {
                apply_change(&mut self.settings, c);
            }
        }
        actions
    }

    fn idle(&mut self) -> Vec<UiAction> {
        self.run(Input::default())
    }

    fn frame(&self) -> &Frame {
        self.last.as_ref().expect("a frame was run")
    }

    /// Hover, press and release at `p`; returns every action emitted on the way.
    fn click(&mut self, p: Pt) -> Vec<UiAction> {
        let mut out = self.run(pointer(p));
        out.extend(self.run(Input {
            pointer: Some(p),
            down: true,
            pressed: true,
            ..Input::default()
        }));
        out.extend(self.run(Input {
            pointer: Some(p),
            released: true,
            ..Input::default()
        }));
        out
    }

    fn click_text(&mut self, text: &str) -> Vec<UiAction> {
        let p = text_center(self.frame(), text);
        self.click(p)
    }

    fn keys(&mut self, keys: Vec<KeyPress>) -> Vec<UiAction> {
        self.run(Input {
            keys,
            ..Input::default()
        })
    }

    fn typing(&mut self, s: &str) -> Vec<UiAction> {
        self.run(Input {
            text: s.chars().collect(),
            ..Input::default()
        })
    }
}

fn pointer(p: Pt) -> Input {
    Input {
        pointer: Some(p),
        ..Input::default()
    }
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

fn texts(f: &Frame) -> Vec<(Pt, String, TextStyle, Rgba)> {
    f.cmds
        .iter()
        .filter_map(|c| match c {
            Cmd::Text {
                pos,
                text,
                style,
                color,
            } => Some((*pos, text.clone(), *style, *color)),
            _ => None,
        })
        .collect()
}

fn find_text(f: &Frame, text: &str) -> Option<(Pt, TextStyle, Rgba)> {
    texts(f)
        .into_iter()
        .find(|t| t.1 == text)
        .map(|t| (t.0, t.2, t.3))
}

/// Centre of a drawn text box (the fake measure: 7 per char, as tall as the font size).
fn text_center(f: &Frame, text: &str) -> Pt {
    let (pos, style, _) = find_text(f, text).unwrap_or_else(|| panic!("no text {text:?}"));
    Pt {
        x: pos.x + 3.5 * text.chars().count() as f32,
        y: pos.y + style.size / 2.0,
    }
}

fn fills(f: &Frame) -> Vec<(Rect, Rgba, f32)> {
    f.cmds
        .iter()
        .filter_map(|c| match c {
            Cmd::Fill {
                rect,
                color,
                radius,
            } => Some((*rect, *color, *radius)),
            _ => None,
        })
        .collect()
}

fn center(r: Rect) -> Pt {
    Pt {
        x: r.x + r.w / 2.0,
        y: r.y + r.h / 2.0,
    }
}

/// The slider track in the same row as `label`.
fn track_near(f: &Frame, label: &str) -> Rect {
    let y = find_text(f, label).unwrap().0.y;
    fills(f)
        .into_iter()
        .find(|(r, c, _)| {
            r.h == theme::SLIDER_TRACK_H && *c == theme::BORDER && (r.y - y).abs() < 30.0
        })
        .unwrap_or_else(|| panic!("no slider next to {label:?}"))
        .0
}

/// The preview stage: the gradient image, 92 tall.
fn stage(f: &Frame) -> Rect {
    f.cmds
        .iter()
        .find_map(|c| match c {
            Cmd::Image { rect, .. } if rect.h == 92.0 => Some(*rect),
            _ => None,
        })
        .expect("a preview stage")
}

/// The preview overlay's background box (default black at 55%).
fn preview_box(f: &Frame) -> Option<Rect> {
    fills(f)
        .into_iter()
        .find(|(_, c, _)| *c == Rgba(0, 0, 0, 140))
        .map(|f| f.0)
}

fn changes(actions: &[UiAction]) -> Vec<SettingsChange> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Change(c) => Some(c.clone()),
            _ => None,
        })
        .collect()
}

// ---- Overlay section ----

#[test]
fn clicking_stacked_emits_stats_change() {
    let mut p = Page::new(TALL);
    let a = p.click_text("Stacked");
    assert_eq!(
        a,
        vec![UiAction::Change(SettingsChange::Stats(
            StatsLayout::Stacked
        ))]
    );
}

#[test]
fn show_overlay_toggle_emits_visible() {
    let mut p = Page::new(TALL);
    let toggle = fills(p.frame())
        .into_iter()
        .find(|(r, _, _)| r.w == theme::TOGGLE_W && r.h == theme::TOGGLE_H)
        .unwrap()
        .0;
    let a = p.click(center(toggle));
    assert_eq!(a, vec![UiAction::Change(SettingsChange::Visible(false))]);
}

#[test]
fn location_click_emits_change() {
    let mut p = Page::new(TALL);
    let screen = fills(p.frame())
        .into_iter()
        .find(|(r, _, _)| r.w == theme::SCREEN_W && r.h == theme::SCREEN_H)
        .unwrap()
        .0;
    // Bottom-right button: inset 8, steps of 65 across and 30 down, 14 wide.
    let a = p.click(Pt {
        x: screen.x + 8.0 + 130.0 + 7.0,
        y: screen.y + 8.0 + 60.0 + 7.0,
    });
    assert_eq!(
        a,
        vec![UiAction::Change(SettingsChange::Location(
            Location::BottomRight
        ))]
    );
}

// ---- Navigation ----

#[test]
fn sidebar_switches_page() {
    let mut p = Page::new(MOCKUP);
    assert_eq!(p.view.selected, NAV_OVERLAY);
    p.click_text("Hotkeys");
    assert_eq!(p.view.selected, NAV_HOTKEYS);
    // Disabled future pages cannot be selected.
    p.click_text("History");
    assert_eq!(p.view.selected, NAV_HOTKEYS);
}

#[test]
fn appearance_nav_scrolls_to_heading() {
    let mut p = Page::new(MOCKUP);
    let top = find_text(p.frame(), "OVERLAY").unwrap().0.y;
    p.click_text("Appearance");
    p.idle();
    assert_eq!(find_text(p.frame(), "APPEARANCE").unwrap().0.y, top);
}

#[test]
fn overlay_nav_scrolls_to_top() {
    let mut p = Page::new(MOCKUP);
    let top = find_text(p.frame(), "OVERLAY").unwrap().0.y;
    p.click_text("Hotkeys");
    p.idle();
    assert!(find_text(p.frame(), "OVERLAY").unwrap().0.y < top);
    p.click_text("Overlay");
    p.idle();
    assert_eq!(p.st.scroll.get(&id("settings.scroll")), Some(&0.0));
    assert_eq!(find_text(p.frame(), "OVERLAY").unwrap().0.y, top);
}

#[test]
fn content_height_change_asks_for_a_redraw() {
    let mut p = Page::new(TALL);
    p.idle();
    assert_eq!(p.frame().redraw_after_ms, None);
    p.games.clear();
    p.idle();
    assert_eq!(p.frame().redraw_after_ms, Some(p.now));
}

// ---- Preview ----

#[test]
fn preview_shows_144_and_hides_when_overlay_off() {
    let mut p = Page::new(TALL);
    let (_, style, color) = find_text(p.frame(), "144").expect("preview number");
    assert!(style.mono && style.bold);
    assert_eq!(color, Rgba::from(p.settings.overlay.number_color));
    // Stats values are runs of their own, padded: " 138  ".
    assert!(texts(p.frame()).iter().any(|t| t.1.trim() == "138"));
    p.settings.overlay.visible = false;
    p.idle();
    assert!(find_text(p.frame(), "144").is_none());
    assert!(preview_box(p.frame()).is_none());
    // The stage itself stays.
    stage(p.frame());
}

#[test]
fn preview_without_stats_shows_only_the_counter() {
    let mut p = Page::new(TALL);
    p.settings.overlay.stats = StatsLayout::Off;
    p.idle();
    assert!(find_text(p.frame(), "144").is_some());
    assert!(find_text(p.frame(), "avg").is_none());
}

#[test]
fn preview_sits_in_the_chosen_corner_of_the_stage() {
    let mut p = Page::new(TALL);
    let s = stage(p.frame());
    let b = preview_box(p.frame()).unwrap();
    assert_eq!((b.x, b.y), (s.x + 8.0, s.y + 8.0));
    p.settings.overlay.location = Location::BottomRight;
    p.idle();
    let b = preview_box(p.frame()).unwrap();
    assert_eq!((b.right(), b.bottom()), (s.right() - 8.0, s.bottom() - 8.0));
}

#[test]
fn preview_scales_down_only_when_wider_than_the_stage() {
    let p = Page::new(TALL);
    let full = preview_box(p.frame()).unwrap();
    let s = stage(p.frame());
    assert!(full.w < s.w - 16.0, "fits at full size in a wide window");
    let number_size = find_text(p.frame(), "144").unwrap().1.size;
    assert_eq!(number_size, 18.0);

    let p = Page::new(Rect::new(0.0, 0.0, 400.0, 3000.0));
    let s = stage(p.frame());
    let b = preview_box(p.frame()).unwrap();
    assert!(full.w > s.w - 16.0, "too wide for the narrow stage");
    assert!(b.w <= s.w - 16.0 + 0.5, "scaled to fit: {b:?} in {s:?}");
    assert_eq!(b.x, s.x + 8.0);
    let k = b.w / full.w;
    let number_size = find_text(p.frame(), "144").unwrap().1.size;
    assert!((number_size - 18.0 * k).abs() < 0.1);
}

#[test]
fn stage_gradient_is_generated_once() {
    let mut p = Page::new(TALL);
    let image = |f: &Frame| {
        f.cmds
            .iter()
            .find_map(|c| match c {
                Cmd::Image { rect, rgba, .. } if rect.h == 92.0 => Some(rgba.clone()),
                _ => None,
            })
            .unwrap()
    };
    let first = image(p.frame());
    p.idle();
    assert!(std::rc::Rc::ptr_eq(&first, &image(p.frame())));
}

// ---- Appearance ----

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

fn dashed(f: &Frame) -> Vec<Rect> {
    f.cmds
        .iter()
        .filter_map(|c| match c {
            Cmd::Dashed { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect()
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

// ---- Hotkeys ----

#[test]
fn hotkey_capture_emits_proposal() {
    let mut p = Page::new(TALL);
    p.click_text("Ctrl");
    let a = p.keys(vec![KeyPress {
        ctrl: true,
        alt: true,
        ..key(UiKey::Other(0x4B))
    }]);
    assert_eq!(
        a,
        vec![UiAction::ProposeHotkey(
            Hotkey::new(true, true, false, false, Key::Letter('K')).unwrap()
        )]
    );
}

#[test]
fn esc_cancels_hotkey_capture() {
    let mut p = Page::new(TALL);
    p.click_text("Ctrl");
    let a = p.keys(vec![key(UiKey::Esc)]);
    assert_eq!(a, vec![UiAction::CancelHotkeyCapture]);
}

#[test]
fn hotkey_status_shows_under_the_field() {
    let mut p = Page::new(TALL);
    p.status = Some("Saved".to_string());
    p.idle();
    let field_y = find_text(p.frame(), "Ctrl").unwrap().0.y;
    let (pos, _, color) = find_text(p.frame(), "Saved").unwrap();
    assert_eq!(color, theme::MUTED);
    assert!(pos.y > field_y);
    p.status = Some("Already used by another app".to_string());
    p.idle();
    let (_, _, color) = find_text(p.frame(), "Already used by another app").unwrap();
    assert_eq!(color, Rgba(0xFF, 0x8A, 0x80, 255));
}

// ---- Game names ----

/// Clicks the last drawn non-bold `shown` text (a field's text or placeholder; row titles
/// are bold) near its left edge.
fn click_field(p: &mut Page, shown: &str) {
    let f = p.frame();
    let (pos, _, _) = texts(f)
        .into_iter()
        .filter(|t| t.1 == shown && !t.2.bold)
        .map(|t| (t.0, t.2, t.3))
        .next_back()
        .unwrap();
    p.click(Pt {
        x: pos.x + 2.0,
        y: pos.y + 4.0,
    });
}

#[test]
fn game_rename_commit_emits_game_name() {
    let mut p = Page::new(TALL);
    click_field(&mut p, "Minecraft");
    // Typing alone keeps the edit in the view; nothing is saved per keystroke.
    assert_eq!(p.typing("Craft"), vec![]);
    assert!(find_text(p.frame(), "Craft").is_some());
    let a = p.keys(vec![key(UiKey::Enter)]);
    assert_eq!(
        a,
        vec![UiAction::Change(SettingsChange::GameName {
            exe_path: "javaw.exe".to_string(),
            name: Some("Craft".to_string()),
        })]
    );
}

#[test]
fn clearing_rename_emits_none() {
    let mut p = Page::new(TALL);
    click_field(&mut p, "Elden Ring");
    p.keys(vec![
        KeyPress {
            ctrl: true,
            ..key(UiKey::A)
        },
        key(UiKey::Backspace),
    ]);
    let a = p.keys(vec![key(UiKey::Enter)]);
    assert_eq!(
        a,
        vec![UiAction::Change(SettingsChange::GameName {
            exe_path: "eldenring.exe".to_string(),
            name: None,
        })]
    );
}

#[test]
fn rename_is_emitted_on_blur() {
    let mut p = Page::new(TALL);
    click_field(&mut p, "Minecraft");
    p.typing("  Craft ");
    let a = p.click(Pt {
        x: 300.0,
        y: 2990.0,
    });
    assert_eq!(
        changes(&a),
        vec![SettingsChange::GameName {
            exe_path: "javaw.exe".to_string(),
            name: Some("Craft".to_string()),
        }]
    );
}

#[test]
fn unchanged_rename_emits_nothing() {
    let mut p = Page::new(TALL);
    click_field(&mut p, "Elden Ring");
    assert_eq!(p.keys(vec![key(UiKey::Enter)]), vec![]);
}

#[test]
fn rename_is_emitted_when_the_field_disappears() {
    let mut p = Page::new(TALL);
    click_field(&mut p, "Minecraft");
    p.typing("Craft");
    p.games.retain(|g| g.exe_path != "javaw.exe");
    let a = p.idle();
    assert_eq!(
        changes(&a),
        vec![SettingsChange::GameName {
            exe_path: "javaw.exe".to_string(),
            name: Some("Craft".to_string()),
        }]
    );
    // Only once.
    assert_eq!(p.idle(), vec![]);
}

#[test]
fn finish_edits_returns_a_pending_rename() {
    let mut p = Page::new(TALL);
    click_field(&mut p, "Minecraft");
    p.typing("Craft");
    assert_eq!(
        changes(&p.view.finish_edits()),
        vec![SettingsChange::GameName {
            exe_path: "javaw.exe".to_string(),
            name: Some("Craft".to_string()),
        }]
    );
    assert_eq!(p.view.finish_edits(), vec![]);
}

#[test]
fn rename_field_follows_the_settings_when_idle() {
    let mut p = Page::new(TALL);
    p.games[1].override_name = Some("Tarnished".to_string());
    p.idle();
    assert!(find_text(p.frame(), "Tarnished").is_some());
}

#[test]
fn open_stats_folder_button() {
    let mut p = Page::new(TALL);
    assert_eq!(
        p.click_text("Open stats folder"),
        vec![UiAction::OpenStatsFolder]
    );
}

#[test]
fn empty_games_shows_hint() {
    let mut p = Page::new(TALL);
    assert!(find_text(p.frame(), GAMES_HINT).is_none());
    p.games.clear();
    p.idle();
    assert!(find_text(p.frame(), GAMES_HINT).is_some());
    assert!(find_text(p.frame(), "Open stats folder").is_none());
}

// ---- Restore defaults ----

#[test]
fn restore_defaults_needs_confirm_then_emits_restore() {
    let mut p = Page::new(TALL);
    p.settings.overlay.counter_size = 40;
    p.idle();
    assert_eq!(p.click_text("Restore defaults"), vec![]);
    assert!(find_text(p.frame(), "Click again to confirm").is_some());
    let a = p.click_text("Click again to confirm");
    assert_eq!(a, vec![UiAction::Change(SettingsChange::RestoreDefaults)]);
    assert_eq!(p.settings.overlay.counter_size, 18);
    p.idle();
    assert!(find_text(p.frame(), "Defaults restored.").is_some());
}

#[test]
fn restore_confirm_expires_after_3_seconds() {
    let mut p = Page::new(TALL);
    p.click_text("Restore defaults");
    p.now = 3000;
    p.idle();
    assert_eq!(p.click_text("Restore defaults"), vec![]);
}

#[test]
fn restore_resyncs_the_hex_fields() {
    let mut p = Page::new(TALL);
    p.settings.overlay.number_color = rgb("#123456");
    p.idle();
    assert!(find_text(p.frame(), "#123456").is_some());
    p.click_text("Restore defaults");
    p.click_text("Click again to confirm");
    p.idle();
    assert!(find_text(p.frame(), "#123456").is_none());
    assert!(find_text(p.frame(), "#7CFC7C").is_some());
}

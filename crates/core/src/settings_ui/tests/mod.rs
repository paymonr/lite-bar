mod appearance;
mod games;
mod hotkeys;
mod layout;
mod nav;
mod overlay;
mod preview;
mod restore;
mod status;
mod updates;

use super::*;
use crate::color::Rgb;
use crate::hotkey::Key;
use crate::settings::{Location, StatsLayout, MAX_SAVED_COLORS};
use crate::settings_edit::apply_change;
use crate::ui::{id, theme, Cmd, Frame, Input, KeyPress, Pt, Rect, Rgba, TextMeasure, TextStyle};
use crate::ui::{UiKey, UiState, PRESET_COLORS};
use crate::update::{current_version, ReleaseInfo, UpdateState};

/// Fixed-width text: `char_w` per char, lines `line` × the font size tall.
struct Fake {
    char_w: f32,
    line: f32,
}
const FAKE: Fake = Fake {
    char_w: 7.0,
    line: 1.0,
};
/// Wider and taller than Segoe UI, for layout checks.
const CONSERVATIVE: Fake = Fake {
    char_w: 7.5,
    line: 1.35,
};
impl TextMeasure for Fake {
    fn measure(&self, text: &str, style: TextStyle) -> (f32, f32) {
        (
            self.char_w * text.chars().count() as f32,
            self.line * style.size,
        )
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
        override_key: name.map(|_| exe.to_string()),
    }
}

/// The settings page plus a host that applies every `Change` like the app does.
struct Page {
    st: UiState,
    view: SettingsView,
    settings: Settings,
    games: Vec<GameRow>,
    status: Option<String>,
    save_error: Option<String>,
    update: UpdateState,
    area: Rect,
    now: u64,
    last: Option<Frame>,
    measure: &'static Fake,
}

impl Page {
    fn new(area: Rect) -> Page {
        Page::with_measure(area, &FAKE)
    }

    fn with_measure(area: Rect, measure: &'static Fake) -> Page {
        let mut p = Page {
            st: UiState::default(),
            view: SettingsView::default(),
            settings: Settings::default(),
            games: vec![
                game("javaw.exe", "Minecraft", None),
                game("eldenring.exe", "ELDEN RING", Some("Elden Ring")),
            ],
            status: None,
            save_error: None,
            update: UpdateState::Idle,
            area,
            now: 0,
            last: None,
            measure,
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
        let mut ui = Ui::new(self.measure, &input, &mut self.st, self.area);
        let actions = settings_frame(
            &mut ui,
            &mut self.view,
            &self.settings,
            &self.games,
            self.status.as_deref(),
            self.save_error.as_deref(),
            UpdatesInput {
                state: &self.update,
                current: current_version(),
            },
        );
        self.last = Some(ui.finish());
        for a in &actions {
            if let UiAction::Change(c) = a {
                apply_change(&mut self.settings, c);
            }
        }
        actions
    }

    /// Hovers the scrolling content and turns the wheel `notches` (negative scrolls down).
    fn wheel(&mut self, notches: f32) {
        let p = Pt {
            x: self.area.w - 60.0,
            y: self.area.h - 20.0,
        };
        self.run(pointer(p));
        self.run(Input {
            pointer: Some(p),
            wheel: notches,
            ..Input::default()
        });
    }

    fn scroll(&self) -> f32 {
        self.st
            .scroll
            .get(&id("settings.scroll"))
            .copied()
            .unwrap_or(0.0)
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

fn dashed(f: &Frame) -> Vec<Rect> {
    f.cmds
        .iter()
        .filter_map(|c| match c {
            Cmd::Dashed { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect()
}

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

/// A release newer than this build.
fn release() -> ReleaseInfo {
    ReleaseInfo {
        version: "0.5.0".parse().unwrap(),
        page_url: "https://github.com/paymonr/lite-bar/releases/tag/v0.5.0".into(),
        exe_url: "https://github.com/paymonr/lite-bar/releases/download/v0.5.0/lite-bar.exe".into(),
        sha_url: "https://github.com/paymonr/lite-bar/releases/download/v0.5.0/lite-bar.exe.sha256"
            .into(),
    }
}

/// Every state the Updates section can show.
fn all_update_states() -> Vec<UpdateState> {
    vec![
        UpdateState::Idle,
        UpdateState::Checking,
        UpdateState::UpToDate,
        UpdateState::Available(release()),
        UpdateState::Downloading(release()),
        UpdateState::Failed {
            why: "Can't update in this folder — download it from GitHub".into(),
            retry: false,
        },
        UpdateState::Failed {
            why: "download failed (download took too long)".into(),
            retry: true,
        },
        UpdateState::RestartRequired,
        UpdateState::CheckFailed,
    ]
}

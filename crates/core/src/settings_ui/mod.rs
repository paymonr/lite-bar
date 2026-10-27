//! The Settings page (mockup v3), built from the UI toolkit: a sidebar, a pinned live
//! preview and one scrolling page of sections. Pure: no IO, and no clock but `Input::now_ms`.
//! The host applies the returned actions and passes the updated settings next frame.

mod appearance;
mod games;
mod hotkeys;
mod overlay;
mod preview;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::rc::Rc;

use crate::hotkey::Hotkey;
use crate::settings::Settings;
use crate::settings_edit::SettingsChange;
use crate::ui::{
    id, theme, Cmd, HotkeyCapture, Hsv, Id, Pt, Rect, Rgba, SidebarItem, TextEdit, TextStyle, Ui,
};

/// Sidebar items, in order.
pub const NAV_SETTINGS: usize = 0;
pub const NAV_OVERLAY: usize = 1;
pub const NAV_APPEARANCE: usize = 2;
pub const NAV_HOTKEYS: usize = 3;
pub const NAV_GAMES: usize = 4;

pub const GAMES_HINT: &str = "Games appear here after you play one for over a minute.";

const SIDEBAR_W: f32 = 170.0;
const SIDEBAR_BG: Rgba = Rgba(0x19, 0x1B, 0x20, 255);
const LOGO_BG: Rgba = Rgba(0x2F, 0x6F, 0xDB, 255);
/// Line between rows inside a card.
const ROW_LINE: Rgba = Rgba(0x30, 0x34, 0x3D, 255);
const PAGE_PAD_X: f32 = 24.0;
const SCROLL_PAD_TOP: f32 = 14.0;
const SCROLL_PAD_BOTTOM: f32 = 24.0;
const ROW_H: f32 = 32.0;
const CARD_SPACING: f32 = 10.0;

const HINT: TextStyle = TextStyle {
    size: 12.0,
    bold: false,
    mono: false,
};
const HEADING: TextStyle = TextStyle {
    size: 11.0,
    bold: false,
    mono: false,
};

fn scroll_id() -> Id {
    id("settings.scroll")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameRow {
    pub exe_path: String,
    /// The name in the stats store, shown as the row title and the field's placeholder.
    pub stored_name: String,
    /// The user's name from `[game_names]`, if any.
    pub override_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UiAction {
    Change(SettingsChange),
    /// Try to register this hotkey; on failure set `SettingsView::hotkey` to
    /// `HotkeyCapture::rejected(..)`.
    ProposeHotkey(Hotkey),
    OpenStatsFolder,
    CancelHotkeyCapture,
}

/// A game's name field. The text stays here while it is edited and is only emitted
/// (as `GameName`) on Enter, on blur or when the field goes away.
#[derive(Clone, Debug)]
struct GameEdit {
    edit: TextEdit,
    /// The override the field last showed or emitted.
    synced: Option<String>,
    /// Edited since the last commit.
    pending: bool,
}

impl GameEdit {
    fn new(name: Option<&str>) -> GameEdit {
        GameEdit {
            edit: TextEdit::new(name.unwrap_or("")),
            synced: name.map(str::to_string),
            pending: false,
        }
    }

    /// Ends an edit: the trimmed text (empty clears the override), as an action when it
    /// differs from what the field started with.
    fn commit(&mut self, exe_path: &str) -> Option<UiAction> {
        self.pending = false;
        let trimmed = self.edit.text.trim();
        let name = (!trimmed.is_empty()).then(|| trimmed.to_string());
        self.edit = TextEdit::new(name.as_deref().unwrap_or(""));
        if name == self.synced {
            return None;
        }
        self.synced = name.clone();
        Some(UiAction::Change(SettingsChange::GameName {
            exe_path: exe_path.to_string(),
            name,
        }))
    }
}

/// State the page keeps between frames.
#[derive(Clone, Debug)]
pub struct SettingsView {
    /// Selected sidebar item (`NAV_*`).
    pub selected: usize,
    pub hotkey: HotkeyCapture,
    /// Hex fields of the Number, Text and Background color rows.
    pub hex: [TextEdit; 3],
    /// The color mixer's color.
    pub mixer: Hsv,
    pub mixer_hex: TextEdit,
    /// Name fields by exe path.
    games: BTreeMap<String, GameEdit>,
    /// Scroll offsets that bring each section heading to the top, from the last frame.
    headings: [f32; 4],
    /// Height of the scrolling content in the last frame.
    content_h: f32,
    /// Shows "Defaults restored." until the next change.
    restored: bool,
    /// The preview stage's background, generated once.
    stage_bg: Option<Rc<Vec<u8>>>,
}

impl Default for SettingsView {
    fn default() -> Self {
        SettingsView {
            selected: NAV_OVERLAY,
            hotkey: HotkeyCapture::Idle,
            hex: Default::default(),
            mixer: Hsv {
                h: 0.0,
                s: 0.0,
                v: 1.0,
            },
            mixer_hex: TextEdit::default(),
            games: BTreeMap::new(),
            headings: [0.0; 4],
            content_h: 0.0,
            restored: false,
            stage_bg: None,
        }
    }
}

impl SettingsView {
    /// Ends any game-name edit still in progress, e.g. when the window closes.
    pub fn finish_edits(&mut self) -> Vec<UiAction> {
        self.games
            .iter_mut()
            .filter(|(_, g)| g.pending)
            .filter_map(|(exe, g)| g.commit(exe))
            .collect()
    }
}

/// Builds the whole window: sidebar, pinned header with the preview, scrolling sections.
pub fn settings_frame(
    ui: &mut Ui,
    view: &mut SettingsView,
    settings: &Settings,
    games: &[GameRow],
    hotkey_status: Option<&str>,
) -> Vec<UiAction> {
    let mut out = Vec::new();
    let area = ui.available();
    let side = Rect::new(area.x, area.y, SIDEBAR_W.min(area.w), area.h);
    sidebar(ui, view, side);
    let main = Rect::new(
        side.right() + 1.0,
        area.y,
        (area.w - side.w - 1.0).max(0.0),
        area.h,
    );
    let header_h = preview::header(ui, view, &settings.overlay, main);
    let body = Rect::new(
        main.x,
        main.y + header_h,
        main.w,
        (main.h - header_h).max(0.0),
    );
    let content_h = view.content_h;
    let mut new_h = content_h;
    ui.region(body, |ui| {
        ui.scroll_area(scroll_id(), body.h, content_h, |ui| {
            new_h = content(ui, view, settings, games, hotkey_status, &mut out);
        });
    });
    if new_h != view.content_h {
        view.content_h = new_h;
        ui.request_redraw();
    }
    for a in &out {
        if let UiAction::Change(c) = a {
            view.restored = *c == SettingsChange::RestoreDefaults;
        }
    }
    out
}

fn sidebar(ui: &mut Ui, view: &mut SettingsView, side: Rect) {
    ui.paint(Cmd::Fill {
        rect: side,
        color: SIDEBAR_BG,
        radius: 0.0,
    });
    ui.paint(Cmd::Fill {
        rect: Rect::new(side.right(), side.y, 1.0, side.h),
        color: theme::BORDER,
        radius: 0.0,
    });
    let inner = Rect::new(
        side.x + 8.0,
        side.y + 14.0,
        (side.w - 16.0).max(0.0),
        (side.h - 14.0).max(0.0),
    );
    ui.region(inner, |ui| {
        ui.row(22.0, |ui| {
            ui.space(8.0);
            let a = ui.available();
            let logo = Rect::new(a.x, a.y, 22.0, 22.0);
            ui.paint(Cmd::Fill {
                rect: logo,
                color: LOGO_BG,
                radius: 5.0,
            });
            paint_centered(ui, logo, "L", TextStyle::BOLD, theme::SIDEBAR_SELECTED_TEXT);
            ui.space(22.0 + 8.0);
            ui.label("Lite Bar", TextStyle::BOLD, theme::TEXT);
        });
        ui.space(14.0);
        let item = |label, indent| SidebarItem {
            label,
            indent,
            enabled: true,
            note: None,
        };
        let items = [
            item("Settings", false),
            item("Overlay", true),
            item("Appearance", true),
            item("Hotkeys", true),
            item("Game names", true),
        ];
        if ui.sidebar(id("settings.nav"), &items, &mut view.selected) {
            let y = match view.selected {
                NAV_SETTINGS | NAV_OVERLAY => 0.0,
                i => view.headings[i - 1],
            };
            ui.set_scroll(scroll_id(), y);
        }
        ui.space(14.0);
        let later = |label, note| SidebarItem {
            label,
            indent: false,
            enabled: false,
            note: Some(note),
        };
        let future = [
            later("Sound mixer", "soon"),
            later("Optimize", "later"),
            later("History", "later"),
        ];
        let mut none = usize::MAX;
        ui.sidebar(id("settings.nav.later"), &future, &mut none);
    });
}

/// The scrolling sections; returns their total height.
fn content(
    ui: &mut Ui,
    view: &mut SettingsView,
    settings: &Settings,
    games: &[GameRow],
    hotkey_status: Option<&str>,
    out: &mut Vec<UiAction>,
) -> f32 {
    let top = ui.available();
    let inner = Rect::new(
        top.x + PAGE_PAD_X,
        top.y,
        (top.w - 2.0 * PAGE_PAD_X).max(0.0),
        f32::MAX / 4.0,
    );
    let mut bottom = top.y;
    ui.region(inner, |ui| {
        ui.space(SCROLL_PAD_TOP);
        let first = heading(ui, "OVERLAY", 2.0);
        overlay::section(ui, &settings.overlay, out);
        view.headings[1] = heading(ui, "APPEARANCE", 16.0) - first;
        appearance::section(ui, view, settings, out);
        view.headings[2] = heading(ui, "HOTKEYS", 16.0) - first;
        hotkeys::section(ui, view, settings, hotkey_status, out);
        view.headings[3] = heading(ui, "GAME NAMES", 16.0) - first;
        games::section(ui, view, games, out);
        restore_row(ui, view, out);
        bottom = ui.available().y;
    });
    bottom - top.y + SCROLL_PAD_BOTTOM
}

/// A small-caps section heading `margin` below the previous item; returns its y.
fn heading(ui: &mut Ui, text: &str, margin: f32) -> f32 {
    ui.space(margin);
    let y = ui.available().y;
    ui.label(text, HEADING, theme::MUTED);
    ui.space(8.0);
    y
}

/// Space after a card.
fn end_card(ui: &mut Ui) {
    ui.space(CARD_SPACING);
}

fn restore_row(ui: &mut Ui, view: &SettingsView, out: &mut Vec<UiAction>) {
    const TEXT: &str = "Restore defaults";
    const CONFIRM: &str = "Click again to confirm";
    let m = ui.text_measure();
    // Room for either label, so the button does not jump when armed.
    let w = m
        .measure(TEXT, TextStyle::BODY)
        .0
        .max(m.measure(CONFIRM, TextStyle::BOLD).0)
        + 2.0 * theme::BUTTON_PAD_X;
    ui.space(18.0 - CARD_SPACING);
    ui.row(ROW_H, |ui| {
        ui.right_aligned(w, |ui| {
            if ui.confirm_button(id("settings.restore"), TEXT, CONFIRM) {
                out.push(UiAction::Change(SettingsChange::RestoreDefaults));
            }
        });
        ui.column(|ui| {
            let lines: &[&str] = if view.restored {
                &["Defaults restored."]
            } else {
                &[
                    "Puts every overlay setting and the hotkey back to how Lite Bar ships.",
                    "Your game names and saved colors are kept.",
                ]
            };
            text_block(ui, ROW_H, lines);
        });
    });
}

/// Muted lines, vertically centred in a column `h` tall.
fn text_block(ui: &mut Ui, h: f32, lines: &[&str]) {
    let line_h = ui.text_measure().measure("Ag", HINT).1;
    let n = lines.len() as f32;
    let total = line_h * n + 2.0 * (n - 1.0).max(0.0);
    ui.space(((h - total) / 2.0).max(0.0));
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            ui.space(2.0);
        }
        ui.label(l, HINT, theme::MUTED);
    }
}

/// A card row `h` tall: `title` (bold) and an optional muted `hint` on the left, and the
/// widgets built by `control`, `control_w` wide, on the right.
fn setting_row<'a>(
    ui: &mut Ui<'a>,
    h: f32,
    title: &str,
    hint: Option<&str>,
    control_w: f32,
    control: impl FnOnce(&mut Ui<'a>),
) {
    ui.row(h, |ui| {
        ui.right_aligned(control_w, control);
        ui.column(|ui| title_block(ui, h, title, hint));
    });
}

/// A bold title over an optional muted hint, vertically centred in a column `h` tall.
fn title_block(ui: &mut Ui, h: f32, title: &str, hint: Option<&str>) {
    let m = ui.text_measure();
    let title_h = m.measure(title, TextStyle::BOLD).1;
    let hint_h = hint.map_or(0.0, |t| 2.0 + m.measure(t, HINT).1);
    ui.space(((h - title_h - hint_h) / 2.0).max(0.0));
    ui.label(title, TextStyle::BOLD, theme::TEXT);
    if let Some(hint) = hint {
        ui.space(2.0);
        ui.label(hint, HINT, theme::MUTED);
    }
}

/// The line between two rows of a card.
fn row_line(ui: &mut Ui) {
    ui.space(7.0);
    let a = ui.available();
    ui.paint(Cmd::Fill {
        rect: Rect::new(a.x, a.y, a.w, 1.0),
        color: ROW_LINE,
        radius: 0.0,
    });
    ui.space(8.0);
}

fn paint_centered(ui: &mut Ui, rect: Rect, text: &str, style: TextStyle, color: Rgba) {
    let (w, h) = ui.text_measure().measure(text, style);
    ui.paint(Cmd::Text {
        pos: Pt {
            x: rect.x + (rect.w - w) / 2.0,
            y: rect.y + (rect.h - h) / 2.0,
        },
        text: text.to_string(),
        style,
        color,
    });
}

/// Width of a `Ui::button` labelled `text`.
fn button_width(ui: &Ui, text: &str) -> f32 {
    ui.text_measure().measure(text, TextStyle::BODY).0 + 2.0 * theme::BUTTON_PAD_X
}

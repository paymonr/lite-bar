//! The Settings page (mockup v3), built from the UI toolkit: a sidebar, a pinned live
//! preview and one scrolling page of sections. Pure: no IO, and no clock but `Input::now_ms`.
//! The host applies the returned actions and passes the updated settings next frame.

mod appearance;
mod games;
mod general;
mod hotkeys;
mod overlay;
mod preview;
mod updates;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::rc::Rc;

use crate::hotkey::Hotkey;
use crate::settings::Settings;
use crate::settings_edit::SettingsChange;
use crate::startup_task::StartupState;
use crate::ui::{
    id, theme, Cmd, HotkeyCapture, Hsv, Id, Pt, Rect, Rgba, SidebarItem, TextEdit, TextStyle, Ui,
};
use crate::update::{UpdateState, Version};

/// Sidebar sections under the static "Settings" header, in page order.
pub const NAV_OVERLAY: usize = 0;
pub const NAV_APPEARANCE: usize = 1;
pub const NAV_GENERAL: usize = 2;
pub const NAV_HOTKEYS: usize = 3;
pub const NAV_UPDATES: usize = 4;
pub const NAV_GAMES: usize = 5;
const SECTIONS: usize = 6;

/// Smallest window (width, height) in DIPs the page is laid out for.
pub const MIN_WINDOW_DIP: (f32, f32) = (720.0, 520.0);

pub const GAMES_HINT: &str = "Games appear here after you play one for over a minute.";

const SIDEBAR_W: f32 = 170.0;
const SIDEBAR_BG: Rgba = Rgba(0x19, 0x1B, 0x20, 255);
const LOGO_BG: Rgba = Rgba(0x2F, 0x6F, 0xDB, 255);
/// Status text for problems: a failed save, a hotkey another app holds.
const PROBLEM_TEXT: Rgba = Rgba(0xFF, 0x8A, 0x80, 255);
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
    /// The `[game_names]` key `override_name` came from (the full path or the exe file name);
    /// renames and clears edit this key. `None` adds a key for `exe_path`.
    pub override_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UiAction {
    Change(SettingsChange),
    /// Try to register this hotkey; on failure set `SettingsView::hotkey` to
    /// `HotkeyCapture::rejected(..)`.
    ProposeHotkey(Hotkey),
    OpenStatsFolder,
    /// Hotkey capture began: release the toggle hotkey until capture ends
    /// (`ProposeHotkey`, `CancelHotkeyCapture` or the window closing).
    StartHotkeyCapture,
    CancelHotkeyCapture,
    /// "Check now": check for an update right away, even with checks turned off.
    CheckForUpdates,
    /// "Update now" (Settings or the tray): download and install the available release.
    InstallUpdate,
    /// Open the available release's page on GitHub.
    OpenReleasePage,
    /// "Restart now": start the already installed new version and quit this one.
    RestartToUpdate,
    /// The "Start with Windows" switch: create (true) or delete the logon task.
    SetStartup(bool),
}

/// What the Updates section shows: the app's update state and this build's version.
#[derive(Clone, Copy, Debug)]
pub struct UpdatesInput<'a> {
    pub state: &'a UpdateState,
    pub current: Version,
}

/// What the General section shows: the startup task's state and whether Lite Bar runs as
/// administrator (if not, changing it asks for admin once).
#[derive(Clone, Copy, Debug)]
pub struct StartupInput<'a> {
    pub state: &'a StartupState,
    pub elevated: bool,
}

/// What the app feeds the page besides the settings.
#[derive(Clone, Copy, Debug)]
pub struct PageInput<'a> {
    pub updates: UpdatesInput<'a>,
    pub startup: StartupInput<'a>,
}

/// A game's name field. The text stays here while it is edited and is only emitted
/// (as `GameName`) on Enter, on blur or when the field goes away.
#[derive(Clone, Debug)]
struct GameEdit {
    edit: TextEdit,
    /// The `[game_names]` key a commit edits (see `GameRow::override_key`).
    key: String,
    /// The override the field last showed or emitted.
    synced: Option<String>,
    /// Edited since the last commit.
    pending: bool,
}

impl GameEdit {
    fn new(key: &str, name: Option<&str>) -> GameEdit {
        GameEdit {
            edit: TextEdit::new(name.unwrap_or("")),
            key: key.to_string(),
            synced: name.map(str::to_string),
            pending: false,
        }
    }

    /// Ends an edit: the trimmed text (empty clears the override), as an action when it
    /// differs from what the field started with.
    fn commit(&mut self) -> Option<UiAction> {
        self.pending = false;
        let trimmed = self.edit.text.trim();
        let name = (!trimmed.is_empty()).then(|| trimmed.to_string());
        self.edit = TextEdit::new(name.as_deref().unwrap_or(""));
        if name == self.synced {
            return None;
        }
        self.synced = name.clone();
        Some(UiAction::Change(SettingsChange::GameName {
            exe_path: self.key.clone(),
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
    headings: [f32; SECTIONS],
    /// Height of the scrolling content in the last frame.
    content_h: f32,
    /// Scroll offset right after a sidebar click; while the page stays there the clicked
    /// section stays highlighted (the last one may never reach the top).
    nav_scroll: Option<f32>,
    /// Shows "Defaults restored." until the next change.
    restored: bool,
    /// The preview stage's background, generated once.
    stage_bg: Option<Rc<Vec<u8>>>,
    /// The Start with Windows switch's last real state (On or Off), kept for while the
    /// state is busy or a change failed.
    startup_shown: bool,
    /// The value a click asked for, shown while the change runs.
    startup_target: Option<bool>,
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
            headings: [0.0; SECTIONS],
            content_h: 0.0,
            nav_scroll: None,
            restored: false,
            stage_bg: None,
            startup_shown: false,
            startup_target: None,
        }
    }
}

impl SettingsView {
    /// Ends any game-name edit still in progress, e.g. when the window closes.
    pub fn finish_edits(&mut self) -> Vec<UiAction> {
        self.games
            .values_mut()
            .filter(|g| g.pending)
            .filter_map(GameEdit::commit)
            .collect()
    }
}

/// Builds the whole window: sidebar, pinned header with the preview, scrolling sections.
/// `save_error` (lines split by `\n`) shows pinned above the sections while saving fails.
/// `input` feeds the Updates and General sections.
pub fn settings_frame(
    ui: &mut Ui,
    view: &mut SettingsView,
    settings: &Settings,
    games: &[GameRow],
    hotkey_status: Option<&str>,
    save_error: Option<&str>,
    input: PageInput,
) -> Vec<UiAction> {
    let mut out = Vec::new();
    let area = ui.available();
    let side = Rect::new(area.x, area.y, SIDEBAR_W.min(area.w), area.h);
    let clicked = sidebar(ui, view, side);
    let main = Rect::new(
        side.right() + 1.0,
        area.y,
        (area.w - side.w - 1.0).max(0.0),
        area.h,
    );
    let mut header_h = preview::header(ui, view, &settings.overlay, main);
    if let Some(msg) = save_error {
        header_h += save_error_strip(ui, msg, main, main.y + header_h);
    }
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
            new_h = content(ui, view, settings, games, hotkey_status, input, &mut out);
        });
    });
    let scroll = ui.scroll_offset(scroll_id());
    if clicked {
        view.nav_scroll = Some(scroll);
    } else if view.nav_scroll != Some(scroll) {
        // Scroll-spy: the last section whose heading is at or above the viewport top, or
        // the last section once scrolled to the bottom (its heading may never reach the top).
        view.nav_scroll = None;
        let max = (content_h - body.h).max(0.0);
        view.selected = if max > 0.0 && scroll >= max - 0.5 {
            NAV_GAMES
        } else {
            view.headings
                .iter()
                .rposition(|h| *h <= scroll + 0.5)
                .unwrap_or(NAV_OVERLAY)
        };
    }
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

/// The lines of `msg` in the error color across `main` from `top`; returns the height used.
fn save_error_strip(ui: &mut Ui, msg: &str, main: Rect, top: f32) -> f32 {
    const PAD_Y: f32 = 8.0;
    let m = ui.text_measure();
    let max_w = (main.w - 2.0 * PAGE_PAD_X).max(0.0);
    let mut y = top + PAD_Y;
    for line in msg.lines() {
        let text = ellipsize(ui, line, TextStyle::BODY, max_w);
        let h = m.measure(&text, TextStyle::BODY).1;
        ui.paint(Cmd::Text {
            pos: Pt {
                x: main.x + PAGE_PAD_X,
                y,
            },
            text,
            style: TextStyle::BODY,
            color: PROBLEM_TEXT,
        });
        y += h + 2.0;
    }
    y + PAD_Y - 2.0 - top
}

/// Returns whether a section was clicked.
fn sidebar(ui: &mut Ui, view: &mut SettingsView, side: Rect) -> bool {
    let mut clicked = false;
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
        settings_header(ui);
        let item = |label| SidebarItem {
            label,
            indent: true,
            enabled: true,
            note: None,
        };
        let items = [
            item("Overlay"),
            item("Appearance"),
            item("General"),
            item("Hotkeys"),
            item("Updates"),
            item("Game names"),
        ];
        if let Some(i) = ui.sidebar(id("settings.nav"), &items, &mut view.selected) {
            // Any click scrolls, also on the selected section.
            ui.set_scroll(scroll_id(), view.headings[i]);
            clicked = true;
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
    clicked
}

/// The always-on "Settings" header above its sections, drawn like a selected item.
fn settings_header(ui: &mut Ui) {
    let a = ui.available();
    let rect = Rect::new(a.x, a.y, a.w, theme::NAV_H);
    ui.paint(Cmd::Fill {
        rect,
        color: theme::SURFACE,
        radius: theme::NAV_RADIUS,
    });
    ui.paint(Cmd::Fill {
        rect: Rect::new(rect.x, rect.y, 3.0, rect.h),
        color: theme::ACCENT,
        radius: 0.0,
    });
    let h = ui.text_measure().measure("Settings", TextStyle::BODY).1;
    ui.paint(Cmd::Text {
        pos: Pt {
            x: rect.x + theme::NAV_PAD_X,
            y: rect.y + (rect.h - h) / 2.0,
        },
        text: "Settings".to_string(),
        style: TextStyle::BODY,
        color: theme::SIDEBAR_SELECTED_TEXT,
    });
    ui.space(theme::NAV_H + theme::NAV_SPACING);
}

/// The scrolling sections; returns their total height.
fn content(
    ui: &mut Ui,
    view: &mut SettingsView,
    settings: &Settings,
    games: &[GameRow],
    hotkey_status: Option<&str>,
    input: PageInput,
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
        view.headings[NAV_APPEARANCE] = heading(ui, "APPEARANCE", 16.0) - first;
        appearance::section(ui, view, settings, out);
        view.headings[NAV_GENERAL] = heading(ui, "GENERAL", 16.0) - first;
        general::section(ui, view, input.startup, out);
        view.headings[NAV_HOTKEYS] = heading(ui, "HOTKEYS", 16.0) - first;
        hotkeys::section(ui, view, settings, hotkey_status, out);
        view.headings[NAV_UPDATES] = heading(ui, "UPDATES", 16.0) - first;
        updates::section(ui, settings, input.updates, out);
        view.headings[NAV_GAMES] = heading(ui, "GAME NAMES", 16.0) - first;
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
    let lines: &[&str] = if view.restored {
        &["Defaults restored."]
    } else {
        &[
            "Puts every overlay setting and the hotkey back to how Lite Bar ships.",
            "Your game names and saved colors are kept.",
        ]
    };
    ui.space(18.0 - CARD_SPACING);
    setting_row(ui, theme::CONTROL_HEIGHT, None, lines, w, |ui| {
        if ui.confirm_button(id("settings.restore"), TEXT, CONFIRM) {
            out.push(UiAction::Change(SettingsChange::RestoreDefaults));
        }
    });
}

/// `text` cut to fit `max_w`, ending in "…" when shortened.
fn ellipsize(ui: &Ui, text: &str, style: TextStyle, max_w: f32) -> String {
    let m = ui.text_measure();
    if m.measure(text, style).0 <= max_w {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let s: String = chars.iter().collect::<String>().trim_end().to_string() + "…";
        if m.measure(&s, style).0 <= max_w {
            return s;
        }
    }
    String::new()
}

/// Width and height of an optional bold title over muted lines.
fn text_size(ui: &Ui, title: Option<&str>, lines: &[&str]) -> (f32, f32) {
    let m = ui.text_measure();
    let mut w: f32 = 0.0;
    let mut h: f32 = 0.0;
    if let Some(t) = title {
        let (tw, th) = m.measure(t, TextStyle::BOLD);
        w = tw;
        h = th;
    }
    for (i, l) in lines.iter().enumerate() {
        let (lw, lh) = m.measure(l, HINT);
        w = w.max(lw);
        h += lh + if i > 0 || title.is_some() { 2.0 } else { 0.0 };
    }
    (w, h)
}

/// An optional bold title over muted lines, each cut to `max_w`, vertically centred in
/// `h` of the current column.
fn text_block(ui: &mut Ui, h: f32, title: Option<&str>, lines: &[&str], max_w: f32) {
    text_block_colored(ui, h, title, lines, theme::MUTED, max_w);
}

/// `text_block` with the lines in `color`.
fn text_block_colored(
    ui: &mut Ui,
    h: f32,
    title: Option<&str>,
    lines: &[&str],
    color: Rgba,
    max_w: f32,
) {
    let (_, text_h) = text_size(ui, title, lines);
    ui.space(((h - text_h) / 2.0).max(0.0));
    if let Some(t) = title {
        let t = ellipsize(ui, t, TextStyle::BOLD, max_w);
        ui.label(&t, TextStyle::BOLD, theme::TEXT);
    }
    for (i, l) in lines.iter().enumerate() {
        if i > 0 || title.is_some() {
            ui.space(2.0);
        }
        let l = ellipsize(ui, l, HINT, max_w);
        ui.label(&l, HINT, color);
    }
}

/// A card row: an optional bold `title` over muted `lines` on the left and the widgets
/// built by `control` (`control_w` wide, `control_h` tall) on the right. When both do not
/// fit side by side the control goes under the text, left-aligned. Text never runs
/// under the control; it is cut with "…" instead. Returns whether it stacked.
fn setting_row<'a>(
    ui: &mut Ui<'a>,
    control_h: f32,
    title: Option<&str>,
    lines: &[&str],
    control_w: f32,
    control: impl FnOnce(&mut Ui<'a>),
) -> bool {
    setting_row_colored(
        ui,
        control_h,
        title,
        lines,
        theme::MUTED,
        control_w,
        control,
    )
}

/// `setting_row` with the lines in `color`.
fn setting_row_colored<'a>(
    ui: &mut Ui<'a>,
    control_h: f32,
    title: Option<&str>,
    lines: &[&str],
    color: Rgba,
    control_w: f32,
    control: impl FnOnce(&mut Ui<'a>),
) -> bool {
    let row_w = ui.available().w;
    let (text_w, text_h) = text_size(ui, title, lines);
    if text_w + 2.0 * theme::GAP + control_w <= row_w {
        let h = control_h.max(text_h).max(ROW_H);
        ui.row(h, |ui| {
            ui.right_aligned(control_w, control);
            ui.column(|ui| {
                text_block_colored(ui, h, title, lines, color, row_w - control_w - theme::GAP)
            });
        });
        false
    } else {
        text_block_colored(ui, text_h, title, lines, color, row_w);
        ui.space(6.0);
        ui.row(control_h, |ui| {
            ui.next_width(control_w.min(row_w));
            ui.row(control_h, control);
        });
        true
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

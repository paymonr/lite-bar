//! The Games page: "Open stats folder", a card for the game being played (or played last),
//! the Games | Other apps tabs and a table of what Lite Bar recorded. Clicking a game's name renames it; "Not a game" moves it to Other apps and
//! "It's a game" moves it back (both are `[not_games]` changes, applied by the host).

use super::games_text::{fps_whole, last_played_text, played_short};
use super::{
    button_width, ellipsize, end_card, paint_centered, row_line, setting_row, text_block, GameEdit,
    GameRow, GamesInput, LiveGame, SettingsView, UiAction, GAMES_HINT, HEADING, HINT, PAGE_PAD_X,
    ROW_H, SCROLL_PAD_BOTTOM, SCROLL_PAD_TOP,
};
use std::collections::BTreeMap;

use crate::game_name::find_override;
use crate::history::LocalTime;
use crate::settings::Settings;
use crate::settings_edit::SettingsChange;
use crate::ui::{id, theme, Cmd, Id, Pt, Rect, Rgba, TextEdit, TextFieldEvent, TextStyle, Ui};

const TITLE: &str = "Games";
const PAGE_LINE: &str = "Everything you've played for over a minute. Click a name to rename it.";
const OPEN_FOLDER: &str = "Open stats folder";
const NOT_A_GAME: &str = "Not a game";
const IS_A_GAME: &str = "It's a game";
const OTHER_APPS_LINES: [&str; 2] = [
    "Not recorded, and left out of games.csv and sessions.csv.",
    "Their history is kept, so \"It's a game\" puts everything back.",
];
const NO_OTHER_APPS: &str =
    "No other apps. Use \"Not a game\" on the Games tab for anything that isn't a game.";

/// The narrowest the name column may get before the Sessions and Last columns are left out.
const MIN_NAME_W: f32 = 140.0;
const TABLE_ROW_H: f32 = 40.0;
const TILE: f32 = 30.0;
const COL_GAP: f32 = 8.0;
const FIELD_W: f32 = 220.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum GamesTab {
    #[default]
    Games,
    OtherApps,
}

/// What the page keeps between frames.
#[derive(Clone, Debug, Default)]
pub(super) struct GamesView {
    pub(super) tab: GamesTab,
    /// The key of the game whose name is being edited (games sharing an exe, like Java
    /// ones, share its name override but are edited one at a time).
    pub(super) editing: Option<String>,
    /// Height of the scrolling content in the last frame.
    pub(super) content_h: f32,
}

pub(super) fn scroll_id() -> Id {
    id("games.scroll")
}

/// One table row.
struct Row<'a> {
    key: &'a str,
    /// What the row shows as its name.
    name: &'a str,
    /// The exe file name, or the key of a marked app the store doesn't list.
    detail: &'a str,
    game: Option<&'a GameRow>,
}

/// The page's scrolling content; returns its height.
pub(super) fn content(
    ui: &mut Ui,
    view: &mut SettingsView,
    settings: &Settings,
    games: &[GameRow],
    input: GamesInput,
    out: &mut Vec<UiAction>,
) -> f32 {
    end_gone_edits(view, games, out);
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
        let bw = button_width(ui, OPEN_FOLDER);
        setting_row(
            ui,
            theme::CONTROL_HEIGHT,
            Some(TITLE),
            &[PAGE_LINE],
            bw,
            |ui| {
                if ui.button(id("games.open_stats"), OPEN_FOLDER) {
                    out.push(UiAction::OpenStatsFolder);
                }
            },
        );
        ui.space(14.0);
        if let Some(card) = active_card(games, &settings.not_games, input.live) {
            active_game(ui, view, &card, &settings.game_names, input.now, out);
            ui.space(4.0);
        }
        table(ui, view, settings, games, input.now, out);
        bottom = ui.available().y;
    });
    bottom - top.y + SCROLL_PAD_BOTTOM
}

/// Edits of games that went away (the list changed) end now.
fn end_gone_edits(view: &mut SettingsView, games: &[GameRow], out: &mut Vec<UiAction>) {
    let gone: Vec<String> = view
        .games
        .keys()
        .filter(|exe| !games.iter().any(|g| &g.exe_path == *exe))
        .cloned()
        .collect();
    for exe in gone {
        if let Some(mut g) = view.games.remove(&exe) {
            if g.pending {
                out.extend(g.commit());
            }
        }
    }
    let editing = view.games_view.editing.as_deref();
    if editing.is_some_and(|key| !games.iter().any(|g| g.key == key)) {
        view.games_view.editing = None;
    }
}

/// The tabs and the table under them.
fn table(
    ui: &mut Ui,
    view: &mut SettingsView,
    settings: &Settings,
    games: &[GameRow],
    now: LocalTime,
    out: &mut Vec<UiAction>,
) {
    let not_games = &settings.not_games;
    let mut sorted: Vec<&GameRow> = games.iter().collect();
    sorted.sort_by(|a, b| {
        b.last_played.cmp(&a.last_played).then_with(|| {
            a.stored_name
                .to_lowercase()
                .cmp(&b.stored_name.to_lowercase())
        })
    });
    let row_of = |g: &'_ GameRow| -> (String, String) {
        (
            g.override_name.clone().unwrap_or(g.stored_name.clone()),
            file_name(&g.exe_path).to_string(),
        )
    };
    let shown: Vec<&GameRow> = sorted
        .iter()
        .copied()
        .filter(|g| !not_games.contains_key(&g.key))
        .collect();
    let hidden: Vec<&GameRow> = sorted
        .iter()
        .copied()
        .filter(|g| not_games.contains_key(&g.key))
        .collect();
    let unlisted: Vec<(&String, &String)> = not_games
        .iter()
        .filter(|(key, _)| !games.iter().any(|g| &g.key == *key))
        .collect();

    let labels = [
        format!("Games {}", shown.len()),
        format!("Other apps {}", hidden.len() + unlisted.len()),
    ];
    let mut tab = match view.games_view.tab {
        GamesTab::Games => 0,
        GamesTab::OtherApps => 1,
    };
    if ui.segmented(id("games.tabs"), &mut tab, &[&labels[0], &labels[1]]) {
        out.extend(view.finish_edits());
        view.games_view.editing = None;
        view.games_view.tab = if tab == 0 {
            GamesTab::Games
        } else {
            GamesTab::OtherApps
        };
    }
    let other_apps = view.games_view.tab == GamesTab::OtherApps;
    if other_apps {
        ui.space(10.0);
        let w = ui.available().w;
        text_block(ui, 0.0, None, &OTHER_APPS_LINES, w);
    }
    ui.space(10.0);

    let names: Vec<(String, String)> = if other_apps { &hidden } else { &shown }
        .iter()
        .map(|g| row_of(g))
        .collect();
    let mut rows: Vec<Row> = if other_apps { &hidden } else { &shown }
        .iter()
        .zip(&names)
        .map(|(g, (name, exe))| Row {
            key: &g.key,
            name,
            detail: exe,
            game: Some(g),
        })
        .collect();
    if other_apps {
        rows.extend(unlisted.iter().map(|(key, name)| Row {
            key,
            name,
            detail: key,
            game: None,
        }));
    }

    ui.card(|ui| {
        if rows.is_empty() {
            let w = ui.available().w;
            let hint = if other_apps {
                NO_OTHER_APPS
            } else {
                GAMES_HINT
            };
            text_block(ui, ROW_H, None, &[hint], w);
            return;
        }
        let label = if other_apps { IS_A_GAME } else { NOT_A_GAME };
        let cols = Columns::new(ui.available(), button_width(ui, label));
        header_row(ui, &cols);
        for row in &rows {
            row_line(ui);
            ui.row(TABLE_ROW_H, |ui| {
                let r = ui.available();
                let cols = cols.at(r.y);
                table_row(ui, view, &cols, row, label, !other_apps, now, out);
            });
        }
    });
    end_card(ui);
}

/// Where the table's cells go, from a row's left edge `x` and width.
#[derive(Clone, Copy)]
struct Columns {
    x: f32,
    y: f32,
    name_x: f32,
    name_w: f32,
    /// Sessions, Played, Last, Avg, 1% low: the right edge of each, `None` when left out.
    nums: [Option<f32>; 5],
    button: Rect,
}

const NUM_HEADS: [&str; 5] = ["SESSIONS", "PLAYED", "LAST", "AVG", "1% LOW"];
const NUM_W: [f32; 5] = [56.0, 60.0, 104.0, 44.0, 52.0];

impl Columns {
    fn new(area: Rect, button_w: f32) -> Columns {
        let right = area.x + area.w;
        let button = Rect::new(right - button_w, 0.0, button_w, theme::CONTROL_HEIGHT);
        let name_x = area.x + TILE + COL_GAP;
        let layout = |wide: bool| {
            let mut nums = [None; 5];
            let mut edge = button.x - COL_GAP;
            for i in (0..5).rev() {
                if !wide && (i == 0 || i == 2) {
                    continue;
                }
                nums[i] = Some(edge);
                edge -= NUM_W[i] + COL_GAP;
            }
            (nums, edge)
        };
        let (mut nums, mut edge) = layout(true);
        if edge - name_x < MIN_NAME_W {
            (nums, edge) = layout(false);
        }
        Columns {
            x: area.x,
            y: area.y,
            name_x,
            name_w: (edge - name_x).max(0.0),
            nums,
            button,
        }
    }

    /// The same columns for a row at `y`.
    fn at(&self, y: f32) -> Columns {
        Columns {
            y,
            button: Rect::new(
                self.button.x,
                y + (TABLE_ROW_H - theme::CONTROL_HEIGHT) / 2.0,
                self.button.w,
                self.button.h,
            ),
            ..*self
        }
    }
}

fn header_row(ui: &mut Ui, cols: &Columns) {
    let h = ui.text_measure().measure("GAME", HEADING).1;
    ui.row(h + 4.0, |ui| {
        let y = ui.available().y;
        paint_text(ui, cols.x, y, "GAME", HEADING, theme::MUTED);
        for (head, right) in NUM_HEADS.iter().zip(cols.nums) {
            if let Some(right) = right {
                paint_right(ui, right, y, head, HEADING, theme::MUTED);
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn table_row(
    ui: &mut Ui,
    view: &mut SettingsView,
    cols: &Columns,
    row: &Row,
    label: &str,
    renamable: bool,
    now: LocalTime,
    out: &mut Vec<UiAction>,
) {
    let y = cols.y;
    let tile = Rect::new(cols.x, y + (TABLE_ROW_H - TILE) / 2.0, TILE, TILE);
    ui.paint(Cmd::Fill {
        rect: tile,
        color: theme::BUTTON_BG,
        radius: 6.0,
    });
    let letter: String = row
        .name
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    paint_centered(ui, tile, &letter, TextStyle::BOLD, theme::TEXT);

    let editing = renamable
        && row
            .game
            .is_some_and(|g| view.games_view.editing.as_deref() == Some(g.key.as_str()));
    match row.game {
        Some(game) if renamable => name_cell(ui, view, cols, game, row.name, editing, out),
        _ => {
            let name = ellipsize(ui, row.name, TextStyle::BOLD, cols.name_w);
            paint_text(
                ui,
                cols.name_x,
                y + 3.0,
                &name,
                TextStyle::BOLD,
                theme::TEXT,
            );
        }
    }
    if !editing {
        let detail = ellipsize(ui, row.detail, HINT, cols.name_w);
        paint_text(ui, cols.name_x, y + 22.0, &detail, HINT, theme::MUTED);
    }

    if let Some(g) = row.game {
        let texts = [
            g.sessions.to_string(),
            played_short(g.play_secs),
            last_played_text(g.last_played, now),
            fps_whole(g.summary.map(|s| s.avg_fps)),
            fps_whole(g.summary.map(|s| s.low_1pct_fps)),
        ];
        for (i, (text, right)) in texts.iter().zip(cols.nums).enumerate() {
            if let Some(right) = right {
                let color = if i == 4 {
                    theme::SEGMENT_TEXT
                } else {
                    theme::TEXT
                };
                let style = if i == 2 { TextStyle::BODY } else { MONO };
                let h = ui.text_measure().measure(text, style).1;
                paint_right(ui, right, y + (TABLE_ROW_H - h) / 2.0, text, style, color);
            }
        }
    }

    let mut clicked = false;
    ui.place(cols.button, |ui| {
        clicked = ui.button(id(&format!("games.mark:{}", row.key)), label);
    });
    if clicked {
        let name = renamable.then(|| row.name.to_string());
        out.push(UiAction::Change(SettingsChange::NotAGame {
            key: row.key.to_string(),
            name,
        }));
    }
}

const MONO: TextStyle = TextStyle {
    size: 13.0,
    bold: false,
    mono: true,
};

/// A game's name: text that starts a rename when clicked, or the rename field.
fn name_cell(
    ui: &mut Ui,
    view: &mut SettingsView,
    cols: &Columns,
    game: &GameRow,
    shown: &str,
    editing: bool,
    out: &mut Vec<UiAction>,
) {
    let field = field_id(game);
    let key = game.override_key.as_deref().unwrap_or(&game.exe_path);
    let e = view
        .games
        .entry(game.exe_path.clone())
        .or_insert_with(|| GameEdit::new(key, game.override_name.as_deref()));
    if !e.pending && !ui.is_focused(field) && !editing {
        // Follow the settings (a reload, or our own commit applied).
        let current = game.override_name.as_deref().unwrap_or("");
        if e.edit.text != current {
            e.edit = TextEdit::new(current);
        }
        e.synced = game.override_name.clone();
        e.key = key.to_string();
    }
    let y = cols.y;
    if editing {
        let rect = Rect::new(
            cols.name_x,
            y + (TABLE_ROW_H - theme::CONTROL_HEIGHT) / 2.0,
            FIELD_W.min(cols.name_w),
            theme::CONTROL_HEIGHT,
        );
        let mut done = false;
        ui.place(rect, |ui| {
            match ui.text_field(field, &mut e.edit, &game.stored_name) {
                TextFieldEvent::None => {}
                TextFieldEvent::Changed => e.pending = true,
                TextFieldEvent::Committed => {
                    // The stored name itself is no override.
                    if e.edit.text.trim() == game.stored_name {
                        e.edit = TextEdit::new("");
                    }
                    out.extend(e.commit());
                    done = true;
                }
                TextFieldEvent::Cancelled => {
                    e.pending = false;
                    done = true;
                }
            }
        });
        if done {
            view.games_view.editing = None;
        }
        return;
    }
    let mut clicked = false;
    let h = ui.text_measure().measure(shown, TextStyle::BOLD).1;
    ui.place(Rect::new(cols.name_x, y + 3.0, cols.name_w, h), |ui| {
        clicked = ui.text_button(
            id(&format!("games.name:{}", game.exe_path)),
            shown,
            TextStyle::BOLD,
            theme::TEXT,
        );
    });
    if clicked {
        start_rename(ui, view, game, out);
    }
}

fn paint_text(ui: &mut Ui, x: f32, y: f32, text: &str, style: TextStyle, color: Rgba) {
    ui.paint(Cmd::Text {
        pos: Pt { x, y },
        text: text.to_string(),
        style,
        color,
    });
}

/// `text` ending at `right`.
fn paint_right(ui: &mut Ui, right: f32, y: f32, text: &str, style: TextStyle, color: Rgba) {
    let w = ui.text_measure().measure(text, style).0;
    paint_text(ui, right - w, y, text, style, color);
}

/// The last path component, for showing `C:\Games\x.exe` as `x.exe`.
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// What the card on top shows.
#[derive(Debug)]
pub(super) enum Card<'a> {
    /// The open session that had the focus last, with its game's row when it has one.
    Playing {
        live: &'a LiveGame,
        row: Option<&'a GameRow>,
    },
    /// Nothing is playing: the game played last.
    LastPlayed(&'a GameRow),
}

/// The card's game: the live one unless it's marked "Not a game", else the unmarked game
/// played last; `None` when there's neither.
pub(super) fn active_card<'a>(
    games: &'a [GameRow],
    not_games: &BTreeMap<String, String>,
    live: Option<&'a LiveGame>,
) -> Option<Card<'a>> {
    if let Some(live) = live.filter(|l| !not_games.contains_key(&l.key)) {
        let row = games.iter().find(|g| g.key == live.key);
        return Some(Card::Playing { live, row });
    }
    games
        .iter()
        .filter(|g| !not_games.contains_key(&g.key))
        .max_by_key(|g| g.last_played)
        .map(Card::LastPlayed)
}

const CARD_TILE: f32 = 40.0;
const CARD_TOP_H: f32 = 44.0;
const PANEL_H: f32 = 74.0;
/// Below this width the two panels stack.
const PANELS_SIDE_BY_SIDE_W: f32 = 560.0;
const PLAYING: Rgba = Rgba(0x7C, 0xFC, 0x7C, 255);
const PANEL_BG: Rgba = Rgba(0x1F, 0x21, 0x27, 255);
const RENAME: &str = "Rename";
/// The PLAYING NOW dot's diameter.
const DOT: f32 = 7.0;
const BIG: TextStyle = TextStyle {
    size: 22.0,
    bold: true,
    mono: true,
};
const MONO_BIG: TextStyle = TextStyle {
    size: 16.0,
    bold: false,
    mono: true,
};

/// One panel's three numbers with their captions.
struct Panel {
    title: &'static str,
    values: [(String, String); 3],
    /// The first value in the overlay's green (the live session).
    live: bool,
}

fn dashes(captions: [&str; 3]) -> [(String, String); 3] {
    captions.map(|c| ("—".to_string(), c.to_string()))
}

/// The card's two panels: this or the last session, and all time.
fn panels(card: &Card, row: Option<&GameRow>) -> (Panel, Panel) {
    let first = match card {
        Card::Playing { live, .. } => Panel {
            title: "THIS SESSION",
            values: match live.session {
                Some(s) => [
                    (fps_whole(Some(s.avg_fps)), "avg FPS".into()),
                    (fps_whole(Some(s.low_1pct_fps)), "1% low".into()),
                    (played_short(s.play_secs), "so far".into()),
                ],
                None => dashes(["avg FPS", "1% low", "so far"]),
            },
            live: true,
        },
        Card::LastPlayed(r) => Panel {
            title: "LAST SESSION",
            values: match r.last_session {
                Some(l) => [
                    (fps_whole(Some(l.avg_fps)), "avg FPS".into()),
                    (fps_whole(Some(l.low_1pct_fps)), "1% low".into()),
                    (played_short(l.play_secs), "played".into()),
                ],
                None => dashes(["avg FPS", "1% low", "played"]),
            },
            live: false,
        },
    };
    let all_time = Panel {
        title: "ALL TIME",
        values: match row {
            Some(r) => [
                (fps_whole(r.summary.map(|s| s.avg_fps)), "avg FPS".into()),
                (
                    fps_whole(r.summary.map(|s| s.low_1pct_fps)),
                    "1% low".into(),
                ),
                (
                    played_short(r.play_secs),
                    format!(
                        "{} session{}",
                        r.sessions,
                        if r.sessions == 1 { "" } else { "s" }
                    ),
                ),
            ],
            None => dashes(["avg FPS", "1% low", "played"]),
        },
        live: false,
    };
    (first, all_time)
}

fn active_game(
    ui: &mut Ui,
    view: &mut SettingsView,
    card: &Card,
    game_names: &BTreeMap<String, String>,
    now: LocalTime,
    out: &mut Vec<UiAction>,
) {
    let (name, key, row, badge) = match card {
        // Your rename wins over the looked-up name, as in the stats files.
        Card::Playing { live, row } => (
            find_override(game_names, &live.exe_path)
                .map_or_else(|| live.name.clone(), |(_, name)| name.to_string()),
            live.key.clone(),
            *row,
            "PLAYING NOW".to_string(),
        ),
        Card::LastPlayed(row) => (
            row.override_name.clone().unwrap_or(row.stored_name.clone()),
            row.key.clone(),
            Some(*row),
            format!("LAST PLAYED · {}", last_played_text(row.last_played, now)),
        ),
    };
    let (first, all_time) = panels(card, row);
    let playing = matches!(card, Card::Playing { .. });

    ui.card(|ui| {
        ui.row(CARD_TOP_H, |ui| {
            let r = ui.available();
            let tile = Rect::new(
                r.x,
                r.y + (CARD_TOP_H - CARD_TILE) / 2.0,
                CARD_TILE,
                CARD_TILE,
            );
            ui.paint(Cmd::Fill {
                rect: tile,
                color: theme::BUTTON_BG,
                radius: 8.0,
            });
            paint_centered(ui, tile, &initial(&name), TextStyle::BOLD, theme::TEXT);

            // On the right: Not a game, and Rename for a game the stats already list.
            let by = r.y + (CARD_TOP_H - theme::CONTROL_HEIGHT) / 2.0;
            let mark_w = button_width(ui, NOT_A_GAME);
            let mark = Rect::new(r.right() - mark_w, by, mark_w, theme::CONTROL_HEIGHT);
            let mut marked = false;
            ui.place(mark, |ui| {
                marked = ui.button(id("games.card.mark"), NOT_A_GAME)
            });
            if marked {
                out.push(UiAction::Change(SettingsChange::NotAGame {
                    key: key.clone(),
                    name: Some(name.clone()),
                }));
            }
            let mut left = mark.x;
            if let Some(row) = row {
                let w = button_width(ui, RENAME);
                left = mark.x - COL_GAP - w;
                let rect = Rect::new(left, by, w, theme::CONTROL_HEIGHT);
                let mut clicked = false;
                ui.place(rect, |ui| {
                    clicked = ui.button(id("games.card.rename"), RENAME)
                });
                if clicked {
                    start_rename(ui, view, row, out);
                }
            }

            let text_x = tile.right() + 12.0;
            let text_w = (left - COL_GAP - text_x).max(0.0);
            let badge_color = if playing { PLAYING } else { theme::MUTED };
            let mut badge_x = text_x;
            if playing {
                // A small green dot before PLAYING NOW.
                let h = ui.text_measure().measure(&badge, HEADING).1;
                ui.paint(Cmd::Fill {
                    rect: Rect::new(text_x, r.y + 4.0 + (h - DOT) / 2.0, DOT, DOT),
                    color: PLAYING,
                    radius: DOT / 2.0,
                });
                badge_x += DOT + 6.0;
            }
            let badge = ellipsize(ui, &badge, HEADING, (text_w - (badge_x - text_x)).max(0.0));
            paint_text(ui, badge_x, r.y + 4.0, &badge, HEADING, badge_color);
            let shown = ellipsize(ui, &name, TextStyle::BOLD, text_w);
            paint_text(ui, text_x, r.y + 22.0, &shown, TextStyle::BOLD, theme::TEXT);
        });
        ui.space(10.0);
        if ui.available().w >= PANELS_SIDE_BY_SIDE_W {
            ui.row(PANEL_H, |ui| {
                let r = ui.available();
                let half = (r.w - 12.0) / 2.0;
                panel(ui, Rect::new(r.x, r.y, half, PANEL_H), &first);
                panel(
                    ui,
                    Rect::new(r.x + half + 12.0, r.y, half, PANEL_H),
                    &all_time,
                );
            });
        } else {
            ui.row(PANEL_H, |ui| {
                let r = ui.available();
                panel(ui, r, &first);
            });
            ui.space(8.0);
            ui.row(PANEL_H, |ui| {
                let r = ui.available();
                panel(ui, r, &all_time);
            });
        }
    });
    end_card(ui);
}

fn panel(ui: &mut Ui, rect: Rect, p: &Panel) {
    ui.paint(Cmd::Fill {
        rect,
        color: PANEL_BG,
        radius: 6.0,
    });
    let x = rect.x + 12.0;
    paint_text(ui, x, rect.y + 8.0, p.title, HEADING, theme::MUTED);
    let col_w = ((rect.w - 24.0) / 3.0).max(0.0);
    let base = rect.y + 52.0;
    for (i, (value, caption)) in p.values.iter().enumerate() {
        let vx = x + col_w * i as f32;
        let (style, color) = match (i, p.live) {
            (0, true) => (BIG, PLAYING),
            (0, false) => (BIG, theme::TEXT),
            _ => (MONO_BIG, theme::TEXT),
        };
        let value = ellipsize(ui, value, style, (col_w - 8.0).max(0.0));
        let vh = ui.text_measure().measure(&value, style).1;
        paint_text(ui, vx, base - vh, &value, style, color);
        let caption = ellipsize(ui, caption, HINT, (col_w - 8.0).max(0.0));
        paint_text(ui, vx, base + 2.0, &caption, HINT, theme::MUTED);
    }
}

/// The first letter of `name`, upper case, for the letter tile.
fn initial(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default()
}

/// Starts renaming `row` in the table, switching to the Games tab if needed.
fn start_rename(ui: &mut Ui, view: &mut SettingsView, row: &GameRow, out: &mut Vec<UiAction>) {
    if view.games_view.tab != GamesTab::Games {
        out.extend(view.finish_edits());
        view.games_view.tab = GamesTab::Games;
    }
    view.games_view.editing = Some(row.key.clone());
    // The field starts with the name shown, selected, so typing replaces it.
    let key = row.override_key.as_deref().unwrap_or(&row.exe_path);
    let e = view
        .games
        .entry(row.exe_path.clone())
        .or_insert_with(|| GameEdit::new(key, row.override_name.as_deref()));
    if !e.pending {
        let shown = row.override_name.as_deref().unwrap_or(&row.stored_name);
        e.edit = TextEdit::new(shown);
        e.edit.all_selected = true;
    }
    ui.focus(field_id(row));
}

/// The rename field of `game`'s row.
fn field_id(game: &GameRow) -> Id {
    id(&format!("settings.game:{}", game.key))
}

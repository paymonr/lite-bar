//! Games section: a name field and a "Not a game" button per played game, the apps marked
//! "Not a game" with "It's a game" to undo it, and the stats folder button.

use std::collections::BTreeMap;

use super::{
    button_width, end_card, row_line, setting_row, text_block, GameEdit, GameRow, SettingsView,
    UiAction, GAMES_HINT, ROW_H,
};
use crate::settings_edit::SettingsChange;
use crate::ui::{id, theme, TextEdit, TextFieldEvent, TextStyle, Ui};

const FIELD_W: f32 = 220.0;
const GAME_ROW_H: f32 = theme::CONTROL_HEIGHT;
const OPEN_FOLDER: &str = "Open stats folder";
const NOT_A_GAME: &str = "Not a game";
const IS_A_GAME: &str = "It's a game";
const NOT_GAMES: &str = "Not games";
const NOT_GAMES_HINT: &str = "Not recorded, and left out of the stats files";

pub(super) fn section(
    ui: &mut Ui,
    view: &mut SettingsView,
    games: &[GameRow],
    not_games: &BTreeMap<String, String>,
    out: &mut Vec<UiAction>,
) {
    // Fields that went away (the list changed) end their edit now.
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

    let (shown, hidden): (Vec<&GameRow>, Vec<&GameRow>) =
        games.iter().partition(|g| !not_games.contains_key(&g.key));
    // Marked apps the stats store doesn't list (yet, or any more) can still be undone.
    let unlisted: Vec<(&String, &String)> = not_games
        .iter()
        .filter(|(key, _)| !games.iter().any(|g| &g.key == *key))
        .collect();

    ui.card(|ui| {
        if games.is_empty() && unlisted.is_empty() {
            let w = ui.available().w;
            text_block(ui, ROW_H, None, &[GAMES_HINT], w);
            return;
        }
        if shown.is_empty() {
            let w = ui.available().w;
            text_block(ui, ROW_H, None, &[GAMES_HINT], w);
        }
        for (i, game) in shown.iter().enumerate() {
            if i > 0 {
                row_line(ui);
            }
            let key = game.override_key.as_deref().unwrap_or(&game.exe_path);
            let edit = view
                .games
                .entry(game.exe_path.clone())
                .or_insert_with(|| GameEdit::new(key, game.override_name.as_deref()));
            game_row(ui, game, edit, out);
        }
        if !hidden.is_empty() || !unlisted.is_empty() {
            row_line(ui);
            text_block(
                ui,
                ROW_H,
                Some(NOT_GAMES),
                &[NOT_GAMES_HINT],
                ui.available().w,
            );
            for game in &hidden {
                row_line(ui);
                let name = game.override_name.as_deref().unwrap_or(&game.stored_name);
                is_a_game_row(ui, &game.key, name, file_name(&game.exe_path), out);
            }
            for (key, name) in &unlisted {
                row_line(ui);
                is_a_game_row(ui, key, name, key, out);
            }
        }
        row_line(ui);
        let bw = button_width(ui, OPEN_FOLDER);
        let lines = ["games.csv and sessions.csv open in Excel"];
        setting_row(ui, theme::CONTROL_HEIGHT, None, &lines, bw, |ui| {
            if ui.button(id("settings.open_stats"), OPEN_FOLDER) {
                out.push(UiAction::OpenStatsFolder);
            }
        });
    });
    end_card(ui);
}

fn game_row(ui: &mut Ui, game: &GameRow, e: &mut GameEdit, out: &mut Vec<UiAction>) {
    let field = id(&format!("settings.game:{}", game.exe_path));
    if !e.pending && !ui.is_focused(field) {
        // Follow the settings (a reload, or our own commit applied).
        let shown = game.override_name.as_deref().unwrap_or("");
        if e.edit.text != shown {
            e.edit = TextEdit::new(shown);
        }
        e.synced = game.override_name.clone();
        e.key = game
            .override_key
            .clone()
            .unwrap_or_else(|| game.exe_path.clone());
    }
    let exe = file_name(&game.exe_path);
    let bw = ui.text_measure().measure(NOT_A_GAME, TextStyle::BODY).0 + 2.0 * theme::BUTTON_PAD_X;
    let field_w = FIELD_W.min(ui.available().w - bw - theme::GAP).max(0.0);
    setting_row(
        ui,
        GAME_ROW_H,
        Some(&game.stored_name),
        &[exe],
        field_w + theme::GAP + bw,
        |ui| {
            ui.next_width(field_w);
            match ui.text_field(field, &mut e.edit, &game.stored_name) {
                TextFieldEvent::None => {}
                TextFieldEvent::Changed => e.pending = true,
                TextFieldEvent::Committed => out.extend(e.commit()),
                TextFieldEvent::Cancelled => e.pending = false,
            }
            if ui.button(id(&format!("settings.not_game:{}", game.key)), NOT_A_GAME) {
                // Under the name it shows now, so the Not games list reads the same.
                let name = game
                    .override_name
                    .clone()
                    .unwrap_or(game.stored_name.clone());
                out.push(UiAction::Change(SettingsChange::NotAGame {
                    key: game.key.clone(),
                    name: Some(name),
                }));
            }
        },
    );
}

/// A marked app under "Not games": its name, `detail` (the exe) and "It's a game".
fn is_a_game_row(ui: &mut Ui, key: &str, name: &str, detail: &str, out: &mut Vec<UiAction>) {
    let bw = button_width(ui, IS_A_GAME);
    setting_row(ui, GAME_ROW_H, Some(name), &[detail], bw, |ui| {
        if ui.button(id(&format!("settings.is_game:{key}")), IS_A_GAME) {
            out.push(UiAction::Change(SettingsChange::NotAGame {
                key: key.to_string(),
                name: None,
            }));
        }
    });
}

/// The last path component, for showing `C:\Games\x.exe` as `x.exe`.
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

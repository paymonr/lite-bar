//! Game names section: a name field per played game, and the stats folder button.

use super::{
    button_width, end_card, row_line, text_block, title_block, GameEdit, GameRow, SettingsView,
    UiAction, GAMES_HINT, ROW_H,
};
use crate::ui::{id, TextEdit, TextFieldEvent, Ui};

const FIELD_W: f32 = 220.0;
const GAME_ROW_H: f32 = 40.0;
const OPEN_FOLDER: &str = "Open stats folder";

pub(super) fn section(
    ui: &mut Ui,
    view: &mut SettingsView,
    games: &[GameRow],
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
                out.extend(g.commit(&exe));
            }
        }
    }

    ui.card(|ui| {
        if games.is_empty() {
            text_block(ui, ROW_H, &[GAMES_HINT]);
            return;
        }
        for (i, game) in games.iter().enumerate() {
            if i > 0 {
                row_line(ui);
            }
            let edit = view
                .games
                .entry(game.exe_path.clone())
                .or_insert_with(|| GameEdit::new(game.override_name.as_deref()));
            game_row(ui, game, edit, out);
        }
        row_line(ui);
        let bw = button_width(ui, OPEN_FOLDER);
        ui.row(ROW_H, |ui| {
            ui.right_aligned(bw, |ui| {
                if ui.button(id("settings.open_stats"), OPEN_FOLDER) {
                    out.push(UiAction::OpenStatsFolder);
                }
            });
            ui.column(|ui| text_block(ui, ROW_H, &["games.csv and sessions.csv open in Excel"]));
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
    }
    let exe = file_name(&game.exe_path);
    ui.row(GAME_ROW_H, |ui| {
        ui.right_aligned(FIELD_W, |ui| {
            match ui.text_field(field, &mut e.edit, &game.stored_name) {
                TextFieldEvent::None => {}
                TextFieldEvent::Changed => e.pending = true,
                TextFieldEvent::Committed => out.extend(e.commit(&game.exe_path)),
            }
        });
        ui.column(|ui| title_block(ui, GAME_ROW_H, &game.stored_name, Some(exe)));
    });
}

/// The last path component, for showing `C:\Games\x.exe` as `x.exe`.
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

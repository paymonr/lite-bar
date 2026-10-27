//! Game names section: a name field per played game, and the stats folder button.

use super::{
    button_width, end_card, row_line, setting_row, text_block, GameEdit, GameRow, SettingsView,
    UiAction, GAMES_HINT, ROW_H,
};
use crate::ui::{id, theme, TextEdit, TextFieldEvent, Ui};

const FIELD_W: f32 = 220.0;
const GAME_ROW_H: f32 = theme::CONTROL_HEIGHT;
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
                out.extend(g.commit());
            }
        }
    }

    ui.card(|ui| {
        if games.is_empty() {
            let w = ui.available().w;
            text_block(ui, ROW_H, None, &[GAMES_HINT], w);
            return;
        }
        for (i, game) in games.iter().enumerate() {
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
    let field_w = FIELD_W.min(ui.available().w);
    setting_row(
        ui,
        GAME_ROW_H,
        Some(&game.stored_name),
        &[exe],
        field_w,
        |ui| match ui.text_field(field, &mut e.edit, &game.stored_name) {
            TextFieldEvent::None => {}
            TextFieldEvent::Changed => e.pending = true,
            TextFieldEvent::Committed => out.extend(e.commit()),
        },
    );
}

/// The last path component, for showing `C:\Games\x.exe` as `x.exe`.
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

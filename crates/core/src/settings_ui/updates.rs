//! Updates section: the "Check for updates" switch, Check now, and the status line with
//! Update now while a newer release is available.

use super::{
    button_width, end_card, row_line, setting_row, setting_row_colored, UiAction, UpdatesInput,
    PROBLEM_TEXT, ROW_H,
};
use crate::settings::Settings;
use crate::settings_edit::SettingsChange;
use crate::ui::{id, theme, Rgba, TextStyle, Ui};
use crate::update::UpdateState;

const CHECK_NOW: &str = "Check now";
const UPDATE_NOW: &str = "Update now";
const RELEASE_PAGE: &str = "Open release page";

pub(super) fn section(
    ui: &mut Ui,
    settings: &Settings,
    updates: UpdatesInput,
    out: &mut Vec<UiAction>,
) {
    ui.card(|ui| {
        setting_row(
            ui,
            ROW_H,
            Some("Check for updates"),
            &["Asks GitHub once a day for a newer release"],
            theme::TOGGLE_W,
            |ui| {
                let mut v = settings.updates.check;
                if ui.toggle(id("settings.updates.check"), &mut v) {
                    out.push(UiAction::Change(SettingsChange::CheckUpdates(v)));
                }
            },
        );
        row_line(ui);
        let (text, color) = status(&updates);
        let available = matches!(updates.state, UpdateState::Available(_));
        let mut w = button_width(ui, CHECK_NOW);
        if available {
            let m = ui.text_measure();
            w += m.measure(RELEASE_PAGE, TextStyle::BODY).0
                + m.measure(UPDATE_NOW, TextStyle::BOLD).0
                + 2.0 * theme::BUTTON_PAD_X
                + 2.0 * theme::GAP;
        }
        setting_row_colored(ui, theme::CONTROL_HEIGHT, None, &[&text], color, w, |ui| {
            if available {
                if ui.link(id("settings.release_page"), RELEASE_PAGE) {
                    out.push(UiAction::OpenReleasePage);
                }
                if ui.button_accent(id("settings.update_now"), UPDATE_NOW) {
                    out.push(UiAction::InstallUpdate);
                }
            }
            if ui.button(id("settings.check_now"), CHECK_NOW) {
                out.push(UiAction::CheckForUpdates);
            }
        });
    });
    end_card(ui);
}

/// The status line and its color. Before any check it names the running version.
fn status(updates: &UpdatesInput) -> (String, Rgba) {
    let text = match updates.state {
        UpdateState::Idle => format!("Lite Bar v{}", updates.current),
        s => s.status_text(&updates.current),
    };
    let color = match updates.state {
        UpdateState::Available(_) => theme::ACCENT,
        UpdateState::CheckFailed | UpdateState::Failed(_) => PROBLEM_TEXT,
        _ => theme::MUTED,
    };
    (text, color)
}

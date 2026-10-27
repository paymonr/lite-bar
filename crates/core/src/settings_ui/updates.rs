//! Updates section: the "Check for updates" switch, Check now, and the status line with
//! Update now while a newer release is available. A failed install keeps the release page
//! link (so the release can be downloaded by hand) and offers Try again when retrying may
//! work; an installed update waiting for a restart offers Restart now.

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
const TRY_AGAIN: &str = "Try again";
const RESTART_NOW: &str = "Restart now";

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
        let page = matches!(
            updates.state,
            UpdateState::Available(_) | UpdateState::Failed { .. }
        );
        let primary = primary_button(updates.state);
        let mut w = button_width(ui, CHECK_NOW);
        let m = ui.text_measure();
        if page {
            w += m.measure(RELEASE_PAGE, TextStyle::BODY).0 + theme::GAP;
        }
        if let Some((_, label, _)) = primary {
            w += m.measure(label, TextStyle::BOLD).0 + 2.0 * theme::BUTTON_PAD_X + theme::GAP;
        }
        setting_row_colored(ui, theme::CONTROL_HEIGHT, None, &[&text], color, w, |ui| {
            if page && ui.link(id("settings.release_page"), RELEASE_PAGE) {
                out.push(UiAction::OpenReleasePage);
            }
            if let Some((key, label, action)) = primary {
                if ui.button_accent(id(key), label) {
                    out.push(action);
                }
            }
            if ui.button(id("settings.check_now"), CHECK_NOW) {
                out.push(UiAction::CheckForUpdates);
            }
        });
    });
    end_card(ui);
}

/// The accent button the state offers (id key, label, action): Update now for an
/// available release, Try again after a failure that may pass, Restart now once installed.
fn primary_button(state: &UpdateState) -> Option<(&'static str, &'static str, UiAction)> {
    match state {
        UpdateState::Available(_) => {
            Some(("settings.update_now", UPDATE_NOW, UiAction::InstallUpdate))
        }
        UpdateState::Failed { retry: true, .. } => {
            Some(("settings.try_again", TRY_AGAIN, UiAction::InstallUpdate))
        }
        UpdateState::RestartRequired => Some((
            "settings.restart_now",
            RESTART_NOW,
            UiAction::RestartToUpdate,
        )),
        _ => None,
    }
}

/// The status line and its color. Before any check it names the running version.
fn status(updates: &UpdatesInput) -> (String, Rgba) {
    let text = match updates.state {
        UpdateState::Idle => format!("Lite Bar v{}", updates.current),
        s => s.status_text(&updates.current),
    };
    let color = match updates.state {
        UpdateState::Available(_) | UpdateState::RestartRequired => theme::ACCENT,
        UpdateState::Failed { .. } => PROBLEM_TEXT,
        _ => theme::MUTED,
    };
    (text, color)
}

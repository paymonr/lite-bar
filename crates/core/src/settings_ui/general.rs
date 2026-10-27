//! General section: the "Start with Windows" switch, showing whether the logon task really
//! exists. The host queries and changes the task; while it does, clicks are ignored.

use super::{
    ellipsize, end_card, setting_row, SettingsView, StartupInput, UiAction, HINT, PROBLEM_TEXT,
    ROW_H,
};
use crate::startup_task::StartupState;
use crate::ui::{id, theme, Ui};

const STARTUP_HINT: &str = "Starts as administrator at sign-in, with no prompt.";
const STARTUP_ASKS_ADMIN: &str = "Turning this on asks for admin once.";
const OTHER_COPY: &str = "On (points to another copy)";
const CHECKING: &str = "Checking…";
const WORKING: &str = "Working…";

pub(super) fn section(
    ui: &mut Ui,
    view: &mut SettingsView,
    startup: StartupInput,
    out: &mut Vec<UiAction>,
) {
    let state = startup.state;
    // The switch shows the task's real state; while busy the value being set, after a
    // failure the last real state.
    match state {
        StartupState::On => view.startup_shown = true,
        StartupState::Off | StartupState::OnOtherCopy => view.startup_shown = false,
        _ => {}
    }
    if *state != StartupState::Busy {
        view.startup_target = None;
    }
    let shown = match state {
        StartupState::Busy => view.startup_target.unwrap_or(view.startup_shown),
        StartupState::Unknown => false,
        _ => view.startup_shown,
    };
    let mut lines = vec![STARTUP_HINT];
    match state {
        StartupState::Unknown => lines.push(CHECKING),
        StartupState::Busy => lines.push(WORKING),
        StartupState::OnOtherCopy => lines.push(OTHER_COPY),
        StartupState::Off if !startup.elevated => lines.push(STARTUP_ASKS_ADMIN),
        _ => {}
    }
    let ready = !matches!(state, StartupState::Unknown | StartupState::Busy);
    ui.card(|ui| {
        let stacked = setting_row(
            ui,
            ROW_H,
            Some("Start with Windows"),
            &lines,
            theme::TOGGLE_W,
            |ui| {
                let mut v = shown;
                if ui.toggle(id("settings.startup"), &mut v) && ready {
                    view.startup_target = Some(v);
                    out.push(UiAction::SetStartup(v));
                }
            },
        );
        if let StartupState::Error(msg) = state {
            let text = ellipsize(ui, msg, HINT, ui.available().w);
            let th = ui.text_measure().measure(&text, HINT).1;
            if !stacked {
                ui.space(4.0);
            }
            ui.row(th, |ui| ui.label(&text, HINT, PROBLEM_TEXT));
        }
    });
    end_card(ui);
}

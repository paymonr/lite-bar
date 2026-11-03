//! General section: the "Start with Windows" switch, showing whether the logon task really
//! exists. The host queries and changes the task; until a state is verified, and while a
//! change runs, clicks are ignored. Below it, while Lite Bar runs from outside Program Files,
//! "Install to Program Files" does what the welcome popup's button does.

use super::{
    ellipsize, end_card, row_line, setting_row, InstallInput, SettingsView, StartupInput, UiAction,
    HINT, PROBLEM_TEXT, ROW_H,
};
use crate::install::{MoveOutcome, MoveStatus};
use crate::move_prompt_ui::{status_line, MOVING};
use crate::startup_task::StartupState;
use crate::ui::{id, theme, Rgba, TextStyle, Ui};

const STARTUP_HINT: &str = "Starts as administrator at sign-in, with no prompt.";
const STARTUP_ASKS_ADMIN: &str = "Turning this on asks for admin once.";
const OTHER_COPY: &str = "On (points to another copy)";
const CHECKING: &str = "Checking…";
const WORKING: &str = "Working…";
const INSTALL_TITLE: &str = "Install to Program Files";
const INSTALL_HINT: &str = "Adds Lite Bar to Start and Installed apps.";
const INSTALL_ASKS_ADMIN: &str = "Asks for admin once.";
const INSTALL: &str = "Install";

pub(super) fn section(
    ui: &mut Ui,
    view: &mut SettingsView,
    startup: StartupInput,
    install: InstallInput,
    out: &mut Vec<UiAction>,
) {
    let state = startup.state;
    // The switch shows only a verified value: the task's state, an error's checked state,
    // or while busy the value the click asked for. Anything else is neither on nor off and
    // takes no clicks.
    if *state != StartupState::Busy {
        view.startup_target = None;
    }
    let shown = match state {
        StartupState::Busy => view.startup_target,
        s => s.actual(),
    };
    let ready = state.actual().is_some();
    let mut lines = vec![STARTUP_HINT];
    match state {
        StartupState::Unknown => lines.push(CHECKING),
        StartupState::Busy => lines.push(WORKING),
        StartupState::OnOtherCopy => lines.push(OTHER_COPY),
        StartupState::Off if !startup.elevated => lines.push(STARTUP_ASKS_ADMIN),
        _ => {}
    }
    ui.card(|ui| {
        let stacked = setting_row(
            ui,
            ROW_H,
            Some("Start with Windows"),
            &lines,
            theme::TOGGLE_W,
            |ui| match shown {
                Some(mut v) => {
                    if ui.toggle(id("settings.startup"), &mut v) && ready {
                        view.startup_target = Some(v);
                        out.push(UiAction::SetStartup(v));
                    }
                }
                None => ui.toggle_mixed(),
            },
        );
        if let StartupState::Error { msg, .. } = state {
            let text = ellipsize(ui, msg, HINT, ui.available().w);
            let th = ui.text_measure().measure(&text, HINT).1;
            if !stacked {
                ui.space(4.0);
            }
            ui.row(th, |ui| ui.label(&text, HINT, PROBLEM_TEXT));
        }
        if install.offer {
            row_line(ui);
            install_row(ui, install.status, startup.elevated, out);
        }
    });
    end_card(ui);
}

/// "Install to Program Files" with its Install button, "Installing…" while it runs, and the
/// outcome underneath. Once the copy is in Program Files (but couldn't be started) there is
/// nothing left to install, so the button goes.
fn install_row(ui: &mut Ui, status: &MoveStatus, elevated: bool, out: &mut Vec<UiAction>) {
    let mut lines = vec![INSTALL_HINT];
    if !elevated {
        lines.push(INSTALL_ASKS_ADMIN);
    }
    let moving = *status == MoveStatus::Moving;
    let installed = matches!(status, MoveStatus::Done(MoveOutcome::MovedNotStarted(_)));
    let m = ui.text_measure();
    let control_w = if moving {
        m.measure(MOVING, TextStyle::BODY).0
    } else if installed {
        0.0
    } else {
        m.measure(INSTALL, TextStyle::BOLD).0 + 2.0 * theme::BUTTON_PAD_X
    };
    let stacked = setting_row(ui, ROW_H, Some(INSTALL_TITLE), &lines, control_w, |ui| {
        if moving {
            ui.label(MOVING, TextStyle::BODY, theme::MUTED);
        } else if !installed && ui.button_accent(id("settings.install"), INSTALL) {
            out.push(UiAction::InstallToProgramFiles);
        }
    });
    let (text, color): (String, Rgba) = if moving {
        (String::new(), theme::MUTED)
    } else {
        status_line(status)
    };
    if !text.is_empty() {
        let text = ellipsize(ui, &text, HINT, ui.available().w);
        let th = ui.text_measure().measure(&text, HINT).1;
        if !stacked {
            ui.space(4.0);
        }
        ui.row(th, |ui| ui.label(&text, HINT, color));
    }
}

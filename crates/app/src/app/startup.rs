//! The startup-task side of `App`: the Settings window's "Start with Windows" switch. The
//! task is queried when the window opens and changed when the switch is clicked, each on a
//! worker thread (`startup::spawn`) whose result comes back as `WM_STARTUP_RESULT`.

use lite_bar_core::startup_task::StartupState;

use super::App;
use crate::log;
use crate::startup::{spawn, StartupResult};

impl App {
    /// Asks for the task's real state (the window opened). The shown state stays until the
    /// answer arrives; a change in progress reports its own.
    pub(super) fn refresh_startup(&mut self) {
        if self.startup == StartupState::Busy {
            return;
        }
        let query = self.startup_query;
        if !spawn(self.hwnd, move || StartupResult::State(query())) {
            log::error("could not start the startup-task thread");
        }
    }

    /// The switch was clicked: changes the task on a worker. Ignored until the state is
    /// known and while a change runs.
    pub(super) fn set_startup(&mut self, enabled: bool) {
        if matches!(self.startup, StartupState::Unknown | StartupState::Busy) {
            return;
        }
        let (change, elevated) = (self.startup_change, self.elevated);
        // The UAC prompt belongs to the settings window.
        let parent = self
            .settings_ui
            .as_ref()
            .map_or(self.hwnd, |ui| ui.hwnd())
            .0 as usize;
        self.startup_before = self.startup.clone();
        if spawn(self.hwnd, move || change(enabled, elevated, parent)) {
            self.set_startup_state(StartupState::Busy);
        } else {
            log::error("could not start the startup-task thread");
            self.set_startup_state(StartupState::Error(
                "Couldn't change the startup task".into(),
            ));
        }
    }

    pub(super) fn on_startup_result(&mut self, result: StartupResult) {
        let state = match result {
            StartupResult::State(s) => s,
            // Nothing changed: back to how it was, with no error.
            StartupResult::Declined => self.startup_before.clone(),
        };
        self.set_startup_state(state);
    }

    pub(super) fn set_startup_state(&mut self, state: StartupState) {
        self.startup = state;
        if let Some(ui) = &self.settings_ui {
            ui.set_startup(&self.startup, self.elevated);
        }
    }
}

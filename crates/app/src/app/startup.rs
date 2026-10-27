//! The startup-task side of `App`: the Settings window's "Start with Windows" switch. The
//! task is queried when the window opens and changed when the switch is clicked, each on a
//! worker thread (`startup::spawn`) whose result comes back as `WM_STARTUP_RESULT`.

use lite_bar_core::startup_task::StartupState;

use super::App;
use crate::log;
use crate::startup::{check_failed, failed_change, spawn, StartupResult, PANICKED};

impl App {
    /// Asks for the task's real state (the window opened). The shown state stays until the
    /// answer arrives; a change in progress reports its own.
    pub(super) fn refresh_startup(&mut self) {
        if self.startup == StartupState::Busy {
            return;
        }
        let query = self.startup_query;
        let job = move || StartupResult::Query(query());
        if !spawn(self.hwnd, job, StartupResult::Query(check_failed(PANICKED))) {
            log::error("could not start the startup-task thread");
        }
    }

    /// The switch was clicked: changes the task on a worker. Ignored until a state is
    /// verified and while a change runs.
    pub(super) fn set_startup(&mut self, enabled: bool) {
        // Not before a state is verified, and not while a change runs (Busy has none).
        if self.startup.actual().is_none() {
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
        // Nothing is known after a panic mid-change.
        let if_panics = StartupResult::Changed(failed_change(PANICKED, &StartupState::Unknown));
        if spawn(
            self.hwnd,
            move || change(enabled, elevated, parent),
            if_panics,
        ) {
            self.set_startup_state(StartupState::Busy);
        } else {
            log::error("could not start the startup-task thread");
            let failed = failed_change("couldn't start a thread", &self.startup_before);
            self.set_startup_state(failed);
        }
    }

    pub(super) fn on_startup_result(&mut self, result: StartupResult) {
        let state = match result {
            // Started before the change now running: its answer may be stale.
            StartupResult::Query(_) if self.startup == StartupState::Busy => return,
            StartupResult::Query(s) | StartupResult::Changed(s) => s,
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

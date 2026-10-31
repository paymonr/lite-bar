//! The welcome popup side of `App`: offering the move to Program Files at start, the
//! popup's choices, the move itself on a worker thread (`mover::spawn`, whose outcome comes
//! back as `WM_MOVE_RESULT`), and the moved copy's first start (`--moved-from`).

use lite_bar_core::install::{should_offer, target_dir, MoveOutcome, MoveStatus};
use lite_bar_core::move_prompt_ui::MovePromptAction;
use lite_bar_core::settings_edit::SettingsChange;

use super::{quit, set_owns_trace, App};
use crate::log;
use crate::move_prompt_window::MovePromptState;

/// Shown when the move's thread could not start.
const NO_THREAD: &str = "couldn't start the move";

impl App {
    /// Opens the welcome popup when it hasn't been answered and Lite Bar runs from outside
    /// Program Files.
    pub(super) fn maybe_offer_move(&mut self) {
        let exe = self
            .exe_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        if !should_offer(
            self.settings.install.move_prompt,
            exe.as_deref(),
            &self.pf_roots,
        ) {
            return;
        }
        // The first root is `mover::program_files()`.
        let Some(pf) = self.pf_roots.first() else {
            return;
        };
        let shown = MovePromptState {
            hotkey: self.settings.hotkeys.toggle_overlay.to_string(),
            target_dir: target_dir(pf),
            status: self.move_status.clone(),
        };
        match (self.open_move_prompt)(self.hwnd, shown) {
            Ok(ui) => self.move_prompt = Some(ui),
            Err(e) => log::error(&format!("could not open the welcome popup: {e}")),
        }
    }

    /// Applies what the popup queued, in order.
    pub(super) fn drain_move_prompt_actions(&mut self) {
        while let Some(ui) = &self.move_prompt {
            let actions = ui.take();
            if actions.is_empty() {
                return;
            }
            for a in actions {
                self.handle_move_prompt_action(a);
            }
        }
    }

    pub(super) fn handle_move_prompt_action(&mut self, a: MovePromptAction) {
        match a {
            MovePromptAction::Later => self.close_move_prompt(),
            MovePromptAction::NoThanks => {
                self.apply_settings_change(SettingsChange::MovePrompt(false));
                self.close_move_prompt();
            }
            MovePromptAction::Move => self.start_move(),
        }
    }

    /// Move was clicked: moves on a worker. Ignored while a move runs.
    fn start_move(&mut self) {
        if !matches!(self.move_status, MoveStatus::Idle | MoveStatus::Done(_)) {
            return;
        }
        let (job, elevated) = (self.move_job, self.elevated);
        // The UAC prompt belongs to the popup.
        let parent = self
            .move_prompt
            .as_ref()
            .map_or(self.hwnd, |ui| ui.hwnd())
            .0 as usize;
        if (self.spawn_move)(self.hwnd, Box::new(move || job(elevated, parent))) {
            self.set_move_status(MoveStatus::Moving);
        } else {
            log::error("could not start the move thread");
            self.set_move_status(MoveStatus::Done(MoveOutcome::Failed(NO_THREAD.into())));
        }
    }

    /// The move finished. Applied even when the popup was closed meanwhile: then a move
    /// that didn't happen is only logged.
    pub(super) fn apply_move_outcome(&mut self, o: MoveOutcome) {
        log::info(&format!("move to Program Files: {o:?}"));
        match o {
            MoveOutcome::Moved => self.exit_after_move(),
            o => {
                // The copy in Program Files exists: don't offer the move again.
                if matches!(o, MoveOutcome::MovedNotStarted(_)) {
                    self.apply_settings_change(SettingsChange::MovePrompt(false));
                }
                self.set_move_status(MoveStatus::Done(o));
            }
        }
    }

    /// The moved copy runs: this one saves everything, lets go of capture, hotkey and tray
    /// icon, and quits. The mutex is kept until the process exits (the new copy waits for
    /// it).
    pub(super) fn exit_after_move(&mut self) {
        self.close_for_exit();
        self.flush_settings_save();
        set_owns_trace(false);
        quit();
    }

    /// The first start of the moved copy: the popup was answered, and the old copy goes,
    /// unless "Start with Windows" still starts it (the helper's re-point failed and
    /// re-creating the task for this copy fails again).
    pub(super) fn finish_moved_from(&mut self, old: String) {
        self.apply_settings_change(SettingsChange::MovePrompt(false));
        let Some(exe) = &self.exe_path else {
            log::warn(&format!(
                "can't find Lite Bar's own file: the old copy at {old} was left in place"
            ));
            return;
        };
        let exe = exe.to_string_lossy().into_owned();
        match (self.repoint_old_startup)(&old, &exe) {
            Ok(true) => log::info(&format!("startup task now starts {exe}")),
            Ok(false) => {}
            Err(e) => {
                log::warn(&format!(
                    "kept the old copy at {old}: Start with Windows still points at it ({e})"
                ));
                return;
            }
        }
        (self.delete_old)(old, exe, self.pf_roots.clone());
    }

    /// Whether a move runs, so `what` (a relaunch) must wait; logged when it does.
    pub(super) fn held_by_move(&self, what: &str) -> bool {
        let moving = self.move_status == MoveStatus::Moving;
        if moving {
            log::info(&format!(
                "{what} ignored: Lite Bar is moving to Program Files"
            ));
        }
        moving
    }

    /// Closes the popup, if open; a move in progress still reports its outcome.
    pub(super) fn close_move_prompt(&mut self) {
        // Dropping it closes the window; the "Later" its close queues goes with it.
        self.move_prompt = None;
    }

    fn set_move_status(&mut self, status: MoveStatus) {
        self.move_status = status;
        if let Some(ui) = &self.move_prompt {
            ui.set_status(&self.move_status);
        }
    }
}

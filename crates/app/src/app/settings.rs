//! The settings-window side of `App`: opening and draining the window, live apply,
//! hotkey switching and the debounced, format-preserving save. Split out of `main.rs`.

use std::time::Instant;

use lite_bar_core::hotkey::Hotkey;
use lite_bar_core::settings::Settings;
use lite_bar_core::settings_edit::{apply_change, edit_settings_text, push_change, SettingsChange};
use lite_bar_core::settings_ui::{GameRow, UiAction};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use super::{shell_open, App, SAVE_TIMER_ID, SETTINGS_FILE};
use crate::hotkeys::{self, HotkeyId, Registration};
use crate::log;
use crate::settings_window::{switch_hotkey, Drained, HotkeyDecision, SettingsUi, SAVE_DELAY};
use crate::stats_files::{list_games, write_atomic};

/// Shown in the window when the file cannot be edited (not valid TOML, a bad section).
pub(super) const SAVE_FAILED_FILE: &str =
    "Can't save: the settings file has an error.\nFix it, then choose Reload settings.";
/// Shown in the window when the file cannot be read or written.
pub(super) const SAVE_FAILED_WRITE: &str =
    "Can't save: the settings file could not be written.\nYour changes are kept and saved with the next change.";

/// Status when the toggle hotkey could not be registered again after capture.
fn restore_failed(hk: Hotkey) -> String {
    format!("Couldn't restore {hk} — another app took it")
}

impl App {
    /// The settings the window shows: the app's, with "Show overlay" following whether
    /// the overlay is on right now (the hotkey toggles it without changing the file).
    pub(super) fn shown_settings(&self) -> Settings {
        let mut s = self.settings.clone();
        s.overlay.visible = self.overlay_on;
        s
    }

    pub(super) fn sync_settings_window(&self) {
        if let Some(ui) = &self.settings_ui {
            ui.show(&self.shown_settings());
        }
    }

    /// Tray "Settings…" and a double-click on the icon. Focuses the window if open.
    pub(super) fn open_settings(&mut self) {
        self.open_settings_with(SettingsUi::open);
    }

    #[cfg(test)]
    pub(super) fn open_settings_hidden(&mut self) {
        self.open_settings_with(SettingsUi::open_hidden);
    }

    pub(super) fn open_settings_with(
        &mut self,
        open: fn(HWND, Settings, Vec<GameRow>) -> windows::core::Result<SettingsUi>,
    ) {
        if let Some(ui) = &self.settings_ui {
            if ui.is_open() {
                ui.focus();
                return;
            }
        }
        self.settings_ui = None;
        let games = list_games(self.writer.stats_dir());
        match open(self.hwnd, self.shown_settings(), games) {
            Ok(ui) => {
                ui.set_save_error(self.save_error);
                self.settings_ui = Some(ui);
            }
            Err(e) => log::error(&format!("could not open the settings window: {e}")),
        }
    }

    /// Applies what the settings window queued, in order; drops the window once it
    /// closed and writes its pending changes.
    /// Loops: applying an action can pump messages (`ShellExecuteW`), which may run
    /// frames that queue more.
    pub(super) fn drain_settings_actions(&mut self) {
        while let Some(ui) = &self.settings_ui {
            let drained = ui.take();
            if drained == Drained::default() {
                return;
            }
            for action in drained.actions {
                self.apply_ui_action(action);
            }
            if drained.closed {
                self.settings_ui = None;
                self.end_hotkey_capture();
                self.flush_settings_save();
            }
        }
    }

    /// Before an elevated relaunch: ends edits still in progress and writes everything, with
    /// the window left open (UAC may be declined).
    pub(super) fn save_settings_before_restart(&mut self) {
        if let Some(ui) = &self.settings_ui {
            ui.finish_edits();
        }
        self.drain_settings_actions();
        self.flush_settings_save();
    }

    /// Closes the window (Quit, restart, shutdown), applies its last edits and saves.
    pub(super) fn close_settings_window(&mut self) {
        if let Some(ui) = self.settings_ui.take() {
            for action in ui.close().actions {
                self.apply_ui_action(action);
            }
        }
        self.end_hotkey_capture();
        self.flush_settings_save();
    }

    /// Closes the window when this process is about to exit (after a successful elevated
    /// relaunch): a released hotkey stays released so the new instance can register it.
    pub(super) fn close_settings_window_for_exit(&mut self) {
        self.hotkey_released = false;
        self.close_settings_window();
    }

    pub(super) fn apply_ui_action(&mut self, action: UiAction) {
        match action {
            UiAction::Change(change) => self.apply_settings_change(change),
            UiAction::ProposeHotkey(hk) => {
                let restored = self.end_hotkey_capture();
                if !restored && hk == self.settings.hotkeys.toggle_overlay {
                    // "Keep" would hide that the hotkey no longer works.
                    self.set_hotkey_status(Some(&restore_failed(hk)));
                } else {
                    self.propose_hotkey(hk);
                }
            }
            UiAction::OpenStatsFolder => self.open_stats_folder(),
            UiAction::StartHotkeyCapture => self.start_hotkey_capture(),
            UiAction::CancelHotkeyCapture => {
                if self.end_hotkey_capture() {
                    self.set_hotkey_status(None);
                } else {
                    let hk = self.settings.hotkeys.toggle_overlay;
                    self.set_hotkey_status(Some(&restore_failed(hk)));
                }
            }
        }
    }

    /// Releases the toggle hotkey while the capture field listens, so pressing the current
    /// combination reaches the field (and becomes "keep") instead of toggling the overlay.
    fn start_hotkey_capture(&mut self) {
        if !self.hotkey_released {
            hotkeys::unregister(self.hwnd, HotkeyId::ToggleOverlay);
            self.hotkey_released = true;
        }
    }

    /// Takes the toggle hotkey back after capture (a proposal, a cancel, the window closing).
    /// Returns false when another app took it in the meantime.
    fn end_hotkey_capture(&mut self) -> bool {
        if !self.hotkey_released {
            return true;
        }
        // Unregisters first, so a hotkey registered again meanwhile is not "taken".
        self.register_hotkey();
        self.hotkey_ok
    }

    /// One change from the window: live in memory and on the overlay, saved shortly.
    pub(super) fn apply_settings_change(&mut self, change: SettingsChange) {
        let old_hotkey = self.settings.hotkeys.toggle_overlay;
        apply_change(&mut self.settings, &change);
        self.overlay.set_settings(&self.settings.overlay);
        self.overlay.redraw();
        self.queue_save(change.clone());
        match &change {
            SettingsChange::Visible(_) | SettingsChange::RestoreDefaults
                if self.settings.overlay.visible != self.overlay_on =>
            {
                if self.overlay_on {
                    self.stop_overlay();
                } else {
                    self.start_overlay();
                }
            }
            SettingsChange::GameName { .. } => {
                self.writer.mark_games_csv_dirty();
                self.writer.flush(&self.settings.game_names);
            }
            _ => {}
        }
        let new_hotkey = self.settings.hotkeys.toggle_overlay;
        if new_hotkey != old_hotkey {
            // Restore defaults: the default hotkey must be registered too.
            if let HotkeyDecision::Reject(msg) = self.change_hotkey(new_hotkey, old_hotkey) {
                let keep = SettingsChange::ToggleHotkey(old_hotkey);
                apply_change(&mut self.settings, &keep);
                self.queue_save(keep);
                // Not a capture the user made: a status line, not a field error.
                self.set_hotkey_status(Some(&msg));
            }
        }
        self.sync_settings_window();
    }

    /// Unregisters `current`, registers `proposed`, or `current` again if that fails.
    pub(super) fn change_hotkey(&mut self, proposed: Hotkey, current: Hotkey) -> HotkeyDecision {
        let hwnd = self.hwnd;
        let mut registered = self.hotkey_ok;
        let decision = switch_hotkey(
            proposed,
            current,
            || hotkeys::unregister(hwnd, HotkeyId::ToggleOverlay),
            |hk| {
                registered = matches!(
                    hotkeys::register(hwnd, HotkeyId::ToggleOverlay, hk),
                    Registration::Ok
                );
                registered
            },
        );
        self.hotkey_ok = registered;
        decision
    }

    pub(super) fn propose_hotkey(&mut self, proposed: Hotkey) {
        let current = self.settings.hotkeys.toggle_overlay;
        match self.change_hotkey(proposed, current) {
            HotkeyDecision::Keep => self.set_hotkey_status(None),
            HotkeyDecision::Switch => {
                log::info(&format!("toggle hotkey is now {proposed}"));
                let change = SettingsChange::ToggleHotkey(proposed);
                apply_change(&mut self.settings, &change);
                self.queue_save(change);
                self.set_hotkey_status(None);
                self.sync_settings_window();
            }
            HotkeyDecision::Reject(msg) => {
                log::info(&format!("hotkey {proposed} is taken; keeping {current}"));
                self.reject_hotkey(&msg);
            }
        }
    }

    pub(super) fn set_hotkey_status(&self, status: Option<&str>) {
        if let Some(ui) = &self.settings_ui {
            ui.set_status(status);
        }
    }

    pub(super) fn reject_hotkey(&self, msg: &str) {
        if let Some(ui) = &self.settings_ui {
            ui.reject_hotkey(msg);
        }
    }

    pub(super) fn open_stats_folder(&self) {
        let dir = self.writer.stats_dir();
        match std::fs::create_dir_all(dir) {
            Ok(()) => shell_open(dir, "stats folder"),
            Err(e) => log::error(&format!("could not create {}: {e}", dir.display())),
        }
    }

    /// Marks `change` for the next save, `SAVE_DELAY` after the last change.
    pub(super) fn queue_save(&mut self, change: SettingsChange) {
        push_change(&mut self.unsaved, change);
        self.save.change(Instant::now());
        // SAFETY: plain FFI on our own window; re-setting the id restarts the timer.
        unsafe {
            SetTimer(
                Some(self.hwnd),
                SAVE_TIMER_ID,
                SAVE_DELAY.as_millis() as u32,
                None,
            );
        }
    }

    pub(super) fn on_save_timer(&mut self, now: Instant) {
        if self.save.due(now) {
            self.flush_settings_save();
        }
    }

    /// Writes the pending changes now, if any.
    pub(super) fn flush_settings_save(&mut self) {
        // SAFETY: plain FFI on our own window.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), SAVE_TIMER_ID);
        }
        // Changes left over from a failed save are retried too.
        if self.save.flush() || !self.unsaved.is_empty() {
            self.write_settings_file();
        } else {
            self.set_save_error(None);
        }
    }

    fn set_save_error(&mut self, msg: Option<&'static str>) {
        self.save_error = msg;
        if let Some(ui) = &self.settings_ui {
            ui.set_save_error(msg);
        }
    }

    /// Applies the pending changes to the file's current text, touching only their keys,
    /// and writes it atomically. A missing file starts from the commented template; a file
    /// that is no longer valid TOML is left alone (the settings stay in memory). Changes that
    /// could not be written stay pending, so the next save (a change, closing the window,
    /// Reload) tries again.
    pub(super) fn write_settings_file(&mut self) {
        let result = self.try_write_settings_file();
        if result.is_err() {
            log::warn(
                "settings changes kept; they are saved with the next change, on close or on Reload",
            );
        }
        self.set_save_error(result.err());
    }

    /// On failure `unsaved` is untouched and the error is the message for the window.
    fn try_write_settings_file(&mut self) -> Result<(), &'static str> {
        let path = self.dir.join(SETTINGS_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                log::warn(&format!(
                    "settings not saved: cannot read {}: {e}",
                    path.display()
                ));
                return Err(SAVE_FAILED_WRITE);
            }
        };
        let written = match edit_settings_text(&text, &self.unsaved) {
            Ok(new_text) if new_text == text => Ok(()),
            Ok(new_text) => write_atomic(&path, new_text.as_bytes())
                .map_err(|e| (format!("{}: {e}", path.display()), SAVE_FAILED_WRITE)),
            Err(e) => Err((e, SAVE_FAILED_FILE)),
        };
        match written {
            Ok(()) => {
                self.unsaved.clear();
                Ok(())
            }
            Err((e, shown)) => {
                log::warn(&format!("settings not saved: {e}"));
                Err(shown)
            }
        }
    }
}

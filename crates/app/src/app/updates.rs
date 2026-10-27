//! The update side of `App`: the check schedule and its owner-window timer, starting checks,
//! applying their results, the Settings and tray actions, and Update now's install. Split out of `main.rs`.

use std::path::Path;
use std::time::Duration;

use lite_bar_core::update::{current_version, newer_release, CheckStart, UpdateState};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use super::{acquire_guard, shell_open, App, UPDATE_TIMER_ID};
use crate::hotkeys::{self, HotkeyId};
use crate::installer::{spawn_download, swap, InstallError, InstallResult, RESTART_TO_FINISH};
use crate::log;
use crate::updater::{is_web_url, next_state, spawn_check, CheckResult};

/// Shown when Windows can't say where this exe is.
const NO_EXE: &str = "can't find Lite Bar's own file";
/// Shown when the download thread could not start.
const NO_THREAD: &str = "couldn't start the download";

/// `SetTimer` takes at most about 24.8 days; 24 h fits, this only guards the cast.
const MAX_TIMER_MS: u128 = 0x7FFF_FFFF;

impl App {
    /// Sets the owner-window timer for the next automatic check, or stops it when none is
    /// due (checks off, or one running).
    pub(super) fn arm_update_timer(&self) {
        let now = self.clock.elapsed();
        // SAFETY: plain FFI on our own window; re-setting the id restarts the timer.
        unsafe {
            match self.update_schedule.next_due() {
                Some(at) => {
                    let ms = at.saturating_sub(now).as_millis().clamp(1, MAX_TIMER_MS);
                    SetTimer(Some(self.hwnd), UPDATE_TIMER_ID, ms as u32, None);
                }
                None => {
                    let _ = KillTimer(Some(self.hwnd), UPDATE_TIMER_ID);
                }
            }
        }
    }

    /// Follows `[updates] check`: on schedules a check 30 s from now, off cancels it.
    pub(super) fn sync_update_schedule(&mut self) {
        let now = self.clock.elapsed();
        self.update_schedule
            .set_enabled(self.settings.updates.check, now);
        self.arm_update_timer();
    }

    /// The update timer fired (`now` is `clock.elapsed()`). A background check leaves the
    /// shown state alone until its result arrives.
    pub(super) fn on_update_timer(&mut self, now: Duration) {
        if self.update_schedule.poll(now) {
            self.start_update_check(now);
        }
        self.arm_update_timer();
    }

    /// "Check now": runs even with checks off, and shows "Checking…" (except over an
    /// available update, which keeps its Update now until a check says otherwise).
    pub(super) fn check_for_updates_now(&mut self) {
        if matches!(self.update, UpdateState::Downloading(_)) {
            return;
        }
        if self.update_schedule.check_now() == CheckStart::Started {
            self.start_update_check(self.clock.elapsed());
        }
        if self.update_schedule.is_running() && !matches!(self.update, UpdateState::Available(_)) {
            self.set_update_state(UpdateState::Checking);
        }
        self.arm_update_timer();
    }

    fn start_update_check(&mut self, now: Duration) {
        let (fetch, url) = (self.fetch, self.latest_url.clone());
        if !spawn_check(self.hwnd, move || fetch(&url)) {
            log::error("could not start the update check thread");
            self.update_schedule.finished(now);
        }
    }

    /// A check finished: the state follows its result. Failures are logged, never shown
    /// beyond the Settings status line.
    pub(super) fn on_update_result(&mut self, result: CheckResult) {
        self.update_schedule.finished(self.clock.elapsed());
        let current = current_version();
        match &result {
            Err(e) => log::warn(&format!("update check failed: {e}")),
            Ok(r) if newer_release(&current, r) => {
                log::info(&format!("update available: v{}", r.version))
            }
            Ok(_) => {}
        }
        let state = next_state(&self.update, &result, &current);
        self.set_update_state(state);
        self.arm_update_timer();
    }

    pub(super) fn set_update_state(&mut self, state: UpdateState) {
        self.update = state;
        if let Some(ui) = &self.settings_ui {
            ui.set_update(&self.update);
        }
    }

    /// Update now (Settings or the tray): downloads the available release on a worker
    /// thread and shows Downloading meanwhile. Only an available release newer than this
    /// build is installed, and only from its own URLs; a click while downloading is ignored.
    pub(super) fn install_update(&mut self) {
        let UpdateState::Available(r) = &self.update else {
            return;
        };
        let r = r.clone();
        if !newer_release(&current_version(), &r) {
            log::warn(&format!("not installing v{}: not newer", r.version));
            return;
        }
        let Some(exe) = self.exe_path.clone() else {
            self.set_update_state(UpdateState::Failed(NO_EXE.into()));
            return;
        };
        log::info(&format!("downloading v{}", r.version));
        self.installing = Some(r.clone());
        self.set_update_state(UpdateState::Downloading(r.clone()));
        let download = self.download;
        if !spawn_download(self.hwnd, move || download(&r, &exe)) {
            log::error("could not start the update download thread");
            self.set_update_state(UpdateState::Failed(NO_THREAD.into()));
        }
    }

    /// A download finished: a verified exe is swapped in and started, and this instance
    /// quits (returns true). Any failure leaves the running exe in place and shows why.
    pub(super) fn on_install_result(&mut self, result: InstallResult) -> bool {
        let UpdateState::Downloading(r) = &self.update else {
            log::warn("install result without a download in progress");
            return false;
        };
        let version = r.version;
        let (new, Some(exe)) = (result, self.exe_path.clone()) else {
            self.set_update_state(UpdateState::Failed(NO_EXE.into()));
            return false;
        };
        let new = match new {
            Ok(new) => new,
            Err(why) => {
                log::warn(&format!("update to v{version} failed: {why}"));
                self.set_update_state(UpdateState::Failed(why));
                return false;
            }
        };
        if let Err(e) = swap(&exe, &new) {
            match &e {
                InstallError::RollbackFailed { old, why } => log::error(&format!(
                    "UPDATE ROLLBACK FAILED: {} is missing, the previous exe is at {} ({why})",
                    exe.display(),
                    old.display()
                )),
                e => log::error(&format!("update to v{version} not installed: {e:?}")),
            }
            self.set_update_state(UpdateState::Failed(e.to_string()));
            return false;
        }
        log::info(&format!("installed v{version}; restarting"));
        self.relaunch_updated(&exe)
    }

    /// Runs Quit's close path (the window's edits, sessions and stats are saved; capture,
    /// hotkey and tray icon go; the mutex is released), then starts `exe --updated`.
    /// Returns true when it started. Otherwise this instance carries on as before and asks
    /// for a manual restart: the new exe is already in place.
    fn relaunch_updated(&mut self, exe: &Path) -> bool {
        let was_on = self.overlay_on;
        let had_guard = self.guard.is_some();
        self.close_settings_window_for_exit();
        hotkeys::unregister(self.hwnd, HotkeyId::ToggleOverlay);
        self.stop_overlay();
        self.tray.remove();
        self.finish_all_sessions();
        self.guard = None;
        match (self.relaunch)(exe) {
            Ok(()) => true,
            Err(e) => {
                log::error(&format!("could not start the updated Lite Bar: {e}"));
                if let Err(e) = self.tray.add() {
                    log::error(&format!("could not re-add tray icon: {e}"));
                }
                if had_guard {
                    self.guard = acquire_guard();
                }
                self.register_hotkey();
                if was_on {
                    self.start_overlay();
                }
                self.set_update_state(UpdateState::Failed(RESTART_TO_FINISH.into()));
                false
            }
        }
    }

    /// The page of the release shown in Settings, or of the one a failed install was for.
    pub(super) fn release_page(&self) -> Option<&str> {
        match &self.update {
            UpdateState::Available(r) | UpdateState::Downloading(r) => Some(&r.page_url),
            UpdateState::Failed(_) => self.installing.as_ref().map(|r| r.page_url.as_str()),
            _ => None,
        }
    }

    /// Opens that release page, if it is an https link.
    pub(super) fn open_release_page(&self) {
        let Some(url) = self.release_page() else {
            return;
        };
        if is_web_url(url) {
            shell_open(url, "release page");
        } else {
            log::warn(&format!("not opening release page {url:?}"));
        }
    }
}

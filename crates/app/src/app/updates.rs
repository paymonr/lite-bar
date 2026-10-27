//! The update side of `App`: the check schedule and its owner-window timer, starting checks,
//! applying their results, and the Settings and tray actions. Split out of `main.rs`.

use std::time::Duration;

use lite_bar_core::update::{current_version, newer_release, CheckStart, UpdateState};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use super::{shell_open, App, UPDATE_TIMER_ID};
use crate::log;
use crate::updater::{is_web_url, next_state, spawn_check, CheckResult};

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

    /// Update now (Settings or the tray). Installing comes in the next task.
    pub(super) fn install_update(&mut self) {
        log::info("install requested");
    }

    /// Opens the available release's page, if it is an https link.
    pub(super) fn open_release_page(&self) {
        let (UpdateState::Available(r) | UpdateState::Downloading(r)) = &self.update else {
            return;
        };
        if is_web_url(&r.page_url) {
            shell_open(&r.page_url, "release page");
        } else {
            log::warn(&format!("not opening release page {:?}", r.page_url));
        }
    }
}

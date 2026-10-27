//! Lite Bar: Windows tray app with an FPS overlay.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod elevation;
#[cfg(windows)]
mod etw;
#[cfg(windows)]
mod foreground;
#[cfg(windows)]
mod game_lookup;
#[cfg(windows)]
mod hotkeys;
#[cfg(windows)]
mod http;
#[cfg(windows)]
mod installer;
#[cfg(windows)]
mod log;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod paths;
#[cfg(windows)]
mod process_info;
#[cfg(windows)]
mod settings_window;
#[cfg(windows)]
mod single_instance;
#[cfg(windows)]
mod startup;
#[cfg(windows)]
mod stats_files;
#[cfg(all(windows, test))]
mod test_util;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod ui_host;
#[cfg(windows)]
mod ui_paint;
#[cfg(windows)]
mod updater;

#[cfg(windows)]
use app::run;

#[cfg(not(windows))]
fn run() {
    eprintln!("Lite Bar only runs on Windows.");
}

fn main() {
    run();
}

#[cfg(windows)]
mod app {
    use std::any::Any;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use lite_bar_core::fps::{FocusClock, FpsTracker};
    use lite_bar_core::game_name::{display_name, is_known_non_game};
    use lite_bar_core::history::LocalTime;
    use lite_bar_core::layout::{fps_text, stats_text};
    use lite_bar_core::sessions::{
        liveness_query_due, session_log_line, OpenSession, QueryGate, SessionBook,
    };
    use lite_bar_core::settings::{Settings, StatsLayout};
    use lite_bar_core::settings_edit::SettingsChange;
    use lite_bar_core::startup_task::StartupState;
    use lite_bar_core::update::{
        current_version, ReleaseInfo, UpdateSchedule, UpdateState, Version,
    };
    use windows::core::{w, HSTRING};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::{
        ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DestroyWindow,
        DispatchMessageW, GetMessageW, KillTimer, PostQuitMessage, RegisterClassExW,
        RegisterWindowMessageW, SetTimer, TranslateMessage, MSG, MSGFLT_ALLOW, SW_SHOWNORMAL,
        WINDOW_EX_STYLE, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_ENDSESSION, WM_HOTKEY,
        WM_LBUTTONDBLCLK, WM_RBUTTONUP, WM_TIMER, WNDCLASSEXW, WS_OVERLAPPED,
    };

    mod settings;
    mod startup;
    mod updates;

    use crate::elevation::{is_elevated, relaunch_as_admin};
    use crate::etw::{anchored_focus_clock, offers_admin, CaptureError, PresentCapture};
    use crate::foreground::{foreground_window, window_title};
    use crate::game_lookup::resolve_name;
    use crate::hotkeys::{self, HotkeyId, Registration};
    use crate::installer::{
        download_release, is_updated_launch, relaunch, shell_execute_result, spawn_cleanup,
        InstallResult, WM_INSTALL_RESULT,
    };
    use crate::overlay::Overlay;
    use crate::paths::{app_dir, load_or_create_settings};
    use crate::process_info::{process_info, process_state, ProcessInfo, ProcessState};
    use crate::settings_window::{SaveDebounce, SettingsUi, WM_SETTINGS_ACTIONS};
    use crate::single_instance::{
        acquire_single_instance, wait_for, InstanceGuard, UPDATED_POLL, UPDATED_WAIT,
    };
    use crate::startup::{StartupResult, WM_STARTUP_RESULT};
    use crate::stats_files::{list_games, local_now, StatsWriter};
    use crate::tray::{MenuItem, Tray, TrayMenuState, WM_TRAY};
    use crate::updater::{fetch_latest, take_result, CheckResult, LATEST_URL, WM_UPDATE_RESULT};
    use crate::{log, tray};

    const MUTEX_NAME: &str = "Local\\LiteBar.SingleInstance";
    const TIMER_ID: usize = 1;
    const TIMER_MS: u32 = 250;
    /// Owner-window timer that writes the settings file once the save debounce is due.
    const SAVE_TIMER_ID: usize = 2;
    /// Owner-window timer for the next automatic update check.
    const UPDATE_TIMER_ID: usize = 3;
    const SETTINGS_FILE: &str = "settings.toml";
    /// Subfolder of the app dir holding `games.toml`, `games.csv` and `sessions.csv`.
    const STATS_DIR: &str = "stats";
    /// Prune the tracker every this many timer ticks (5 s).
    const PRUNE_EVERY_TICKS: u32 = 20;
    /// Check whether tracked game processes are still running every this many ticks (1 s).
    const ALIVE_CHECK_EVERY_TICKS: u32 = 4;

    thread_local! {
        static APP: RefCell<Option<App>> = const { RefCell::new(None) };
        /// True while the tray menu's modal loop runs (it re-enters the window procedure).
        static MENU_OPEN: Cell<bool> = const { Cell::new(false) };
        /// Id of the registered "TaskbarCreated" broadcast.
        static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
        /// An update check result taken from its message, until the app can apply it.
        static PENDING_UPDATE: RefCell<Option<CheckResult>> = const { RefCell::new(None) };
        /// A finished update download, until the app can apply it.
        static PENDING_INSTALL: RefCell<Option<InstallResult>> = const { RefCell::new(None) };
        /// A startup-task job's result, until the app can apply it.
        static PENDING_STARTUP: RefCell<Option<StartupResult>> = const { RefCell::new(None) };
    }

    /// Runs `f` on the app state, if it exists and is not already borrowed (re-entrancy).
    /// Afterwards applies whatever the settings window queued and an update check result
    /// that is waiting, so a nudge or a result that arrived while the app was busy is never
    /// lost.
    fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
        APP.with(|app| {
            let mut guard = app.try_borrow_mut().ok()?;
            let app = guard.as_mut()?;
            // Before `f`, so it sees the latest state, and after, for one that arrived
            // while `f` pumped messages.
            app.apply_pending_update();
            let r = f(app);
            app.apply_pending_update();
            app.drain_settings_actions();
            Some(r)
        })
    }

    /// Hidden top-level owner window (not message-only: those never see the `TaskbarCreated`
    /// broadcast). Never shown. Destroyed on drop.
    struct OwnerWindow(HWND);

    impl Drop for OwnerWindow {
        fn drop(&mut self) {
            // SAFETY: created on this thread, destroyed exactly once.
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    fn create_owner_window() -> windows::core::Result<OwnerWindow> {
        // SAFETY: plain FFI; the class name is static and NUL-terminated.
        unsafe {
            let instance = GetModuleHandleW(None)?.into();
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: w!("LiteBarOwner"),
                ..Default::default()
            };
            RegisterClassExW(&class);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("LiteBarOwner"),
                w!("Lite Bar"),
                WS_OVERLAPPED, // no WS_VISIBLE: never shown
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            )?;
            Ok(OwnerWindow(hwnd))
        }
    }

    struct App {
        hwnd: HWND,
        dir: PathBuf,
        settings: Settings,
        overlay: Overlay,
        tray: Rc<Tray>,
        tracker: Arc<Mutex<FpsTracker>>,
        /// Time base shared with the ETW callback (`received_at`).
        clock: Instant,
        /// Stamps focus changes in the ETW event time base; anchored at each capture start.
        focus_clock: FocusClock,
        capture: Option<PresentCapture>,
        guard: Option<InstanceGuard>,
        /// Whether the overlay is currently running (timer, capture, window shown).
        overlay_on: bool,
        needs_admin: bool,
        hotkey_ok: bool,
        ticks: u32,
        /// Open play sessions; they survive the overlay being hidden.
        sessions: SessionBook,
        writer: StatsWriter,
        /// Keeps a refused process query from being retried during the same focus.
        gate: QueryGate,
        /// Set by `start_overlay`: the next tick runs the process-exit check.
        just_shown: bool,
        /// App-clock time each tracked pid's process was last queried.
        last_query: HashMap<u32, f64>,
        /// The settings window while it is open.
        settings_ui: Option<SettingsUi>,
        /// When the window's changes are due to be written.
        save: SaveDebounce,
        /// Changes not yet written to the settings file, in order.
        unsaved: Vec<SettingsChange>,
        /// The toggle hotkey is unregistered while the window's capture field listens.
        hotkey_released: bool,
        /// Why the last settings save failed (shown in the window), until one works.
        save_error: Option<&'static str>,
        /// What the last update check found (shown in the tray and the window).
        update: UpdateState,
        /// When update checks run; times are `clock.elapsed()`.
        update_schedule: UpdateSchedule,
        /// Where checks ask for the latest release (a local stub in tests).
        latest_url: String,
        /// Runs on the worker thread for each check (`updater::fetch_latest`).
        fetch: fn(&str) -> CheckResult,
        /// This process's exe, which an update replaces (a temp file in tests).
        exe_path: Option<PathBuf>,
        /// Runs on the worker thread for Update now (`installer::download_release`).
        download: fn(&ReleaseInfo, &Path) -> InstallResult,
        /// Starts the updated exe (`installer::relaunch`).
        relaunch: fn(&Path) -> Result<(), String>,
        /// The release Update now last started on (its page stays reachable on failure).
        installing: Option<ReleaseInfo>,
        /// The version now on disk after a swap, while this older instance still runs.
        installed_version: Option<Version>,
        /// The startup task's state, as the Settings window shows it.
        startup: StartupState,
        /// The state before the change in progress (restored when UAC is declined).
        startup_before: StartupState,
        /// Whether this process runs as administrator.
        elevated: bool,
        /// Runs on a worker thread to query the task (`startup::query_now`).
        startup_query: fn() -> StartupState,
        /// Runs on a worker thread to change it: (on, elevated, parent window) →
        /// `startup::change`.
        startup_change: fn(bool, bool, usize) -> StartupResult,
    }

    impl App {
        fn apply_pending_update(&mut self) {
            if let Some(result) = PENDING_UPDATE.take() {
                self.on_update_result(result);
            }
            if let Some(result) = PENDING_INSTALL.take() {
                if self.on_install_result(result) {
                    quit();
                }
            }
            if let Some(result) = PENDING_STARTUP.take() {
                self.on_startup_result(result);
            }
        }

        fn menu_state(&self) -> TrayMenuState {
            TrayMenuState {
                overlay_visible: self.overlay_on,
                toggle_hotkey: self.settings.hotkeys.toggle_overlay.to_string(),
                toggle_available: self.hotkey_ok,
                needs_admin: self.needs_admin,
                update_available: match &self.update {
                    UpdateState::Available(r) => Some(r.version),
                    _ => None,
                },
            }
        }

        fn register_hotkey(&mut self) {
            self.hotkey_released = false;
            hotkeys::unregister(self.hwnd, HotkeyId::ToggleOverlay);
            let hk = self.settings.hotkeys.toggle_overlay;
            self.hotkey_ok = matches!(
                hotkeys::register(self.hwnd, HotkeyId::ToggleOverlay, hk),
                Registration::Ok
            );
            if !self.hotkey_ok {
                log::warn(&format!("hotkey {hk} is unavailable (already in use?)"));
            }
        }

        fn start_overlay(&mut self) {
            self.overlay_on = true;
            self.needs_admin = false;
            // ETW's event times run on QPC from the trace start, without later clock
            // corrections: anchor the focus stamps to the same start.
            self.focus_clock = anchored_focus_clock();
            // Show the overlay (with "—") and start the timer first, so the ETW start latency
            // is not part of what the user sees.
            self.overlay.show();
            // SAFETY: plain FFI on our own window.
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_ID, TIMER_MS, None);
            }
            self.just_shown = true;
            self.on_timer();
            match PresentCapture::start(self.tracker.clone(), self.clock) {
                Ok(capture) => self.capture = Some(capture),
                Err(err) => {
                    match &err {
                        CaptureError::NeedsAdmin => {
                            log::info("FPS capture needs administrator rights")
                        }
                        CaptureError::Other(msg) => {
                            log::error(&format!("FPS capture failed: {msg}"))
                        }
                    }
                    self.needs_admin = offers_admin(&err, is_elevated());
                }
            }
        }

        fn stop_overlay(&mut self) {
            self.overlay_on = false;
            // A capture restart must never count the gap as a frame time.
            self.tracker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_focus(None, self.focus_clock.now());
            // Hide and stop the timer first, so the ETW stop latency is not visible.
            self.overlay.hide();
            // SAFETY: plain FFI on our own window.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_ID);
            }
            if let Some(capture) = self.capture.take() {
                capture.stop();
            }
        }

        fn toggle_overlay(&mut self) {
            if self.overlay_on {
                self.stop_overlay();
            } else {
                self.start_overlay();
            }
            self.sync_settings_window();
        }

        fn on_timer(&mut self) {
            let now = self.clock.elapsed().as_secs_f64();
            // One read of the foreground window: the pid and the title come from the same one.
            let foreground = foreground_window();
            let pid = foreground.map(|(pid, _)| pid);
            let want_stats = self.settings.overlay.stats != StatsLayout::Off;
            let refused = self.gate.on_focus(pid);
            // One lock for focus, FPS and stats; the summary is Copy so it is released early.
            let (fps, summary, has_stats) = {
                let mut tracker = self.tracker.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(refused) = refused {
                    // Focus left a process that refused the query: its frames can't be
                    // recorded.
                    tracker.take_stats(refused);
                }
                tracker.set_focus(pid, self.focus_clock.now());
                match pid {
                    Some(pid) => {
                        let hist = tracker.stats(pid);
                        (
                            tracker.fps(pid, now),
                            hist.and_then(|h| h.summary()),
                            hist.is_some(),
                        )
                    }
                    None => (None, None, false),
                }
            };
            let stats = want_stats.then(|| stats_text(summary.as_ref()));
            self.overlay.update(&fps_text(fps), stats.as_ref());
            self.ticks = self.ticks.wrapping_add(1);
            // Before tracking: a pid reused while the overlay was hidden must not add frames
            // to the earlier run's session.
            if session_check_due(self.ticks, std::mem::take(&mut self.just_shown)) {
                self.check_sessions();
            }
            if let Some((pid, hwnd)) = foreground {
                if pid_to_track(Some(pid), has_stats, &self.sessions, &self.gate).is_some() {
                    self.start_session(pid, hwnd, now);
                }
            }
            if self.ticks.is_multiple_of(PRUNE_EVERY_TICKS) {
                self.tracker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .prune(now);
            }
        }

        /// Opens a session for the foreground `pid`, which has just counted its first frames.
        /// `hwnd` is the foreground window read on this tick (the session takes its title);
        /// `now` is the app clock.
        fn start_session(&mut self, pid: u32, hwnd: HWND, now: f64) {
            let Some(info) = process_info(pid) else {
                // Live stats still show; the frames are discarded when focus moves away.
                self.gate.refused(pid);
                log::warn(&format!(
                    "stats: cannot query process {pid}; its play time is not recorded"
                ));
                return;
            };
            match new_session(pid, info, || window_title(hwnd), local_now()) {
                Some(session) => {
                    // The query just made counts for the liveness check.
                    self.last_query.insert(pid, now);
                    if let Some(old) = self.sessions.track(session) {
                        log::warn(&format!(
                            "stats: pid {} was reused; ending the earlier session",
                            old.pid
                        ));
                        let end = old.last_alive;
                        self.finish_session(old, end);
                    }
                }
                // A known non-game: not asked again during this focus, and its frames are
                // discarded when focus moves away. Live stats still show.
                None => self.gate.refused(pid),
            }
        }

        /// The 1 s check: sessions whose process exited end when it was last seen alive. A
        /// process that is still presenting is alive and is only queried every 30 s.
        fn check_sessions(&mut self) {
            let now = self.clock.elapsed().as_secs_f64();
            let last_present: Vec<(u32, Option<f64>)> = {
                let tracker = self.tracker.lock().unwrap_or_else(|e| e.into_inner());
                let pids = self.sessions.pids();
                pids.into_iter()
                    .map(|pid| (pid, tracker.last_received(pid)))
                    .collect()
            };
            let last_query = &mut self.last_query;
            let finished = self.sessions.sweep(local_now(), |s| {
                let presented = last_present
                    .iter()
                    .find(|(pid, _)| *pid == s.pid)
                    .and_then(|(_, at)| *at);
                if !liveness_query_due(presented, last_query.get(&s.pid).copied(), now) {
                    return true;
                }
                last_query.insert(s.pid, now);
                still_running(s)
            });
            for (session, end) in finished {
                self.last_query.remove(&session.pid);
                self.finish_session(session, end);
            }
        }

        /// Quit or sign-out: every session ends (now, if its process still runs) and queued
        /// stats writes are retried; what still fails is saved for the next start.
        fn finish_all_sessions(&mut self) {
            let finished = self.sessions.drain(local_now(), still_running);
            for (session, end) in finished {
                self.finish_session(session, end);
            }
            self.writer.shutdown_flush(&self.settings.game_names);
        }

        /// Takes the session's frame stats and stores it if it is long enough.
        fn finish_session(&mut self, session: OpenSession, end: LocalTime) {
            let hist = self
                .tracker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take_stats(session.pid);
            let Some(hist) = hist else {
                return;
            };
            if let Some(record) = session.finish(end, hist, resolve_name) {
                let names = &self.settings.game_names;
                let shown = display_name(&record.name, &record.exe_path, names);
                log::info(&session_log_line(&record, &shown));
                self.writer.record(&record, names);
                if let Some(ui) = &self.settings_ui {
                    ui.set_games(list_games(self.writer.stats_dir()));
                }
            }
        }

        fn reload_settings(&mut self) {
            self.reload_settings_file();
            self.register_hotkey();
            log::info("settings reloaded");
        }

        /// Re-reads the file (after writing the window's pending changes into it) and shows
        /// the result in the overlay and the open window.
        fn reload_settings_file(&mut self) {
            self.flush_settings_save();
            let (settings, warnings) = load_or_create_settings(&self.dir);
            for w in &warnings {
                log::warn(w);
            }
            if game_names_changed(&self.settings, &settings) {
                self.writer.mark_games_csv_dirty();
            }
            self.overlay.set_settings(&settings.overlay);
            self.overlay.redraw();
            self.settings = settings;
            self.sync_update_schedule();
            self.writer.flush(&self.settings.game_names);
            if let Some(ui) = &self.settings_ui {
                ui.set_games(list_games(self.writer.stats_dir()));
            }
            self.sync_settings_window();
        }

        fn open_settings_file(&self) {
            shell_open(self.dir.join("settings.toml"), "settings file");
        }

        fn open_game_stats(&self) {
            match self.writer.open_target() {
                Ok(target) => shell_open(&target, "game stats"),
                Err(e) => log::error(&format!(
                    "could not create {}: {e}",
                    self.writer.stats_dir().display()
                )),
            }
        }

        /// Relaunches elevated. Returns true if this process should exit.
        fn restart_as_admin(&mut self) -> bool {
            let was_on = self.overlay_on;
            // The new instance reads the file; the window stays open if UAC is declined.
            self.save_settings_before_restart();
            self.guard = None;
            hotkeys::unregister(self.hwnd, HotkeyId::ToggleOverlay);
            self.stop_overlay();
            self.tray.remove();
            match relaunch_as_admin() {
                Ok(()) => {
                    self.close_settings_window_for_exit();
                    true
                }
                Err(e) => {
                    // Typically the user declined UAC (error 1223). Carry on as before.
                    log::info(&format!("elevated relaunch not started: {e}"));
                    if let Err(e) = self.tray.add() {
                        log::error(&format!("could not re-add tray icon: {e}"));
                    }
                    self.guard = acquire_guard();
                    self.register_hotkey();
                    if was_on {
                        self.start_overlay();
                    }
                    false
                }
            }
        }

        fn handle_menu(&mut self, item: MenuItem) {
            match item {
                MenuItem::OpenSettings => self.open_settings(),
                MenuItem::ToggleOverlay => self.toggle_overlay(),
                MenuItem::OpenSettingsFile => self.open_settings_file(),
                MenuItem::OpenGameStats => self.open_game_stats(),
                MenuItem::ReloadSettings => self.reload_settings(),
                MenuItem::RestartAsAdmin => {
                    if self.restart_as_admin() {
                        quit();
                    }
                }
                MenuItem::InstallUpdate => self.install_update(),
                MenuItem::Quit => {
                    self.close_settings_window();
                    quit();
                }
            }
        }

        /// Stops capture, removes the tray icon and saves the open sessions (the icon and
        /// window go with the drop).
        fn shutdown(&mut self) {
            log::info("shutting down");
            self.close_settings_window();
            hotkeys::unregister(self.hwnd, HotkeyId::ToggleOverlay);
            self.stop_overlay();
            self.tray.remove();
            self.finish_all_sessions();
        }

        /// The user is signing out or Windows is shutting down: the process may be ended as
        /// soon as this returns.
        fn end_session(&mut self) {
            // Drains the window and ends its edits too, so nothing typed is lost.
            self.close_settings_window();
            self.stop_overlay();
            self.finish_all_sessions();
        }
    }

    /// Whether a session's process still runs; logs why when it doesn't.
    fn still_running(s: &OpenSession) -> bool {
        match process_state(s.pid, s.created) {
            ProcessState::Running => true,
            ProcessState::Exited => {
                log::info(&format!("stats: {} (pid {}) exited", s.exe_path, s.pid));
                false
            }
            ProcessState::Unqueryable => {
                log::warn(&format!(
                    "stats: {} (pid {}) can no longer be queried; ending its session",
                    s.exe_path, s.pid
                ));
                false
            }
        }
    }

    /// Opens `target` (a file, a folder or a web page) in its default program.
    fn shell_open(target: impl AsRef<OsStr>, what: &str) {
        let path = HSTRING::from(target.as_ref());
        // SAFETY: valid NUL-terminated strings for the duration of the call.
        let result = unsafe { ShellExecuteW(None, w!("open"), &path, None, None, SW_SHOWNORMAL) };
        if let Err(e) = shell_execute_result(result.0 as usize) {
            log::error(&format!("could not open {what}: {e}"));
        }
    }

    /// What the owner window does about a lifecycle message.
    #[derive(Debug, PartialEq, Eq)]
    enum Command {
        /// Same path as the tray's Quit item.
        Quit,
        /// The window is going away anyway: make sure the message loop ends.
        PostQuit,
        /// The session is ending: stop the ETW session so it does not outlive the logoff, and
        /// save the open play sessions.
        StopCapture,
    }

    /// The session for `pid`, started at `now`; `None` for a known non-game (a browser,
    /// launcher, the shell, ...). `window_title` reads the title of the window found on the
    /// same tick, and only runs for a game.
    fn new_session(
        pid: u32,
        info: ProcessInfo,
        window_title: impl FnOnce() -> Option<String>,
        now: LocalTime,
    ) -> Option<OpenSession> {
        if is_known_non_game(&info.exe_path) {
            return None;
        }
        Some(OpenSession {
            pid,
            created: info.created,
            exe_path: info.exe_path,
            window_title: window_title(),
            start: now,
            last_alive: now,
        })
    }

    /// The foreground pid that needs a new session: it has counted frames, none is open, and
    /// its process hasn't already refused a query during this focus.
    fn pid_to_track(
        pid: Option<u32>,
        has_stats: bool,
        sessions: &SessionBook,
        gate: &QueryGate,
    ) -> Option<u32> {
        pid.filter(|pid| has_stats && !sessions.contains(*pid) && gate.may_query(*pid))
    }

    /// Whether this tick (counted after incrementing) runs the process-exit check: every 1 s,
    /// and on the first tick after the overlay is shown (before new sessions are tracked).
    fn session_check_due(ticks: u32, first_tick_after_show: bool) -> bool {
        first_tick_after_show || ticks.is_multiple_of(ALIVE_CHECK_EVERY_TICKS)
    }

    /// A reload regenerates `games.csv` only when the display-name overrides changed.
    fn game_names_changed(old: &Settings, new: &Settings) -> bool {
        old.game_names != new.game_names
    }

    fn window_command(msg: u32, wparam: usize) -> Option<Command> {
        match msg {
            WM_CLOSE => Some(Command::Quit),
            WM_DESTROY => Some(Command::PostQuit),
            WM_ENDSESSION if wparam != 0 => Some(Command::StopCapture),
            _ => None,
        }
    }

    /// Lets an elevated process receive `msg` from lower-integrity senders (Explorer's
    /// `TaskbarCreated` broadcast is otherwise dropped by UIPI).
    fn allow_through_uipi(hwnd: HWND, msg: u32) -> windows::core::Result<()> {
        // SAFETY: plain FFI on a window we own.
        unsafe { ChangeWindowMessageFilterEx(hwnd, msg, MSGFLT_ALLOW, None) }
    }

    fn quit() {
        // SAFETY: plain FFI.
        unsafe { PostQuitMessage(0) };
    }

    /// This process's exe file, or `None` (logged) when Windows can't say.
    fn own_exe() -> Option<PathBuf> {
        std::env::current_exe()
            .inspect_err(|e| log::error(&format!("can't find Lite Bar's exe: {e}")))
            .ok()
    }

    /// The single-instance mutex for an instance started by an update: the old one may still
    /// be exiting, so `acquire` is retried (every 200 ms, up to 10 s) while another instance
    /// holds it. An unexpected error ends the wait.
    fn guard_after_update<G>(
        mut acquire: impl FnMut() -> windows::core::Result<Option<G>>,
    ) -> windows::core::Result<Option<G>> {
        let start = Instant::now();
        wait_for(
            UPDATED_WAIT,
            UPDATED_POLL,
            || start.elapsed(),
            std::thread::sleep,
            || match acquire() {
                Ok(None) => None,
                other => Some(other),
            },
        )
        .unwrap_or(Ok(None))
    }

    /// Takes the single-instance mutex. Fails open: an unexpected error is logged and the app
    /// runs without a guard. `None` is also returned if another instance holds it.
    fn acquire_guard() -> Option<InstanceGuard> {
        match acquire_single_instance(MUTEX_NAME) {
            Ok(guard) => guard,
            Err(e) => {
                log::error(&format!("single-instance check failed: {e}"));
                None
            }
        }
    }

    fn on_tray_menu() {
        if MENU_OPEN.get() {
            return;
        }
        // Release the borrow before the menu's modal loop, which re-enters the window procedure.
        let Some((tray, state)) = with_app(|app| (app.tray.clone(), app.menu_state())) else {
            return;
        };
        MENU_OPEN.set(true);
        let chosen = tray.show_menu(&state);
        MENU_OPEN.set(false);
        if let Some(item) = chosen {
            with_app(|app| app.handle_menu(item));
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_TIMER if wparam.0 == TIMER_ID => {
                with_app(App::on_timer);
                return LRESULT(0);
            }
            WM_TIMER if wparam.0 == SAVE_TIMER_ID => {
                with_app(|app| app.on_save_timer(Instant::now()));
                return LRESULT(0);
            }
            WM_TIMER if wparam.0 == UPDATE_TIMER_ID => {
                with_app(|app| app.on_update_timer(app.clock.elapsed()));
                return LRESULT(0);
            }
            WM_UPDATE_RESULT => {
                // SAFETY: posted by `updater::post_result`; each message is taken once.
                let result = unsafe { take_result(lparam) };
                // Applied by `with_app`, now or after whatever has the app borrowed.
                PENDING_UPDATE.set(Some(result));
                with_app(|_| ());
                return LRESULT(0);
            }
            WM_INSTALL_RESULT => {
                // SAFETY: posted by `installer::spawn_download`; each message is taken once.
                let result = unsafe { crate::installer::take_result(lparam) };
                PENDING_INSTALL.set(Some(result));
                with_app(|_| ());
                return LRESULT(0);
            }
            WM_STARTUP_RESULT => {
                // SAFETY: posted by `startup::spawn`; each message is taken once.
                let result = unsafe { crate::startup::take_result(lparam) };
                PENDING_STARTUP.set(Some(result));
                with_app(|_| ());
                return LRESULT(0);
            }
            WM_SETTINGS_ACTIONS => {
                // `with_app` drains the window's queue.
                with_app(|_| ());
                return LRESULT(0);
            }
            WM_HOTKEY if wparam.0 == HotkeyId::ToggleOverlay as usize => {
                with_app(App::toggle_overlay);
                return LRESULT(0);
            }
            WM_TRAY => {
                let event = (lparam.0 & 0xFFFF) as u32;
                if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                    on_tray_menu();
                } else if event == WM_LBUTTONDBLCLK && !MENU_OPEN.get() {
                    with_app(App::open_settings);
                }
                return LRESULT(0);
            }
            m if m != 0 && m == TASKBAR_CREATED.get() => {
                // Explorer restarted: its notification area is empty again.
                with_app(|app| {
                    app.tray.remove();
                    if let Err(e) = app.tray.add() {
                        log::error(&format!("could not re-add tray icon: {e}"));
                    }
                });
                return LRESULT(0);
            }
            _ => {}
        }
        match window_command(msg, wparam.0) {
            Some(Command::Quit) => {
                quit();
                return LRESULT(0);
            }
            Some(Command::PostQuit) => quit(),
            Some(Command::StopCapture) => {
                if with_app(App::end_session).is_none() {
                    log::warn("session ending while the app is busy; open play sessions not saved");
                }
                return LRESULT(0);
            }
            None => {}
        }
        // SAFETY: forwards the arguments unchanged from the system.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// Human-readable text for the panic hook's log line.
    fn panic_message(payload: &(dyn Any + Send), location: Option<(&str, u32, u32)>) -> String {
        let text = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<non-string payload>");
        match location {
            Some((file, line, col)) => format!("panic: {text} at {file}:{line}:{col}"),
            None => format!("panic: {text} at <unknown location>"),
        }
    }

    /// Whether the panic hook stops the ETW trace. A panic inside the settings window's frame
    /// is caught by its host (the window closes, the app carries on), and one on an update
    /// check, download, cleanup or startup-task thread (`thread` is the panicking thread's name) by that
    /// worker, so the trace must stay.
    fn panic_stops_trace(in_ui_frame: bool, thread: Option<&str>) -> bool {
        let workers = [
            crate::updater::WORKER_THREAD,
            crate::installer::WORKER_THREAD,
            crate::installer::CLEANUP_THREAD,
            crate::startup::WORKER_THREAD,
        ];
        !in_ui_frame && !thread.is_some_and(|t| workers.contains(&t))
    }

    /// A panic in the window procedure would abort at the FFI boundary: the app would vanish
    /// silently and leak its ETW session. Log it and stop the session on the way out. The
    /// previous hook is not called (there is no console).
    fn install_panic_hook() {
        std::panic::set_hook(Box::new(|info| {
            let location = info.location().map(|l| (l.file(), l.line(), l.column()));
            log::error(&panic_message(info.payload(), location));
            let thread = std::thread::current();
            if panic_stops_trace(crate::ui_host::in_ui_frame(), thread.name()) {
                let _ = ferrisetw::trace::stop_trace_by_name(crate::etw::SESSION_NAME);
            }
        }));
    }

    pub fn run() {
        // SAFETY: plain FFI. Fails (ignored) if a manifest already set the awareness.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let dir = app_dir();
        log::init(&dir);
        install_panic_hook();

        // The elevated helper a non-elevated Lite Bar starts for "Start with Windows": no
        // mutex, tray, overlay or capture; it changes the task and exits.
        if let Some(flag) = crate::startup::parse_flag(std::env::args().skip(1)) {
            std::process::exit(crate::startup::run_mode(flag));
        }

        let updated = is_updated_launch(std::env::args().skip(1));
        let acquired = if updated {
            guard_after_update(|| acquire_single_instance(MUTEX_NAME))
        } else {
            acquire_single_instance(MUTEX_NAME)
        };
        let guard = match acquired {
            Ok(Some(guard)) => Some(guard),
            Ok(None) => {
                if updated {
                    log::warn("updated, but the previous Lite Bar is still running; exiting");
                }
                return; // already running: exit silently
            }
            Err(e) => {
                log::error(&format!(
                    "single-instance check failed, running anyway: {e}"
                ));
                None
            }
        };

        if updated {
            log::info(&format!("updated to v{}", current_version()));
        }
        // Also after a run that crashed or was killed between the swap and the cleanup.
        if let Some(exe) = own_exe() {
            spawn_cleanup(exe);
        }

        let (settings, warnings) = load_or_create_settings(&dir);
        for w in &warnings {
            log::warn(w);
        }

        if let Err(e) = start(dir, settings, guard) {
            log::error(&format!("startup failed: {e}"));
        }
    }

    fn start(
        dir: PathBuf,
        settings: Settings,
        guard: Option<InstanceGuard>,
    ) -> windows::core::Result<()> {
        // SAFETY: plain FFI with a static NUL-terminated name.
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        TASKBAR_CREATED.set(taskbar_created);
        let owner = create_owner_window()?;
        if let Err(e) = allow_through_uipi(owner.0, taskbar_created) {
            log::warn(&format!("could not allow TaskbarCreated through UIPI: {e}"));
        }
        let tray = Rc::new(tray::Tray::new(owner.0)?);
        let overlay = Overlay::new(&settings.overlay)?;

        let start_visible = settings.overlay.visible;
        let update_checks = settings.updates.check;
        let mut writer = StatsWriter::new(dir.join(STATS_DIR));
        // Writes what an earlier run saved in pending.toml.
        writer.flush(&settings.game_names);
        let mut app = App {
            hwnd: owner.0,
            dir,
            settings,
            overlay,
            tray,
            tracker: Arc::new(Mutex::new(FpsTracker::new())),
            clock: Instant::now(),
            focus_clock: anchored_focus_clock(),
            capture: None,
            guard,
            overlay_on: false,
            needs_admin: false,
            hotkey_ok: false,
            ticks: 0,
            sessions: SessionBook::new(),
            writer,
            gate: QueryGate::default(),
            just_shown: false,
            last_query: HashMap::new(),
            settings_ui: None,
            save: SaveDebounce::default(),
            unsaved: Vec::new(),
            hotkey_released: false,
            save_error: None,
            update: UpdateState::Idle,
            update_schedule: UpdateSchedule::new(update_checks, Duration::ZERO),
            latest_url: LATEST_URL.to_string(),
            fetch: fetch_latest,
            exe_path: own_exe(),
            download: download_release,
            relaunch,
            installing: None,
            installed_version: None,
            startup: StartupState::Unknown,
            startup_before: StartupState::Unknown,
            elevated: is_elevated(),
            startup_query: crate::startup::query_now,
            startup_change: crate::startup::change,
        };
        app.arm_update_timer();
        app.register_hotkey();
        if start_visible {
            app.start_overlay();
        }
        log::info(&format!(
            "started (elevated: {}, hotkey available: {})",
            is_elevated(),
            app.hotkey_ok
        ));
        APP.with(|slot| *slot.borrow_mut() = Some(app));

        // SAFETY: standard message loop on the thread that owns the windows.
        unsafe {
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        if let Some(mut app) = APP.with(|slot| slot.borrow_mut().take()) {
            app.shutdown();
        }
        drop(owner);
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::test_util::TempDir;
        use crate::updater::post_result;
        use lite_bar_core::hotkey::Hotkey;
        use lite_bar_core::settings_ui::UiAction;

        #[test]
        fn panic_message_formats_str_payload_and_location() {
            let payload: Box<dyn Any + Send> = Box::new("boom");
            assert_eq!(
                panic_message(&*payload, Some(("src/main.rs", 12, 5))),
                "panic: boom at src/main.rs:12:5"
            );
        }

        /// A user's settings file: the template plus a comment of their own, CRLF-free.
        fn user_settings_text() -> String {
            lite_bar_core::settings::default_settings_toml()
                .replace("counter_size = 18", "counter_size = 18 # my size")
        }

        /// An `App` over `dir` with hidden windows: no tray icon, no hotkey, no capture.
        fn test_app(dir: &Path) -> (App, OwnerWindow) {
            let owner = create_owner_window().unwrap();
            let (settings, _) = load_or_create_settings(dir);
            let overlay = Overlay::new(&settings.overlay).unwrap();
            let checks = settings.updates.check;
            let app = App {
                hwnd: owner.0,
                dir: dir.to_path_buf(),
                settings,
                overlay,
                tray: Rc::new(Tray::new_hidden(owner.0).unwrap()),
                tracker: Arc::new(Mutex::new(FpsTracker::new())),
                clock: Instant::now(),
                focus_clock: anchored_focus_clock(),
                capture: None,
                guard: None,
                overlay_on: false,
                needs_admin: false,
                hotkey_ok: false,
                ticks: 0,
                sessions: SessionBook::new(),
                writer: StatsWriter::new(dir.join(STATS_DIR)),
                gate: QueryGate::default(),
                just_shown: false,
                last_query: HashMap::new(),
                settings_ui: None,
                save: SaveDebounce::default(),
                unsaved: Vec::new(),
                hotkey_released: false,
                save_error: None,
                update: UpdateState::Idle,
                update_schedule: UpdateSchedule::new(checks, Duration::ZERO),
                // A closed local port: tests never ask GitHub.
                latest_url: "http://127.0.0.1:1/".to_string(),
                fetch: fetch_latest,
                // Tests never touch the real exe, GitHub or the shell.
                exe_path: None,
                download: |_, _| {
                    Err(crate::installer::InstallError::Io(
                        "tests never download".into(),
                    ))
                },
                relaunch: |_| Err("tests never relaunch".into()),
                installing: None,
                installed_version: None,
                startup: StartupState::Unknown,
                startup_before: StartupState::Unknown,
                elevated: true,
                // Tests never touch the real scheduled task.
                startup_query: || StartupState::Off,
                startup_change: |_, _, _| {
                    StartupResult::Changed(StartupState::Error {
                        msg: "tests never change it".into(),
                        actual: None,
                    })
                },
            };
            (app, owner)
        }

        fn settings_file(dir: &Path) -> String {
            std::fs::read_to_string(dir.join("settings.toml")).unwrap()
        }

        fn counter(n: u16) -> UiAction {
            UiAction::Change(SettingsChange::CounterSize(n))
        }

        #[test]
        fn slider_change_applies_live_and_saves_with_comments() {
            let dir = TempDir::new("settings-live");
            std::fs::write(dir.path().join("settings.toml"), user_settings_text()).unwrap();
            let (mut app, _owner) = test_app(dir.path());
            app.open_settings_hidden();
            let ui = app.settings_ui.as_ref().expect("window open");
            ui.paint(); // one real frame of the page
            assert!(ui.is_open());
            ui.queue(counter(30));
            app.drain_settings_actions();

            assert_eq!(app.settings.overlay.counter_size, 30);
            assert_eq!(app.overlay.settings().counter_size, 30);
            let shown = app.settings_ui.as_ref().unwrap().shown();
            assert_eq!(shown.overlay.counter_size, 30);
            assert_eq!(
                settings_file(dir.path()),
                user_settings_text(),
                "not written before the debounce"
            );

            app.on_save_timer(Instant::now() + Duration::from_millis(400));
            assert_eq!(
                settings_file(dir.path()),
                user_settings_text().replace("counter_size = 18 #", "counter_size = 30 #")
            );
        }

        #[test]
        fn close_flushes_pending_save() {
            let dir = TempDir::new("settings-close");
            std::fs::write(dir.path().join("settings.toml"), user_settings_text()).unwrap();
            let (mut app, _owner) = test_app(dir.path());
            app.open_settings_hidden();
            app.settings_ui.as_ref().unwrap().queue(counter(40));
            app.drain_settings_actions();
            app.close_settings_window();
            assert!(app.settings_ui.is_none(), "window dropped");
            assert!(settings_file(dir.path()).contains("counter_size = 40 # my size"));
        }

        #[test]
        fn window_x_button_drains_closes_and_saves() {
            let dir = TempDir::new("settings-x");
            let (mut app, _owner) = test_app(dir.path());
            app.open_settings_hidden();
            let ui = app.settings_ui.as_ref().unwrap();
            ui.queue(UiAction::Change(SettingsChange::TextSize(14)));
            ui.send_close();
            app.drain_settings_actions();
            assert!(app.settings_ui.is_none());
            assert_eq!(app.settings.overlay.text_size, 14);
            // The default file written at load, edited in place.
            assert!(settings_file(dir.path()).contains("text_size = 14\n"));
            assert!(settings_file(dir.path()).contains("# Height of the \"FPS\" label"));
        }

        #[test]
        fn missing_file_is_saved_from_the_template() {
            let dir = TempDir::new("settings-missing");
            let (mut app, _owner) = test_app(dir.path());
            std::fs::remove_file(dir.path().join("settings.toml")).unwrap();
            app.open_settings_hidden();
            app.settings_ui.as_ref().unwrap().queue(counter(25));
            app.drain_settings_actions();
            app.close_settings_window();
            let text = settings_file(dir.path());
            assert!(text.starts_with("# Lite Bar settings."));
            assert!(text.contains("counter_size = 25\n"));
        }

        #[test]
        fn invalid_file_is_left_alone_and_settings_stay() {
            let dir = TempDir::new("settings-invalid");
            let (mut app, _owner) = test_app(dir.path());
            std::fs::write(dir.path().join("settings.toml"), "broken [[[").unwrap();
            app.open_settings_hidden();
            app.settings_ui.as_ref().unwrap().queue(counter(26));
            app.drain_settings_actions();
            app.close_settings_window();
            assert_eq!(settings_file(dir.path()), "broken [[[");
            assert_eq!(app.settings.overlay.counter_size, 26);
        }

        #[test]
        fn game_rename_is_saved_and_shown() {
            let dir = TempDir::new("settings-rename");
            let (mut app, _owner) = test_app(dir.path());
            app.open_settings_hidden();
            app.settings_ui
                .as_ref()
                .unwrap()
                .queue(UiAction::Change(SettingsChange::GameName {
                    exe_path: "javaw.exe".into(),
                    name: Some("Minecraft".into()),
                }));
            app.drain_settings_actions();
            assert_eq!(
                app.settings.game_names.get("javaw.exe").map(String::as_str),
                Some("Minecraft")
            );
            app.close_settings_window();
            assert!(settings_file(dir.path()).contains("\"javaw.exe\" = \"Minecraft\""));
        }

        #[test]
        fn opening_twice_keeps_one_window() {
            let dir = TempDir::new("settings-twice");
            let (mut app, _owner) = test_app(dir.path());
            app.open_settings_hidden();
            let first = app.settings_ui.as_ref().unwrap().id();
            app.open_settings_hidden();
            assert_eq!(app.settings_ui.as_ref().unwrap().id(), first);
        }

        #[test]
        fn reload_refreshes_the_open_window() {
            let dir = TempDir::new("settings-reload");
            let (mut app, _owner) = test_app(dir.path());
            app.open_settings_hidden();
            let text = settings_file(dir.path()).replace("counter_size = 18", "counter_size = 33");
            std::fs::write(dir.path().join("settings.toml"), text).unwrap();
            app.reload_settings_file();
            assert_eq!(
                app.settings_ui
                    .as_ref()
                    .unwrap()
                    .shown()
                    .overlay
                    .counter_size,
                33
            );
        }

        fn at(x: i32, y: i32) -> isize {
            ((y as isize) << 16) | (x as isize & 0xFFFF)
        }

        /// `games.toml` with one played game, `C:\Games\Minecraft.exe`.
        fn write_one_game(dir: &Path) {
            use lite_bar_core::frame_stats::FrameHistogram;
            use lite_bar_core::history::{GameStore, SessionRecord};
            let t = |minute| LocalTime {
                year: 2026,
                month: 10,
                day: 27,
                hour: 10,
                minute,
                second: 0,
            };
            let mut hist = FrameHistogram::new();
            for _ in 0..600 {
                hist.add(0.1);
            }
            let mut store = GameStore::default();
            store.add_session(&SessionRecord {
                start: t(0),
                end: t(5),
                key: "minecraft".into(),
                name: "Minecraft".into(),
                exe_path: r"C:\Games\Minecraft.exe".into(),
                hist,
            });
            let stats = dir.join(STATS_DIR);
            std::fs::create_dir_all(&stats).unwrap();
            std::fs::write(stats.join("games.toml"), store.to_toml()).unwrap();
        }

        #[test]
        fn end_session_keeps_window_edits() {
            let dir = TempDir::new("settings-endsession");
            write_one_game(dir.path());
            let (mut app, _owner) = test_app(dir.path());
            type_uncommitted_game_name(&mut app, "Mine"); // typed, never committed
            let ui = app.settings_ui.as_ref().unwrap();
            ui.queue(counter(41)); // queued, never drained
            app.end_session();

            assert!(app.settings_ui.is_none(), "window closed");
            let (saved, _) = load_or_create_settings(dir.path());
            assert_eq!(saved.overlay.counter_size, 41);
            assert_eq!(
                saved
                    .game_names
                    .get(r"C:\Games\Minecraft.exe")
                    .map(String::as_str),
                Some("Mine")
            );
        }

        /// Opens the window on a played game and types `name` into its name field without
        /// committing it.
        fn type_uncommitted_game_name(app: &mut App, name: &str) {
            use windows::Win32::UI::WindowsAndMessaging::{
                WM_CHAR, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
            };
            app.open_settings_hidden();
            let ui = app.settings_ui.as_ref().unwrap();
            ui.paint();
            let s = ui.scale();
            let mid = at((450.0 * s) as i32, (400.0 * s) as i32);
            ui.send_msg(WM_MOUSEMOVE, 0, mid);
            for _ in 0..30 {
                ui.send_msg(WM_MOUSEWHEEL, ((-120i16) as u16 as usize) << 16, mid);
            }
            ui.paint();
            let (x, y) = ui
                .find_text_px("Minecraft", lite_bar_core::ui::theme::PLACEHOLDER)
                .expect("the game's empty name field");
            ui.send_msg(WM_MOUSEMOVE, 0, at(x, y));
            ui.send_msg(WM_LBUTTONDOWN, 1, at(x, y));
            ui.send_msg(WM_LBUTTONUP, 0, at(x, y));
            for c in name.chars() {
                ui.send_msg(WM_CHAR, c as usize, 0);
            }
        }

        #[test]
        fn restart_prep_saves_an_in_progress_rename_and_keeps_the_window() {
            let dir = TempDir::new("settings-restart");
            write_one_game(dir.path());
            let (mut app, _owner) = test_app(dir.path());
            type_uncommitted_game_name(&mut app, "Mine");
            app.save_settings_before_restart();
            assert!(
                app.settings_ui.as_ref().is_some_and(|ui| ui.is_open()),
                "window stays open (UAC may be declined)"
            );
            let (saved, _) = load_or_create_settings(dir.path());
            assert_eq!(
                saved
                    .game_names
                    .get(r"C:\Games\Minecraft.exe")
                    .map(String::as_str),
                Some("Mine")
            );
        }

        #[test]
        fn failed_save_is_retried_by_the_next_save() {
            let dir = TempDir::new("settings-retry");
            std::fs::write(dir.path().join("settings.toml"), user_settings_text()).unwrap();
            let (mut app, _owner) = test_app(dir.path());
            app.open_settings_hidden();
            std::fs::write(dir.path().join("settings.toml"), "broken [[[").unwrap();
            let ui = app.settings_ui.as_ref().unwrap();
            ui.queue(UiAction::Change(SettingsChange::BackgroundOpacity(70)));
            app.drain_settings_actions();
            app.on_save_timer(Instant::now() + Duration::from_millis(400));
            assert_eq!(settings_file(dir.path()), "broken [[[");

            // The user fixes the file; the next change saves both.
            std::fs::write(dir.path().join("settings.toml"), user_settings_text()).unwrap();
            app.settings_ui.as_ref().unwrap().queue(counter(22));
            app.drain_settings_actions();
            app.on_save_timer(Instant::now() + Duration::from_millis(400));
            let text = settings_file(dir.path());
            assert!(text.contains("background_opacity = 70\n"), "{text}");
            assert!(text.contains("counter_size = 22 # my size"), "{text}");
        }

        /// A message-only window; destroyed on drop.
        struct MsgWindow(HWND);

        impl Drop for MsgWindow {
            fn drop(&mut self) {
                unsafe {
                    let _ = DestroyWindow(self.0);
                }
            }
        }

        fn msg_window() -> MsgWindow {
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!(""),
                    WS_OVERLAPPED,
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    None,
                    None,
                )
            }
            .unwrap();
            MsgWindow(hwnd)
        }

        #[test]
        fn restore_with_a_taken_default_hotkey_shows_a_status() {
            let default = Settings::default().hotkeys.toggle_overlay;
            // Held by this window, or already by another app (e.g. a running Lite Bar).
            let holder = msg_window();
            let _ = hotkeys::register(holder.0, HotkeyId::ToggleOverlay, default);
            let mine: lite_bar_core::hotkey::Hotkey = "Ctrl+Alt+Shift+F23".parse().unwrap();

            let dir = TempDir::new("settings-restore-taken");
            let text = user_settings_text().replace(
                "toggle_overlay = \"Ctrl+Alt+Shift+L\"",
                "toggle_overlay = \"Ctrl+Alt+Shift+F23\"",
            );
            std::fs::write(dir.path().join("settings.toml"), text).unwrap();
            let (mut app, _owner) = test_app(dir.path());
            app.register_hotkey();
            app.open_settings_hidden();
            app.settings_ui
                .as_ref()
                .unwrap()
                .queue(UiAction::Change(SettingsChange::RestoreDefaults));
            app.drain_settings_actions();

            assert_eq!(app.settings.hotkeys.toggle_overlay, mine);
            let ui = app.settings_ui.as_ref().unwrap();
            assert_eq!(
                ui.status().as_deref(),
                Some(crate::settings_window::HOTKEY_TAKEN)
            );
            assert_eq!(ui.pending_reject(), None, "no capture-field error");
            app.close_settings_window();
            assert!(settings_file(dir.path()).contains("toggle_overlay = \"Ctrl+Alt+Shift+F23\""));
            hotkeys::unregister(holder.0, HotkeyId::ToggleOverlay);
        }

        #[test]
        fn ui_frame_panics_keep_the_trace() {
            assert!(panic_stops_trace(false, Some("main")));
            assert!(panic_stops_trace(false, None));
            assert!(!panic_stops_trace(true, Some("main")));
        }

        #[test]
        fn update_worker_panics_keep_the_trace() {
            // Caught by the worker, which posts a failed check instead.
            assert!(!panic_stops_trace(
                false,
                Some(crate::updater::WORKER_THREAD)
            ));
        }

        #[test]
        fn panic_message_formats_string_payload() {
            let payload: Box<dyn Any + Send> = Box::new(String::from("bad index 7"));
            assert_eq!(
                panic_message(&*payload, Some(("a.rs", 1, 2))),
                "panic: bad index 7 at a.rs:1:2"
            );
        }

        #[test]
        fn panic_message_handles_unknown_payload_and_location() {
            let payload: Box<dyn Any + Send> = Box::new(42_u32);
            assert_eq!(
                panic_message(&*payload, None),
                "panic: <non-string payload> at <unknown location>"
            );
        }
        use windows::Win32::UI::WindowsAndMessaging::{
            HWND_MESSAGE, WM_CLOSE, WM_DESTROY, WM_ENDSESSION,
        };

        #[test]
        fn close_destroy_and_endsession_map_to_commands() {
            assert_eq!(window_command(WM_CLOSE, 0), Some(Command::Quit));
            assert_eq!(window_command(WM_DESTROY, 0), Some(Command::PostQuit));
            assert_eq!(window_command(WM_ENDSESSION, 1), Some(Command::StopCapture));
        }

        fn tracked(pid: u32) -> SessionBook {
            let at = LocalTime {
                year: 2026,
                month: 10,
                day: 26,
                hour: 10,
                minute: 0,
                second: 0,
            };
            let mut book = SessionBook::new();
            book.track(OpenSession {
                pid,
                created: 1,
                exe_path: r"C:\Games\a.exe".into(),
                window_title: None,
                start: at,
                last_alive: at,
            });
            book
        }

        fn open_gate() -> QueryGate {
            QueryGate::default()
        }

        #[test]
        fn foreground_with_stats_and_no_session_is_tracked() {
            let none = SessionBook::new();
            assert_eq!(pid_to_track(Some(7), true, &none, &open_gate()), Some(7));
            assert_eq!(
                pid_to_track(Some(8), true, &tracked(7), &open_gate()),
                Some(8)
            );
        }

        #[test]
        fn tracked_pid_or_one_without_stats_is_not_tracked_again() {
            let none = SessionBook::new();
            assert_eq!(pid_to_track(Some(7), true, &tracked(7), &open_gate()), None);
            assert_eq!(pid_to_track(Some(7), false, &none, &open_gate()), None);
            assert_eq!(pid_to_track(None, true, &none, &open_gate()), None);
        }

        #[test]
        fn refused_pid_is_not_queried_again_while_focused() {
            let mut gate = QueryGate::default();
            gate.on_focus(Some(7));
            gate.refused(7);
            gate.on_focus(Some(7));
            let none = SessionBook::new();
            assert_eq!(pid_to_track(Some(7), true, &none, &gate), None);
        }

        fn info(exe_path: &str) -> ProcessInfo {
            ProcessInfo {
                exe_path: exe_path.into(),
                created: 5,
            }
        }

        #[test]
        fn game_gets_a_session_with_the_title_from_its_tick() {
            let at = LocalTime {
                year: 2026,
                month: 10,
                day: 26,
                hour: 10,
                minute: 0,
                second: 0,
            };
            let s = new_session(7, info(r"C:\Games\a.exe"), || Some("A - Menu".into()), at)
                .expect("a game is tracked");
            assert_eq!(s.pid, 7);
            assert_eq!(s.created, 5);
            assert_eq!(s.window_title.as_deref(), Some("A - Menu"));
            assert_eq!((s.start, s.last_alive), (at, at));
        }

        #[test]
        fn known_non_game_gets_no_session() {
            let at = LocalTime {
                year: 2026,
                month: 10,
                day: 26,
                hour: 10,
                minute: 0,
                second: 0,
            };
            let chrome = r"C:\Program Files\Google\Chrome\Application\chrome.exe";
            let title = || -> Option<String> { panic!("a non-game's title is never read") };
            assert_eq!(new_session(7, info(chrome), title, at), None);
        }

        #[test]
        fn exit_check_runs_every_fourth_tick() {
            let due: Vec<u32> = (1..=12).filter(|t| session_check_due(*t, false)).collect();
            assert_eq!(due, vec![4, 8, 12]);
            assert_eq!(ALIVE_CHECK_EVERY_TICKS * TIMER_MS, 1000);
        }

        #[test]
        fn exit_check_runs_on_the_first_tick_after_show() {
            assert!(session_check_due(5, true));
        }

        #[test]
        fn game_names_changed_ignores_other_settings() {
            let old = Settings::default();
            let mut new = Settings::default();
            new.overlay.visible = !old.overlay.visible;
            assert!(!game_names_changed(&old, &new));
            new.game_names
                .insert("javaw.exe".into(), "Minecraft".into());
            assert!(game_names_changed(&old, &new));
        }

        #[test]
        fn cancelled_endsession_and_other_messages_have_no_command() {
            assert_eq!(window_command(WM_ENDSESSION, 0), None);
            assert_eq!(window_command(WM_TIMER, 1), None);
        }

        #[test]
        fn taskbar_created_can_be_allowed_through_uipi() {
            // SAFETY: plain FFI; the predefined STATIC class needs no registration.
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!(""),
                    WS_OVERLAPPED,
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    None,
                    None,
                )
            }
            .unwrap();
            let id = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
            assert!(allow_through_uipi(hwnd, id).is_ok());
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }

        /// Whether another window could register `hk` right now (it is released again).
        fn hotkey_is_free(hk: Hotkey) -> bool {
            let probe = create_owner_window().unwrap();
            let free = matches!(
                hotkeys::register(probe.0, HotkeyId::ToggleOverlay, hk),
                Registration::Ok
            );
            hotkeys::unregister(probe.0, HotkeyId::ToggleOverlay);
            free
        }

        #[test]
        fn hotkey_capture_releases_the_toggle_hotkey_until_it_ends() {
            let dir = TempDir::new("settings-capture");
            let (mut app, _owner) = test_app(dir.path());
            let hk: Hotkey = "Ctrl+Alt+Shift+F22".parse().unwrap();
            app.settings.hotkeys.toggle_overlay = hk;
            app.register_hotkey();
            assert!(app.hotkey_ok && !hotkey_is_free(hk));
            app.open_settings_hidden();

            let ends = [UiAction::CancelHotkeyCapture, UiAction::ProposeHotkey(hk)];
            for end in ends {
                app.apply_ui_action(UiAction::StartHotkeyCapture);
                assert!(hotkey_is_free(hk), "released while capturing");
                app.apply_ui_action(end.clone());
                assert!(!hotkey_is_free(hk), "held again after {end:?}");
                assert!(app.hotkey_ok);
            }
            app.apply_ui_action(UiAction::StartHotkeyCapture);
            app.close_settings_window();
            assert!(!hotkey_is_free(hk), "held again after close");
            hotkeys::unregister(app.hwnd, HotkeyId::ToggleOverlay);
        }

        #[test]
        fn hotkey_capture_ends_when_the_window_is_closed_with_x() {
            let dir = TempDir::new("settings-capture-x");
            let (mut app, _owner) = test_app(dir.path());
            let hk: Hotkey = "Ctrl+Alt+Shift+F21".parse().unwrap();
            app.settings.hotkeys.toggle_overlay = hk;
            app.register_hotkey();
            app.open_settings_hidden();
            app.apply_ui_action(UiAction::StartHotkeyCapture);
            app.settings_ui.as_ref().unwrap().send_close();
            app.drain_settings_actions();
            assert!(!hotkey_is_free(hk));
            hotkeys::unregister(app.hwnd, HotkeyId::ToggleOverlay);
        }

        #[test]
        fn capture_takes_a_key_up_whose_key_down_never_arrived() {
            use windows::Win32::UI::WindowsAndMessaging::{
                WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
            };
            const NEEDS_MODIFIER: &str =
                "Add Ctrl, Alt or Win (Shift alone only works with F-keys)";
            let error = lite_bar_core::ui::theme::ERROR;
            let dir = TempDir::new("settings-keyup");
            let (mut app, _owner) = test_app(dir.path());
            app.open_settings_hidden();
            let ui = app.settings_ui.as_ref().unwrap();
            ui.paint();
            let s = ui.scale();
            // F5 alone (no modifier) is refused with a message, which shows it was taken.
            ui.send_msg(WM_KEYUP, 0x74, 0);
            ui.paint();
            assert!(
                ui.find_text_px(NEEDS_MODIFIER, error).is_none(),
                "not capturing"
            );
            let mid = at((450.0 * s) as i32, (400.0 * s) as i32);
            ui.send_msg(WM_MOUSEMOVE, 0, mid);
            let cap = |ui: &SettingsUi| ui.find_text_px("Ctrl", lite_bar_core::ui::theme::TEXT);
            for _ in 0..40 {
                if cap(ui).is_some_and(|(_, y)| y > (200.0 * s) as i32 && y < (600.0 * s) as i32) {
                    break;
                }
                ui.send_msg(WM_MOUSEWHEEL, ((-120i16) as u16 as usize) << 16, mid);
                ui.paint();
            }
            let (x, y) = cap(ui).expect("the hotkey field");
            ui.send_msg(WM_MOUSEMOVE, 0, at(x, y));
            ui.send_msg(WM_LBUTTONDOWN, 1, at(x, y));
            ui.send_msg(WM_LBUTTONUP, 0, at(x, y));
            ui.paint();
            ui.send_msg(WM_KEYUP, 0x74, 0);
            ui.paint();
            assert!(ui.find_text_px(NEEDS_MODIFIER, error).is_some());
            app.close_settings_window();
        }

        #[test]
        fn failed_save_shows_status_and_success_clears_it() {
            use crate::app::settings::{SAVE_FAILED_FILE, SAVE_FAILED_WRITE};
            let red = lite_bar_core::ui::Rgba(0xFF, 0x8A, 0x80, 255);
            let first_line = |m: &str| m.lines().next().unwrap().to_string();
            let dir = TempDir::new("settings-save-status");
            let path = dir.path().join("settings.toml");
            std::fs::write(&path, user_settings_text()).unwrap();
            let (mut app, _owner) = test_app(dir.path());
            app.open_settings_hidden();
            let save = |app: &mut App, n: u16| {
                app.settings_ui.as_ref().unwrap().queue(counter(n));
                app.drain_settings_actions();
                app.on_save_timer(Instant::now() + Duration::from_millis(400));
            };

            std::fs::write(&path, "broken [[[").unwrap();
            save(&mut app, 21);
            let ui = app.settings_ui.as_ref().unwrap();
            assert_eq!(ui.save_error().as_deref(), Some(SAVE_FAILED_FILE));
            ui.paint();
            assert!(ui
                .find_text_px(&first_line(SAVE_FAILED_FILE), red)
                .is_some());

            // Fixed by hand: the next save works and clears it.
            std::fs::write(&path, user_settings_text()).unwrap();
            save(&mut app, 22);
            assert_eq!(app.settings_ui.as_ref().unwrap().save_error(), None);

            // Broken again, then fixed and reloaded.
            std::fs::write(&path, "broken [[[").unwrap();
            save(&mut app, 23);
            assert!(app.settings_ui.as_ref().unwrap().save_error().is_some());
            std::fs::write(&path, user_settings_text()).unwrap();
            app.reload_settings_file();
            assert_eq!(app.settings_ui.as_ref().unwrap().save_error(), None);
            assert!(settings_file(dir.path()).contains("counter_size = 23 # my size"));

            // A write error has its own message.
            std::fs::create_dir(dir.path().join("settings.toml.tmp")).unwrap();
            save(&mut app, 24);
            assert_eq!(
                app.settings_ui.as_ref().unwrap().save_error().as_deref(),
                Some(SAVE_FAILED_WRITE)
            );
            std::fs::remove_dir(dir.path().join("settings.toml.tmp")).unwrap();
            app.close_settings_window();
            assert!(settings_file(dir.path()).contains("counter_size = 24 # my size"));
        }

        #[test]
        fn a_save_error_shows_when_the_window_opens_again() {
            use crate::app::settings::SAVE_FAILED_FILE;
            let dir = TempDir::new("settings-save-reopen");
            let path = dir.path().join("settings.toml");
            std::fs::write(&path, user_settings_text()).unwrap();
            let (mut app, _owner) = test_app(dir.path());
            app.open_settings_hidden();
            app.settings_ui.as_ref().unwrap().queue(counter(30));
            app.drain_settings_actions();
            std::fs::write(&path, "broken [[[").unwrap();
            app.close_settings_window();
            app.open_settings_hidden();
            assert_eq!(
                app.settings_ui.as_ref().unwrap().save_error().as_deref(),
                Some(SAVE_FAILED_FILE)
            );
        }

        #[test]
        fn capture_end_after_a_re_register_is_not_reported_taken() {
            let dir = TempDir::new("settings-capture-rereg");
            let (mut app, _owner) = test_app(dir.path());
            let hk: Hotkey = "Ctrl+Alt+Shift+F19".parse().unwrap();
            app.settings.hotkeys.toggle_overlay = hk;
            app.register_hotkey();
            app.open_settings_hidden();
            app.apply_ui_action(UiAction::StartHotkeyCapture);
            // E.g. a declined restart-as-admin registers the hotkey again mid-capture.
            app.register_hotkey();
            app.apply_ui_action(UiAction::CancelHotkeyCapture);
            assert!(app.hotkey_ok, "no false \"taken\"");
            assert!(!hotkey_is_free(hk));
            assert_eq!(app.settings_ui.as_ref().unwrap().status(), None);
            hotkeys::unregister(app.hwnd, HotkeyId::ToggleOverlay);
        }

        #[test]
        fn closing_for_exit_does_not_take_the_hotkey_back() {
            let dir = TempDir::new("settings-capture-exit");
            let (mut app, _owner) = test_app(dir.path());
            let hk: Hotkey = "Ctrl+Alt+Shift+F18".parse().unwrap();
            app.settings.hotkeys.toggle_overlay = hk;
            app.register_hotkey();
            app.open_settings_hidden();
            app.apply_ui_action(UiAction::StartHotkeyCapture);
            app.close_settings_window_for_exit();
            assert!(app.settings_ui.is_none());
            assert!(hotkey_is_free(hk), "the new instance can register it");
        }

        fn release(version: &str) -> ReleaseInfo {
            ReleaseInfo {
                version: version.parse().unwrap(),
                page_url: format!("https://github.com/paymonr/lite-bar/releases/tag/v{version}"),
                exe_url: "e".into(),
                sha_url: "s".into(),
            }
        }

        /// Waits up to `timeout` for a `WM_UPDATE_RESULT` posted to `hwnd` and removes it.
        fn wait_update_msg(hwnd: HWND, timeout: Duration) -> Option<MSG> {
            use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, PM_REMOVE};
            let end = Instant::now() + timeout;
            let mut msg = MSG::default();
            while Instant::now() < end {
                let got = unsafe {
                    PeekMessageW(
                        &mut msg,
                        Some(hwnd),
                        WM_UPDATE_RESULT,
                        WM_UPDATE_RESULT,
                        PM_REMOVE,
                    )
                };
                if got.as_bool() {
                    return Some(msg);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            None
        }

        fn wait_update_result(hwnd: HWND, timeout: Duration) -> Option<CheckResult> {
            wait_update_msg(hwnd, timeout).map(|m| unsafe { take_result(m.lParam) })
        }

        #[test]
        fn fake_update_result_message_moves_the_state() {
            let dir = TempDir::new("update-msg");
            let (app, _owner) = test_app(dir.path());
            let hwnd = app.hwnd;
            APP.with(|slot| *slot.borrow_mut() = Some(app));
            assert!(post_result(hwnd, Ok(release("99.0.0"))));
            let msg = wait_update_msg(hwnd, Duration::from_secs(1)).unwrap();
            unsafe { window_proc(hwnd, msg.message, msg.wParam, msg.lParam) };
            let state = with_app(|app| app.update.clone()).unwrap();
            assert_eq!(state, UpdateState::Available(release("99.0.0")));

            // Arriving while the app is busy, it waits for the next `with_app`.
            assert!(post_result(hwnd, Ok(release("0.0.1"))));
            let msg = wait_update_msg(hwnd, Duration::from_secs(1)).unwrap();
            APP.with(|slot| {
                let _busy = slot.borrow_mut();
                unsafe { window_proc(hwnd, msg.message, msg.wParam, msg.lParam) };
            });
            let state = with_app(|app| app.update.clone()).unwrap();
            assert_eq!(state, UpdateState::UpToDate);
            APP.with(|slot| slot.borrow_mut().take());
        }

        #[test]
        fn update_state_shows_in_the_tray_and_the_window() {
            let dir = TempDir::new("update-shown");
            let (mut app, _owner) = test_app(dir.path());
            app.open_settings_hidden();
            app.on_update_result(Ok(release("99.0.0")));
            let avail = UpdateState::Available(release("99.0.0"));
            assert_eq!(app.update, avail);
            assert_eq!(
                app.menu_state().update_available,
                Some("99.0.0".parse().unwrap())
            );
            assert_eq!(app.settings_ui.as_ref().unwrap().update_state(), avail);
            // A failed check keeps the note.
            app.on_update_result(Err("offline".into()));
            assert_eq!(app.update, avail);
            app.on_update_result(Ok(release("0.0.1")));
            assert_eq!(app.update, UpdateState::UpToDate);
            assert_eq!(app.menu_state().update_available, None);
            assert_eq!(
                app.settings_ui.as_ref().unwrap().update_state(),
                UpdateState::UpToDate
            );
            // A window opened later shows the state too.
            app.close_settings_window();
            app.on_update_result(Ok(release("99.0.0")));
            app.open_settings_hidden();
            assert_eq!(app.settings_ui.as_ref().unwrap().update_state(), avail);
        }

        #[test]
        fn check_now_runs_with_checks_off_and_shows_checking() {
            let dir = TempDir::new("update-check-now");
            let (mut app, _owner) = test_app(dir.path());
            app.apply_settings_change(SettingsChange::CheckUpdates(false));
            app.apply_ui_action(UiAction::CheckForUpdates);
            assert_eq!(app.update, UpdateState::Checking);
            assert!(app.update_schedule.is_running());
            // A second click starts no second check.
            app.apply_ui_action(UiAction::CheckForUpdates);
            // The worker asks a closed local port, so it fails.
            let r = wait_update_result(app.hwnd, Duration::from_secs(10)).expect("a result");
            assert!(r.is_err());
            assert!(wait_update_result(app.hwnd, Duration::from_secs(3)).is_none());
            app.on_update_result(r);
            assert_eq!(app.update, UpdateState::CheckFailed);
            assert!(!app.update_schedule.is_running());
            assert_eq!(app.update_schedule.next_due(), None, "still off");
        }

        #[test]
        fn background_check_keeps_the_shown_state() {
            let dir = TempDir::new("update-background");
            let (mut app, _owner) = test_app(dir.path());
            let avail = UpdateState::Available(release("99.0.0"));
            app.update = avail.clone();
            app.on_update_timer(Duration::from_secs(29));
            assert!(!app.update_schedule.is_running(), "not due yet");
            app.on_update_timer(Duration::from_secs(31));
            assert!(app.update_schedule.is_running());
            assert_eq!(app.update, avail, "no Checking for a background check");
            let r = wait_update_result(app.hwnd, Duration::from_secs(10)).expect("a result");
            app.on_update_result(r);
            assert_eq!(app.update, avail);
            assert!(
                app.update_schedule.next_due().is_some(),
                "next one in a day"
            );
        }

        #[test]
        fn a_failed_check_now_keeps_an_available_update() {
            let dir = TempDir::new("update-keep");
            let (mut app, _owner) = test_app(dir.path());
            app.open_settings_hidden();
            app.on_update_result(Ok(release("99.0.0")));
            let avail = UpdateState::Available(release("99.0.0"));
            app.apply_ui_action(UiAction::CheckForUpdates);
            assert!(app.update_schedule.is_running());
            assert_eq!(app.update, avail, "Update now stays while it checks");
            let r = wait_update_result(app.hwnd, Duration::from_secs(10)).expect("a result");
            assert!(r.is_err());
            app.on_update_result(r);
            assert_eq!(app.update, avail);
            assert!(app.menu_state().update_available.is_some());
            assert_eq!(app.settings_ui.as_ref().unwrap().update_state(), avail);
        }

        #[test]
        fn a_panicking_check_does_not_stop_later_checks() {
            let dir = TempDir::new("update-panic");
            let (mut app, _owner) = test_app(dir.path());
            app.fetch = |_| panic!("worker bug");
            app.on_update_timer(Duration::from_secs(31));
            assert!(app.update_schedule.is_running());
            let r = wait_update_result(app.hwnd, Duration::from_secs(10)).expect("a result");
            app.on_update_result(r);
            assert!(!app.update_schedule.is_running());
            assert!(
                app.update_schedule.next_due().is_some(),
                "the next check is scheduled"
            );
            assert_eq!(app.update, UpdateState::CheckFailed);
        }

        #[test]
        fn toggling_checks_schedules_and_cancels() {
            let dir = TempDir::new("update-toggle");
            let (mut app, _owner) = test_app(dir.path());
            assert!(app.settings.updates.check);
            assert!(app.update_schedule.next_due().is_some());
            app.apply_settings_change(SettingsChange::CheckUpdates(false));
            assert_eq!(app.update_schedule.next_due(), None);
            app.apply_settings_change(SettingsChange::CheckUpdates(true));
            let due = app.update_schedule.next_due().expect("scheduled");
            let now = app.clock.elapsed();
            assert!(due > now + Duration::from_secs(29) && due <= now + Duration::from_secs(30));
        }

        /// Waits up to `timeout` for a `WM_INSTALL_RESULT` posted to `hwnd` and takes it.
        fn wait_install_result(hwnd: HWND, timeout: Duration) -> Option<InstallResult> {
            use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, PM_REMOVE};
            let end = Instant::now() + timeout;
            let mut msg = MSG::default();
            while Instant::now() < end {
                let got = unsafe {
                    PeekMessageW(
                        &mut msg,
                        Some(hwnd),
                        WM_INSTALL_RESULT,
                        WM_INSTALL_RESULT,
                        PM_REMOVE,
                    )
                };
                if got.as_bool() {
                    return Some(unsafe { crate::installer::take_result(msg.lParam) });
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            None
        }

        const OLD_EXE: &[u8] = b"MZ running lite bar";
        const NEW_EXE: &[u8] = b"MZ lite bar v99";

        /// The app's exe is `lite-bar.exe` in the temp `dir`, never the real one.
        fn with_temp_exe(app: &mut App, dir: &Path) -> PathBuf {
            let exe = dir.join("lite-bar.exe");
            std::fs::write(&exe, OLD_EXE).unwrap();
            app.exe_path = Some(exe.clone());
            exe
        }

        /// The real download pipeline, minus the GitHub-only URL check (the stub is local).
        fn download_from_stub(r: &ReleaseInfo, exe: &Path) -> InstallResult {
            crate::installer::fetch_and_stage(r, exe, |_| true)
        }

        #[test]
        fn install_update_shows_downloading_then_failed_on_a_bad_checksum() {
            use crate::test_util::{http_ok, serve_paths};
            let dir = TempDir::new("install-app-bad-sha");
            let (mut app, _owner) = test_app(dir.path());
            let exe = with_temp_exe(&mut app, dir.path());
            app.download = download_from_stub;
            let wrong = format!("{}  lite-bar.exe\n", "00".repeat(32));
            let base = serve_paths(vec![
                ("/sha", http_ok(wrong.as_bytes())),
                ("/exe", http_ok(NEW_EXE)),
            ]);
            let r = ReleaseInfo {
                exe_url: format!("{base}/exe"),
                sha_url: format!("{base}/sha"),
                ..release("99.0.0")
            };
            app.on_update_result(Ok(r.clone()));
            app.open_settings_hidden();

            app.install_update();
            assert_eq!(app.update, UpdateState::Downloading(r.clone()));
            assert_eq!(
                app.settings_ui.as_ref().unwrap().update_state(),
                UpdateState::Downloading(r.clone())
            );
            app.install_update(); // a second click while downloading is ignored
            let result = wait_install_result(app.hwnd, Duration::from_secs(10)).expect("result");
            assert!(
                wait_install_result(app.hwnd, Duration::from_secs(2)).is_none(),
                "one download"
            );
            assert!(!app.on_install_result(result), "keeps running");
            let UpdateState::Failed { why, .. } = &app.update else {
                panic!("{:?}", app.update);
            };
            assert!(why.contains("checksum"), "{why}");
            assert_eq!(app.settings_ui.as_ref().unwrap().update_state(), app.update);
            assert_eq!(std::fs::read(&exe).unwrap(), OLD_EXE);
            assert!(!crate::installer::new_path(&exe).exists());
            // The release page stays reachable for a manual download.
            assert_eq!(app.release_page(), Some(r.page_url.as_str()));

            // A corrupted download may be a one-off: Try again downloads the same release.
            assert!(matches!(
                app.update,
                UpdateState::Failed { retry: true, .. }
            ));
            app.install_update();
            assert_eq!(app.update, UpdateState::Downloading(r.clone()));
            let result = wait_install_result(app.hwnd, Duration::from_secs(10)).expect("result");
            app.on_install_result(result);
            assert!(matches!(
                app.update,
                UpdateState::Failed { retry: true, .. }
            ));
        }

        #[test]
        fn an_unwritable_folder_offers_no_retry() {
            let dir = TempDir::new("install-app-unwritable");
            let (mut app, _owner) = test_app(dir.path());
            with_temp_exe(&mut app, dir.path());
            app.update = UpdateState::Downloading(release("99.0.0"));
            app.installing = Some(release("99.0.0"));
            app.on_install_result(Err(crate::installer::InstallError::Unwritable));
            let failed = UpdateState::Failed {
                why: crate::installer::UNWRITABLE.into(),
                retry: false,
            };
            assert_eq!(app.update, failed);
            app.download = |_, _| panic!("must not download");
            app.install_update();
            assert_eq!(app.update, failed);
            assert!(app.release_page().is_some());
        }

        #[test]
        fn without_its_exe_path_the_release_page_still_opens() {
            let dir = TempDir::new("install-app-no-exe");
            let (mut app, _owner) = test_app(dir.path());
            app.download = |_, _| panic!("must not download");
            app.update = UpdateState::Available(release("99.0.0"));
            app.install_update();
            assert!(
                matches!(app.update, UpdateState::Failed { retry: false, .. }),
                "{:?}",
                app.update
            );
            assert_eq!(
                app.release_page(),
                Some(release("99.0.0").page_url.as_str())
            );
        }

        #[test]
        fn an_installed_version_is_not_offered_again() {
            let dir = TempDir::new("install-app-installed");
            let (mut app, _owner) = test_app(dir.path());
            app.installed_version = Some("99.0.0".parse().unwrap());
            app.on_update_result(Ok(release("99.0.0")));
            assert_eq!(app.update, UpdateState::UpToDate);
            app.on_update_result(Ok(release("99.0.1")));
            assert_eq!(app.update, UpdateState::Available(release("99.0.1")));
        }

        #[test]
        fn install_needs_an_available_newer_release() {
            let dir = TempDir::new("install-app-guard");
            let (mut app, _owner) = test_app(dir.path());
            with_temp_exe(&mut app, dir.path());
            app.download = |_, _| panic!("must not download");
            app.install_update();
            assert_eq!(app.update, UpdateState::Idle);
            // Not newer than this build (a stale state): ignored too.
            let stale = UpdateState::Available(release("0.0.1"));
            app.update = stale.clone();
            app.install_update();
            assert_eq!(app.update, stale);
            assert!(wait_install_result(app.hwnd, Duration::from_millis(300)).is_none());
        }

        thread_local! {
            static RELAUNCHED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
        }

        fn fake_relaunch(exe: &Path) -> Result<(), String> {
            RELAUNCHED.with(|r| r.borrow_mut().push(exe.to_path_buf()));
            Ok(())
        }

        /// The app mid-download of v99, with the verified exe staged as `.new`.
        fn staged_app(dir: &Path) -> (App, OwnerWindow, PathBuf, PathBuf) {
            let (mut app, owner) = test_app(dir);
            let exe = with_temp_exe(&mut app, dir);
            let new = crate::installer::stage_update(
                &exe,
                NEW_EXE,
                &lite_bar_core::update::sha256(NEW_EXE),
            )
            .unwrap();
            app.update = UpdateState::Downloading(release("99.0.0"));
            app.installing = Some(release("99.0.0"));
            (app, owner, exe, new)
        }

        #[test]
        fn verified_download_is_swapped_in_and_relaunched() {
            let dir = TempDir::new("install-app-swap");
            let (mut app, _owner, exe, new) = staged_app(dir.path());
            app.relaunch = fake_relaunch;
            app.open_settings_hidden();
            app.settings_ui.as_ref().unwrap().queue(counter(37));
            assert!(app.on_install_result(Ok(new)), "quits");
            assert_eq!(std::fs::read(&exe).unwrap(), NEW_EXE);
            assert_eq!(
                std::fs::read(crate::installer::old_path(&exe)).unwrap(),
                OLD_EXE
            );
            assert_eq!(RELAUNCHED.with(|r| r.borrow().clone()), vec![exe]);
            // The Quit path ran first: the window's edits are saved.
            assert!(app.settings_ui.is_none());
            assert!(settings_file(dir.path()).contains("counter_size = 37"));
        }

        #[test]
        fn relaunch_failure_keeps_running_and_asks_for_a_restart() {
            let dir = TempDir::new("install-app-relaunch-fails");
            let (mut app, _owner, exe, new) = staged_app(dir.path());
            app.relaunch = |_| Err("no shell".into());
            assert!(!app.on_install_result(Ok(new)), "keeps running");
            assert_eq!(app.update, UpdateState::RestartRequired);
            assert_eq!(std::fs::read(&exe).unwrap(), NEW_EXE, "installed");
            assert_eq!(app.installed_version, Some("99.0.0".parse().unwrap()));

            // The installed release is never offered again, by a check or Check now.
            app.on_update_result(Ok(release("99.0.0")));
            assert_eq!(app.update, UpdateState::RestartRequired);
            app.check_for_updates_now();
            assert_eq!(app.update, UpdateState::RestartRequired);
            assert_eq!(app.menu_state().update_available, None);

            // Restart now tries again: failing, it still waits for a restart...
            assert!(!app.restart_to_update());
            assert_eq!(app.update, UpdateState::RestartRequired);
            // ...and once the new exe starts, this one quits.
            app.relaunch = fake_relaunch;
            assert!(app.restart_to_update());
            assert_eq!(RELAUNCHED.with(|r| r.borrow().clone()), vec![exe]);
        }

        #[test]
        fn restart_now_does_nothing_without_an_installed_update() {
            let dir = TempDir::new("install-app-restart-idle");
            let (mut app, _owner) = test_app(dir.path());
            with_temp_exe(&mut app, dir.path());
            app.relaunch = |_| panic!("must not relaunch");
            assert!(!app.restart_to_update());
            assert_eq!(app.update, UpdateState::Idle);
        }

        #[test]
        fn failed_swap_keeps_the_running_exe() {
            let dir = TempDir::new("install-app-swap-fails");
            let (mut app, _owner, exe, new) = staged_app(dir.path());
            app.relaunch = |_| panic!("must not relaunch");
            std::fs::remove_file(&new).unwrap(); // the second rename has nothing to move
            assert!(!app.on_install_result(Ok(new)));
            assert!(
                matches!(app.update, UpdateState::Failed { .. }),
                "{:?}",
                app.update
            );
            assert_eq!(std::fs::read(&exe).unwrap(), OLD_EXE);
        }

        #[test]
        fn after_an_update_the_guard_is_taken_once_the_old_instance_lets_go() {
            let mut tries = 0;
            let got = guard_after_update(|| {
                tries += 1;
                Ok((tries == 3).then_some("guard"))
            });
            assert_eq!(got.ok(), Some(Some("guard")));
            assert_eq!(tries, 3);
            // An unexpected error stops the wait (the caller runs without a guard).
            let mut tries = 0;
            let got: windows::core::Result<Option<()>> = guard_after_update(|| {
                tries += 1;
                Err(windows::core::Error::empty())
            });
            assert!(got.is_err());
            assert_eq!(tries, 1);
        }

        #[test]
        fn install_threads_panics_keep_the_trace() {
            for t in [
                crate::installer::WORKER_THREAD,
                crate::installer::CLEANUP_THREAD,
            ] {
                assert!(!panic_stops_trace(false, Some(t)), "{t}");
            }
        }

        #[test]
        fn hotkey_taken_during_capture_is_reported_in_the_window() {
            let dir = TempDir::new("settings-capture-taken");
            let (mut app, _owner) = test_app(dir.path());
            let hk: Hotkey = "Ctrl+Alt+Shift+F20".parse().unwrap();
            app.settings.hotkeys.toggle_overlay = hk;
            app.register_hotkey();
            app.open_settings_hidden();
            app.apply_ui_action(UiAction::StartHotkeyCapture);
            let thief = create_owner_window().unwrap();
            assert!(matches!(
                hotkeys::register(thief.0, HotkeyId::ToggleOverlay, hk),
                Registration::Ok
            ));
            app.apply_ui_action(UiAction::CancelHotkeyCapture);
            assert!(!app.hotkey_ok);
            assert_eq!(
                app.settings_ui.as_ref().unwrap().status().as_deref(),
                Some("Couldn't restore Ctrl+Alt+Shift+F20 — another app took it")
            );
            // Proposing the same keys again reports it too, not "kept".
            app.apply_ui_action(UiAction::StartHotkeyCapture);
            app.apply_ui_action(UiAction::ProposeHotkey(hk));
            assert_eq!(
                app.settings_ui.as_ref().unwrap().status().as_deref(),
                Some("Couldn't restore Ctrl+Alt+Shift+F20 — another app took it")
            );
            hotkeys::unregister(thief.0, HotkeyId::ToggleOverlay);
        }

        /// Waits up to `timeout` for a `WM_STARTUP_RESULT` posted to `hwnd` and removes it.
        fn wait_startup_msg(hwnd: HWND, timeout: Duration) -> Option<MSG> {
            use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, PM_REMOVE};
            let end = Instant::now() + timeout;
            let mut msg = MSG::default();
            while Instant::now() < end {
                let got = unsafe {
                    PeekMessageW(
                        &mut msg,
                        Some(hwnd),
                        WM_STARTUP_RESULT,
                        WM_STARTUP_RESULT,
                        PM_REMOVE,
                    )
                };
                if got.as_bool() {
                    return Some(msg);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            None
        }

        /// Applies the next startup result posted to the app's window.
        fn apply_startup_result(app: &mut App) {
            let msg = wait_startup_msg(app.hwnd, Duration::from_secs(5)).expect("a result");
            app.on_startup_result(unsafe { crate::startup::take_result(msg.lParam) });
        }

        fn shown_startup(app: &App) -> StartupState {
            app.settings_ui.as_ref().unwrap().startup_state()
        }

        #[test]
        fn opening_settings_queries_the_startup_task_off_the_ui_thread() {
            let dir = TempDir::new("startup-open");
            let (mut app, _owner) = test_app(dir.path());
            app.startup_query = || {
                assert_eq!(
                    std::thread::current().name(),
                    Some(crate::startup::WORKER_THREAD)
                );
                StartupState::On
            };
            app.open_settings_hidden();
            assert_eq!(shown_startup(&app), StartupState::Unknown);
            let hwnd = app.hwnd;
            APP.with(|slot| *slot.borrow_mut() = Some(app));
            let msg = wait_startup_msg(hwnd, Duration::from_secs(5)).unwrap();
            unsafe { window_proc(hwnd, msg.message, msg.wParam, msg.lParam) };
            let shown = with_app(|app| (app.startup.clone(), shown_startup(app))).unwrap();
            assert_eq!(shown, (StartupState::On, StartupState::On));
            APP.with(|slot| slot.borrow_mut().take());
        }

        static ELEVATED_CALLS: Mutex<Vec<(bool, bool)>> = Mutex::new(Vec::new());

        #[test]
        fn elevated_switch_changes_the_task_on_a_worker_then_shows_it() {
            let dir = TempDir::new("startup-elevated");
            let (mut app, _owner) = test_app(dir.path());
            app.elevated = true;
            app.startup_change = |on, elevated, _| {
                ELEVATED_CALLS.lock().unwrap().push((on, elevated));
                StartupResult::Changed(if on {
                    StartupState::On
                } else {
                    StartupState::Off
                })
            };
            app.open_settings_hidden();
            apply_startup_result(&mut app);
            assert_eq!(shown_startup(&app), StartupState::Off);

            app.apply_ui_action(UiAction::SetStartup(true));
            assert_eq!(app.startup, StartupState::Busy);
            assert_eq!(shown_startup(&app), StartupState::Busy);
            // Ignored while the first change runs.
            app.apply_ui_action(UiAction::SetStartup(false));
            apply_startup_result(&mut app);
            assert_eq!(shown_startup(&app), StartupState::On);
            assert_eq!(*ELEVATED_CALLS.lock().unwrap(), vec![(true, true)]);
            assert!(wait_startup_msg(app.hwnd, Duration::from_millis(200)).is_none());
        }

        #[test]
        fn a_stale_query_result_does_not_end_a_change() {
            let dir = TempDir::new("startup-stale-query");
            let (mut app, _owner) = test_app(dir.path());
            app.startup_change = |_, _, _| StartupResult::Declined;
            app.open_settings_hidden();
            apply_startup_result(&mut app);
            assert_eq!(app.startup, StartupState::Off);
            // Reopened: a query is under way when the switch is clicked.
            app.apply_ui_action(UiAction::SetStartup(true));
            assert_eq!(app.startup, StartupState::Busy);
            app.on_startup_result(StartupResult::Query(StartupState::Off));
            assert_eq!(app.startup, StartupState::Busy);
            assert_eq!(shown_startup(&app), StartupState::Busy);
            // Still busy: a second click is ignored.
            app.startup_before = StartupState::Unknown;
            app.apply_ui_action(UiAction::SetStartup(false));
            assert_eq!(app.startup_before, StartupState::Unknown);
            app.on_startup_result(StartupResult::Changed(StartupState::On));
            assert_eq!(app.startup, StartupState::On);
            assert_eq!(shown_startup(&app), StartupState::On);
            // Drain what the fake change posted.
            let _ = wait_startup_msg(app.hwnd, Duration::from_secs(5));
        }

        #[test]
        fn a_failed_change_shows_the_checked_state() {
            let dir = TempDir::new("startup-failed-change");
            let (mut app, _owner) = test_app(dir.path());
            app.open_settings_hidden();
            apply_startup_result(&mut app);
            let failed = StartupState::Error {
                msg: "Couldn't change the startup task: see lite-bar.log".into(),
                actual: Some(false),
            };
            app.on_startup_result(StartupResult::Changed(failed.clone()));
            assert_eq!(shown_startup(&app), failed);
        }

        static UAC_CALLS: Mutex<Vec<(bool, bool)>> = Mutex::new(Vec::new());

        #[test]
        fn declining_uac_puts_the_switch_back_without_an_error() {
            let dir = TempDir::new("startup-declined");
            let (mut app, _owner) = test_app(dir.path());
            app.elevated = false;
            app.startup_query = || StartupState::On;
            app.startup_change = |on, elevated, _| {
                UAC_CALLS.lock().unwrap().push((on, elevated));
                StartupResult::Declined
            };
            app.open_settings_hidden();
            apply_startup_result(&mut app);
            app.apply_ui_action(UiAction::SetStartup(false));
            assert_eq!(app.startup, StartupState::Busy);
            apply_startup_result(&mut app);
            assert_eq!(app.startup, StartupState::On);
            assert_eq!(shown_startup(&app), StartupState::On);
            assert_eq!(*UAC_CALLS.lock().unwrap(), vec![(false, false)]);
        }

        #[test]
        fn the_window_learns_whether_lite_bar_is_elevated() {
            let dir = TempDir::new("startup-elevated-flag");
            let (mut app, _owner) = test_app(dir.path());
            app.elevated = true;
            app.open_settings_hidden();
            assert!(app.settings_ui.as_ref().unwrap().shows_elevated());
        }

        #[test]
        fn startup_worker_panics_keep_the_trace() {
            assert!(!panic_stops_trace(
                false,
                Some(crate::startup::WORKER_THREAD)
            ));
        }
    }
}

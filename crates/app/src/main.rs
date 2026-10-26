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
mod log;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod paths;
#[cfg(windows)]
mod process_info;
#[cfg(windows)]
mod single_instance;
#[cfg(windows)]
mod stats_files;
#[cfg(all(windows, test))]
mod test_util;
#[cfg(windows)]
mod tray;

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
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use lite_bar_core::fps::FpsTracker;
    use lite_bar_core::game_name::display_name;
    use lite_bar_core::history::LocalTime;
    use lite_bar_core::layout::{fps_text, stats_text};
    use lite_bar_core::sessions::{session_log_line, OpenSession, SessionBook};
    use lite_bar_core::settings::{Settings, StatsLayout};
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
        WM_RBUTTONUP, WM_TIMER, WNDCLASSEXW, WS_OVERLAPPED,
    };

    use crate::elevation::{is_elevated, relaunch_as_admin};
    use crate::etw::{offers_admin, CaptureError, PresentCapture};
    use crate::foreground::{foreground_pid, foreground_title};
    use crate::game_lookup::resolve_name;
    use crate::hotkeys::{self, HotkeyId, Registration};
    use crate::overlay::Overlay;
    use crate::paths::{app_dir, load_or_create_settings};
    use crate::process_info::{is_running, process_info};
    use crate::single_instance::{acquire_single_instance, InstanceGuard};
    use crate::stats_files::{local_now, StatsWriter};
    use crate::tray::{MenuItem, Tray, TrayMenuState, WM_TRAY};
    use crate::{log, tray};

    const MUTEX_NAME: &str = "Local\\LiteBar.SingleInstance";
    const TIMER_ID: usize = 1;
    const TIMER_MS: u32 = 250;
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
    }

    /// Runs `f` on the app state, if it exists and is not already borrowed (re-entrancy).
    fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
        APP.with(|app| {
            let mut guard = app.try_borrow_mut().ok()?;
            guard.as_mut().map(f)
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
        /// Pids whose process could not be queried, so the failure is logged once per pid.
        untrackable: HashSet<u32>,
    }

    impl App {
        fn menu_state(&self) -> TrayMenuState {
            TrayMenuState {
                overlay_visible: self.overlay_on,
                toggle_hotkey: self.settings.hotkeys.toggle_overlay.to_string(),
                toggle_available: self.hotkey_ok,
                needs_admin: self.needs_admin,
            }
        }

        fn register_hotkey(&mut self) {
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
            // Show the overlay (with "—") and start the timer first, so the ETW start latency
            // is not part of what the user sees.
            self.overlay.show();
            // SAFETY: plain FFI on our own window.
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_ID, TIMER_MS, None);
            }
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
                .set_focus(None);
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
        }

        fn on_timer(&mut self) {
            let now = self.clock.elapsed().as_secs_f64();
            let pid = foreground_pid();
            let want_stats = self.settings.overlay.stats != StatsLayout::Off;
            // One lock for focus, FPS and stats; the summary is Copy so it is released early.
            let (fps, summary, has_stats) = {
                let mut tracker = self.tracker.lock().unwrap_or_else(|e| e.into_inner());
                tracker.set_focus(pid);
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
            if let Some(pid) = pid_to_track(pid, has_stats, &self.sessions) {
                self.start_session(pid);
            }
            self.ticks = self.ticks.wrapping_add(1);
            if alive_check_due(self.ticks) {
                self.check_sessions();
            }
            if self.ticks.is_multiple_of(PRUNE_EVERY_TICKS) {
                self.tracker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .prune(now);
            }
        }

        /// Opens a session for the foreground `pid`, which has just counted its first frames.
        fn start_session(&mut self, pid: u32) {
            match process_info(pid) {
                Some(info) => {
                    let now = local_now();
                    self.sessions.track(OpenSession {
                        pid,
                        created: info.created,
                        exe_path: info.exe_path,
                        window_title: foreground_title(),
                        start: now,
                        last_alive: now,
                    });
                }
                None => {
                    // Without its identity the frames can't be stored; drop them.
                    self.tracker
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .take_stats(pid);
                    if self.untrackable.insert(pid) {
                        log::warn(&format!(
                            "stats: cannot query process {pid}; its play time is not recorded"
                        ));
                    }
                }
            }
        }

        /// The 1 s check: sessions whose process exited end when it was last seen alive.
        fn check_sessions(&mut self) {
            let finished = self
                .sessions
                .sweep(local_now(), |s| is_running(s.pid, s.created));
            for (session, end) in finished {
                self.finish_session(session, end);
            }
        }

        /// Quit or sign-out: every session ends (now, if its process still runs) and queued
        /// stats writes are retried.
        fn finish_all_sessions(&mut self) {
            let finished = self
                .sessions
                .drain(local_now(), |s| is_running(s.pid, s.created));
            for (session, end) in finished {
                self.finish_session(session, end);
            }
            self.writer.flush(&self.settings.game_names);
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
            }
        }

        fn reload_settings(&mut self) {
            let (settings, warnings) = load_or_create_settings(&self.dir);
            for w in &warnings {
                log::warn(w);
            }
            if game_names_changed(&self.settings, &settings) {
                self.writer.mark_games_csv_dirty();
            }
            self.overlay.set_settings(&settings.overlay);
            self.settings = settings;
            self.writer.flush(&self.settings.game_names);
            self.register_hotkey();
            log::info("settings reloaded");
        }

        fn open_settings_file(&self) {
            shell_open(&self.dir.join("settings.toml"), "settings file");
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
            self.guard = None;
            hotkeys::unregister(self.hwnd, HotkeyId::ToggleOverlay);
            self.stop_overlay();
            self.tray.remove();
            match relaunch_as_admin() {
                Ok(()) => true,
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
                MenuItem::ToggleOverlay => self.toggle_overlay(),
                MenuItem::OpenSettingsFile => self.open_settings_file(),
                MenuItem::OpenGameStats => self.open_game_stats(),
                MenuItem::ReloadSettings => self.reload_settings(),
                MenuItem::RestartAsAdmin => {
                    if self.restart_as_admin() {
                        quit();
                    }
                }
                MenuItem::Quit => quit(),
            }
        }

        /// Stops capture, removes the tray icon and saves the open sessions (the icon and
        /// window go with the drop).
        fn shutdown(&mut self) {
            log::info("shutting down");
            hotkeys::unregister(self.hwnd, HotkeyId::ToggleOverlay);
            self.stop_overlay();
            self.tray.remove();
            self.finish_all_sessions();
        }

        /// The user is signing out or Windows is shutting down: the process may be ended as
        /// soon as this returns.
        fn end_session(&mut self) {
            self.stop_overlay();
            self.finish_all_sessions();
        }
    }

    /// Opens `path` (a file or a folder) in its default program.
    fn shell_open(path: &Path, what: &str) {
        let path = HSTRING::from(path.as_os_str());
        // SAFETY: valid NUL-terminated strings for the duration of the call.
        let result = unsafe { ShellExecuteW(None, w!("open"), &path, None, None, SW_SHOWNORMAL) };
        if result.0 as usize <= 32 {
            log::error(&format!(
                "could not open {what}: {}",
                windows::core::Error::from_thread()
            ));
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

    /// The foreground pid that needs a new session: it has counted frames and none is open.
    fn pid_to_track(pid: Option<u32>, has_stats: bool, sessions: &SessionBook) -> Option<u32> {
        pid.filter(|pid| has_stats && !sessions.contains(*pid))
    }

    /// Whether this tick (counted after incrementing) runs the 1 s process-exit check.
    fn alive_check_due(ticks: u32) -> bool {
        ticks.is_multiple_of(ALIVE_CHECK_EVERY_TICKS)
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
            WM_HOTKEY if wparam.0 == HotkeyId::ToggleOverlay as usize => {
                with_app(App::toggle_overlay);
                return LRESULT(0);
            }
            WM_TRAY => {
                let event = (lparam.0 & 0xFFFF) as u32;
                if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                    on_tray_menu();
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
                with_app(App::end_session);
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

    /// A panic in the window procedure would abort at the FFI boundary: the app would vanish
    /// silently and leak its ETW session. Log it and stop the session on the way out. The
    /// previous hook is not called (there is no console).
    fn install_panic_hook() {
        std::panic::set_hook(Box::new(|info| {
            let location = info.location().map(|l| (l.file(), l.line(), l.column()));
            log::error(&panic_message(info.payload(), location));
            let _ = ferrisetw::trace::stop_trace_by_name(crate::etw::SESSION_NAME);
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

        let guard = match acquire_single_instance(MUTEX_NAME) {
            Ok(Some(guard)) => Some(guard),
            Ok(None) => return, // already running: exit silently
            Err(e) => {
                log::error(&format!(
                    "single-instance check failed, running anyway: {e}"
                ));
                None
            }
        };

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
        let writer = StatsWriter::new(dir.join(STATS_DIR));
        let mut app = App {
            hwnd: owner.0,
            dir,
            settings,
            overlay,
            tray,
            tracker: Arc::new(Mutex::new(FpsTracker::new())),
            clock: Instant::now(),
            capture: None,
            guard,
            overlay_on: false,
            needs_admin: false,
            hotkey_ok: false,
            ticks: 0,
            sessions: SessionBook::new(),
            writer,
            untrackable: HashSet::new(),
        };
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

        #[test]
        fn panic_message_formats_str_payload_and_location() {
            let payload: Box<dyn Any + Send> = Box::new("boom");
            assert_eq!(
                panic_message(&*payload, Some(("src/main.rs", 12, 5))),
                "panic: boom at src/main.rs:12:5"
            );
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

        #[test]
        fn foreground_with_stats_and_no_session_is_tracked() {
            assert_eq!(pid_to_track(Some(7), true, &SessionBook::new()), Some(7));
            assert_eq!(pid_to_track(Some(8), true, &tracked(7)), Some(8));
        }

        #[test]
        fn tracked_pid_or_one_without_stats_is_not_tracked_again() {
            assert_eq!(pid_to_track(Some(7), true, &tracked(7)), None);
            assert_eq!(pid_to_track(Some(7), false, &SessionBook::new()), None);
            assert_eq!(pid_to_track(None, true, &SessionBook::new()), None);
        }

        #[test]
        fn exit_check_runs_every_fourth_tick() {
            let due: Vec<u32> = (1..=12).filter(|t| alive_check_due(*t)).collect();
            assert_eq!(due, vec![4, 8, 12]);
            assert_eq!(ALIVE_CHECK_EVERY_TICKS * TIMER_MS, 1000);
        }

        #[test]
        fn only_changed_game_names_regenerate_games_csv() {
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
    }
}

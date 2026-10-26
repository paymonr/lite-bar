//! Lite Bar: Windows tray app with an FPS overlay.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod elevation;
#[cfg(windows)]
mod etw;
#[cfg(windows)]
mod foreground;
#[cfg(windows)]
mod hotkeys;
#[cfg(windows)]
mod log;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod paths;
#[cfg(windows)]
mod single_instance;
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
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use lite_bar_core::fps::FpsTracker;
    use lite_bar_core::layout::fps_text;
    use lite_bar_core::settings::Settings;
    use windows::core::{w, HSTRING};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer,
        PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, SetTimer, TranslateMessage, MSG,
        SW_SHOWNORMAL, WINDOW_EX_STYLE, WM_CONTEXTMENU, WM_HOTKEY, WM_RBUTTONUP, WM_TIMER,
        WNDCLASSEXW, WS_OVERLAPPED,
    };

    use crate::elevation::{is_elevated, relaunch_as_admin};
    use crate::etw::{CaptureError, PresentCapture};
    use crate::foreground::foreground_pid;
    use crate::hotkeys::{self, HotkeyId, Registration};
    use crate::overlay::Overlay;
    use crate::paths::{app_dir, load_or_create_settings};
    use crate::single_instance::{acquire_single_instance, InstanceGuard};
    use crate::tray::{MenuItem, Tray, TrayMenuState, WM_TRAY};
    use crate::{log, tray};

    const MUTEX_NAME: &str = "Local\\LiteBar.SingleInstance";
    const TIMER_ID: usize = 1;
    const TIMER_MS: u32 = 250;
    /// Prune the tracker every this many timer ticks (5 s).
    const PRUNE_EVERY_TICKS: u32 = 20;

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
            match PresentCapture::start(self.tracker.clone(), self.clock) {
                Ok(capture) => self.capture = Some(capture),
                Err(CaptureError::NeedsAdmin) => {
                    self.needs_admin = true;
                    log::info("FPS capture needs administrator rights");
                }
                Err(CaptureError::Other(msg)) => log::error(&format!("FPS capture failed: {msg}")),
            }
            self.overlay.show();
            // SAFETY: plain FFI on our own window.
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_ID, TIMER_MS, None);
            }
            self.on_timer();
        }

        fn stop_overlay(&mut self) {
            self.overlay_on = false;
            // SAFETY: plain FFI on our own window.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_ID);
            }
            if let Some(capture) = self.capture.take() {
                capture.stop();
            }
            self.overlay.hide();
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
            let fps = foreground_pid().and_then(|pid| {
                self.tracker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .fps(pid, now)
            });
            self.overlay.update(&fps_text(fps));
            self.ticks = self.ticks.wrapping_add(1);
            if self.ticks.is_multiple_of(PRUNE_EVERY_TICKS) {
                self.tracker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .prune(now);
            }
        }

        fn reload_settings(&mut self) {
            let (settings, warnings) = load_or_create_settings(&self.dir);
            for w in &warnings {
                log::warn(w);
            }
            self.overlay.set_settings(&settings.overlay);
            self.settings = settings;
            self.register_hotkey();
            log::info("settings reloaded");
        }

        fn open_settings_file(&self) {
            let path = HSTRING::from(self.dir.join("settings.toml").as_os_str());
            // SAFETY: valid NUL-terminated strings for the duration of the call.
            let result =
                unsafe { ShellExecuteW(None, w!("open"), &path, None, None, SW_SHOWNORMAL) };
            if result.0 as usize <= 32 {
                log::error(&format!(
                    "could not open settings file: {}",
                    windows::core::Error::from_thread()
                ));
            }
        }

        /// Relaunches elevated. Returns true if this process should exit.
        fn restart_as_admin(&mut self) -> bool {
            let was_on = self.overlay_on;
            self.guard = None;
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
                MenuItem::ReloadSettings => self.reload_settings(),
                MenuItem::RestartAsAdmin => {
                    if self.restart_as_admin() {
                        quit();
                    }
                }
                MenuItem::Quit => quit(),
            }
        }

        /// Stops capture and removes the tray icon (the icon and window go with the drop).
        fn shutdown(&mut self) {
            hotkeys::unregister(self.hwnd, HotkeyId::ToggleOverlay);
            self.stop_overlay();
            self.tray.remove();
        }
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
        // SAFETY: forwards the arguments unchanged from the system.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    pub fn run() {
        // SAFETY: plain FFI. Fails (ignored) if a manifest already set the awareness.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let dir = app_dir();
        log::init(&dir);

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
        TASKBAR_CREATED.set(unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) });
        let owner = create_owner_window()?;
        let tray = Rc::new(tray::Tray::new(owner.0)?);
        let overlay = Overlay::new(&settings.overlay)?;

        let start_visible = settings.overlay.visible;
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
}

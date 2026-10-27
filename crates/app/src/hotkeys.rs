//! Global hotkey registration (`RegisterHotKey` only; no keyboard hooks).

use lite_bar_core::hotkey::Hotkey;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT, MOD_WIN,
};

/// Identifiers of the app's hotkeys, as delivered in `WM_HOTKEY`'s `wParam`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyId {
    ToggleOverlay = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Registration {
    Ok,
    /// Another application already owns the combination (or the call failed).
    Unavailable,
}

/// Registers `hk` for `hwnd`. Auto-repeat is suppressed.
pub fn register(hwnd: HWND, id: HotkeyId, hk: Hotkey) -> Registration {
    let mut mods = MOD_NOREPEAT;
    let flags: [(bool, HOT_KEY_MODIFIERS); 4] = [
        (hk.ctrl, MOD_CONTROL),
        (hk.alt, MOD_ALT),
        (hk.shift, MOD_SHIFT),
        (hk.win, MOD_WIN),
    ];
    for (on, flag) in flags {
        if on {
            mods |= flag;
        }
    }
    // SAFETY: plain FFI with a window handle owned by the caller.
    let result =
        unsafe { RegisterHotKey(Some(hwnd), id as i32, mods, u32::from(hk.key.virtual_key())) };
    match result {
        Ok(()) => Registration::Ok,
        Err(_) => Registration::Unavailable,
    }
}

/// Releases a hotkey registered with `register`. Harmless if it was never registered.
pub fn unregister(hwnd: HWND, id: HotkeyId) {
    // SAFETY: plain FFI with a window handle owned by the caller.
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), id as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lite_bar_core::hotkey::Hotkey;
    use windows::core::w;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
    };

    /// A message-only window; destroyed on drop.
    struct TestWindow(HWND);

    impl Drop for TestWindow {
        fn drop(&mut self) {
            // SAFETY: the window was created by this thread and is destroyed once.
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    fn test_window() -> TestWindow {
        // SAFETY: plain FFI; the predefined STATIC class needs no registration.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                WINDOW_STYLE(0),
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
        .expect("create test window");
        TestWindow(hwnd)
    }

    #[test]
    fn second_registration_reports_unavailable() {
        let hk: Hotkey = "Ctrl+Alt+Shift+F24".parse().unwrap();
        let (a, b) = (test_window(), test_window());
        assert!(matches!(
            register(a.0, HotkeyId::ToggleOverlay, hk),
            Registration::Ok
        ));
        assert!(matches!(
            register(b.0, HotkeyId::ToggleOverlay, hk),
            Registration::Unavailable
        ));
        unregister(a.0, HotkeyId::ToggleOverlay);
    }
}

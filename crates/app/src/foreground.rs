//! Foreground window queries.

use lite_bar_core::layout::Rect;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, GetWindowTextW, GetWindowThreadProcessId, SM_CXSCREEN,
    SM_CYSCREEN,
};

const BASE_DPI: f32 = 96.0;

/// The foreground window and the process id owning it; `None` when there is no foreground
/// window. Read once per tick so the pid and the title come from the same window.
pub fn foreground_window() -> Option<(u32, HWND)> {
    // SAFETY: plain FFI calls; a null window handle is checked before use.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (pid != 0).then_some((pid, hwnd))
    }
}

/// Title of `hwnd`; `None` when it has none (or the window is gone).
pub fn window_title(hwnd: HWND) -> Option<String> {
    let mut buf = [0u16; 512];
    // SAFETY: plain FFI; the buffer outlives the call and its length is passed implicitly.
    let len = usize::try_from(unsafe { GetWindowTextW(hwnd, &mut buf) }).ok()?;
    (len > 0).then(|| String::from_utf16_lossy(&buf[..len]))
}

/// Full rectangle (not the work area: games cover the taskbar) and DPI scale of the monitor
/// holding the foreground window. Falls back to the primary monitor when there is no
/// foreground window.
pub fn foreground_monitor() -> (Rect, f32) {
    // SAFETY: plain FFI calls with properly sized, initialised out-parameters.
    unsafe {
        let monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY);

        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let rect = if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let r = info.rcMonitor;
            Rect {
                left: r.left,
                top: r.top,
                right: r.right,
                bottom: r.bottom,
            }
        } else {
            Rect {
                left: 0,
                top: 0,
                right: GetSystemMetrics(SM_CXSCREEN),
                bottom: GetSystemMetrics(SM_CYSCREEN),
            }
        };

        let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
        let scale = match GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) {
            Ok(()) if dpi_x > 0 => dpi_x as f32 / BASE_DPI,
            _ => 1.0,
        };
        (rect, scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_monitor_returns_nonempty_rect_and_sane_scale() {
        let (rect, scale) = foreground_monitor();
        assert!(rect.right > rect.left && rect.bottom > rect.top, "{rect:?}");
        assert!((1.0..=6.0).contains(&scale), "scale {scale}");
    }

    #[test]
    fn foreground_window_has_a_nonzero_pid() {
        if let Some((pid, hwnd)) = foreground_window() {
            assert_ne!(pid, 0);
            assert!(!hwnd.is_invalid());
        }
    }

    #[test]
    fn window_title_reads_that_window() {
        use windows::core::{w, PCWSTR};
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WS_OVERLAPPED,
        };
        let create = |title: PCWSTR| {
            // SAFETY: plain FFI; the predefined STATIC class needs no registration.
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    title,
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
            .unwrap()
        };
        let titled = create(w!("Lite Bar title test"));
        let untitled = create(w!(""));
        assert_eq!(
            window_title(titled),
            Some("Lite Bar title test".to_string())
        );
        assert_eq!(window_title(untitled), None);
        // SAFETY: created above on this thread.
        unsafe {
            let _ = DestroyWindow(titled);
            let _ = DestroyWindow(untitled);
        }
    }
}

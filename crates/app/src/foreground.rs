//! Foreground window queries.

use lite_bar_core::layout::Rect;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, GetWindowTextW, GetWindowThreadProcessId, SM_CXSCREEN,
    SM_CYSCREEN,
};

const BASE_DPI: f32 = 96.0;

/// Process id owning the foreground window; `None` when there is no foreground window.
pub fn foreground_pid() -> Option<u32> {
    // SAFETY: plain FFI calls; a null window handle is checked before use.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (pid != 0).then_some(pid)
    }
}

/// Title of the foreground window; `None` when there is no window or it has no title.
pub fn foreground_title() -> Option<String> {
    // SAFETY: plain FFI calls; the buffer outlives the call and its length is passed implicitly.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut buf = [0u16; 512];
        let len = usize::try_from(GetWindowTextW(hwnd, &mut buf)).ok()?;
        (len > 0).then(|| String::from_utf16_lossy(&buf[..len]))
    }
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
    fn foreground_title_is_none_or_nonempty() {
        if let Some(title) = foreground_title() {
            assert!(!title.is_empty());
        }
    }

    #[test]
    fn foreground_pid_is_none_or_nonzero() {
        if let Some(pid) = foreground_pid() {
            assert_ne!(pid, 0);
        }
    }
}

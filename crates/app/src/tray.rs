//! Tray icon and context menu.

use std::cell::Cell;

use windows::core::{Error, PCWSTR};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION,
    NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos,
    PostMessageW, SetForegroundWindow, TrackPopupMenu, HICON, ICONINFO, MF_SEPARATOR, MF_STRING,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_NULL,
};

/// Callback message the shell sends to the owner window. Uses `NOTIFYICON_VERSION_4`
/// semantics: the low word of `lParam` is the event (`WM_RBUTTONUP`, `WM_CONTEXTMENU`, ...).
pub const WM_TRAY: u32 = WM_APP + 1;

const TRAY_ICON_ID: u32 = 1;
const TOOLTIP: &str = "Lite Bar";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    ToggleOverlay,
    OpenSettingsFile,
    ReloadSettings,
    RestartAsAdmin,
    Quit,
}

pub struct TrayMenuState {
    pub overlay_visible: bool,
    pub toggle_hotkey: String,
    pub toggle_available: bool,
    pub needs_admin: bool,
}

/// The menu entries in display order (separators are added by `Tray::show_menu`).
pub fn menu_labels(state: &TrayMenuState) -> Vec<(MenuItem, String)> {
    let action = if state.overlay_visible {
        "Hide FPS overlay"
    } else {
        "Show FPS overlay"
    };
    let toggle = if state.toggle_available {
        format!("{action}\t{}", state.toggle_hotkey)
    } else {
        format!("{action}\t({} unavailable)", state.toggle_hotkey)
    };
    let mut items = vec![
        (MenuItem::ToggleOverlay, toggle),
        (MenuItem::OpenSettingsFile, "Open settings file".to_owned()),
        (MenuItem::ReloadSettings, "Reload settings".to_owned()),
    ];
    if state.needs_admin {
        items.push((
            MenuItem::RestartAsAdmin,
            "Restart as administrator to show FPS".to_owned(),
        ));
    }
    items.push((MenuItem::Quit, "Quit".to_owned()));
    items
}

const ICON_SIZE: usize = 32;
const ICON_RADIUS: f32 = 6.0;
const ICON_BLUE: u32 = 0xFF2F_6FDB;
const ICON_WHITE: u32 = 0xFFFF_FFFF;

/// 32x32 `0xAARRGGBB` pixels, row-major, top-down: a rounded `#2F6FDB` square with a white "L".
/// Alpha is either 0 or 255, so premultiplication does not matter.
fn icon_pixels() -> Vec<u32> {
    let size = ICON_SIZE as f32;
    let mut px = vec![0u32; ICON_SIZE * ICON_SIZE];
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            // Distance from the pixel centre to the nearest corner-circle centre, if in a corner.
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            let ox = (ICON_RADIUS - cx).max(cx - (size - ICON_RADIUS)).max(0.0);
            let oy = (ICON_RADIUS - cy).max(cy - (size - ICON_RADIUS)).max(0.0);
            if ox * ox + oy * oy > ICON_RADIUS * ICON_RADIUS {
                continue;
            }
            let stem = (10..14).contains(&x) && (7..25).contains(&y);
            let foot = (10..24).contains(&x) && (21..25).contains(&y);
            px[y * ICON_SIZE + x] = if stem || foot { ICON_WHITE } else { ICON_BLUE };
        }
    }
    px
}

/// Builds the icon from `icon_pixels` with a 32-bit ARGB DIB and an all-zero mask.
fn create_icon() -> windows::core::Result<HICON> {
    let pixels = icon_pixels();
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: ICON_SIZE as i32,
            biHeight: -(ICON_SIZE as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: `info` is a valid BITMAPINFO, `bits` a valid out-pointer. The DIB holds
    // ICON_SIZE * ICON_SIZE 32-bit pixels, which is exactly what is copied in. Both bitmaps are
    // deleted before returning (CreateIconIndirect copies them).
    unsafe {
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u32>(), pixels.len());
        // 1 bpp mask, rows padded to 16 bits: 32 px = 4 bytes per row.
        let mask_bits = [0u8; ICON_SIZE * 4];
        let mask = CreateBitmap(
            ICON_SIZE as i32,
            ICON_SIZE as i32,
            1,
            1,
            Some(mask_bits.as_ptr().cast()),
        );
        let icon_info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&icon_info);
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

/// The notification-area icon and its context menu. Drop removes the icon.
pub struct Tray {
    hwnd: HWND,
    icon: HICON,
    added: Cell<bool>,
}

impl Tray {
    /// Creates the icon and tries to add it. If the shell is not ready yet the add is retried
    /// by the caller (on `TaskbarCreated`) via `add`; only icon creation failure is an error.
    pub fn new(hwnd: HWND) -> windows::core::Result<Tray> {
        let tray = Tray {
            hwnd,
            icon: create_icon()?,
            added: Cell::new(false),
        };
        if let Err(e) = tray.add() {
            crate::log::warn(&format!(
                "tray icon not added yet ({e}); will retry when the taskbar is created"
            ));
        }
        Ok(tray)
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ICON_ID,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: self.icon,
            ..Default::default()
        };
        for (dst, src) in nid.szTip.iter_mut().zip(TOOLTIP.encode_utf16()) {
            *dst = src;
        }
        nid
    }

    /// Adds the icon to the notification area (again, after Explorer restarted).
    pub fn add(&self) -> windows::core::Result<()> {
        let mut nid = self.data();
        // SAFETY: `nid` is fully initialised and outlives the calls.
        unsafe {
            if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                return Err(Error::from_thread());
            }
            self.added.set(true);
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            if !Shell_NotifyIconW(NIM_SETVERSION, &nid).as_bool() {
                return Err(Error::from_thread());
            }
        }
        Ok(())
    }

    /// Removes the icon. Does nothing if it is not currently added.
    pub fn remove(&self) {
        if self.added.replace(false) {
            let nid = self.data();
            // SAFETY: `nid` is fully initialised and outlives the call.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            }
        }
    }

    /// Shows the context menu at the cursor and returns the chosen item, if any.
    pub fn show_menu(&self, state: &TrayMenuState) -> Option<MenuItem> {
        let labels = menu_labels(state);
        // SAFETY: plain FFI; the menu is destroyed before returning and every string outlives
        // its AppendMenuW call.
        unsafe {
            let menu = CreatePopupMenu().ok()?;
            let separator = |menu| {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            };
            let last = labels.len() - 1;
            for (i, (item, label)) in labels.iter().enumerate() {
                if i == last {
                    separator(menu); // before Quit
                }
                let text: Vec<u16> = label.encode_utf16().chain(Some(0)).collect();
                // Command ids are index + 1 (0 means "nothing chosen").
                let _ = AppendMenuW(menu, MF_STRING, i + 1, PCWSTR(text.as_ptr()));
                if *item == MenuItem::ToggleOverlay {
                    separator(menu);
                }
            }
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // Without this the menu does not close when the user clicks elsewhere.
            let _ = SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                pt.x,
                pt.y,
                None,
                self.hwnd,
                None,
            );
            let _ = PostMessageW(
                Some(self.hwnd),
                WM_NULL,
                Default::default(),
                Default::default(),
            );
            let _ = DestroyMenu(menu);
            let index = usize::try_from(cmd.0).ok()?.checked_sub(1)?;
            labels.get(index).map(|(item, _)| *item)
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.remove();
        // SAFETY: the icon was created by CreateIconIndirect and is destroyed exactly once.
        unsafe {
            let _ = DestroyIcon(self.icon);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_labels_normal() {
        let s = TrayMenuState {
            overlay_visible: true,
            toggle_hotkey: "Ctrl+Alt+Shift+L".into(),
            toggle_available: true,
            needs_admin: false,
        };
        let l = menu_labels(&s);
        assert_eq!(
            l[0],
            (
                MenuItem::ToggleOverlay,
                "Hide FPS overlay\tCtrl+Alt+Shift+L".into()
            )
        );
        assert!(!l.iter().any(|(m, _)| *m == MenuItem::RestartAsAdmin));
        assert_eq!(l.last().unwrap().0, MenuItem::Quit);
    }

    #[test]
    fn menu_labels_unavailable_hotkey_and_admin() {
        let s = TrayMenuState {
            overlay_visible: false,
            toggle_hotkey: "Ctrl+Alt+Shift+L".into(),
            toggle_available: false,
            needs_admin: true,
        };
        let l = menu_labels(&s);
        assert_eq!(l[0].1, "Show FPS overlay\t(Ctrl+Alt+Shift+L unavailable)");
        assert!(l.contains(&(
            MenuItem::RestartAsAdmin,
            "Restart as administrator to show FPS".into()
        )));
    }

    #[test]
    fn menu_order_matches_spec() {
        let s = TrayMenuState {
            overlay_visible: true,
            toggle_hotkey: "Ctrl+Alt+Shift+L".into(),
            toggle_available: true,
            needs_admin: true,
        };
        let items: Vec<MenuItem> = menu_labels(&s).into_iter().map(|(m, _)| m).collect();
        assert_eq!(
            items,
            vec![
                MenuItem::ToggleOverlay,
                MenuItem::OpenSettingsFile,
                MenuItem::ReloadSettings,
                MenuItem::RestartAsAdmin,
                MenuItem::Quit
            ]
        );
    }

    #[test]
    fn icon_is_rounded_blue_square_with_white_l() {
        let px = icon_pixels();
        let at = |x: usize, y: usize| px[y * ICON_SIZE + x];
        // Rounded corner is transparent, the body is opaque #2F6FDB.
        assert_eq!(at(0, 0) >> 24, 0);
        assert_eq!(at(3, 16), 0xFF2F6FDB);
        // The stem of the L is white; the area right of the stem, above the foot, is blue.
        assert_eq!(at(11, 12), 0xFFFFFFFF);
        assert_eq!(at(20, 12), 0xFF2F6FDB);
        // The foot of the L.
        assert_eq!(at(18, 23), 0xFFFFFFFF);
    }
}

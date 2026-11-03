//! Tray icon and context menu.

use std::cell::Cell;

use lite_bar_core::update::Version;

use windows::core::{Error, PCWSTR};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION,
    NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos, GetSystemMetrics,
    LoadImageW, PostMessageW, SetForegroundWindow, SetMenuDefaultItem, TrackPopupMenu, HICON,
    IMAGE_ICON, LR_DEFAULTCOLOR, MF_SEPARATOR, MF_STRING, SM_CXICON, SM_CXSMICON, SM_CYICON,
    SM_CYSMICON, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_NULL,
};

/// Callback message the shell sends to the owner window. Uses `NOTIFYICON_VERSION_4`
/// semantics: the low word of `lParam` is the event (`WM_RBUTTONUP`, `WM_CONTEXTMENU`, ...).
pub const WM_TRAY: u32 = WM_APP + 1;

const TRAY_ICON_ID: u32 = 1;
const TOOLTIP: &str = "Lite Bar";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    OpenSettings,
    ToggleOverlay,
    OpenSettingsFile,
    OpenGameStats,
    ReloadSettings,
    RestartAsAdmin,
    InstallUpdate,
    Quit,
}

pub struct TrayMenuState {
    pub overlay_visible: bool,
    pub toggle_hotkey: String,
    pub toggle_available: bool,
    pub needs_admin: bool,
    /// A newer release can be installed.
    pub update_available: Option<Version>,
}

/// Index of the bold default entry (Settings…), the one a double-click opens.
pub fn default_item(labels: &[(MenuItem, String)]) -> Option<usize> {
    labels
        .iter()
        .position(|(m, _)| *m == MenuItem::OpenSettings)
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
        (MenuItem::OpenSettings, "Settings…".to_owned()),
        (MenuItem::ToggleOverlay, toggle),
        (MenuItem::OpenSettingsFile, "Open settings file".to_owned()),
        (MenuItem::OpenGameStats, "Open game stats".to_owned()),
        (MenuItem::ReloadSettings, "Reload settings".to_owned()),
    ];
    if state.needs_admin {
        items.push((
            MenuItem::RestartAsAdmin,
            "Restart as administrator to show FPS".to_owned(),
        ));
    }
    if let Some(v) = &state.update_available {
        items.push((MenuItem::InstallUpdate, format!("Update to v{v}…")));
    }
    items.push((MenuItem::Quit, "Quit".to_owned()));
    items
}

/// Resource id of the app icon group. `build.rs` embeds `assets/lite-bar.ico` under it; the
/// exe, its shortcut and its Installed apps entry all show it.
pub const APP_ICON_ID: u16 = 1;

/// Which of the system's icon sizes to load.
#[derive(Clone, Copy, Debug)]
pub enum IconSize {
    /// Tray, title bar: `SM_CXSMICON`.
    Small,
    /// Taskbar, Alt+Tab: `SM_CXICON`.
    Large,
}

/// The app icon at `size`, loaded from the exe's own resources. Destroy it with `DestroyIcon`.
pub(crate) fn create_icon(size: IconSize) -> windows::core::Result<HICON> {
    let (cx, cy) = match size {
        IconSize::Small => (SM_CXSMICON, SM_CYSMICON),
        IconSize::Large => (SM_CXICON, SM_CYICON),
    };
    // SAFETY: plain FFI; the name is an integer resource id, as MAKEINTRESOURCE makes. Without
    // LR_SHARED the icon is a new one the caller owns.
    unsafe {
        let module = GetModuleHandleW(None)?;
        let handle = LoadImageW(
            Some(module.into()),
            PCWSTR(APP_ICON_ID as usize as *const u16),
            IMAGE_ICON,
            GetSystemMetrics(cx),
            GetSystemMetrics(cy),
            LR_DEFAULTCOLOR,
        )?;
        Ok(HICON(handle.0))
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
            icon: create_icon(IconSize::Small)?,
            added: Cell::new(false),
        };
        if let Err(e) = tray.add() {
            crate::log::warn(&format!(
                "tray icon not added yet ({e}); will retry when the taskbar is created"
            ));
        }
        Ok(tray)
    }

    /// A tray whose icon is never added (tests).
    #[cfg(test)]
    pub fn new_hidden(hwnd: HWND) -> windows::core::Result<Tray> {
        Ok(Tray {
            hwnd,
            icon: create_icon(IconSize::Small)?,
            added: Cell::new(false),
        })
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
            if let Some(i) = default_item(&labels) {
                // Bold: what a double-click on the icon does.
                let _ = SetMenuDefaultItem(menu, (i + 1) as u32, 0);
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
        // SAFETY: the icon was loaded by create_icon and is destroyed exactly once.
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
            update_available: None,
        };
        let l = menu_labels(&s);
        assert_eq!(l[0], (MenuItem::OpenSettings, "Settings…".into()));
        assert_eq!(default_item(&l), Some(0));
        assert_eq!(
            l[1],
            (
                MenuItem::ToggleOverlay,
                "Hide FPS overlay\tCtrl+Alt+Shift+L".into()
            )
        );
        assert!(l.contains(&(MenuItem::OpenGameStats, "Open game stats".into())));
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
            update_available: None,
        };
        let l = menu_labels(&s);
        assert_eq!(l[1].1, "Show FPS overlay\t(Ctrl+Alt+Shift+L unavailable)");
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
            update_available: None,
        };
        let items: Vec<MenuItem> = menu_labels(&s).into_iter().map(|(m, _)| m).collect();
        assert_eq!(
            items,
            vec![
                MenuItem::OpenSettings,
                MenuItem::ToggleOverlay,
                MenuItem::OpenSettingsFile,
                MenuItem::OpenGameStats,
                MenuItem::ReloadSettings,
                MenuItem::RestartAsAdmin,
                MenuItem::Quit
            ]
        );
    }

    fn state_with_update(needs_admin: bool) -> TrayMenuState {
        TrayMenuState {
            overlay_visible: true,
            toggle_hotkey: "Ctrl+Alt+Shift+L".into(),
            toggle_available: true,
            needs_admin,
            update_available: Some("0.5.0".parse().unwrap()),
        }
    }

    #[test]
    fn update_note_sits_directly_above_quit() {
        let l = menu_labels(&state_with_update(false));
        let n = l.len();
        assert_eq!(
            l[n - 2],
            (MenuItem::InstallUpdate, "Update to v0.5.0…".into())
        );
        assert_eq!(l[n - 1].0, MenuItem::Quit);
    }

    #[test]
    fn menu_order_with_an_update() {
        let items: Vec<MenuItem> = menu_labels(&state_with_update(true))
            .into_iter()
            .map(|(m, _)| m)
            .collect();
        assert_eq!(
            items,
            vec![
                MenuItem::OpenSettings,
                MenuItem::ToggleOverlay,
                MenuItem::OpenSettingsFile,
                MenuItem::OpenGameStats,
                MenuItem::ReloadSettings,
                MenuItem::RestartAsAdmin,
                MenuItem::InstallUpdate,
                MenuItem::Quit
            ]
        );
    }

    #[test]
    fn no_update_note_without_an_update() {
        let mut s = state_with_update(true);
        s.update_available = None;
        assert!(!menu_labels(&s)
            .iter()
            .any(|(m, _)| *m == MenuItem::InstallUpdate));
    }

    #[test]
    fn exe_carries_the_app_icon() {
        use windows::Win32::System::LibraryLoader::{FindResourceW, GetModuleHandleW};
        use windows::Win32::UI::WindowsAndMessaging::RT_GROUP_ICON;
        // SAFETY: plain FFI; the name is an integer resource id, as MAKEINTRESOURCE makes.
        let found = unsafe {
            let module = GetModuleHandleW(None).unwrap();
            FindResourceW(
                Some(module),
                PCWSTR(APP_ICON_ID as usize as *const u16),
                RT_GROUP_ICON,
            )
        };
        assert!(
            !found.is_invalid(),
            "no icon group {APP_ICON_ID} in the exe"
        );
    }

    /// Width of the color bitmap of `icon`.
    fn icon_width(icon: HICON) -> i32 {
        use windows::Win32::Graphics::Gdi::{DeleteObject, GetObjectW, BITMAP};
        use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};
        let mut info = ICONINFO::default();
        let mut bitmap = BITMAP::default();
        // SAFETY: plain FFI with live out-parameters; GetIconInfo's bitmaps are ours to delete.
        unsafe {
            GetIconInfo(icon, &mut info).unwrap();
            GetObjectW(
                info.hbmColor.into(),
                size_of::<BITMAP>() as i32,
                Some((&mut bitmap as *mut BITMAP).cast()),
            );
            let _ = DeleteObject(info.hbmColor.into());
            let _ = DeleteObject(info.hbmMask.into());
        }
        bitmap.bmWidth
    }

    #[test]
    fn create_icon_has_the_system_icon_sizes() {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXICON, SM_CXSMICON};
        for (size, metric) in [(IconSize::Small, SM_CXSMICON), (IconSize::Large, SM_CXICON)] {
            let icon = create_icon(size).unwrap();
            // SAFETY: plain FFI.
            let expected = unsafe { GetSystemMetrics(metric) };
            assert_eq!(icon_width(icon), expected, "{size:?}");
            // SAFETY: the icon was loaded for this test and is destroyed once.
            unsafe {
                let _ = DestroyIcon(icon);
            }
        }
    }
}

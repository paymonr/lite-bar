//! The uninstall window: a fixed-size window showing `uninstall_page`.
//!
//! The page and the owner of the window share one `PageState` (`Rc<RefCell<..>>`). The
//! owner writes the status into it; the page reads it each frame and appends the user's
//! choices to a queue. The uninstall mode owns the message loop and drains the queue after
//! every message it handles (`uninstall::Session`).

use std::cell::RefCell;
use std::rc::Rc;

use lite_bar_core::ui::Ui;
use lite_bar_core::uninstall_ui::{
    uninstall_page, UninstallAction, UninstallInput, UninstallStatus, UNINSTALL_SIZE_DIP,
    UNINSTALL_TITLE,
};

use crate::ui_host::{UiApp, UiWindow};

/// What the window shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UninstallState {
    pub installed: bool,
    /// The settings and stats folder.
    pub data_dir: String,
    pub delete_data: bool,
    pub status: UninstallStatus,
}

/// What the owner and the page share.
struct PageState {
    shown: UninstallState,
    queue: Vec<UninstallAction>,
}

/// The `UiApp` of the window. Never pumps messages.
struct UninstallPage {
    state: Rc<RefCell<PageState>>,
}

impl UninstallPage {
    fn queue(&self, actions: Vec<UninstallAction>) {
        if actions.is_empty() {
            return;
        }
        match self.state.try_borrow_mut() {
            Ok(mut st) => st.queue.extend(actions),
            Err(_) => crate::log::error("uninstall window: state busy; a click was lost"),
        }
    }
}

impl UiApp for UninstallPage {
    fn frame(&mut self, ui: &mut Ui) {
        let actions = {
            let Ok(st) = self.state.try_borrow() else {
                return;
            };
            let input = UninstallInput {
                installed: st.shown.installed,
                data_dir: &st.shown.data_dir,
                delete_data: st.shown.delete_data,
                status: &st.shown.status,
            };
            uninstall_page(ui, &input)
        };
        self.queue(actions);
    }

    /// The X button (or any other close).
    fn closed(&mut self) {
        self.queue(vec![UninstallAction::Close]);
    }
}

/// `UiWindow::open_fixed` or its hidden test variant.
type OpenWindow = fn(&str, (f32, f32), Box<dyn UiApp>) -> windows::core::Result<UiWindow>;

/// The open window. Dropping it closes it.
pub struct UninstallUi {
    window: UiWindow,
    state: Rc<RefCell<PageState>>,
}

impl UninstallUi {
    pub fn open(shown: UninstallState) -> windows::core::Result<Self> {
        UninstallUi::create(shown, UiWindow::open_fixed)
    }

    #[cfg(test)]
    pub fn open_hidden(shown: UninstallState) -> windows::core::Result<Self> {
        UninstallUi::create(shown, UiWindow::open_fixed_hidden)
    }

    fn create(shown: UninstallState, open: OpenWindow) -> windows::core::Result<Self> {
        let state = Rc::new(RefCell::new(PageState {
            shown,
            queue: Vec::new(),
        }));
        let page = UninstallPage {
            state: state.clone(),
        };
        let window = open(UNINSTALL_TITLE, UNINSTALL_SIZE_DIP, Box::new(page))?;
        Ok(UninstallUi { window, state })
    }

    pub fn is_open(&self) -> bool {
        self.window.is_open()
    }

    /// Everything the page queued since the last call.
    pub fn take(&self) -> Vec<UninstallAction> {
        std::mem::take(&mut self.state.borrow_mut().queue)
    }

    /// Shows the uninstall's new status.
    pub fn set_status(&self, status: &UninstallStatus) {
        let mut st = self.state.borrow_mut();
        if st.shown.status == *status {
            return;
        }
        st.shown.status = status.clone();
        drop(st);
        self.window.request_redraw();
    }

    /// Shows the "delete my settings and stats" toggle's new value.
    pub fn set_delete_data(&self, on: bool) {
        let mut st = self.state.borrow_mut();
        if st.shown.delete_data == on {
            return;
        }
        st.shown.delete_data = on;
        drop(st);
        self.window.request_redraw();
    }

    #[cfg(test)]
    pub fn status(&self) -> UninstallStatus {
        self.state.borrow().shown.status.clone()
    }

    /// Queues `action` as if the page had.
    #[cfg(test)]
    pub fn push_for_test(&self, action: UninstallAction) {
        self.state.borrow_mut().queue.push(action);
    }

    /// Runs and paints one frame now.
    #[cfg(test)]
    pub fn paint(&self) {
        self.window.request_redraw();
        self.send(windows::Win32::UI::WindowsAndMessaging::WM_PAINT, 0, 0);
    }

    /// The window's close button.
    #[cfg(test)]
    pub fn send_close(&self) {
        self.send(windows::Win32::UI::WindowsAndMessaging::WM_CLOSE, 0, 0);
    }

    /// Where the text `text` was drawn by the last frame, in pixels (its top-left plus a
    /// few DIPs, so inside a button that carries it).
    #[cfg(test)]
    pub fn find_text_px(&self, text: &str) -> Option<(i32, i32)> {
        let s = self.window.scale();
        self.window.cmds().into_iter().find_map(|c| match c {
            lite_bar_core::ui::Cmd::Text { pos, text: t, .. } if t == text => Some((
                ((pos.x + 4.0) * s).round() as i32,
                ((pos.y + 4.0) * s).round() as i32,
            )),
            _ => None,
        })
    }

    /// A left click at pixel `(x, y)` of the client area.
    #[cfg(test)]
    pub fn click(&self, x: i32, y: i32) {
        use windows::Win32::UI::WindowsAndMessaging::{WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};
        let p = ((y as isize) << 16) | (x as isize & 0xFFFF);
        self.send(WM_MOUSEMOVE, 0, p);
        self.send(WM_LBUTTONDOWN, 1, p);
        self.send(WM_LBUTTONUP, 0, p);
    }

    #[cfg(test)]
    fn send(&self, msg: u32, wparam: usize, lparam: isize) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        // SAFETY: plain FFI on our own window.
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                self.window.hwnd(),
                msg,
                Some(WPARAM(wparam)),
                Some(LPARAM(lparam)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> UninstallState {
        UninstallState {
            installed: true,
            data_dir: r"C:\Users\x\AppData\Roaming\LiteBar".into(),
            delete_data: false,
            status: UninstallStatus::Confirm,
        }
    }

    #[test]
    fn cancel_click_queues_cancel() {
        let ui = UninstallUi::open_hidden(state()).unwrap();
        ui.paint();
        let (x, y) = ui.find_text_px("Cancel").expect("a Cancel button");
        ui.click(x, y);
        assert_eq!(ui.take(), vec![UninstallAction::Cancel]);
    }

    #[test]
    fn close_button_queues_close() {
        let ui = UninstallUi::open_hidden(state()).unwrap();
        ui.paint();
        ui.send_close();
        assert!(!ui.is_open());
        assert_eq!(ui.take(), vec![UninstallAction::Close]);
    }

    #[test]
    fn set_status_replaces_the_page() {
        let ui = UninstallUi::open_hidden(state()).unwrap();
        ui.paint();
        assert!(ui.find_text_px("Close").is_none());
        ui.set_status(&UninstallStatus::Done("Lite Bar was uninstalled.".into()));
        ui.paint();
        assert!(ui.find_text_px("Close").is_some());
    }
}

//! The welcome popup wired into the tray app: a fixed-size window showing
//! `move_prompt_frame`.
//!
//! Ownership follows the settings window (`settings_window.rs`): the app and the page share
//! one `PopupState` (`Rc<RefCell<..>>`). The app writes the move's status into it; the page
//! reads it each frame and appends the user's choices to the queue, posting
//! `WM_MOVE_PROMPT_ACTIONS` to the owner window when the queue goes from empty to non-empty.
//! The app drains the queue after every message it handles, so moving, saving and closing
//! all run in the app, on the owner window's thread.

use std::cell::RefCell;
use std::rc::Rc;

use lite_bar_core::install::MoveStatus;
use lite_bar_core::move_prompt_ui::{
    move_prompt_frame, MovePromptAction, MovePromptInput, POPUP_SIZE_DIP, TITLE,
};
use lite_bar_core::ui::Ui;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::log;
use crate::ui_host::{UiApp, UiWindow};

/// Posted to the owner window when the popup queued actions.
pub const WM_MOVE_PROMPT_ACTIONS: u32 = WM_APP + 7;

/// What the popup shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovePromptState {
    /// The toggle hotkey as shown to the user.
    pub hotkey: String,
    /// The folder the exe would move to.
    pub target_dir: String,
    pub status: MoveStatus,
}

/// What the app and the page share.
struct PopupState {
    shown: MovePromptState,
    queue: Vec<MovePromptAction>,
}

impl PopupState {
    /// Adds actions; returns true when the queue was empty (the app needs a nudge).
    fn push(&mut self, actions: impl IntoIterator<Item = MovePromptAction>) -> bool {
        let was_empty = self.queue.is_empty();
        self.queue.extend(actions);
        was_empty && !self.queue.is_empty()
    }
}

/// The `UiApp` of the popup. Never pumps messages.
struct MovePromptPage {
    state: Rc<RefCell<PopupState>>,
    owner: HWND,
}

impl MovePromptPage {
    fn nudge(&self) {
        // SAFETY: plain FFI; posting returns at once.
        unsafe {
            let _ = PostMessageW(
                Some(self.owner),
                WM_MOVE_PROMPT_ACTIONS,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }

    fn queue(&self, actions: Vec<MovePromptAction>) {
        if actions.is_empty() {
            return;
        }
        let Ok(mut st) = self.state.try_borrow_mut() else {
            log::error("move popup: state busy; a click was lost");
            return;
        };
        if st.push(actions) {
            self.nudge();
        }
    }
}

impl UiApp for MovePromptPage {
    fn frame(&mut self, ui: &mut Ui) {
        let actions = {
            let Ok(st) = self.state.try_borrow() else {
                return;
            };
            let input = MovePromptInput {
                hotkey: &st.shown.hotkey,
                target_dir: &st.shown.target_dir,
                status: &st.shown.status,
            };
            move_prompt_frame(ui, &input)
        };
        self.queue(actions);
    }

    /// The X button (or any other close) is "Ask me later".
    fn closed(&mut self) {
        self.queue(vec![MovePromptAction::Later]);
    }
}

/// `UiWindow::open_fixed` or its hidden test variant.
type OpenWindow = fn(&str, (f32, f32), Box<dyn UiApp>) -> windows::core::Result<UiWindow>;

/// The open popup. Dropping it closes the window.
pub struct MovePromptUi {
    window: UiWindow,
    state: Rc<RefCell<PopupState>>,
}

impl MovePromptUi {
    pub fn open(owner: HWND, shown: MovePromptState) -> windows::core::Result<Self> {
        MovePromptUi::create(owner, shown, UiWindow::open_fixed)
    }

    #[cfg(test)]
    pub fn open_hidden(owner: HWND, shown: MovePromptState) -> windows::core::Result<Self> {
        MovePromptUi::create(owner, shown, UiWindow::open_fixed_hidden)
    }

    fn create(
        owner: HWND,
        shown: MovePromptState,
        open: OpenWindow,
    ) -> windows::core::Result<Self> {
        let state = Rc::new(RefCell::new(PopupState {
            shown,
            queue: Vec::new(),
        }));
        let page = MovePromptPage {
            state: state.clone(),
            owner,
        };
        let window = open(TITLE, POPUP_SIZE_DIP, Box::new(page))?;
        Ok(MovePromptUi { window, state })
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.window.is_open()
    }

    pub fn hwnd(&self) -> HWND {
        self.window.hwnd()
    }

    /// Everything the page queued since the last call.
    pub fn take(&self) -> Vec<MovePromptAction> {
        std::mem::take(&mut self.state.borrow_mut().queue)
    }

    /// Shows the move's new status.
    pub fn set_status(&self, status: &MoveStatus) {
        let mut st = self.state.borrow_mut();
        if st.shown.status == *status {
            return;
        }
        st.shown.status = status.clone();
        drop(st);
        self.window.request_redraw();
    }

    #[cfg(test)]
    pub fn status(&self) -> MoveStatus {
        self.state.borrow().shown.status.clone()
    }

    /// Runs and paints one frame now.
    #[cfg(test)]
    pub fn paint(&self) {
        self.window.request_redraw();
        self.send(windows::Win32::UI::WindowsAndMessaging::WM_PAINT);
    }

    /// The window's close button.
    #[cfg(test)]
    pub fn send_close(&self) {
        self.send(windows::Win32::UI::WindowsAndMessaging::WM_CLOSE);
    }

    #[cfg(test)]
    fn send(&self, msg: u32) {
        // SAFETY: plain FFI on our own window.
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                self.window.hwnd(),
                msg,
                None,
                None,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::MsgWindow;

    fn state() -> MovePromptState {
        MovePromptState {
            hotkey: "Ctrl+Shift+F".into(),
            target_dir: r"C:\Program Files\Lite Bar".into(),
            status: MoveStatus::Idle,
        }
    }

    #[test]
    fn close_button_queues_later() {
        let owner = MsgWindow::new();
        let ui = MovePromptUi::open_hidden(owner.0, state()).unwrap();
        ui.paint();
        ui.send_close();
        assert!(!ui.is_open());
        assert_eq!(ui.take(), vec![MovePromptAction::Later]);
    }
}

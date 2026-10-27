//! The settings window wired into the tray app: pure helpers (hotkey switch, save debounce,
//! action queue) and the `UiApp` that shows `settings_frame`.

//!
//! Ownership: the window runs on the tray's thread, but `App` sits behind the `with_app`
//! borrow while it handles a message, so the page never touches `App`. Both share one
//! `WindowState` (`Rc<RefCell<..>>`): the app writes the settings snapshot, the game list
//! and the hotkey status into it; the page reads them each frame and appends its actions
//! to the queue. When the queue goes from empty to non-empty the page posts
//! `WM_SETTINGS_ACTIONS` to the owner window (posting never pumps messages), and the app
//! drains the queue after every message it handles. Every side effect (saving,
//! `RegisterHotKey`, `ShellExecuteW`) therefore runs in the app, after the frame returned,
//! on the owner window's thread.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use lite_bar_core::hotkey::Hotkey;
use lite_bar_core::settings::Settings;
use lite_bar_core::settings_edit::apply_change;
use lite_bar_core::settings_ui::{settings_frame, GameRow, SettingsView, UiAction, MIN_WINDOW_DIP};
use lite_bar_core::ui::{HotkeyCapture, Ui};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::log;
use crate::ui_host::{UiApp, UiWindow};

/// Posted to the owner window when the settings window queued actions.
pub const WM_SETTINGS_ACTIONS: u32 = WM_APP + 2;

/// Client size the window opens at, in DIPs.
const DEFAULT_SIZE_DIP: (f32, f32) = (900.0, 660.0);

/// Status shown when another application holds the proposed hotkey.
pub const HOTKEY_TAKEN: &str = "Already used by another app";

/// Settings are written this long after the last change.
pub const SAVE_DELAY: Duration = Duration::from_millis(300);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyDecision {
    /// The proposal is the current hotkey: nothing to do.
    Keep,
    /// The proposal is registered and becomes the hotkey.
    Switch,
    /// The proposal could not be registered; the old hotkey stays. Carries the status text.
    Reject(String),
}

pub fn hotkey_decision(proposed: Hotkey, current: Hotkey, register_ok: bool) -> HotkeyDecision {
    if proposed == current {
        HotkeyDecision::Keep
    } else if register_ok {
        HotkeyDecision::Switch
    } else {
        HotkeyDecision::Reject(HOTKEY_TAKEN.to_string())
    }
}

/// Switches from `current` to `proposed`: unregisters the current one, tries the proposal and,
/// when that fails, registers `current` again. A proposal equal to `current` makes no calls.
pub fn switch_hotkey(
    proposed: Hotkey,
    current: Hotkey,
    unregister: impl FnOnce(),
    mut register: impl FnMut(Hotkey) -> bool,
) -> HotkeyDecision {
    if proposed == current {
        return HotkeyDecision::Keep;
    }
    unregister();
    let ok = register(proposed);
    if !ok {
        register(current);
    }
    hotkey_decision(proposed, current, ok)
}

/// When the settings file is due to be written: `SAVE_DELAY` after the last change.
#[derive(Debug, Default)]
pub struct SaveDebounce {
    last_change: Option<Instant>,
}

impl SaveDebounce {
    pub fn change(&mut self, now: Instant) {
        self.last_change = Some(now);
    }

    pub fn due(&self, now: Instant) -> bool {
        self.last_change
            .is_some_and(|at| now.saturating_duration_since(at) >= SAVE_DELAY)
    }

    /// Ends the wait; returns whether a save was pending.
    pub fn flush(&mut self) -> bool {
        self.last_change.take().is_some()
    }
}

/// What the window queued for the app since the last drain.
#[derive(Debug, Default, PartialEq)]
pub struct Drained {
    /// In the order the window produced them.
    pub actions: Vec<UiAction>,
    /// The window closed; `actions` already holds its final edits.
    pub closed: bool,
}

/// The queue the window fills and the app drains.
#[derive(Debug, Default)]
pub struct ActionQueue {
    actions: Vec<UiAction>,
    closed: bool,
}

impl ActionQueue {
    /// Adds actions; returns true when the queue was empty (the app needs a nudge).
    pub fn push(&mut self, actions: impl IntoIterator<Item = UiAction>) -> bool {
        let was_empty = self.is_empty();
        self.actions.extend(actions);
        was_empty && !self.is_empty()
    }

    /// Records that the window closed, after its final actions; returns true when the queue
    /// was empty.
    pub fn close(&mut self, final_actions: Vec<UiAction>) -> bool {
        let was_empty = self.is_empty();
        self.actions.extend(final_actions);
        self.closed = true;
        was_empty
    }

    pub fn take(&mut self) -> Drained {
        Drained {
            actions: std::mem::take(&mut self.actions),
            closed: std::mem::take(&mut self.closed),
        }
    }

    fn is_empty(&self) -> bool {
        self.actions.is_empty() && !self.closed
    }
}

/// `rows` with each game's name override from the settings.
fn with_overrides(rows: &[GameRow], settings: &Settings) -> Vec<GameRow> {
    rows.iter()
        .map(|g| GameRow {
            override_name: settings.game_names.get(&g.exe_path).cloned(),
            ..g.clone()
        })
        .collect()
}

/// What the app and the page share.
#[derive(Default)]
struct WindowState {
    /// The settings the page shows (the app's, with `visible` = whether the overlay is on).
    settings: Settings,
    /// The played games, without overrides.
    games: Vec<GameRow>,
    /// Shown under the hotkey field.
    status: Option<String>,
    /// A rejected hotkey proposal, put into the capture field by the next frame.
    reject: Option<String>,
    queue: ActionQueue,
}

/// The `UiApp` of the settings window. Never pumps messages.
struct SettingsPage {
    state: Rc<RefCell<WindowState>>,
    view: SettingsView,
    owner: HWND,
}

impl SettingsPage {
    fn nudge(&self) {
        // SAFETY: plain FFI; posting returns at once.
        unsafe {
            let _ = PostMessageW(Some(self.owner), WM_SETTINGS_ACTIONS, WPARAM(0), LPARAM(0));
        }
    }
}

impl UiApp for SettingsPage {
    fn frame(&mut self, ui: &mut Ui) {
        let Ok(mut st) = self.state.try_borrow_mut() else {
            return;
        };
        if let Some(msg) = st.reject.take() {
            self.view.hotkey = HotkeyCapture::rejected(&msg);
        }
        let games = with_overrides(&st.games, &st.settings);
        let actions = settings_frame(
            ui,
            &mut self.view,
            &st.settings,
            &games,
            st.status.as_deref(),
        );
        // The page shows its own changes right away; the app applies them for real.
        for a in &actions {
            if let UiAction::Change(c) = a {
                apply_change(&mut st.settings, c);
            }
        }
        if st.queue.push(actions) {
            self.nudge();
        }
    }

    fn finish_edits(&mut self) {
        let last = self.view.finish_edits();
        let Ok(mut st) = self.state.try_borrow_mut() else {
            return;
        };
        for a in &last {
            if let UiAction::Change(c) = a {
                apply_change(&mut st.settings, c);
            }
        }
        if st.queue.push(last) {
            self.nudge();
        }
    }

    fn closed(&mut self) {
        let last = self.view.finish_edits();
        let Ok(mut st) = self.state.try_borrow_mut() else {
            log::error("settings window: state busy at close; last edits lost");
            return;
        };
        if st.queue.close(last) {
            self.nudge();
        }
    }
}

/// `UiWindow::open` or its hidden test variant.
type OpenWindow =
    fn(&str, (f32, f32), (f32, f32), Box<dyn UiApp>) -> windows::core::Result<UiWindow>;

/// The open settings window. Dropping it closes the window and frees everything it held.
pub struct SettingsUi {
    window: UiWindow,
    state: Rc<RefCell<WindowState>>,
}

impl SettingsUi {
    pub fn open(
        owner: HWND,
        settings: Settings,
        games: Vec<GameRow>,
    ) -> windows::core::Result<Self> {
        SettingsUi::create(owner, settings, games, UiWindow::open)
    }

    #[cfg(test)]
    pub fn open_hidden(
        owner: HWND,
        settings: Settings,
        games: Vec<GameRow>,
    ) -> windows::core::Result<Self> {
        SettingsUi::create(owner, settings, games, UiWindow::open_hidden)
    }

    fn create(
        owner: HWND,
        settings: Settings,
        games: Vec<GameRow>,
        open: OpenWindow,
    ) -> windows::core::Result<Self> {
        let state = Rc::new(RefCell::new(WindowState {
            settings,
            games,
            ..WindowState::default()
        }));
        let page = SettingsPage {
            state: state.clone(),
            view: SettingsView::default(),
            owner,
        };
        let window = open(
            "Lite Bar Settings",
            DEFAULT_SIZE_DIP,
            MIN_WINDOW_DIP,
            Box::new(page),
        )?;
        Ok(SettingsUi { window, state })
    }

    pub fn is_open(&self) -> bool {
        self.window.is_open()
    }

    pub fn focus(&self) {
        self.window.focus();
    }

    /// Everything the page queued since the last call.
    pub fn take(&self) -> Drained {
        self.state.borrow_mut().queue.take()
    }

    /// Shows new settings (after a change, a reload or a hotkey toggle).
    pub fn show(&self, settings: &Settings) {
        let mut st = self.state.borrow_mut();
        if st.settings == *settings {
            return;
        }
        st.settings = settings.clone();
        drop(st);
        self.window.request_redraw();
    }

    pub fn set_games(&self, games: Vec<GameRow>) {
        let mut st = self.state.borrow_mut();
        if st.games == games {
            return;
        }
        st.games = games;
        drop(st);
        self.window.request_redraw();
    }

    pub fn set_status(&self, status: Option<&str>) {
        self.state.borrow_mut().status = status.map(str::to_string);
        self.window.request_redraw();
    }

    /// Shows a refused hotkey in the capture field and the status line.
    pub fn reject_hotkey(&self, msg: &str) {
        {
            let mut st = self.state.borrow_mut();
            st.reject = Some(msg.to_string());
            st.status = Some(msg.to_string());
        }
        self.window.request_redraw();
    }

    /// Ends edits in progress (an unfinished game name) into the queue; the window stays open.
    pub fn finish_edits(&self) {
        self.window.finish_edits();
    }

    /// Closes the window; returns what it queued, including its final edits.
    pub fn close(self) -> Drained {
        self.window.close();
        self.take()
    }

    #[cfg(test)]
    pub fn queue(&self, a: UiAction) {
        self.state.borrow_mut().queue.push([a]);
    }

    #[cfg(test)]
    pub fn shown(&self) -> Settings {
        self.state.borrow().settings.clone()
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
    pub fn status(&self) -> Option<String> {
        self.state.borrow().status.clone()
    }

    /// A rejection waiting to be shown in the capture field.
    #[cfg(test)]
    pub fn pending_reject(&self) -> Option<String> {
        self.state.borrow().reject.clone()
    }

    /// Where the text `text` in `color` was drawn by the last frame, in pixels (its top-left
    /// plus a few DIPs).
    #[cfg(test)]
    pub fn find_text_px(&self, text: &str, color: lite_bar_core::ui::Rgba) -> Option<(i32, i32)> {
        let s = self.window.scale();
        self.window.cmds().into_iter().find_map(|c| match c {
            lite_bar_core::ui::Cmd::Text {
                pos,
                text: t,
                color: c,
                ..
            } if t == text && c == color => Some((
                ((pos.x + 4.0) * s).round() as i32,
                ((pos.y + 4.0) * s).round() as i32,
            )),
            _ => None,
        })
    }

    #[cfg(test)]
    pub fn scale(&self) -> f32 {
        self.window.scale()
    }

    #[cfg(test)]
    pub fn send_msg(&self, msg: u32, wparam: usize, lparam: isize) {
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

    #[cfg(test)]
    pub fn id(&self) -> usize {
        self.window.hwnd().0 as usize
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
    use std::cell::RefCell;

    use lite_bar_core::settings_edit::SettingsChange;

    use super::*;

    fn hk(s: &str) -> Hotkey {
        s.parse().unwrap()
    }

    #[test]
    fn same_hotkey_is_kept() {
        let h = hk("Ctrl+Alt+K");
        assert_eq!(hotkey_decision(h, h, false), HotkeyDecision::Keep);
        assert_eq!(hotkey_decision(h, h, true), HotkeyDecision::Keep);
    }

    #[test]
    fn free_hotkey_switches() {
        assert_eq!(
            hotkey_decision(hk("Ctrl+Alt+K"), hk("Ctrl+Alt+Shift+L"), true),
            HotkeyDecision::Switch
        );
    }

    #[test]
    fn taken_hotkey_is_rejected() {
        assert_eq!(
            hotkey_decision(hk("Ctrl+Alt+K"), hk("Ctrl+Alt+Shift+L"), false),
            HotkeyDecision::Reject("Already used by another app".to_string())
        );
    }

    #[test]
    fn same_hotkey_makes_no_registration_calls() {
        let h = hk("Ctrl+Alt+K");
        let calls = RefCell::new(Vec::new());
        let d = switch_hotkey(
            h,
            h,
            || calls.borrow_mut().push("unregister".to_string()),
            |x| {
                calls.borrow_mut().push(format!("register {x}"));
                true
            },
        );
        assert_eq!(d, HotkeyDecision::Keep);
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn free_combo_replaces_the_old_hotkey() {
        let calls = RefCell::new(Vec::new());
        let d = switch_hotkey(
            hk("Ctrl+Alt+K"),
            hk("Ctrl+Alt+Shift+L"),
            || calls.borrow_mut().push("unregister".to_string()),
            |x| {
                calls.borrow_mut().push(format!("register {x}"));
                true
            },
        );
        assert_eq!(d, HotkeyDecision::Switch);
        assert_eq!(*calls.borrow(), vec!["unregister", "register Ctrl+Alt+K"]);
    }

    #[test]
    fn taken_combo_keeps_old_hotkey() {
        let calls = RefCell::new(Vec::new());
        let taken = hk("Ctrl+Alt+K");
        let d = switch_hotkey(
            taken,
            hk("Ctrl+Alt+Shift+L"),
            || calls.borrow_mut().push("unregister".to_string()),
            |x| {
                calls.borrow_mut().push(format!("register {x}"));
                x != taken
            },
        );
        assert_eq!(d, HotkeyDecision::Reject(HOTKEY_TAKEN.to_string()));
        assert_eq!(
            *calls.borrow(),
            vec![
                "unregister",
                "register Ctrl+Alt+K",
                "register Ctrl+Alt+Shift+L"
            ]
        );
    }

    #[test]
    fn save_is_due_300ms_after_the_last_change() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut d = SaveDebounce::default();
        assert!(!d.due(ms(1000)), "nothing changed");
        d.change(t0);
        assert!(!d.due(ms(299)));
        assert!(d.due(ms(300)));
        // A later change pushes the save back.
        d.change(ms(200));
        assert!(!d.due(ms(450)));
        assert!(d.due(ms(500)));
    }

    #[test]
    fn flush_reports_a_pending_save_once() {
        let mut d = SaveDebounce::default();
        assert!(!d.flush());
        d.change(Instant::now());
        assert!(d.flush());
        assert!(!d.flush());
        assert!(!d.due(Instant::now() + Duration::from_secs(5)));
    }

    fn size(n: u16) -> UiAction {
        UiAction::Change(SettingsChange::CounterSize(n))
    }

    #[test]
    fn queue_drains_in_order_with_close_last() {
        let mut q = ActionQueue::default();
        assert!(q.push([size(20)]), "first push nudges the app");
        assert!(!q.push([size(21), UiAction::OpenStatsFolder]));
        assert!(!q.close(vec![size(22)]));
        assert_eq!(
            q.take(),
            Drained {
                actions: vec![size(20), size(21), UiAction::OpenStatsFolder, size(22)],
                closed: true,
            }
        );
        assert_eq!(q.take(), Drained::default());
    }

    #[test]
    fn overrides_come_from_the_settings() {
        let rows = vec![GameRow {
            exe_path: "javaw.exe".into(),
            stored_name: "Java".into(),
            override_name: None,
        }];
        let mut s = Settings::default();
        assert_eq!(with_overrides(&rows, &s), rows);
        s.game_names.insert("javaw.exe".into(), "Minecraft".into());
        assert_eq!(
            with_overrides(&rows, &s)[0].override_name.as_deref(),
            Some("Minecraft")
        );
    }

    #[test]
    fn empty_push_does_not_nudge() {
        let mut q = ActionQueue::default();
        assert!(!q.push(Vec::new()));
        assert!(q.close(Vec::new()), "closing always nudges");
    }
}

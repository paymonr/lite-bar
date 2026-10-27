//! State machine behind `Ui::hotkey_field`: click, press a combination, Esc cancels.

use std::collections::HashSet;

use super::input::{KeyPress, UiKey};
use crate::hotkey::{Hotkey, Key};

/// The modifier keys held at some moment.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

/// While capturing: a key-up of a non-modifier key whose key-down never arrived (another
/// app's `RegisterHotKey` took it) counts as pressing it with the modifiers held at key-up.
/// `pressed` holds the keys whose key-down this window saw.
pub fn capture_key_up(pressed: &HashSet<u16>, vk: u16, mods: Modifiers) -> Option<KeyPress> {
    if is_modifier_vk(vk) || pressed.contains(&vk) {
        return None;
    }
    Some(KeyPress {
        key: UiKey::Other(vk),
        ctrl: mods.ctrl,
        alt: mods.alt,
        shift: mods.shift,
        win: mods.win,
    })
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum HotkeyCapture {
    #[default]
    Idle,
    /// Listening for the next key combination.
    Waiting,
    /// Still listening after an invalid combination; shows why.
    Retry(String),
    /// The host refused the proposed hotkey; shows why, not listening, until clicked again.
    Error(String),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HotkeyFieldEvent {
    None,
    /// Capture began (a click, or Tab into the field). The host should release its own
    /// hotkey until capture ends, so pressing it reaches the field.
    Started,
    /// A valid combination was pressed; the host should try to register it and call
    /// `HotkeyCapture::rejected` if that fails.
    Proposed(Hotkey),
    Cancelled,
    Invalid(String),
}

/// Shown when the pressed key cannot be a hotkey key.
pub const UNSUPPORTED_KEY: &str = "Use a letter, digit or F-key";
/// Shown when a combination breaks the modifier rules.
pub const NEEDS_MODIFIER: &str = "Add Ctrl, Alt or Win (Shift alone only works with F-keys)";

/// Shift, Ctrl, Alt and the Win keys, including their left/right variants.
fn is_modifier_vk(vk: u16) -> bool {
    matches!(vk, 0x10..=0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

fn key_of(k: UiKey) -> Option<Key> {
    match k {
        UiKey::A => Some(Key::Letter('A')),
        UiKey::C => Some(Key::Letter('C')),
        UiKey::V => Some(Key::Letter('V')),
        UiKey::Other(vk) => Key::from_virtual_key(vk),
        _ => None,
    }
}

impl HotkeyCapture {
    /// The host could not use the proposed hotkey, e.g. "Already used by another app".
    pub fn rejected(msg: &str) -> HotkeyCapture {
        HotkeyCapture::Error(msg.to_string())
    }

    /// Whether key presses are being captured (`Waiting` or `Retry`).
    pub fn is_listening(&self) -> bool {
        matches!(self, HotkeyCapture::Waiting | HotkeyCapture::Retry(_))
    }

    pub fn start(&mut self) {
        *self = HotkeyCapture::Waiting;
    }

    /// Handles one key press. Only does anything while listening; an invalid
    /// combination keeps listening and shows why (`Retry`).
    pub fn feed(&mut self, k: &KeyPress) -> HotkeyFieldEvent {
        if !self.is_listening() {
            return HotkeyFieldEvent::None;
        }
        let ev = match k.key {
            UiKey::Esc => HotkeyFieldEvent::Cancelled,
            UiKey::Other(vk) if is_modifier_vk(vk) => return HotkeyFieldEvent::None,
            key => match key_of(key) {
                None => HotkeyFieldEvent::Invalid(UNSUPPORTED_KEY.to_string()),
                Some(key) => match Hotkey::new(k.ctrl, k.alt, k.shift, k.win, key) {
                    Ok(h) => HotkeyFieldEvent::Proposed(h),
                    Err(_) => HotkeyFieldEvent::Invalid(NEEDS_MODIFIER.to_string()),
                },
            },
        };
        *self = match &ev {
            HotkeyFieldEvent::Invalid(msg) => HotkeyCapture::Retry(msg.clone()),
            _ => HotkeyCapture::Idle,
        };
        ev
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL_ALT: Modifiers = Modifiers {
        ctrl: true,
        alt: true,
        shift: false,
        win: false,
    };

    #[test]
    fn key_up_without_its_key_down_is_a_press() {
        // Another app's RegisterHotKey ate the key-down of Ctrl+Alt+K.
        let pressed: HashSet<u16> = [0x11, 0x12].into_iter().collect();
        assert_eq!(
            capture_key_up(&pressed, 0x4B, CTRL_ALT),
            Some(KeyPress {
                key: UiKey::Other(0x4B),
                ctrl: true,
                alt: true,
                shift: false,
                win: false,
            })
        );
    }

    #[test]
    fn key_up_after_its_key_down_is_nothing() {
        let pressed: HashSet<u16> = [0x11, 0x12, 0x4B].into_iter().collect();
        assert_eq!(capture_key_up(&pressed, 0x4B, CTRL_ALT), None);
    }

    #[test]
    fn modifier_key_up_is_nothing() {
        let pressed = HashSet::new();
        for vk in [0x10, 0x11, 0x12, 0x5B, 0xA0, 0xA5] {
            assert_eq!(capture_key_up(&pressed, vk, CTRL_ALT), None, "{vk:#x}");
        }
    }

    #[test]
    fn key_up_press_proposes_through_feed() {
        let mut c = HotkeyCapture::Waiting;
        let k = capture_key_up(&HashSet::new(), 0x4B, CTRL_ALT).unwrap();
        assert_eq!(
            c.feed(&k),
            HotkeyFieldEvent::Proposed("Ctrl+Alt+K".parse().unwrap())
        );
    }

    fn kp(key: UiKey, ctrl: bool, alt: bool, shift: bool) -> KeyPress {
        KeyPress {
            key,
            ctrl,
            alt,
            shift,
            win: false,
        }
    }

    fn waiting() -> HotkeyCapture {
        let mut c = HotkeyCapture::Idle;
        c.start();
        assert_eq!(c, HotkeyCapture::Waiting);
        c
    }

    #[test]
    fn ctrl_alt_k_is_proposed() {
        let mut c = waiting();
        let ev = c.feed(&kp(UiKey::Other(0x4B), true, true, false));
        assert_eq!(
            ev,
            HotkeyFieldEvent::Proposed(
                Hotkey::new(true, true, false, false, Key::Letter('K')).unwrap()
            )
        );
        assert_eq!(c, HotkeyCapture::Idle);
    }

    #[test]
    fn named_letter_keys_count() {
        let mut c = waiting();
        assert!(matches!(
            c.feed(&kp(UiKey::A, true, false, false)),
            HotkeyFieldEvent::Proposed(h) if h.key == Key::Letter('A')
        ));
    }

    #[test]
    fn shift_a_is_invalid() {
        let mut c = waiting();
        let ev = c.feed(&kp(UiKey::A, false, false, true));
        assert_eq!(ev, HotkeyFieldEvent::Invalid(NEEDS_MODIFIER.to_string()));
        assert_eq!(c, HotkeyCapture::Retry(NEEDS_MODIFIER.to_string()));
    }

    #[test]
    fn invalid_keeps_listening() {
        let mut c = waiting();
        c.feed(&kp(UiKey::Other(0x20), true, false, false));
        assert!(matches!(
            c.feed(&kp(UiKey::Other(0x4B), true, true, false)),
            HotkeyFieldEvent::Proposed(_)
        ));
        assert_eq!(c, HotkeyCapture::Idle);
        let mut c = waiting();
        c.feed(&kp(UiKey::A, false, false, true));
        assert_eq!(
            c.feed(&kp(UiKey::Esc, false, false, false)),
            HotkeyFieldEvent::Cancelled
        );
    }

    #[test]
    fn rejected_does_not_listen() {
        let mut c = HotkeyCapture::rejected("Already used by another app");
        assert_eq!(
            c.feed(&kp(UiKey::Other(0x4B), true, true, false)),
            HotkeyFieldEvent::None
        );
        assert!(matches!(c, HotkeyCapture::Error(_)));
    }

    #[test]
    fn unsupported_key_is_invalid() {
        let mut c = waiting();
        // Ctrl+Space
        let ev = c.feed(&kp(UiKey::Other(0x20), true, false, false));
        assert_eq!(ev, HotkeyFieldEvent::Invalid(UNSUPPORTED_KEY.to_string()));
        let mut c = waiting();
        let ev = c.feed(&kp(UiKey::Tab, true, false, false));
        assert_eq!(ev, HotkeyFieldEvent::Invalid(UNSUPPORTED_KEY.to_string()));
    }

    #[test]
    fn modifier_keys_alone_keep_waiting() {
        let mut c = waiting();
        for vk in [
            0x10, 0x11, 0x12, 0x5B, 0x5C, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5,
        ] {
            assert_eq!(
                c.feed(&kp(UiKey::Other(vk), true, true, true)),
                HotkeyFieldEvent::None
            );
        }
        assert_eq!(c, HotkeyCapture::Waiting);
    }

    #[test]
    fn esc_cancels() {
        let mut c = waiting();
        assert_eq!(
            c.feed(&kp(UiKey::Esc, false, false, false)),
            HotkeyFieldEvent::Cancelled
        );
        assert_eq!(c, HotkeyCapture::Idle);
    }

    #[test]
    fn keys_are_ignored_unless_waiting() {
        let mut c = HotkeyCapture::Idle;
        assert_eq!(
            c.feed(&kp(UiKey::Other(0x4B), true, true, false)),
            HotkeyFieldEvent::None
        );
        assert_eq!(c, HotkeyCapture::Idle);
    }

    #[test]
    fn rejected_shows_the_error() {
        assert_eq!(
            HotkeyCapture::rejected("Already used by another app"),
            HotkeyCapture::Error("Already used by another app".to_string())
        );
    }
}

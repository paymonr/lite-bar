//! State machine behind `Ui::hotkey_field`: click, press a combination, Esc cancels.

use super::input::{KeyPress, UiKey};
use crate::hotkey::{Hotkey, Key};

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum HotkeyCapture {
    #[default]
    Idle,
    /// Listening for the next key combination.
    Waiting,
    /// Shows a message (invalid combination, or refused by the host) until clicked again.
    Error(String),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HotkeyFieldEvent {
    None,
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

    pub fn start(&mut self) {
        *self = HotkeyCapture::Waiting;
    }

    /// Handles one key press. Only does anything while `Waiting`.
    pub fn feed(&mut self, k: &KeyPress) -> HotkeyFieldEvent {
        if *self != HotkeyCapture::Waiting {
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
            HotkeyFieldEvent::Invalid(msg) => HotkeyCapture::Error(msg.clone()),
            _ => HotkeyCapture::Idle,
        };
        ev
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(c, HotkeyCapture::Error(NEEDS_MODIFIER.to_string()));
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

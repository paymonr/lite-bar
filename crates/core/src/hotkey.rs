//! Global hotkey parsing and formatting.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The non-modifier key of a hotkey.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    /// Uppercase `A`-`Z`.
    Letter(char),
    /// `0`-`9`.
    Digit(u8),
    /// Function key `F1`-`F24`.
    F(u8),
}

impl Key {
    /// The Windows virtual-key code for this key.
    pub fn virtual_key(self) -> u16 {
        match self {
            Key::Letter(c) => u16::from(c as u8),
            Key::Digit(d) => 0x30 + u16::from(d),
            Key::F(n) => 0x70 + u16::from(n) - 1,
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Letter(c) => write!(f, "{c}"),
            Key::Digit(d) => write!(f, "{d}"),
            Key::F(n) => write!(f, "F{n}"),
        }
    }
}

/// A modifier combination plus one key. At least one modifier is required.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub key: Key,
}

#[derive(thiserror::Error, Debug)]
pub enum HotkeyError {
    #[error("hotkey has no key")]
    NoKey,
    #[error("hotkey has more than one key")]
    MultipleKeys,
    #[error("hotkey needs at least one modifier (Ctrl, Alt, Shift or Win)")]
    NoModifier,
    #[error("unknown hotkey part: {0:?}")]
    UnknownPart(String),
}

fn parse_key(part: &str) -> Option<Key> {
    let mut chars = part.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        if first.is_ascii_alphabetic() {
            return Some(Key::Letter(first.to_ascii_uppercase()));
        }
        return first
            .is_ascii_digit()
            .then(|| Key::Digit(first as u8 - b'0'));
    }
    let number = part.strip_prefix(['f', 'F'])?;
    if !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    number
        .parse::<u8>()
        .ok()
        .filter(|n| (1..=24).contains(n))
        .map(Key::F)
}

impl FromStr for Hotkey {
    type Err = HotkeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (mut ctrl, mut alt, mut shift, mut win) = (false, false, false, false);
        let mut key = None;
        for part in s.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" => alt = true,
                "shift" => shift = true,
                "win" => win = true,
                _ => {
                    let parsed = parse_key(part)
                        .ok_or_else(|| HotkeyError::UnknownPart(part.to_string()))?;
                    if key.replace(parsed).is_some() {
                        return Err(HotkeyError::MultipleKeys);
                    }
                }
            }
        }
        let key = key.ok_or(HotkeyError::NoKey)?;
        if !(ctrl || alt || shift || win) {
            return Err(HotkeyError::NoModifier);
        }
        Ok(Hotkey {
            ctrl,
            alt,
            shift,
            win,
            key,
        })
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.win, "Win"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        write!(f, "{}", self.key)
    }
}

impl Serialize for Hotkey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Hotkey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_displays_canonically() {
        let h: Hotkey = " shift + ctrl + f ".parse().unwrap();
        assert_eq!(
            h,
            Hotkey {
                ctrl: true,
                alt: false,
                shift: true,
                win: false,
                key: Key::Letter('F')
            }
        );
        assert_eq!(h.to_string(), "Ctrl+Shift+F");
    }

    #[test]
    fn function_and_digit_keys() {
        assert_eq!("Alt+F12".parse::<Hotkey>().unwrap().key.virtual_key(), 0x7B);
        assert_eq!("Ctrl+7".parse::<Hotkey>().unwrap().key.virtual_key(), 0x37);
    }

    #[test]
    fn rejects_invalid() {
        assert!(matches!(
            "F".parse::<Hotkey>(),
            Err(HotkeyError::NoModifier)
        ));
        assert!(matches!(
            "Ctrl+Shift".parse::<Hotkey>(),
            Err(HotkeyError::NoKey)
        ));
        assert!(matches!(
            "Ctrl+A+B".parse::<Hotkey>(),
            Err(HotkeyError::MultipleKeys)
        ));
        assert!(matches!(
            "Ctrl+F25".parse::<Hotkey>(),
            Err(HotkeyError::UnknownPart(_))
        ));
    }
}

//! Single-line text editing model used by `Ui::text_field`.

use super::input::{KeyPress, UiKey};

/// System clipboard access, supplied by the host.
pub trait Clipboard {
    fn get(&mut self) -> Option<String>;
    fn set(&mut self, s: &str);
}

/// A single line of text with a caret. `caret` counts chars, not bytes.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct TextEdit {
    pub text: String,
    pub caret: usize,
    /// Set by Ctrl+A: the whole text is selected, so the next typed char, paste,
    /// Backspace or Delete replaces or clears it. Any caret move clears it.
    pub all_selected: bool,
    /// `Input::now_ms` of the last click or keystroke in the field; the caret blink
    /// restarts from it so the caret shows right after typing.
    pub last_input_ms: u64,
    /// How far the text is scrolled left in the field, in DIPs, so the caret stays visible.
    /// Kept by the text field.
    pub scroll: f32,
}

/// Ctrl without Alt: Windows reports AltGr as Ctrl+Alt, and AltGr combinations type
/// characters rather than run shortcuts.
fn shortcut(k: &KeyPress) -> bool {
    k.ctrl && !k.alt
}

impl TextEdit {
    /// Starts with `text` and the caret at its end.
    pub fn new(text: &str) -> TextEdit {
        TextEdit {
            text: text.to_string(),
            caret: text.chars().count(),
            all_selected: false,
            last_input_ms: 0,
            scroll: 0.0,
        }
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// Byte offset of char index `i` (clamped to the end).
    fn byte_at(&self, i: usize) -> usize {
        self.text
            .char_indices()
            .nth(i)
            .map_or(self.text.len(), |(b, _)| b)
    }

    /// Clears a whole-text selection, returning whether there was one.
    fn take_selection(&mut self) -> bool {
        if std::mem::take(&mut self.all_selected) {
            self.text.clear();
            self.caret = 0;
            true
        } else {
            false
        }
    }

    fn move_to(&mut self, caret: usize) {
        self.all_selected = false;
        self.caret = caret;
    }

    /// Applies an editing key. Returns whether the text changed.
    pub fn apply(&mut self, k: &KeyPress, clipboard: &mut dyn Clipboard) -> bool {
        self.caret = self.caret.min(self.len());
        match k.key {
            UiKey::A if shortcut(k) => {
                self.all_selected = true;
                false
            }
            UiKey::C if shortcut(k) => {
                if self.all_selected {
                    clipboard.set(&self.text);
                }
                false
            }
            UiKey::V if shortcut(k) => {
                let paste: String = clipboard
                    .get()
                    .unwrap_or_default()
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect();
                if paste.is_empty() {
                    return false;
                }
                self.take_selection();
                let b = self.byte_at(self.caret);
                self.text.insert_str(b, &paste);
                self.caret += paste.chars().count();
                true
            }
            UiKey::Backspace => {
                if self.take_selection() {
                    return true;
                }
                if self.caret == 0 {
                    return false;
                }
                self.caret -= 1;
                let b = self.byte_at(self.caret);
                self.text.remove(b);
                true
            }
            UiKey::Delete => {
                if self.take_selection() {
                    return true;
                }
                if self.caret >= self.len() {
                    return false;
                }
                let b = self.byte_at(self.caret);
                self.text.remove(b);
                true
            }
            UiKey::Left => {
                let to = if self.all_selected {
                    0
                } else {
                    self.caret.saturating_sub(1)
                };
                self.move_to(to);
                false
            }
            UiKey::Right => {
                let to = if self.all_selected {
                    self.len()
                } else {
                    (self.caret + 1).min(self.len())
                };
                self.move_to(to);
                false
            }
            UiKey::Home => {
                self.move_to(0);
                false
            }
            UiKey::End => {
                self.move_to(self.len());
                false
            }
            _ => false,
        }
    }

    /// Inserts a typed char at the caret. Control chars are ignored.
    pub fn insert(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        self.take_selection();
        self.caret = self.caret.min(self.len());
        let b = self.byte_at(self.caret);
        self.text.insert(b, c);
        self.caret += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeClipboard(Option<String>);
    impl Clipboard for FakeClipboard {
        fn get(&mut self) -> Option<String> {
            self.0.clone()
        }
        fn set(&mut self, s: &str) {
            self.0 = Some(s.to_string());
        }
    }

    fn key(key: UiKey) -> KeyPress {
        KeyPress {
            key,
            ctrl: false,
            alt: false,
            shift: false,
            win: false,
        }
    }

    fn ctrl(key: UiKey) -> KeyPress {
        KeyPress {
            ctrl: true,
            ..self::key(key)
        }
    }

    fn apply(e: &mut TextEdit, k: KeyPress) -> bool {
        e.apply(&k, &mut FakeClipboard::default())
    }

    #[test]
    fn new_puts_caret_at_end() {
        let e = TextEdit::new("héllo");
        assert_eq!(e.caret, 5);
        assert!(!e.all_selected);
    }

    #[test]
    fn typing_inserts_at_caret() {
        let mut e = TextEdit::new("ac");
        e.caret = 1;
        e.insert('b');
        assert_eq!(e.text, "abc");
        assert_eq!(e.caret, 2);
        e.insert('\u{8}');
        e.insert('\r');
        assert_eq!(e.text, "abc");
    }

    #[test]
    fn backspace_and_delete_at_the_ends() {
        let mut e = TextEdit::new("ab");
        assert!(!apply(&mut e, key(UiKey::Delete)));
        assert!(apply(&mut e, key(UiKey::Backspace)));
        assert_eq!((e.text.as_str(), e.caret), ("a", 1));
        e.caret = 0;
        assert!(!apply(&mut e, key(UiKey::Backspace)));
        assert!(apply(&mut e, key(UiKey::Delete)));
        assert_eq!((e.text.as_str(), e.caret), ("", 0));
        assert!(!apply(&mut e, key(UiKey::Backspace)));
        assert!(!apply(&mut e, key(UiKey::Delete)));
    }

    #[test]
    fn home_and_end_move_the_caret() {
        let mut e = TextEdit::new("abc");
        apply(&mut e, key(UiKey::Home));
        assert_eq!(e.caret, 0);
        apply(&mut e, key(UiKey::End));
        assert_eq!(e.caret, 3);
    }

    #[test]
    fn arrows_step_over_multibyte_chars() {
        let mut e = TextEdit::new("é日x");
        apply(&mut e, key(UiKey::Home));
        apply(&mut e, key(UiKey::Right));
        apply(&mut e, key(UiKey::Right));
        assert_eq!(e.caret, 2);
        e.insert('-');
        assert_eq!(e.text, "é日-x");
        apply(&mut e, key(UiKey::Left));
        apply(&mut e, key(UiKey::Left));
        assert!(apply(&mut e, key(UiKey::Backspace)));
        assert_eq!(e.text, "日-x");
        // Arrows stop at the ends.
        apply(&mut e, key(UiKey::Left));
        assert_eq!(e.caret, 0);
        for _ in 0..5 {
            apply(&mut e, key(UiKey::Right));
        }
        assert_eq!(e.caret, 3);
    }

    #[test]
    fn ctrl_a_then_typing_replaces_everything() {
        let mut e = TextEdit::new("old name");
        assert!(!apply(&mut e, ctrl(UiKey::A)));
        assert!(e.all_selected);
        e.insert('N');
        assert_eq!((e.text.as_str(), e.caret), ("N", 1));
        assert!(!e.all_selected);
    }

    #[test]
    fn ctrl_a_then_backspace_clears() {
        let mut e = TextEdit::new("old");
        apply(&mut e, ctrl(UiKey::A));
        assert!(apply(&mut e, key(UiKey::Backspace)));
        assert_eq!((e.text.as_str(), e.caret), ("", 0));
    }

    #[test]
    fn ctrl_alt_is_altgr_text_not_shortcuts() {
        let altgr = |key| KeyPress {
            alt: true,
            ..ctrl(key)
        };
        let mut e = TextEdit::new("abc");
        assert!(!apply(&mut e, altgr(UiKey::A)));
        assert!(!e.all_selected);
        e.all_selected = true;
        let mut cb = FakeClipboard(Some("pasted".to_string()));
        assert!(!e.apply(&altgr(UiKey::C), &mut cb));
        assert_eq!(cb.0.as_deref(), Some("pasted"));
        assert!(!e.apply(&altgr(UiKey::V), &mut cb));
        assert_eq!(e.text, "abc");
    }

    #[test]
    fn a_without_ctrl_is_not_select_all() {
        let mut e = TextEdit::new("x");
        apply(&mut e, key(UiKey::A));
        assert!(!e.all_selected);
    }

    #[test]
    fn caret_move_clears_selection() {
        let mut e = TextEdit::new("abc");
        apply(&mut e, ctrl(UiKey::A));
        // Left collapses the selection to the start, like other editors.
        apply(&mut e, key(UiKey::Left));
        assert!(!e.all_selected);
        e.insert('x');
        assert_eq!(e.text, "xabc");
    }

    #[test]
    fn paste_inserts_at_caret() {
        let mut e = TextEdit::new("ad");
        e.caret = 1;
        let mut cb = FakeClipboard(Some("b\r\nc".to_string()));
        assert!(e.apply(&ctrl(UiKey::V), &mut cb));
        // Line breaks are dropped: the field is a single line.
        assert_eq!((e.text.as_str(), e.caret), ("abcd", 3));
        let mut empty = FakeClipboard::default();
        assert!(!e.apply(&ctrl(UiKey::V), &mut empty));
    }

    #[test]
    fn paste_replaces_selection() {
        let mut e = TextEdit::new("old");
        apply(&mut e, ctrl(UiKey::A));
        let mut cb = FakeClipboard(Some("#5FA8FF".to_string()));
        assert!(e.apply(&ctrl(UiKey::V), &mut cb));
        assert_eq!(e.text, "#5FA8FF");
    }

    #[test]
    fn copy_puts_selection_on_clipboard() {
        let mut e = TextEdit::new("abc");
        let mut cb = FakeClipboard::default();
        e.apply(&ctrl(UiKey::C), &mut cb);
        assert_eq!(cb.0, None);
        e.apply(&ctrl(UiKey::A), &mut cb);
        assert!(!e.apply(&ctrl(UiKey::C), &mut cb));
        assert_eq!(cb.0.as_deref(), Some("abc"));
        assert_eq!(e.text, "abc");
    }

    #[test]
    fn enter_does_not_edit() {
        let mut e = TextEdit::new("abc");
        assert!(!apply(&mut e, key(UiKey::Enter)));
        assert_eq!(e.text, "abc");
    }

    #[test]
    fn caret_past_the_end_is_clamped() {
        let mut e = TextEdit::new("ab");
        e.text = "a".to_string();
        e.insert('b');
        assert_eq!((e.text.as_str(), e.caret), ("ab", 2));
    }
}

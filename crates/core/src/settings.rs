//! Tolerant settings: every invalid field falls back to its default with a warning.

use serde::{Deserialize, Serialize};

use crate::color::Rgb;
use crate::hotkey::{Hotkey, Key};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct OverlaySettings {
    pub visible: bool,
    pub corner: Corner,
    pub number_color: Rgb,
    pub label_color: Rgb,
    pub background_color: Rgb,
    /// Background opacity in percent, 0-100.
    pub background_opacity: u8,
    /// Number text size in pixels, 8-72.
    pub text_size: u16,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        OverlaySettings {
            visible: true,
            corner: Corner::TopLeft,
            number_color: Rgb {
                r: 0x7C,
                g: 0xFC,
                b: 0x7C,
            },
            label_color: Rgb {
                r: 0xFF,
                g: 0xFF,
                b: 0xFF,
            },
            background_color: Rgb { r: 0, g: 0, b: 0 },
            background_opacity: 55,
            text_size: 18,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct HotkeySettings {
    pub toggle_overlay: Hotkey,
}

impl Default for HotkeySettings {
    /// `Ctrl+Alt+Shift+F`: plain `Ctrl+Shift+F` would steal Find-in-Files from editors.
    fn default() -> Self {
        HotkeySettings {
            toggle_overlay: Hotkey {
                ctrl: true,
                alt: true,
                shift: true,
                win: false,
                key: Key::Letter('F'),
            },
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize)]
pub struct Settings {
    pub overlay: OverlaySettings,
    pub hotkeys: HotkeySettings,
}

/// Reads `section.key` as a `T`, pushing a warning and returning `None` when it is
/// present but invalid. A missing key returns `None` silently.
fn read_field<T: serde::de::DeserializeOwned>(
    section: &toml::Table,
    section_name: &str,
    key: &str,
    warnings: &mut Vec<String>,
) -> Option<T> {
    let value = section.get(key)?;
    match value.clone().try_into::<T>() {
        Ok(v) => Some(v),
        Err(err) => {
            warnings.push(format!(
                "{section_name}.{key}: {}; using default",
                err.message()
            ));
            None
        }
    }
}

/// Like `read_field` for an integer that must lie in `range`.
fn read_ranged<T: TryFrom<i64>>(
    section: &toml::Table,
    section_name: &str,
    key: &str,
    range: std::ops::RangeInclusive<i64>,
    warnings: &mut Vec<String>,
) -> Option<T> {
    let n: i64 = read_field(section, section_name, key, warnings)?;
    match T::try_from(n) {
        Ok(v) if range.contains(&n) => Some(v),
        _ => {
            warnings.push(format!(
                "{section_name}.{key}: {n} is out of range {}-{}; using default",
                range.start(),
                range.end()
            ));
            None
        }
    }
}

/// Returns the named sub-table, or an empty one (with a warning if it has the wrong type).
fn read_section(root: &toml::Table, name: &str, warnings: &mut Vec<String>) -> toml::Table {
    match root.get(name) {
        None => toml::Table::new(),
        Some(toml::Value::Table(t)) => t.clone(),
        Some(_) => {
            warnings.push(format!("{name}: expected a table; using defaults"));
            toml::Table::new()
        }
    }
}

/// Parses settings text, falling back to the default for every missing or invalid field.
/// Returns the settings plus one human-readable warning per ignored field.
pub fn parse_settings(text: &str) -> (Settings, Vec<String>) {
    let root: toml::Table = match text.parse() {
        Ok(t) => t,
        Err(err) => {
            return (
                Settings::default(),
                vec![format!("settings file is not valid TOML: {err}")],
            )
        }
    };
    let mut warnings = Vec::new();
    let mut settings = Settings::default();

    let overlay = read_section(&root, "overlay", &mut warnings);
    let o = &mut settings.overlay;
    if let Some(v) = read_field(&overlay, "overlay", "visible", &mut warnings) {
        o.visible = v;
    }
    if let Some(v) = read_field(&overlay, "overlay", "corner", &mut warnings) {
        o.corner = v;
    }
    if let Some(v) = read_field(&overlay, "overlay", "number_color", &mut warnings) {
        o.number_color = v;
    }
    if let Some(v) = read_field(&overlay, "overlay", "label_color", &mut warnings) {
        o.label_color = v;
    }
    if let Some(v) = read_field(&overlay, "overlay", "background_color", &mut warnings) {
        o.background_color = v;
    }
    if let Some(v) = read_ranged(
        &overlay,
        "overlay",
        "background_opacity",
        0..=100,
        &mut warnings,
    ) {
        o.background_opacity = v;
    }
    if let Some(v) = read_ranged(&overlay, "overlay", "text_size", 8..=72, &mut warnings) {
        o.text_size = v;
    }

    let hotkeys = read_section(&root, "hotkeys", &mut warnings);
    if let Some(v) = read_field(&hotkeys, "hotkeys", "toggle_overlay", &mut warnings) {
        settings.hotkeys.toggle_overlay = v;
    }

    (settings, warnings)
}

/// The default settings serialized as TOML, used to create a first-run settings file.
pub fn default_settings_toml() -> String {
    toml::to_string_pretty(&Settings::default()).expect("default settings serialize to TOML")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults_without_warnings() {
        let (s, w) = parse_settings("");
        assert_eq!(s, Settings::default());
        assert!(w.is_empty());
        assert_eq!(s.overlay.number_color.to_hex(), "#7CFC7C");
        assert_eq!(s.overlay.background_opacity, 55);
        assert_eq!(s.hotkeys.toggle_overlay.to_string(), "Ctrl+Alt+Shift+F");
    }

    #[test]
    fn invalid_field_falls_back_with_warning() {
        let (s, w) =
            parse_settings("[overlay]\nbackground_opacity = 300\nnumber_color = \"#FF0000\"\n");
        assert_eq!(s.overlay.background_opacity, 55);
        assert_eq!(s.overlay.number_color.to_hex(), "#FF0000");
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("overlay.background_opacity"));
    }

    #[test]
    fn bad_hotkey_falls_back_with_warning() {
        let (s, w) = parse_settings("[hotkeys]\ntoggle_overlay = \"F\"\n");
        assert_eq!(s.hotkeys.toggle_overlay.to_string(), "Ctrl+Alt+Shift+F");
        assert!(w[0].contains("hotkeys.toggle_overlay"));
    }

    #[test]
    fn garbage_file_gives_defaults() {
        let (s, w) = parse_settings("this is = = not toml [");
        assert_eq!(s, Settings::default());
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn default_toml_roundtrips() {
        let (s, w) = parse_settings(&default_settings_toml());
        assert_eq!(s, Settings::default());
        assert!(w.is_empty());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let (_, w) = parse_settings("[mixer]\nfoo = 1\n[overlay]\nmode = \"stacked\"\n");
        assert!(w.is_empty());
    }
}

//! Tolerant settings: every invalid field falls back to its default with a warning.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::color::Rgb;
use crate::hotkey::{Hotkey, Key};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Location {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

/// How the live FPS statistics are drawn next to the counter.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatsLayout {
    Off,
    Inline,
    Stacked,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct OverlaySettings {
    pub visible: bool,
    pub location: Location,
    pub stats: StatsLayout,
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
            location: Location::TopLeft,
            stats: StatsLayout::Inline,
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
    /// `Ctrl+Alt+Shift+L`: a plain `Ctrl+Shift+<letter>` would collide with editor shortcuts.
    fn default() -> Self {
        HotkeySettings {
            toggle_overlay: Hotkey {
                ctrl: true,
                alt: true,
                shift: true,
                win: false,
                key: Key::Letter('L'),
            },
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize)]
pub struct Settings {
    pub overlay: OverlaySettings,
    pub hotkeys: HotkeySettings,
    /// Display names for the history view. Keys are an exe file name or a full path.
    pub game_names: BTreeMap<String, String>,
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
    if let Some(v) = read_field(&overlay, "overlay", "location", &mut warnings) {
        o.location = v;
    }
    if let Some(v) = read_field(&overlay, "overlay", "stats", &mut warnings) {
        o.stats = v;
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

    let game_names = read_section(&root, "game_names", &mut warnings);
    for (key, value) in &game_names {
        match value {
            toml::Value::String(name) => {
                settings.game_names.insert(key.clone(), name.clone());
            }
            _ => warnings.push(format!(
                "game_names.{key}: expected a string; ignoring this entry"
            )),
        }
    }

    (settings, warnings)
}

/// The commented first-run settings file. It parses back to `Settings::default()`.
pub fn default_settings_toml() -> String {
    DEFAULT_SETTINGS_TOML.to_string()
}

const DEFAULT_SETTINGS_TOML: &str = r##"# Lite Bar settings. Edit and save, then choose "Reload settings" in the tray menu.
# A value that is missing or invalid falls back to its default.

[overlay]
# Show the FPS overlay at startup: true or false.
visible = true
# Where the counter sits: top-left, top-center, top-right, center-left,
# center-right, bottom-left, bottom-center or bottom-right.
location = "top-left"
# Live FPS statistics next to the counter: "off", "inline" (one line) or "stacked" (below).
stats = "inline"
# Color of the FPS number, as #RRGGBB.
number_color = "#7CFC7C"
# Color of the "FPS" label, as #RRGGBB.
label_color = "#FFFFFF"
# Color of the panel behind the text, as #RRGGBB.
background_color = "#000000"
# Opacity of that panel in percent, 0-100.
background_opacity = 55
# Height of the FPS number in pixels, 8-72.
text_size = 18

[hotkeys]
# Key that shows or hides the overlay: modifiers joined by "+", then one key,
# e.g. "Ctrl+Alt+Shift+L" or "Win+F9". A letter or digit needs Ctrl, Alt or Win;
# Shift alone is only enough for F1-F24.
toggle_overlay = "Ctrl+Alt+Shift+L"

[game_names]
# Display names shown in the history view. The key is an exe file name or a full path;
# the value is the name to show. Use single quotes for a path so backslashes need no escaping.
# "javaw.exe" = "Minecraft"
# 'C:\Games\Foo\foo.exe' = "Foo: Remastered"
"##;

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
        assert_eq!(s.hotkeys.toggle_overlay.to_string(), "Ctrl+Alt+Shift+L");
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
        assert_eq!(s.hotkeys.toggle_overlay.to_string(), "Ctrl+Alt+Shift+L");
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
    fn default_hotkey_is_ctrl_alt_shift_l() {
        assert_eq!(
            Settings::default().hotkeys.toggle_overlay.to_string(),
            "Ctrl+Alt+Shift+L"
        );
    }

    #[test]
    fn parses_every_location() {
        let cases = [
            ("top-left", Location::TopLeft),
            ("top-center", Location::TopCenter),
            ("top-right", Location::TopRight),
            ("center-left", Location::CenterLeft),
            ("center-right", Location::CenterRight),
            ("bottom-left", Location::BottomLeft),
            ("bottom-center", Location::BottomCenter),
            ("bottom-right", Location::BottomRight),
        ];
        for (text, want) in cases {
            let (s, w) = parse_settings(&format!("[overlay]\nlocation = \"{text}\"\n"));
            assert_eq!(s.overlay.location, want, "{text}");
            assert!(w.is_empty(), "{text}: {w:?}");
        }
        let (s, w) = parse_settings("[overlay]\nlocation = \"middle\"\n");
        assert_eq!(s.overlay.location, Location::TopLeft);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("overlay.location"));
    }

    #[test]
    fn old_corner_key_is_ignored() {
        let (s, w) = parse_settings("[overlay]\ncorner = \"bottom-right\"\n");
        assert_eq!(s.overlay.location, Location::TopLeft);
        assert!(w.is_empty());
    }

    #[test]
    fn parses_stats_layout() {
        assert_eq!(Settings::default().overlay.stats, StatsLayout::Inline);
        for (text, want) in [
            ("off", StatsLayout::Off),
            ("inline", StatsLayout::Inline),
            ("stacked", StatsLayout::Stacked),
        ] {
            let (s, w) = parse_settings(&format!("[overlay]\nstats = \"{text}\"\n"));
            assert_eq!(s.overlay.stats, want, "{text}");
            assert!(w.is_empty(), "{text}: {w:?}");
        }
        let (s, w) = parse_settings("[overlay]\nstats = \"big\"\n");
        assert_eq!(s.overlay.stats, StatsLayout::Inline);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("overlay.stats"));
    }

    #[test]
    fn parses_game_names() {
        let (s, w) = parse_settings(
            "[game_names]\n\"javaw.exe\" = \"Minecraft\"\n'C:\\Games\\x.exe' = \"X\"\nbad = 3\n",
        );
        assert_eq!(s.game_names.len(), 2);
        assert_eq!(s.game_names["javaw.exe"], "Minecraft");
        assert_eq!(s.game_names["C:\\Games\\x.exe"], "X");
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("game_names.bad"));
    }

    #[test]
    fn template_has_a_comment_for_every_key() {
        let template = default_settings_toml();
        let lines: Vec<&str> = template.lines().collect();
        for key in [
            "visible",
            "location",
            "stats",
            "number_color",
            "label_color",
            "background_color",
            "background_opacity",
            "text_size",
            "toggle_overlay",
        ] {
            let at = lines
                .iter()
                .position(|l| l.starts_with(&format!("{key} =")))
                .unwrap_or_else(|| panic!("no `{key} =` line"));
            assert!(
                at > 0 && lines[at - 1].starts_with('#'),
                "`{key}` has no comment line before it"
            );
        }
        for location in [
            "top-left",
            "top-center",
            "top-right",
            "center-left",
            "center-right",
            "bottom-left",
            "bottom-center",
            "bottom-right",
        ] {
            assert!(template.contains(location), "missing {location}");
        }
        assert!(template.contains("0-100"));
        assert!(template.contains("8-72"));
        assert!(lines.contains(&"[game_names]"));
        let example = lines
            .iter()
            .skip_while(|l| **l != "[game_names]")
            .any(|l| l.starts_with('#') && l.contains("= \""));
        assert!(example, "[game_names] needs a commented-out example entry");
        assert!(
            !lines
                .iter()
                .skip_while(|l| **l != "[game_names]")
                .skip(1)
                .any(|l| !l.starts_with('#') && !l.trim().is_empty()),
            "[game_names] must have no active entries"
        );
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let (_, w) = parse_settings("[mixer]\nfoo = 1\n[overlay]\nmode = \"stacked\"\n");
        assert!(w.is_empty());
    }
}

//! Format-preserving edits of the settings file.
//!
//! The settings window applies each change live and writes it into the file without losing the
//! user's comments: `edit_settings_text` touches only the values of the keys involved.

use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::color::Rgb;
use crate::hotkey::Hotkey;
use crate::settings::{
    default_settings_toml, Location, OverlaySettings, Settings, StatsLayout, MAX_SAVED_COLORS,
};

#[derive(Clone, Debug, PartialEq)]
pub enum SettingsChange {
    Visible(bool),
    Location(Location),
    Stats(StatsLayout),
    NumberColor(Rgb),
    LabelColor(Rgb),
    BackgroundColor(Rgb),
    BackgroundOpacity(u8),
    CounterSize(u16),
    TextSize(u16),
    SavedColors(Vec<Rgb>),
    /// Resets `overlay` and `hotkeys`; keeps `game_names` and `saved_colors`.
    RestoreDefaults,
    ToggleHotkey(Hotkey),
    /// `None` removes the override.
    GameName {
        exe_path: String,
        name: Option<String>,
    },
}

/// Applies `change` to the in-memory settings.
pub fn apply_change(settings: &mut Settings, change: &SettingsChange) {
    let o = &mut settings.overlay;
    match change {
        SettingsChange::Visible(v) => o.visible = *v,
        SettingsChange::Location(v) => o.location = *v,
        SettingsChange::Stats(v) => o.stats = *v,
        SettingsChange::NumberColor(v) => o.number_color = *v,
        SettingsChange::LabelColor(v) => o.label_color = *v,
        SettingsChange::BackgroundColor(v) => o.background_color = *v,
        SettingsChange::BackgroundOpacity(v) => o.background_opacity = *v,
        SettingsChange::CounterSize(v) => o.counter_size = *v,
        SettingsChange::TextSize(v) => o.text_size = *v,
        SettingsChange::SavedColors(v) => {
            settings.saved_colors = v.iter().take(MAX_SAVED_COLORS).copied().collect();
        }
        SettingsChange::RestoreDefaults => {
            let d = Settings::default();
            settings.overlay = d.overlay;
            settings.hotkeys = d.hotkeys;
        }
        SettingsChange::ToggleHotkey(v) => settings.hotkeys.toggle_overlay = *v,
        SettingsChange::GameName { exe_path, name } => match name {
            Some(n) => {
                settings.game_names.insert(exe_path.clone(), n.clone());
            }
            None => {
                settings.game_names.remove(exe_path);
            }
        },
    }
}

/// Returns `text` with `changes` applied, rewriting only the touched keys. An empty file starts
/// from the commented default template. Invalid TOML is an `Err` and nothing should be written.
pub fn edit_settings_text(text: &str, changes: &[SettingsChange]) -> Result<String, String> {
    let source = if text.trim().is_empty() {
        default_settings_toml()
    } else {
        text.to_string()
    };
    let mut doc: DocumentMut = source
        .parse()
        .map_err(|e| format!("settings file is not valid TOML: {e}"))?;
    for change in changes {
        edit_doc(&mut doc, change)?;
    }
    let out = doc.to_string();
    // toml_edit writes LF for every line it adds or re-emits; keep a CRLF file CRLF.
    if text.contains("\r\n") {
        Ok(out.replace("\r\n", "\n").replace('\n', "\r\n"))
    } else {
        Ok(out)
    }
}

fn edit_doc(doc: &mut DocumentMut, change: &SettingsChange) -> Result<(), String> {
    match change {
        SettingsChange::Visible(v) => set(doc, "overlay", "visible", (*v).into()),
        SettingsChange::Location(v) => set(doc, "overlay", "location", kebab(v).into()),
        SettingsChange::Stats(v) => set(doc, "overlay", "stats", lower(v).into()),
        SettingsChange::NumberColor(v) => set(doc, "overlay", "number_color", v.to_hex().into()),
        SettingsChange::LabelColor(v) => set(doc, "overlay", "label_color", v.to_hex().into()),
        SettingsChange::BackgroundColor(v) => {
            set(doc, "overlay", "background_color", v.to_hex().into())
        }
        SettingsChange::BackgroundOpacity(v) => {
            set(doc, "overlay", "background_opacity", i64::from(*v).into())
        }
        SettingsChange::CounterSize(v) => set(doc, "overlay", "counter_size", i64::from(*v).into()),
        SettingsChange::TextSize(v) => set(doc, "overlay", "text_size", i64::from(*v).into()),
        SettingsChange::SavedColors(v) => {
            let array: Array = v
                .iter()
                .take(MAX_SAVED_COLORS)
                .map(|c| c.to_hex())
                .collect();
            set(doc, "colors", "saved", array.into())
        }
        SettingsChange::RestoreDefaults => {
            let d = OverlaySettings::default();
            edit_doc(doc, &SettingsChange::Visible(d.visible))?;
            edit_doc(doc, &SettingsChange::Location(d.location))?;
            edit_doc(doc, &SettingsChange::Stats(d.stats))?;
            edit_doc(doc, &SettingsChange::NumberColor(d.number_color))?;
            edit_doc(doc, &SettingsChange::LabelColor(d.label_color))?;
            edit_doc(doc, &SettingsChange::BackgroundColor(d.background_color))?;
            edit_doc(
                doc,
                &SettingsChange::BackgroundOpacity(d.background_opacity),
            )?;
            edit_doc(doc, &SettingsChange::CounterSize(d.counter_size))?;
            edit_doc(doc, &SettingsChange::TextSize(d.text_size))?;
            let hotkey = Settings::default().hotkeys.toggle_overlay;
            edit_doc(doc, &SettingsChange::ToggleHotkey(hotkey))
        }
        SettingsChange::ToggleHotkey(v) => {
            set(doc, "hotkeys", "toggle_overlay", v.to_string().into())
        }
        SettingsChange::GameName { exe_path, name } => match name {
            Some(n) => set(doc, "game_names", exe_path, n.as_str().into()),
            None => {
                if let Some(table) = doc.get_mut("game_names").and_then(Item::as_table_mut) {
                    table.remove(exe_path);
                }
                Ok(())
            }
        },
    }
}

/// Sets `section.key`, creating the section or key when missing (new keys go last). An existing
/// value keeps its surrounding whitespace and trailing comment.
fn set(doc: &mut DocumentMut, section: &str, key: &str, mut value: Value) -> Result<(), String> {
    if doc.get(section).is_none() {
        doc.insert(section, Item::Table(Table::new()));
    }
    let table = doc
        .get_mut(section)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| format!("[{section}] is not a table"))?;
    if let Some(old) = table.get_mut(key).and_then(Item::as_value_mut) {
        *value.decor_mut() = old.decor().clone();
        *old = value;
    } else {
        table.insert(key, Item::Value(value));
    }
    Ok(())
}

fn kebab(l: &Location) -> &'static str {
    match l {
        Location::TopLeft => "top-left",
        Location::TopCenter => "top-center",
        Location::TopRight => "top-right",
        Location::CenterLeft => "center-left",
        Location::CenterRight => "center-right",
        Location::BottomLeft => "bottom-left",
        Location::BottomCenter => "bottom-center",
        Location::BottomRight => "bottom-right",
    }
}

fn lower(s: &StatsLayout) -> &'static str {
    match s {
        StatsLayout::Off => "off",
        StatsLayout::Inline => "inline",
        StatsLayout::Stacked => "stacked",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb;
    use crate::hotkey::Hotkey;
    use crate::settings::{default_settings_toml, parse_settings, Location, Settings, StatsLayout};

    fn rgb(hex: &str) -> Rgb {
        Rgb::parse_hex(hex).unwrap()
    }

    #[test]
    fn editing_keeps_comments_and_order() {
        let template = default_settings_toml();
        let out = edit_settings_text(&template, &[SettingsChange::BackgroundOpacity(80)]).unwrap();
        let (a, b): (Vec<&str>, Vec<&str>) = (template.lines().collect(), out.lines().collect());
        assert_eq!(a.len(), b.len());
        let diffs: Vec<_> = a.iter().zip(&b).filter(|(x, y)| x != y).collect();
        assert_eq!(
            diffs,
            [(&"background_opacity = 55", &"background_opacity = 80")]
        );
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn editing_one_key_leaves_other_lines_alone() {
        let input = "# my note\n[overlay]\nbackground_opacity = 300   # mine\nnumber_color = \"#000000\" # c\n\n# tail\n";
        let out =
            edit_settings_text(input, &[SettingsChange::NumberColor(rgb("#FF0000"))]).unwrap();
        assert_eq!(
            out,
            "# my note\n[overlay]\nbackground_opacity = 300   # mine\nnumber_color = \"#FF0000\" # c\n\n# tail\n"
        );
    }

    #[test]
    fn creates_missing_section_and_key() {
        let hk: Hotkey = "Win+F9".parse().unwrap();
        for input in ["", "[overlay]\nvisible = false\n"] {
            let out = edit_settings_text(input, &[SettingsChange::ToggleHotkey(hk)]).unwrap();
            let (s, w) = parse_settings(&out);
            assert_eq!(s.hotkeys.toggle_overlay, hk);
            assert!(w.is_empty(), "{w:?}");
        }
        let out = edit_settings_text(
            "[overlay]\nvisible = false\n\n[hotkeys]\n",
            &[SettingsChange::CounterSize(30)],
        )
        .unwrap();
        assert_eq!(
            out,
            "[overlay]\nvisible = false\ncounter_size = 30\n\n[hotkeys]\n"
        );
    }

    #[test]
    fn game_name_set_and_remove() {
        let set = |exe: &str, name: &str| SettingsChange::GameName {
            exe_path: exe.to_string(),
            name: Some(name.to_string()),
        };
        let out = edit_settings_text(
            "",
            &[set("C:\\Games\\x.exe", "X"), set("javaw.exe", "Minecraft")],
        )
        .unwrap();
        let (s, w) = parse_settings(&out);
        assert_eq!(s.game_names.len(), 2);
        assert!(w.is_empty(), "{w:?}");
        assert!(out.contains("'C:\\Games\\x.exe' = \"X\""), "{out}");
        let out = edit_settings_text(
            &out,
            &[SettingsChange::GameName {
                exe_path: "javaw.exe".into(),
                name: None,
            }],
        )
        .unwrap();
        let (s, _) = parse_settings(&out);
        assert_eq!(s.game_names.len(), 1);
        assert_eq!(s.game_names["C:\\Games\\x.exe"], "X");
    }

    #[test]
    fn invalid_toml_is_err() {
        assert!(edit_settings_text("this = = [", &[SettingsChange::Visible(true)]).is_err());
    }

    #[test]
    fn crlf_file_stays_crlf_and_untouched_lines_identical() {
        let input = "# mine\r\n[overlay]\r\nvisible = true # v\r\nnumber_color = \"#000000\"\r\n\r\n[hotkeys]\r\ntoggle_overlay = \"Win+F9\"\r\n";
        let out = edit_settings_text(
            input,
            &[
                SettingsChange::NumberColor(rgb("#FF0000")),
                SettingsChange::CounterSize(30),
            ],
        )
        .unwrap();
        assert_eq!(
            out,
            "# mine\r\n[overlay]\r\nvisible = true # v\r\nnumber_color = \"#FF0000\"\r\ncounter_size = 30\r\n\r\n[hotkeys]\r\ntoggle_overlay = \"Win+F9\"\r\n"
        );
        assert!(!out.replace("\r\n", "").contains('\n'));
    }

    fn rich_text() -> String {
        "# keep me\n[overlay]\nvisible = false\nlocation = \"bottom-right\"\nstats = \"off\"\nnumber_color = \"#010203\" # n\nlabel_color = \"#040506\"\nbackground_color = \"#070809\"\nbackground_opacity = 10\ncounter_size = 40\ntext_size = 20\n\n[hotkeys]\ntoggle_overlay = \"Win+F9\"\n\n[colors]\nsaved = [\"#112233\"]\n\n[game_names]\n\"javaw.exe\" = \"Minecraft\"\n'C:\\g.exe' = \"G\"\n".to_string()
    }

    #[test]
    fn restore_defaults_keeps_game_names_and_saved_colors() {
        let mut s = parse_settings(&rich_text()).0;
        apply_change(&mut s, &SettingsChange::RestoreDefaults);
        let d = Settings::default();
        assert_eq!(s.overlay, d.overlay);
        assert_eq!(s.hotkeys, d.hotkeys);
        assert_eq!(s.game_names.len(), 2);
        assert_eq!(s.saved_colors.len(), 1);

        let out = edit_settings_text(&rich_text(), &[SettingsChange::RestoreDefaults]).unwrap();
        let (t, w) = parse_settings(&out);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(t, s);
        assert!(out.starts_with("# keep me\n"));
        assert!(out.contains("number_color = \"#7CFC7C\" # n\n"), "{out}");
    }

    #[test]
    fn apply_change_matches_text_edit() {
        let hk: Hotkey = "Ctrl+Alt+F5".parse().unwrap();
        let changes = vec![
            SettingsChange::Visible(true),
            SettingsChange::Location(Location::CenterRight),
            SettingsChange::Stats(StatsLayout::Stacked),
            SettingsChange::NumberColor(rgb("#FF0000")),
            SettingsChange::LabelColor(rgb("#00FF00")),
            SettingsChange::BackgroundColor(rgb("#0000FF")),
            SettingsChange::BackgroundOpacity(77),
            SettingsChange::CounterSize(72),
            SettingsChange::TextSize(6),
            SettingsChange::SavedColors(vec![rgb("#ABCDEF"), rgb("#123456")]),
            SettingsChange::SavedColors(vec![]),
            SettingsChange::SavedColors((0..20).map(|i| Rgb { r: 0, g: 0, b: i }).collect()),
            SettingsChange::GameName {
                exe_path: "D:\\Games\\y\\y.exe".into(),
                name: Some("Y".into()),
            },
            SettingsChange::RestoreDefaults,
            SettingsChange::ToggleHotkey(hk),
            SettingsChange::GameName {
                exe_path: "new.exe".into(),
                name: Some("New".into()),
            },
            SettingsChange::GameName {
                exe_path: "javaw.exe".into(),
                name: Some("Renamed".into()),
            },
            SettingsChange::GameName {
                exe_path: "javaw.exe".into(),
                name: None,
            },
        ];
        let out_of_range = "[overlay]\nbackground_opacity = 300\ncounter_size = 1\n".to_string();
        for text in [
            rich_text(),
            default_settings_toml(),
            String::new(),
            out_of_range,
        ] {
            for c in &changes {
                let out = edit_settings_text(&text, std::slice::from_ref(c)).unwrap();
                let (edited, w) = parse_settings(&out);
                if !text.contains("= 300") {
                    assert!(w.is_empty(), "{c:?}: {w:?}");
                }
                let base = if text.is_empty() {
                    parse_settings(&default_settings_toml()).0
                } else {
                    parse_settings(&text).0
                };
                let mut applied = base;
                apply_change(&mut applied, c);
                assert_eq!(edited, applied, "{c:?}");
            }
        }
    }
}

//! Pure game-naming rules: launcher records (Steam .acf, Epic .item, GOG/Epic install dirs),
//! Xbox package manifests, generic-name blocklist, window-title cleanup and name picking.
//! Paths are Windows-style strings (backslashes), handled as plain strings so this runs anywhere.

use std::collections::BTreeMap;

/// The fields of a Steam `appmanifest_*.acf` that naming needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcfApp {
    pub name: String,
    pub installdir: String,
}

/// Inputs for [`pick_name`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameCandidates {
    pub exe_path: String,
    pub launcher: Option<String>,
    pub product_name: Option<String>,
    pub file_description: Option<String>,
    pub window_title: Option<String>,
}

// ---- Steam ----

/// Tokens of Valve's KeyValues text format.
enum KvToken {
    Str(String),
    Open,
    Close,
}

fn kv_tokens(text: &str) -> Vec<KvToken> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '{' => out.push(KvToken::Open),
            '}' => out.push(KvToken::Close),
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('"') => s.push('"'),
                            Some('\\') => s.push('\\'),
                            Some(other) => {
                                s.push('\\');
                                s.push(other);
                            }
                            None => s.push('\\'),
                        },
                        other => s.push(other),
                    }
                }
                out.push(KvToken::Str(s));
            }
            _ => {}
        }
    }
    out
}

/// Reads `name` and `installdir` from the top-level block of a Steam `appmanifest_*.acf`.
pub fn parse_acf(text: &str) -> Option<AcfApp> {
    let tokens = kv_tokens(text);
    let mut depth = 0usize;
    let mut name = None;
    let mut installdir = None;
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            KvToken::Open => depth += 1,
            KvToken::Close => depth = depth.saturating_sub(1),
            KvToken::Str(key) if depth == 1 => {
                if let Some(KvToken::Str(value)) = tokens.get(i + 1) {
                    if key.eq_ignore_ascii_case("name") {
                        name = Some(value.clone());
                    } else if key.eq_ignore_ascii_case("installdir") {
                        installdir = Some(value.clone());
                    }
                    i += 1;
                }
            }
            KvToken::Str(_) => {}
        }
        i += 1;
    }
    match (name, installdir) {
        (Some(name), Some(installdir)) if !name.is_empty() && !installdir.is_empty() => {
            Some(AcfApp { name, installdir })
        }
        _ => None,
    }
}

/// For a path inside `...\steamapps\common\<folder>\...` returns `(steamapps_dir, folder)`.
pub fn steam_location(exe_path: &str) -> Option<(String, String)> {
    const MARKER: &str = r"\steamapps\common\";
    // ASCII lowercasing and `/` -> `\` keep byte offsets identical to the original.
    let norm = exe_path.replace('/', r"\").to_ascii_lowercase();
    let at = norm.find(MARKER)?;
    let steamapps_end = at + MARKER.len() - r"\common\".len();
    let folder_start = at + MARKER.len();
    let folder_len = norm[folder_start..].find('\\')?;
    if folder_len == 0 {
        return None;
    }
    Some((
        exe_path[..steamapps_end].to_string(),
        exe_path[folder_start..folder_start + folder_len].to_string(),
    ))
}

// ---- Epic / GOG ----

/// Reads `(DisplayName, InstallLocation)` from an Epic `.item` JSON manifest.
pub fn parse_epic_item(json: &str) -> Option<(String, String)> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let field = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some((field("DisplayName")?, field("InstallLocation")?))
}

fn normalize_path(path: &str) -> String {
    path.replace('/', r"\").to_lowercase()
}

/// Name of the longest install dir that is a whole-directory prefix of `exe_path`.
pub fn match_install_dir(exe_path: &str, installs: &[(String, String)]) -> Option<String> {
    let exe = normalize_path(exe_path);
    installs
        .iter()
        .filter_map(|(name, dir)| {
            let dir = normalize_path(dir);
            let dir = dir.trim_end_matches('\\');
            if dir.is_empty() {
                return None;
            }
            let rest = exe.strip_prefix(dir)?;
            rest.starts_with('\\').then_some((dir.len(), name))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, name)| name.clone())
}

// ---- Xbox ----

fn decode_xml_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = [
            ("&amp;", '&'),
            ("&lt;", '<'),
            ("&gt;", '>'),
            ("&quot;", '"'),
            ("&apos;", '\''),
        ]
        .into_iter()
        .find(|(entity, _)| rest.starts_with(entity));
        match decoded {
            Some((entity, ch)) => {
                out.push(ch);
                rest = &rest[entity.len()..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn non_empty_decoded(raw: &str) -> Option<String> {
    let decoded = decode_xml_entities(raw);
    let trimmed = decoded.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// `DefaultDisplayName` of the `<ShellVisuals .../>` element in `MicrosoftGame.config`.
pub fn parse_microsoft_game_config(xml: &str) -> Option<String> {
    let tag_start = xml.find("<ShellVisuals")?;
    let tag = &xml[tag_start..];
    let tag = &tag[..tag.find('>')?];
    let mut search = tag;
    while let Some(at) = search.find("DefaultDisplayName") {
        let before_ok = search[..at]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace);
        let after = search[at + "DefaultDisplayName".len()..].trim_start();
        if before_ok {
            if let Some(value) = after.strip_prefix('=') {
                let value = value.trim_start();
                let quote = value.chars().next().filter(|q| *q == '"' || *q == '\'')?;
                let inner = &value[1..];
                let end = inner.find(quote)?;
                return non_empty_decoded(&inner[..end]);
            }
        }
        search = &search[at + "DefaultDisplayName".len()..];
    }
    None
}

/// `<DisplayName>` text of an `appxmanifest.xml`; `None` for unresolved `ms-resource:` values.
pub fn parse_appx_display_name(xml: &str) -> Option<String> {
    const OPEN: &str = "<DisplayName>";
    let start = xml.find(OPEN)? + OPEN.len();
    let len = xml[start..].find("</DisplayName>")?;
    let name = non_empty_decoded(&xml[start..start + len])?;
    (!name.to_ascii_lowercase().starts_with("ms-resource:")).then_some(name)
}

// ---- Generic names ----

/// True for names that identify an engine, runtime or placeholder rather than a game.
pub fn is_generic_product_name(name: &str) -> bool {
    const EXACT: &[&str] = &[
        "unity player",
        "unityplayer",
        "bootstrappackagedgame",
        "unreal engine",
        "ue4game",
        "ue5game",
        "openjdk platform binary",
        "java(tm) platform se binary",
        "launcher",
        "game",
        "application",
    ];
    let lower = name.trim().to_lowercase();
    lower.is_empty() || lower.starts_with("java(tm) platform") || EXACT.contains(&lower.as_str())
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// True for host executables whose own name says nothing about the game (Java).
pub fn is_generic_host(exe_path: &str) -> bool {
    let name = file_name(exe_path).to_ascii_lowercase();
    name == "java.exe" || name == "javaw.exe"
}

// ---- Window titles ----

fn is_version_token(token: &str) -> bool {
    let rest = token.strip_prefix(['v', 'V']).unwrap_or(token);
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let mut n = digits(rest);
    if n == 0 {
        return false;
    }
    loop {
        let tail = &rest[n..];
        match tail.strip_prefix('.') {
            Some(after) if digits(after) > 0 => n += 1 + digits(after),
            _ => break,
        }
    }
    let tail = &rest[n..];
    tail.is_empty() || (tail.starts_with('-') && tail.len() > 1)
}

/// Strips subtitle, dirty-marker and version tokens from a window title.
pub fn clean_window_title(title: &str) -> Option<String> {
    let cut = [" - ", " | "]
        .iter()
        .filter_map(|sep| title.find(sep))
        .min()
        .unwrap_or(title.len());
    let words: Vec<&str> = title[..cut]
        .split_whitespace()
        .map(|w| w.trim_end_matches('*'))
        .filter(|w| !w.is_empty() && !is_version_token(w))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

// ---- Keys and names ----

/// File name without a trailing `.exe` (case-insensitive).
pub fn exe_stem(exe_path: &str) -> String {
    let name = file_name(exe_path);
    let split = name.len().saturating_sub(4);
    match name.get(split..) {
        Some(ext) if name.len() >= 4 && ext.eq_ignore_ascii_case(".exe") => {
            name[..split].to_string()
        }
        _ => name.to_string(),
    }
}

/// Stable per-game key: lowercased path, plus the title name for generic hosts.
pub fn game_key(exe_path: &str, title_name: Option<&str>) -> String {
    let mut key = exe_path.replace('/', r"\").to_lowercase();
    if let Some(title) = title_name.filter(|t| !t.is_empty()) {
        if is_generic_host(exe_path) {
            key.push('|');
            key.push_str(&title.to_lowercase());
        }
    }
    key
}

fn usable(value: &Option<String>) -> Option<String> {
    let trimmed = value.as_deref()?.trim();
    (!is_generic_product_name(trimmed)).then(|| trimmed.to_string())
}

/// Launcher name, then version-info names (non-generic hosts only), then window title, then exe stem.
pub fn pick_name(c: &NameCandidates) -> String {
    let version_info = if is_generic_host(&c.exe_path) {
        None
    } else {
        usable(&c.product_name).or_else(|| usable(&c.file_description))
    };
    usable(&c.launcher)
        .or(version_info)
        .or_else(|| c.window_title.as_deref().and_then(clean_window_title))
        .unwrap_or_else(|| exe_stem(&c.exe_path))
}

/// User override by full path, then by exe file name (both case-insensitive), else the stored name.
pub fn display_name(
    stored_name: &str,
    exe_path: &str,
    overrides: &BTreeMap<String, String>,
) -> String {
    let lookup = |wanted: &str| {
        let wanted = normalize_path(wanted);
        overrides
            .iter()
            .find(|(key, _)| normalize_path(key) == wanted)
            .map(|(_, value)| value.clone())
    };
    lookup(exe_path)
        .or_else(|| lookup(file_name(exe_path)))
        .unwrap_or_else(|| stored_name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_acf ----

    #[test]
    fn acf_reads_name_and_installdir() {
        let text = "\"AppState\"\n{\n\t\"appid\"\t\t\"1245620\"\n\t\"name\"\t\t\"ELDEN RING\"\n\t\"installdir\"\t\t\"ELDEN RING\"\n}\n";
        assert_eq!(
            parse_acf(text),
            Some(AcfApp {
                name: "ELDEN RING".into(),
                installdir: "ELDEN RING".into()
            })
        );
    }

    #[test]
    fn acf_keys_are_case_insensitive() {
        let text = "\"AppState\"\n{\n\t\"NAME\"\t\"Hades\"\n\t\"InstallDir\"\t\"Hades\"\n}";
        let app = parse_acf(text).unwrap();
        assert_eq!(app.name, "Hades");
        assert_eq!(app.installdir, "Hades");
    }

    #[test]
    fn acf_decodes_escaped_quotes_and_backslashes() {
        let text = "\"AppState\"\n{\n\t\"name\"\t\"The \\\"Best\\\" Game\"\n\t\"installdir\"\t\"A\\\\B\"\n}";
        let app = parse_acf(text).unwrap();
        assert_eq!(app.name, "The \"Best\" Game");
        assert_eq!(app.installdir, "A\\B");
    }

    #[test]
    fn acf_ignores_nested_blocks() {
        let text = "\"AppState\"\n{\n\t\"UserConfig\"\n\t{\n\t\t\"name\"\t\"nested\"\n\t\t\"installdir\"\t\"nested\"\n\t}\n\t\"name\"\t\"Real\"\n\t\"installdir\"\t\"RealDir\"\n}";
        let app = parse_acf(text).unwrap();
        assert_eq!(app.name, "Real");
        assert_eq!(app.installdir, "RealDir");
    }

    #[test]
    fn acf_missing_field_or_garbage_is_none() {
        assert_eq!(
            parse_acf("\"AppState\"\n{\n\t\"name\"\t\"Only Name\"\n}"),
            None
        );
        assert_eq!(parse_acf("not an acf at all"), None);
        assert_eq!(parse_acf(""), None);
    }

    // ---- steam_location ----

    #[test]
    fn steam_location_finds_library_and_folder() {
        assert_eq!(
            steam_location(r"D:\SteamLibrary\steamapps\common\ELDEN RING\Game\eldenring.exe"),
            Some((
                r"D:\SteamLibrary\steamapps".to_string(),
                "ELDEN RING".to_string()
            ))
        );
    }

    #[test]
    fn steam_location_is_case_insensitive_and_accepts_forward_slashes() {
        assert_eq!(
            steam_location("C:/Program Files (x86)/Steam/SteamApps/Common/Hades/x64/Hades.exe"),
            Some((
                "C:/Program Files (x86)/Steam/SteamApps".to_string(),
                "Hades".to_string()
            ))
        );
    }

    #[test]
    fn steam_location_non_steam_path_is_none() {
        assert_eq!(steam_location(r"C:\Games\Foo\foo.exe"), None);
        // exe directly in common\ has no install folder
        assert_eq!(steam_location(r"D:\steamapps\common\foo.exe"), None);
    }

    // ---- parse_epic_item ----

    #[test]
    fn epic_item_reads_display_name_and_install_location() {
        let json = r#"{"FormatVersion":0,"DisplayName":"Fortnite","InstallLocation":"C:\\Program Files\\Epic Games\\Fortnite","AppName":"x"}"#;
        assert_eq!(
            parse_epic_item(json),
            Some((
                "Fortnite".to_string(),
                r"C:\Program Files\Epic Games\Fortnite".to_string()
            ))
        );
    }

    #[test]
    fn epic_item_missing_field_or_bad_json_is_none() {
        assert_eq!(parse_epic_item(r#"{"DisplayName":"X"}"#), None);
        assert_eq!(parse_epic_item(r#"{"InstallLocation":"C:\\X"}"#), None);
        assert_eq!(
            parse_epic_item(r#"{"DisplayName":1,"InstallLocation":"C:\\X"}"#),
            None
        );
        assert_eq!(parse_epic_item("nope"), None);
    }

    // ---- match_install_dir ----

    fn installs() -> Vec<(String, String)> {
        vec![
            ("Foo".to_string(), r"C:\Games\Foo".to_string()),
            ("Foo Bar".to_string(), r"C:\Games\Foo Bar".to_string()),
        ]
    }

    #[test]
    fn install_dir_picks_longest_prefix() {
        assert_eq!(
            match_install_dir("c:/games/foo bar/bin/x.exe", &installs()),
            Some("Foo Bar".to_string())
        );
    }

    #[test]
    fn install_dir_requires_whole_directory() {
        let only_foo = vec![("Foo".to_string(), r"C:\Games\Foo".to_string())];
        assert_eq!(match_install_dir(r"C:\Games\Foobar\x.exe", &only_foo), None);
    }

    #[test]
    fn install_dir_allows_trailing_separator_and_matches_nested() {
        let with_slash = vec![("Foo".to_string(), r"C:\Games\Foo\".to_string())];
        assert_eq!(
            match_install_dir(r"C:\GAMES\foo\bin\x.exe", &with_slash),
            Some("Foo".to_string())
        );
    }

    #[test]
    fn install_dir_empty_install_never_matches() {
        let empty = vec![("Nothing".to_string(), String::new())];
        assert_eq!(match_install_dir(r"C:\x.exe", &empty), None);
    }

    // ---- Xbox manifests ----

    #[test]
    fn game_config_reads_default_display_name() {
        let xml = r#"<?xml version="1.0"?>
<Game configVersion="1">
  <Identity Name="Foo" Publisher="CN=Bar" Version="1.0.0.0"/>
  <ShellVisuals DefaultDisplayName="Forza &amp; Friends &quot;X&quot;" PublisherDisplayName="Bar" Square150x150Logo="a.png"/>
</Game>"#;
        assert_eq!(
            parse_microsoft_game_config(xml),
            Some("Forza & Friends \"X\"".to_string())
        );
    }

    #[test]
    fn game_config_without_shell_visuals_is_none() {
        assert_eq!(parse_microsoft_game_config("<Game></Game>"), None);
        assert_eq!(
            parse_microsoft_game_config(r#"<Game><Identity DefaultDisplayName="x"/></Game>"#),
            None
        );
    }

    #[test]
    fn appx_reads_display_name_element_and_decodes_entities() {
        let xml = r#"<Package><Properties><DisplayName>Tom &lt;&amp;&gt; Jerry&apos;s</DisplayName></Properties></Package>"#;
        assert_eq!(
            parse_appx_display_name(xml),
            Some("Tom <&> Jerry's".to_string())
        );
    }

    #[test]
    fn appx_ms_resource_or_missing_is_none() {
        assert_eq!(
            parse_appx_display_name(
                "<Properties><DisplayName>ms-resource:AppName</DisplayName></Properties>"
            ),
            None
        );
        assert_eq!(parse_appx_display_name("<Package/>"), None);
        assert_eq!(
            parse_appx_display_name("<Properties><DisplayName>  </DisplayName></Properties>"),
            None
        );
    }

    // ---- generic names ----

    #[test]
    fn generic_product_names_are_detected() {
        for n in [
            "",
            "   ",
            "Unity Player",
            "UNITYPLAYER",
            "BootstrapPackagedGame",
            "Unreal Engine",
            "UE4Game",
            "ue5game",
            "OpenJDK Platform binary",
            "Java(TM) Platform SE binary",
            "Java(TM) Platform SE 8 U301",
            " Launcher ",
            "Game",
            "Application",
        ] {
            assert!(is_generic_product_name(n), "{n:?} should be generic");
        }
    }

    #[test]
    fn real_names_are_not_generic() {
        for n in [
            "Hollow Knight",
            "Game Dev Tycoon",
            "Launcher Of Doom",
            "Unity Player Pro",
        ] {
            assert!(!is_generic_product_name(n), "{n:?} should not be generic");
        }
    }

    #[test]
    fn generic_host_is_java() {
        assert!(is_generic_host(r"C:\Program Files\Java\bin\javaw.exe"));
        assert!(is_generic_host("C:/jdk/bin/JAVA.EXE"));
        assert!(!is_generic_host(r"C:\Games\javaw.exe.bak"));
        assert!(!is_generic_host(r"C:\Games\notjava.exe"));
    }

    // ---- window titles ----

    #[test]
    fn title_cuts_at_separator_and_drops_versions() {
        assert_eq!(
            clean_window_title("Minecraft* 1.21.1 - Multiplayer (3rd-party Server)"),
            Some("Minecraft".to_string())
        );
        assert_eq!(
            clean_window_title("Minecraft 1.20.4"),
            Some("Minecraft".to_string())
        );
        assert_eq!(clean_window_title("Hades II"), Some("Hades II".to_string()));
        assert_eq!(
            clean_window_title("Foo Bar | Main Menu"),
            Some("Foo Bar".to_string())
        );
    }

    #[test]
    fn title_version_tokens() {
        assert_eq!(clean_window_title("1.2.3"), None);
        assert_eq!(
            clean_window_title("Minecraft 1.21-pre1"),
            Some("Minecraft".to_string())
        );
        assert_eq!(
            clean_window_title("Minecraft v1.2"),
            Some("Minecraft".to_string())
        );
        assert_eq!(
            clean_window_title("Minecraft 1."),
            Some("Minecraft 1.".to_string())
        );
        assert_eq!(clean_window_title("Foo 1.0a"), Some("Foo 1.0a".to_string()));
        assert_eq!(clean_window_title("Foo 1.0-"), Some("Foo 1.0-".to_string()));
    }

    #[test]
    fn title_empty_or_separator_only_is_none() {
        assert_eq!(clean_window_title(""), None);
        assert_eq!(clean_window_title("   "), None);
        assert_eq!(clean_window_title(" - Something"), None);
        assert_eq!(clean_window_title("*"), None);
    }

    // ---- exe_stem / game_key ----

    #[test]
    fn exe_stem_strips_dirs_and_extension() {
        assert_eq!(exe_stem(r"C:\Games\Foo\Foo.EXE"), "Foo");
        assert_eq!(exe_stem("C:/Games/Foo/bar.exe"), "bar");
        assert_eq!(exe_stem("plain"), "plain");
        assert_eq!(exe_stem(r"C:\Games\tool.exe.bak"), "tool.exe.bak");
        assert_eq!(exe_stem(r"C:\Games\héllo.exe"), "héllo");
    }

    #[test]
    fn game_key_lowercases_and_normalises_separators() {
        assert_eq!(
            game_key("C:/Games/Foo/Foo.exe", None),
            r"c:\games\foo\foo.exe"
        );
        // title name is ignored for non-generic hosts
        assert_eq!(
            game_key(r"C:\Games\Foo.exe", Some("Whatever")),
            r"c:\games\foo.exe"
        );
    }

    #[test]
    fn game_key_for_generic_host_appends_title() {
        assert_eq!(
            game_key(r"C:\Java\bin\javaw.exe", Some("Minecraft")),
            r"c:\java\bin\javaw.exe|minecraft"
        );
        assert_eq!(
            game_key(r"C:\Java\bin\javaw.exe", None),
            r"c:\java\bin\javaw.exe"
        );
    }

    // ---- pick_name ----

    fn cands(exe: &str) -> NameCandidates {
        NameCandidates {
            exe_path: exe.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn pick_name_prefers_launcher() {
        let mut c = cands(r"C:\Games\x.exe");
        c.launcher = Some("ELDEN RING".into());
        c.product_name = Some("Other".into());
        c.window_title = Some("Window".into());
        assert_eq!(pick_name(&c), "ELDEN RING");
    }

    #[test]
    fn pick_name_skips_generic_product_for_description() {
        let mut c = cands(r"C:\Games\Hollow Knight\hollow_knight.exe");
        c.product_name = Some("Unity Player".into());
        c.file_description = Some("Hollow Knight".into());
        assert_eq!(pick_name(&c), "Hollow Knight");
    }

    #[test]
    fn pick_name_uses_product_name_before_description() {
        let mut c = cands(r"C:\Games\x.exe");
        c.product_name = Some("Product".into());
        c.file_description = Some("Description".into());
        assert_eq!(pick_name(&c), "Product");
    }

    #[test]
    fn pick_name_java_host_uses_window_title() {
        let mut c = cands(r"C:\Program Files\Java\bin\javaw.exe");
        c.product_name = Some("OpenJDK Platform binary".into());
        c.file_description = Some("OpenJDK Platform binary".into());
        c.window_title = Some("Minecraft 1.21.1".into());
        assert_eq!(pick_name(&c), "Minecraft");
    }

    #[test]
    fn pick_name_generic_host_skips_even_non_generic_version_info() {
        let mut c = cands(r"C:\Java\bin\javaw.exe");
        c.product_name = Some("Some Oracle Product".into());
        c.window_title = Some("Minecraft".into());
        assert_eq!(pick_name(&c), "Minecraft");
    }

    #[test]
    fn pick_name_generic_host_still_honours_launcher() {
        let mut c = cands(r"C:\Java\bin\javaw.exe");
        c.launcher = Some("Some Java Game".into());
        c.window_title = Some("Minecraft".into());
        assert_eq!(pick_name(&c), "Some Java Game");
    }

    #[test]
    fn pick_name_falls_back_to_window_title_then_exe_stem() {
        let mut c = cands(r"C:\Games\cool_game.exe");
        c.product_name = Some("Launcher".into());
        c.window_title = Some("Cool Game - Level 3".into());
        assert_eq!(pick_name(&c), "Cool Game");

        let c = cands(r"C:\Games\cool_game.exe");
        assert_eq!(pick_name(&c), "cool_game");

        let mut c = cands(r"C:\Games\cool_game.exe");
        c.window_title = Some("1.2.3".into());
        assert_eq!(pick_name(&c), "cool_game");
    }

    // ---- display_name ----

    fn overrides() -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert("game.exe".to_string(), "By File".to_string());
        m.insert(r"C:\Games\Game.exe".to_string(), "By Path".to_string());
        m
    }

    #[test]
    fn display_name_full_path_override_beats_file_name_override() {
        assert_eq!(
            display_name("Stored", r"c:/games/GAME.EXE", &overrides()),
            "By Path"
        );
    }

    #[test]
    fn display_name_file_name_override_then_stored() {
        assert_eq!(
            display_name("Stored", r"D:\Other\Game.exe", &overrides()),
            "By File"
        );
        assert_eq!(
            display_name("Stored", r"D:\Other\Different.exe", &overrides()),
            "Stored"
        );
        assert_eq!(
            display_name("Stored", r"D:\Other\Game.exe", &BTreeMap::new()),
            "Stored"
        );
    }
}

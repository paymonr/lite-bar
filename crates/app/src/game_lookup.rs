//! Resolves a friendly game name for an exe from launcher records and version info.
//!
//! Every source is best effort: a missing, unreadable or odd file just yields `None`.

use std::path::{Path, PathBuf};

use lite_bar_core::game_name::{
    match_install_dir, parse_acf, parse_appx_display_name, parse_epic_item,
    parse_microsoft_game_config, pick_name, steam_location, NameCandidates,
};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
    RRF_RT_REG_SZ,
};

/// Launcher manifests are scanned up to this many files per directory or registry key.
const MAX_MANIFESTS: usize = 500;
/// Files larger than this are not launcher manifests.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// The exe's own folder plus this many parents are searched for Xbox package files.
const XBOX_PARENT_LEVELS: usize = 3;

/// Best name for the program at `exe_path`: launcher record, then version info, then the
/// window title, then the exe file name.
pub fn resolve_name(exe_path: &str, window_title: Option<&str>) -> String {
    let launcher = steam_name(exe_path)
        .or_else(|| epic_name(exe_path))
        .or_else(|| gog_name(exe_path))
        .or_else(|| xbox_name(exe_path));
    let (product_name, file_description) = version_names(exe_path);
    pick_name(&NameCandidates {
        exe_path: exe_path.to_string(),
        launcher,
        product_name,
        file_description,
        window_title: window_title.map(str::to_string),
    })
}

// ---- files ----

/// Reads a launcher file as text: UTF-8 (BOM stripped) or UTF-16 LE with a BOM.
fn read_text(path: &Path) -> Option<String> {
    if std::fs::metadata(path).ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if let Some(wide) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = wide
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        return Some(String::from_utf16_lossy(&units));
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// Files in `dir` whose name has the given prefix and extension (case-insensitive), capped.
fn manifest_files(dir: &Path, prefix: &str, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let ext_ok = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(extension));
            ext_ok && name.starts_with(prefix)
        })
        .take(MAX_MANIFESTS)
        .collect()
}

// ---- Steam ----

fn steam_name(exe_path: &str) -> Option<String> {
    let (steamapps, folder) = steam_location(exe_path)?;
    let folder = folder.to_lowercase();
    manifest_files(Path::new(&steamapps), "appmanifest_", "acf")
        .iter()
        .filter_map(|path| parse_acf(&read_text(path)?))
        .find(|app| app.installdir.to_lowercase() == folder)
        .map(|app| app.name)
}

// ---- Epic ----

fn epic_name(exe_path: &str) -> Option<String> {
    let program_data = std::env::var_os("ProgramData")?;
    let manifests = Path::new(&program_data).join(r"Epic\EpicGamesLauncher\Data\Manifests");
    epic_name_in(&manifests, exe_path)
}

fn epic_name_in(manifest_dir: &Path, exe_path: &str) -> Option<String> {
    manifest_files(manifest_dir, "", "item")
        .iter()
        .filter_map(|path| parse_epic_item(&read_text(path)?))
        .find_map(|install| match_install_dir(exe_path, &[install]))
}

// ---- GOG ----

const GOG_GAMES_KEY: PCWSTR = w!(r"SOFTWARE\WOW6432Node\GOG.com\Games");

/// Closes the registry key on drop.
struct RegKey(HKEY);

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: the key was opened by `RegOpenKeyExW` and is closed exactly once, here.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn reg_string(key: HKEY, subkey: &HSTRING, value: PCWSTR) -> Option<String> {
    let mut bytes = 0u32;
    // SAFETY: size query only (no data pointer); `bytes` is a live u32.
    let status = unsafe {
        RegGetValueW(
            key,
            subkey,
            value,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS || bytes < 2 {
        return None;
    }
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
    // SAFETY: `buf` has at least `bytes` writable bytes; `bytes` is updated to the bytes written.
    let status = unsafe {
        RegGetValueW(
            key,
            subkey,
            value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let units = &buf[..(bytes as usize / 2).min(buf.len())];
    let text = String::from_utf16_lossy(units);
    let text = text.trim_end_matches('\0');
    (!text.is_empty()).then(|| text.to_string())
}

fn gog_installs() -> Vec<(String, String)> {
    let mut root = HKEY::default();
    // SAFETY: plain FFI call; `root` is a live out-parameter.
    let status =
        unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, GOG_GAMES_KEY, None, KEY_READ, &mut root) };
    if status != ERROR_SUCCESS {
        return Vec::new();
    }
    let root = RegKey(root);
    let mut installs = Vec::new();
    for index in 0..MAX_MANIFESTS as u32 {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        // SAFETY: `name` has `len` writable u16s; the optional out-parameters are omitted.
        let status = unsafe {
            RegEnumKeyExW(
                root.0,
                index,
                Some(windows::core::PWSTR(name.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            break;
        }
        let subkey = HSTRING::from_wide(&name[..len as usize]);
        let path = reg_string(root.0, &subkey, w!("path"));
        let game = reg_string(root.0, &subkey, w!("gameName"));
        if let (Some(game), Some(path)) = (game, path) {
            installs.push((game, path));
        }
    }
    installs
}

fn gog_name(exe_path: &str) -> Option<String> {
    match_install_dir(exe_path, &gog_installs())
}

// ---- Xbox ----

fn xbox_name(exe_path: &str) -> Option<String> {
    Path::new(exe_path)
        .ancestors()
        .skip(1)
        .take(XBOX_PARENT_LEVELS + 1)
        .find_map(|dir| {
            read_text(&dir.join("MicrosoftGame.config"))
                .and_then(|xml| parse_microsoft_game_config(&xml))
                .or_else(|| {
                    read_text(&dir.join("appxmanifest.xml"))
                        .and_then(|xml| parse_appx_display_name(&xml))
                })
        })
}

// ---- version info ----

/// `(ProductName, FileDescription)` of the exe's version resource (first translation).
fn version_names(exe_path: &str) -> (Option<String>, Option<String>) {
    let Some(block) = VersionBlock::load(exe_path) else {
        return (None, None);
    };
    let Some(prefix) = block.translation_prefix() else {
        return (None, None);
    };
    (
        block.string(&prefix, "ProductName"),
        block.string(&prefix, "FileDescription"),
    )
}

/// A loaded version resource. Backed by `u32`s so pointers into it are suitably aligned.
struct VersionBlock(Vec<u32>);

impl VersionBlock {
    fn load(exe_path: &str) -> Option<VersionBlock> {
        let path = HSTRING::from(exe_path);
        // SAFETY: `path` is a valid NUL-terminated wide string.
        let size = unsafe { GetFileVersionInfoSizeW(&path, None) };
        if size == 0 {
            return None;
        }
        let mut data = vec![0u32; (size as usize).div_ceil(4)];
        // SAFETY: `data` holds at least `size` writable bytes.
        unsafe { GetFileVersionInfoW(&path, None, size, data.as_mut_ptr().cast()) }.ok()?;
        Some(VersionBlock(data))
    }

    /// Returns the pointer and length (in elements) `VerQueryValueW` reports for `sub_block`.
    fn query(&self, sub_block: &HSTRING) -> Option<(*const u16, usize)> {
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        // SAFETY: the block was filled by `GetFileVersionInfoW`; the result points into it.
        let found =
            unsafe { VerQueryValueW(self.0.as_ptr().cast(), sub_block, &mut ptr, &mut len) };
        (found.as_bool() && !ptr.is_null() && len > 0)
            .then_some((ptr.cast_const().cast(), len as usize))
    }

    /// `\StringFileInfo\<lang><codepage>\` for the first entry of `\VarFileInfo\Translation`.
    fn translation_prefix(&self) -> Option<String> {
        let (ptr, len) = self.query(&HSTRING::from(r"\VarFileInfo\Translation"))?;
        // The value is an array of (language, codepage) u16 pairs; `len` is in bytes here.
        if len < 4 {
            return None;
        }
        // SAFETY: `query` returned at least 4 readable bytes (two u16s) inside the block.
        let (lang, codepage) = unsafe { (ptr.read_unaligned(), ptr.add(1).read_unaligned()) };
        Some(format!(r"\StringFileInfo\{lang:04x}{codepage:04x}\"))
    }

    fn string(&self, prefix: &str, name: &str) -> Option<String> {
        let (ptr, len) = self.query(&HSTRING::from(format!("{prefix}{name}")))?;
        // SAFETY: `query` returned `len` readable u16 units (including the NUL) inside the block.
        let units: Vec<u16> = (0..len)
            .map(|i| unsafe { ptr.add(i).read_unaligned() })
            .collect();
        let text = String::from_utf16_lossy(&units);
        let text = text.trim_end_matches('\0').trim();
        (!text.is_empty()).then(|| text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    const ACF: &str =
        "\"AppState\"\n{\n\t\"appid\"\t\"1\"\n\t\"name\"\t\"Some Game: Deluxe\"\n\t\"installdir\"\t\"some game\"\n}\n";

    #[test]
    fn resolves_steam_name_from_appmanifest() {
        let tmp = TempDir::new("steam");
        let steamapps = tmp.path().join("steamapps");
        write(&steamapps.join("appmanifest_1.acf"), ACF.as_bytes());
        let exe = steamapps.join(r"common\Some Game\bin\x.exe");
        let name = resolve_name(&exe.to_string_lossy(), Some("Window - Title"));
        assert_eq!(name, "Some Game: Deluxe");
    }

    #[test]
    fn steam_manifest_with_utf8_bom_is_read() {
        let tmp = TempDir::new("steam-bom");
        let steamapps = tmp.path().join("steamapps");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(ACF.as_bytes());
        write(&steamapps.join("appmanifest_1.acf"), &bytes);
        let exe = steamapps.join(r"common\Some Game\x.exe");
        assert_eq!(
            resolve_name(&exe.to_string_lossy(), None),
            "Some Game: Deluxe"
        );
    }

    #[test]
    fn resolves_xbox_name_from_microsoft_game_config() {
        let tmp = TempDir::new("xbox");
        let root = tmp.path().join("XboxGames").join("Halo");
        let config = r#"<Game><ShellVisuals DefaultDisplayName="Halo Test &amp; Co"/></Game>"#;
        write(
            &root.join("Content").join("MicrosoftGame.config"),
            config.as_bytes(),
        );
        let exe = root.join(r"Content\bin\halo.exe");
        assert_eq!(resolve_name(&exe.to_string_lossy(), None), "Halo Test & Co");
    }

    #[test]
    fn xbox_appxmanifest_in_utf16_is_read() {
        let tmp = TempDir::new("xbox-utf16");
        let root = tmp.path().join("Game");
        let xml =
            "<Package><Properties><DisplayName>Wide Game</DisplayName></Properties></Package>";
        let mut bytes = vec![0xFF, 0xFE];
        for unit in xml.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        write(&root.join("appxmanifest.xml"), &bytes);
        let exe = root.join(r"bin\wide.exe");
        assert_eq!(resolve_name(&exe.to_string_lossy(), None), "Wide Game");
    }

    #[test]
    fn resolves_epic_name_from_item_manifest() {
        let tmp = TempDir::new("epic");
        let install = tmp.path().join("Epic Games").join("Fortnite");
        let json = format!(
            r#"{{"DisplayName":"Fortnite Test","InstallLocation":"{}"}}"#,
            install.to_string_lossy().replace('\\', "\\\\")
        );
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(json.as_bytes());
        let manifests = tmp.path().join("Manifests");
        write(&manifests.join("a.item"), &bytes);
        let exe = install.join(r"bin\f.exe");
        assert_eq!(
            epic_name_in(&manifests, &exe.to_string_lossy()),
            Some("Fortnite Test".to_string())
        );
        let other = tmp.path().join(r"Elsewhere\f.exe");
        assert_eq!(epic_name_in(&manifests, &other.to_string_lossy()), None);
    }

    #[test]
    fn gog_lookup_of_an_unknown_exe_finds_nothing() {
        // Smoke test: enumerates the real registry key (absent on most machines) without failing.
        assert_eq!(gog_name(r"C:\lite-bar-test-no-such-dir\x.exe"), None);
    }

    #[test]
    fn falls_back_to_exe_stem() {
        let exe = r"C:\lite-bar-test-no-such-dir\Cool Game.exe";
        assert_eq!(resolve_name(exe, None), "Cool Game");
    }

    #[test]
    fn falls_back_to_window_title_before_exe_stem() {
        let exe = r"C:\lite-bar-test-no-such-dir\game.exe";
        assert_eq!(resolve_name(exe, Some("Cool Game - Level 1")), "Cool Game");
    }

    #[test]
    fn reads_version_info_of_a_real_exe() {
        // notepad.exe's resource carries ProductName "Microsoft(R) Windows(R) Operating System"
        // and a FileDescription containing "Notepad".
        let exe = r"C:\Windows\System32\notepad.exe";
        if !Path::new(exe).exists() {
            return;
        }
        let (product, description) = version_names(exe);
        assert!(product.is_some_and(|p| p.contains("Windows")));
        assert!(description.is_some_and(|d| d.contains("Notepad")));
        assert_eq!(
            version_names(r"C:\lite-bar-test-no-such-dir\x.exe"),
            (None, None)
        );
    }
}

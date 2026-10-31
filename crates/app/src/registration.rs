//! The all-users Start menu shortcut and the HKLM "Installed apps" entry for the copy under
//! Program Files (what they hold: `lite_bar_core::registration`). The move steps write them
//! after copying, and an elevated Lite Bar running from the installed copy refreshes them at
//! start (`spawn_refresh`), so an update that replaced the exe keeps them current.
//! Everything takes the registry root and the paths, so the tests use HKCU and temp dirs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use lite_bar_core::install::same_path;
use lite_bar_core::registration::{
    entry_values, stale_values, RegValue, SHORTCUT_FILE, UNINSTALL_KEY,
};
use windows::core::{Interface, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, RPC_E_CHANGED_MODE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    STGM_READ,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyExW, RegEnumValueW, RegOpenKeyExW, RegSetValueExW,
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, KEY_WOW64_64KEY, REG_DWORD,
    REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE,
};
use windows::Win32::UI::Shell::{FOLDERID_CommonPrograms, IShellLinkW, ShellLink};

use crate::log;
use crate::mover::known_folder;

/// Name of the start-time refresh thread; the panic hook leaves the ETW trace alone for it.
pub const REGISTER_THREAD: &str = "register";

/// The all-users Start menu "Programs" folder.
pub fn common_programs() -> Option<PathBuf> {
    known_folder(&FOLDERID_CommonPrograms).map(PathBuf::from)
}

/// Closes the registry key on drop.
struct RegKey(HKEY);

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: the key was opened by this module and is closed exactly once, here.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// Every value of `root\subkey`; a missing key (or one that can't be read) gives none.
/// Values that are neither strings nor DWORDs are skipped.
pub fn read_values(root: HKEY, subkey: &str) -> BTreeMap<String, RegValue> {
    let mut values = BTreeMap::new();
    let mut key = HKEY::default();
    // SAFETY: plain FFI; `key` is a live out-parameter and the subkey a valid string.
    let status = unsafe {
        RegOpenKeyExW(
            root,
            &HSTRING::from(subkey),
            None,
            KEY_READ | KEY_WOW64_64KEY,
            &mut key,
        )
    };
    if status != ERROR_SUCCESS {
        return values;
    }
    let key = RegKey(key);
    for index in 0.. {
        let mut name = [0u16; 256];
        let mut name_len = name.len() as u32;
        let mut kind = 0u32;
        let mut data = vec![0u8; 8192];
        let mut data_len = data.len() as u32;
        // SAFETY: the buffers are live and their lengths are passed alongside; each length is
        // updated to what was written.
        let status = unsafe {
            RegEnumValueW(
                key.0,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr()),
                Some(&mut data_len),
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status == ERROR_MORE_DATA {
            continue; // a value too large for the buffer: not one of ours
        }
        if status != ERROR_SUCCESS {
            break; // e.g. the key was deleted meanwhile: it won't get better
        }
        let name = String::from_utf16_lossy(&name[..name_len as usize]);
        let data = &data[..data_len as usize];
        match REG_VALUE_TYPE(kind) {
            REG_SZ if data.len() >= 2 => {
                let units: Vec<u16> = data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .collect();
                let text = String::from_utf16_lossy(&units);
                values.insert(name, RegValue::Str(text.trim_end_matches('\0').to_string()));
            }
            REG_DWORD if data.len() == 4 => {
                let n = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                values.insert(name, RegValue::Dword(n));
            }
            _ => {}
        }
    }
    values
}

/// Sets `values` under `root\subkey`, creating the key.
pub fn write_values(
    root: HKEY,
    subkey: &str,
    values: &[&(&'static str, RegValue)],
) -> Result<(), String> {
    let mut key = HKEY::default();
    // SAFETY: plain FFI; `key` is a live out-parameter and the subkey a valid string.
    let status = unsafe {
        RegCreateKeyExW(
            root,
            &HSTRING::from(subkey),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_WOW64_64KEY,
            None,
            &mut key,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("couldn't open {subkey}: {}", status.0));
    }
    let key = RegKey(key);
    for (name, value) in values {
        let (kind, data): (REG_VALUE_TYPE, Vec<u8>) = match value {
            RegValue::Str(text) => (
                REG_SZ,
                text.encode_utf16()
                    .chain(std::iter::once(0))
                    .flat_map(u16::to_le_bytes)
                    .collect(),
            ),
            RegValue::Dword(n) => (REG_DWORD, n.to_le_bytes().to_vec()),
        };
        // SAFETY: the name is a valid string and `data` outlives the call.
        let status =
            unsafe { RegSetValueExW(key.0, &HSTRING::from(*name), None, kind, Some(&data)) };
        if status != ERROR_SUCCESS {
            return Err(format!("couldn't write {name}: {}", status.0));
        }
    }
    Ok(())
}

/// Deletes the key `root\subkey` (it has no subkeys); a missing key is fine.
pub fn delete_key(root: HKEY, subkey: &str) -> Result<(), String> {
    // SAFETY: plain FFI with a valid string.
    let status = unsafe { RegDeleteKeyExW(root, &HSTRING::from(subkey), KEY_WOW64_64KEY.0, None) };
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(format!("couldn't delete {subkey}: {}", status.0))
    }
}

/// Apartment-threaded COM on this thread. A thread already in another mode keeps it.
fn init_com() {
    // SAFETY: plain FFI; an unbalanced init is harmless for a short-lived worker, and the
    // result (S_FALSE, RPC_E_CHANGED_MODE) only says COM was already usable.
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if hr.is_err() && hr != RPC_E_CHANGED_MODE {
        log::warn(&format!("CoInitializeEx failed: {hr:?}"));
    }
}

/// The exe the shortcut at `lnk` starts; `None` when it can't be read.
pub fn shortcut_target(lnk: &Path) -> Option<PathBuf> {
    init_com();
    // SAFETY: COM calls on interfaces created here; the buffer is live and its length passed.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        file.Load(&HSTRING::from(lnk.as_os_str()), STGM_READ).ok()?;
        let mut buf = [0u16; 1024];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
    }
}

/// Writes the shortcut at `lnk` to start `exe` from its folder, with the exe's own icon.
pub fn write_shortcut(lnk: &Path, exe: &Path) -> Result<(), String> {
    init_com();
    let fail = |what: &str, e: windows::core::Error| format!("shortcut {what}: {e}");
    // SAFETY: COM calls on interfaces created here; every string is a valid NUL-terminated
    // HSTRING for the duration of its call.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| fail("create", e))?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))
            .map_err(|e| fail("target", e))?;
        let dir = exe.parent().unwrap_or(Path::new(""));
        link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))
            .map_err(|e| fail("folder", e))?;
        link.SetIconLocation(&HSTRING::from(exe.as_os_str()), 0)
            .map_err(|e| fail("icon", e))?;
        let file: IPersistFile = link.cast().map_err(|e| fail("save", e))?;
        file.Save(&HSTRING::from(lnk.as_os_str()), true)
            .map_err(|e| fail("save", e))
    }
}

/// Brings the entry under `root\subkey` and the shortcut at `lnk` up to date for `exe`:
/// writes only the stale values, and the shortcut when it is missing or starts another file.
/// Both are tried; the first error is returned.
pub fn register_at(
    root: HKEY,
    subkey: &str,
    lnk: &Path,
    exe: &Path,
    version: &str,
) -> Result<(), String> {
    let size = std::fs::metadata(exe).map_or(0, |m| m.len());
    let wanted = entry_values(&exe.to_string_lossy(), version, size);
    let stale = stale_values(&read_values(root, subkey), &wanted);
    let entry = if stale.is_empty() {
        Ok(())
    } else {
        write_values(root, subkey, &stale)
    };
    let current = shortcut_target(lnk);
    let shortcut =
        if current.is_some_and(|t| same_path(&t.to_string_lossy(), &exe.to_string_lossy())) {
            Ok(())
        } else {
            write_shortcut(lnk, exe)
        };
    entry.and(shortcut)
}

/// `register_at` with the real HKLM key and the all-users Start menu. Needs elevation.
pub fn register(exe: &Path) -> Result<(), String> {
    let lnk = common_programs()
        .ok_or("can't find the Start menu folder")?
        .join(SHORTCUT_FILE);
    register_at(
        HKEY_LOCAL_MACHINE,
        UNINSTALL_KEY,
        &lnk,
        exe,
        env!("CARGO_PKG_VERSION"),
    )
}

/// Runs `register` on a worker thread, logging a failure as a warning.
pub fn spawn_refresh(exe: PathBuf) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(REGISTER_THREAD.into())
        .spawn(move || {
            if let Err(e) = register(&exe) {
                log::warn(&format!("couldn't refresh the Start menu entry: {e}"));
            }
        })
        .inspect_err(|e| log::warn(&format!("couldn't start the registration thread: {e}")))
        .ok()
}

#[cfg(test)]
mod tests {
    use windows::Win32::System::Registry::HKEY_CURRENT_USER;

    use super::*;
    use crate::test_util::TempDir;

    /// A HKCU subkey unique to the test, deleted on drop.
    struct TestKey(String);

    impl TestKey {
        fn new(name: &str) -> TestKey {
            let key = TestKey(format!(
                r"Software\LiteBarTest\{name}-{}",
                std::process::id()
            ));
            delete_key(HKEY_CURRENT_USER, &key.0).unwrap();
            key
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            let _ = delete_key(HKEY_CURRENT_USER, &self.0);
        }
    }

    fn s(v: &str) -> RegValue {
        RegValue::Str(v.to_string())
    }

    fn exe_file(dir: &TempDir, name: &str) -> PathBuf {
        let exe = dir.path().join(name);
        std::fs::write(&exe, b"MZ").unwrap();
        exe
    }

    #[test]
    fn write_then_read_values_round_trip() {
        let key = TestKey::new("round-trip");
        let values = [("Name", s("Lite Bar")), ("Size", RegValue::Dword(42))];
        let refs: Vec<_> = values.iter().collect();
        write_values(HKEY_CURRENT_USER, &key.0, &refs).unwrap();
        let read = read_values(HKEY_CURRENT_USER, &key.0);
        assert_eq!(read.get("Name"), Some(&s("Lite Bar")));
        assert_eq!(read.get("Size"), Some(&RegValue::Dword(42)));
        assert_eq!(read.len(), 2);
    }

    #[test]
    fn read_values_of_missing_key_is_empty() {
        let key = TestKey::new("read-missing");
        assert!(read_values(HKEY_CURRENT_USER, &key.0).is_empty());
    }

    #[test]
    fn delete_key_missing_is_ok() {
        let key = TestKey::new("delete-missing");
        assert_eq!(delete_key(HKEY_CURRENT_USER, &key.0), Ok(()));
    }

    #[test]
    fn register_rewrites_only_stale_values() {
        let key = TestKey::new("stale");
        let dir = TempDir::new("reg-stale");
        let exe = exe_file(&dir, "lite-bar.exe");
        let lnk = dir.path().join("Lite Bar.lnk");
        register_at(HKEY_CURRENT_USER, &key.0, &lnk, &exe, "0.4.1").unwrap();
        let sentinel = [("Publisher", s("X")), ("Keep", s("mine"))];
        let refs: Vec<_> = sentinel.iter().collect();
        write_values(HKEY_CURRENT_USER, &key.0, &refs).unwrap();

        register_at(HKEY_CURRENT_USER, &key.0, &lnk, &exe, "0.5.0").unwrap();

        let read = read_values(HKEY_CURRENT_USER, &key.0);
        assert_eq!(read.get("Publisher"), Some(&s("Lite Bar")));
        assert_eq!(read.get("DisplayVersion"), Some(&s("0.5.0")));
        assert_eq!(read.get("Keep"), Some(&s("mine")));
        assert_eq!(read.get("NoModify"), Some(&RegValue::Dword(1)));
    }

    #[test]
    fn shortcut_written_and_repointed() {
        let key = TestKey::new("repoint");
        let dir = TempDir::new("reg-repoint");
        let (a, b) = (exe_file(&dir, "a.exe"), exe_file(&dir, "b.exe"));
        let lnk = dir.path().join("Lite Bar.lnk");
        write_shortcut(&lnk, &a).unwrap();
        assert_eq!(shortcut_target(&lnk), Some(a));

        register_at(HKEY_CURRENT_USER, &key.0, &lnk, &b, "0.5.0").unwrap();
        assert_eq!(shortcut_target(&lnk), Some(b));
    }

    #[test]
    fn register_at_creates_missing_shortcut() {
        let key = TestKey::new("create-lnk");
        let dir = TempDir::new("reg-create-lnk");
        let exe = exe_file(&dir, "lite-bar.exe");
        let lnk = dir.path().join("Lite Bar.lnk");
        register_at(HKEY_CURRENT_USER, &key.0, &lnk, &exe, "0.5.0").unwrap();
        assert_eq!(shortcut_target(&lnk), Some(exe));
    }

    #[test]
    fn shortcut_target_of_missing_file_is_none() {
        let dir = TempDir::new("reg-no-lnk");
        assert_eq!(shortcut_target(&dir.path().join("nope.lnk")), None);
    }

    #[test]
    fn common_programs_is_a_known_folder() {
        assert!(common_programs().is_some_and(|p| p.is_absolute()));
    }
}

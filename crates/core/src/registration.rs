//! What the "Installed apps" (Add/Remove Programs) entry holds, and when to write it. Pure
//! values and decisions; the registry and shortcut calls live in the app crate.

use crate::install::{same_path, target_exe};
use std::collections::BTreeMap;

/// The entry's key under HKLM.
pub const UNINSTALL_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\LiteBar";
/// The all-users Start menu shortcut's file name.
pub const SHORTCUT_FILE: &str = "Lite Bar.lnk";
/// The project page the entry links to.
pub const REPO_URL: &str = "https://github.com/paymonr/lite-bar";

/// A registry value the entry holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegValue {
    Str(String),
    Dword(u32),
}

/// The entry's values for the exe at `exe`, in the order they are written.
pub fn entry_values(exe: &str, version: &str, size_bytes: u64) -> Vec<(&'static str, RegValue)> {
    let dir = exe.rsplit_once('\\').map_or("", |(dir, _)| dir);
    let text = |v: &str| RegValue::Str(v.to_string());
    vec![
        ("DisplayName", text("Lite Bar")),
        ("DisplayVersion", text(version)),
        ("Publisher", text("Lite Bar")),
        ("DisplayIcon", text(&format!("\"{exe}\",0"))),
        ("InstallLocation", text(dir)),
        ("UninstallString", text(&format!("\"{exe}\" --uninstall"))),
        ("URLInfoAbout", text(REPO_URL)),
        (
            "EstimatedSize",
            RegValue::Dword(size_bytes.div_ceil(1024) as u32),
        ),
        ("NoModify", RegValue::Dword(1)),
        ("NoRepair", RegValue::Dword(1)),
    ]
}

/// The wanted values that are missing from `current` or differ. Names compare without case,
/// as the registry does; `current` keys are matched that way too.
pub fn stale_values<'a>(
    current: &BTreeMap<String, RegValue>,
    wanted: &'a [(&'static str, RegValue)],
) -> Vec<&'a (&'static str, RegValue)> {
    wanted
        .iter()
        .filter(|(name, value)| {
            !current
                .iter()
                .any(|(n, v)| n.eq_ignore_ascii_case(name) && v == value)
        })
        .collect()
}

/// Whether `exe` is the copy under Program Files (not Program Files (x86)).
pub fn is_installed_copy(exe: &str, program_files: &str) -> bool {
    same_path(exe, &target_exe(program_files))
}

/// Whether to write the entry at startup: elevated, and running from the installed copy.
pub fn should_register(elevated: bool, exe: Option<&str>, program_files: Option<&str>) -> bool {
    match (exe, program_files) {
        (Some(exe), Some(pf)) => elevated && is_installed_copy(exe, pf),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const PF: &str = r"C:\Program Files";
    const EXE: &str = r"C:\Program Files\Lite Bar\lite-bar.exe";

    fn s(v: &str) -> RegValue {
        RegValue::Str(v.to_string())
    }

    #[test]
    fn entry_values_for_installed_exe() {
        assert_eq!(
            entry_values(EXE, "0.5.0", 1_500_000),
            vec![
                ("DisplayName", s("Lite Bar")),
                ("DisplayVersion", s("0.5.0")),
                ("Publisher", s("Lite Bar")),
                (
                    "DisplayIcon",
                    s(r#""C:\Program Files\Lite Bar\lite-bar.exe",0"#)
                ),
                ("InstallLocation", s(r"C:\Program Files\Lite Bar")),
                (
                    "UninstallString",
                    s(r#""C:\Program Files\Lite Bar\lite-bar.exe" --uninstall"#)
                ),
                ("URLInfoAbout", s("https://github.com/paymonr/lite-bar")),
                ("EstimatedSize", RegValue::Dword(1465)),
                ("NoModify", RegValue::Dword(1)),
                ("NoRepair", RegValue::Dword(1)),
            ]
        );
    }

    #[test]
    fn estimated_size_rounds_up() {
        let size = |b| match entry_values(EXE, "1", b)
            .into_iter()
            .find(|(n, _)| *n == "EstimatedSize")
        {
            Some((_, RegValue::Dword(kb))) => kb,
            other => panic!("{other:?}"),
        };
        assert_eq!(size(1024), 1);
        assert_eq!(size(1025), 2);
        assert_eq!(size(0), 0);
    }

    fn current_from(values: &[(&'static str, RegValue)]) -> BTreeMap<String, RegValue> {
        values
            .iter()
            .map(|(n, v)| (n.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn stale_values_empty_when_all_match() {
        let wanted = entry_values(EXE, "0.5.0", 1_500_000);
        let current = wanted
            .iter()
            .map(|(n, v)| (n.to_lowercase(), v.clone()))
            .collect();
        assert!(stale_values(&current, &wanted).is_empty());
    }

    #[test]
    fn stale_values_lists_only_changed_and_missing() {
        let wanted = entry_values(EXE, "0.5.0", 1_500_000);
        let mut current = current_from(&entry_values(EXE, "0.4.1", 1_500_000));
        current.remove("NoRepair");
        let names: Vec<&str> = stale_values(&current, &wanted)
            .iter()
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(names, vec!["DisplayVersion", "NoRepair"]);
    }

    #[test]
    fn stale_values_type_change_counts() {
        let wanted = entry_values(EXE, "0.5.0", 1_500_000);
        let mut current = current_from(&wanted);
        current.insert("EstimatedSize".to_string(), s("1465"));
        let names: Vec<&str> = stale_values(&current, &wanted)
            .iter()
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(names, vec!["EstimatedSize"]);
    }

    #[test]
    fn should_register_only_elevated_installed_copy() {
        let installed = r"c:\program files\lite bar\LITE-BAR.EXE";
        assert!(should_register(true, Some(installed), Some(PF)));
        assert!(!should_register(false, Some(installed), Some(PF)));
        assert!(!should_register(
            true,
            Some(r"C:\Program Files (x86)\Lite Bar\lite-bar.exe"),
            Some(PF)
        ));
        assert!(!should_register(
            true,
            Some(r"C:\Users\a\lite-bar.exe"),
            Some(PF)
        ));
        assert!(!should_register(true, None, Some(PF)));
        assert!(!should_register(true, Some(installed), None));
        assert!(is_installed_copy(installed, PF));
    }
}

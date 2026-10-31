//! The pure side of uninstalling: the command line, which steps run, when the install folder
//! may go, and what the user is told. The Win32 side lives in the app crate.

use crate::install::{flag_value, quote, same_path, target_dir, EXE_NAME, FOR_FLAG};

/// Runs the uninstall: bare from Installed apps, or `--uninstall --for "<user>"` elevated.
pub const UNINSTALL_FLAG: &str = "--uninstall";

/// Shown when the running Lite Bar won't close.
pub const QUIT_FAILED: &str = "Couldn't close Lite Bar. Quit it from the tray icon and try again.";
/// Shown when the elevation was another account's.
pub const OTHER_ACCOUNT: &str = "Uninstalling needs your own administrator account.";

/// Parses the uninstall arguments. `None` when the first arg isn't `--uninstall`;
/// `Some(None)` without a usable `--for` user (a flag or empty value is no user).
pub fn parse_uninstall(args: impl Iterator<Item = String>) -> Option<Option<String>> {
    let args: Vec<String> = args.collect();
    if args.first().map(String::as_str) != Some(UNINSTALL_FLAG) {
        return None;
    }
    Some(
        flag_value(&args, FOR_FLAG)
            .flatten()
            .filter(|user| !user.is_empty()),
    )
}

/// The elevated uninstall's command line: `--uninstall --for "<user>"`. `user` must not
/// contain `"`.
pub fn uninstall_args(user: &str) -> String {
    format!("{UNINSTALL_FLAG} {FOR_FLAG} {}", quote(user))
}

/// One thing the uninstall removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UninstallStep {
    QuitRunning,
    RemoveStartup,
    RemoveShortcut,
    RemoveData,
    RemoveProgram,
    RemoveEntry,
}

/// The steps to run, in order. A portable copy has no shortcut, program folder or entry.
pub fn uninstall_plan(installed: bool, delete_data: bool) -> Vec<UninstallStep> {
    use UninstallStep::*;
    let mut plan = vec![QuitRunning, RemoveStartup];
    if installed {
        plan.push(RemoveShortcut);
    }
    if delete_data {
        plan.push(RemoveData);
    }
    if installed {
        plan.extend([RemoveProgram, RemoveEntry]);
    }
    plan
}

/// Whether the install folder may be deleted: it is the Program Files target, and holds
/// only the exe and its update leftovers (no case).
pub fn folder_removable(dir: &str, program_files: &str, entries: &[String]) -> bool {
    let known = [
        EXE_NAME.to_string(),
        format!("{EXE_NAME}.new"),
        format!("{EXE_NAME}.old"),
    ];
    same_path(dir, &target_dir(program_files))
        && entries
            .iter()
            .all(|e| known.iter().any(|k| k.eq_ignore_ascii_case(e)))
}

/// What the step removes, as the user reads it.
pub fn step_label(step: UninstallStep) -> &'static str {
    match step {
        UninstallStep::QuitRunning => "Lite Bar",
        UninstallStep::RemoveStartup => "Start with Windows",
        UninstallStep::RemoveShortcut => "Start menu shortcut",
        UninstallStep::RemoveData => "Settings and stats",
        UninstallStep::RemoveProgram => EXE_NAME,
        UninstallStep::RemoveEntry => "Installed apps entry",
    }
}

/// The closing message, naming what couldn't be removed.
pub fn finish_message(failed: &[UninstallStep]) -> String {
    if failed.is_empty() {
        return "Lite Bar was uninstalled.".to_string();
    }
    let labels: Vec<&str> = failed.iter().map(|s| step_label(*s)).collect();
    format!(
        "Lite Bar was uninstalled, but these couldn't be removed: {}",
        labels.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::tests::shell_split;

    const PF: &str = r"C:\Program Files";

    fn args(v: &[&str]) -> std::vec::IntoIter<String> {
        v.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn parse_uninstall_forms() {
        assert_eq!(parse_uninstall(args(&[])), None);
        assert_eq!(parse_uninstall(args(&["--move-to", "x"])), None);
        assert_eq!(parse_uninstall(args(&["--uninstall"])), Some(None));
        assert_eq!(
            parse_uninstall(args(&["--uninstall", "--for", r"PC\a"])),
            Some(Some(r"PC\a".to_string()))
        );
        assert_eq!(
            parse_uninstall(args(&["--uninstall", "--for", "--x"])),
            Some(None)
        );
        assert_eq!(
            parse_uninstall(args(&["--uninstall", "--for", ""])),
            Some(None)
        );
    }

    #[test]
    fn uninstall_args_round_trip() {
        let line = uninstall_args(r"PC\a b");
        assert_eq!(
            parse_uninstall(shell_split(&line).into_iter()),
            Some(Some(r"PC\a b".to_string()))
        );
    }

    use UninstallStep::*;

    #[test]
    fn plan_installed_with_and_without_data() {
        assert_eq!(
            uninstall_plan(true, true),
            vec![
                QuitRunning,
                RemoveStartup,
                RemoveShortcut,
                RemoveData,
                RemoveProgram,
                RemoveEntry
            ]
        );
        assert_eq!(
            uninstall_plan(true, false),
            vec![
                QuitRunning,
                RemoveStartup,
                RemoveShortcut,
                RemoveProgram,
                RemoveEntry
            ]
        );
    }

    #[test]
    fn plan_portable_with_and_without_data() {
        assert_eq!(
            uninstall_plan(false, true),
            vec![QuitRunning, RemoveStartup, RemoveData]
        );
        assert_eq!(
            uninstall_plan(false, false),
            vec![QuitRunning, RemoveStartup]
        );
    }

    #[test]
    fn folder_removable_only_target_with_known_files() {
        let files = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let target = r"C:\Program Files\Lite Bar";
        assert!(folder_removable(target, PF, &files(&["lite-bar.exe"])));
        assert!(folder_removable(
            target,
            PF,
            &files(&["LITE-BAR.EXE", "lite-bar.exe.new"])
        ));
        assert!(!folder_removable(
            target,
            PF,
            &files(&["lite-bar.exe", "notes.txt"])
        ));
        assert!(!folder_removable(
            r"C:\Program Files\Other",
            PF,
            &files(&["lite-bar.exe"])
        ));
        assert!(!folder_removable(PF, PF, &files(&["lite-bar.exe"])));
    }

    #[test]
    fn finish_message_lists_failures() {
        assert_eq!(finish_message(&[]), "Lite Bar was uninstalled.");
        assert_eq!(
            finish_message(&[RemoveShortcut, RemoveProgram]),
            "Lite Bar was uninstalled, but these couldn't be removed: Start menu shortcut, lite-bar.exe"
        );
    }

    #[test]
    fn step_labels_and_copy() {
        assert_eq!(step_label(QuitRunning), "Lite Bar");
        assert_eq!(step_label(RemoveStartup), "Start with Windows");
        assert_eq!(step_label(RemoveData), "Settings and stats");
        assert_eq!(step_label(RemoveEntry), "Installed apps entry");
        assert_eq!(
            QUIT_FAILED,
            "Couldn't close Lite Bar. Quit it from the tray icon and try again."
        );
        assert_eq!(
            OTHER_ACCOUNT,
            "Uninstalling needs your own administrator account."
        );
    }
}

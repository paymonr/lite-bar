//! The pure decision logic for moving lite-bar.exe into Program Files: where it goes, when to
//! offer, which old copy is safe to delete, and the helper's command line and exit codes.
//! String-based so it runs (and is tested) anywhere; the Win32 side lives in the app crate.

/// The folder under Program Files that holds the moved exe.
pub const TARGET_FOLDER: &str = "Lite Bar";
/// The exe's file name, also the only file name the old-copy cleanup may delete.
pub const EXE_NAME: &str = "lite-bar.exe";
/// Runs the elevated helper: `--move-to "<dir>" --for "<DOMAIN\user>"`.
pub const MOVE_TO_FLAG: &str = "--move-to";
/// The account the helper moves for, after [`MOVE_TO_FLAG`].
pub const FOR_FLAG: &str = "--for";
/// Marks the first start of the moved copy: `--moved-from "<old exe>"`.
pub const MOVED_FROM_FLAG: &str = "--moved-from";

/// A path with `/` as `\`, no case, and no trailing `\`.
fn norm(p: &str) -> String {
    p.replace('/', "\\")
        .to_lowercase()
        .trim_end_matches('\\')
        .to_string()
}

/// Whether two Windows paths name the same file: no case, either slash.
pub fn same_path(a: &str, b: &str) -> bool {
    let slashed = |p: &str| p.replace('/', "\\").to_lowercase();
    slashed(a) == slashed(b)
}

/// Whether two `DOMAIN\user` names are the same account (no case).
pub fn same_user(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Whether `path` is inside `root` (not `root` itself): no case, either slash, and the match
/// must end on a separator, so `C:\Program FilesX` isn't under `C:\Program Files`.
pub fn is_under(path: &str, root: &str) -> bool {
    let prefix = format!("{}\\", norm(root));
    let path = path.replace('/', "\\").to_lowercase();
    path.len() > prefix.len() && path.starts_with(&prefix)
}

/// Whether `path` is inside any of `roots`.
pub fn is_under_any(path: &str, roots: &[String]) -> bool {
    roots.iter().any(|r| is_under(path, r))
}

/// Whether to show the welcome popup: the setting is on, the exe path is known, there are
/// Program Files roots to compare with, and the exe isn't already under one.
pub fn should_offer(move_prompt: bool, exe: Option<&str>, roots: &[String]) -> bool {
    match exe {
        Some(exe) => move_prompt && !roots.is_empty() && !is_under_any(exe, roots),
        None => false,
    }
}

/// The folder the exe moves to, under the given Program Files folder.
pub fn target_dir(program_files: &str) -> String {
    format!("{}\\{TARGET_FOLDER}", program_files.trim_end_matches('\\'))
}

/// The moved exe's full path.
pub fn target_exe(program_files: &str) -> String {
    format!("{}\\{EXE_NAME}", target_dir(program_files))
}

/// Whether the old copy at `old` may be deleted after a move: it is named `lite-bar.exe`, it
/// isn't the running exe, and it isn't under Program Files.
pub fn safe_to_delete(old: &str, running: &str, roots: &[String]) -> bool {
    let name = old.rsplit(['\\', '/']).next().unwrap_or("");
    name.eq_ignore_ascii_case(EXE_NAME) && !same_path(old, running) && !is_under_any(old, roots)
}

/// The elevated helper's arguments: `--move-to "<dir>" --for "<user>"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveArgs {
    pub dir: String,
    pub for_user: String,
}

/// The value after `flag`, if the flag is there: `Some(None)` when it has no value.
fn flag_value(args: &[String], flag: &str) -> Option<Option<String>> {
    let i = args.iter().position(|a| a == flag)?;
    Some(args.get(i + 1).cloned())
}

/// Parses the helper's arguments. `None` without `--move-to`, `Err` when the dir or the
/// `--for` value is missing.
pub fn parse_move_to(args: impl Iterator<Item = String>) -> Option<Result<MoveArgs, String>> {
    let args: Vec<String> = args.collect();
    let dir = flag_value(&args, MOVE_TO_FLAG)?;
    Some(match (dir, flag_value(&args, FOR_FLAG).flatten()) {
        (Some(dir), Some(for_user)) => Ok(MoveArgs { dir, for_user }),
        (None, _) => Err(format!("{MOVE_TO_FLAG} needs a folder")),
        (_, None) => Err(format!("{FOR_FLAG} needs a user name")),
    })
}

/// `value` in double quotes for a Windows command line. Backslashes right before the closing
/// quote are doubled, or the quote would be read as an escaped literal. `value` must not
/// contain `"`.
fn quote(value: &str) -> String {
    let trailing = value.len() - value.trim_end_matches('\\').len();
    format!("\"{value}{}\"", "\\".repeat(trailing))
}

/// The command line that starts the helper, both values quoted. The values must not
/// contain `"`.
pub fn move_to_args(dir: &str, user: &str) -> String {
    format!("{MOVE_TO_FLAG} {} {FOR_FLAG} {}", quote(dir), quote(user))
}

/// The old exe path after `--moved-from`, if the flag and its value are there.
pub fn parse_moved_from(args: impl Iterator<Item = String>) -> Option<String> {
    let args: Vec<String> = args.collect();
    flag_value(&args, MOVED_FROM_FLAG).flatten()
}

/// The moved copy's command line: `--moved-from "<old exe>"`. `old` must not contain `"`.
pub fn moved_from_args(old: &str) -> String {
    format!("{MOVED_FROM_FLAG} {}", quote(old))
}

/// The elevated helper's exit code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum HelperExit {
    Moved = 0,
    Failed = 1,
    /// The elevation was another account's, so the files would go to the wrong user.
    OtherAccount = 2,
    MovedNotStarted = 3,
}

impl HelperExit {
    /// The variant for a process exit code; unknown codes (even `STILL_ACTIVE`) are `Failed`.
    pub fn from_code(code: u32) -> HelperExit {
        match code {
            0 => HelperExit::Moved,
            2 => HelperExit::OtherAccount,
            3 => HelperExit::MovedNotStarted,
            _ => HelperExit::Failed,
        }
    }
}

/// How a move attempt ended, as the popup reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoveOutcome {
    Moved,
    /// The user said no at the UAC prompt.
    Declined,
    OtherAccount,
    Failed(String),
    MovedNotStarted(String),
}

/// The popup's state.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum MoveStatus {
    #[default]
    Idle,
    Moving,
    Done(MoveOutcome),
}

#[cfg(test)]
mod tests {
    use super::*;

    const PF: &str = r"C:\Program Files";
    const PF86: &str = r"C:\Program Files (x86)";

    fn roots() -> Vec<String> {
        vec![PF.to_string(), PF86.to_string()]
    }

    /// Split like Windows argv parsing (CommandLineToArgvW): spaces outside quotes separate
    /// args; 2n backslashes + quote -> n backslashes and the quote toggles; 2n+1 backslashes +
    /// quote -> n backslashes and a literal quote; other backslashes are literal.
    fn shell_split(s: &str) -> Vec<String> {
        let chars: Vec<char> = s.chars().collect();
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut quoted = false;
        let mut any = false;
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '\\' => {
                    let mut n = 0;
                    while i < chars.len() && chars[i] == '\\' {
                        n += 1;
                        i += 1;
                    }
                    if i < chars.len() && chars[i] == '"' {
                        cur.push_str(&"\\".repeat(n / 2));
                        if n % 2 == 1 {
                            cur.push('"');
                            i += 1;
                        }
                    } else {
                        cur.push_str(&"\\".repeat(n));
                    }
                    any = true;
                    continue;
                }
                '"' => {
                    quoted = !quoted;
                    any = true;
                }
                ' ' if !quoted => {
                    if any {
                        out.push(std::mem::take(&mut cur));
                        any = false;
                    }
                }
                c => {
                    cur.push(c);
                    any = true;
                }
            }
            i += 1;
        }
        if any {
            out.push(cur);
        }
        out
    }

    #[test]
    fn args_with_trailing_backslash_round_trip() {
        let line = move_to_args(r"C:\Program Files\", r"DOM\u");
        assert_eq!(
            parse_move_to(shell_split(&line).into_iter()),
            Some(Ok(MoveArgs {
                dir: r"C:\Program Files\".to_string(),
                for_user: r"DOM\u".to_string(),
            }))
        );
        let line = moved_from_args(r"C:\old dir\");
        assert_eq!(
            parse_moved_from(shell_split(&line).into_iter()),
            Some(r"C:\old dir\".to_string())
        );
    }

    #[test]
    fn is_under_needs_separator_boundary_and_ignores_case() {
        assert!(is_under(r"C:\Program Files\Lite Bar\lite-bar.exe", PF));
        assert!(is_under(
            "c:/program files/lite bar/LITE-BAR.EXE",
            r"C:\Program Files\"
        ));
        assert!(is_under_any(r"C:\Program Files (x86)\x\a.exe", &roots()));
        assert!(!is_under(r"C:\Program Files Foo\x.exe", PF));
        assert!(!is_under(r"C:\Program FilesX\x.exe", PF));
        assert!(!is_under(PF, PF));
    }

    #[test]
    fn should_offer_cases() {
        let exe = r"C:\Users\p\Downloads\lite-bar.exe";
        assert!(should_offer(true, Some(exe), &roots()));
        assert!(!should_offer(false, Some(exe), &roots()));
        assert!(!should_offer(true, None, &roots()));
        assert!(!should_offer(
            true,
            Some(r"C:\Program Files\Lite Bar\lite-bar.exe"),
            &roots()
        ));
        assert!(!should_offer(true, Some(exe), &[]));
    }

    #[test]
    fn target_paths() {
        assert_eq!(target_dir(PF), r"C:\Program Files\Lite Bar");
        assert_eq!(
            target_dir(r"C:\Program Files\"),
            r"C:\Program Files\Lite Bar"
        );
        assert_eq!(target_exe(PF), r"C:\Program Files\Lite Bar\lite-bar.exe");
    }

    #[test]
    fn safe_to_delete_rules() {
        let running = r"C:\Program Files\Lite Bar\lite-bar.exe";
        let r = roots();
        assert!(safe_to_delete(
            r"C:\Users\p\Downloads\lite-bar.exe",
            running,
            &r
        ));
        assert!(safe_to_delete(r"C:\Users\p\LITE-BAR.EXE", running, &r));
        assert!(!safe_to_delete(running, running, &r));
        assert!(!safe_to_delete(r"C:\Users\p\notes.txt", running, &r));
        assert!(!safe_to_delete(r"C:\Users\p\lite-bar.exe.old", running, &r));
        assert!(!safe_to_delete(
            r"C:\Program Files (x86)\Lite Bar\lite-bar.exe",
            running,
            &r
        ));
        assert!(!safe_to_delete("", running, &r));
    }

    #[test]
    fn parse_move_to_round_trips() {
        let line = move_to_args(r"C:\Program Files\Lite Bar", r"DOM\Paymon Q");
        let parsed = parse_move_to(shell_split(&line).into_iter());
        assert_eq!(
            parsed,
            Some(Ok(MoveArgs {
                dir: r"C:\Program Files\Lite Bar".to_string(),
                for_user: r"DOM\Paymon Q".to_string(),
            }))
        );
        let args = |v: &[&str]| {
            v.iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .into_iter()
        };
        assert!(matches!(parse_move_to(args(&["--move-to"])), Some(Err(_))));
        assert!(matches!(
            parse_move_to(args(&["--move-to", "X"])),
            Some(Err(_))
        ));
        assert_eq!(parse_move_to(args(&["--updated"])), None);
    }

    #[test]
    fn parse_moved_from_cases() {
        let args = |v: &[&str]| {
            v.iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .into_iter()
        };
        assert_eq!(
            parse_moved_from(args(&["--moved-from", r"C:\a b\lite-bar.exe"])),
            Some(r"C:\a b\lite-bar.exe".to_string())
        );
        assert_eq!(parse_moved_from(args(&["--moved-from"])), None);
        assert_eq!(parse_moved_from(args(&["--updated"])), None);
        let line = moved_from_args(r"C:\a b\lite-bar.exe");
        assert_eq!(
            parse_moved_from(shell_split(&line).into_iter()),
            Some(r"C:\a b\lite-bar.exe".to_string())
        );
    }

    #[test]
    fn helper_exit_codes() {
        assert_eq!(HelperExit::from_code(0), HelperExit::Moved);
        assert_eq!(HelperExit::from_code(2), HelperExit::OtherAccount);
        assert_eq!(HelperExit::from_code(3), HelperExit::MovedNotStarted);
        for code in [1, 7, 259] {
            assert_eq!(HelperExit::from_code(code), HelperExit::Failed);
        }
        assert_eq!(HelperExit::OtherAccount as i32, 2);
    }

    #[test]
    fn same_user_ignores_case() {
        assert!(same_user(r"DOM\Paymon", r"dom\paymon"));
        assert!(!same_user(r"DOM\a", r"DOM\b"));
    }

    #[test]
    fn same_path_ignores_case_and_slash_direction() {
        assert!(same_path(r"C:\A\b.exe", "c:/a/B.EXE"));
        assert!(!same_path(r"C:\A\b.exe", r"C:\A\c.exe"));
    }

    #[test]
    fn move_status_defaults_to_idle() {
        assert_eq!(MoveStatus::default(), MoveStatus::Idle);
        let _ = MoveStatus::Done(MoveOutcome::Failed("x".into()));
    }
}

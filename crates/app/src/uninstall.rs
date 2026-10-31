//! Runs the uninstall steps (the plan and the messages: `lite_bar_core::uninstall`). The
//! steps go through `UninstallOps`, so the tests run the plan with fakes and never remove
//! anything.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use lite_bar_core::registration::{SHORTCUT_FILE, UNINSTALL_KEY};
use lite_bar_core::startup_task::StartupState;
use lite_bar_core::uninstall::{folder_removable, UninstallStep, QUIT_FAILED};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};
use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};

use crate::log;
use crate::registration::{common_programs, delete_key};
use crate::single_instance::{acquire_single_instance, wait_for};
use crate::startup::{query, schtasks_path, set_now, SystemRunner, CREATE_NO_WINDOW};

/// Name of the uninstall thread; the panic hook leaves the ETW trace alone for it.
pub const UNINSTALL_THREAD: &str = "uninstall";

/// How long the running Lite Bar gets to close, and how often that is checked.
const QUIT_WAIT: Duration = Duration::from_secs(10);
const QUIT_POLL: Duration = Duration::from_millis(200);

/// What each step does. Fields are plain `fn`s so the tests can swap in fakes.
pub struct UninstallOps {
    pub quit_running: fn() -> Result<(), String>,
    pub remove_startup: fn(&str) -> Result<(), String>,
    pub remove_shortcut: fn() -> Result<(), String>,
    pub remove_data: fn() -> Result<(), String>,
    /// Takes the exe path.
    pub remove_program: fn(&str) -> Result<(), String>,
    pub remove_entry: fn() -> Result<(), String>,
}

/// The ops that really remove things.
pub fn real_ops() -> UninstallOps {
    UninstallOps {
        quit_running,
        remove_startup,
        remove_shortcut,
        remove_data,
        remove_program,
        remove_entry,
    }
}

/// Runs `plan` with `ops`. When Lite Bar can't be closed nothing else runs (`Err`);
/// otherwise every step runs and the failed ones are returned. The entry is kept (not run,
/// not reported) when the program couldn't be removed, so Uninstall can be retried.
pub fn run_plan(
    plan: &[UninstallStep],
    exe: &str,
    ops: &UninstallOps,
) -> Result<Vec<UninstallStep>, String> {
    let mut failed = Vec::new();
    for &step in plan {
        let result = match step {
            UninstallStep::QuitRunning => (ops.quit_running)(),
            UninstallStep::RemoveStartup => (ops.remove_startup)(exe),
            UninstallStep::RemoveShortcut => (ops.remove_shortcut)(),
            UninstallStep::RemoveData => (ops.remove_data)(),
            UninstallStep::RemoveProgram => (ops.remove_program)(exe),
            UninstallStep::RemoveEntry => {
                if failed.contains(&UninstallStep::RemoveProgram) {
                    continue;
                }
                (ops.remove_entry)()
            }
        };
        if let Err(e) = result {
            log::warn(&format!("uninstall: {step:?} failed: {e}"));
            if step == UninstallStep::QuitRunning {
                return Err(QUIT_FAILED.into());
            }
            failed.push(step);
        }
    }
    Ok(failed)
}

/// Asks a running Lite Bar to quit (as the tray's Quit does, saving the sessions) and waits
/// for it to let go of the single-instance mutex.
fn quit_running() -> Result<(), String> {
    // SAFETY: plain FFI with a static NUL-terminated class name.
    if let Ok(hwnd) = unsafe { FindWindowW(w!("LiteBarOwner"), None) } {
        // SAFETY: plain FFI; a window that has gone since is only an error result.
        if let Err(e) =
            unsafe { PostMessageW(Some(hwnd), WM_CLOSE, Default::default(), Default::default()) }
        {
            log::warn(&format!("uninstall: couldn't ask Lite Bar to close: {e}"));
        }
    }
    let start = Instant::now();
    let free = wait_for(
        QUIT_WAIT,
        QUIT_POLL,
        || start.elapsed(),
        std::thread::sleep,
        // Held only for the check: dropped right away.
        || matches!(acquire_single_instance(crate::app::MUTEX_NAME), Ok(Some(_))).then_some(()),
    );
    free.ok_or_else(|| "Lite Bar is still running".to_string())
}

/// Deletes the startup task when it starts this exe.
fn remove_startup(exe: &str) -> Result<(), String> {
    if query(&SystemRunner, &schtasks_path(), exe) == StartupState::On {
        set_now(false)
    } else {
        Ok(())
    }
}

/// A missing file is fine.
fn remove_file_if_present(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("couldn't delete {}: {e}", path.display())),
    }
}

fn remove_shortcut() -> Result<(), String> {
    match common_programs() {
        Some(dir) => remove_file_if_present(&dir.join(SHORTCUT_FILE)),
        None => Err("couldn't find the Start menu folder".into()),
    }
}

fn remove_data() -> Result<(), String> {
    let dir = crate::paths::app_dir();
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("couldn't delete {}: {e}", dir.display())),
    }
}

/// The PowerShell command that waits for process `pid` to exit, then deletes the exe (and
/// the folder). Paths are single-quoted `-LiteralPath`s, so `%`, `$` and `&` stay literal.
fn cleanup_command(pid: u32, exe: &str, dir: &str, remove_dir: bool) -> String {
    let quoted = |p: &str| format!("'{}'", p.replace('\'', "''"));
    let mut command = format!(
        "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; Remove-Item -LiteralPath {} -Force -ErrorAction SilentlyContinue",
        quoted(exe)
    );
    if remove_dir {
        command.push_str(&format!(
            "; Remove-Item -LiteralPath {} -Recurse -Force -ErrorAction SilentlyContinue",
            quoted(dir)
        ));
    }
    command
}

/// Whether the install folder goes too: only when it was listed (a failed listing is not an
/// empty folder), Program Files is known, and `folder_removable` agrees.
fn removes_folder(
    listing: Result<Vec<String>, String>,
    dir: &str,
    program_files: Option<&str>,
) -> bool {
    match (listing, program_files) {
        (Ok(entries), Some(pf)) => folder_removable(dir, pf, &entries),
        _ => false,
    }
}

fn list_names(dir: &str) -> Result<Vec<String>, String> {
    std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .map(|e| {
            e.map(|e| e.file_name().to_string_lossy().into_owned())
                .map_err(|e| e.to_string())
        })
        .collect()
}

/// Schedules `path` for deletion at the next restart.
fn delete_at_restart(path: &str) -> Result<(), String> {
    // SAFETY: the path string outlives the call; a null new name means delete.
    unsafe {
        MoveFileExW(
            &HSTRING::from(path),
            PCWSTR::null(),
            MOVEFILE_DELAY_UNTIL_REBOOT,
        )
    }
    .map_err(|e| format!("couldn't schedule {path} for deletion at restart: {e}"))
}

/// Starts a hidden PowerShell that waits for this process to exit (the exe is locked until
/// then) and deletes the exe and, if `remove_dir`, the folder.
fn spawn_cleaner(exe: &str, dir: &str, remove_dir: bool) -> Result<(), String> {
    let system = Path::new(&schtasks_path())
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let powershell = system.join(r"WindowsPowerShell\v1.0\powershell.exe");
    let command = cleanup_command(std::process::id(), exe, dir, remove_dir);
    Command::new(powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
        ])
        .arg(command)
        // Not the install folder, which would stay in use and so couldn't be removed.
        .current_dir(system)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("couldn't start the cleanup: {e}"))
}

/// Deletes the exe (and its folder when only Lite Bar's own files are in it). The exe is
/// locked while this process runs, so it is both scheduled for deletion at restart and
/// handed to a cleaner that waits for this process to exit. `Ok` when either was set up.
fn remove_program(exe: &str) -> Result<(), String> {
    let dir = Path::new(exe)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let pf = crate::mover::program_files();
    let remove_dir = removes_folder(list_names(&dir), &dir, pf.as_deref());
    // The folder after the exe: pending deletes run in order, and it must be empty by then.
    let mut restart = delete_at_restart(exe);
    if restart.is_ok() && remove_dir {
        restart = delete_at_restart(&dir);
    }
    let cleaner = spawn_cleaner(exe, &dir, remove_dir);
    if let (Err(r), Err(c)) = (&restart, &cleaner) {
        return Err(format!("{r}; {c}"));
    }
    for e in [restart.err(), cleaner.err()].into_iter().flatten() {
        log::warn(&format!("uninstall: {e}"));
    }
    Ok(())
}

fn remove_entry() -> Result<(), String> {
    delete_key(HKEY_LOCAL_MACHINE, UNINSTALL_KEY)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use lite_bar_core::uninstall::{uninstall_plan, UninstallStep, QUIT_FAILED};

    use super::*;

    thread_local! {
        static CALLS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    fn call(name: &'static str, result: Result<(), String>) -> Result<(), String> {
        CALLS.with(|c| c.borrow_mut().push(name));
        result
    }

    fn calls() -> Vec<&'static str> {
        CALLS.with(|c| std::mem::take(&mut *c.borrow_mut()))
    }

    fn ok_ops() -> UninstallOps {
        UninstallOps {
            quit_running: || call("quit", Ok(())),
            remove_startup: |_| call("startup", Ok(())),
            remove_shortcut: || call("shortcut", Ok(())),
            remove_data: || call("data", Ok(())),
            remove_program: |_| call("program", Ok(())),
            remove_entry: || call("entry", Ok(())),
        }
    }

    #[test]
    fn steps_run_in_order() {
        calls();
        let plan = uninstall_plan(true, true);
        assert_eq!(run_plan(&plan, "x.exe", &ok_ops()), Ok(vec![]));
        assert_eq!(
            calls(),
            ["quit", "startup", "shortcut", "data", "program", "entry"]
        );
    }

    #[test]
    fn quit_failure_aborts_before_any_change() {
        calls();
        let ops = UninstallOps {
            quit_running: || call("quit", Err("stuck".into())),
            ..ok_ops()
        };
        let plan = uninstall_plan(true, true);
        assert_eq!(run_plan(&plan, "x.exe", &ops), Err(QUIT_FAILED.into()));
        assert_eq!(calls(), ["quit"]);
    }

    #[test]
    fn failures_are_collected_and_rest_still_run() {
        calls();
        let ops = UninstallOps {
            remove_shortcut: || call("shortcut", Err("busy".into())),
            ..ok_ops()
        };
        let plan = uninstall_plan(true, true);
        assert_eq!(
            run_plan(&plan, "x.exe", &ops),
            Ok(vec![UninstallStep::RemoveShortcut])
        );
        assert_eq!(
            calls(),
            ["quit", "startup", "shortcut", "data", "program", "entry"]
        );
    }

    #[test]
    fn entry_kept_when_program_not_removed() {
        calls();
        let ops = UninstallOps {
            remove_program: |_| call("program", Err("in use".into())),
            ..ok_ops()
        };
        let plan = uninstall_plan(true, true);
        assert_eq!(
            run_plan(&plan, "x.exe", &ops),
            Ok(vec![UninstallStep::RemoveProgram])
        );
        assert_eq!(calls(), ["quit", "startup", "shortcut", "data", "program"]);
    }

    #[test]
    fn portable_plan_never_touches_program_or_entry() {
        calls();
        let plan = uninstall_plan(false, true);
        assert_eq!(run_plan(&plan, "x.exe", &ok_ops()), Ok(vec![]));
        assert_eq!(calls(), ["quit", "startup", "data"]);
    }

    const EXE: &str = r"C:\Program Files\Lite Bar\lite-bar.exe";
    const DIR: &str = r"C:\Program Files\Lite Bar";

    #[test]
    fn cleanup_command_waits_for_the_process_then_deletes() {
        assert_eq!(
            cleanup_command(42, EXE, DIR, true),
            r"Wait-Process -Id 42 -ErrorAction SilentlyContinue; Remove-Item -LiteralPath 'C:\Program Files\Lite Bar\lite-bar.exe' -Force -ErrorAction SilentlyContinue; Remove-Item -LiteralPath 'C:\Program Files\Lite Bar' -Recurse -Force -ErrorAction SilentlyContinue"
        );
        assert_eq!(
            cleanup_command(42, EXE, DIR, false),
            r"Wait-Process -Id 42 -ErrorAction SilentlyContinue; Remove-Item -LiteralPath 'C:\Program Files\Lite Bar\lite-bar.exe' -Force -ErrorAction SilentlyContinue"
        );
    }

    #[test]
    fn cleanup_command_doubles_single_quotes() {
        assert_eq!(
            cleanup_command(1, r"C:\It's\lite-bar.exe", r"C:\It's", true),
            r"Wait-Process -Id 1 -ErrorAction SilentlyContinue; Remove-Item -LiteralPath 'C:\It''s\lite-bar.exe' -Force -ErrorAction SilentlyContinue; Remove-Item -LiteralPath 'C:\It''s' -Recurse -Force -ErrorAction SilentlyContinue"
        );
    }

    #[test]
    fn cleanup_command_keeps_percent_and_ampersand_literal() {
        assert_eq!(
            cleanup_command(1, r"C:\a%PATH%&b\lite-bar.exe", r"C:\a%PATH%&b", false),
            r"Wait-Process -Id 1 -ErrorAction SilentlyContinue; Remove-Item -LiteralPath 'C:\a%PATH%&b\lite-bar.exe' -Force -ErrorAction SilentlyContinue"
        );
    }

    #[test]
    fn folder_is_removed_only_with_a_listing_and_program_files() {
        let only_exe = || Ok(vec!["lite-bar.exe".to_string()]);
        let pf = Some(r"C:\Program Files");
        assert!(removes_folder(only_exe(), DIR, pf));
        assert!(!removes_folder(Err("denied".into()), DIR, pf));
        assert!(!removes_folder(only_exe(), DIR, None));
        assert!(!removes_folder(
            Ok(vec!["lite-bar.exe".into(), "notes.txt".into()]),
            DIR,
            pf
        ));
    }
}

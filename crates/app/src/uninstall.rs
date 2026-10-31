//! Runs the uninstall steps (the plan and the messages: `lite_bar_core::uninstall`). The
//! steps go through `UninstallOps`, so the tests run the plan with fakes and never remove
//! anything.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use lite_bar_core::install::same_user;
use lite_bar_core::registration::{is_installed_copy, SHORTCUT_FILE, UNINSTALL_KEY};
use lite_bar_core::startup_task::StartupState;
use lite_bar_core::uninstall::{
    finish_message, folder_removable, uninstall_args, uninstall_plan, UninstallStep, OTHER_ACCOUNT,
    QUIT_FAILED,
};
use lite_bar_core::uninstall_ui::{UninstallAction, UninstallStatus};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};
use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, FindWindowW, GetMessageW, PostMessageW,
    TranslateMessage, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WM_APP, WM_CLOSE, WS_OVERLAPPED,
};

use crate::elevation::{is_elevated, run_as_admin};
use crate::log;
use crate::registration::{common_programs, delete_key};
use crate::single_instance::{acquire_single_instance, wait_for};
use crate::startup::{
    current_user, query, schtasks_path, set_now, ElevatedError, SystemRunner, CREATE_NO_WINDOW,
};
use crate::uninstall_window::{UninstallState, UninstallUi};

/// Name of the uninstall thread; the panic hook leaves the ETW trace alone for it.
pub const UNINSTALL_THREAD: &str = "uninstall";

/// Posted to the owner window with the uninstall's result (`UninstallResult`).
pub const WM_UNINSTALL_RESULT: u32 = WM_APP + 8;

/// What the worker reports: the steps that failed, or why nothing was changed.
type UninstallResult = Result<Vec<UninstallStep>, String>;

/// Shown when the worker panicked.
const PANICKED: &str = "The uninstall stopped unexpectedly. See the log for details.";

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

/// How the `--uninstall` mode starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// Not elevated: start an elevated copy and leave.
    Relaunch,
    /// Elevated as another account: say so.
    Refuse,
    /// Show the confirmation.
    Confirm,
}

/// `for_user` is the `--for` account the elevated copy was started for, `me` the account
/// this process runs as.
pub fn uninstall_entry(elevated: bool, for_user: Option<&str>, me: &str) -> Entry {
    match for_user {
        _ if !elevated => Entry::Relaunch,
        Some(u) if !same_user(u, me) => Entry::Refuse,
        _ => Entry::Confirm,
    }
}

/// Whether the message loop goes on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    Exit,
}

/// Starts the worker for `(owner, installed, delete_data)`; false when it couldn't start.
type StartJob = fn(HWND, bool, bool) -> bool;

/// The open window and what the user chose so far.
struct Session {
    ui: UninstallUi,
    owner: HWND,
    installed: bool,
    delete_data: bool,
    working: bool,
    start: StartJob,
}

impl Session {
    fn new(ui: UninstallUi, owner: HWND, installed: bool, start: StartJob) -> Session {
        Session {
            ui,
            owner,
            installed,
            delete_data: false,
            working: false,
            start,
        }
    }

    /// Applies what the page queued. Cancel and Close end it, except while the uninstall
    /// runs: that finishes first (`on_result` ends it).
    fn drain(&mut self) -> Flow {
        for action in self.ui.take() {
            match action {
                UninstallAction::SetDeleteData(on) => {
                    self.delete_data = on;
                    self.ui.set_delete_data(on);
                }
                UninstallAction::Uninstall if !self.working => {
                    self.working = true;
                    self.ui.set_status(&UninstallStatus::Working);
                    if !(self.start)(self.owner, self.installed, self.delete_data) {
                        self.working = false;
                        let msg = "Couldn't start the uninstall.".to_string();
                        self.ui.set_status(&UninstallStatus::Failed(msg));
                    }
                }
                UninstallAction::Cancel | UninstallAction::Close if !self.working => {
                    return Flow::Exit;
                }
                _ => {}
            }
        }
        Flow::Continue
    }

    /// Shows the result. Ends the loop when the user closed the window meanwhile.
    fn on_result(&mut self, result: UninstallResult) -> Flow {
        self.working = false;
        let status = match result {
            Ok(failed) => UninstallStatus::Done(finish_message(&failed)),
            Err(e) => UninstallStatus::Failed(e),
        };
        self.ui.set_status(&status);
        if self.ui.is_open() {
            Flow::Continue
        } else {
            Flow::Exit
        }
    }
}

/// Runs the plan on its own thread and posts the result to `owner`.
fn start_job(owner: HWND, installed: bool, delete_data: bool) -> bool {
    // HWND is not Send; the worker only posts to it.
    let raw = owner.0 as usize;
    std::thread::Builder::new()
        .name(UNINSTALL_THREAD.into())
        .spawn(move || {
            let result = std::panic::catch_unwind(|| {
                log::info(&format!(
                    "uninstall started (installed: {installed}, delete data: {delete_data})"
                ));
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                let plan = uninstall_plan(installed, delete_data);
                run_plan(&plan, &exe.to_string_lossy(), &real_ops())
            })
            .unwrap_or_else(|_| Err(PANICKED.into()));
            // Dropped (harmlessly) when the data step deleted the log folder.
            log::info(&format!("uninstall finished: {result:?}"));
            crate::updater::post_boxed(
                HWND(raw as *mut std::ffi::c_void),
                WM_UNINSTALL_RESULT,
                result,
            );
        })
        .is_ok()
}

/// A message-only window that receives the worker's result; the confirmation window can be
/// closed while the worker still runs.
fn create_owner() -> Option<HWND> {
    // SAFETY: plain FFI; the predefined STATIC class needs no registration.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!(""),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
    }
    .ok()
}

/// The `--uninstall` mode: relaunches elevated when needed, otherwise shows the
/// confirmation and uninstalls on a worker thread. Returns the process exit code. Takes no
/// single-instance mutex: `quit_running` waits for the running Lite Bar to release it.
pub fn run_uninstall_mode(for_user: Option<String>) -> i32 {
    let me = current_user();
    match uninstall_entry(is_elevated(), for_user.as_deref(), &me) {
        Entry::Relaunch => {
            match run_as_admin(&uninstall_args(&me)) {
                Ok(()) => log::info("uninstall: started the elevated copy"),
                Err(ElevatedError::Declined) => {
                    log::info("uninstall: the administrator prompt was declined");
                }
                Err(ElevatedError::Other(e)) => {
                    log::warn(&format!("uninstall: couldn't start the elevated copy: {e}"));
                }
            }
            0
        }
        entry => show_window(entry),
    }
}

/// Opens the window (`Confirm` or `Refuse`) and runs its message loop until it ends.
fn show_window(entry: Entry) -> i32 {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let installed = crate::mover::program_files().is_some_and(|pf| is_installed_copy(&exe, &pf));
    let status = if entry == Entry::Refuse {
        UninstallStatus::Failed(OTHER_ACCOUNT.into())
    } else {
        UninstallStatus::Confirm
    };
    let Some(owner) = create_owner() else {
        log::error("uninstall: couldn't create the owner window");
        return 1;
    };
    let ui = UninstallUi::open(UninstallState {
        installed,
        data_dir: crate::paths::app_dir().to_string_lossy().into_owned(),
        delete_data: false,
        status,
    });
    let ui = match ui {
        Ok(ui) => ui,
        Err(e) => {
            log::error(&format!("uninstall: couldn't open the window: {e}"));
            // SAFETY: our own window, destroyed once.
            let _ = unsafe { DestroyWindow(owner) };
            return 1;
        }
    };
    let mut session = Session::new(ui, owner, installed, start_job);
    // SAFETY: standard message loop on the thread that owns the windows.
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let flow = if msg.hwnd == owner && msg.message == WM_UNINSTALL_RESULT {
                // SAFETY: posted by `start_job` with an `UninstallResult`; taken once.
                session.on_result(take_result(msg.lParam))
            } else {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                session.drain()
            };
            if flow == Flow::Exit {
                break;
            }
        }
        let _ = DestroyWindow(owner);
    }
    0
}

/// Takes back the result a `WM_UNINSTALL_RESULT` carries.
///
/// # Safety
/// `lparam` must come from a `WM_UNINSTALL_RESULT` posted by `start_job`, taken once.
unsafe fn take_result(lparam: LPARAM) -> UninstallResult {
    // SAFETY: the caller's guarantee.
    unsafe { crate::updater::take_boxed(lparam) }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use lite_bar_core::uninstall::{finish_message, uninstall_plan, UninstallStep, QUIT_FAILED};

    use super::*;
    use crate::test_util::MsgWindow;

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

    #[test]
    fn entry_rules() {
        let me = r"PC\me";
        assert_eq!(uninstall_entry(false, None, me), Entry::Relaunch);
        assert_eq!(uninstall_entry(false, Some(me), me), Entry::Relaunch);
        assert_eq!(uninstall_entry(true, None, me), Entry::Confirm);
        assert_eq!(uninstall_entry(true, Some(r"pc\ME"), me), Entry::Confirm);
        assert_eq!(uninstall_entry(true, Some(r"PC\other"), me), Entry::Refuse);
        // An unknown current user can't be matched.
        assert_eq!(uninstall_entry(true, Some(me), ""), Entry::Refuse);
    }

    thread_local! {
        static STARTED: RefCell<Vec<(bool, bool)>> = const { RefCell::new(Vec::new()) };
    }

    fn fake_start(_owner: HWND, installed: bool, delete_data: bool) -> bool {
        STARTED.with(|s| s.borrow_mut().push((installed, delete_data)));
        true
    }

    fn session(owner: &MsgWindow) -> Session {
        STARTED.with(|s| s.borrow_mut().clear());
        let ui = UninstallUi::open_hidden(UninstallState {
            installed: true,
            data_dir: r"C:\Users\x\AppData\Roaming\LiteBar".into(),
            delete_data: false,
            status: UninstallStatus::Confirm,
        })
        .unwrap();
        Session::new(ui, owner.0, true, fake_start)
    }

    fn started() -> Vec<(bool, bool)> {
        STARTED.with(|s| s.borrow().clone())
    }

    #[test]
    fn cancel_ends_the_loop_without_starting() {
        let owner = MsgWindow::new();
        let mut s = session(&owner);
        s.ui.push_for_test(UninstallAction::Cancel);
        assert_eq!(s.drain(), Flow::Exit);
        assert!(started().is_empty());
    }

    #[test]
    fn uninstall_starts_the_job_with_the_toggle_and_shows_working() {
        let owner = MsgWindow::new();
        let mut s = session(&owner);
        s.ui.push_for_test(UninstallAction::SetDeleteData(true));
        s.ui.push_for_test(UninstallAction::Uninstall);
        assert_eq!(s.drain(), Flow::Continue);
        assert_eq!(started(), vec![(true, true)]);
        assert_eq!(s.ui.status(), UninstallStatus::Working);
        // A second click while working starts nothing more.
        s.ui.push_for_test(UninstallAction::Uninstall);
        s.drain();
        assert_eq!(started().len(), 1);
    }

    #[test]
    fn result_shows_the_message_and_close_ends() {
        let owner = MsgWindow::new();
        let mut s = session(&owner);
        s.ui.push_for_test(UninstallAction::Uninstall);
        s.drain();
        assert_eq!(
            s.on_result(Ok(vec![UninstallStep::RemoveShortcut])),
            Flow::Continue
        );
        assert_eq!(
            s.ui.status(),
            UninstallStatus::Done(finish_message(&[UninstallStep::RemoveShortcut]))
        );
        s.ui.push_for_test(UninstallAction::Close);
        assert_eq!(s.drain(), Flow::Exit);
    }

    #[test]
    fn failed_result_is_shown_as_failed() {
        let owner = MsgWindow::new();
        let mut s = session(&owner);
        s.ui.push_for_test(UninstallAction::Uninstall);
        s.drain();
        s.on_result(Err(QUIT_FAILED.into()));
        assert_eq!(s.ui.status(), UninstallStatus::Failed(QUIT_FAILED.into()));
    }

    #[test]
    fn closing_the_window_mid_uninstall_waits_for_the_result() {
        let owner = MsgWindow::new();
        let mut s = session(&owner);
        s.ui.push_for_test(UninstallAction::Uninstall);
        s.drain();
        s.ui.send_close();
        assert_eq!(s.drain(), Flow::Continue);
        assert_eq!(s.on_result(Ok(vec![])), Flow::Exit);
    }
}

//! Moving lite-bar.exe into `<Program Files>\Lite Bar` (see `lite_bar_core::install`).
//!
//! A Lite Bar that is not elevated starts `lite-bar.exe --move-to "<dir>" --for "<user>"` as
//! administrator (one UAC prompt) and waits for it; that process (`run_move_mode`) copies
//! the exe, re-points the "Start with Windows" task if it started the old exe, starts the
//! new copy with `--moved-from "<old exe>"` and exits with a `HelperExit` code. An elevated
//! Lite Bar does the same in-process. Everything here blocks, so the app runs it on a worker
//! thread (`spawn`) that posts a `MoveOutcome` back as `WM_MOVE_RESULT`.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use lite_bar_core::install::{
    is_allowed_target, move_to_args, moved_from_args, safe_to_delete, same_path, same_user,
    target_dir, HelperExit, MoveArgs, MoveOutcome, EXE_NAME,
};
use lite_bar_core::startup_task::StartupState;
use windows::core::{GUID, HSTRING};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, SHGetKnownFolderPath, ShellExecuteW,
    KF_FLAG_DEFAULT,
};
use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWNORMAL, WM_APP};

use crate::elevation::is_elevated;
use crate::installer::{
    move_file, remove_with_retries, shell_execute_result, CLEANUP_DELAY, CLEANUP_TRIES,
};
use crate::log;
use crate::startup::{
    current_user, query, run_elevated_helper, schtasks_path, set, CommandRunner, ElevatedError,
    SystemRunner, PANICKED,
};
use crate::updater::post_boxed;

/// Posted to the owner window when a move finished; `LPARAM` is a `Box<MoveOutcome>`.
pub const WM_MOVE_RESULT: u32 = WM_APP + 6;

/// Name of the move's worker thread; the panic hook leaves the ETW trace alone for it.
pub const WORKER_THREAD: &str = "move-to-program-files";

/// Name of the thread that deletes the old copy after a move.
pub const DELETE_THREAD: &str = "move-cleanup";

/// What a failed helper's outcome says; the details are in its log.
const SEE_LOG: &str = "see lite-bar.log";

/// The Program Files folders: `FOLDERID_ProgramFiles`, then `FOLDERID_ProgramFilesX86`
/// (once when they are the same). One that can't be found is skipped.
pub fn program_files_roots() -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    for id in [&FOLDERID_ProgramFiles, &FOLDERID_ProgramFilesX86] {
        if let Some(path) = known_folder(id) {
            if !roots.iter().any(|r| same_path(r, &path)) {
                roots.push(path);
            }
        }
    }
    roots
}

fn known_folder(id: &GUID) -> Option<String> {
    // SAFETY: plain FFI; on success the returned string is ours to free, once, below.
    let p = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.ok()?;
    // SAFETY: `p` is a valid NUL-terminated string until it is freed.
    let path = unsafe { p.to_string() };
    // SAFETY: allocated by the shell with the COM allocator.
    unsafe { CoTaskMemFree(Some(p.0 as *const c_void)) };
    path.ok().filter(|s| !s.is_empty())
}

/// The Program Files folder the exe moves under (the first of `program_files_roots`).
pub fn program_files() -> Option<String> {
    program_files_roots().into_iter().next()
}

/// Copies `exe` into `dir` (created if missing) as `lite-bar.exe`: first to
/// `lite-bar.exe.new`, checked to be as long as `exe`, then moved over any existing
/// `lite-bar.exe`. On any error the `.new` file is removed. Returns the new exe's path.
pub fn copy_into(exe: &Path, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    let target = dir.join(EXE_NAME);
    let new = dir.join(format!("{EXE_NAME}.new"));
    let placed = place(exe, &new, &target);
    if placed.is_err() {
        let _ = std::fs::remove_file(&new);
    }
    placed.map(|()| target)
}

fn place(exe: &Path, new: &Path, target: &Path) -> Result<(), String> {
    std::fs::copy(exe, new).map_err(|e| format!("couldn't copy to {}: {e}", new.display()))?;
    let len = |p: &Path| {
        std::fs::metadata(p)
            .map(|m| m.len())
            .map_err(|e| format!("couldn't check {}: {e}", p.display()))
    };
    let (want, got) = (len(exe)?, len(new)?);
    if want != got {
        return Err(format!(
            "the copy has {got} bytes, not {want}: {}",
            new.display()
        ));
    }
    move_file(new, target).map_err(|e| format!("couldn't replace {}: {e}", target.display()))
}

/// Re-creates the "Start with Windows" task for `new` when it starts `old`: `Ok(true)` when
/// re-created, `Ok(false)` when there is none or it starts another copy.
pub fn repoint_startup(
    runner: &dyn CommandRunner,
    schtasks: &str,
    old: &str,
    new: &str,
    user: &str,
    temp: &Path,
) -> Result<bool, String> {
    match query(runner, schtasks, old) {
        StartupState::On => set(runner, schtasks, true, new, user, temp).map(|()| true),
        StartupState::Error { msg, .. } => Err(msg),
        _ => Ok(false),
    }
}

/// `repoint_startup` for this user with the real schtasks. Needs elevation to re-create.
pub fn repoint_startup_now(old: &str, new: &str) -> Result<bool, String> {
    let temp = std::env::temp_dir();
    repoint_startup(
        &SystemRunner,
        &schtasks_path(),
        old,
        new,
        &current_user(),
        &temp,
    )
}

/// Starts the moved copy `new` with `--moved-from "<old>"`, in its own folder. An elevated
/// caller's child is elevated too.
pub fn start_moved(new: &Path, old: &Path) -> Result<(), String> {
    let file = HSTRING::from(new.as_os_str());
    let dir = HSTRING::from(new.parent().unwrap_or(Path::new("")).as_os_str());
    let args = HSTRING::from(moved_from_args(&old.to_string_lossy()));
    // SAFETY: valid NUL-terminated strings for the duration of the call.
    let result = unsafe {
        ShellExecuteW(
            None,
            windows::core::w!("open"),
            &file,
            &args,
            &dir,
            SW_SHOWNORMAL,
        )
    };
    shell_execute_result(result.0 as usize)
}

/// The move steps for `user`: copies `exe` into `dir`, re-points the startup task (a
/// failure is only logged) and starts the new copy. Each error is logged. Needs elevation.
pub fn perform_move(exe: &Path, dir: &Path, user: &str) -> HelperExit {
    exit_code(&move_steps(exe, dir, user))
}

/// The exit code that reports `outcome`.
fn exit_code(outcome: &MoveOutcome) -> HelperExit {
    match outcome {
        MoveOutcome::Moved => HelperExit::Moved,
        MoveOutcome::OtherAccount => HelperExit::OtherAccount,
        MoveOutcome::MovedNotStarted(_) => HelperExit::MovedNotStarted,
        MoveOutcome::Declined | MoveOutcome::Failed(_) => HelperExit::Failed,
    }
}

/// `perform_move` with the error texts.
fn move_steps(exe: &Path, dir: &Path, user: &str) -> MoveOutcome {
    let new = match copy_into(exe, dir) {
        Ok(new) => new,
        Err(e) => {
            log::error(&format!("move to Program Files failed: {e}"));
            return MoveOutcome::Failed(e);
        }
    };
    let (old_s, new_s) = (exe.to_string_lossy(), new.to_string_lossy());
    let temp = std::env::temp_dir();
    match repoint_startup(&SystemRunner, &schtasks_path(), &old_s, &new_s, user, &temp) {
        Ok(true) => log::info(&format!("startup task now starts {new_s}")),
        Ok(false) => {}
        Err(e) => log::error(&format!("startup task not re-pointed after the move: {e}")),
    }
    match start_moved(&new, exe) {
        Ok(()) => {
            log::info(&format!("copied {old_s} to {new_s} and started it"));
            MoveOutcome::Moved
        }
        Err(e) => {
            log::error(&format!("copied to {new_s}, but couldn't start it: {e}"));
            MoveOutcome::MovedNotStarted(e)
        }
    }
}

/// Whether the helper may move into `dir`: only `<Program Files>\Lite Bar` is.
fn target_allowed(dir: &str) -> bool {
    program_files().is_some_and(|pf| is_allowed_target(dir, &pf))
}

/// The `--move-to "<dir>" --for "<user>"` helper: returns its `HelperExit` code. Refuses
/// without elevation, and changes nothing when it runs as another account than `--for` or
/// `<dir>` isn't `<Program Files>\Lite Bar`.
pub fn run_move_mode(args: Result<MoveArgs, String>) -> i32 {
    let args = match args {
        Ok(args) => args,
        Err(e) => {
            log::error(&e);
            return HelperExit::Failed as i32;
        }
    };
    if !is_elevated() {
        log::error("move: --move-to needs administrator rights");
        return HelperExit::Failed as i32;
    }
    let me = current_user();
    if !same_user(&me, &args.for_user) {
        log::error(&format!(
            "move: running as {me}, not {}; nothing changed",
            args.for_user
        ));
        return HelperExit::OtherAccount as i32;
    }
    if !target_allowed(&args.dir) {
        log::error(&format!(
            "move: {} isn't the Program Files target; nothing changed",
            args.dir
        ));
        return HelperExit::Failed as i32;
    }
    match std::env::current_exe() {
        Ok(exe) => perform_move(&exe, Path::new(&args.dir), &args.for_user) as i32,
        Err(e) => {
            log::error(&format!("move: can't find Lite Bar's own file: {e}"));
            HelperExit::Failed as i32
        }
    }
}

/// Moves this exe to `<Program Files>\Lite Bar`. An elevated Lite Bar does it in-process;
/// otherwise through the elevated helper (`parent` owns the UAC prompt). Blocks: run it on
/// a worker thread.
pub fn move_now(elevated: bool, parent: usize) -> MoveOutcome {
    let Some(dir) = program_files().map(|pf| target_dir(&pf)) else {
        log::error("move: can't find the Program Files folder");
        return MoveOutcome::Failed("can't find the Program Files folder".into());
    };
    if elevated {
        return match std::env::current_exe() {
            Ok(exe) => move_steps(&exe, Path::new(&dir), &current_user()),
            Err(e) => MoveOutcome::Failed(format!("can't find Lite Bar's own file: {e}")),
        };
    }
    match run_elevated_helper(&move_to_args(&dir, &current_user()), parent) {
        Ok(code) => {
            let exit = HelperExit::from_code(code);
            if exit != HelperExit::Moved {
                log::warn(&format!(
                    "move: the elevated helper exited with {code} (or didn't finish)"
                ));
            }
            outcome_of(exit)
        }
        Err(ElevatedError::Declined) => {
            log::info("move: the administrator prompt was declined");
            MoveOutcome::Declined
        }
        Err(ElevatedError::Other(e)) => {
            log::warn(&format!("move: couldn't start the elevated helper: {e}"));
            MoveOutcome::Failed(e)
        }
    }
}

/// The outcome a helper's exit code reports; its error texts are in its log.
fn outcome_of(exit: HelperExit) -> MoveOutcome {
    match exit {
        HelperExit::Moved => MoveOutcome::Moved,
        HelperExit::OtherAccount => MoveOutcome::OtherAccount,
        HelperExit::MovedNotStarted => MoveOutcome::MovedNotStarted(SEE_LOG.into()),
        HelperExit::Failed => MoveOutcome::Failed(SEE_LOG.into()),
    }
}

/// Starts `job` on a new thread and posts its outcome (`Failed` if it panics) to `owner`.
/// Returns false when the thread could not be started.
pub fn spawn(owner: HWND, job: impl FnOnce() -> MoveOutcome + Send + 'static) -> bool {
    // HWND is not Send; the worker only posts to it.
    let raw = owner.0 as usize;
    std::thread::Builder::new()
        .name(WORKER_THREAD.into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                .unwrap_or_else(|_| MoveOutcome::Failed(PANICKED.into()));
            post_boxed(HWND(raw as *mut c_void), WM_MOVE_RESULT, outcome);
        })
        .is_ok()
}

/// Takes back the outcome a `WM_MOVE_RESULT` carries.
///
/// # Safety
/// `lparam` must come from a `WM_MOVE_RESULT` posted by `spawn`, taken once.
pub unsafe fn take_result(lparam: LPARAM) -> MoveOutcome {
    // SAFETY: the caller's guarantee.
    unsafe { crate::updater::take_boxed(lparam) }
}

/// After a move: deletes the old copy `old` on its own thread, with retries (the old
/// instance may still be exiting), only when `safe_to_delete` allows it. The result is
/// logged. Returns the thread, or `None` when nothing is deleted.
pub fn spawn_delete_old(
    old: String,
    running: String,
    roots: Vec<String>,
) -> Option<JoinHandle<()>> {
    if !safe_to_delete(&old, &running, &roots) {
        log::warn(&format!("not deleting the old copy {old}"));
        return None;
    }
    std::thread::Builder::new()
        .name(DELETE_THREAD.into())
        .spawn(move || {
            let path = PathBuf::from(&old);
            let result = std::panic::catch_unwind(|| {
                remove_with_retries(&path, CLEANUP_TRIES, CLEANUP_DELAY, std::thread::sleep)
            })
            .unwrap_or_else(|_| Err(format!("could not remove {old}: {PANICKED}")));
            match result {
                Ok(true) => log::info(&format!("removed the old copy {old}")),
                Ok(false) => {}
                Err(e) => log::warn(&e),
            }
        })
        .inspect_err(|e| log::error(&format!("could not start the delete thread: {e}")))
        .ok()
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    use lite_bar_core::startup_task::{query_args, query_xml_args, task_xml};

    use super::*;
    use crate::startup::Output;
    use crate::test_util::{FakeRunner, TempDir};

    const SCHTASKS: &str = r"C:\Windows\System32\schtasks.exe";
    const OLD: &str = r"C:\Users\paymon\Downloads\lite-bar.exe";
    const NEW: &str = r"C:\Program Files\Lite Bar\lite-bar.exe";
    const USER: &str = r"PC\paymon";

    fn exit(code: i32) -> Output {
        Ok((code, Vec::new()))
    }

    fn printed(xml: &str) -> Output {
        Ok((0, xml.as_bytes().to_vec()))
    }

    fn creates(r: &FakeRunner) -> Vec<Vec<String>> {
        r.calls()
            .into_iter()
            .filter(|c| c.first().map(String::as_str) == Some("/Create"))
            .collect()
    }

    #[test]
    fn copy_into_creates_dir_and_replaces_existing() {
        let tmp = TempDir::new("mover-copy");
        let src = tmp.path().join("src").join("lite-bar.exe");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, b"MZ!").unwrap();
        let dir = tmp.path().join("Program Files").join("Lite Bar");
        // `dir` doesn't exist yet: it is created.
        let got = copy_into(&src, &dir).unwrap();
        assert_eq!(got, dir.join("lite-bar.exe"));
        assert_eq!(std::fs::read(&got).unwrap(), b"MZ!");
        assert!(!dir.join("lite-bar.exe.new").exists());
        // Again over the existing copy.
        std::fs::write(dir.join("lite-bar.exe"), b"an older copy").unwrap();
        let got = copy_into(&src, &dir).unwrap();
        assert_eq!(std::fs::read(&got).unwrap(), b"MZ!");
        assert!(!dir.join("lite-bar.exe.new").exists());
    }

    #[test]
    fn copy_into_failure_leaves_no_new_file() {
        let tmp = TempDir::new("mover-copy-fail");
        let src = tmp.path().join("lite-bar.exe");
        std::fs::write(&src, b"MZ!").unwrap();
        let dir = tmp.path().join("target");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("lite-bar.exe");
        std::fs::write(&target, b"in use").unwrap();
        let held = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&target)
            .unwrap();
        assert!(copy_into(&src, &dir).is_err());
        drop(held);
        assert!(!dir.join("lite-bar.exe.new").exists());
        assert_eq!(std::fs::read(&src).unwrap(), b"MZ!");
        assert_eq!(std::fs::read(&target).unwrap(), b"in use");
    }

    #[test]
    fn repoint_startup_only_when_task_points_at_old() {
        let tmp = TempDir::new("mover-repoint");
        // Points at the old exe: re-created for the new one.
        let r = FakeRunner::new(vec![exit(0), printed(&task_xml(OLD, USER)), exit(0)]);
        assert_eq!(
            repoint_startup(&r, SCHTASKS, OLD, NEW, USER, tmp.path()),
            Ok(true)
        );
        assert_eq!(r.calls()[..2], [query_args(), query_xml_args()]);
        assert_eq!(creates(&r).len(), 1);
        let xml = crate::startup::decode_output(&r.seen_file.borrow().clone().unwrap());
        assert!(xml.contains(NEW), "{xml}");

        // Points at another copy: left alone.
        let other = task_xml(r"D:\other\lite-bar.exe", USER);
        let r = FakeRunner::new(vec![exit(0), printed(&other)]);
        assert_eq!(
            repoint_startup(&r, SCHTASKS, OLD, NEW, USER, tmp.path()),
            Ok(false)
        );
        assert!(creates(&r).is_empty());

        // No task.
        let r = FakeRunner::new(vec![exit(1)]);
        assert_eq!(
            repoint_startup(&r, SCHTASKS, OLD, NEW, USER, tmp.path()),
            Ok(false)
        );
        assert!(creates(&r).is_empty());

        // The create fails: an error.
        let r = FakeRunner::new(vec![exit(0), printed(&task_xml(OLD, USER)), exit(1)]);
        assert!(repoint_startup(&r, SCHTASKS, OLD, NEW, USER, tmp.path()).is_err());

        // The task can't be checked: an error, and nothing is created.
        let r = FakeRunner::new(vec![Err("no schtasks".into())]);
        assert!(repoint_startup(&r, SCHTASKS, OLD, NEW, USER, tmp.path()).is_err());
        assert!(creates(&r).is_empty());
    }

    #[test]
    fn only_the_program_files_target_is_accepted() {
        let tmp = TempDir::new("mover-target");
        assert!(!target_allowed(&tmp.path().to_string_lossy()));
        assert!(!target_allowed(""));
        let pf = program_files().unwrap();
        assert!(target_allowed(&target_dir(&pf)));
        assert!(!target_allowed(&pf));
    }

    #[test]
    fn run_move_mode_refuses_other_account() {
        let tmp = TempDir::new("mover-other-account");
        let dir = tmp.path().join("Lite Bar");
        let code = run_move_mode(Ok(MoveArgs {
            dir: dir.to_string_lossy().into_owned(),
            for_user: r"NOPE\nobody".into(),
        }));
        let expected = if is_elevated() { 2 } else { 1 };
        assert_eq!(code, expected);
        assert!(!dir.join("lite-bar.exe").exists());
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[test]
    fn helper_exit_codes_and_outcomes_match() {
        let see_log = || "see lite-bar.log".to_string();
        assert_eq!(outcome_of(HelperExit::Moved), MoveOutcome::Moved);
        assert_eq!(
            outcome_of(HelperExit::OtherAccount),
            MoveOutcome::OtherAccount
        );
        assert_eq!(
            outcome_of(HelperExit::Failed),
            MoveOutcome::Failed(see_log())
        );
        assert_eq!(
            outcome_of(HelperExit::MovedNotStarted),
            MoveOutcome::MovedNotStarted(see_log())
        );
        for exit in [
            HelperExit::Moved,
            HelperExit::Failed,
            HelperExit::OtherAccount,
            HelperExit::MovedNotStarted,
        ] {
            assert_eq!(exit_code(&outcome_of(exit)), exit);
        }
        assert_eq!(exit_code(&MoveOutcome::Declined), HelperExit::Failed);
    }

    #[test]
    fn run_move_mode_bad_args_fails() {
        assert_eq!(run_move_mode(Err("x".into())), 1);
    }

    #[test]
    fn program_files_roots_are_known_folders() {
        let roots = program_files_roots();
        assert!(!roots.is_empty());
        for r in &roots {
            assert!(Path::new(r).is_absolute(), "{r}");
            assert!(!r.ends_with('\\'), "{r}");
        }
        let pf = std::env::var("ProgramFiles").unwrap();
        assert!(roots.iter().any(|r| same_path(r, &pf)), "{roots:?}");
        assert_eq!(
            program_files().as_deref(),
            roots.first().map(String::as_str)
        );
    }

    #[test]
    fn delete_old_is_refused_for_unsafe_paths() {
        let tmp = TempDir::new("mover-delete-unsafe");
        let other = tmp.path().join("notes.txt");
        std::fs::write(&other, b"keep").unwrap();
        let path = other.to_string_lossy().into_owned();
        assert!(spawn_delete_old(path, "C:\\x\\lite-bar.exe".into(), vec![]).is_none());
        assert!(other.exists());
    }

    #[test]
    fn delete_old_removes_the_old_copy() {
        let tmp = TempDir::new("mover-delete");
        let old = tmp.path().join("lite-bar.exe");
        std::fs::write(&old, b"MZ old").unwrap();
        let worker = spawn_delete_old(
            old.to_string_lossy().into_owned(),
            r"C:\Program Files\Lite Bar\lite-bar.exe".into(),
            vec![r"C:\Program Files".into()],
        )
        .expect("a delete thread");
        assert_eq!(worker.thread().name(), Some(DELETE_THREAD));
        worker.join().unwrap();
        assert!(!old.exists());
    }

    #[test]
    fn spawned_move_posts_its_outcome_and_a_panic_posts_a_failure() {
        use std::time::{Duration, Instant};
        use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, MSG, PM_REMOVE};
        let w = crate::test_util::MsgWindow::new();
        let wait = || {
            let end = Instant::now() + Duration::from_secs(5);
            let mut msg = MSG::default();
            while Instant::now() < end {
                let got = unsafe {
                    PeekMessageW(
                        &mut msg,
                        Some(w.0),
                        WM_MOVE_RESULT,
                        WM_MOVE_RESULT,
                        PM_REMOVE,
                    )
                };
                if got.as_bool() {
                    return Some(unsafe { take_result(msg.lParam) });
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            None
        };
        assert!(spawn(w.0, || {
            assert_eq!(std::thread::current().name(), Some(WORKER_THREAD));
            MoveOutcome::Declined
        }));
        assert_eq!(wait(), Some(MoveOutcome::Declined));
        assert!(spawn(w.0, || panic!("boom")));
        assert_eq!(
            wait(),
            Some(MoveOutcome::Failed(crate::startup::PANICKED.into()))
        );
    }
}

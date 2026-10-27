//! "Start with Windows": creates, deletes and queries the elevated logon task through
//! `schtasks.exe` (see `lite_bar_core::startup_task`).
//!
//! Everything here blocks, so the app runs it on a worker thread (`spawn`) that posts a
//! `StartupResult` back to the owner window as `WM_STARTUP_RESULT` (the boxed-`LPARAM`
//! scheme of `updater::post_boxed`). A Lite Bar that is not elevated starts
//! `lite-bar.exe --startup on|off` as administrator (one UAC prompt) and waits for it; that
//! process makes the change (`run_mode`) and exits without a tray.

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use lite_bar_core::startup_task::{
    create_args, delete_args, query_args, query_xml_args, state_from_query, task_xml, StartupState,
};
use windows::core::{w, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HWND, LPARAM, WAIT_OBJECT_0};
use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameSamCompatible};
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, WM_APP};

use crate::elevation::is_elevated;
use crate::log;
use crate::updater::post_boxed;

/// Posted to the owner window when a startup-task job finished; `LPARAM` is a
/// `Box<StartupResult>`.
pub const WM_STARTUP_RESULT: u32 = WM_APP + 5;

/// Name of the worker threads; the panic hook leaves the ETW trace alone for them.
pub const WORKER_THREAD: &str = "startup-task";

/// The command-line flag of the elevated helper mode: `--startup on` or `--startup off`.
pub const STARTUP_FLAG: &str = "--startup";

/// `CREATE_NO_WINDOW`: schtasks is a console program; no console flashes up.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// `FILE_SHARE_READ`.
const SHARE_READ: u32 = 0x0000_0001;
/// How long a non-elevated Lite Bar waits for its elevated helper.
const HELPER_WAIT_MS: u32 = 60_000;

/// What a job that panicked reports.
const PANICKED: &str = "Couldn't change the startup task (unexpected error)";

/// What a startup-task job reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartupResult {
    /// The task's state now (or why a change failed).
    State(StartupState),
    /// The user declined the UAC prompt: nothing changed.
    Declined,
}

/// A program's exit code and stdout, or why it couldn't run.
pub type Output = Result<(i32, Vec<u8>), String>;

/// Runs a program and returns its exit code (the seam tests replace).
pub trait CommandRunner {
    fn run(&self, exe: &str, args: &[String]) -> Result<i32, String>;
    /// Like `run`, also returning what it printed to stdout.
    fn output(&self, exe: &str, args: &[String]) -> Output;
}

/// Runs programs for real, without a console window.
pub struct SystemRunner;

impl SystemRunner {
    fn command(exe: &str, args: &[String]) -> Command {
        let mut c = Command::new(exe);
        c.args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        c
    }
}

impl CommandRunner for SystemRunner {
    fn run(&self, exe: &str, args: &[String]) -> Result<i32, String> {
        let status = Self::command(exe, args)
            .stdout(Stdio::null())
            .status()
            .map_err(|e| format!("couldn't run {exe}: {e}"))?;
        Ok(status.code().unwrap_or(-1))
    }

    fn output(&self, exe: &str, args: &[String]) -> Output {
        let out = Self::command(exe, args)
            .output()
            .map_err(|e| format!("couldn't run {exe}: {e}"))?;
        Ok((out.status.code().unwrap_or(-1), out.stdout))
    }
}

/// `--startup on|off` in the command line (without the program name): `Some(Ok(on))`, or
/// `Some(Err(..))` for a missing or unknown value. `None` without the flag.
pub fn parse_flag(mut args: impl Iterator<Item = String>) -> Option<Result<bool, String>> {
    args.find(|a| a == STARTUP_FLAG)?;
    Some(match args.next() {
        Some(v) if v.eq_ignore_ascii_case("on") => Ok(true),
        Some(v) if v.eq_ignore_ascii_case("off") => Ok(false),
        Some(v) => Err(format!("{STARTUP_FLAG} takes on or off, not {v:?}")),
        None => Err(format!("{STARTUP_FLAG} needs on or off")),
    })
}

/// schtasks output as text: UTF-16 (with a BOM, or ASCII-looking with zero high bytes)
/// or UTF-8. `None` when it is neither.
pub fn decode_output(bytes: &[u8]) -> Option<String> {
    let utf16 = |b: &[u8]| {
        let units: Vec<u16> = b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        String::from_utf16(&units).ok()
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest);
    }
    if bytes.len() >= 2 && bytes.len().is_multiple_of(2) && bytes[1] == 0 && bytes[0] != 0 {
        return utf16(bytes);
    }
    String::from_utf8(bytes.to_vec()).ok()
}

/// Whether the task exists (`schtasks /Query` exits 0).
pub fn is_enabled(runner: &dyn CommandRunner, schtasks: &str) -> bool {
    runner.run(schtasks, &query_args()) == Ok(0)
}

/// The task's state for `own_exe`: Off, On, OnOtherCopy, or Error when schtasks can't run.
pub fn query(runner: &dyn CommandRunner, schtasks: &str, own_exe: &str) -> StartupState {
    match runner.run(schtasks, &query_args()) {
        Err(e) => StartupState::Error(format!("Couldn't check the startup task: {e}")),
        Ok(0) => {
            let xml = match runner.output(schtasks, &query_xml_args()) {
                Ok((0, out)) => decode_output(&out),
                _ => None,
            };
            state_from_query(true, xml.as_deref(), own_exe)
        }
        Ok(_) => StartupState::Off,
    }
}

/// Creates the task for `exe` and `user` (replacing any), or deletes it. The XML goes
/// through a file in `temp_dir`, removed afterwards. Deleting a task that is already gone
/// succeeds.
pub fn set(
    runner: &dyn CommandRunner,
    schtasks: &str,
    enabled: bool,
    exe: &str,
    user: &str,
    temp_dir: &Path,
) -> Result<(), String> {
    if !enabled {
        return match runner.run(schtasks, &delete_args())? {
            0 => Ok(()),
            _ if !is_enabled(runner, schtasks) => Ok(()),
            code => Err(format!(
                "schtasks couldn't delete the task (exit code {code})"
            )),
        };
    }
    let bytes = utf16_bom(&task_xml(exe, user));
    let path = xml_file_path(temp_dir);
    let held = write_locked(&path, &bytes)?;
    let code = runner.run(schtasks, &create_args(&path.to_string_lossy()));
    drop(held);
    let _ = std::fs::remove_file(&path);
    match code? {
        0 => Ok(()),
        code => Err(format!(
            "schtasks couldn't create the task (exit code {code})"
        )),
    }
}

fn xml_file_path(temp_dir: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    temp_dir.join(format!(
        "lite-bar-startup-{}-{nanos:09}.xml",
        std::process::id()
    ))
}

/// Writes `bytes` to a new file at `path`, then reopens it read-only with only reading
/// shared and checks it still holds `bytes`. While the returned handle is open nobody can
/// change, rename or delete the file, so schtasks reads exactly what was written. A file
/// already at `path` is an error and is left alone; on any later error the new file is
/// removed.
fn write_locked(path: &Path, bytes: &[u8]) -> Result<File, String> {
    let fail = |e: std::io::Error| format!("couldn't write {}: {e}", path.display());
    let f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(path)
        .map_err(fail)?;
    let held = fill_and_hold(f, path, bytes);
    if held.is_err() {
        let _ = std::fs::remove_file(path);
    }
    held
}

fn fill_and_hold(mut f: File, path: &Path, bytes: &[u8]) -> Result<File, String> {
    let fail = |e: std::io::Error| format!("couldn't write {}: {e}", path.display());
    f.write_all(bytes).map_err(fail)?;
    drop(f);
    let mut held = OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ)
        .open(path)
        .map_err(fail)?;
    let mut back = Vec::new();
    held.read_to_end(&mut back).map_err(fail)?;
    if back != bytes {
        return Err(format!("{} changed while it was written", path.display()));
    }
    Ok(held)
}

/// `s` as UTF-16 LE with a byte order mark.
pub fn utf16_bom(s: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    out.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
    out
}

/// `DOMAIN\user` from `%USERDOMAIN%` and `%USERNAME%`.
pub fn user_from_env(domain: Option<String>, name: Option<String>) -> Option<String> {
    let name = name.filter(|n| !n.is_empty())?;
    Some(match domain.filter(|d| !d.is_empty()) {
        Some(d) => format!("{d}\\{name}"),
        None => name,
    })
}

/// The signed-in user as `DOMAIN\user`.
pub fn current_user() -> String {
    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` u16s; on success `len` is the name's length.
    let ok = unsafe { GetUserNameExW(NameSamCompatible, Some(PWSTR(buf.as_mut_ptr())), &mut len) };
    if ok && len > 0 && (len as usize) < buf.len() {
        return String::from_utf16_lossy(&buf[..len as usize]);
    }
    user_from_env(
        std::env::var("USERDOMAIN").ok(),
        std::env::var("USERNAME").ok(),
    )
    .unwrap_or_default()
}

/// `schtasks.exe` in the system folder (never one found on the search path).
pub fn schtasks_path() -> String {
    let mut buf = [0u16; 260];
    // SAFETY: the buffer and its length are passed together.
    let n = unsafe { GetSystemDirectoryW(Some(&mut buf)) } as usize;
    let dir = if n > 0 && n < buf.len() {
        String::from_utf16_lossy(&buf[..n])
    } else {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        format!(r"{root}\System32")
    };
    format!(r"{dir}\schtasks.exe")
}

/// This exe's path, as the task should start it.
fn own_exe() -> Result<String, String> {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| format!("can't find Lite Bar's own file: {e}"))
}

/// The task's state for this exe, with the real schtasks.
pub fn query_now() -> StartupState {
    match own_exe() {
        Ok(exe) => query(&SystemRunner, &schtasks_path(), &exe),
        Err(e) => StartupState::Error(format!("Couldn't check the startup task: {e}")),
    }
}

/// Creates or deletes the task for this exe, with the real schtasks. Needs elevation.
pub fn set_now(enabled: bool) -> Result<(), String> {
    let exe = own_exe()?;
    set(
        &SystemRunner,
        &schtasks_path(),
        enabled,
        &exe,
        &current_user(),
        &std::env::temp_dir(),
    )
}

/// The `--startup on|off` helper: makes the change, logs it, and returns the exit code
/// (0 on success). Refuses without elevation.
pub fn run_mode(flag: Result<bool, String>) -> i32 {
    let enabled = match flag {
        Ok(on) => on,
        Err(e) => {
            log::error(&e);
            return 1;
        }
    };
    if !is_elevated() {
        log::error("startup task: --startup needs administrator rights");
        return 1;
    }
    match set_now(enabled) {
        Ok(()) => {
            let what = if enabled { "created" } else { "removed" };
            log::info(&format!("startup task {what}"));
            0
        }
        Err(e) => {
            log::error(&format!("startup task not changed: {e}"));
            1
        }
    }
}

/// The text shown when a change failed.
fn change_failed(e: &str) -> StartupState {
    StartupState::Error(format!("Couldn't change the startup task: {e}"))
}

/// Turns the task on or off for this exe and reports the state after. An elevated Lite Bar
/// changes it directly; otherwise through the elevated helper (`parent` owns the UAC
/// prompt). Blocks: run it on a worker thread.
pub fn change(enabled: bool, elevated: bool, parent: usize) -> StartupResult {
    if elevated {
        change_elevated(enabled)
    } else {
        change_via_uac(enabled, parent)
    }
}

/// An elevated Lite Bar's change: makes it, then reports the state. Blocks.
fn change_elevated(enabled: bool) -> StartupResult {
    match set_now(enabled) {
        Ok(()) => StartupResult::State(query_now()),
        Err(e) => {
            log::warn(&format!("startup task not changed: {e}"));
            StartupResult::State(change_failed(&e))
        }
    }
}

/// A non-elevated Lite Bar's change: starts `lite-bar.exe --startup on|off` as
/// administrator (UAC, owned by the window `parent`), waits for it, then reports the
/// state. Blocks.
fn change_via_uac(enabled: bool, parent: usize) -> StartupResult {
    let exe = match own_exe() {
        Ok(exe) => exe,
        Err(e) => return StartupResult::State(change_failed(&e)),
    };
    let file = HSTRING::from(exe.as_str());
    let params = HSTRING::from(format!(
        "{STARTUP_FLAG} {}",
        if enabled { "on" } else { "off" }
    ));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        hwnd: HWND(parent as *mut c_void),
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // ShellExecuteEx may use COM; this worker thread has it to itself.
    // SAFETY: `info` and the strings it points to live through the call; COM is
    // uninitialized only when it was initialized here.
    let launched = unsafe {
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let r = ShellExecuteExW(&mut info);
        if com.is_ok() {
            CoUninitialize();
        }
        r
    };
    if let Err(e) = launched {
        if e.code() == ERROR_CANCELLED.to_hresult() {
            log::info("startup task: the administrator prompt was declined");
            return StartupResult::Declined;
        }
        log::warn(&format!(
            "startup task: couldn't start the elevated helper: {e}"
        ));
        return StartupResult::State(change_failed(&e.message()));
    }
    let mut code = 1u32;
    if !info.hProcess.is_invalid() {
        // SAFETY: our own child's handle, closed once.
        unsafe {
            if WaitForSingleObject(info.hProcess, HELPER_WAIT_MS) == WAIT_OBJECT_0 {
                let _ = GetExitCodeProcess(info.hProcess, &mut code);
            }
            let _ = CloseHandle(info.hProcess);
        }
    }
    let state = query_now();
    if code != 0 && !matches!(state, StartupState::Error(_)) {
        log::warn(&format!(
            "startup task: the elevated helper exited with {code}"
        ));
        return StartupResult::State(change_failed("see lite-bar.log"));
    }
    StartupResult::State(state)
}

/// Starts `job` on a new thread and posts its result (a failure if it panics) to `owner`.
/// Returns false when the thread could not be started.
pub fn spawn(owner: HWND, job: impl FnOnce() -> StartupResult + Send + 'static) -> bool {
    // HWND is not Send; the worker only posts to it.
    let raw = owner.0 as usize;
    std::thread::Builder::new()
        .name(WORKER_THREAD.into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                .unwrap_or_else(|_| StartupResult::State(StartupState::Error(PANICKED.into())));
            post_boxed(HWND(raw as *mut c_void), WM_STARTUP_RESULT, result);
        })
        .is_ok()
}

/// Takes back the result a `WM_STARTUP_RESULT` carries.
///
/// # Safety
/// `lparam` must come from a `WM_STARTUP_RESULT` posted by `spawn`, taken once.
pub unsafe fn take_result(lparam: LPARAM) -> StartupResult {
    // SAFETY: the caller's guarantee.
    unsafe { crate::updater::take_boxed(lparam) }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use lite_bar_core::startup_task::{
        create_args, delete_args, query_args, query_xml_args, task_xml,
    };

    use super::*;
    use crate::test_util::{MsgWindow, TempDir};

    const SCHTASKS: &str = r"C:\Windows\System32\schtasks.exe";
    const EXE: &str = r"C:\Games & Tools\Lite Bar\lite-bar.exe";
    const USER: &str = r"PC\paymon";

    /// Records every call and answers with scripted exit codes and stdout, in order.
    #[derive(Default)]
    struct FakeRunner {
        calls: RefCell<Vec<(String, Vec<String>)>>,
        /// Answers for the calls in order; a call past the end fails.
        answers: RefCell<Vec<Output>>,
        /// The XML file's bytes, read while the create command "runs".
        seen_file: RefCell<Option<Vec<u8>>>,
    }

    impl FakeRunner {
        fn new(answers: Vec<Output>) -> FakeRunner {
            FakeRunner {
                answers: RefCell::new(answers.into_iter().rev().collect()),
                ..FakeRunner::default()
            }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.borrow().iter().map(|c| c.1.clone()).collect()
        }

        fn answer(&self, exe: &str, args: &[String]) -> Output {
            self.calls
                .borrow_mut()
                .push((exe.to_string(), args.to_vec()));
            if args.first().map(String::as_str) == Some("/Create") {
                *self.seen_file.borrow_mut() = std::fs::read(&args[4]).ok();
            }
            self.answers
                .borrow_mut()
                .pop()
                .unwrap_or_else(|| Err("unexpected call".into()))
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, exe: &str, args: &[String]) -> Result<i32, String> {
            self.answer(exe, args).map(|a| a.0)
        }

        fn output(&self, exe: &str, args: &[String]) -> Output {
            self.answer(exe, args)
        }
    }

    fn exit(code: i32) -> Output {
        Ok((code, Vec::new()))
    }

    fn printed(xml: &str) -> Output {
        Ok((0, xml.as_bytes().to_vec()))
    }

    fn args(v: &[&str]) -> impl Iterator<Item = String> {
        v.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn parses_the_startup_flag() {
        assert_eq!(parse_flag(args(&[])), None);
        assert_eq!(parse_flag(args(&["--updated"])), None);
        assert_eq!(parse_flag(args(&["--startup", "on"])), Some(Ok(true)));
        assert_eq!(parse_flag(args(&["--startup", "off"])), Some(Ok(false)));
        assert_eq!(parse_flag(args(&["--startup", "OFF"])), Some(Ok(false)));
        assert!(matches!(parse_flag(args(&["--startup"])), Some(Err(_))));
        assert!(matches!(
            parse_flag(args(&["--startup", "maybe"])),
            Some(Err(_))
        ));
    }

    #[test]
    fn is_enabled_follows_the_query_exit_code() {
        let r = FakeRunner::new(vec![exit(0)]);
        assert!(is_enabled(&r, SCHTASKS));
        assert_eq!(r.calls(), vec![query_args()]);
        assert_eq!(r.calls.borrow()[0].0, SCHTASKS);
        let r = FakeRunner::new(vec![exit(1)]);
        assert!(!is_enabled(&r, SCHTASKS));
        let r = FakeRunner::new(vec![Err("not found".into())]);
        assert!(!is_enabled(&r, SCHTASKS));
    }

    #[test]
    fn query_checks_where_the_task_points() {
        let r = FakeRunner::new(vec![exit(1)]);
        assert_eq!(query(&r, SCHTASKS, EXE), StartupState::Off);
        assert_eq!(r.calls(), vec![query_args()]);

        let r = FakeRunner::new(vec![exit(0), printed(&task_xml(EXE, USER))]);
        assert_eq!(query(&r, SCHTASKS, EXE), StartupState::On);
        assert_eq!(r.calls(), vec![query_args(), query_xml_args()]);

        let other = task_xml(r"D:\old copy\lite-bar.exe", USER);
        let r = FakeRunner::new(vec![exit(0), printed(&other)]);
        assert_eq!(query(&r, SCHTASKS, EXE), StartupState::OnOtherCopy);

        // The XML query failing doesn't hide that the task exists.
        let r = FakeRunner::new(vec![exit(0), exit(1)]);
        assert_eq!(query(&r, SCHTASKS, EXE), StartupState::On);

        let r = FakeRunner::new(vec![Err("cannot start".into())]);
        assert!(matches!(query(&r, SCHTASKS, EXE), StartupState::Error(_)));
    }

    #[test]
    fn turning_on_creates_from_a_utf16_xml_file_then_removes_it() {
        let dir = TempDir::new("startup-set-on");
        let r = FakeRunner::new(vec![exit(0)]);
        set(&r, SCHTASKS, true, EXE, USER, dir.path()).unwrap();
        let calls = r.calls();
        assert_eq!(calls.len(), 1);
        let xml_path = calls[0][4].clone();
        assert_eq!(calls[0], create_args(&xml_path));
        assert!(Path::new(&xml_path).starts_with(dir.path()));
        let bytes = r.seen_file.borrow().clone().expect("the file existed");
        assert_eq!(bytes, utf16_bom(&task_xml(EXE, USER)));
        assert!(!Path::new(&xml_path).exists(), "the temp file is removed");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_failed_create_is_an_error_and_still_cleans_up() {
        let dir = TempDir::new("startup-set-fail");
        let r = FakeRunner::new(vec![exit(1)]);
        assert!(set(&r, SCHTASKS, true, EXE, USER, dir.path()).is_err());
        let r = FakeRunner::new(vec![Err("cannot start".into())]);
        assert!(set(&r, SCHTASKS, true, EXE, USER, dir.path()).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn write_locked_never_reuses_or_removes_an_existing_file() {
        let dir = TempDir::new("startup-existing");
        let path = dir.path().join("planted.xml");
        std::fs::write(&path, b"someone else's").unwrap();
        assert!(write_locked(&path, b"ours").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"someone else's");
        // A new file is held: it can be read, but not written or removed.
        let fresh = dir.path().join("fresh.xml");
        let held = write_locked(&fresh, b"ours").unwrap();
        assert_eq!(std::fs::read(&fresh).unwrap(), b"ours");
        assert!(std::fs::OpenOptions::new()
            .write(true)
            .open(&fresh)
            .is_err());
        assert!(std::fs::remove_file(&fresh).is_err());
        drop(held);
        std::fs::remove_file(&fresh).unwrap();
    }

    #[test]
    fn turning_off_deletes_and_a_missing_task_is_fine() {
        let dir = TempDir::new("startup-set-off");
        let r = FakeRunner::new(vec![exit(0)]);
        set(&r, SCHTASKS, false, EXE, USER, dir.path()).unwrap();
        assert_eq!(r.calls(), vec![delete_args()]);
        // Delete fails, but the task is gone: done.
        let r = FakeRunner::new(vec![exit(1), exit(1)]);
        set(&r, SCHTASKS, false, EXE, USER, dir.path()).unwrap();
        assert_eq!(r.calls(), vec![delete_args(), query_args()]);
        // Delete fails and the task is still there.
        let r = FakeRunner::new(vec![exit(1), exit(0)]);
        assert!(set(&r, SCHTASKS, false, EXE, USER, dir.path()).is_err());
    }

    #[test]
    fn utf16_bom_bytes() {
        assert_eq!(utf16_bom("A<"), vec![0xFF, 0xFE, b'A', 0, b'<', 0]);
    }

    #[test]
    fn decodes_schtasks_output() {
        assert_eq!(decode_output(b"<Task/>").as_deref(), Some("<Task/>"));
        assert_eq!(decode_output(&utf16_bom("<Ü/>")).as_deref(), Some("<Ü/>"));
        let no_bom: Vec<u8> = "<T/>".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(decode_output(&no_bom).as_deref(), Some("<T/>"));
        // Console code page bytes that are not UTF-8.
        assert_eq!(decode_output(&[b'<', 0xDC, b'>']), None);
    }

    #[test]
    fn user_from_the_environment() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(
            user_from_env(s("PC"), s("paymon")).as_deref(),
            Some(r"PC\paymon")
        );
        assert_eq!(user_from_env(None, s("paymon")).as_deref(), Some("paymon"));
        assert_eq!(user_from_env(s("PC"), None), None);
        assert_eq!(user_from_env(s(""), s("")), None);
    }

    #[test]
    fn current_user_names_someone() {
        let u = current_user();
        assert!(!u.is_empty());
        assert!(u.contains('\\'), "{u}");
    }

    #[test]
    fn schtasks_lives_in_system32() {
        let p = schtasks_path();
        assert!(p.to_lowercase().ends_with(r"\system32\schtasks.exe"), "{p}");
    }

    #[test]
    fn system_runner_reports_exit_codes_without_creating_anything() {
        // cmd's exit code travels back; nothing is changed.
        let cmd =
            std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(SystemRunner.run(&cmd, &a(&["/C", "exit 3"])), Ok(3));
        let (code, out) = SystemRunner.output(&cmd, &a(&["/C", "echo hi"])).unwrap();
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out).trim(), "hi");
        assert!(SystemRunner.run(r"C:\no\such\program.exe", &[]).is_err());
    }

    #[test]
    fn spawned_job_posts_its_result_and_a_panic_posts_an_error() {
        use std::time::{Duration, Instant};
        use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, MSG, PM_REMOVE};
        let w = MsgWindow::new();
        let wait = || {
            let end = Instant::now() + Duration::from_secs(5);
            let mut msg = MSG::default();
            while Instant::now() < end {
                let got = unsafe {
                    PeekMessageW(
                        &mut msg,
                        Some(w.0),
                        WM_STARTUP_RESULT,
                        WM_STARTUP_RESULT,
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
        assert!(spawn(w.0, || StartupResult::Declined));
        assert_eq!(wait(), Some(StartupResult::Declined));
        assert!(spawn(w.0, || panic!("boom")));
        assert!(matches!(
            wait(),
            Some(StartupResult::State(StartupState::Error(_)))
        ));
    }
}

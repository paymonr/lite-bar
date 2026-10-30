//! One-click update install. A worker thread downloads the release's checksum and exe,
//! verifies the SHA-256 and writes `<exe>.new` next to the running exe; the result goes
//! back to the owner window as `WM_INSTALL_RESULT` (same boxed-`LPARAM` scheme as update
//! checks, see `updater`). The UI thread then swaps the files (`swap`, which rolls back on
//! failure) and relaunches with `--updated`; that instance removes `<exe>.old`.
//!
//! The running exe is never touched until the new bytes are verified and on disk, and
//! Windows lets a running exe be renamed (not deleted) within its volume.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lite_bar_core::update::{current_version, parse_sha256_file, sha256, ReleaseInfo, Version};
use windows::core::HSTRING;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWNORMAL, WM_APP};

use crate::stats_files::write_atomic;
use crate::updater::{post_boxed, take_boxed, user_agent};
use crate::{http, log};

/// Posted to the owner window when a download finished; `LPARAM` is a `Box<InstallResult>`.
pub const WM_INSTALL_RESULT: u32 = WM_APP + 4;

/// Name of the download threads; the panic hook leaves the ETW trace alone for them.
pub const WORKER_THREAD: &str = "update-install";
/// Name of the thread that removes `<exe>.old` after an update.
pub const CLEANUP_THREAD: &str = "update-cleanup";

/// The `.sha256` file is one line; anything over this is refused.
pub const MAX_SHA_BYTES: usize = 64 * 1024;
/// Lite Bar is a few MB; anything over this is refused.
pub const MAX_EXE_BYTES: usize = 50 * 1024 * 1024;
/// The whole download (checksum and exe) must finish within this.
pub const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(5 * 60);

/// The command-line flag the relaunched instance gets.
pub const UPDATED_FLAG: &str = "--updated";

/// Shown when the exe's folder can't be written (e.g. Program Files without elevation).
pub const UNWRITABLE: &str = "Can't update in this folder — download it from GitHub";

/// What a download that panicked reports.
const PANICKED: &str = "update download failed unexpectedly";

/// Hosts release downloads may come from (GitHub and its asset CDN).
const TRUSTED_HOSTS: [&str; 4] = [
    "github.com",
    "api.github.com",
    "objects.githubusercontent.com",
    "github-releases.githubusercontent.com",
];

/// `<exe>.new` (staged, verified) or why not.
pub type InstallResult = Result<PathBuf, InstallError>;

#[derive(Debug, PartialEq, Eq)]
pub enum InstallError {
    /// A download URL is not an https link to GitHub.
    Untrusted(String),
    Download(String),
    /// The `.sha256` file holds no SHA-256.
    BadChecksumFile,
    ChecksumMismatch,
    /// The exe's folder can't be written.
    Unwritable,
    Io(String),
    /// The swap failed and the original exe is back in place.
    RolledBack(String),
    /// The swap failed and so did putting the original back: it is at `old`.
    RollbackFailed {
        old: PathBuf,
        why: String,
    },
    /// The download thread panicked.
    Unexpected(String),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallError::Untrusted(_) => f.write_str("the download is not from GitHub"),
            InstallError::Download(e) => write!(f, "download failed ({e})"),
            InstallError::BadChecksumFile => f.write_str("the checksum file is not valid"),
            InstallError::ChecksumMismatch => {
                f.write_str("the download was corrupted (checksum mismatch)")
            }
            InstallError::Unwritable => f.write_str(UNWRITABLE),
            InstallError::Io(e) => write!(f, "couldn't write the update ({e})"),
            InstallError::RolledBack(e) => write!(f, "couldn't replace Lite Bar ({e})"),
            InstallError::Unexpected(e) => f.write_str(e),
            InstallError::RollbackFailed { old, .. } => write!(
                f,
                "Lite Bar was left at {} — rename it back to {}",
                old.display(),
                old.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
        }
    }
}

impl InstallError {
    /// Whether trying again may work (Settings then offers Try again).
    pub fn retryable(&self) -> bool {
        !matches!(
            self,
            InstallError::Unwritable
                | InstallError::Untrusted(_)
                | InstallError::RollbackFailed { .. }
        )
    }

    /// An error writing the new exe into the exe's folder: access denied means the folder
    /// is not ours to write. (Only for that write: a refused rename or delete is about one
    /// file, which may just be in use.)
    fn staging(e: io::Error) -> InstallError {
        if e.kind() == io::ErrorKind::PermissionDenied {
            InstallError::Unwritable
        } else {
            InstallError::Io(e.to_string())
        }
    }
}

/// Whether `url` is an https link on one of GitHub's download hosts (default port, no
/// credentials). Redirects from there are WinHTTP's (https to http is refused).
pub fn is_trusted_download_url(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    scheme.eq_ignore_ascii_case("https")
        && path.starts_with('/')
        && url.chars().all(|c| c.is_ascii_graphic())
        && TRUSTED_HOSTS.iter().any(|h| host.eq_ignore_ascii_case(h))
}

/// `<exe>.new`: the verified download, before the swap.
pub fn new_path(exe: &Path) -> PathBuf {
    with_suffix(exe, ".new")
}

/// `<exe>.old`: the previous exe, after the swap.
pub fn old_path(exe: &Path) -> PathBuf {
    with_suffix(exe, ".old")
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Downloads `release`'s checksum and exe (only when `trusted` accepts both URLs),
/// verifies the exe and stages it as `<exe>.new`. Blocks: call on a worker thread.
pub fn fetch_and_stage(
    release: &ReleaseInfo,
    exe: &Path,
    trusted: fn(&str) -> bool,
) -> Result<PathBuf, InstallError> {
    for url in [&release.sha_url, &release.exe_url] {
        if !trusted(url) {
            return Err(InstallError::Untrusted(url.clone()));
        }
    }
    let deadline = Some(Instant::now() + DOWNLOAD_DEADLINE);
    let ua = user_agent();
    let get = |url: &str, max| {
        http::get_with_deadline(url, max, &ua, &[], deadline)
            .map_err(|e| InstallError::Download(e.to_string()))
    };
    let sha_text = get(&release.sha_url, MAX_SHA_BYTES)?;
    let expected = std::str::from_utf8(&sha_text)
        .ok()
        .and_then(parse_sha256_file)
        .ok_or(InstallError::BadChecksumFile)?;
    let bytes = get(&release.exe_url, MAX_EXE_BYTES)?;
    stage_update(exe, &bytes, &expected)
}

/// The production download: GitHub URLs only.
pub fn download_release(release: &ReleaseInfo, exe: &Path) -> InstallResult {
    fetch_and_stage(release, exe, is_trusted_download_url)
}

/// Checks `bytes` against `expected` and writes them to `<exe>.new` (fsynced), replacing a
/// stale one. Nothing is written when the checksum does not match.
pub fn stage_update(
    exe: &Path,
    bytes: &[u8],
    expected: &[u8; 32],
) -> Result<PathBuf, InstallError> {
    if sha256(bytes) != *expected {
        return Err(InstallError::ChecksumMismatch);
    }
    let new = new_path(exe);
    match std::fs::remove_file(&new) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            return Err(InstallError::Io(format!(
                "can't remove the old {}: {e}",
                new.display()
            )))
        }
        _ => {}
    }
    write_atomic(&new, bytes).map_err(InstallError::staging)?;
    Ok(new)
}

/// Puts `new` in place of `exe`, keeping the previous exe as `<exe>.old`. On failure the
/// original is put back; if even that fails the error says where it is.
pub fn swap(exe: &Path, new: &Path) -> Result<(), InstallError> {
    swap_with(exe, new, move_file)
}

fn swap_with(
    exe: &Path,
    new: &Path,
    mut mv: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(), InstallError> {
    let old = old_path(exe);
    mv(exe, &old).map_err(|e| InstallError::RolledBack(e.to_string()))?;
    let Err(e) = mv(new, exe) else {
        return Ok(());
    };
    match mv(&old, exe) {
        Ok(()) => Err(InstallError::RolledBack(e.to_string())),
        Err(back) => Err(InstallError::RollbackFailed {
            old,
            why: format!("{e}; moving it back: {back}"),
        }),
    }
}

/// `MoveFileExW`, replacing the target and flushing before it returns. Works on a running
/// exe within its volume.
pub(crate) fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    let (from, to) = (
        HSTRING::from(from.as_os_str()),
        HSTRING::from(to.as_os_str()),
    );
    // SAFETY: valid NUL-terminated paths for the duration of the call.
    unsafe {
        MoveFileExW(
            &from,
            &to,
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))
}

/// Tries this many times to remove `<exe>.old` (the old process may still be exiting).
pub const CLEANUP_TRIES: u32 = 10;
pub const CLEANUP_DELAY: Duration = Duration::from_secs(1);

/// Removes `<exe>.old`: `Ok(true)` when removed, `Ok(false)` when there was none.
pub fn cleanup_old(exe: &Path) -> Result<bool, String> {
    remove_with_retries(
        &old_path(exe),
        CLEANUP_TRIES,
        CLEANUP_DELAY,
        std::thread::sleep,
    )
}

/// Removes `old`, trying `tries` times `delay` apart (`sleep` waits): `Ok(true)` when
/// removed, `Ok(false)` when there was none.
pub(crate) fn remove_with_retries(
    old: &Path,
    tries: u32,
    delay: Duration,
    mut sleep: impl FnMut(Duration),
) -> Result<bool, String> {
    let mut last = String::new();
    for i in 0..tries {
        if i > 0 {
            sleep(delay);
        }
        match std::fs::remove_file(old) {
            Ok(()) => return Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => last = e.to_string(),
        }
    }
    Err(format!("could not remove {}: {last}", old.display()))
}

/// What staging can leave when a run ends before the swap (a crash, a kill, a failed swap):
/// `<exe>.new` and `write_atomic`'s `<exe>.new.tmp`.
pub fn leftover_paths(exe: &Path) -> [PathBuf; 2] {
    [new_path(exe), with_suffix(exe, ".new.tmp")]
}

/// Removes those leftovers (one try each: nothing else has them open once this instance
/// holds the mutex). Returns the ones removed; a failure is logged.
pub fn cleanup_leftovers(exe: &Path) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    for path in leftover_paths(exe) {
        match std::fs::remove_file(&path) {
            Ok(()) => removed.push(path),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => log::warn(&format!("could not remove {}: {e}", path.display())),
        }
    }
    removed
}

/// At every start: removes what staging left and a `<exe>.old` an update left (the old
/// process may still be exiting) on its own thread, with retries, so the tray is never
/// held up.
pub fn spawn_cleanup(exe: PathBuf) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name(CLEANUP_THREAD.into())
        .spawn(move || {
            if let Ok(removed) = std::panic::catch_unwind(|| cleanup_leftovers(&exe)) {
                for path in removed {
                    log::info(&format!("removed {}", path.display()));
                }
            }
            let result = std::panic::catch_unwind(|| cleanup_old(&exe))
                .unwrap_or_else(|_| Err("cleanup failed unexpectedly".into()));
            match result {
                Ok(true) => log::info(&format!("removed {}", old_path(&exe).display())),
                Ok(false) => {} // nothing to clean, the usual case
                Err(e) => log::warn(&e),
            }
        })
        .inspect_err(|e| log::error(&format!("could not start the cleanup thread: {e}")))
        .ok()
}

/// Whether the command line (without the program name) asks for the post-update start,
/// and the version it was updated from: `None` without the flag, `Some(None)` for a bare
/// `--updated` (from a release that did not pass the version).
pub fn updated_launch(mut args: impl Iterator<Item = String>) -> Option<Option<Version>> {
    args.find(|a| a == UPDATED_FLAG)?;
    Some(args.next().and_then(|v| v.parse().ok()))
}

/// The arguments `relaunch` gives the updated exe: the flag and the version updated from.
pub fn relaunch_args(from: Version) -> String {
    format!("{UPDATED_FLAG} {from}")
}

/// The updated instance's first log line.
pub fn updated_log_line(from: Option<Version>, to: Version) -> String {
    match from {
        Some(from) => format!("updated from v{from} to v{to}"),
        None => format!("updated to v{to}"),
    }
}

/// `ShellExecuteW`'s result: its return value is above 32 on success, else an error code.
pub fn shell_execute_result(code: usize) -> Result<(), String> {
    if code <= 32 {
        return Err(format!("ShellExecuteW failed (code {code})"));
    }
    Ok(())
}

/// Starts `exe --updated <this version>`. An elevated caller's child is elevated too.
pub fn relaunch(exe: &Path) -> Result<(), String> {
    let file = HSTRING::from(exe.as_os_str());
    let dir = HSTRING::from(exe.parent().unwrap_or(Path::new("")).as_os_str());
    let args = HSTRING::from(relaunch_args(current_version()));
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

/// Starts a download on a new thread: `download` runs there and its result, or a failure
/// if it panics, is posted to `owner`. Returns false when the thread could not be started.
pub fn spawn_download(
    owner: HWND,
    download: impl FnOnce() -> InstallResult + Send + 'static,
) -> bool {
    let raw = owner.0 as usize;
    std::thread::Builder::new()
        .name(WORKER_THREAD.into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(download))
                .unwrap_or_else(|_| Err(InstallError::Unexpected(PANICKED.into())));
            post_boxed(
                HWND(raw as *mut std::ffi::c_void),
                WM_INSTALL_RESULT,
                result,
            );
        })
        .is_ok()
}

/// Takes back the result a `WM_INSTALL_RESULT` carries.
///
/// # Safety
/// `lparam` must come from a `WM_INSTALL_RESULT` posted by `spawn_download`, taken once.
pub unsafe fn take_result(lparam: LPARAM) -> InstallResult {
    // SAFETY: the caller's guarantee.
    unsafe { take_boxed(lparam) }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    use lite_bar_core::update::Version;

    use super::*;
    use crate::test_util::{http_ok as ok, TempDir};

    const OLD_EXE: &[u8] = b"MZ old lite bar";
    const NEW_EXE: &[u8] = b"MZ new lite bar v0.5.0";

    fn hex(d: &[u8; 32]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A temp dir holding `lite-bar.exe` (the "current" exe).
    fn exe_dir(name: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new(name);
        let exe = dir.path().join("lite-bar.exe");
        std::fs::write(&exe, OLD_EXE).unwrap();
        (dir, exe)
    }

    /// Serves `/sha` and `/exe` with the given raw responses; the release points there.
    fn serve_release(sha: Vec<u8>, exe: Vec<u8>) -> ReleaseInfo {
        let base = crate::test_util::serve_paths(vec![("/sha", sha), ("/exe", exe)]);
        ReleaseInfo {
            version: "0.5.0".parse::<Version>().unwrap(),
            page_url: "https://github.com/paymonr/lite-bar/releases/tag/v0.5.0".into(),
            exe_url: format!("{base}/exe"),
            sha_url: format!("{base}/sha"),
        }
    }

    fn any_url(_: &str) -> bool {
        true
    }

    fn sha_file(bytes: &[u8]) -> Vec<u8> {
        ok(format!("{}  lite-bar.exe\n", hex(&sha256(bytes))).as_bytes())
    }

    #[test]
    fn trusted_download_urls_are_https_on_github_hosts() {
        for good in [
            "https://github.com/paymonr/lite-bar/releases/download/v0.5.0/lite-bar.exe",
            "HTTPS://GitHub.com/paymonr/lite-bar/releases/download/v0.5.0/lite-bar.exe",
            "https://api.github.com/repos/paymonr/lite-bar/releases/assets/1",
            "https://objects.githubusercontent.com/github-production-release-asset/x",
            "https://github-releases.githubusercontent.com/1/2?x=y",
        ] {
            assert!(is_trusted_download_url(good), "{good}");
        }
    }

    #[test]
    fn untrusted_host_refused() {
        for bad in [
            "http://github.com/paymonr/lite-bar/releases/download/v0.5.0/lite-bar.exe",
            "https://evil.com/lite-bar.exe",
            "https://github.com.evil.com/lite-bar.exe",
            "https://evilgithub.com/lite-bar.exe",
            "https://github.com@evil.com/lite-bar.exe",
            "https://user:pw@github.com/lite-bar.exe",
            "https://github.com:8443/lite-bar.exe",
            "https://github.com",
            "https://github.com/a b",
            "https://127.0.0.1/lite-bar.exe",
            "file:///C:/lite-bar.exe",
            "C:\\lite-bar.exe",
            "",
        ] {
            assert!(!is_trusted_download_url(bad), "{bad}");
        }
        // The production download refuses before connecting: nothing listens on port 1.
        let (_dir, exe) = exe_dir("install-untrusted");
        let release = ReleaseInfo {
            version: "0.5.0".parse().unwrap(),
            page_url: "https://github.com/x".into(),
            exe_url: "https://github.com/paymonr/lite-bar/releases/download/v0.5.0/lite-bar.exe"
                .into(),
            sha_url: "http://127.0.0.1:1/sha".into(),
        };
        assert_eq!(
            fetch_and_stage(&release, &exe, is_trusted_download_url),
            Err(InstallError::Untrusted("http://127.0.0.1:1/sha".into()))
        );
        assert_eq!(std::fs::read(&exe).unwrap(), OLD_EXE);
        assert!(!new_path(&exe).exists());
    }

    #[test]
    fn verified_download_is_staged_as_new() {
        let (_dir, exe) = exe_dir("install-staged");
        let release = serve_release(sha_file(NEW_EXE), ok(NEW_EXE));
        let staged = fetch_and_stage(&release, &exe, any_url).unwrap();
        assert_eq!(staged, new_path(&exe));
        assert_eq!(staged.file_name().unwrap(), "lite-bar.exe.new");
        assert_eq!(std::fs::read(&staged).unwrap(), NEW_EXE);
        assert_eq!(
            std::fs::read(&exe).unwrap(),
            OLD_EXE,
            "current exe untouched"
        );
    }

    #[test]
    fn checksum_mismatch_aborts() {
        let (_dir, exe) = exe_dir("install-mismatch");
        let release = serve_release(sha_file(b"something else"), ok(NEW_EXE));
        assert_eq!(
            fetch_and_stage(&release, &exe, any_url),
            Err(InstallError::ChecksumMismatch)
        );
        assert!(!new_path(&exe).exists());
        assert_eq!(std::fs::read(&exe).unwrap(), OLD_EXE);
        // Directly, too.
        assert_eq!(
            stage_update(&exe, NEW_EXE, &sha256(b"other")),
            Err(InstallError::ChecksumMismatch)
        );
        assert!(!new_path(&exe).exists());
    }

    #[test]
    fn bad_checksum_file_aborts() {
        let (_dir, exe) = exe_dir("install-badsha");
        let release = serve_release(ok(b"not a checksum"), ok(NEW_EXE));
        assert_eq!(
            fetch_and_stage(&release, &exe, any_url),
            Err(InstallError::BadChecksumFile)
        );
        assert!(!new_path(&exe).exists());
    }

    #[test]
    fn interrupted_download_leaves_current_exe() {
        let (_dir, exe) = exe_dir("install-interrupted");
        // Promises 1000 bytes, closes after 10.
        let cut = b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\nMZ01234567"
            .to_vec();
        let release = serve_release(sha_file(NEW_EXE), cut);
        assert!(matches!(
            fetch_and_stage(&release, &exe, any_url),
            Err(InstallError::Download(_))
        ));
        assert!(!new_path(&exe).exists());
        assert_eq!(std::fs::read(&exe).unwrap(), OLD_EXE);
    }

    #[test]
    fn stale_new_is_replaced() {
        let (_dir, exe) = exe_dir("install-stale-new");
        std::fs::write(new_path(&exe), b"stale half download").unwrap();
        let staged = stage_update(&exe, NEW_EXE, &sha256(NEW_EXE)).unwrap();
        assert_eq!(std::fs::read(staged).unwrap(), NEW_EXE);
    }

    #[test]
    fn access_denied_while_staging_is_unwritable_folder() {
        let e = InstallError::staging(io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(e, InstallError::Unwritable);
        assert_eq!(
            e.to_string(),
            "Can't update in this folder — download it from GitHub"
        );
        assert!(matches!(
            InstallError::staging(io::Error::other("disk full")),
            InstallError::Io(_)
        ));
    }

    #[test]
    fn swap_puts_new_exe_in_place_and_keeps_old() {
        let (_dir, exe) = exe_dir("install-swap");
        std::fs::write(old_path(&exe), b"stale old").unwrap();
        let new = stage_update(&exe, NEW_EXE, &sha256(NEW_EXE)).unwrap();
        swap(&exe, &new).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), NEW_EXE);
        assert_eq!(std::fs::read(old_path(&exe)).unwrap(), OLD_EXE);
        assert!(!new.exists());
    }

    #[test]
    fn rename_failure_rolls_back() {
        let (_dir, exe) = exe_dir("install-rollback");
        let new = stage_update(&exe, NEW_EXE, &sha256(NEW_EXE)).unwrap();
        // Nobody else may open it: moving `.new` over the exe fails.
        let hold = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&new)
            .unwrap();
        assert!(matches!(swap(&exe, &new), Err(InstallError::RolledBack(_))));
        drop(hold);
        assert_eq!(std::fs::read(&exe).unwrap(), OLD_EXE, "original back");
        assert!(!old_path(&exe).exists());
    }

    #[test]
    fn failed_rollback_still_leaves_a_working_exe() {
        let (_dir, exe) = exe_dir("install-rollback-fails");
        let new = stage_update(&exe, NEW_EXE, &sha256(NEW_EXE)).unwrap();
        // The first move (exe to .old) works; the swap and the move back both fail.
        let mut calls = 0;
        let got = swap_with(&exe, &new, |from, to| {
            calls += 1;
            if calls == 1 {
                move_file(from, to)
            } else {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        });
        assert_eq!(calls, 3);
        let Err(InstallError::RollbackFailed { old, .. }) = &got else {
            panic!("{got:?}");
        };
        assert_eq!(*old, old_path(&exe));
        // The user still has the original exe under one of the two names.
        let working = [exe.clone(), old_path(&exe)]
            .iter()
            .any(|p| std::fs::read(p).ok().as_deref() == Some(OLD_EXE));
        assert!(working);
        assert!(got.unwrap_err().to_string().contains("lite-bar.exe.old"));
    }

    /// Run by `running_exe_can_be_swapped` in a copy of this test binary: keeps the process
    /// (and so its exe) alive for a while.
    #[test]
    #[ignore]
    fn sleep_helper() {
        if std::env::var_os("LITE_BAR_SLEEP_HELPER").is_some() {
            std::thread::sleep(Duration::from_secs(10));
        }
    }

    #[test]
    fn running_exe_can_be_swapped() {
        let dir = TempDir::new("install-running");
        let exe = dir.path().join("lite-bar.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        let mut child = std::process::Command::new(&exe)
            .args(["installer::tests::sleep_helper", "--exact", "--ignored"])
            .env("LITE_BAR_SLEEP_HELPER", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(child.try_wait().unwrap().is_none(), "still running");
        let new = stage_update(&exe, NEW_EXE, &sha256(NEW_EXE)).unwrap();
        let swapped = swap(&exe, &new);
        // Our own child, started above.
        let _ = child.kill();
        let _ = child.wait();
        swapped.unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), NEW_EXE);
        assert!(old_path(&exe).exists());
    }

    #[test]
    fn cleanup_removes_old() {
        let (_dir, exe) = exe_dir("install-cleanup");
        std::fs::write(old_path(&exe), OLD_EXE).unwrap();
        assert_eq!(cleanup_old(&exe), Ok(true));
        assert!(!old_path(&exe).exists());
        assert!(exe.exists());
        // Nothing to clean is fine.
        assert_eq!(cleanup_old(&exe), Ok(false));
    }

    #[test]
    fn cleanup_thread_removes_old() {
        let (_dir, exe) = exe_dir("install-cleanup-thread");
        std::fs::write(old_path(&exe), OLD_EXE).unwrap();
        let worker = spawn_cleanup(exe.clone()).expect("thread");
        assert_eq!(worker.thread().name(), Some(CLEANUP_THREAD));
        worker.join().unwrap();
        assert!(!old_path(&exe).exists());
        assert!(exe.exists());
    }

    #[test]
    fn cleanup_retries_while_the_old_exe_is_in_use() {
        let (_dir, exe) = exe_dir("install-cleanup-retry");
        std::fs::write(old_path(&exe), OLD_EXE).unwrap();
        let mut hold = Some(
            OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(old_path(&exe))
                .unwrap(),
        );
        let mut sleeps = Vec::new();
        // The old process lets go during the first wait.
        let got = remove_with_retries(&old_path(&exe), 5, CLEANUP_DELAY, |d| {
            sleeps.push(d);
            hold = None;
        });
        assert_eq!(got, Ok(true));
        assert_eq!(sleeps, vec![CLEANUP_DELAY]);
        assert!(!old_path(&exe).exists());

        // Still in use after every try: gives up after 5.
        std::fs::write(old_path(&exe), OLD_EXE).unwrap();
        let _hold = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(old_path(&exe))
            .unwrap();
        let mut tries = 0;
        assert!(remove_with_retries(&old_path(&exe), 5, CLEANUP_DELAY, |_| tries += 1).is_err());
        assert_eq!(tries, 4, "waits between the 5 tries");
    }

    #[test]
    fn cleanup_budget_is_ten_tries_a_second_apart() {
        assert_eq!(CLEANUP_TRIES, 10);
        assert_eq!(CLEANUP_DELAY, Duration::from_secs(1));
    }

    #[test]
    fn transient_failures_can_be_retried() {
        for e in [
            InstallError::Download("download took too long".into()),
            InstallError::BadChecksumFile,
            InstallError::ChecksumMismatch,
            InstallError::Io("disk full".into()),
            InstallError::RolledBack("in use".into()),
            InstallError::Unexpected("boom".into()),
        ] {
            assert!(e.retryable(), "{e:?}");
        }
        for e in [
            InstallError::Unwritable,
            InstallError::Untrusted("https://evil.com/x".into()),
            InstallError::RollbackFailed {
                old: "lite-bar.exe.old".into(),
                why: "x".into(),
            },
        ] {
            assert!(!e.retryable(), "{e:?}");
        }
    }

    #[test]
    fn access_denied_during_the_swap_is_not_an_unwritable_folder() {
        let (_dir, exe) = exe_dir("install-swap-denied");
        let new = stage_update(&exe, NEW_EXE, &sha256(NEW_EXE)).unwrap();
        // Moving the running exe aside is refused (e.g. antivirus holding it).
        let got = swap_with(&exe, &new, |_, _| {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        });
        assert!(matches!(got, Err(InstallError::RolledBack(_))), "{got:?}");
        assert_eq!(std::fs::read(&exe).unwrap(), OLD_EXE);
    }

    #[test]
    fn a_stale_new_that_cannot_be_removed_is_not_an_unwritable_folder() {
        let (_dir, exe) = exe_dir("install-stale-stuck");
        std::fs::create_dir(new_path(&exe)).unwrap();
        let got = stage_update(&exe, NEW_EXE, &sha256(NEW_EXE));
        assert!(matches!(got, Err(InstallError::Io(_))), "{got:?}");
        assert_eq!(std::fs::read(&exe).unwrap(), OLD_EXE);
    }

    #[test]
    fn shell_execute_failures_report_its_return_code() {
        assert_eq!(shell_execute_result(42), Ok(()));
        let e = shell_execute_result(2).unwrap_err();
        assert!(e.contains("code 2"), "{e}");
        assert!(shell_execute_result(32).is_err());
    }

    #[test]
    fn updated_flag_is_recognised() {
        let args = |a: &[&str]| {
            a.iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .into_iter()
        };
        assert!(updated_launch(args(&["--updated"])).is_some());
        assert!(updated_launch(args(&[])).is_none());
        assert!(updated_launch(args(&["--update"])).is_none());
    }

    #[test]
    fn updated_launch_carries_the_old_version() {
        let args = |a: &[&str]| {
            a.iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .into_iter()
        };
        let v = |s: &str| s.parse::<Version>().unwrap();
        assert_eq!(updated_launch(args(&[])), None);
        assert_eq!(updated_launch(args(&["--update", "0.4.0"])), None);
        // A bare flag, from a release before the version was passed.
        assert_eq!(updated_launch(args(&["--updated"])), Some(None));
        assert_eq!(
            updated_launch(args(&["--updated", "0.4.0"])),
            Some(Some(v("0.4.0")))
        );
        // Something that is not a version is ignored.
        assert_eq!(updated_launch(args(&["--updated", "--x"])), Some(None));
    }

    #[test]
    fn relaunch_passes_the_running_version() {
        let from: Version = "0.4.0".parse().unwrap();
        assert_eq!(relaunch_args(from), "--updated 0.4.0");
    }

    #[test]
    fn updated_log_line_names_both_versions() {
        let v = |s: &str| s.parse::<Version>().unwrap();
        assert_eq!(
            updated_log_line(Some(v("0.4.0")), v("0.4.1")),
            "updated from v0.4.0 to v0.4.1"
        );
        assert_eq!(updated_log_line(None, v("0.4.1")), "updated to v0.4.1");
    }

    #[test]
    fn startup_cleanup_removes_stale_new() {
        let (_dir, exe) = exe_dir("install-cleanup-new");
        let tmp = PathBuf::from(format!("{}.tmp", new_path(&exe).display()));
        std::fs::write(new_path(&exe), NEW_EXE).unwrap();
        std::fs::write(&tmp, NEW_EXE).unwrap();
        std::fs::write(old_path(&exe), OLD_EXE).unwrap();
        spawn_cleanup(exe.clone()).expect("thread").join().unwrap();
        assert!(!new_path(&exe).exists());
        assert!(!tmp.exists());
        assert!(!old_path(&exe).exists());
        assert!(exe.exists());
    }

    #[test]
    fn download_limits() {
        assert_eq!(MAX_SHA_BYTES, 64 * 1024);
        assert_eq!(MAX_EXE_BYTES, 50 * 1024 * 1024);
        assert_eq!(DOWNLOAD_DEADLINE, Duration::from_secs(300));
    }

    #[test]
    fn a_panicking_download_still_posts_a_failure() {
        use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, MSG, PM_REMOVE};
        let win = crate::test_util::MsgWindow::new();
        assert!(spawn_download(win.0, || panic!("boom")));
        let end = Instant::now() + Duration::from_secs(10);
        let mut msg = MSG::default();
        while Instant::now() < end {
            // SAFETY: plain FFI on this thread's window.
            let got = unsafe {
                PeekMessageW(
                    &mut msg,
                    Some(win.0),
                    WM_INSTALL_RESULT,
                    WM_INSTALL_RESULT,
                    PM_REMOVE,
                )
            };
            if got.as_bool() {
                // SAFETY: posted by `spawn_download`, taken once.
                let r = unsafe { take_result(msg.lParam) };
                assert_eq!(
                    r,
                    Err(InstallError::Unexpected(
                        "update download failed unexpectedly".into()
                    ))
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("no result");
    }
}

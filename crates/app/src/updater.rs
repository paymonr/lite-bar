//! Update checks: one short-lived worker thread per check asks GitHub for the latest
//! release and posts the result back to the owner window as `WM_UPDATE_RESULT`.
//!
//! The result travels as a `Box<CheckResult>` turned into the message's `LPARAM`; the owner
//! window takes it back with `take_result`. If the post fails (the window is gone) the worker
//! frees it. On Quit a worker may still be running: the process exits without waiting for
//! it. If it finishes after the owner window was destroyed, its post fails and it frees the
//! result itself; if it posted just before the window went away, the message is never
//! retrieved and that one box leaks as the process exits. Either way nothing runs on a
//! dead `App`.

use std::ffi::c_void;

use lite_bar_core::update::{
    current_version, newer_release, parse_latest_release, ReleaseInfo, UpdateState, Version,
};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::http;

/// Posted to the owner window when a check finished; `LPARAM` is a `Box<CheckResult>`.
pub const WM_UPDATE_RESULT: u32 = WM_APP + 3;

/// GitHub's latest-release endpoint for Lite Bar.
pub const LATEST_URL: &str = "https://api.github.com/repos/paymonr/lite-bar/releases/latest";

/// The release JSON is a few KB; anything over this is refused.
const MAX_RELEASE_JSON: usize = 1024 * 1024;

/// The latest release, or why the check failed.
pub type CheckResult = Result<ReleaseInfo, String>;

pub fn user_agent() -> String {
    format!("Lite-Bar/{}", current_version())
}

/// Asks `url` for the latest release. Blocks: call on a worker thread.
pub fn fetch_latest(url: &str) -> CheckResult {
    let body = http::get(
        url,
        MAX_RELEASE_JSON,
        &user_agent(),
        &[("Accept", "application/vnd.github+json")],
    )
    .map_err(|e| format!("{e:?}"))?;
    let text = String::from_utf8(body).map_err(|_| "release JSON is not UTF-8".to_string())?;
    parse_latest_release(&text)
}

/// Starts a check on a new thread; its result is posted to `owner`. Returns false when the
/// thread could not be started (nothing will be posted).
pub fn spawn_check(owner: HWND, url: String) -> bool {
    // HWND is not Send; the worker only posts to it.
    let raw = owner.0 as usize;
    std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let result = fetch_latest(&url);
            post_result(HWND(raw as *mut c_void), result);
        })
        .is_ok()
}

/// Posts `result` to `owner`. Returns false (and frees it) when the post fails.
pub fn post_result(owner: HWND, result: CheckResult) -> bool {
    let ptr = Box::into_raw(Box::new(result));
    // SAFETY: plain FFI; on success the owner window takes the box back in `take_result`.
    let posted = unsafe {
        PostMessageW(
            Some(owner),
            WM_UPDATE_RESULT,
            WPARAM(0),
            LPARAM(ptr as isize),
        )
    };
    if posted.is_err() {
        // SAFETY: not posted, so nobody else owns it.
        drop(unsafe { Box::from_raw(ptr) });
        return false;
    }
    true
}

/// Takes back the result a `WM_UPDATE_RESULT` carries.
///
/// # Safety
/// `lparam` must come from a `WM_UPDATE_RESULT` posted by `post_result`, and is taken once.
pub unsafe fn take_result(lparam: LPARAM) -> CheckResult {
    // SAFETY: the caller guarantees it is the box `post_result` leaked, taken once.
    *unsafe { Box::from_raw(lparam.0 as *mut CheckResult) }
}

/// The state after a check: newer release → `Available`, none → `UpToDate`, error →
/// `CheckFailed`. An `Available` release stays until a later successful check says
/// otherwise, and a download in progress is never interrupted.
pub fn next_state(current: &UpdateState, result: &CheckResult, version: &Version) -> UpdateState {
    match (current, result) {
        (UpdateState::Downloading(_), _) => current.clone(),
        (UpdateState::Available(_), Err(_)) => current.clone(),
        (_, Err(_)) => UpdateState::CheckFailed,
        (_, Ok(r)) if newer_release(version, r) => UpdateState::Available(r.clone()),
        (_, Ok(_)) => UpdateState::UpToDate,
    }
}

/// Whether `url` is safe to hand to the shell as a web page (an https link).
pub fn is_web_url(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    scheme.eq_ignore_ascii_case("https")
        && !rest.is_empty()
        && !rest.starts_with('/')
        && url.chars().all(|c| c.is_ascii_graphic())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, PeekMessageW, HWND_MESSAGE, MSG, PM_REMOVE,
        WINDOW_EX_STYLE, WS_OVERLAPPED,
    };

    use super::*;

    const RELEASE_JSON: &str = r#"{
      "html_url": "https://github.com/paymonr/lite-bar/releases/tag/v0.5.0",
      "tag_name": "v0.5.0",
      "draft": false,
      "prerelease": false,
      "assets": [
        {"name": "lite-bar.exe",
         "browser_download_url": "https://github.com/paymonr/lite-bar/releases/download/v0.5.0/lite-bar.exe"},
        {"name": "lite-bar.exe.sha256",
         "browser_download_url": "https://github.com/paymonr/lite-bar/releases/download/v0.5.0/lite-bar.exe.sha256"}
      ]
    }"#;

    fn release(version: &str) -> ReleaseInfo {
        ReleaseInfo {
            version: version.parse().unwrap(),
            page_url: format!("https://github.com/paymonr/lite-bar/releases/tag/v{version}"),
            exe_url: "e".into(),
            sha_url: "s".into(),
        }
    }

    /// A local stub that answers one request: the release JSON when GitHub's Accept header
    /// was sent, otherwise 400.
    fn serve_release() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let Ok((mut s, _)) = listener.accept() else {
                return;
            };
            let mut req = Vec::new();
            let mut buf = [0u8; 1024];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }
            let head = String::from_utf8_lossy(&req).to_ascii_lowercase();
            let ua = format!("user-agent: lite-bar/{}\r\n", current_version());
            let resp = if head.contains("accept: application/vnd.github+json\r\n")
                && head.contains(&ua)
            {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{RELEASE_JSON}",
                    RELEASE_JSON.len()
                )
            } else {
                "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_string()
            };
            let _ = s.write_all(resp.as_bytes());
        });
        format!("http://127.0.0.1:{port}/repos/paymonr/lite-bar/releases/latest")
    }

    struct MsgWindow(HWND);

    impl Drop for MsgWindow {
        fn drop(&mut self) {
            // SAFETY: created by this test thread, destroyed once.
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    fn msg_window() -> MsgWindow {
        // SAFETY: plain FFI; the predefined STATIC class needs no registration.
        let hwnd = unsafe {
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
        .unwrap();
        MsgWindow(hwnd)
    }

    /// Waits up to `timeout` for a `WM_UPDATE_RESULT` to `hwnd` and takes its result.
    fn wait_result(hwnd: HWND, timeout: Duration) -> Option<CheckResult> {
        let end = Instant::now() + timeout;
        let mut msg = MSG::default();
        while Instant::now() < end {
            // SAFETY: plain FFI on this thread's window.
            let got = unsafe {
                PeekMessageW(
                    &mut msg,
                    Some(hwnd),
                    WM_UPDATE_RESULT,
                    WM_UPDATE_RESULT,
                    PM_REMOVE,
                )
            };
            if got.as_bool() {
                // SAFETY: posted by `post_result`, taken once.
                return Some(unsafe { take_result(msg.lParam) });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    #[test]
    fn user_agent_names_the_version() {
        assert_eq!(user_agent(), format!("Lite-Bar/{}", current_version()));
    }

    #[test]
    fn latest_url_is_lite_bars_github_releases() {
        assert_eq!(
            LATEST_URL,
            "https://api.github.com/repos/paymonr/lite-bar/releases/latest"
        );
    }

    #[test]
    fn fetch_parses_the_latest_release() {
        let url = serve_release();
        let r = fetch_latest(&url).unwrap();
        assert_eq!(r.version.to_string(), "0.5.0");
        assert!(r.exe_url.ends_with("/v0.5.0/lite-bar.exe"));
    }

    #[test]
    fn fetch_failure_is_an_error() {
        assert!(fetch_latest("http://127.0.0.1:1/").is_err());
    }

    #[test]
    fn result_round_trips_through_the_message() {
        let win = msg_window();
        assert!(post_result(win.0, Ok(release("0.5.0"))));
        assert_eq!(
            wait_result(win.0, Duration::from_secs(1)),
            Some(Ok(release("0.5.0")))
        );
        assert!(post_result(win.0, Err("offline".into())));
        assert_eq!(
            wait_result(win.0, Duration::from_secs(1)),
            Some(Err("offline".into()))
        );
    }

    #[test]
    fn posting_to_a_destroyed_window_fails() {
        let win = msg_window();
        let hwnd = win.0;
        drop(win);
        assert!(!post_result(hwnd, Ok(release("0.5.0"))));
    }

    #[test]
    fn worker_thread_posts_its_result() {
        let win = msg_window();
        let url = serve_release();
        assert!(spawn_check(win.0, url));
        let got = wait_result(win.0, Duration::from_secs(10)).expect("a result");
        assert_eq!(got.unwrap().version.to_string(), "0.5.0");
    }

    #[test]
    fn next_state_follows_the_result() {
        let cur: Version = "0.4.0".parse().unwrap();
        let idle = UpdateState::Idle;
        assert_eq!(
            next_state(&idle, &Ok(release("0.5.0")), &cur),
            UpdateState::Available(release("0.5.0"))
        );
        assert_eq!(
            next_state(&idle, &Ok(release("0.4.0")), &cur),
            UpdateState::UpToDate
        );
        assert_eq!(
            next_state(&UpdateState::Checking, &Err("x".into()), &cur),
            UpdateState::CheckFailed
        );
        assert_eq!(
            next_state(&UpdateState::UpToDate, &Err("x".into()), &cur),
            UpdateState::CheckFailed
        );
    }

    #[test]
    fn available_stays_until_a_successful_check_says_otherwise() {
        let cur: Version = "0.4.0".parse().unwrap();
        let avail = UpdateState::Available(release("0.5.0"));
        assert_eq!(next_state(&avail, &Err("offline".into()), &cur), avail);
        assert_eq!(
            next_state(&avail, &Ok(release("0.5.1")), &cur),
            UpdateState::Available(release("0.5.1"))
        );
        assert_eq!(
            next_state(&avail, &Ok(release("0.4.0")), &cur),
            UpdateState::UpToDate
        );
    }

    #[test]
    fn a_download_in_progress_is_kept() {
        let cur: Version = "0.4.0".parse().unwrap();
        let down = UpdateState::Downloading(release("0.5.0"));
        assert_eq!(next_state(&down, &Ok(release("0.6.0")), &cur), down);
        assert_eq!(next_state(&down, &Err("x".into()), &cur), down);
    }

    #[test]
    fn only_https_pages_are_opened() {
        assert!(is_web_url(
            "https://github.com/paymonr/lite-bar/releases/tag/v0.5.0"
        ));
        assert!(is_web_url("HTTPS://github.com/x"));
        for bad in [
            "http://github.com/x",
            "file:///C:/Windows/system32/calc.exe",
            "C:\\Windows\\system32\\calc.exe",
            "calc.exe",
            "https://",
            "https://github.com/x y",
            "",
        ] {
            assert!(!is_web_url(bad), "{bad}");
        }
    }
}

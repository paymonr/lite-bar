//! Shared helpers for the app crate's tests.

use std::path::{Path, PathBuf};

/// A fresh, empty directory under `%TEMP%` that is removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("lite-bar-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A message-only window for tests that wait for posted messages; destroyed on drop.
pub struct MsgWindow(pub windows::Win32::Foundation::HWND);

impl MsgWindow {
    pub fn new() -> MsgWindow {
        use windows::core::w;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, HWND_MESSAGE, WINDOW_EX_STYLE, WS_OVERLAPPED,
        };
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
        .expect("message window");
        MsgWindow(hwnd)
    }
}

impl Drop for MsgWindow {
    fn drop(&mut self) {
        // SAFETY: created by this test thread, destroyed once.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0);
        }
    }
}

/// A local HTTP stub: answers every request for one of `routes`' paths with its raw
/// response bytes (404 otherwise), until the test process ends. Returns the base url.
pub fn serve_paths(routes: Vec<(&'static str, Vec<u8>)>) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("port").port();
    std::thread::spawn(move || {
        for s in listener.incoming() {
            let Ok(mut s) = s else { return };
            let mut req = Vec::new();
            let mut buf = [0u8; 1024];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }
            let head = String::from_utf8_lossy(&req).into_owned();
            let path = head.split_whitespace().nth(1).unwrap_or("");
            let resp = routes
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, r)| r.as_slice());
            let _ = s.write_all(resp.unwrap_or(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            ));
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// A `200 OK` response carrying `body`.
pub fn http_ok(body: &[u8]) -> Vec<u8> {
    let mut v = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    v.extend_from_slice(body);
    v
}

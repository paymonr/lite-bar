//! Minimal synchronous WinHTTP GET client. Call from a worker thread only.

#[derive(Debug, PartialEq, Eq)]
pub enum HttpError {
    Network(String),
    Status(u16),
    TooLarge,
}

use std::ffi::c_void;
use std::fmt;

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpError::Network(msg) => f.write_str(msg),
            HttpError::Status(code) => write!(f, "HTTP status {code}"),
            HttpError::TooLarge => f.write_str("response too large"),
        }
    }
}

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::GetLastError;
use windows::Win32::Networking::WinHttp::{
    WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpCrackUrl, WinHttpOpen,
    WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData,
    WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts,
    URL_COMPONENTS, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ADDREQ_FLAG_ADD,
    WINHTTP_ADDREQ_FLAG_REPLACE, WINHTTP_FLAG_SECURE, WINHTTP_INTERNET_SCHEME_HTTP,
    WINHTTP_INTERNET_SCHEME_HTTPS, WINHTTP_OPEN_REQUEST_FLAGS, WINHTTP_OPTION_REDIRECT_POLICY,
    WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP, WINHTTP_QUERY_CONTENT_LENGTH,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};

const TIMEOUT_MS: i32 = 15_000;

/// Owned WinHTTP handle, closed on drop.
struct Handle(*mut c_void);

impl Handle {
    fn new(raw: *mut c_void, what: &str) -> Result<Self, HttpError> {
        if raw.is_null() {
            Err(last_error(what))
        } else {
            Ok(Handle(raw))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: self.0 is a valid handle returned by WinHTTP and closed once.
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

fn last_error(what: &str) -> HttpError {
    // SAFETY: plain thread-local error read.
    let code = unsafe { GetLastError().0 };
    HttpError::Network(format!("{what} failed (Win32 error {code})"))
}

fn api_error(what: &str, e: windows::core::Error) -> HttpError {
    // HRESULT_FROM_WIN32 keeps the Win32 code in the low 16 bits.
    HttpError::Network(format!(
        "{what} failed (Win32 error {})",
        e.code().0 & 0xFFFF
    ))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Synchronous GET of an `http://` or `https://` URL, sending `headers` (name, value) too.
/// Returns the body of a 200 response of at most `max_bytes`.
pub fn get(
    url: &str,
    max_bytes: usize,
    user_agent: &str,
    headers: &[(&str, &str)],
) -> Result<Vec<u8>, HttpError> {
    let scheme = url.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    if !matches!(scheme.as_deref(), Some("http" | "https")) {
        return Err(HttpError::Network("unsupported URL scheme".into()));
    }
    let url_w: Vec<u16> = url.encode_utf16().collect();
    let mut host = [0u16; 256];
    let mut path = [0u16; 2048];
    let mut comps = URL_COMPONENTS {
        dwStructSize: std::mem::size_of::<URL_COMPONENTS>() as u32,
        lpszHostName: PWSTR(host.as_mut_ptr()),
        dwHostNameLength: host.len() as u32,
        lpszUrlPath: PWSTR(path.as_mut_ptr()),
        dwUrlPathLength: path.len() as u32,
        ..Default::default()
    };
    // SAFETY: comps points at buffers that outlive the call.
    unsafe { WinHttpCrackUrl(&url_w, 0, &mut comps) }
        .map_err(|e| api_error("WinHttpCrackUrl", e))?;
    if comps.dwHostNameLength as usize >= host.len() || comps.dwUrlPathLength as usize >= path.len()
    {
        return Err(HttpError::Network("URL too long".into()));
    }
    // Buffers are zero-initialised, so they are NUL-terminated after the cracked length.
    let secure = match comps.nScheme {
        WINHTTP_INTERNET_SCHEME_HTTPS => true,
        WINHTTP_INTERNET_SCHEME_HTTP => false,
        _ => return Err(HttpError::Network("unsupported URL scheme".into())),
    };
    let ua = wide(user_agent);

    // SAFETY: all pointers passed below reference live, NUL-terminated buffers.
    unsafe {
        let session = Handle::new(
            WinHttpOpen(
                PCWSTR(ua.as_ptr()),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ),
            "WinHttpOpen",
        )?;
        WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS)
            .map_err(|e| api_error("WinHttpSetTimeouts", e))?;
        let conn = Handle::new(
            WinHttpConnect(session.0, PCWSTR(host.as_ptr()), comps.nPort, 0),
            "WinHttpConnect",
        )?;
        let flags = if secure {
            WINHTTP_FLAG_SECURE
        } else {
            WINHTTP_OPEN_REQUEST_FLAGS(0)
        };
        let req = Handle::new(
            WinHttpOpenRequest(
                conn.0,
                w!("GET"),
                PCWSTR(path.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                flags,
            ),
            "WinHttpOpenRequest",
        )?;
        let policy = WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP.to_ne_bytes();
        WinHttpSetOption(Some(req.0), WINHTTP_OPTION_REDIRECT_POLICY, Some(&policy))
            .map_err(|e| api_error("WinHttpSetOption", e))?;

        for (name, value) in headers {
            let line: Vec<u16> = format!("{name}: {value}").encode_utf16().collect();
            WinHttpAddRequestHeaders(
                req.0,
                &line,
                WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE,
            )
            .map_err(|e| api_error("WinHttpAddRequestHeaders", e))?;
        }
        WinHttpSendRequest(req.0, None, None, 0, 0, 0)
            .map_err(|e| api_error("WinHttpSendRequest", e))?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut())
            .map_err(|e| api_error("WinHttpReceiveResponse", e))?;

        let mut status: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut len,
            std::ptr::null_mut(),
        )
        .map_err(|e| api_error("WinHttpQueryHeaders", e))?;
        if status != 200 {
            return Err(HttpError::Status(status as u16));
        }

        // Absent for chunked responses; then truncation can't be detected here.
        let mut declared: u32 = 0;
        let mut dlen = std::mem::size_of::<u32>() as u32;
        let declared = WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut declared as *mut u32 as *mut c_void),
            &mut dlen,
            std::ptr::null_mut(),
        )
        .ok()
        .map(|()| declared as usize);

        let mut body = Vec::new();
        loop {
            let mut avail: u32 = 0;
            WinHttpQueryDataAvailable(req.0, &mut avail)
                .map_err(|e| api_error("WinHttpQueryDataAvailable", e))?;
            if avail == 0 {
                break;
            }
            let start = body.len();
            if start + avail as usize > max_bytes {
                return Err(HttpError::TooLarge);
            }
            body.resize(start + avail as usize, 0);
            let mut read: u32 = 0;
            WinHttpReadData(
                req.0,
                body[start..].as_mut_ptr() as *mut c_void,
                avail,
                &mut read,
            )
            .map_err(|e| api_error("WinHttpReadData", e))?;
            body.truncate(start + read as usize);
            if read == 0 {
                break;
            }
        }
        if declared.is_some_and(|d| body.len() < d) {
            return Err(HttpError::Network(
                "connection closed before the full body arrived".into(),
            ));
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    /// Serves `connections` connections in order, each with the raw bytes `handler`
    /// returns for the request head (request line and headers). Returns the base url.
    fn serve(handler: fn(&str) -> Vec<u8>, connections: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for _ in 0..connections {
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
                let text = String::from_utf8_lossy(&req);
                let _ = s.write_all(&handler(&text));
                // dropping `s` closes the connection
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    /// The path of a request head.
    fn path(req: &str) -> &str {
        req.split_whitespace().nth(1).unwrap_or("/")
    }

    fn ok(body: &[u8]) -> Vec<u8> {
        let mut v = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn http_200_returns_body() {
        let base = serve(|_| ok(b"hello"), 1);
        assert_eq!(
            get(&format!("{base}/a"), 1024, "lite-bar-test", &[]),
            Ok(b"hello".to_vec())
        );
    }

    #[test]
    fn http_302_redirect_is_followed() {
        let base = serve(
            |req| {
                if path(req) == "/start" {
                    b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_vec()
                } else {
                    ok(b"landed")
                }
            },
            2,
        );
        assert_eq!(
            get(&format!("{base}/start"), 1024, "t", &[]),
            Ok(b"landed".to_vec())
        );
    }

    #[test]
    fn query_string_is_sent() {
        let base = serve(|req| ok(path(req).as_bytes()), 1);
        assert_eq!(
            get(&format!("{base}/a/b?x=1&y=2"), 1024, "t", &[]),
            Ok(b"/a/b?x=1&y=2".to_vec())
        );
    }

    #[test]
    fn http_403_is_check_failed() {
        let base = serve(
            |_| {
                b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
            },
            1,
        );
        assert_eq!(
            get(&format!("{base}/x"), 1024, "t", &[]),
            Err(HttpError::Status(403))
        );
    }

    #[test]
    fn oversize_body_is_too_large() {
        let base = serve(|_| ok(&[7u8; 5000]), 1);
        assert_eq!(
            get(&format!("{base}/big"), 1000, "t", &[]),
            Err(HttpError::TooLarge)
        );
    }

    #[test]
    fn connection_closed_mid_body_is_network_error() {
        let base = serve(
            |_| {
                b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n0123456789"
                    .to_vec()
            },
            1,
        );
        assert!(matches!(
            get(&format!("{base}/cut"), 1024, "t", &[]),
            Err(HttpError::Network(_))
        ));
    }

    #[test]
    fn unreachable_host_is_network_error() {
        // Port 1 on loopback: nothing listens, connection is refused.
        assert!(matches!(
            get("http://127.0.0.1:1/", 16, "t", &[]),
            Err(HttpError::Network(_))
        ));
    }

    #[test]
    fn extra_headers_and_user_agent_are_sent() {
        // Echoes the request head back as the body.
        let base = serve(|req| ok(req.as_bytes()), 1);
        let body = get(
            &format!("{base}/h"),
            4096,
            "Lite-Bar/9.9.9",
            &[("Accept", "application/vnd.github+json")],
        )
        .unwrap();
        let head = String::from_utf8(body).unwrap().to_ascii_lowercase();
        assert!(
            head.contains("\r\nuser-agent: lite-bar/9.9.9\r\n"),
            "{head}"
        );
        assert!(
            head.contains("\r\naccept: application/vnd.github+json\r\n"),
            "{head}"
        );
    }

    #[test]
    fn errors_read_as_text() {
        assert_eq!(HttpError::Status(403).to_string(), "HTTP status 403");
        assert_eq!(HttpError::TooLarge.to_string(), "response too large");
        assert_eq!(
            HttpError::Network("WinHttpConnect failed".into()).to_string(),
            "WinHttpConnect failed"
        );
    }

    #[test]
    fn non_http_schemes_are_refused() {
        for url in [
            "ftp://127.0.0.1/x",
            "file:///C:/Windows/win.ini",
            "C:\\Windows\\win.ini",
            "javascript:alert(1)",
            "",
        ] {
            assert!(
                matches!(get(url, 16, "t", &[]), Err(HttpError::Network(ref m)) if m.contains("scheme")),
                "{url}"
            );
        }
    }
}

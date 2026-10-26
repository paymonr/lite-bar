//! Image path, creation time and liveness of another process, via
//! `PROCESS_QUERY_LIMITED_INFORMATION` only. Handles never outlive a call.

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// `GetExitCodeProcess` result for a process that hasn't exited (`STILL_ACTIVE`).
const STILL_ACTIVE_CODE: u32 = 259;

/// What identifies one running instance of a program: its image path and creation time
/// (FILETIME ticks), which together survive pid reuse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessInfo {
    pub exe_path: String,
    pub created: u64,
}

/// Owns a process handle opened for limited queries and closes it on drop.
struct QueryHandle(HANDLE);

impl QueryHandle {
    fn open(pid: u32) -> Option<QueryHandle> {
        if pid == 0 {
            return None;
        }
        // SAFETY: plain FFI call; the access mask is query-only (no memory access, no waits).
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
            .ok()
            .map(QueryHandle)
    }

    fn creation_time(&self) -> Option<u64> {
        let (mut created, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        // SAFETY: the handle is open and all four out-pointers refer to live FILETIMEs.
        unsafe { GetProcessTimes(self.0, &mut created, &mut exit, &mut kernel, &mut user) }.ok()?;
        Some(u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime))
    }

    fn image_path(&self) -> Option<String> {
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        // SAFETY: `buf` has `len` writable u16s; on success `len` is the count written.
        unsafe {
            QueryFullProcessImageNameW(
                self.0,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
        }
        .ok()?;
        let path = String::from_utf16_lossy(buf.get(..len as usize)?);
        (!path.is_empty()).then_some(path)
    }

    fn is_active(&self) -> bool {
        let mut code = 0u32;
        // SAFETY: the handle is open and `code` is a live u32.
        unsafe { GetExitCodeProcess(self.0, &mut code) }.is_ok() && code == STILL_ACTIVE_CODE
    }
}

impl Drop for QueryHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `open` and is closed exactly once, here.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Image path and creation time of `pid`; `None` when it can't be opened (gone, or protected).
pub fn process_info(pid: u32) -> Option<ProcessInfo> {
    let handle = QueryHandle::open(pid)?;
    Some(ProcessInfo {
        exe_path: handle.image_path()?,
        created: handle.creation_time()?,
    })
}

/// True while `pid` is alive and was created at `created` (so a reused pid doesn't count).
pub fn is_running(pid: u32, created: u64) -> bool {
    QueryHandle::open(pid)
        .is_some_and(|handle| handle.is_active() && handle.creation_time() == Some(created))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Threading::GetCurrentProcessId;

    fn own_pid() -> u32 {
        // SAFETY: no arguments, cannot fail.
        unsafe { GetCurrentProcessId() }
    }

    #[test]
    fn own_process_info_has_exe_path_and_creation_time() {
        let info = process_info(own_pid()).expect("own process is queryable");
        let expected = std::env::current_exe().unwrap();
        assert_eq!(
            info.exe_path.to_lowercase(),
            expected.to_string_lossy().to_lowercase()
        );
        assert!(info.created > 0);
    }

    #[test]
    fn own_process_is_running() {
        let info = process_info(own_pid()).unwrap();
        assert!(is_running(own_pid(), info.created));
    }

    #[test]
    fn wrong_creation_time_is_not_running() {
        let info = process_info(own_pid()).unwrap();
        assert!(!is_running(own_pid(), info.created + 1));
    }

    #[test]
    fn pid_zero_has_no_info() {
        assert_eq!(process_info(0), None);
        assert!(!is_running(0, 1));
    }
}

//! Image path, creation time and liveness of another process, via
//! `PROCESS_QUERY_LIMITED_INFORMATION` only. Handles never outlive a call.

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, HANDLE};
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
        QueryHandle::try_open(pid).ok()
    }

    fn try_open(pid: u32) -> windows::core::Result<QueryHandle> {
        // SAFETY: plain FFI call; the access mask is query-only (no memory access, no waits).
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.map(QueryHandle)
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

/// Whether the run of `pid` created at `created` is still going.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    /// Gone, or the pid now belongs to a newer process.
    Exited,
    /// The process exists but can no longer be queried.
    Unqueryable,
}

/// `Running` while `pid` is alive and was created at `created` (so a reused pid counts as
/// `Exited`).
pub fn process_state(pid: u32, created: u64) -> ProcessState {
    if pid == 0 {
        return ProcessState::Exited;
    }
    let handle = match QueryHandle::try_open(pid) {
        Ok(handle) => handle,
        Err(e) => return state_after_open_error(&e),
    };
    match handle.creation_time() {
        None => ProcessState::Unqueryable,
        Some(t) if t != created => ProcessState::Exited,
        Some(_) if handle.is_active() => ProcessState::Running,
        Some(_) => ProcessState::Exited,
    }
}

/// What an `OpenProcess` failure says about the process.
fn state_after_open_error(error: &windows::core::Error) -> ProcessState {
    // OpenProcess reports a pid that doesn't exist as an invalid parameter.
    if error.code() == ERROR_INVALID_PARAMETER.to_hresult() {
        ProcessState::Exited
    } else {
        ProcessState::Unqueryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::ERROR_ACCESS_DENIED;
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
    fn own_process_state_is_running() {
        let info = process_info(own_pid()).unwrap();
        assert_eq!(
            process_state(own_pid(), info.created),
            ProcessState::Running
        );
    }

    #[test]
    fn reused_or_missing_pid_state_is_exited() {
        let info = process_info(own_pid()).unwrap();
        assert_eq!(
            process_state(own_pid(), info.created + 1),
            ProcessState::Exited
        );
        // Far above any pid Windows hands out.
        assert_eq!(process_state(0xFFFF_FFFC, 1), ProcessState::Exited);
        assert_eq!(process_state(0, 1), ProcessState::Exited);
    }

    #[test]
    fn access_denied_open_means_unqueryable_and_invalid_pid_means_exited() {
        let denied = windows::core::Error::from(ERROR_ACCESS_DENIED.to_hresult());
        assert_eq!(state_after_open_error(&denied), ProcessState::Unqueryable);
        let invalid = windows::core::Error::from(ERROR_INVALID_PARAMETER.to_hresult());
        assert_eq!(state_after_open_error(&invalid), ProcessState::Exited);
    }

    #[test]
    fn pid_zero_has_no_info() {
        assert_eq!(process_info(0), None);
    }
}

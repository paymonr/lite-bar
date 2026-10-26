//! Single-instance guard backed by a named mutex.

use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

/// Holds the named mutex for the lifetime of the process; released on drop.
pub struct InstanceGuard(HANDLE);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by CreateMutexW and is closed exactly once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Takes the named mutex. Returns `None` if another holder already has it
/// (or the mutex cannot be created at all).
pub fn acquire_single_instance(name: &str) -> Option<InstanceGuard> {
    let name = HSTRING::from(name);
    // SAFETY: plain FFI call with a valid, NUL-terminated name.
    let handle = unsafe { CreateMutexW(None, false, &name) }.ok()?;
    // SAFETY: read immediately after CreateMutexW, before any other Win32 call.
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_exists {
        // SAFETY: we own this duplicate handle.
        unsafe {
            let _ = CloseHandle(handle);
        }
        return None;
    }
    Some(InstanceGuard(handle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_until_first_dropped() {
        let name = format!("Local\\LiteBar.Test.{}", std::process::id());
        let g = acquire_single_instance(&name).expect("first");
        assert!(acquire_single_instance(&name).is_none());
        drop(g);
        assert!(acquire_single_instance(&name).is_some());
    }
}

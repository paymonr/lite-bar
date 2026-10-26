//! Single-instance guard backed by a named mutex.

use windows::core::HSTRING;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, HANDLE,
};
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

/// Takes the named mutex.
///
/// - `Ok(Some(guard))`: acquired; keep the guard alive for the process lifetime.
/// - `Ok(None)`: another instance already holds it.
/// - `Err(e)`: the mutex could not be created for any other reason; the caller
///   decides what to do (e.g. log it and run without a guard).
pub fn acquire_single_instance(name: &str) -> windows::core::Result<Option<InstanceGuard>> {
    let name = HSTRING::from(name);
    // SAFETY: plain FFI call with a valid, NUL-terminated name.
    let handle = match unsafe { CreateMutexW(None, false, &name) } {
        Ok(handle) => handle,
        // An elevated instance's mutex cannot be opened by a non-elevated
        // process, so access denied means another instance is running.
        Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => return Ok(None),
        Err(e) => return Err(e),
    };
    // SAFETY: read immediately after CreateMutexW, before any other Win32 call.
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_exists {
        // SAFETY: we own this duplicate handle.
        unsafe {
            let _ = CloseHandle(handle);
        }
        return Ok(None);
    }
    Ok(Some(InstanceGuard(handle)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_until_first_dropped() {
        let name = format!("Local\\LiteBar.Test.{}", std::process::id());
        let g = acquire_single_instance(&name).unwrap().expect("first");
        assert!(acquire_single_instance(&name).unwrap().is_none());
        drop(g);
        assert!(acquire_single_instance(&name).unwrap().is_some());
    }

    #[test]
    fn invalid_name_is_an_error_not_already_running() {
        // "Bogus\\x" names a nonexistent object-manager namespace, so
        // CreateMutexW fails with ERROR_PATH_NOT_FOUND.
        assert!(acquire_single_instance("Bogus\\x").is_err());
    }
}

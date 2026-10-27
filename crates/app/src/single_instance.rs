//! Single-instance guard backed by a named mutex.

use std::time::Duration;

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

/// How long a just-updated instance waits for the old one to let go of the mutex.
pub const UPDATED_WAIT: Duration = Duration::from_secs(10);
/// How often it tries.
pub const UPDATED_POLL: Duration = Duration::from_millis(200);

/// Tries `try_take` every `interval` until it gives a value or `timeout` has passed since
/// the first try (`now` reads the clock, `sleep` waits).
pub fn wait_for<T>(
    timeout: Duration,
    interval: Duration,
    mut now: impl FnMut() -> Duration,
    mut sleep: impl FnMut(Duration),
    mut try_take: impl FnMut() -> Option<T>,
) -> Option<T> {
    let start = now();
    loop {
        if let Some(v) = try_take() {
            return Some(v);
        }
        if now().saturating_sub(start) >= timeout {
            return None;
        }
        sleep(interval);
    }
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

    /// A fake clock that `sleep` advances.
    struct FakeClock(std::cell::Cell<Duration>);

    impl FakeClock {
        fn wait<T>(&self, try_take: impl FnMut() -> Option<T>) -> Option<T> {
            wait_for(
                UPDATED_WAIT,
                UPDATED_POLL,
                || self.0.get(),
                |d| self.0.set(self.0.get() + d),
                try_take,
            )
        }
    }

    #[test]
    fn updated_flag_waits_for_mutex() {
        assert_eq!(UPDATED_WAIT, Duration::from_secs(10));
        assert_eq!(UPDATED_POLL, Duration::from_millis(200));
        // The old instance lets go of the mutex on the 4th try.
        let clock = FakeClock(Default::default());
        let mut tries = 0;
        let got = clock.wait(|| {
            tries += 1;
            (tries == 4).then_some("guard")
        });
        assert_eq!(got, Some("guard"));
        assert_eq!(clock.0.get(), Duration::from_millis(600));

        // It never does: gives up after 10 s, trying every 200 ms.
        let clock = FakeClock(Default::default());
        let mut tries = 0;
        let got: Option<()> = clock.wait(|| {
            tries += 1;
            None
        });
        assert_eq!(got, None);
        assert_eq!(tries, 51);
        assert_eq!(clock.0.get(), Duration::from_secs(10));
    }

    #[test]
    fn updated_wait_takes_the_real_mutex_once_released() {
        let name = format!("Local\\LiteBar.TestWait.{}", std::process::id());
        let held = acquire_single_instance(&name).unwrap().expect("first");
        let clock = FakeClock(Default::default());
        let mut held = Some(held);
        let got = clock.wait(|| {
            // Released after the first try, as the old instance exits.
            let taken = acquire_single_instance(&name).unwrap();
            held = None;
            taken
        });
        assert!(got.is_some());
        assert_eq!(clock.0.get(), UPDATED_POLL);
    }

    #[test]
    fn invalid_name_is_an_error_not_already_running() {
        // "Bogus\\x" names a nonexistent object-manager namespace, so
        // CreateMutexW fails with ERROR_PATH_NOT_FOUND.
        assert!(acquire_single_instance("Bogus\\x").is_err());
    }
}

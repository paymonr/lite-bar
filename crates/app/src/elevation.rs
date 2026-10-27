//! Elevation (UAC) helpers.

use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Whether the current process token is elevated. `false` on any failure.
pub fn is_elevated() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: querying our own process token; the handle is closed below.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut TOKEN_ELEVATION as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// Starts a new elevated copy of this executable via the UAC `runas` verb.
/// The caller is expected to exit after this returns `Ok`.
pub fn relaunch_as_admin() -> windows::core::Result<()> {
    let exe = HSTRING::from(std::env::current_exe()?.as_os_str());
    // SAFETY: all string arguments are valid and NUL-terminated for the call.
    let result = unsafe { ShellExecuteW(None, w!("runas"), &exe, None, None, SW_SHOWNORMAL) };
    // ShellExecuteW returns a value greater than 32 on success.
    if result.0 as usize > 32 {
        Ok(())
    } else {
        Err(windows::core::Error::from_thread())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_elevated_does_not_panic() {
        let _ = is_elevated();
    }
}

//! Elevation (UAC) helpers.

use crate::startup::ElevatedError;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{
    ShellExecuteExW, ShellExecuteW, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
};
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

/// Starts this exe with `params` as administrator (one UAC prompt) in a visible window and
/// doesn't wait for it. A declined prompt is `Declined`.
pub fn run_as_admin(params: &str) -> Result<(), ElevatedError> {
    let exe = std::env::current_exe().map_err(|e| ElevatedError::Other(e.to_string()))?;
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(params);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        // The caller exits right after: the launch must finish before this returns.
        fMask: SEE_MASK_NOASYNC,
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // ShellExecuteEx may use COM; it is initialized here only when this thread had none.
    // SAFETY: `info` and the strings it points to live through the call.
    let launched = unsafe {
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let r = ShellExecuteExW(&mut info);
        if com.is_ok() {
            CoUninitialize();
        }
        r
    };
    match launched {
        Ok(()) => Ok(()),
        Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => Err(ElevatedError::Declined),
        Err(e) => Err(ElevatedError::Other(e.message())),
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

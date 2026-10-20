//! Starting programs the way the Run dialog does, optionally elevated.

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{SEE_MASK_FLAG_NO_UI, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::{SHOW_WINDOW_CMD, SW_HIDE, SW_SHOWNORMAL};
use windows::core::{PCWSTR, w};

use crate::Result;
use crate::handle::OwnedHandle;

/// Whether the current process runs with an elevated (administrator) token.
pub fn is_elevated() -> Result<bool> {
    let mut token = HANDLE::default();
    // SAFETY: GetCurrentProcess returns a pseudo-handle; `token` is writable.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)? };
    let token = OwnedHandle::new(token);
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: `elevation` is writable for its size.
    unsafe {
        GetTokenInformation(
            token.raw(),
            TokenElevation,
            Some((&raw mut elevation).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )?
    };
    Ok(elevation.TokenIsElevated != 0)
}

/// Opens `file` (a program, document, folder or URL) with `parameters`, as
/// the shell would. `elevated` asks for administrator rights (UAC prompt);
/// declining it fails with `ERROR_CANCELLED`.
pub fn launch(file: &str, parameters: &str, elevated: bool) -> Result<()> {
    launch_with(file, parameters, elevated, SW_SHOWNORMAL)
}

/// Like `launch`, without showing a window (for helper commands).
pub fn launch_hidden(file: &str, parameters: &str) -> Result<()> {
    launch_with(file, parameters, false, SW_HIDE)
}

fn launch_with(file: &str, parameters: &str, elevated: bool, show: SHOW_WINDOW_CMD) -> Result<()> {
    let file: Vec<u16> = file.encode_utf16().chain([0]).collect();
    let parameters: Vec<u16> = parameters.encode_utf16().chain([0]).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        // Errors are returned to us instead of shown in a message box.
        fMask: SEE_MASK_FLAG_NO_UI,
        lpVerb: if elevated {
            w!("runas")
        } else {
            PCWSTR::null()
        },
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: show.0,
        ..Default::default()
    };
    // SAFETY: the strings outlive the call; `info` is fully initialised.
    unsafe { ShellExecuteExW(&mut info)? };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elevation_can_be_queried() {
        // Either answer is fine; the query itself must work.
        is_elevated().unwrap();
    }

    #[test]
    fn launches_programs_and_reports_missing_ones() {
        // Hidden, and exits at once.
        launch_hidden("cmd.exe", "/c exit 0").unwrap();
        let err = launch(r"C:\definitely\missing-program.exe", "", false).unwrap_err();
        assert!(!err.to_string().is_empty());
    }
}

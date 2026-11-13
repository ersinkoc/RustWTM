//! Starting programs the way the Run dialog does, optionally elevated.

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{SEE_MASK_FLAG_NO_UI, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::{SHOW_WINDOW_CMD, SW_HIDE, SW_SHOWNORMAL};
use windows::core::{PCWSTR, w};

use crate::Result;
use crate::handle::OwnedHandle;

/// Trusted Windows system directory, independent of PATH and user variables.
pub fn system_directory() -> Result<std::path::PathBuf> {
    let mut buffer = [0u16; 260];
    // SAFETY: fixed writable buffer; the returned length is checked before use.
    let n = unsafe {
        windows::Win32::System::SystemInformation::GetSystemDirectoryW(Some(&mut buffer))
    } as usize;
    if n == 0 || n >= buffer.len() {
        return Err(windows::core::Error::from_thread().into());
    }
    Ok(std::path::PathBuf::from(String::from_utf16_lossy(
        &buffer[..n],
    )))
}

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

/// Opens `file` with `parameters` through the desktop's own Explorer, the
/// way the Run dialog does, so it runs with the signed-in user's standard
/// rights even when this process runs as administrator. Fails when no
/// Explorer shows the desktop. Explorer reports errors about the file itself
/// (such as a missing program) in its own message box.
pub fn launch_as_desktop_user(file: &str, parameters: &str) -> Result<()> {
    desktop_shell_execute(file, parameters, SW_SHOWNORMAL)
}

fn desktop_shell_execute(file: &str, parameters: &str, show: SHOW_WINDOW_CMD) -> Result<()> {
    use windows::Win32::System::Com::{
        CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize, IDispatch, IServiceProvider,
    };
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Shell::{
        CSIDL_DESKTOP, IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellView,
        IShellWindows, SID_STopLevelBrowser, SVGIO_BACKGROUND, SWC_DESKTOP, SWFO_NEEDDISPATCH,
        ShellWindows,
    };
    use windows::core::{BSTR, Interface};

    // SAFETY: balanced below; a thread already in another apartment mode
    // keeps it and must not be uninitialised by us.
    let initialised = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let result = (|| -> windows::core::Result<()> {
        // SAFETY: COM calls on interfaces obtained from the calls before
        // them; every out-value is owned by the returned smart pointers.
        unsafe {
            let windows: IShellWindows =
                CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
            let desktop = VARIANT::from(CSIDL_DESKTOP as i32);
            let root = VARIANT::default();
            let mut window = 0i32;
            let found: IDispatch = windows.FindWindowSW(
                &desktop,
                &root,
                SWC_DESKTOP,
                &mut window,
                SWFO_NEEDDISPATCH,
            )?;
            let provider: IServiceProvider = found.cast()?;
            let browser: IShellBrowser = provider.QueryService(&SID_STopLevelBrowser)?;
            let view: IShellView = browser.QueryActiveShellView()?;
            let background: IDispatch = view.GetItemObject(SVGIO_BACKGROUND)?;
            let folder: IShellFolderViewDual = background.cast()?;
            let shell: IShellDispatch2 = folder.Application()?.cast()?;
            shell.ShellExecute(
                &BSTR::from(file),
                &VARIANT::from(BSTR::from(parameters)),
                &VARIANT::from(BSTR::new()),
                &VARIANT::from(BSTR::new()),
                &VARIANT::from(show.0),
            )
        }
    })();
    if initialised {
        // SAFETY: pairs the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
    Ok(result?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_shell_starts_a_hidden_command() {
        // Hosted CI may run without a desktop; then there is nothing to ask.
        if crate::explorer::desktop_pid().is_none() {
            return;
        }
        let cmd = system_directory().unwrap().join("cmd.exe");
        desktop_shell_execute(&cmd.to_string_lossy(), "/d /c exit 0", SW_HIDE).unwrap();
    }

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

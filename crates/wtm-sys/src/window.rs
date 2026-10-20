//! The desktop's foreground window, and bringing our own window forward.

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    FLASHW_ALL, FLASHW_TIMERNOFG, FLASHWINFO, FindWindowW, FlashWindowEx, GetForegroundWindow,
    GetWindowThreadProcessId, HWND_NOTOPMOST, HWND_TOPMOST, IsIconic, IsWindowVisible, SW_HIDE,
    SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    SetForegroundWindow, SetWindowPos, ShowWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LR_SHARED, LoadImageW,
    SM_CXICON, SM_CXSMICON, SendMessageW, WM_SETICON,
};
use windows::core::PCWSTR;

/// The process owning the foreground window of the calling thread's desktop;
/// `None` when there is none (locked screen, service desktop).
pub fn foreground_pid() -> Option<u32> {
    // SAFETY: no arguments; returns a null HWND when there is no window.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return None;
    }
    let mut pid = 0u32;
    // SAFETY: `hwnd` came from the system; `pid` is a writable u32.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

/// A top-level window (shown or hidden) with exactly this title that belongs
/// to another process: its handle and process id.
pub fn find_other(title: &str) -> Option<(isize, u32)> {
    let wide: Vec<u16> = title.encode_utf16().chain([0]).collect();
    // SAFETY: `wide` is NUL-terminated; a missing window is an error.
    let h = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(wide.as_ptr())) }.ok()?;
    let mut pid = 0u32;
    // SAFETY: `h` came from the system; `pid` is a writable u32.
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    (pid != 0 && pid != std::process::id()).then_some((h.0 as isize, pid))
}

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut core::ffi::c_void)
}

/// Shows the window (restored if minimized or hidden), keeps it above other
/// windows and tries to give it the focus. Windows may refuse the focus to a
/// background process; the window then still appears on top and flashes in
/// the taskbar. Returns whether it got the focus.
pub fn bring_to_front(raw: isize) -> bool {
    let h = hwnd(raw);
    // SAFETY: these calls validate the handle and fail harmlessly on a
    // destroyed window.
    unsafe {
        let _ = ShowWindow(
            h,
            if IsIconic(h).as_bool() {
                SW_RESTORE
            } else {
                SW_SHOW
            },
        );
        let _ = SetWindowPos(
            h,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        let focused = SetForegroundWindow(h).as_bool();
        if !focused {
            let info = FLASHWINFO {
                cbSize: size_of::<FLASHWINFO>() as u32,
                hwnd: h,
                dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
                uCount: 0,
                dwTimeout: 0,
            };
            let _ = FlashWindowEx(&info);
        }
        focused
    }
}

/// Ends "always on top" without moving or activating the window.
pub fn release_topmost(raw: isize) {
    // SAFETY: see `bring_to_front`.
    unsafe {
        let _ = SetWindowPos(
            hwnd(raw),
            Some(HWND_NOTOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

pub fn hide(raw: isize) {
    // SAFETY: see `bring_to_front`.
    unsafe {
        let _ = ShowWindow(hwnd(raw), SW_HIDE);
    }
}

/// Gives the window the executable's own icon (resource 1) in the taskbar,
/// Alt+Tab and the title bar. Returns `false` when the executable has none.
pub fn use_exe_icon(raw: isize) -> bool {
    let h = hwnd(raw);
    // SAFETY: the module handle of our own executable; LoadImageW returns
    // shared icons owned by the system, so nothing needs freeing.
    unsafe {
        let Ok(module) = GetModuleHandleW(PCWSTR::null()) else {
            return false;
        };
        let mut ok = true;
        for (kind, metric) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)] {
            let size = GetSystemMetrics(metric);
            match LoadImageW(
                Some(HINSTANCE(module.0)),
                PCWSTR(1 as *const u16),
                IMAGE_ICON,
                size,
                size,
                LR_DEFAULTCOLOR | LR_SHARED,
            ) {
                Ok(icon) => {
                    SendMessageW(
                        h,
                        WM_SETICON,
                        Some(WPARAM(kind as usize)),
                        Some(LPARAM(icon.0 as isize)),
                    );
                }
                Err(_) => ok = false,
            }
        }
        ok
    }
}

pub fn is_visible(raw: isize) -> bool {
    // SAFETY: see `bring_to_front`.
    unsafe { IsWindowVisible(hwnd(raw)).as_bool() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW,
    };
    use windows::core::w;

    #[test]
    fn foreground_pid_is_a_real_process_or_none() {
        // CI runs without an interactive desktop, so both are possible.
        if let Some(pid) = super::foreground_pid() {
            assert!(pid != 0);
        }
    }

    #[test]
    fn shows_raises_and_hides_a_window() {
        // A tiny window far off-screen, so nothing appears on the desktop.
        // SAFETY: a predefined class; the window is destroyed below.
        let h = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("wtm-test"),
                WS_OVERLAPPEDWINDOW,
                -32000,
                -32000,
                10,
                10,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap();
        let raw = h.0 as isize;
        assert!(!is_visible(raw));
        // Our own windows are not "another instance".
        assert_eq!(find_other("wtm-test"), None);
        assert_eq!(find_other("no window has this title 0x5f3a"), None);
        // The focus may be refused (no interactive desktop on CI).
        let _ = bring_to_front(raw);
        assert!(is_visible(raw));
        release_topmost(raw);
        // Test binaries carry no icon resource.
        assert!(!use_exe_icon(raw));
        hide(raw);
        assert!(!is_visible(raw));
        // SAFETY: created above.
        unsafe { DestroyWindow(h).unwrap() };
        // A destroyed window is ignored.
        assert!(!bring_to_front(raw));
        assert!(!is_visible(raw));
    }
}

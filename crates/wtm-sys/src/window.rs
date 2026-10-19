//! The desktop's foreground window.

use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

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

#[cfg(test)]
mod tests {
    #[test]
    fn foreground_pid_is_a_real_process_or_none() {
        // CI runs without an interactive desktop, so both are possible.
        if let Some(pid) = super::foreground_pid() {
            assert!(pid != 0);
        }
    }
}

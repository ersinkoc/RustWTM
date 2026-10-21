//! The desktop's foreground window, and bringing our own window forward.

use std::sync::{Mutex, OnceLock, PoisonError};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HINSTANCE, HWND,
    LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, EnumWindows, GetMessageW, GetWindowTextW,
    MSG, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WM_ENDSESSION, WM_QUERYENDSESSION,
    WNDCLASSW, WS_OVERLAPPED,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FLASHW_ALL, FLASHW_TIMERNOFG, FLASHWINFO, FlashWindowEx, GetForegroundWindow,
    GetWindowThreadProcessId, HWND_NOTOPMOST, HWND_TOPMOST, IsIconic, IsWindowVisible, SW_HIDE,
    SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    SetForegroundWindow, SetWindowPos, ShowWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LR_SHARED, LoadImageW,
    SM_CXICON, SM_CXSMICON, SendMessageW, WM_SETICON,
};
use windows::core::{BOOL, PCWSTR, w};

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

/// The foreground process and whether its window covers its whole monitor
/// (a game, a full-screen video or presentation). The desktop and the
/// taskbar never count as full screen.
pub fn foreground_fullscreen() -> Option<(u32, bool)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowRect};
    // SAFETY: no arguments; returns a null HWND when there is no window.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return None;
    }
    let mut pid = 0u32;
    // SAFETY: `hwnd` came from the system; `pid` is a writable u32.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    let mut class = [0u16; 64];
    // SAFETY: `class` is writable for its length.
    let len = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
    let class = String::from_utf16_lossy(&class[..len.min(class.len())]);
    if [
        "Progman",
        "WorkerW",
        "Shell_TrayWnd",
        "Shell_SecondaryTrayWnd",
    ]
    .contains(&class.as_str())
    {
        return Some((pid, false));
    }
    let mut rect = RECT::default();
    // SAFETY: `rect` is writable.
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return Some((pid, false));
    }
    // SAFETY: a valid window handle; the monitor handle needs no release.
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is sized for MONITORINFO.
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return Some((pid, false));
    }
    Some((pid, covers(rect, info.rcMonitor)))
}

/// A window covers its monitor when it reaches every edge.
fn covers(
    window: windows::Win32::Foundation::RECT,
    monitor: windows::Win32::Foundation::RECT,
) -> bool {
    window.left <= monitor.left
        && window.top <= monitor.top
        && window.right >= monitor.right
        && window.bottom >= monitor.bottom
}

/// A top-level window (shown or hidden) with exactly this title that belongs
/// to another copy of this program (same executable file name): its handle
/// and process id. A folder of that name open in Explorer does not count.
pub fn find_other(title: &str) -> Option<(isize, u32)> {
    let own = std::env::current_exe().ok()?;
    let own = own.file_name()?.to_string_lossy().into_owned();
    let mut search = Search {
        title: title.encode_utf16().collect(),
        found: Vec::new(),
    };
    // SAFETY: `collect_titled` only borrows `search` behind the pointer and
    // runs synchronously for the duration of the call.
    unsafe {
        let _ = EnumWindows(
            Some(collect_titled),
            LPARAM(&mut search as *mut Search as isize),
        );
    }
    let me = std::process::id();
    search.found.into_iter().find(|&(_, pid)| {
        pid != 0
            && pid != me
            && crate::control::ProcessHandle::open(pid, crate::control::Access::QUERY)
                .and_then(|p| p.image_path())
                .is_ok_and(|path| same_file_name(&path, &own))
    })
}

struct Search {
    title: Vec<u16>,
    found: Vec<(isize, u32)>,
}

/// The window enumeration callback of `find_other`: every top-level window
/// with the searched title.
unsafe extern "system" fn collect_titled(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` carries the `&mut Search` set up by `find_other`, which
    // outlives the enumeration; the window handle is valid for query here.
    unsafe {
        let search = &mut *(lparam.0 as *mut Search);
        let mut text = [0u16; 64];
        let len = GetWindowTextW(hwnd, &mut text).max(0) as usize;
        if text[..len] == search.title[..] {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            search.found.push((hwnd.0 as isize, pid));
        }
    }
    true.into()
}

/// Whether `path` names a file called `name` (Windows file names ignore case).
fn same_file_name(path: &str, name: &str) -> bool {
    std::path::Path::new(path)
        .file_name()
        .is_some_and(|f| f.to_string_lossy().to_lowercase() == name.to_lowercase())
}

/// A named mutex held for the life of the process, so a second copy can tell
/// that one already runs. Unlike a window title, no unrelated window can
/// pretend to be it.
#[derive(Debug)]
pub struct InstanceLock(Option<HANDLE>);

impl Drop for InstanceLock {
    fn drop(&mut self) {
        if let Some(h) = self.0 {
            // SAFETY: created by `single_instance` and closed only here.
            unsafe {
                let _ = CloseHandle(h);
            }
        }
    }
}

/// Takes the mutex `name` (e.g. `Local\App`, one per session). `None` when
/// another process holds it, including one with rights we lack (an elevated
/// copy). Should the mutex not be creatable at all, the caller gets a lock
/// that holds nothing rather than refusing to start.
pub fn single_instance(name: &str) -> Option<InstanceLock> {
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: `wide` is NUL-terminated; the handle is owned by the lock.
    match unsafe { CreateMutexW(None, false, PCWSTR(wide.as_ptr())) } {
        Ok(h) => {
            let lock = InstanceLock(Some(h));
            // SAFETY: no arguments; read right after the call that set it.
            (unsafe { GetLastError() } != ERROR_ALREADY_EXISTS).then_some(lock)
        }
        Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => None,
        Err(_) => Some(InstanceLock(None)),
    }
}

/// Calls `on_end` once Windows ends the session, or Restart Manager (an
/// installer replacing our files) asks the program to close. Both are
/// announced only to top-level windows with `WM_ENDSESSION`, which the
/// window library ignores; a hidden window on its own thread listens
/// instead. A later call replaces `on_end`. Returns the listening window, or
/// `None` when it could not be created.
pub fn watch_session_end(on_end: impl Fn() + Send + 'static) -> Option<isize> {
    static WINDOW: OnceLock<Option<isize>> = OnceLock::new();
    *SESSION_END.lock().unwrap_or_else(PoisonError::into_inner) = Some(Box::new(on_end));
    *WINDOW.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("session-end".into())
            .spawn(move || session_window_loop(&tx))
            .ok()?;
        rx.recv().ok().flatten()
    })
}

type SessionEndHandler = Box<dyn Fn() + Send>;

static SESSION_END: Mutex<Option<SessionEndHandler>> = Mutex::new(None);

/// Creates the hidden window, reports it and pumps its messages forever.
fn session_window_loop(created: &std::sync::mpsc::Sender<Option<isize>>) {
    let class = w!("RustWTM.SessionEnd");
    // SAFETY: the class and window use static strings and our own module;
    // the loop only dispatches messages of this thread's window.
    unsafe {
        let Ok(module) = GetModuleHandleW(PCWSTR::null()) else {
            let _ = created.send(None);
            return;
        };
        let wc = WNDCLASSW {
            lpfnWndProc: Some(session_proc),
            hInstance: HINSTANCE(module.0),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        // Top-level and never shown: a message-only window is not told
        // about the end of the session.
        let window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR::null(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(HINSTANCE(module.0)),
            None,
        );
        let _ = created.send(window.ok().map(|h| h.0 as isize));
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn session_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        // Never object: the program can always close.
        WM_QUERYENDSESSION => LRESULT(1),
        // `wparam` is zero when the end of the session was called off.
        WM_ENDSESSION => {
            if wparam.0 != 0
                && let Some(on_end) = SESSION_END
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
            {
                on_end();
            }
            LRESULT(0)
        }
        // SAFETY: the arguments are the ones Windows passed to us.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
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
                // MAKEINTRESOURCE(1): an integer resource id, not a pointer.
                PCWSTR(std::ptr::without_provenance(1)),
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

    #[test]
    fn full_screen_means_reaching_every_edge() {
        use windows::Win32::Foundation::RECT;
        let monitor = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        assert!(covers(monitor, monitor));
        // Exclusive full screen often overshoots by the border width.
        assert!(covers(
            RECT {
                left: -8,
                top: -8,
                right: 1928,
                bottom: 1088
            },
            monitor
        ));
        let maximized = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        assert!(
            !covers(maximized, monitor),
            "a maximized window leaves the taskbar"
        );
        let _ = foreground_fullscreen();
    }

    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        DestroyWindow, ENDSESSION_CLOSEAPP, WS_OVERLAPPEDWINDOW,
    };

    #[test]
    fn other_copies_are_recognized_by_their_executable() {
        assert!(same_file_name(
            r"C:\Program Files\RustWTM\RustWTM.EXE",
            "rustwtm.exe"
        ));
        assert!(!same_file_name(r"C:\Windows\explorer.exe", "rustwtm.exe"));
        assert!(!same_file_name("", "rustwtm.exe"));
    }

    #[test]
    fn only_one_holder_of_an_instance_lock() {
        let name = format!(r"Local\wtm-sys-test-{}", std::process::id());
        let first = single_instance(&name);
        assert!(first.is_some());
        assert!(single_instance(&name).is_none(), "already held");
        drop(first);
        assert!(single_instance(&name).is_some(), "released on drop");
    }

    #[test]
    fn the_end_of_the_session_is_announced() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static ENDED: AtomicUsize = AtomicUsize::new(0);
        let raw = watch_session_end(|| {}).expect("listening window");
        let again = watch_session_end(|| {
            ENDED.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(again, Some(raw), "created once");
        let send = |message, wparam, lparam: u32| {
            // SAFETY: a window of this process; its own thread handles the
            // message while we wait.
            unsafe {
                SendMessageW(
                    hwnd(raw),
                    message,
                    Some(WPARAM(wparam)),
                    Some(LPARAM(lparam as isize)),
                )
            }
        };
        assert_eq!(send(WM_QUERYENDSESSION, 0, ENDSESSION_CLOSEAPP).0, 1);
        // Called off: nothing happens.
        send(WM_ENDSESSION, 0, ENDSESSION_CLOSEAPP);
        assert_eq!(ENDED.load(Ordering::Relaxed), 0);
        send(WM_ENDSESSION, 1, ENDSESSION_CLOSEAPP);
        assert_eq!(ENDED.load(Ordering::Relaxed), 1);
        assert!(!is_visible(raw));
    }

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

//! One optional user-selected global shortcut, with thread-owned registration.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_HOTKEY, WM_QUIT,
};
const ID: i32 = 0x5254;
static LIVE: AtomicBool = AtomicBool::new(false);
struct Slot;
impl Drop for Slot {
    fn drop(&mut self) {
        LIVE.store(false, Ordering::Release);
    }
}
pub struct Hotkey {
    thread: u32,
    stop: Arc<AtomicBool>,
}
impl Hotkey {
    pub fn new(
        function_key: u8,
        mut pressed: impl FnMut() + Send + 'static,
    ) -> Result<Self, String> {
        if !(2..=12).contains(&function_key) {
            return Err("Choose a function key from F2 to F12".into());
        }
        // Rebinding may overlap the retiring thread briefly. Never accumulate
        // message threads if Windows takes longer to release a registration.
        for _ in 0..10 {
            if !LIVE.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        LIVE.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                "The previous shortcut is still stopping; toggle the shortcut to retry".to_owned()
            })?;
        let slot = Slot;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("global-shortcut".into())
            .spawn(move || {
                let _slot = slot;
                let mut message = MSG::default();
                // SAFETY: initialized writable MSG. Creating the queue before
                // acknowledging registration makes teardown WM_QUIT reliable.
                let _ = unsafe { PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE) };
                // SAFETY: registers one thread-owned key; no window or borrowed pointer is retained.
                let result = unsafe {
                    RegisterHotKey(
                        None,
                        ID,
                        MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                        0x6f + u32::from(function_key),
                    )
                };
                if let Err(e) = result {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
                struct Registration;
                impl Drop for Registration {
                    fn drop(&mut self) {
                        // SAFETY: drop occurs on the same thread that registered ID.
                        let _ = unsafe { UnregisterHotKey(None, ID) };
                    }
                }
                let _registration = Registration;
                // SAFETY: takes no pointers and identifies this live message thread.
                let id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
                let _ = tx.send(Ok(id));
                while !stopped.load(Ordering::Acquire) {
                    // SAFETY: message points to writable initialized storage for this thread.
                    let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
                    if result.0 <= 0 {
                        break;
                    }
                    if message.message == WM_HOTKEY && message.wParam.0 == ID as usize {
                        pressed();
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        match rx.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(Ok(thread)) => Ok(Self { thread, stop }),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                stop.store(true, Ordering::Release);
                Err("Global shortcut registration timed out".into())
            }
        }
    }
}
impl Drop for Hotkey {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // SAFETY: posts only WM_QUIT to the registration thread, no pointer payload.
        let _ = unsafe { PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn invalid_keys_do_not_register_or_spawn_threads() {
        for k in [0, 1, 13, 255] {
            assert!(super::Hotkey::new(k, || {}).is_err());
        }
    }
}

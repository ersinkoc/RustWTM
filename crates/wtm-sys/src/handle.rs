use windows::Win32::Foundation::{CloseHandle, HANDLE};

/// A kernel handle closed on drop.
#[derive(Debug)]
pub struct OwnedHandle(HANDLE);

// SAFETY: kernel handles are process-wide and usable from any thread.
unsafe impl Send for OwnedHandle {}
// SAFETY: as above; the wrapper exposes no interior mutability.
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    /// Takes ownership of `handle`, which must be valid and not owned elsewhere.
    pub(crate) fn new(handle: HANDLE) -> Self {
        Self(handle)
    }

    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

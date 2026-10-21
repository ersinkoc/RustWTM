//! Drive letters with their label, file system and free space.

use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW,
};
use windows::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOOPENFILEERRORBOX, SetThreadErrorMode, THREAD_ERROR_MODE,
};
use windows::core::PCWSTR;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DriveKind {
    Fixed,
    Removable,
    Network,
    Optical,
    RamDisk,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Volume {
    /// Root path, e.g. `C:\`.
    pub root: String,
    pub label: String,
    pub file_system: String,
    pub kind: DriveKind,
    pub total: u64,
    /// Free space available to the calling user (quotas applied).
    pub free: u64,
}

impl Volume {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }

    /// Share of the volume in use, 0..=100.
    pub fn used_percent(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.used() as f64 / self.total as f64 * 100.0) as f32
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn text(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Every drive letter with media. Drives without media (an empty card reader
/// or DVD drive) and drives that do not answer are left out. A network drive
/// that is slow to answer delays the call, so call it off the UI thread.
pub fn volumes() -> Vec<Volume> {
    let mut previous = THREAD_ERROR_MODE::default();
    // SAFETY: plain FFI; suppresses "insert a disk" dialogs on this thread
    // only, and the previous mode is restored below.
    let _ = unsafe {
        SetThreadErrorMode(
            SEM_FAILCRITICALERRORS | SEM_NOOPENFILEERRORBOX,
            Some(&mut previous),
        )
    };
    let mut buf = [0u16; 512];
    // SAFETY: `buf` is writable for its length.
    let len = unsafe { GetLogicalDriveStringsW(Some(&mut buf)) } as usize;
    let roots: Vec<String> = if len == 0 || len > buf.len() {
        Vec::new()
    } else {
        buf[..len]
            .split(|&c| c == 0)
            .filter(|s| !s.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    };
    let list = roots.iter().filter_map(|r| volume(r)).collect();
    // SAFETY: restores the mode saved above.
    let _ = unsafe { SetThreadErrorMode(previous, None) };
    list
}

fn volume(root: &str) -> Option<Volume> {
    let path = wide(root);
    // SAFETY: `path` is NUL-terminated and outlives the call.
    let kind = match unsafe { GetDriveTypeW(PCWSTR(path.as_ptr())) } {
        2 => DriveKind::Removable,
        3 => DriveKind::Fixed,
        4 => DriveKind::Network,
        5 => DriveKind::Optical,
        6 => DriveKind::RamDisk,
        // 0 unknown, 1 no root directory.
        1 => return None,
        _ => DriveKind::Unknown,
    };
    let mut label = [0u16; 261];
    let mut fs = [0u16; 261];
    // SAFETY: both buffers are writable for their lengths; the optional
    // out-pointers are not requested.
    unsafe {
        GetVolumeInformationW(
            PCWSTR(path.as_ptr()),
            Some(&mut label),
            None,
            None,
            None,
            Some(&mut fs),
        )
    }
    .ok()?;
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: the out-pointers reference live u64s.
    unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(path.as_ptr()),
            Some(&mut free),
            Some(&mut total),
            None,
        )
    }
    .ok()?;
    Some(Volume {
        root: root.to_owned(),
        label: text(&label),
        file_system: text(&fs),
        kind,
        total,
        free,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_system_drive_with_its_space() {
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let list = volumes();
        let c = list
            .iter()
            .find(|v| v.root.eq_ignore_ascii_case(&format!("{system}\\")))
            .expect("the system drive is listed");
        assert_eq!(c.kind, DriveKind::Fixed);
        assert!(c.total > 0 && c.free <= c.total, "{c:?}");
        assert!(!c.file_system.is_empty());
        assert!((0.0..=100.0).contains(&c.used_percent()));
    }

    #[test]
    fn a_missing_root_is_skipped() {
        assert_eq!(volume(r"\\?\no-such-volume\"), None);
        let empty = Volume {
            root: "X:\\".into(),
            label: String::new(),
            file_system: String::new(),
            kind: DriveKind::Unknown,
            total: 0,
            free: 0,
        };
        assert_eq!(empty.used_percent(), 0.0);
    }
}

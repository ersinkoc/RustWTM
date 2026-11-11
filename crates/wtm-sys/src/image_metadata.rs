//! Read display names from Windows version resources without running the image.
use std::ffi::c_void;
use windows::Win32::Storage::FileSystem::{
    GetDriveTypeW, GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::core::PCWSTR;

const MAX_VERSION_BYTES: u32 = 256 * 1024;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn query<'a>(block: &'a [u8], key: &str, unit: usize) -> Option<&'a [u8]> {
    let key = wide(key);
    let mut value = std::ptr::null_mut::<c_void>();
    let mut length = 0;
    // SAFETY: block is a live version resource returned by GetFileVersionInfoW.
    // The key is NUL terminated and both output pointers are writable.
    if !unsafe {
        VerQueryValueW(
            block.as_ptr().cast(),
            PCWSTR(key.as_ptr()),
            &mut value,
            &mut length,
        )
    }
    .as_bool()
    {
        return None;
    }
    let start = (value as usize).checked_sub(block.as_ptr() as usize)?;
    let end = start.checked_add((length as usize).checked_mul(unit)?)?;
    block.get(start..end)
}

pub fn description(path: &str) -> Option<String> {
    let drive = path.get(..3)?;
    if !drive.as_bytes()[0].is_ascii_alphabetic()
        || &drive.as_bytes()[1..] != b":\\"
        || path.contains('\0')
    {
        return None;
    }
    let drive = wide(drive);
    // SAFETY: valid NUL-terminated drive root. DRIVE_FIXED = 3.
    if unsafe { GetDriveTypeW(PCWSTR(drive.as_ptr())) } != 3 {
        return None;
    }
    let path = wide(path);
    // SAFETY: path is a live NUL-terminated absolute file name.
    let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(path.as_ptr()), None) };
    if !(1..=MAX_VERSION_BYTES).contains(&size) {
        return None;
    }
    let mut block = vec![0_u32; (size as usize).div_ceil(4)];
    // SAFETY: the aligned, initialized buffer provides at least size writable bytes.
    unsafe { GetFileVersionInfoW(PCWSTR(path.as_ptr()), None, size, block.as_mut_ptr().cast()) }
        .ok()?;
    // SAFETY: the first size bytes are initialized and live until block is dropped.
    let bytes = unsafe { std::slice::from_raw_parts(block.as_ptr().cast::<u8>(), size as usize) };
    let translations = query(bytes, r"\VarFileInfo\Translation", 1).unwrap_or_default();
    let candidates = translations
        .as_chunks::<4>()
        .0
        .iter()
        .take(16)
        .map(|pair| {
            (
                u16::from_le_bytes([pair[0], pair[1]]),
                u16::from_le_bytes([pair[2], pair[3]]),
            )
        })
        .chain([(0x0409, 1200), (0x0409, 1252)]);
    for (language, code_page) in candidates {
        let key = format!(r"\StringFileInfo\{language:04x}{code_page:04x}\FileDescription");
        let Some(value) = query(bytes, &key, 2) else {
            continue;
        };
        let utf16: Vec<_> = value
            .as_chunks::<2>()
            .0
            .iter()
            .take(256)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|&character| character != 0)
            .collect();
        let name = String::from_utf16_lossy(&utf16);
        let name = name.trim();
        if !name.is_empty() && !name.chars().any(char::is_control) {
            return Some(name.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::description;

    #[test]
    fn rejects_relative_network_device_and_nul_paths() {
        for path in [
            "",
            "app.exe",
            r"C:app.exe",
            r"\Windows\app.exe",
            r"\\server\share\app.exe",
            r"\\?\C:\app.exe",
            "C:\\app\0.exe",
        ] {
            assert!(description(path).is_none(), "{path:?}");
        }
    }
}

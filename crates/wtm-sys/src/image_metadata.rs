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

/// The version resource of a local fixed-disk image, read without running it.
fn version_block(path: &str) -> Option<Vec<u32>> {
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
    block.truncate((size as usize).div_ceil(4));
    Some(block)
}

fn block_bytes(block: &[u32]) -> &[u8] {
    // SAFETY: every u32 of the block is initialized; u8 has no alignment needs.
    unsafe { std::slice::from_raw_parts(block.as_ptr().cast::<u8>(), size_of_val(block)) }
}

/// What a version resource says about an image.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    pub description: Option<String>,
    /// `major.minor.build.revision` from the fixed file information.
    pub version: Option<String>,
}

/// Description and file version, reading the image's resource once.
pub fn metadata(path: &str) -> Metadata {
    let Some(block) = version_block(path) else {
        return Metadata::default();
    };
    let bytes = block_bytes(&block);
    Metadata {
        description: description_in(bytes),
        version: query(bytes, "\\", 1).and_then(fixed_version),
    }
}

pub fn description(path: &str) -> Option<String> {
    description_in(block_bytes(&version_block(path)?))
}

/// `VS_FIXEDFILEINFO` file version, or `None` when the signature is wrong.
fn fixed_version(info: &[u8]) -> Option<String> {
    let word = |i: usize| -> Option<u32> {
        Some(u32::from_le_bytes(
            info.get(i * 4..i * 4 + 4)?.try_into().ok()?,
        ))
    };
    if word(0)? != 0xFEEF_04BD {
        return None;
    }
    let (ms, ls) = (word(2)?, word(3)?);
    Some(format!(
        "{}.{}.{}.{}",
        ms >> 16,
        ms & 0xffff,
        ls >> 16,
        ls & 0xffff
    ))
}

fn description_in(bytes: &[u8]) -> Option<String> {
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
    use super::{description, fixed_version, metadata};

    #[test]
    fn fixed_file_info_gives_four_part_version_and_checks_its_signature() {
        let mut info = [0u8; 52];
        info[..4].copy_from_slice(&0xFEEF_04BDu32.to_le_bytes());
        info[8..12].copy_from_slice(&((10u32 << 16) | 2).to_le_bytes());
        info[12..16].copy_from_slice(&((19041u32 << 16) | 3636).to_le_bytes());
        assert_eq!(fixed_version(&info).as_deref(), Some("10.2.19041.3636"));
        info[0] = 0;
        assert_eq!(fixed_version(&info), None);
        assert_eq!(fixed_version(&info[..8]), None);
    }

    #[test]
    fn this_system_dll_reports_a_version() {
        let dir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let m = metadata(&format!(r"{dir}\System32\kernel32.dll"));
        assert!(m.version.is_some_and(|v| v.split('.').count() == 4));
    }

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

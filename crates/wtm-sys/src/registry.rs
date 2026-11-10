//! Fixed-buffer registry value reads; never allocate from a registry-reported size.
use windows::{
    Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{
            HKEY, REG_DWORD, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE, RegQueryValueExW,
        },
    },
    core::PCWSTR,
};

pub enum Value {
    Text(String),
    Dword(u32),
}

fn decode(kind: REG_VALUE_TYPE, bytes: &[u8]) -> Option<Value> {
    if bytes.len() > 8192 {
        return None;
    }
    if kind == REG_DWORD && bytes.len() == 4 {
        return Some(Value::Dword(u32::from_le_bytes(bytes.try_into().ok()?)));
    }
    if (kind == REG_SZ || kind == REG_EXPAND_SZ) && bytes.len().is_multiple_of(2) {
        let words: Vec<_> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .take_while(|v| *v != 0)
            .collect();
        return Some(Value::Text(String::from_utf16_lossy(&words)));
    }
    None
}

/// `key` is a borrowed Windows registry handle; invalid/closed handles fail.
/// Strings over 8192 bytes and unsupported value types are unavailable.
pub fn value(key: usize, name: &str) -> Option<Value> {
    if name.len() > 256 || name.contains('\0') {
        return None;
    }
    let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let mut bytes = [0u8; 8192];
    let mut size = bytes.len() as u32;
    let mut kind = REG_VALUE_TYPE::default();
    // SAFETY: all buffers are valid for their supplied lengths. The handle is
    // borrowed, never dereferenced or closed here. ERROR_MORE_DATA is rejected;
    // no allocation uses the untrusted returned size, including registry races.
    let result = unsafe {
        RegQueryValueExW(
            HKEY(key as *mut _),
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            Some(bytes.as_mut_ptr()),
            Some(&mut size),
        )
    };
    if result != ERROR_SUCCESS || size as usize > bytes.len() {
        return None;
    }
    decode(kind, &bytes[..size as usize])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_value_decoding_rejects_wrong_types_and_oversize_without_large_allocations() {
        assert!(matches!(
            decode(REG_DWORD, &[1, 0, 0, 0]),
            Some(Value::Dword(1))
        ));
        assert!(matches!(decode(REG_SZ,&[b'A',0,0,0]),Some(Value::Text(s)) if s=="A"));
        assert!(decode(REG_SZ, &[1]).is_none());
        assert!(decode(REG_DWORD, &[0; 8]).is_none());
        assert!(decode(REG_SZ, &[0; 8193]).is_none());
        assert!(value(0, "invalid").is_none());
    }
}

//! Per-user secrets through DPAPI (`CryptProtectData`): only the Windows
//! account that sealed a secret can open it again, on this machine or where
//! its roaming profile goes. Used for AI provider API keys.

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows::core::PCWSTR;

use crate::Result;

/// Output DPAPI allocated; freed with `LocalFree`.
struct Owned(CRYPT_INTEGER_BLOB);

impl Owned {
    fn bytes(&self) -> Vec<u8> {
        if self.0.pbData.is_null() {
            return Vec::new();
        }
        // SAFETY: DPAPI returned `cbData` bytes at `pbData`.
        unsafe { std::slice::from_raw_parts(self.0.pbData, self.0.cbData as usize).to_vec() }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.pbData.is_null() {
            // SAFETY: DPAPI allocated the buffer with LocalAlloc.
            unsafe { LocalFree(Some(HLOCAL(self.0.pbData.cast()))) };
        }
    }
}

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        // DPAPI only reads input blobs.
        pbData: data.as_ptr().cast_mut(),
    }
}

/// Seals `secret` for the current Windows user. `entropy` must be given
/// again to open it, so a secret sealed for one purpose cannot be opened
/// by code asking for another.
pub fn protect(secret: &[u8], entropy: &[u8]) -> Result<Vec<u8>> {
    let input = blob(secret);
    let extra = blob(entropy);
    let mut out = Owned(CRYPT_INTEGER_BLOB::default());
    // SAFETY: the input blobs point into live slices; DPAPI fills `out`.
    unsafe {
        CryptProtectData(
            &input,
            PCWSTR::null(),
            Some(&extra),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out.0,
        )?;
    }
    Ok(out.bytes())
}

/// Opens what [`protect`] sealed with the same `entropy`, for the same user.
pub fn unprotect(sealed: &[u8], entropy: &[u8]) -> Result<Vec<u8>> {
    let input = blob(sealed);
    let extra = blob(entropy);
    let mut out = Owned(CRYPT_INTEGER_BLOB::default());
    // SAFETY: as in `protect`.
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            Some(&extra),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out.0,
        )?;
    }
    Ok(out.bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_needs_the_same_entropy() {
        let sealed = protect(b"sk-test-123", b"purpose-a").unwrap();
        assert!(!sealed.windows(11).any(|w| w == b"sk-test-123"));
        assert_eq!(unprotect(&sealed, b"purpose-a").unwrap(), b"sk-test-123");
        assert!(unprotect(&sealed, b"purpose-b").is_err());
        assert!(unprotect(b"not sealed", b"purpose-a").is_err());
        // An empty secret survives too.
        let empty = protect(b"", b"x").unwrap();
        assert_eq!(unprotect(&empty, b"x").unwrap(), b"");
    }
}

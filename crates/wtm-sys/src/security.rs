//! File-system ACLs.

use std::path::Path;

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT,
    SetNamedSecurityInfoW,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR,
};
use windows::core::{BOOL, PCWSTR};

use crate::Result;

/// Replaces the DACL of `path` with the one in `sddl` (e.g.
/// `D:P(A;OICI;FA;;;SY)`) and stops inheriting from the parent.
pub fn set_dacl(path: &Path, sddl: &str) -> Result<()> {
    let sddl: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
    let path: Vec<u16> = path.as_os_str().encode_wide_nul();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `sddl` is NUL-terminated; `sd` receives a LocalAlloc'd block.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )?
    };
    let result = (|| {
        let mut present = BOOL::default();
        let mut defaulted = BOOL::default();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        // SAFETY: `sd` is a valid descriptor; out-pointers are writable.
        unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted)? };
        // SAFETY: `path` is NUL-terminated; `dacl` points into `sd`.
        unsafe {
            SetNamedSecurityInfoW(
                PCWSTR(path.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(dacl),
                None,
            )
            .ok()?
        };
        Ok(())
    })();
    // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
    unsafe { LocalFree(Some(HLOCAL(sd.0))) };
    result
}

trait EncodeWideNul {
    fn encode_wide_nul(&self) -> Vec<u16>;
}

impl EncodeWideNul for std::ffi::OsStr {
    fn encode_wide_nul(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain([0]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_dacl_to_scratch_dir() {
        let dir = std::env::temp_dir().join(format!("wtm-acl-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Owner rights keep the test able to clean up.
        set_dacl(&dir, "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)").unwrap();
        std::fs::write(dir.join("f.txt"), b"x").unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

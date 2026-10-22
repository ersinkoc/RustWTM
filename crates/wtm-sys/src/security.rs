//! File-system ACLs.

use std::path::Path;

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, ProgressInvokeNever, SDDL_REVISION_1,
    SE_FILE_OBJECT, SetNamedSecurityInfoW, TreeResetNamedSecurityInfoW,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SE_RESTORE_NAME, SE_TAKE_OWNERSHIP_NAME,
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

/// Makes `path` and everything below it what `sddl` says: its owner (an
/// `O:` part, optional) and its DACL, protected at `path`, inherited by
/// every file and folder below with their own explicit entries removed. A
/// folder someone else created first, or files they left in it with their
/// own permissions, are taken back.
pub fn secure_tree(path: &Path, sddl: &str) -> Result<()> {
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
        let (mut present, mut defaulted) = (BOOL::default(), BOOL::default());
        let mut dacl: *mut ACL = std::ptr::null_mut();
        // SAFETY: `sd` is a valid descriptor; out-pointers are writable.
        unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted)? };
        if !present.as_bool() || dacl.is_null() {
            // Without a DACL the objects would be open to everyone.
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_INVALIDARG,
            )
            .into());
        }
        let mut owner = PSID::default();
        // SAFETY: as above.
        unsafe { GetSecurityDescriptorOwner(sd, &mut owner, &mut defaulted)? };
        let mut info = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
        if !owner.is_invalid() {
            info |= OWNER_SECURITY_INFORMATION;
            // Giving away ownership needs these; held but off for SYSTEM.
            let _ = crate::privilege::enable(SE_TAKE_OWNERSHIP_NAME);
            let _ = crate::privilege::enable(SE_RESTORE_NAME);
        }
        // SAFETY: `path` is NUL-terminated; `owner` and `dacl` point into `sd`.
        unsafe {
            TreeResetNamedSecurityInfoW(
                PCWSTR(path.as_ptr()),
                SE_FILE_OBJECT,
                info,
                (!owner.is_invalid()).then_some(owner),
                None,
                Some(dacl),
                None,
                false,
                None,
                ProgressInvokeNever,
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

    #[test]
    fn securing_a_tree_takes_back_files_with_their_own_permissions() {
        use windows::Win32::Security::Authorization::GetNamedSecurityInfoW;
        use windows::Win32::Security::{GetSecurityDescriptorControl, SE_DACL_PROTECTED};
        let dir = std::env::temp_dir().join(format!("wtm-acl-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let planted = dir.join("sub").join("config.toml");
        std::fs::write(&planted, b"x").unwrap();
        // A file someone left with permissions of its own.
        set_dacl(&planted, "D:P(A;;FA;;;OW)(A;;FA;;;WD)").unwrap();
        let protected = |p: &Path| {
            let wide = p.as_os_str().encode_wide_nul();
            let mut sd = PSECURITY_DESCRIPTOR::default();
            // SAFETY: `wide` is NUL-terminated; `sd` is freed below.
            unsafe {
                let e = GetNamedSecurityInfoW(
                    PCWSTR(wide.as_ptr()),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    None,
                    None,
                    None,
                    None,
                    &mut sd,
                );
                assert!(e.is_ok(), "{e:?}");
                let (mut control, mut revision) = (0u16, 0u32);
                GetSecurityDescriptorControl(sd, &mut control, &mut revision).unwrap();
                LocalFree(Some(HLOCAL(sd.0)));
                control & SE_DACL_PROTECTED.0 != 0
            }
        };
        assert!(protected(&planted));
        secure_tree(&dir, "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)").unwrap();
        assert!(protected(&dir));
        assert!(!protected(&planted), "now inherits from the secured folder");
        // A descriptor without a DACL would open everything; refused.
        assert!(secure_tree(&dir, "O:BA").is_err());
        // Giving the tree to Administrators needs elevation (CI has it).
        if crate::privilege::enable_debug_privilege().unwrap() {
            secure_tree(&dir, "O:BAD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)").unwrap();
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_malformed_sddl_before_touching_the_path() {
        let dir = std::env::temp_dir();
        assert!(set_dacl(&dir, "not an sddl string").is_err());
    }
}

//! File-system ACLs.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_FILE_OBJECT, SetNamedSecurityInfoW, SetSecurityInfo,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, IsWellKnownSid,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, WinBuiltinAdministratorsSid, WinLocalSystemSid,
};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
    MOVE_FILE_FLAGS, MoveFileExW, OPEN_EXISTING, READ_CONTROL, WRITE_DAC,
};
use windows::core::{BOOL, PCWSTR};

use crate::Result;
use crate::handle::OwnedHandle;

/// Replaces the DACL of `path` with the one in `sddl` (e.g.
/// `D:P(A;OICI;FA;;;SY)`) and stops inheriting from the parent.
pub fn set_dacl(path: &Path, sddl: &str) -> Result<()> {
    let sd = Descriptor::from_sddl(sddl)?;
    let path: Vec<u16> = path.as_os_str().encode_wide_nul();
    // SAFETY: `path` is NUL-terminated; the DACL points into `sd`.
    unsafe {
        SetNamedSecurityInfoW(
            PCWSTR(path.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(sd.dacl()?),
            None,
        )
        .ok()?
    };
    Ok(())
}

/// What [`secure_dir`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum Secured {
    /// There was nothing; it was created with the descriptor.
    Created,
    /// A directory of SYSTEM or Administrators: its DACL was replaced.
    Kept,
    /// Not to be trusted (a link, or someone else's): moved to this path,
    /// and created anew.
    Replaced(PathBuf),
    /// Something untrusted kept coming back at the path while it was being
    /// replaced: nothing in it may be relied on.
    Contested,
}

/// Makes `path` a directory with the DACL in `sddl` that nobody but SYSTEM
/// and Administrators controls. One owned by anyone else, which a standard
/// user could have created before installation with files and permissions
/// of their own, or a junction or other link to somewhere else, is moved
/// aside (the link itself, never its target) and a new one created. Only
/// the directory is changed, through a handle, so no link inside or at the
/// path is ever followed.
pub fn secure_dir(path: &Path, sddl: &str) -> Result<Secured> {
    secure_dir_with(path, sddl, trusted_owner)
}

fn secure_dir_with(path: &Path, sddl: &str, trusted: impl Fn(PSID) -> bool) -> Result<Secured> {
    let sd = Descriptor::from_sddl(sddl)?;
    let dacl = sd.dacl()?;
    let wide: Vec<u16> = path.as_os_str().encode_wide_nul();
    let mut moved = None;
    for _ in 0..3 {
        match open_dir(&wide) {
            Err(e) if is_missing(&e) => {
                let attributes = SECURITY_ATTRIBUTES {
                    nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: sd.0.0,
                    bInheritHandle: false.into(),
                };
                // SAFETY: `wide` is NUL-terminated; `attributes` points to
                // the descriptor in `sd`, alive for the call.
                match unsafe { CreateDirectoryW(PCWSTR(wide.as_ptr()), Some(&attributes)) } {
                    Ok(()) => return Ok(moved.map_or(Secured::Created, Secured::Replaced)),
                    // Someone else created it meanwhile: look at it again.
                    Err(e) if e.code() == ERROR_ALREADY_EXISTS.to_hresult() => continue,
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(dir) if ours(&dir, &trusted)? => {
                // SAFETY: `dir` is open with WRITE_DAC; the DACL points into `sd`.
                unsafe {
                    SetSecurityInfo(
                        dir.raw(),
                        SE_FILE_OBJECT,
                        DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                        None,
                        None,
                        Some(dacl),
                        None,
                    )
                    .ok()?
                };
                return Ok(moved.map_or(Secured::Kept, Secured::Replaced));
            }
            // A link, a file, someone else's directory, or one that even
            // refuses us: renamed (a link is renamed, not followed).
            Ok(_) => moved = Some(move_aside(path, &wide)?),
            Err(e) if e.is_access_denied() => moved = Some(move_aside(path, &wide)?),
            Err(e) => return Err(e),
        }
    }
    Ok(Secured::Contested)
}

/// Opens `path` itself, a link included, to read and set its security.
fn open_dir(path: &[u16]) -> Result<OwnedHandle> {
    // SAFETY: `path` is NUL-terminated; the handle is owned below.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            (READ_CONTROL | WRITE_DAC).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )?
    };
    Ok(OwnedHandle::new(handle))
}

/// A real directory (no link) whose owner `trusted` accepts.
fn ours(dir: &OwnedHandle, trusted: &impl Fn(PSID) -> bool) -> Result<bool> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `dir` is a valid handle; `info` is writable.
    unsafe { GetFileInformationByHandle(dir.raw(), &mut info)? };
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0
    {
        return Ok(false);
    }
    let mut owner = PSID::default();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `dir` is open with READ_CONTROL; `owner` points into `sd`,
    // which is freed below after its last use.
    unsafe {
        GetSecurityInfo(
            dir.raw(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            Some(&mut sd),
        )
        .ok()?;
        let trusted = trusted(owner);
        LocalFree(Some(HLOCAL(sd.0)));
        Ok(trusted)
    }
}

fn trusted_owner(owner: PSID) -> bool {
    // SAFETY: `owner` points to a valid SID.
    unsafe {
        IsWellKnownSid(owner, WinLocalSystemSid).as_bool()
            || IsWellKnownSid(owner, WinBuiltinAdministratorsSid).as_bool()
    }
}

/// Renames `path` to an unused `<path>.untrusted-<n>` next to it.
fn move_aside(path: &Path, wide: &[u16]) -> Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".untrusted-{stamp}"));
    let aside = PathBuf::from(name);
    let to: Vec<u16> = aside.as_os_str().encode_wide_nul();
    // SAFETY: both paths are NUL-terminated.
    unsafe {
        MoveFileExW(
            PCWSTR(wide.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVE_FILE_FLAGS(0),
        )?
    };
    Ok(aside)
}

fn is_missing(e: &crate::Error) -> bool {
    matches!(e, crate::Error::Win32(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult()
        || e.code() == ERROR_PATH_NOT_FOUND.to_hresult())
}

/// A security descriptor parsed from SDDL, freed on drop.
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    fn from_sddl(sddl: &str) -> Result<Self> {
        let sddl: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
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
        Ok(Self(sd))
    }

    /// Its DACL; an error when it has none, which would open the object to
    /// everyone.
    fn dacl(&self) -> Result<*const ACL> {
        let (mut present, mut defaulted) = (BOOL::default(), BOOL::default());
        let mut dacl: *mut ACL = std::ptr::null_mut();
        // SAFETY: `self.0` is a valid descriptor; out-pointers are writable.
        unsafe { GetSecurityDescriptorDacl(self.0, &mut present, &mut dacl, &mut defaulted)? };
        if !present.as_bool() || dacl.is_null() {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_INVALIDARG,
            )
            .into());
        }
        Ok(dacl)
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
    }
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

    const SDDL: &str = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)";

    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wtm-acl-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Whether the DACL of `path` (not followed if a link) is protected.
    fn protected(path: &Path) -> bool {
        use windows::Win32::Security::{GetSecurityDescriptorControl, SE_DACL_PROTECTED};
        let dir = open_dir(&path.as_os_str().encode_wide_nul()).unwrap();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `dir` is open with READ_CONTROL; `sd` is freed below.
        unsafe {
            GetSecurityInfo(
                dir.raw(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                None,
                None,
                Some(&mut sd),
            )
            .ok()
            .unwrap();
            let (mut control, mut revision) = (0u16, 0u32);
            GetSecurityDescriptorControl(sd, &mut control, &mut revision).unwrap();
            LocalFree(Some(HLOCAL(sd.0)));
            control & SE_DACL_PROTECTED.0 != 0
        }
    }

    #[test]
    fn applies_dacl_to_scratch_dir() {
        let dir = scratch("set");
        // Owner rights keep the test able to clean up.
        set_dacl(&dir, SDDL).unwrap();
        std::fs::write(dir.join("f.txt"), b"x").unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_malformed_sddl_before_touching_the_path() {
        let dir = std::env::temp_dir();
        assert!(set_dacl(&dir, "not an sddl string").is_err());
        // A descriptor without a DACL would open everything.
        assert!(secure_dir(&dir.join("never-made"), "O:BA").is_err());
    }

    #[test]
    fn a_missing_directory_is_created_secured() {
        let parent = scratch("create");
        let dir = parent.join("data");
        assert_eq!(secure_dir(&dir, SDDL).unwrap(), Secured::Created);
        assert!(protected(&dir));
        std::fs::remove_dir_all(&parent).unwrap();
    }

    #[test]
    fn a_trusted_directory_keeps_its_files_and_gets_the_dacl() {
        let dir = scratch("kept");
        std::fs::write(dir.join("config.toml"), b"x").unwrap();
        assert!(!protected(&dir));
        assert_eq!(
            secure_dir_with(&dir, SDDL, |_| true).unwrap(),
            Secured::Kept
        );
        assert!(protected(&dir));
        assert!(dir.join("config.toml").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn someone_elses_directory_is_moved_aside_and_made_anew() {
        let parent = scratch("replaced");
        let dir = parent.join("data");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), b"planted").unwrap();
        // The current user owns it here; to the service any user is "else".
        let r = secure_dir(&dir, SDDL).unwrap();
        let Secured::Replaced(aside) = r else {
            panic!("{r:?}")
        };
        assert!(aside.join("config.toml").exists(), "kept aside, not lost");
        assert!(!dir.join("config.toml").exists(), "a fresh directory");
        assert!(protected(&dir));
        std::fs::remove_dir_all(&parent).unwrap();
    }

    #[test]
    fn a_junction_is_moved_aside_never_followed() {
        let parent = scratch("junction");
        let target = parent.join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("precious.txt"), b"x").unwrap();
        let link = parent.join("data");
        let made = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");

        // Even a trusted owner does not make a link acceptable.
        let r = secure_dir_with(&link, SDDL, |_| true).unwrap();
        let Secured::Replaced(aside) = r else {
            panic!("{r:?}")
        };
        assert!(!protected(&target), "the target was not touched");
        assert!(target.join("precious.txt").exists());
        assert!(protected(&link) && !link.join("precious.txt").exists());
        // Removing the moved junction removes the link only.
        std::fs::remove_dir(&aside).unwrap();
        assert!(target.join("precious.txt").exists());
        std::fs::remove_dir_all(&parent).unwrap();
    }
}

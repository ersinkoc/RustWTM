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
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
    DACL_SECURITY_INFORMATION, GetAce, GetAclInformation, GetSecurityDescriptorDacl,
    IsWellKnownSid, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, WinBuiltinAdministratorsSid,
    WinCreatorOwnerRightsSid, WinCreatorOwnerSid, WinLocalSystemSid,
};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ACCESS_RIGHTS,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_WRITE,
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

/// Creates the file `path` (replacing one already there) with the security
/// descriptor in `sddl` from the start: there is no moment in which its
/// inherited permissions let anyone else open it, as setting them after
/// writing would leave (a handle opened meanwhile keeps its access).
pub fn create_private(path: &Path, sddl: &str) -> Result<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    let sd = Descriptor::from_sddl(sddl)?;
    sd.dacl()?;
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io_error(e)),
        _ => {}
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide_nul();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0.0,
        bInheritHandle: false.into(),
    };
    // SAFETY: `wide` is NUL-terminated; `attributes` points to the
    // descriptor in `sd`, alive for the call; the handle is owned below.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_GENERIC_WRITE.0,
            windows::Win32::Storage::FileSystem::FILE_SHARE_MODE(0),
            Some(&attributes),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )?
    };
    // SAFETY: we exclusively own the freshly created handle.
    Ok(unsafe { std::fs::File::from_raw_handle(handle.0) })
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
/// and Administrators controls. It is kept only when it and everything in
/// it (looked at without following any link) belong to them, are no links
/// or extra hard links, and let nobody else write. Anything else, such as
/// a folder a standard user created before installation with files and
/// permissions of their own, or a junction to somewhere else, is moved
/// aside whole (a link itself, never its target) and a new one created.
/// Only the directory's own DACL is changed, through its handle.
pub fn secure_dir(path: &Path, sddl: &str) -> Result<Secured> {
    secure_dir_with(path, sddl, trusted_owner)
}

/// [`secure_dir`] for a folder of programs, which may hold what Windows
/// gives TrustedInstaller under Program Files.
pub fn secure_program_dir(path: &Path, sddl: &str) -> Result<Secured> {
    secure_dir_with(path, sddl, |sid| {
        trusted_owner(sid) || is_trusted_installer(sid)
    })
}

fn secure_dir_with(path: &Path, sddl: &str, trusted: impl Fn(PSID) -> bool) -> Result<Secured> {
    let sd = Descriptor::from_sddl(sddl)?;
    let dacl = sd.dacl()?;
    let wide: Vec<u16> = path.as_os_str().encode_wide_nul();
    let mut moved = None;
    for _ in 0..3 {
        match open(&wide, READ_CONTROL | WRITE_DAC) {
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
            Ok(dir)
                if trustworthy(&dir, &trusted)? == Some(true)
                    && tree_is_ours(path, &trusted, 0)? =>
            {
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

/// Opens `path` itself, a link included (never its target).
fn open(path: &[u16], access: FILE_ACCESS_RIGHTS) -> Result<OwnedHandle> {
    // SAFETY: `path` is NUL-terminated; the handle is owned below.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            access.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )?
    };
    Ok(OwnedHandle::new(handle))
}

/// What is open at `handle`, when it may be trusted: no link, owned by an
/// owner `trusted` accepts, writable by nobody else, and (a file) without a
/// second name elsewhere. `Some(true)` for a directory, `Some(false)` for a
/// file, `None` when it may not be trusted.
fn trustworthy(handle: &OwnedHandle, trusted: &impl Fn(PSID) -> bool) -> Result<Option<bool>> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `handle` is valid; `info` is writable.
    unsafe { GetFileInformationByHandle(handle.raw(), &mut info)? };
    let dir = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || (!dir && info.nNumberOfLinks > 1)
    {
        return Ok(None);
    }
    Ok(guarded(handle, WRITE_RIGHTS, true, trusted)?.then_some(dir))
}

/// Rights that change an object or what is in it.
const WRITE_RIGHTS: u32 = 0x0002 // FILE_WRITE_DATA, FILE_ADD_FILE
    | 0x0004 // FILE_APPEND_DATA, FILE_ADD_SUBDIRECTORY
    | 0x0010 // FILE_WRITE_EA
    | 0x0040 // FILE_DELETE_CHILD
    | 0x0100 // FILE_WRITE_ATTRIBUTES
    | 0x0001_0000 // DELETE
    | 0x0004_0000 // WRITE_DAC
    | 0x0008_0000 // WRITE_OWNER
    | 0x1000_0000 // GENERIC_ALL
    | 0x4000_0000; // GENERIC_WRITE

/// Whether only trusted principals (and the owner, through CREATOR OWNER or
/// OWNER RIGHTS) are allowed any of `rights`; entries that only pass on to
/// what is created inside count when `inherited_only` says so. A missing
/// DACL lets everyone do anything; unusual kinds of allow entries count as
/// untrusted.
///
/// # Safety
/// `dacl` must be null or point to a valid ACL.
unsafe fn only_trusted_may(
    dacl: *const ACL,
    rights: u32,
    inherited_only: bool,
    trusted: &impl Fn(PSID) -> bool,
) -> bool {
    const INHERIT_ONLY: u8 = 0x08;
    const ACCESS_ALLOWED: u8 = 0;
    const DENIED: [u8; 4] = [1, 6, 10, 12];
    if dacl.is_null() {
        return false;
    }
    let mut size = ACL_SIZE_INFORMATION::default();
    // SAFETY: `dacl` is valid; `size` is writable and its size is passed.
    let read = unsafe {
        GetAclInformation(
            dacl,
            (&raw mut size).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    };
    if read.is_err() {
        return false;
    }
    (0..size.AceCount).all(|i| {
        let mut ace = std::ptr::null_mut();
        // SAFETY: `i` is below the ACL's entry count; every entry starts
        // with an ACE_HEADER, and an allow entry is an ACCESS_ALLOWED_ACE
        // whose SID starts at `SidStart`.
        unsafe {
            if GetAce(dacl, i, &mut ace).is_err() {
                return false;
            }
            let header = &*(ace as *const ACE_HEADER);
            if DENIED.contains(&header.AceType)
                || (!inherited_only && header.AceFlags & INHERIT_ONLY != 0)
            {
                return true;
            }
            if header.AceType != ACCESS_ALLOWED {
                return false;
            }
            let allowed = ace as *const ACCESS_ALLOWED_ACE;
            if (*allowed).Mask & rights == 0 {
                return true;
            }
            let sid = PSID((&raw const (*allowed).SidStart).cast_mut().cast());
            trusted(sid)
                || IsWellKnownSid(sid, WinCreatorOwnerSid).as_bool()
                || IsWellKnownSid(sid, WinCreatorOwnerRightsSid).as_bool()
        }
    })
}

/// Whether everything below `dir` may be trusted (see [`trustworthy`]).
/// Only directories already found trustworthy are entered, and nobody else
/// can change what is in them meanwhile.
fn tree_is_ours(dir: &Path, trusted: &impl Fn(PSID) -> bool, depth: u32) -> Result<bool> {
    if depth > 16 {
        return Ok(false);
    }
    for entry in std::fs::read_dir(dir).map_err(io_error)? {
        let path = entry.map_err(io_error)?.path();
        let handle = match open(&path.as_os_str().encode_wide_nul(), READ_CONTROL) {
            Ok(h) => h,
            Err(e) if e.is_access_denied() => return Ok(false),
            Err(e) => return Err(e),
        };
        match trustworthy(&handle, trusted)? {
            None => return Ok(false),
            Some(true) if !tree_is_ours(&path, trusted, depth + 1)? => return Ok(false),
            Some(_) => {}
        }
    }
    Ok(true)
}

fn io_error(e: std::io::Error) -> crate::Error {
    let code = e.raw_os_error().unwrap_or(0) as u32;
    windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code)).into()
}

/// Whether only SYSTEM, Administrators and TrustedInstaller can change the
/// program at `path` or put another in its place: the file and its folder
/// let nobody else write, and no folder above lets anyone else take it,
/// move or remove what is in it, or change who may. A service running as
/// SYSTEM from anywhere else hands SYSTEM to whoever can.
pub fn only_admins_control(path: &Path) -> Result<bool> {
    let trusted = |sid: PSID| trusted_owner(sid) || is_trusted_installer(sid);
    // The program itself: other names for it (Windows' own files have them
    // in WinSxS) share its permissions and need them to be made, so only
    // links are refused.
    let file = open(&path.as_os_str().encode_wide_nul(), READ_CONTROL)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `file` is valid; `info` is writable.
    unsafe { GetFileInformationByHandle(file.raw(), &mut info)? };
    if info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_DIRECTORY.0) != 0
        || !guarded(&file, WRITE_RIGHTS, true, &trusted)?
    {
        return Ok(false);
    }
    // Its folder: nobody else may add a file (a DLL) or change one.
    let Some(folder) = path.parent() else {
        return Ok(false);
    };
    if !only_admins_move(folder)? {
        return Ok(false);
    }
    let handle = open(&folder.as_os_str().encode_wide_nul(), READ_CONTROL)?;
    Ok(trustworthy(&handle, &trusted)? == Some(true))
}

/// Whether nobody but SYSTEM, Administrators and TrustedInstaller can move,
/// remove or take any folder `path` lies in, so that `path` keeps naming
/// what it names. The path must be absolute and plain (no `.` or `..`), and
/// no folder on the way may be a link: the folders checked are then the
/// ones the path goes through.
pub fn only_admins_move(path: &Path) -> Result<bool> {
    use std::path::Component;
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return Ok(false);
    }
    let trusted = |sid: PSID| trusted_owner(sid) || is_trusted_installer(sid);
    const TAKE_OR_MOVE: u32 = 0x0040 // FILE_DELETE_CHILD
        | 0x0001_0000 // DELETE
        | 0x0004_0000 // WRITE_DAC
        | 0x0008_0000 // WRITE_OWNER
        | 0x1000_0000; // GENERIC_ALL
    for above in path.ancestors().skip(1) {
        let handle = open(&above.as_os_str().encode_wide_nul(), READ_CONTROL)?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: `handle` is valid; `info` is writable.
        unsafe { GetFileInformationByHandle(handle.raw(), &mut info)? };
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
            || !guarded(&handle, TAKE_OR_MOVE, false, &trusted)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether what is open at `handle` has an owner `trusted` accepts and lets
/// nobody else have any of `rights` (see [`only_trusted_may`]).
fn guarded(
    handle: &OwnedHandle,
    rights: u32,
    inherited_only: bool,
    trusted: &impl Fn(PSID) -> bool,
) -> Result<bool> {
    let mut owner = PSID::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `handle` is open with READ_CONTROL; `owner` and `dacl` point
    // into `sd`, which is freed below after their last use.
    unsafe {
        GetSecurityInfo(
            handle.raw(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut dacl),
            None,
            Some(&mut sd),
        )
        .ok()?;
        let ok = trusted(owner) && only_trusted_may(dacl, rights, inherited_only, trusted);
        LocalFree(Some(HLOCAL(sd.0)));
        Ok(ok)
    }
}

/// NT SERVICE\TrustedInstaller, which owns much of Windows' own files.
fn is_trusted_installer(sid: PSID) -> bool {
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    const TRUSTED_INSTALLER: &str =
        "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
    let mut text = windows::core::PWSTR::null();
    // SAFETY: `sid` is a valid SID; `text` is freed below.
    unsafe {
        if ConvertSidToStringSidW(sid, &mut text).is_err() {
            return false;
        }
        let same = text.to_string().is_ok_and(|t| t == TRUSTED_INSTALLER);
        LocalFree(Some(HLOCAL(text.0.cast())));
        same
    }
}

fn trusted_owner(owner: PSID) -> bool {
    // SAFETY: `owner` points to a valid SID.
    unsafe {
        IsWellKnownSid(owner, WinLocalSystemSid).as_bool()
            || IsWellKnownSid(owner, WinBuiltinAdministratorsSid).as_bool()
    }
}

/// Renames `path` to `<path>.untrusted-<random>` next to it: a name nobody
/// can guess, and so prepare a link at, in advance.
fn move_aside(path: &Path, wide: &[u16]) -> Result<PathBuf> {
    use std::hash::{BuildHasher, Hasher};
    let mut random = std::collections::hash_map::RandomState::new().build_hasher();
    random.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".untrusted-{:016x}", random.finish()));
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
        let dir = open(&path.as_os_str().encode_wide_nul(), READ_CONTROL).unwrap();
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
    fn a_private_file_has_its_permissions_from_the_start() {
        use std::io::Write;
        let dir = scratch("private");
        let path = dir.join("telegram.toml.new");
        std::fs::write(&path, b"stale").unwrap();
        let mut f = create_private(&path, SDDL).unwrap();
        f.write_all(b"token = \"x\"").unwrap();
        drop(f);
        assert!(protected(&path));
        assert_eq!(std::fs::read(&path).unwrap(), b"token = \"x\"");
        assert!(create_private(&path, "O:BA").is_err(), "no DACL, no file");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn programs_are_controlled_by_administrators_only_where_windows_puts_them() {
        let windows = std::env::var("SystemRoot").unwrap();
        let notepad = Path::new(&windows).join(r"System32\notepad.exe");
        assert!(
            only_admins_control(&notepad).unwrap(),
            "{}",
            notepad.display()
        );
        // A program in the user's temp folder: anyone running as the user
        // could replace it.
        let dir = scratch("program");
        let exe = dir.join("wtm-service.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        assert!(!only_admins_control(&exe).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folders_above_must_be_plain_and_held_by_administrators() {
        let windows = std::env::var("SystemRoot").unwrap();
        assert!(only_admins_move(&Path::new(&windows).join("System32")).unwrap());
        // Relative, or going up and down: not judged.
        assert!(!only_admins_move(Path::new(r"Windows\System32")).unwrap());
        let odd = Path::new(&windows).join(r"..\Windows\System32");
        assert!(!only_admins_move(&odd).unwrap());

        // A folder reached through a link is refused, wherever it leads.
        let dir = scratch("above");
        let link = dir.join("link");
        let made = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&windows)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        assert!(!only_admins_move(&link.join("System32")).unwrap());
        std::fs::remove_dir(&link).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_directory_is_created_secured() {
        let parent = scratch("create");
        let dir = parent.join("data");
        assert_eq!(secure_dir(&dir, SDDL).unwrap(), Secured::Created);
        assert!(protected(&dir));
        let programs = parent.join("programs");
        assert_eq!(
            secure_program_dir(&programs, SDDL).unwrap(),
            Secured::Created
        );
        assert!(protected(&programs));
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

    /// Trusts every owner and writer but Everyone, as the service trusts
    /// SYSTEM and Administrators only.
    fn all_but_everyone(sid: PSID) -> bool {
        use windows::Win32::Security::WinWorldSid;
        // SAFETY: `sid` points to a valid SID.
        !unsafe { IsWellKnownSid(sid, WinWorldSid) }.as_bool()
    }

    #[test]
    fn anything_inside_that_is_not_ours_sets_the_whole_directory_aside() {
        let parent = scratch("inside");
        let target = parent.join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        type Plant = fn(&Path, &Path);
        let plants: [(&str, Plant); 3] = [
            ("writable by everyone", |dir, _| {
                let f = dir.join("logs").join("config.toml");
                std::fs::write(&f, b"x").unwrap();
                set_dacl(&f, "D:P(A;;FA;;;OW)(A;;FA;;;WD)").unwrap();
            }),
            ("a junction", |dir, target| {
                let made = std::process::Command::new("cmd.exe")
                    .args(["/d", "/c", "mklink", "/J"])
                    .arg(dir.join("logs").join("link"))
                    .arg(target)
                    .output()
                    .unwrap();
                assert!(made.status.success(), "{made:?}");
            }),
            ("a second name for a file", |dir, target| {
                let f = target.join("hosts-copy");
                std::fs::write(&f, b"x").unwrap();
                std::fs::hard_link(&f, dir.join("telegram.toml")).unwrap();
            }),
        ];
        for (what, plant) in plants {
            let dir = parent.join("data");
            std::fs::create_dir_all(dir.join("logs")).unwrap();
            std::fs::write(dir.join("logs").join("today.log"), b"x").unwrap();
            // Clean, it is kept.
            let kept = secure_dir_with(&dir, SDDL, all_but_everyone).unwrap();
            assert_eq!(kept, Secured::Kept, "{what}");
            plant(&dir, &target);
            let r = secure_dir_with(&dir, SDDL, all_but_everyone).unwrap();
            let Secured::Replaced(aside) = r else {
                panic!("{what}: {r:?}")
            };
            assert!(!dir.join("logs").exists(), "{what}: a fresh directory");
            assert!(!protected(&target), "{what}: never followed");
            std::fs::remove_dir_all(&aside).unwrap();
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::remove_dir_all(&parent).unwrap();
    }

    #[test]
    fn a_directory_others_may_write_is_not_kept() {
        let dir = scratch("writable");
        set_dacl(&dir, "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)(A;OICI;FW;;;WD)").unwrap();
        let r = secure_dir_with(&dir, SDDL, all_but_everyone).unwrap();
        let Secured::Replaced(aside) = r else {
            panic!("{r:?}")
        };
        std::fs::remove_dir_all(&aside).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
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

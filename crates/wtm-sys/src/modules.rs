//! The modules (DLLs) a process has loaded, and who signed a file. The
//! signature check works offline (no revocation lookups) and accepts catalog
//! signatures, which is how most of Windows' own files are signed.

use std::fs::File;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;

use windows::Win32::Foundation::{
    ERROR_BAD_LENGTH, HANDLE, HWND, TRUST_E_NOSIGNATURE, TRUST_E_PROVIDER_UNKNOWN,
    TRUST_E_SUBJECT_FORM_UNKNOWN,
};
use windows::Win32::Security::Cryptography::Catalog::{
    CATALOG_INFO, CryptCATAdminAcquireContext2, CryptCATAdminCalcHashFromFileHandle2,
    CryptCATAdminEnumCatalogFromHash, CryptCATAdminReleaseCatalogContext,
    CryptCATAdminReleaseContext, CryptCATCatalogInfoFromContext,
};
use windows::Win32::Security::Cryptography::{
    BCRYPT_SHA1_ALGORITHM, BCRYPT_SHA256_ALGORITHM, CERT_NAME_SIMPLE_DISPLAY_TYPE,
    CertGetNameStringW,
};
use windows::Win32::Security::WinTrust::{
    DRIVER_ACTION_VERIFY, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_CATALOG_INFO, WINTRUST_DATA,
    WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_CATALOG,
    WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
    WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain,
    WTHelperProvDataFromStateData, WinVerifyTrust,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE,
    TH32CS_SNAPMODULE32,
};
use windows::core::{HRESULT, PCWSTR};

use crate::handle::OwnedHandle;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Module {
    pub name: String,
    pub path: String,
    /// Bytes mapped into the process.
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Signature {
    Signed {
        signer: Option<String>,
        /// Signed through a Windows catalog rather than inside the file.
        catalog: bool,
    },
    Unsigned,
    /// Signed, but the signature does not hold (tampered, untrusted root).
    Invalid(String),
}

/// How often a module snapshot is retried when the target's loader list is
/// changing underneath it (`ERROR_BAD_LENGTH`).
const SNAPSHOT_ATTEMPTS: usize = 8;

/// Modules mapped into `pid`, started at `create_time` (0: not checked),
/// the executable first. Reads the process's loader data, so only on
/// request. A handle held meanwhile keeps the PID from going to another
/// process; one that started at another time is refused.
pub fn process_modules(pid: u32, create_time: i64) -> crate::Result<Vec<Module>> {
    use crate::control::{Access, ProcessHandle};
    let held = if create_time == 0 {
        None
    } else {
        let handle = ProcessHandle::open(pid, Access::QUERY)?;
        if handle.create_time()? != create_time {
            return Err(crate::Error::Reused { pid });
        }
        Some(handle)
    };
    let snapshot = module_snapshot(pid)?;
    drop(held);
    let mut entry = MODULEENTRY32W {
        dwSize: size_of::<MODULEENTRY32W>() as u32,
        ..Default::default()
    };
    let mut modules = Vec::new();
    // SAFETY: the snapshot handle is open and `entry` has its size filled in.
    let mut more = unsafe { Module32FirstW(snapshot.raw(), &mut entry) }.is_ok();
    while more {
        modules.push(Module {
            name: from_wide(&entry.szModule),
            path: from_wide(&entry.szExePath),
            size: u64::from(entry.modBaseSize),
        });
        // SAFETY: as above; `entry` keeps its size across calls.
        more = unsafe { Module32NextW(snapshot.raw(), &mut entry) }.is_ok();
    }
    Ok(modules)
}

/// A module snapshot of `pid`, covering both 64- and 32-bit modules.
fn module_snapshot(pid: u32) -> crate::Result<OwnedHandle> {
    let mut attempt = 0;
    loop {
        // SAFETY: plain call; the returned handle is owned by the wrapper.
        match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) } {
            Ok(handle) => return Ok(OwnedHandle::new(handle)),
            Err(e)
                if e.code() == ERROR_BAD_LENGTH.to_hresult() && attempt + 1 < SNAPSHOT_ATTEMPTS =>
            {
                attempt += 1;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// The text before the first NUL of a fixed UTF-16 buffer.
fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    crate::utf16_lossy(&buf[..end])
}

/// `s` as a NUL-terminated UTF-16 string.
fn to_wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Who signed the file at `path`, offline.
pub fn signature(path: &str) -> Signature {
    let wide = to_wide(path);
    match verify_embedded(&wide) {
        Verdict::Trusted(signer) => Signature::Signed {
            signer,
            catalog: false,
        },
        Verdict::Failed(code) => Signature::Invalid(describe(code)),
        Verdict::NoSignature => match verify_catalog(path, &wide) {
            Some(Verdict::Trusted(signer)) => Signature::Signed {
                signer,
                catalog: true,
            },
            Some(Verdict::Failed(code)) => Signature::Invalid(describe(code)),
            Some(Verdict::NoSignature) | None => Signature::Unsigned,
        },
    }
}

/// What one WinVerifyTrust call concluded.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Trusted(Option<String>),
    NoSignature,
    Failed(HRESULT),
}

impl Verdict {
    fn from_status(status: HRESULT, signer: impl FnOnce() -> Option<String>) -> Self {
        if status.is_ok() {
            Verdict::Trusted(signer())
        } else if is_missing_signature(status) {
            Verdict::NoSignature
        } else {
            Verdict::Failed(status)
        }
    }
}

/// Statuses that mean "no signature to check" rather than "a bad one".
fn is_missing_signature(status: HRESULT) -> bool {
    status == TRUST_E_NOSIGNATURE
        || status == TRUST_E_SUBJECT_FORM_UNKNOWN
        || status == TRUST_E_PROVIDER_UNKNOWN
}

fn describe(code: HRESULT) -> String {
    let text = code.message();
    let text = text.trim();
    if text.is_empty() {
        format!("error {:#010x}", code.0 as u32)
    } else {
        text.to_owned()
    }
}

/// A WINTRUST_DATA set up for an offline, silent check that keeps its state
/// so the signer can be read afterwards.
fn trust_data(choice: WINTRUST_DATA_0, catalog: bool) -> WINTRUST_DATA {
    WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: if catalog {
            WTD_CHOICE_CATALOG
        } else {
            WTD_CHOICE_FILE
        },
        Anonymous: choice,
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_REVOCATION_CHECK_NONE | WTD_CACHE_ONLY_URL_RETRIEVAL,
        ..Default::default()
    }
}

/// Runs the generic Authenticode policy over `data`, reads the signer while
/// the state is open, then releases the state.
fn run_trust(data: &mut WINTRUST_DATA) -> Verdict {
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: `data` and everything its union points at outlive the call.
    let status = HRESULT(unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            (data as *mut WINTRUST_DATA).cast(),
        )
    });
    let verdict = Verdict::from_status(status, || signer_name(data.hWVTStateData));
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: closing the state the verify call opened; same inputs.
    unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            (data as *mut WINTRUST_DATA).cast(),
        );
    }
    verdict
}

fn verify_embedded(wide: &[u16]) -> Verdict {
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(wide.as_ptr()),
        ..Default::default()
    };
    let mut data = trust_data(WINTRUST_DATA_0 { pFile: &mut file }, false);
    run_trust(&mut data)
}

/// A catalog admin context, released on drop.
struct CatAdmin(isize);

impl Drop for CatAdmin {
    fn drop(&mut self) {
        // SAFETY: the context came from CryptCATAdminAcquireContext2.
        let _ = unsafe { CryptCATAdminReleaseContext(self.0, 0) };
    }
}

/// A catalog found for a hash; released before its admin context.
struct CatInfo<'a> {
    admin: &'a CatAdmin,
    raw: isize,
}

impl Drop for CatInfo<'_> {
    fn drop(&mut self) {
        // SAFETY: `raw` came from CryptCATAdminEnumCatalogFromHash on `admin`.
        let _ = unsafe { CryptCATAdminReleaseCatalogContext(self.admin.0, self.raw, 0) };
    }
}

/// Looks the file's hash up in the system catalogs, trying SHA-256 first and
/// then SHA-1 for older catalogs. `None` when the file cannot be read.
fn verify_catalog(path: &str, wide: &[u16]) -> Option<Verdict> {
    let file = File::open(path).ok()?;
    let handle = HANDLE(file.as_raw_handle());
    let mut last = Verdict::NoSignature;
    for algorithm in [BCRYPT_SHA256_ALGORITHM, BCRYPT_SHA1_ALGORITHM] {
        match verify_catalog_with(algorithm, handle, wide) {
            Some(v @ Verdict::Trusted(_)) => return Some(v),
            Some(v) => last = v,
            None => {}
        }
    }
    Some(last)
}

fn verify_catalog_with(algorithm: PCWSTR, file: HANDLE, wide: &[u16]) -> Option<Verdict> {
    let mut raw = 0isize;
    // SAFETY: out-pointer is valid; the subsystem GUID and algorithm name are
    // static.
    unsafe {
        CryptCATAdminAcquireContext2(&mut raw, Some(&DRIVER_ACTION_VERIFY), algorithm, None, None)
    }
    .ok()?;
    let admin = CatAdmin(raw);

    let mut hash = file_hash(&admin, file)?;
    // SAFETY: the admin context is live and `hash` is the file's digest.
    let found = unsafe { CryptCATAdminEnumCatalogFromHash(admin.0, &hash, None, None) };
    if found == 0 {
        return None;
    }
    let info = CatInfo {
        admin: &admin,
        raw: found,
    };
    let mut catalog = CATALOG_INFO {
        cbStruct: size_of::<CATALOG_INFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info.raw` is a live catalog context; `catalog` is sized.
    unsafe { CryptCATCatalogInfoFromContext(info.raw, &mut catalog, 0) }.ok()?;

    let tag = to_wide(&hex_upper(&hash));
    let mut member = WINTRUST_CATALOG_INFO {
        cbStruct: size_of::<WINTRUST_CATALOG_INFO>() as u32,
        pcwszCatalogFilePath: PCWSTR(catalog.wszCatalogFile.as_ptr()),
        pcwszMemberTag: PCWSTR(tag.as_ptr()),
        pcwszMemberFilePath: PCWSTR(wide.as_ptr()),
        hMemberFile: file,
        pbCalculatedFileHash: hash.as_mut_ptr(),
        cbCalculatedFileHash: hash.len() as u32,
        hCatAdmin: admin.0,
        ..Default::default()
    };
    let mut data = trust_data(
        WINTRUST_DATA_0 {
            pCatalog: &mut member,
        },
        true,
    );
    let verdict = run_trust(&mut data);
    drop(info);
    Some(verdict)
}

/// The file's digest under the admin context's hash algorithm.
fn file_hash(admin: &CatAdmin, file: HANDLE) -> Option<Vec<u8>> {
    let mut len = 0u32;
    // SAFETY: a size query; no buffer is written.
    unsafe { CryptCATAdminCalcHashFromFileHandle2(admin.0, file, &mut len, None, None) }.ok()?;
    if len == 0 {
        return None;
    }
    let mut hash = vec![0u8; len as usize];
    // SAFETY: `hash` holds `len` bytes, as the size query asked for.
    unsafe {
        CryptCATAdminCalcHashFromFileHandle2(admin.0, file, &mut len, Some(hash.as_mut_ptr()), None)
    }
    .ok()?;
    hash.truncate(len as usize);
    Some(hash)
}

fn hex_upper(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02X}");
        out
    })
}

/// The display name on the leaf certificate of the first signer.
fn signer_name(state: HANDLE) -> Option<String> {
    if state.is_invalid() {
        return None;
    }
    // SAFETY: `state` is the open WinVerifyTrust state; every pointer read
    // below is checked for null and lives until the state is closed.
    unsafe {
        let provider = WTHelperProvDataFromStateData(state);
        if provider.is_null() {
            return None;
        }
        let signer = WTHelperGetProvSignerFromChain(provider, 0, false, 0);
        if signer.is_null() || (*signer).csCertChain == 0 || (*signer).pasCertChain.is_null() {
            return None;
        }
        let cert = (*(*signer).pasCertChain).pCert;
        if cert.is_null() {
            return None;
        }
        let len = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, None);
        if len <= 1 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, Some(&mut buf));
        let name = from_wide(&buf);
        (!name.is_empty()).then_some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system32(file: &str) -> String {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        format!(r"{root}\System32\{file}")
    }

    #[test]
    fn wide_round_trip_stops_at_nul() {
        let wide = to_wide("ab");
        assert_eq!(wide, [b'a' as u16, b'b' as u16, 0]);
        assert_eq!(from_wide(&[b'x' as u16, 0, b'y' as u16]), "x");
        assert_eq!(from_wide(&[b'z' as u16; 3]), "zzz");
    }

    #[test]
    fn member_tag_is_upper_hex() {
        assert_eq!(hex_upper(&[0x00, 0xab, 0x7f]), "00AB7F");
        assert_eq!(hex_upper(&[]), "");
    }

    #[test]
    fn missing_signature_is_not_a_failure() {
        assert_eq!(
            Verdict::from_status(TRUST_E_NOSIGNATURE, || None),
            Verdict::NoSignature
        );
        assert_eq!(
            Verdict::from_status(HRESULT(0), || Some("Me".into())),
            Verdict::Trusted(Some("Me".into()))
        );
        let bad = HRESULT(0x8009_6010_u32 as i32); // TRUST_E_BAD_DIGEST
        assert_eq!(Verdict::from_status(bad, || None), Verdict::Failed(bad));
        assert!(!describe(bad).is_empty());
    }

    #[test]
    fn own_process_lists_its_executable_first() {
        let me = std::process::id();
        let started = crate::control::ProcessHandle::open(me, crate::control::Access::QUERY)
            .unwrap()
            .create_time()
            .unwrap();
        assert!(matches!(
            process_modules(me, started + 1),
            Err(crate::Error::Reused { .. })
        ));
        let modules = process_modules(me, started).unwrap();
        assert!(modules.len() > 1);
        let exe = std::env::current_exe().unwrap();
        assert!(modules[0].path.eq_ignore_ascii_case(&exe.to_string_lossy()));
        assert!(modules.iter().all(|m| m.size > 0 && !m.name.is_empty()));
        assert!(
            modules
                .iter()
                .any(|m| m.name.eq_ignore_ascii_case("ntdll.dll"))
        );
    }

    #[test]
    fn windows_files_are_signed() {
        // Whether embedded or through a catalog, both carry Microsoft's name.
        for file in ["kernel32.dll", "notepad.exe"] {
            let path = system32(file);
            if !std::path::Path::new(&path).exists() {
                continue;
            }
            match signature(&path) {
                Signature::Signed { signer, .. } => {
                    assert!(signer.is_some_and(|s| s.contains("Microsoft")), "{file}");
                }
                other => panic!("{file}: {other:?}"),
            }
        }
    }

    #[test]
    fn catalog_signed_windows_files() {
        // ARP.EXE carries no signature of its own; Windows signs it through
        // a catalog.
        let path = system32("ARP.EXE");
        if !std::path::Path::new(&path).exists() {
            return;
        }
        match signature(&path) {
            Signature::Signed { signer, catalog } => {
                assert!(catalog);
                assert!(signer.is_some_and(|s| s.contains("Microsoft")));
            }
            other => panic!("ARP.EXE: {other:?}"),
        }
    }

    #[test]
    fn unsigned_and_missing_files() {
        let dir = std::env::temp_dir().join(format!("wtm-sig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plain = dir.join("plain.txt");
        std::fs::write(&plain, b"not signed").unwrap();
        assert_eq!(signature(&plain.to_string_lossy()), Signature::Unsigned);
        assert!(!matches!(
            signature(&dir.join("absent.dll").to_string_lossy()),
            Signature::Signed { .. }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

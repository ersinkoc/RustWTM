use windows::Win32::Foundation::{ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_DEBUG_NAME,
    SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::Result;
use crate::handle::OwnedHandle;

/// Enables `SeDebugPrivilege` for the current process.
///
/// Returns `Ok(false)` when the token does not hold the privilege
/// (i.e. the process is not elevated).
pub fn enable_debug_privilege() -> Result<bool> {
    let mut token = HANDLE::default();
    // SAFETY: GetCurrentProcess returns a pseudo-handle; `token` is writable.
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )?
    };
    let token = OwnedHandle::new(token);

    let mut luid = LUID::default();
    // SAFETY: SE_DEBUG_NAME is a static wide string; `luid` is writable.
    unsafe { LookupPrivilegeValueW(None, SE_DEBUG_NAME, &mut luid)? };

    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    // SAFETY: `privileges` is a valid TOKEN_PRIVILEGES with one entry.
    unsafe { AdjustTokenPrivileges(token.raw(), false, Some(&privileges), 0, None, None)? };
    // AdjustTokenPrivileges succeeds even if nothing was assigned.
    // SAFETY: reads the calling thread's last-error value.
    Ok(unsafe { GetLastError() } != ERROR_NOT_ALL_ASSIGNED)
}

#[cfg(test)]
mod tests {
    #[test]
    fn enabling_debug_privilege_does_not_fail() {
        // Ok(true) when elevated, Ok(false) otherwise.
        super::enable_debug_privilege().unwrap();
    }
}

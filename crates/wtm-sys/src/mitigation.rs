//! A process's security settings as Task Manager's Details tab shows them:
//! UAC virtualization, Data Execution Prevention and hardware-enforced
//! stack protection (CET shadow stacks).
//!
//! UAC virtualization needs only query rights on the process. DEP and
//! stack protection come from `GetProcessMitigationPolicy`, which needs
//! `PROCESS_QUERY_INFORMATION` ([`crate::control::Access::QUERY_INFORMATION`]); without it
//! they fail, which callers show as unknown rather than as "Disabled".

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{
    GetTokenInformation, TOKEN_QUERY, TokenVirtualizationAllowed, TokenVirtualizationEnabled,
};
use windows::Win32::System::SystemInformation::{IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_UNKNOWN};
use windows::Win32::System::Threading::{
    GetProcessMitigationPolicy, IsWow64Process2, OpenProcessToken, PROCESS_MITIGATION_POLICY,
    ProcessDEPPolicy, ProcessUserShadowStackPolicy,
};

use crate::Result;
use crate::control::ProcessHandle;
use crate::handle::OwnedHandle;

/// Task Manager's "UAC virtualization": whether file and registry writes
/// the process may not make are redirected to per-user copies.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum UacVirtualization {
    /// The token does not allow it, as for services and elevated or
    /// manifested programs.
    NotAllowed,
    Disabled,
    Enabled,
}

impl UacVirtualization {
    /// From the token's `TokenVirtualizationAllowed` and
    /// `TokenVirtualizationEnabled` values.
    pub fn decode(allowed: u32, enabled: u32) -> Self {
        match (allowed != 0, enabled != 0) {
            (false, _) => Self::NotAllowed,
            (true, false) => Self::Disabled,
            (true, true) => Self::Enabled,
        }
    }
}

/// Task Manager's "Hardware-enforced Stack Protection".
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum StackProtection {
    Disabled,
    /// Enforced for modules marked compatible with shadow stacks.
    CompatibleModules {
        audit: bool,
    },
    /// Enforced for all modules (strict mode).
    AllModules {
        audit: bool,
    },
}

impl StackProtection {
    /// From `PROCESS_MITIGATION_USER_SHADOW_STACK_POLICY.Flags`: bit 0
    /// EnableUserShadowStack, bit 1 AuditUserShadowStack, bit 4
    /// EnableUserShadowStackStrictMode.
    pub fn decode(flags: u32) -> Self {
        let enabled = flags & 1 != 0;
        let audit = flags & 2 != 0;
        let strict = flags & 0x10 != 0;
        if !enabled && !audit {
            return Self::Disabled;
        }
        // Audit mode reports what enforcing would block, without blocking.
        let audit = !enabled;
        if strict {
            Self::AllModules { audit }
        } else {
            Self::CompatibleModules { audit }
        }
    }
}

/// The Enable bit of `PROCESS_MITIGATION_DEP_POLICY.Flags`.
pub fn dep_enabled_from(flags: u32) -> bool {
    flags & 1 != 0
}

impl ProcessHandle {
    /// UAC virtualization of the process's token. Needs `Access::QUERY`.
    pub fn uac_virtualization(&self) -> Result<UacVirtualization> {
        let mut raw = HANDLE::default();
        // SAFETY: `raw` receives a token handle that OwnedHandle closes.
        unsafe { OpenProcessToken(self.raw(), TOKEN_QUERY, &mut raw)? };
        let token = OwnedHandle::new(raw);
        let read = |class| -> Result<u32> {
            let mut value = 0u32;
            let mut len = 0u32;
            // SAFETY: `value` is a writable DWORD of the size passed.
            unsafe {
                GetTokenInformation(
                    token.raw(),
                    class,
                    Some(std::ptr::from_mut(&mut value).cast()),
                    size_of::<u32>() as u32,
                    &mut len,
                )?
            };
            Ok(value)
        };
        Ok(UacVirtualization::decode(
            read(TokenVirtualizationAllowed)?,
            read(TokenVirtualizationEnabled)?,
        ))
    }

    /// Whether Data Execution Prevention applies. Always on for native
    /// 64-bit processes; a 32-bit (WOW64) process's policy is read, which
    /// needs `Access::QUERY_INFORMATION`.
    pub fn dep_enabled(&self) -> Result<bool> {
        let mut process = IMAGE_FILE_MACHINE::default();
        // SAFETY: the out-pointer references a live value.
        unsafe { IsWow64Process2(self.raw(), &mut process, None)? };
        if process == IMAGE_FILE_MACHINE_UNKNOWN {
            return Ok(true);
        }
        // PROCESS_MITIGATION_DEP_POLICY: a DWORD of flags, then the
        // BOOLEAN Permanent, padded to 8 bytes.
        let mut policy = [0u32; 2];
        self.mitigation(ProcessDEPPolicy, &mut policy)?;
        Ok(dep_enabled_from(policy[0]))
    }

    /// Hardware-enforced stack protection. Needs
    /// `Access::QUERY_INFORMATION`; fails before Windows 10 2004.
    pub fn stack_protection(&self) -> Result<StackProtection> {
        // PROCESS_MITIGATION_USER_SHADOW_STACK_POLICY: one DWORD of flags.
        let mut policy = [0u32; 1];
        self.mitigation(ProcessUserShadowStackPolicy, &mut policy)?;
        Ok(StackProtection::decode(policy[0]))
    }

    fn mitigation(&self, policy: PROCESS_MITIGATION_POLICY, out: &mut [u32]) -> Result<()> {
        // SAFETY: `out` is writable for the length passed, which is the
        // size of the policy's structure.
        unsafe {
            GetProcessMitigationPolicy(
                self.raw(),
                policy,
                out.as_mut_ptr().cast(),
                std::mem::size_of_val(out),
            )?
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Access;

    #[test]
    fn uac_virtualization_reads_like_task_manager() {
        assert_eq!(
            UacVirtualization::decode(0, 0),
            UacVirtualization::NotAllowed
        );
        assert_eq!(
            UacVirtualization::decode(0, 1),
            UacVirtualization::NotAllowed
        );
        assert_eq!(UacVirtualization::decode(1, 0), UacVirtualization::Disabled);
        assert_eq!(UacVirtualization::decode(1, 1), UacVirtualization::Enabled);
    }

    #[test]
    fn stack_protection_follows_the_policy_bits() {
        use StackProtection::*;
        assert_eq!(StackProtection::decode(0), Disabled);
        assert_eq!(
            StackProtection::decode(1),
            CompatibleModules { audit: false }
        );
        assert_eq!(
            StackProtection::decode(1 | 0x10),
            AllModules { audit: false }
        );
        assert_eq!(
            StackProtection::decode(2),
            CompatibleModules { audit: true }
        );
        assert_eq!(
            StackProtection::decode(2 | 0x10),
            AllModules { audit: true }
        );
        // Other mitigations alone (context IP validation) are not shadow stacks.
        assert_eq!(StackProtection::decode(4), Disabled);
    }

    #[test]
    fn dep_reads_the_enable_bit() {
        assert!(dep_enabled_from(1));
        assert!(dep_enabled_from(3));
        assert!(!dep_enabled_from(2));
    }

    #[test]
    fn own_process_reports_its_settings() {
        let me = ProcessHandle::open(std::process::id(), Access::QUERY_INFORMATION).unwrap();
        assert!(me.uac_virtualization().is_ok());
        // Test binaries are native 64-bit, where DEP always applies.
        #[cfg(target_pointer_width = "64")]
        assert!(me.dep_enabled().unwrap());
        let _ = me.stack_protection();
    }

    #[test]
    fn a_32_bit_process_reports_its_dep_policy() {
        // A 32-bit (WOW64) process is the one case whose policy is read.
        let cmd = std::path::Path::new(r"C:\Windows\SysWOW64\cmd.exe");
        if !cmd.exists() {
            return;
        }
        // `pause` waits on the open pipe until the child is ended.
        let mut child = std::process::Command::new(cmd)
            .args(["/d", "/c", "pause"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let h = ProcessHandle::open(child.id(), Access::QUERY_INFORMATION).unwrap();
        let dep = h.dep_enabled();
        let _ = child.kill();
        let _ = child.wait();
        // Windows 8 and later turn DEP on for every 32-bit process unless
        // its image or policy opts out; cmd.exe does not.
        assert!(dep.unwrap());
    }
}

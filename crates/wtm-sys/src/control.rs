//! Per-process control: terminate, suspend, priority, affinity, EcoQoS,
//! memory dumps.

use std::ops::BitOr;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::time::Duration;

use windows::Win32::Foundation::{
    FILETIME, HANDLE, NTSTATUS, UNICODE_STRING, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Security::{
    GetTokenInformation, LookupAccountSidW, SID_NAME_USE, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::System::Diagnostics::Debug::{
    MINIDUMP_TYPE, MiniDumpWithFullMemory, MiniDumpWithFullMemoryInfo, MiniDumpWithHandleData,
    MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules, MiniDumpWriteDump,
};
use windows::Win32::System::Memory::{
    SETPROCESSWORKINGSETSIZEEX_FLAGS, SetProcessWorkingSetSizeEx,
};
use windows::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetPriorityClass,
    GetProcessAffinityMask, GetProcessInformation, GetProcessTimes, HIGH_PRIORITY_CLASS,
    IDLE_PRIORITY_CLASS, IsProcessCritical, NORMAL_PRIORITY_CLASS, OpenProcess, OpenProcessToken,
    PROCESS_ACCESS_RIGHTS, PROCESS_CREATION_FLAGS, PROCESS_NAME_WIN32,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SET_INFORMATION, PROCESS_SET_QUOTA, PROCESS_SUSPEND_RESUME, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE, PROCESS_VM_READ, ProcessPowerThrottling, QueryFullProcessImageNameW,
    REALTIME_PRIORITY_CLASS, SetPriorityClass, SetProcessAffinityMask, SetProcessInformation,
    TerminateProcess, WaitForSingleObject,
};
use windows::core::PWSTR;

use crate::handle::OwnedHandle;
use crate::{Error, Result};

windows::core::link!("ntdll.dll" "system" fn NtSuspendProcess(process_handle: HANDLE) -> NTSTATUS);
windows::core::link!("ntdll.dll" "system" fn NtResumeProcess(process_handle: HANDLE) -> NTSTATUS);
windows::core::link!("ntdll.dll" "system" fn NtQueryInformationProcess(
    process_handle: HANDLE,
    class: u32,
    info: *mut core::ffi::c_void,
    len: u32,
    returned: *mut u32,
) -> NTSTATUS);

/// `PROCESSINFOCLASS::ProcessCommandLineInformation` (Windows 8.1+).
const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;

/// Access rights requested when opening a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access(PROCESS_ACCESS_RIGHTS);

impl Access {
    pub const QUERY: Self = Self(PROCESS_QUERY_LIMITED_INFORMATION);
    pub const TERMINATE: Self = Self(PROCESS_TERMINATE);
    pub const SUSPEND_RESUME: Self = Self(PROCESS_SUSPEND_RESUME);
    pub const SET_INFORMATION: Self = Self(PROCESS_SET_INFORMATION);
    /// Required (with TERMINATE) to assign the process to a job object.
    pub const SET_QUOTA: Self = Self(PROCESS_SET_QUOTA);
    pub const SYNCHRONIZE: Self = Self(PROCESS_SYNCHRONIZE);
    /// Required to write a memory dump.
    pub const DUMP: Self = Self(PROCESS_ACCESS_RIGHTS(
        PROCESS_QUERY_INFORMATION.0 | PROCESS_VM_READ.0,
    ));
}

impl BitOr for Access {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PriorityClass {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
    Realtime,
}

impl PriorityClass {
    fn to_raw(self) -> PROCESS_CREATION_FLAGS {
        match self {
            Self::Idle => IDLE_PRIORITY_CLASS,
            Self::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
            Self::Normal => NORMAL_PRIORITY_CLASS,
            Self::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
            Self::High => HIGH_PRIORITY_CLASS,
            Self::Realtime => REALTIME_PRIORITY_CLASS,
        }
    }

    fn from_raw(raw: u32) -> Option<Self> {
        [
            Self::Idle,
            Self::BelowNormal,
            Self::Normal,
            Self::AboveNormal,
            Self::High,
            Self::Realtime,
        ]
        .into_iter()
        .find(|p| p.to_raw().0 == raw)
    }
}

/// Power throttling (EcoQoS) state of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EcoQos {
    /// Windows decides (the default for every process).
    SystemManaged,
    /// Always throttled — "Efficiency mode".
    On,
    /// Explicitly opted out of throttling.
    Off,
}

/// An open process handle.
#[derive(Debug)]
pub struct ProcessHandle {
    handle: OwnedHandle,
    pid: u32,
}

impl ProcessHandle {
    pub fn open(pid: u32, access: Access) -> Result<Self> {
        // SAFETY: plain FFI call; the returned handle is owned by us.
        let raw = unsafe { OpenProcess(access.0, false, pid)? };
        Ok(Self {
            handle: OwnedHandle::new(raw),
            pid,
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub(crate) fn raw(&self) -> HANDLE {
        self.handle.raw()
    }

    /// Creation time as a FILETIME value; identifies the process across PID reuse.
    pub fn create_time(&self) -> Result<i64> {
        let mut create = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: all out-pointers reference live FILETIMEs.
        unsafe { GetProcessTimes(self.raw(), &mut create, &mut exit, &mut kernel, &mut user)? };
        Ok(((u64::from(create.dwHighDateTime) << 32) | u64::from(create.dwLowDateTime)) as i64)
    }

    pub fn image_path(&self) -> Result<String> {
        let mut buf = vec![0u16; 32 * 1024];
        let mut len = buf.len() as u32;
        // SAFETY: `buf` holds `len` u16s; on success `len` is the written length.
        unsafe {
            QueryFullProcessImageNameW(
                self.raw(),
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            )?
        };
        Ok(String::from_utf16_lossy(&buf[..len as usize]))
    }

    /// The command line the process was started with.
    pub fn command_line(&self) -> Result<String> {
        let mut len = 0u32;
        // SAFETY: a zero-length query only reports the required size.
        unsafe {
            let _ = NtQueryInformationProcess(
                self.raw(),
                PROCESS_COMMAND_LINE_INFORMATION,
                std::ptr::null_mut(),
                0,
                &mut len,
            );
        }
        // u64 elements keep the UNICODE_STRING header aligned.
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(2)];
        let size = (buf.len() * 8) as u32;
        // SAFETY: uf is writable for size bytes.
        let status = unsafe {
            NtQueryInformationProcess(
                self.raw(),
                PROCESS_COMMAND_LINE_INFORMATION,
                buf.as_mut_ptr().cast(),
                size,
                &mut len,
            )
        };
        nt_ok("NtQueryInformationProcess", status)?;
        // SAFETY: on success the buffer starts with a UNICODE_STRING whose
        // text lies inside the same buffer.
        let text = unsafe {
            let s = &*buf.as_ptr().cast::<UNICODE_STRING>();
            if s.Length == 0 || s.Buffer.is_null() {
                &[][..]
            } else {
                std::slice::from_raw_parts(s.Buffer.0, usize::from(s.Length) / 2)
            }
        };
        Ok(String::from_utf16_lossy(text))
    }

    /// The account the process runs as, e.g. NT AUTHORITY\SYSTEM.
    pub fn user(&self) -> Result<String> {
        let mut raw = HANDLE::default();
        // SAFETY: aw receives a token handle that OwnedHandle closes.
        unsafe { OpenProcessToken(self.raw(), TOKEN_QUERY, &mut raw)? };
        let token = OwnedHandle::new(raw);
        let mut len = 0u32;
        // SAFETY: a null buffer only reports the required size.
        let _ = unsafe { GetTokenInformation(token.raw(), TokenUser, None, 0, &mut len) };
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
        // SAFETY: uf is writable for len bytes and suitably aligned.
        unsafe {
            GetTokenInformation(
                token.raw(),
                TokenUser,
                Some(buf.as_mut_ptr().cast()),
                len,
                &mut len,
            )?
        };
        // SAFETY: on success the buffer holds a TOKEN_USER.
        let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut name = [0u16; 256];
        let mut domain = [0u16; 256];
        let (mut name_len, mut domain_len) = (name.len() as u32, domain.len() as u32);
        let mut kind = SID_NAME_USE::default();
        // SAFETY: both buffers are writable for the lengths passed; sid
        // points into uf, which outlives the call.
        unsafe {
            LookupAccountSidW(
                None,
                sid,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                Some(PWSTR(domain.as_mut_ptr())),
                &mut domain_len,
                &mut kind,
            )?
        };
        let name = String::from_utf16_lossy(&name[..name_len as usize]);
        Ok(match domain_len {
            0 => name,
            n => format!(
                "{}\\{name}",
                String::from_utf16_lossy(&domain[..n as usize])
            ),
        })
    }

    /// Moves as much of the working set as possible to the standby list.
    /// Pages come back on demand; nothing is lost. Requires `Access::SET_QUOTA`.
    pub fn trim_working_set(&self) -> Result<()> {
        // SAFETY: valid handle; (-1, -1) means "remove as many pages as possible".
        unsafe {
            SetProcessWorkingSetSizeEx(
                self.raw(),
                usize::MAX,
                usize::MAX,
                SETPROCESSWORKINGSETSIZEEX_FLAGS(0),
            )?
        };
        Ok(())
    }

    /// Whether terminating this process would bug-check the system.
    pub fn is_critical(&self) -> Result<bool> {
        let mut critical = windows::core::BOOL::default();
        // SAFETY: `critical` is a writable BOOL.
        unsafe { IsProcessCritical(self.raw(), &mut critical)? };
        Ok(critical.as_bool())
    }

    /// Requires `Access::SYNCHRONIZE`.
    pub fn wait_exit(&self, timeout: Duration) -> Result<bool> {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: valid handle owned by `self`.
        match unsafe { WaitForSingleObject(self.raw(), ms) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(windows::core::Error::from_thread().into()),
        }
    }

    /// Writes a full memory dump (like Task Manager's "Create memory dump
    /// file") to `path`, replacing it. Requires `Access::DUMP`.
    pub fn write_dump(&self, path: &Path) -> Result<()> {
        let file = std::fs::File::create(path).map_err(|e| {
            let code = e.raw_os_error().unwrap_or(0) as u32;
            Error::Win32(windows::core::HRESULT::from_win32(code).into())
        })?;
        let kind = MINIDUMP_TYPE(
            MiniDumpWithFullMemory.0
                | MiniDumpWithFullMemoryInfo.0
                | MiniDumpWithHandleData.0
                | MiniDumpWithThreadInfo.0
                | MiniDumpWithUnloadedModules.0,
        );
        // SAFETY: valid process handle; `file` is open for writing and
        // outlives the call.
        let result = unsafe {
            MiniDumpWriteDump(
                self.raw(),
                self.pid,
                HANDLE(file.as_raw_handle()),
                kind,
                None,
                None,
                None,
            )
        };
        drop(file);
        if let Err(e) = result {
            let _ = std::fs::remove_file(path);
            return Err(e.into());
        }
        Ok(())
    }

    pub fn terminate(&self, exit_code: u32) -> Result<()> {
        // SAFETY: valid handle owned by `self`.
        unsafe { TerminateProcess(self.raw(), exit_code)? };
        Ok(())
    }

    pub fn suspend(&self) -> Result<()> {
        // SAFETY: valid handle owned by `self`.
        nt_ok("NtSuspendProcess", unsafe { NtSuspendProcess(self.raw()) })
    }

    pub fn resume(&self) -> Result<()> {
        // SAFETY: valid handle owned by `self`.
        nt_ok("NtResumeProcess", unsafe { NtResumeProcess(self.raw()) })
    }

    pub fn priority(&self) -> Result<PriorityClass> {
        // SAFETY: valid handle owned by `self`.
        let raw = unsafe { GetPriorityClass(self.raw()) };
        if raw == 0 {
            return Err(windows::core::Error::from_thread().into());
        }
        PriorityClass::from_raw(raw).ok_or(Error::Nt {
            call: "GetPriorityClass",
            status: raw,
        })
    }

    pub fn set_priority(&self, priority: PriorityClass) -> Result<()> {
        // SAFETY: valid handle owned by `self`.
        unsafe { SetPriorityClass(self.raw(), priority.to_raw())? };
        Ok(())
    }

    /// Returns `(process_mask, system_mask)` for the process's processor group.
    pub fn affinity(&self) -> Result<(u64, u64)> {
        let mut process = 0usize;
        let mut system = 0usize;
        // SAFETY: both out-pointers reference live usizes.
        unsafe { GetProcessAffinityMask(self.raw(), &mut process, &mut system)? };
        Ok((process as u64, system as u64))
    }

    pub fn set_affinity(&self, mask: u64) -> Result<()> {
        // SAFETY: valid handle owned by `self`.
        unsafe { SetProcessAffinityMask(self.raw(), mask as usize)? };
        Ok(())
    }

    pub fn core_affinity(
        &self,
        selection: crate::cpu::CoreSelection,
    ) -> std::result::Result<u64, String> {
        let mut groups = vec![0u16; 64];
        let mut count = 64u16;
        // SAFETY: the group array is writable for count elements; handle is live.
        unsafe {
            windows::Win32::System::Threading::GetProcessGroupAffinity(
                self.raw(),
                &mut count,
                groups.as_mut_ptr(),
            )
            .ok()
        }
        .map_err(|e| e.to_string())?;
        if count != 1 {
            return Err("Core-type affinity requires a process in one processor group; cross-group masks are not supported".into());
        }
        let topology = crate::cpu::query(Some(self.raw())).map_err(|e| e.to_string())?;
        let (_, allowed) = self.affinity().map_err(|e| e.to_string())?;
        topology.mask(selection, groups[0], allowed)
    }

    pub fn eco_qos(&self) -> Result<EcoQos> {
        let mut state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ..Default::default()
        };
        // SAFETY: `state` is a correctly sized, versioned structure.
        unsafe {
            GetProcessInformation(
                self.raw(),
                ProcessPowerThrottling,
                (&raw mut state).cast(),
                size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            )?
        };
        let bit = PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
        Ok(
            match (state.ControlMask & bit != 0, state.StateMask & bit != 0) {
                (false, _) => EcoQos::SystemManaged,
                (true, true) => EcoQos::On,
                (true, false) => EcoQos::Off,
            },
        )
    }

    pub fn set_eco_qos(&self, mode: EcoQos) -> Result<()> {
        let bit = PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
        let (control, state) = match mode {
            EcoQos::SystemManaged => (0, 0),
            EcoQos::On => (bit, bit),
            EcoQos::Off => (bit, 0),
        };
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: control,
            StateMask: state,
        };
        // SAFETY: `state` is a correctly sized, versioned structure.
        unsafe {
            SetProcessInformation(
                self.raw(),
                ProcessPowerThrottling,
                (&raw const state).cast(),
                size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            )?
        };
        Ok(())
    }
}

fn nt_ok(call: &'static str, status: NTSTATUS) -> Result<()> {
    if status.is_ok() {
        Ok(())
    } else {
        Err(Error::Nt {
            call,
            status: status.0 as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};

    /// Kills the child if a test fails half-way.
    struct ChildGuard(Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn spawn_ping() -> ChildGuard {
        let child = Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn ping");
        ChildGuard(child)
    }

    #[test]
    fn trims_the_working_set_of_a_child() {
        let child = spawn_ping();
        let h = ProcessHandle::open(child.0.id(), Access::QUERY | Access::SET_QUOTA).unwrap();
        h.trim_working_set().unwrap();
        let weak = ProcessHandle::open(child.0.id(), Access::QUERY).unwrap();
        assert!(weak.trim_working_set().unwrap_err().is_access_denied());
    }

    #[test]
    fn writes_a_memory_dump_of_a_child() {
        let child = spawn_ping();
        let path = std::env::temp_dir().join(format!("wtm-dump-test-{}.dmp", std::process::id()));
        let h = ProcessHandle::open(child.0.id(), Access::DUMP).unwrap();
        h.write_dump(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(&bytes[..4], b"MDMP");
        assert!(
            bytes.len() > 100_000,
            "a full dump holds the memory: {}",
            bytes.len()
        );

        // A file that cannot be created fails before anything is dumped.
        let bad = std::env::temp_dir().join("no-such-dir-wtm").join("x.dmp");
        assert!(h.write_dump(&bad).is_err());
    }

    #[test]
    fn reads_command_line_and_user() {
        let child = spawn_ping();
        let h = ProcessHandle::open(child.0.id(), Access::QUERY).unwrap();
        let cmd = h.command_line().unwrap();
        assert!(cmd.to_lowercase().contains("ping"), "{cmd}");
        assert!(cmd.ends_with("-n 60 127.0.0.1"), "{cmd}");
        let user = h.user().unwrap();
        assert!(user.contains('\\') && user.len() > 2, "{user}");

        // A handle without query rights is refused, not misread.
        let h = ProcessHandle::open(child.0.id(), Access::SYNCHRONIZE).unwrap();
        assert!(h.command_line().unwrap_err().is_access_denied());
        assert!(h.user().unwrap_err().is_access_denied());
    }

    #[test]
    fn priority_classes_round_trip() {
        for p in [
            PriorityClass::Idle,
            PriorityClass::BelowNormal,
            PriorityClass::Normal,
            PriorityClass::AboveNormal,
            PriorityClass::High,
            PriorityClass::Realtime,
        ] {
            assert_eq!(PriorityClass::from_raw(p.to_raw().0), Some(p));
        }
        assert_eq!(PriorityClass::from_raw(0), None);
    }

    #[test]
    fn controls_spawned_child_and_waits_for_exit() {
        let child = spawn_ping();
        let pid = child.0.id();
        let access = Access::QUERY | Access::SET_INFORMATION | Access::SYNCHRONIZE;
        let h = ProcessHandle::open(pid, access | Access::TERMINATE).unwrap();
        assert_eq!(h.pid(), pid);

        for p in [
            PriorityClass::AboveNormal,
            PriorityClass::High,
            PriorityClass::Normal,
        ] {
            h.set_priority(p).unwrap();
            assert_eq!(h.priority().unwrap(), p);
        }
        h.set_eco_qos(EcoQos::Off).unwrap();
        assert_eq!(h.eco_qos().unwrap(), EcoQos::Off);

        assert!(!h.wait_exit(Duration::ZERO).unwrap(), "still running");
        h.terminate(3).unwrap();
        assert!(h.wait_exit(Duration::from_secs(10)).unwrap());
    }

    #[test]
    fn calls_without_the_needed_access_fail() {
        let child = spawn_ping();
        let pid = child.0.id();
        // The handle carries only the requested rights, even when elevated
        // with SeDebugPrivilege, so every call below is refused.
        let h = ProcessHandle::open(pid, Access::SYNCHRONIZE).unwrap();
        assert!(h.image_path().is_err());
        assert!(h.priority().is_err());
        assert!(h.eco_qos().is_err());
        assert!(h.set_eco_qos(EcoQos::On).is_err());
        let e = h.suspend().unwrap_err();
        assert!(e.is_access_denied(), "{e}");
        assert!(e.to_string().starts_with("NtSuspendProcess failed"), "{e}");

        // Waiting needs SYNCHRONIZE.
        let q = ProcessHandle::open(pid, Access::QUERY).unwrap();
        assert!(q.wait_exit(Duration::ZERO).is_err());
    }
}

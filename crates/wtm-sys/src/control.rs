//! Per-process control: terminate, suspend, priority, affinity, EcoQoS.

use std::ops::BitOr;
use std::time::Duration;

use windows::Win32::Foundation::{FILETIME, HANDLE, NTSTATUS, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetPriorityClass,
    GetProcessAffinityMask, GetProcessInformation, GetProcessTimes, HIGH_PRIORITY_CLASS,
    IDLE_PRIORITY_CLASS, IsProcessCritical, NORMAL_PRIORITY_CLASS, OpenProcess,
    PROCESS_ACCESS_RIGHTS, PROCESS_CREATION_FLAGS, PROCESS_NAME_WIN32,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    PROCESS_SET_QUOTA, PROCESS_SUSPEND_RESUME, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    ProcessPowerThrottling, QueryFullProcessImageNameW, REALTIME_PRIORITY_CLASS, SetPriorityClass,
    SetProcessAffinityMask, SetProcessInformation, TerminateProcess, WaitForSingleObject,
};
use windows::core::PWSTR;

use crate::handle::OwnedHandle;
use crate::{Error, Result};

windows::core::link!("ntdll.dll" "system" fn NtSuspendProcess(process_handle: HANDLE) -> NTSTATUS);
windows::core::link!("ntdll.dll" "system" fn NtResumeProcess(process_handle: HANDLE) -> NTSTATUS);

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

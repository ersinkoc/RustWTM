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

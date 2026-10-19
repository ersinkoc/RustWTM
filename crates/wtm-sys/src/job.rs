//! Job objects: hard CPU caps and memory limits.

use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_CPU_RATE_CONTROL,
    JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, JOB_OBJECT_LIMIT,
    JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOBOBJECTINFOCLASS, JobObjectCpuRateControlInformation,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows::core::PCWSTR;

use crate::Result;
use crate::control::ProcessHandle;
use crate::handle::OwnedHandle;

/// An anonymous job object.
///
/// Limits stay in force for assigned processes even after this handle is
/// dropped, but they can then no longer be changed or lifted.
#[derive(Debug)]
pub struct Job {
    handle: OwnedHandle,
}

impl Job {
    pub fn create() -> Result<Self> {
        // SAFETY: default security, anonymous job.
        let raw = unsafe { CreateJobObjectW(None, PCWSTR::null())? };
        Ok(Self {
            handle: OwnedHandle::new(raw),
        })
    }

    /// The process must be opened with `Access::SET_QUOTA | Access::TERMINATE`.
    /// Nested jobs are supported, so this works for most processes that are
    /// already in a job.
    pub fn assign(&self, process: &ProcessHandle) -> Result<()> {
        // SAFETY: both handles are valid and owned.
        unsafe { AssignProcessToJobObject(self.handle.raw(), process.raw())? };
        Ok(())
    }

    /// Hard-caps the whole job to `percent` of total machine CPU capacity
    /// (all logical processors). `None` removes the cap.
    pub fn set_cpu_cap(&self, percent: Option<f32>) -> Result<()> {
        let info = match percent {
            Some(p) => JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
                ControlFlags: JOB_OBJECT_CPU_RATE_CONTROL_ENABLE
                    | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
                // CpuRate is in 1/100 of a percent: 1..=10000.
                Anonymous: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 {
                    CpuRate: (p * 100.0).round().clamp(1.0, 10_000.0) as u32,
                },
            },
            None => JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
                ControlFlags: JOB_OBJECT_CPU_RATE_CONTROL(0),
                ..Default::default()
            },
        };
        self.set(JobObjectCpuRateControlInformation, &info)
    }

    /// Per-process committed memory limit. `None` removes it.
    pub fn set_process_memory_limit(&self, bytes: Option<u64>) -> Result<()> {
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        if let Some(bytes) = bytes {
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            info.ProcessMemoryLimit = usize::try_from(bytes).unwrap_or(usize::MAX);
        } else {
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT(0);
        }
        self.set(JobObjectExtendedLimitInformation, &info)
    }

    /// Limit on the committed memory of all processes in the job together.
    /// `None` removes it.
    pub fn set_job_memory_limit(&self, bytes: Option<u64>) -> Result<()> {
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        if let Some(bytes) = bytes {
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_JOB_MEMORY;
            info.JobMemoryLimit = usize::try_from(bytes).unwrap_or(usize::MAX);
        } else {
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT(0);
        }
        self.set(JobObjectExtendedLimitInformation, &info)
    }

    /// Whether `process` (opened with `Access::QUERY`) is in this job.
    pub fn contains(&self, process: &ProcessHandle) -> Result<bool> {
        let mut result = windows::core::BOOL::default();
        // SAFETY: both handles are valid; `result` is a writable BOOL.
        unsafe { IsProcessInJob(process.raw(), Some(self.handle.raw()), &mut result)? };
        Ok(result.as_bool())
    }

    fn set<T>(&self, class: JOBOBJECTINFOCLASS, info: &T) -> Result<()> {
        // SAFETY: `info` is the structure matching `class`, sized by `T`.
        unsafe {
            SetInformationJobObject(
                self.handle.raw(),
                class,
                std::ptr::from_ref(info).cast(),
                size_of::<T>() as u32,
            )?
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_a_cap_that_was_never_set_is_rejected() {
        let job = Job::create().unwrap();
        assert!(job.set_cpu_cap(None).is_err());
        job.set_cpu_cap(Some(50.0)).unwrap();
        job.set_cpu_cap(None).unwrap();
        job.set_process_memory_limit(Some(64 << 20)).unwrap();
        job.set_process_memory_limit(None).unwrap();
        job.set_job_memory_limit(Some(64 << 20)).unwrap();
        job.set_job_memory_limit(None).unwrap();
    }

    #[test]
    fn reports_membership() {
        use crate::control::{Access, ProcessHandle};
        use std::process::{Command, Stdio};
        let mut parent = Command::new("cmd.exe")
            .args(["/c", "ping -n 3 127.0.0.1 >nul"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let job = Job::create().unwrap();
        let h = ProcessHandle::open(
            parent.id(),
            Access::QUERY | Access::SET_QUOTA | Access::TERMINATE,
        )
        .unwrap();
        let other = ProcessHandle::open(std::process::id(), Access::QUERY).unwrap();
        assert!(!job.contains(&h).unwrap());
        job.assign(&h).unwrap();
        assert!(job.contains(&h).unwrap());
        assert!(!job.contains(&other).unwrap());
        job.set_job_memory_limit(Some(1 << 30)).unwrap();
        let _ = parent.kill();
        let _ = parent.wait();
        // Query rights are required.
        let bare = ProcessHandle::open(std::process::id(), Access::SYNCHRONIZE).unwrap();
        assert!(job.contains(&bare).is_err());
    }
}

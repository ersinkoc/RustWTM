//! System-wide CPU and memory counters.

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{GetActiveProcessorCount, GetSystemTimes};

use crate::Result;

const ALL_PROCESSOR_GROUPS: u16 = 0xFFFF;

/// Cumulative system CPU times in 100 ns units. `kernel` includes `idle`.
#[derive(Debug, Clone, Copy, Default)]
pub struct CpuTimes {
    pub idle: u64,
    pub kernel: u64,
    pub user: u64,
}

pub fn cpu_times() -> Result<CpuTimes> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: all three pointers reference live, writable FILETIMEs.
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user))? };
    Ok(CpuTimes {
        idle: filetime_to_u64(idle),
        kernel: filetime_to_u64(kernel),
        user: filetime_to_u64(user),
    })
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryStatus {
    pub load_percent: u32,
    pub total_phys: u64,
    pub avail_phys: u64,
    pub commit_limit: u64,
    pub commit_avail: u64,
}

pub fn memory_status() -> Result<MemoryStatus> {
    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `status` is a valid MEMORYSTATUSEX with dwLength set.
    unsafe { GlobalMemoryStatusEx(&mut status)? };
    Ok(MemoryStatus {
        load_percent: status.dwMemoryLoad,
        total_phys: status.ullTotalPhys,
        avail_phys: status.ullAvailPhys,
        commit_limit: status.ullTotalPageFile,
        commit_avail: status.ullAvailPageFile,
    })
}

/// Logical processors across all processor groups.
pub fn logical_cpu_count() -> u32 {
    // SAFETY: no pointers involved.
    let n = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
    n.max(1)
}

fn filetime_to_u64(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_are_sane() {
        let t = cpu_times().unwrap();
        assert!(t.kernel >= t.idle);
        let m = memory_status().unwrap();
        assert!(m.total_phys > 0 && m.avail_phys <= m.total_phys);
        assert!(logical_cpu_count() >= 1);
    }
}

//! System-wide CPU and memory counters.

use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{GetActiveProcessorCount, GetSystemTimes};

use crate::{Error, Result};

/// `SYSTEM_INFORMATION_CLASS::SystemProcessorPerformanceInformation`.
const PROCESSOR_PERFORMANCE: SYSTEM_INFORMATION_CLASS = SYSTEM_INFORMATION_CLASS(8);

/// `SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ProcessorPerformance {
    idle: i64,
    kernel: i64,
    user: i64,
    _dpc: i64,
    _interrupt: i64,
    _interrupt_count: u32,
}

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

/// Cumulative times of each logical processor of the calling thread's
/// processor group (all of them on machines with up to 64).
pub fn per_cpu_times() -> Result<Vec<CpuTimes>> {
    // On the stack: called every sample, and a group has at most 64.
    let mut buf = [ProcessorPerformance::default(); 64];
    let mut len = 0u32;
    // SAFETY: `buf` is writable for its whole byte size; `len` receives the
    // number of bytes written.
    let status = unsafe {
        NtQuerySystemInformation(
            PROCESSOR_PERFORMANCE,
            buf.as_mut_ptr().cast(),
            size_of_val(&buf) as u32,
            &mut len,
        )
    };
    if status.is_err() {
        return Err(Error::Nt {
            call: "NtQuerySystemInformation",
            status: status.0 as u32,
        });
    }
    let n = (len as usize / size_of::<ProcessorPerformance>()).min(buf.len());
    Ok(buf[..n]
        .iter()
        .map(|p| CpuTimes {
            idle: p.idle as u64,
            kernel: p.kernel as u64,
            user: p.user as u64,
        })
        .collect())
}

/// Bytes received and sent so far by the physical network adapters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetCounters {
    pub received: u64,
    pub sent: u64,
}

pub fn net_counters() -> Result<NetCounters> {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: GetIfTable2 allocates the table; it is freed below.
    unsafe { GetIfTable2(&mut table).ok()? };
    let mut total = NetCounters::default();
    // SAFETY: on success `table` points to NumEntries rows.
    unsafe {
        let rows =
            std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        // Virtual adapters (switches, filters) would count traffic twice.
        for row in rows.iter().filter(|r| {
            r.InterfaceAndOperStatusFlags._bitfield & 1 != 0 && r.OperStatus == IfOperStatusUp
        }) {
            total.received += row.InOctets;
            total.sent += row.OutOctets;
        }
        FreeMibTable(table.cast());
    }
    Ok(total)
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
        let cores = per_cpu_times().unwrap();
        assert!(!cores.is_empty() && cores.len() as u32 <= logical_cpu_count());
        assert!(cores.iter().all(|c| c.kernel >= c.idle));
        let a = net_counters().unwrap();
        let b = net_counters().unwrap();
        assert!(b.received >= a.received && b.sent >= a.sent);
    }
}

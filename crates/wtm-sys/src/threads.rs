//! Per-thread details for a process's Threads view: where each thread
//! started, and the words for its state.

use windows::Win32::Foundation::{HANDLE, NTSTATUS};
use windows::Win32::System::Threading::{OpenThread, THREAD_QUERY_INFORMATION};

use crate::Result;
use crate::handle::OwnedHandle;

windows::core::link!("ntdll.dll" "system" fn NtQueryInformationThread(
    thread_handle: HANDLE,
    class: u32,
    info: *mut core::ffi::c_void,
    len: u32,
    returned: *mut u32,
) -> NTSTATUS);

/// `THREADINFOCLASS::ThreadQuerySetWin32StartAddress`.
const THREAD_WIN32_START_ADDRESS: u32 = 9;

/// The address a thread's code started at, as the program asked for it
/// (`CreateThread`'s start routine) rather than the loader thunk every
/// user thread starts in. Needs `THREAD_QUERY_INFORMATION`; limited query
/// rights are refused for this class.
pub fn win32_start_address(tid: u32) -> Result<u64> {
    // SAFETY: plain call; the handle is owned by the wrapper.
    let thread = OwnedHandle::new(unsafe { OpenThread(THREAD_QUERY_INFORMATION, false, tid)? });
    let mut address = 0usize;
    let mut len = 0u32;
    // SAFETY: `address` is a writable pointer-sized value of the size passed.
    let status = unsafe {
        NtQueryInformationThread(
            thread.raw(),
            THREAD_WIN32_START_ADDRESS,
            (&raw mut address).cast(),
            size_of::<usize>() as u32,
            &mut len,
        )
    };
    status.ok()?;
    Ok(address as u64)
}

/// The calling thread's ID.
pub fn current_thread_id() -> u32 {
    // SAFETY: plain call without arguments.
    unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }
}

/// A loaded module's address range, for naming code addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleRange {
    pub name: String,
    pub base: u64,
    pub size: u64,
}

/// `address` as "module+0xoffset" when it lies inside one of `modules`,
/// otherwise as a plain hexadecimal address.
pub fn symbolize(address: u64, modules: &[ModuleRange]) -> String {
    modules
        .iter()
        .find(|m| address >= m.base && address - m.base < m.size)
        .map_or_else(
            || format!("0x{address:x}"),
            |m| format!("{}+0x{:x}", m.name, address - m.base),
        )
}

/// `KWAIT_REASON` names, by value, as in the phnt headers
/// (`ntkeapi.h`). Newer Windows versions may add reasons at the end.
const WAIT_REASONS: [&str; 43] = [
    "Executive",
    "FreePage",
    "PageIn",
    "PoolAllocation",
    "DelayExecution",
    "Suspended",
    "UserRequest",
    "WrExecutive",
    "WrFreePage",
    "WrPageIn",
    "WrPoolAllocation",
    "WrDelayExecution",
    "WrSuspended",
    "WrUserRequest",
    "WrEventPair",
    "WrQueue",
    "WrLpcReceive",
    "WrLpcReply",
    "WrVirtualMemory",
    "WrPageOut",
    "WrRendezvous",
    "WrKeyedEvent",
    "WrTerminated",
    "WrProcessInSwap",
    "WrCpuRateControl",
    "WrCalloutStack",
    "WrKernel",
    "WrResource",
    "WrPushLock",
    "WrMutex",
    "WrQuantumEnd",
    "WrDispatchInt",
    "WrPreempted",
    "WrYieldExecution",
    "WrFastMutex",
    "WrGuardedMutex",
    "WrRundown",
    "WrAlertByThreadId",
    "WrDeferredPreempt",
    "WrPhysicalFault",
    "WrIoRing",
    "WrMdlCache",
    "WrRcu",
];

/// The name of a `KWAIT_REASON`, or its number when unknown.
pub fn wait_reason_name(reason: u32) -> String {
    WAIT_REASONS
        .get(reason as usize)
        .map_or_else(|| reason.to_string(), |n| (*n).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_named_by_the_module_they_fall_in() {
        let modules = [
            ModuleRange {
                name: "app.exe".into(),
                base: 0x1000,
                size: 0x100,
            },
            ModuleRange {
                name: "ntdll.dll".into(),
                base: 0x7ff0_0000,
                size: 0x2000,
            },
        ];
        assert_eq!(symbolize(0x1000, &modules), "app.exe+0x0");
        assert_eq!(symbolize(0x10ff, &modules), "app.exe+0xff");
        // The first byte past a module belongs to none.
        assert_eq!(symbolize(0x1100, &modules), "0x1100");
        assert_eq!(symbolize(0x7ff0_1a2b, &modules), "ntdll.dll+0x1a2b");
        assert_eq!(symbolize(0x10, &[]), "0x10");
    }

    #[test]
    fn wait_reasons_match_the_ones_the_wait_checks_use() {
        // `ThreadInfo::blocked` treats these as idle.
        assert_eq!(wait_reason_name(4), "DelayExecution");
        assert_eq!(wait_reason_name(5), "Suspended");
        assert_eq!(wait_reason_name(11), "WrDelayExecution");
        assert_eq!(wait_reason_name(12), "WrSuspended");
        assert_eq!(wait_reason_name(15), "WrQueue");
        assert_eq!(wait_reason_name(42), "WrRcu");
        assert_eq!(wait_reason_name(43), "43");
    }

    #[test]
    fn this_threads_start_address_is_readable() {
        // SAFETY: plain call.
        let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        assert!(win32_start_address(tid).unwrap() != 0);
    }
}

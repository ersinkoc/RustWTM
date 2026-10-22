//! Per-process control: terminate, suspend, priority, affinity, EcoQoS,
//! memory dumps.

use std::ops::BitOr;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::time::Duration;

use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows::Win32::Foundation::{
    E_FAIL, FILETIME, HANDLE, NTSTATUS, S_FALSE, UNICODE_STRING, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Security::{
    GetTokenInformation, LookupAccountSidW, SID_NAME_USE, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::System::Diagnostics::Debug::{
    CONTEXT_FLAGS, IsProcessSnapshotCallback, MINIDUMP_CALLBACK_INFORMATION,
    MINIDUMP_CALLBACK_INPUT, MINIDUMP_CALLBACK_OUTPUT, MiniDumpWithFullMemory,
    MiniDumpWithFullMemoryInfo, MiniDumpWithHandleData, MiniDumpWithThreadInfo,
    MiniDumpWithUnloadedModules, MiniDumpWriteDump,
};
use windows::Win32::System::Diagnostics::ProcessSnapshotting::{
    HPSS, PSS_CAPTURE_HANDLE_BASIC_INFORMATION, PSS_CAPTURE_HANDLE_NAME_INFORMATION,
    PSS_CAPTURE_HANDLE_TYPE_SPECIFIC_INFORMATION, PSS_CAPTURE_HANDLES, PSS_CAPTURE_THREAD_CONTEXT,
    PSS_CAPTURE_THREADS, PSS_CAPTURE_VA_CLONE, PSS_CREATE_RELEASE_SECTION,
    PSS_CREATE_USE_VM_ALLOCATIONS, PssCaptureSnapshot, PssFreeSnapshot,
};
use windows::Win32::System::Memory::{
    GetProcessWorkingSetSizeEx, QUOTA_LIMITS_HARDWS_MAX_DISABLE, QUOTA_LIMITS_HARDWS_MIN_ENABLE,
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
use windows::Win32::System::Threading::{
    GetCurrentProcess, PROCESS_CREATE_PROCESS, PROCESS_DUP_HANDLE,
};
use windows::Win32::System::Threading::{
    GetCurrentThread, GetProcessDefaultCpuSets, MEMORY_PRIORITY_INFORMATION,
    MEMORY_PRIORITY_NORMAL, PROCESS_SET_LIMITED_INFORMATION, ProcessMemoryPriority,
    SetProcessDefaultCpuSets, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
    THREAD_PRIORITY_ABOVE_NORMAL,
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
    /// Enough to change default CPU Sets; granted for more processes than
    /// `SET_INFORMATION`.
    pub const SET_LIMITED_INFORMATION: Self = Self(PROCESS_SET_LIMITED_INFORMATION);
    /// Required (with TERMINATE) to assign the process to a job object.
    pub const SET_QUOTA: Self = Self(PROCESS_SET_QUOTA);
    pub const SYNCHRONIZE: Self = Self(PROCESS_SYNCHRONIZE);
    /// Required to write a memory dump.
    pub const DUMP: Self = Self(PROCESS_ACCESS_RIGHTS(
        PROCESS_QUERY_INFORMATION.0 | PROCESS_VM_READ.0,
    ));
    /// A memory dump from a snapshot clone: the process is frozen for
    /// milliseconds instead of the whole write.
    pub const DUMP_CLONE: Self = Self(PROCESS_ACCESS_RIGHTS(
        PROCESS_QUERY_INFORMATION.0
            | PROCESS_VM_READ.0
            | PROCESS_DUP_HANDLE.0
            | PROCESS_CREATE_PROCESS.0,
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
        // Almost every path fits on the stack; long ones get the most a
        // path can be.
        let mut small = [0u16; 1024];
        match self.image_path_into(&mut small) {
            Err(Error::Win32(e)) if e.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() => {
                self.image_path_into(&mut vec![0u16; 32 * 1024])
            }
            r => r,
        }
    }

    fn image_path_into(&self, buf: &mut [u16]) -> Result<String> {
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
        Ok(crate::utf16_lossy(&buf[..len as usize]))
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
        // The process may rewrite its command line between the size query
        // and the read: asked again, a few times, with the new size.
        const TOO_SMALL: [u32; 3] = [0xC000_0004, 0xC000_0023, 0x8000_0005];
        let mut attempts = 0;
        let (buf, status) = loop {
            // u64 elements keep the UNICODE_STRING header aligned.
            let mut buf = vec![0u64; (len as usize).div_ceil(8).max(2)];
            let size = (buf.len() * 8) as u32;
            // SAFETY: buf is writable for size bytes.
            let status = unsafe {
                NtQueryInformationProcess(
                    self.raw(),
                    PROCESS_COMMAND_LINE_INFORMATION,
                    buf.as_mut_ptr().cast(),
                    size,
                    &mut len,
                )
            };
            attempts += 1;
            if !TOO_SMALL.contains(&(status.0 as u32)) || attempts == 4 || len <= size {
                break (buf, status);
            }
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
        Ok(crate::utf16_lossy(text))
    }

    /// The account the process runs as, e.g. NT AUTHORITY\SYSTEM.
    pub fn user(&self) -> Result<String> {
        let mut raw = HANDLE::default();
        // SAFETY: `raw` receives a token handle that OwnedHandle closes.
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
    /// file") to `path`, replacing it. Requires `Access::DUMP`; with
    /// `Access::DUMP_CLONE` the process is first cloned (a copy-on-write
    /// process snapshot) and the dump written from the clone, so it is frozen
    /// for milliseconds rather than for the whole write. Returns whether the
    /// clone was used.
    pub fn write_dump(&self, path: &Path) -> Result<bool> {
        let file = std::fs::File::create(path).map_err(|e| {
            let hr = e.raw_os_error().map_or(E_FAIL, |code| {
                windows::core::HRESULT::from_win32(code as u32)
            });
            Error::Win32(hr.into())
        })?;
        let out = HANDLE(file.as_raw_handle());
        let kind = MiniDumpWithFullMemory
            | MiniDumpWithFullMemoryInfo
            | MiniDumpWithHandleData
            | MiniDumpWithThreadInfo
            | MiniDumpWithUnloadedModules;

        let result = match CloneSnapshot::capture(self.raw()) {
            Some(snapshot) => {
                let callback = MINIDUMP_CALLBACK_INFORMATION {
                    CallbackRoutine: Some(snapshot_callback),
                    CallbackParam: std::ptr::null_mut(),
                };
                // SAFETY: the snapshot handle stays valid until `snapshot` is
                // dropped after the call; `out` is a writable file handle
                // owned by `file`; `callback` outlives the call.
                unsafe {
                    MiniDumpWriteDump(
                        HANDLE(snapshot.0.0),
                        self.pid,
                        out,
                        kind,
                        None,
                        None,
                        Some(&raw const callback),
                    )
                }
                .map(|()| true)
            }
            // SAFETY: valid process handle owned by `self`; `out` is a
            // writable file handle owned by `file`.
            None => unsafe { MiniDumpWriteDump(self.raw(), self.pid, out, kind, None, None, None) }
                .map(|()| false),
        };

        drop(file);
        result.map_err(|e| {
            let _ = std::fs::remove_file(path);
            e.into()
        })
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

    /// The process's default CPU Set IDs; empty when it may use every
    /// processor. Requires `Access::QUERY`.
    pub fn default_cpu_sets(&self) -> Result<Vec<u32>> {
        let mut ids = vec![0u32; 64];
        loop {
            let mut required = 0u32;
            // SAFETY: `ids` is writable for its length; `required` is writable.
            let ok = unsafe {
                GetProcessDefaultCpuSets(self.raw(), Some(&mut ids), &mut required).as_bool()
            };
            if ok {
                ids.truncate(required as usize);
                return Ok(ids);
            }
            let error = windows::core::Error::from_thread();
            if error.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult()
                || required as usize <= ids.len()
            {
                return Err(error.into());
            }
            ids.resize(required as usize, 0);
        }
    }

    /// Confines the process's threads that have no CPU Sets of their own to
    /// `ids`; an empty slice lifts the restriction. Requires
    /// `Access::SET_LIMITED_INFORMATION`.
    pub fn set_default_cpu_sets(&self, ids: &[u32]) -> Result<()> {
        let ids = (!ids.is_empty()).then_some(ids);
        // SAFETY: valid handle; the slice, if any, outlives the call.
        unsafe { SetProcessDefaultCpuSets(self.raw(), ids).ok()? };
        Ok(())
    }

    /// Keeps at least `min_bytes` of the process resident: Windows will not
    /// trim its working set below that, even under memory pressure. Requires
    /// `Access::QUERY | Access::SET_QUOTA`.
    pub fn lock_working_set(&self, min_bytes: usize) -> Result<()> {
        let (mut min, mut max, mut flags) = (0usize, 0usize, 0u32);
        // SAFETY: all out-pointers reference live locals.
        unsafe { GetProcessWorkingSetSizeEx(self.raw(), &mut min, &mut max, &mut flags).ok()? };
        let max = max.max(min_bytes.saturating_add(min_bytes / 2));
        // SAFETY: valid handle; the sizes are plain values.
        unsafe {
            SetProcessWorkingSetSizeEx(
                self.raw(),
                min_bytes,
                max,
                QUOTA_LIMITS_HARDWS_MIN_ENABLE | QUOTA_LIMITS_HARDWS_MAX_DISABLE,
            )?
        };
        Ok(())
    }

    /// Restores normal memory priority, so the process's pages are the last
    /// to be repurposed from the standby list. Requires `Access::SET_INFORMATION`.
    pub fn set_normal_memory_priority(&self) -> Result<()> {
        let info = MEMORY_PRIORITY_INFORMATION {
            MemoryPriority: MEMORY_PRIORITY_NORMAL,
        };
        // SAFETY: `info` is a correctly sized MEMORY_PRIORITY_INFORMATION.
        unsafe {
            SetProcessInformation(
                self.raw(),
                ProcessMemoryPriority,
                (&raw const info).cast(),
                size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
            )?
        };
        Ok(())
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

/// Raises the calling thread above the other threads of its priority class,
/// e.g. so a UI thread keeps painting while workers are busy.
pub fn raise_current_thread() -> Result<()> {
    // SAFETY: the pseudo handle of the current thread needs no closing.
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL)? };
    Ok(())
}

/// Puts the calling thread in background mode: lowest CPU, I/O and memory
/// priority, so long work such as scanning a disk never slows down the
/// machine or the window. Lasts until the thread exits.
pub fn enter_background_mode() -> Result<()> {
    // SAFETY: the pseudo handle of the current thread needs no closing.
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN)? };
    Ok(())
}

/// A copy-on-write clone of a process (`PssCaptureSnapshot`), freed on drop.
struct CloneSnapshot(HPSS);

impl CloneSnapshot {
    /// Clones `process`, or `None` when the handle lacks the rights for it
    /// (`PROCESS_CREATE_PROCESS`, `PROCESS_DUP_HANDLE`) or cloning fails.
    fn capture(process: HANDLE) -> Option<Self> {
        let flags = PSS_CAPTURE_VA_CLONE
            | PSS_CAPTURE_HANDLES
            | PSS_CAPTURE_HANDLE_NAME_INFORMATION
            | PSS_CAPTURE_HANDLE_BASIC_INFORMATION
            | PSS_CAPTURE_HANDLE_TYPE_SPECIFIC_INFORMATION
            | PSS_CAPTURE_THREADS
            | PSS_CAPTURE_THREAD_CONTEXT
            | PSS_CREATE_USE_VM_ALLOCATIONS
            | PSS_CREATE_RELEASE_SECTION;
        let mut snapshot = HPSS::default();
        // SAFETY: valid process handle; `snapshot` is a writable HPSS.
        let status =
            unsafe { PssCaptureSnapshot(process, flags, Some(CONTEXT_ALL.0), &mut snapshot) };
        (status == 0 && !snapshot.is_invalid()).then_some(Self(snapshot))
    }
}

impl Drop for CloneSnapshot {
    fn drop(&mut self) {
        // SAFETY: the snapshot was captured into this process and is freed
        // exactly once.
        unsafe {
            let _ = PssFreeSnapshot(GetCurrentProcess(), self.0);
        }
    }
}

#[cfg(target_arch = "x86_64")]
const CONTEXT_ALL: CONTEXT_FLAGS = windows::Win32::System::Diagnostics::Debug::CONTEXT_ALL_AMD64;
#[cfg(target_arch = "aarch64")]
const CONTEXT_ALL: CONTEXT_FLAGS = windows::Win32::System::Diagnostics::Debug::CONTEXT_ALL_ARM64;
#[cfg(target_arch = "x86")]
const CONTEXT_ALL: CONTEXT_FLAGS = windows::Win32::System::Diagnostics::Debug::CONTEXT_ALL_X86;

/// `MiniDumpWriteDump` callback that, asked whether its handle is a process
/// snapshot, answers `S_FALSE` (yes); every other request keeps its default.
unsafe extern "system" fn snapshot_callback(
    _param: *mut core::ffi::c_void,
    input: *const MINIDUMP_CALLBACK_INPUT,
    output: *mut MINIDUMP_CALLBACK_OUTPUT,
) -> windows::core::BOOL {
    // SAFETY: dbghelp passes valid input and output records; both structs
    // are packed, so fields are accessed through unaligned reads/writes.
    unsafe {
        let kind = std::ptr::addr_of!((*input).CallbackType).read_unaligned();
        if kind == IsProcessSnapshotCallback.0 as u32 {
            std::ptr::addr_of_mut!((*output).Anonymous.Status).write_unaligned(S_FALSE);
        }
    }
    true.into()
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
    fn default_cpu_sets_and_working_set_floor_apply_to_a_child() {
        let child = spawn_ping();
        let access = Access::QUERY | Access::SET_LIMITED_INFORMATION | Access::SET_QUOTA;
        let h = ProcessHandle::open(child.0.id(), access).unwrap();
        assert!(h.default_cpu_sets().unwrap().is_empty());
        let split = crate::cpu::topology().unwrap();
        let first = split.cpus[0].id;
        h.set_default_cpu_sets(&[first]).unwrap();
        assert_eq!(h.default_cpu_sets().unwrap(), [first]);
        h.set_default_cpu_sets(&[]).unwrap();
        assert!(h.default_cpu_sets().unwrap().is_empty());
        h.lock_working_set(8 << 20).unwrap();

        let weak = ProcessHandle::open(child.0.id(), Access::QUERY).unwrap();
        assert!(weak.set_default_cpu_sets(&[first]).is_err());
        assert!(weak.set_normal_memory_priority().is_err());
    }

    #[test]
    fn hardens_the_current_process_and_thread() {
        let access = Access::QUERY | Access::SET_INFORMATION | Access::SET_QUOTA;
        let h = ProcessHandle::open(std::process::id(), access).unwrap();
        h.set_normal_memory_priority().unwrap();
        h.lock_working_set(16 << 20).unwrap();
        raise_current_thread().unwrap();
        std::thread::spawn(|| enter_background_mode().unwrap())
            .join()
            .unwrap();
    }

    #[test]
    fn writes_a_memory_dump_of_a_child() {
        let child = spawn_ping();
        let path = std::env::temp_dir().join(format!("wtm-dump-test-{}.dmp", std::process::id()));
        // With the rights to clone it, the dump is written from a snapshot.
        let h = ProcessHandle::open(child.0.id(), Access::DUMP_CLONE).unwrap();
        assert!(h.write_dump(&path).unwrap(), "written from a clone");
        let h = ProcessHandle::open(child.0.id(), Access::DUMP).unwrap();
        assert!(
            !h.write_dump(&path).unwrap(),
            "live, without the clone rights"
        );
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

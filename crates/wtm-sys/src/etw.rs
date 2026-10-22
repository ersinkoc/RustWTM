//! Minimal real-time ETW: a private trace session, providers enabled on it,
//! and a consumer thread that hands every event to a callback. Needs
//! administrator rights.
//!
//! A session is a kernel object that outlives the process which started it,
//! so names carry the owner's PID and start time ([`owned_name`]) and
//! [`stop_orphans`] clears the ones whose owner is gone, also when another
//! process got its PID since.

use core::ffi::c_void;
use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::thread::JoinHandle;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_INVALID_PARAMETER, WAIT_TIMEOUT, WIN32_ERROR,
};
use windows::Win32::System::Diagnostics::Etw::{
    CONTROLTRACE_HANDLE, CloseTrace, ControlTraceW, ENABLE_TRACE_PARAMETERS,
    ENABLE_TRACE_PARAMETERS_VERSION_2, EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_FILTER_DESCRIPTOR,
    EVENT_FILTER_TYPE_EVENT_ID, EVENT_RECORD, EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_FLAG,
    EVENT_TRACE_LOGFILEW, EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE,
    EVENT_TRACE_SYSTEM_LOGGER_MODE, EnableTraceEx2, OpenTraceW, PROCESS_TRACE_MODE_EVENT_RECORD,
    PROCESS_TRACE_MODE_REAL_TIME, PROCESSTRACE_HANDLE, ProcessTrace, QueryAllTracesW, StartTraceW,
    WNODE_FLAG_TRACED_GUID,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, WaitForSingleObject,
};
use windows::core::{GUID, PCWSTR, PWSTR};

use crate::handle::OwnedHandle;

type Callback = Box<dyn FnMut(&EVENT_RECORD) + Send>;

/// Room for a session name and a log file name, in UTF-16 units each.
const NAME_UNITS: usize = 1024;
/// The most sessions `QueryAllTracesW` reports on any Windows version.
const MAX_SESSIONS: usize = 64;
/// `OpenTraceW` reports failure with an all-ones handle.
const INVALID_TRACE: u64 = u64::MAX;
/// Buffer sizing: 64 KiB buffers, flushed at least once a second so the
/// consumer sees events promptly even when traffic is light.
const BUFFER_KB: u32 = 64;
const MIN_BUFFERS: u32 = 8;
const MAX_BUFFERS: u32 = 64;
const FLUSH_SECONDS: u32 = 1;
/// `WNODE_HEADER::ClientContext`: timestamps from the performance counter.
const CLOCK_QPC: u32 = 1;

/// An `EVENT_TRACE_PROPERTIES` followed by space for the two names ETW
/// writes after it, in a buffer aligned for the structure.
struct Properties {
    buf: Vec<u64>,
}

impl Properties {
    fn new() -> Self {
        let header = size_of::<EVENT_TRACE_PROPERTIES>();
        let bytes = header + 2 * NAME_UNITS * size_of::<u16>();
        let mut this = Self {
            buf: vec![0; bytes.div_ceil(size_of::<u64>())],
        };
        let total = (this.buf.len() * size_of::<u64>()) as u32;
        let p = this.get();
        p.Wnode.BufferSize = total;
        p.LoggerNameOffset = header as u32;
        p.LogFileNameOffset = (header + NAME_UNITS * size_of::<u16>()) as u32;
        this
    }

    fn get(&mut self) -> &mut EVENT_TRACE_PROPERTIES {
        // SAFETY: the buffer is u64-aligned (enough for the structure), at
        // least as large as it, zero-initialised, and the structure is plain
        // data for which any bit pattern is valid.
        unsafe { &mut *self.buf.as_mut_ptr().cast::<EVENT_TRACE_PROPERTIES>() }
    }

    fn as_mut_ptr(&mut self) -> *mut EVENT_TRACE_PROPERTIES {
        self.buf.as_mut_ptr().cast()
    }

    /// The session name ETW stored at `LoggerNameOffset`.
    fn logger_name(&mut self) -> String {
        let offset = self.get().LoggerNameOffset as usize;
        let units = self.buf.len() * size_of::<u64>() / size_of::<u16>();
        let start = offset / size_of::<u16>();
        if offset == 0 || start >= units {
            return String::new();
        }
        // SAFETY: `start..units` lies inside the buffer, which is u16-aligned.
        let all = unsafe { std::slice::from_raw_parts(self.buf.as_ptr().cast::<u16>(), units) };
        let name = &all[start..];
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        String::from_utf16_lossy(&name[..end])
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn check(code: WIN32_ERROR) -> crate::Result<()> {
    code.ok().map_err(crate::Error::from)
}

/// Stops the session called `name` (NUL-terminated), if there is one.
fn stop_by_name(name: &[u16]) -> WIN32_ERROR {
    let mut props = Properties::new();
    // SAFETY: `name` is NUL-terminated and `props` is a correctly sized
    // properties buffer; both outlive the call.
    unsafe {
        ControlTraceW(
            CONTROLTRACE_HANDLE::default(),
            PCWSTR(name.as_ptr()),
            props.as_mut_ptr(),
            EVENT_TRACE_CONTROL_STOP,
        )
    }
}

/// A session name for this process: `{prefix}-{pid}.{start time}`, which
/// [`stop_orphans`] can tell from one of a later process with the same PID.
pub fn owned_name(prefix: &str) -> String {
    let pid = std::process::id();
    // SAFETY: GetCurrentProcess returns a pseudo-handle valid in this process.
    match start_time(unsafe { GetCurrentProcess() }) {
        Some(t) => format!("{prefix}-{pid}.{t}"),
        None => format!("{prefix}-{pid}"),
    }
}

/// The owner in a session name of the form `{prefix}-{pid}[.{start time}]`,
/// optionally followed by `-{anything}`.
fn owner(name: &str, prefix: &str) -> Option<(u32, Option<u64>)> {
    let rest = name.strip_prefix(prefix)?.strip_prefix('-')?;
    let id = rest.split('-').next()?;
    let (pid, started) = match id.split_once('.') {
        Some((pid, started)) => (pid, Some(started)),
        None => (id, None),
    };
    let number = |s: &str| {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<u64>().ok())
            .flatten()
    };
    let pid = u32::try_from(number(pid)?).ok()?;
    match started {
        Some(t) => Some((pid, Some(number(t)?))),
        None => Some((pid, None)),
    }
}

/// When the process behind `handle` started, in FILETIME units.
fn start_time(handle: windows::Win32::Foundation::HANDLE) -> Option<u64> {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: `handle` allows querying; the outputs are writable.
    unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) }.ok()?;
    Some(u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime))
}

/// Whether the session's owner is gone: no process `pid`, or (`started`
/// given) one that started at another time. A process we may not ask counts
/// as the owner.
fn owner_gone(pid: u32, started: Option<u64>) -> bool {
    if !process_running(pid) {
        return true;
    }
    let Some(started) = started else {
        return false;
    };
    // SAFETY: plain call; the handle is owned and closed below.
    match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        Ok(h) => {
            let h = OwnedHandle::new(h);
            start_time(h.raw()).is_some_and(|t| t != started)
        }
        Err(_) => false,
    }
}

/// Whether `pid` names a running process. A process we may not open still
/// counts as running; only "no such process" means it is gone.
fn process_running(pid: u32) -> bool {
    // SAFETY: plain call; the handle is owned and closed below.
    match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
        Ok(h) => {
            let h = OwnedHandle::new(h);
            // SAFETY: `h` is a valid process handle opened with SYNCHRONIZE.
            unsafe { WaitForSingleObject(h.raw(), 0) == WAIT_TIMEOUT }
        }
        Err(e) => e.code() != ERROR_INVALID_PARAMETER.to_hresult(),
    }
}

/// Names of every running trace session.
fn session_names() -> Vec<String> {
    let mut props: Vec<Properties> = (0..MAX_SESSIONS).map(|_| Properties::new()).collect();
    let mut ptrs: Vec<*mut EVENT_TRACE_PROPERTIES> =
        props.iter_mut().map(Properties::as_mut_ptr).collect();
    let mut count = 0u32;
    // SAFETY: every pointer refers to a properties buffer sized and
    // initialised as ETW requires, all alive for the call.
    let code = unsafe { QueryAllTracesW(&mut ptrs, &mut count) };
    if code.is_err() && count == 0 {
        return Vec::new();
    }
    props
        .iter_mut()
        .take(count as usize)
        .map(Properties::logger_name)
        .collect()
}

/// Stops the session called `name` if one is running.
pub(crate) fn stop_named(name: &str) {
    let _ = stop_by_name(&wide(name));
}

/// Stops sessions named by [`owned_name`] (or `{prefix}-{pid}`) whose
/// process is gone: sessions outlive a process that is killed, and Windows
/// allows only 64.
pub fn stop_orphans(prefix: &str) {
    for name in session_names() {
        let Some((pid, started)) = owner(&name, prefix) else {
            continue;
        };
        if owner_gone(pid, started) {
            let _ = stop_by_name(&wide(&name));
        }
    }
}

/// Moves a pointer to the consumer thread.
struct SendPtr(*mut Callback);

// SAFETY: the callback is `Send`, and after `consume` hands the pointer over
// only the consumer thread touches it.
unsafe impl Send for SendPtr {}

/// Called by `ProcessTrace` for every event; forwards to the `Callback`
/// registered as the trace's context.
unsafe extern "system" fn deliver(record: *mut EVENT_RECORD) {
    // SAFETY: ETW passes a record that is valid for the duration of the call.
    let Some(record) = (unsafe { record.as_ref() }) else {
        return;
    };
    let callback = record.UserContext.cast::<Callback>();
    if callback.is_null() {
        return;
    }
    // A panic must not unwind into ETW; the event is dropped instead.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `UserContext` is the boxed callback from `consume`, alive
        // until `ProcessTrace` returns and used only on this thread.
        unsafe { (*callback)(record) }
    }));
}

/// A running trace session; stopped when dropped.
pub struct Session {
    /// NUL-terminated.
    name: Vec<u16>,
    handle: CONTROLTRACE_HANDLE,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = String::from_utf16_lossy(&self.name[..self.name.len().saturating_sub(1)]);
        f.debug_struct("Session").field("name", &name).finish()
    }
}

impl Session {
    /// Starts a real-time session called `name`. A session of that name left
    /// behind by an earlier run (sessions outlive a killed process) is
    /// stopped first.
    pub fn start(name: &str) -> crate::Result<Self> {
        Self::open(name, EVENT_TRACE_REAL_TIME_MODE, EVENT_TRACE_FLAG(0))
    }

    /// A kernel (system logger) session with the classic kernel event
    /// groups in `flags` (`EVENT_TRACE_FLAG_*`), such as disk and file I/O.
    pub fn start_kernel(name: &str, flags: u32) -> crate::Result<Self> {
        Self::open(
            name,
            EVENT_TRACE_REAL_TIME_MODE | EVENT_TRACE_SYSTEM_LOGGER_MODE,
            EVENT_TRACE_FLAG(flags),
        )
    }

    fn open(name: &str, mode: u32, flags: EVENT_TRACE_FLAG) -> crate::Result<Self> {
        let name = wide(name);
        let start = || {
            let mut props = Properties::new();
            let p = props.get();
            p.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
            p.Wnode.ClientContext = CLOCK_QPC;
            p.LogFileMode = mode;
            p.EnableFlags = flags;
            p.BufferSize = BUFFER_KB;
            p.MinimumBuffers = MIN_BUFFERS;
            p.MaximumBuffers = MAX_BUFFERS;
            p.FlushTimer = FLUSH_SECONDS;
            let mut handle = CONTROLTRACE_HANDLE::default();
            // SAFETY: `name` is NUL-terminated and `props` is a properties
            // buffer with room for the name; both outlive the call.
            let code =
                unsafe { StartTraceW(&mut handle, PCWSTR(name.as_ptr()), props.as_mut_ptr()) };
            (code, handle)
        };
        // A leftover of the same name would make the start fail.
        let _ = stop_by_name(&name);
        let (mut code, mut handle) = start();
        if code == ERROR_ALREADY_EXISTS {
            let _ = stop_by_name(&name);
            (code, handle) = start();
        }
        check(code)?;
        Ok(Self { name, handle })
    }

    /// Enables `provider`; with `ids`, only those events reach the session
    /// (ETW drops the rest before they are buffered).
    pub fn enable(
        &self,
        provider: &GUID,
        level: u8,
        keywords: u64,
        ids: &[u16],
    ) -> crate::Result<()> {
        let filter = event_id_filter(ids);
        let mut descriptor = EVENT_FILTER_DESCRIPTOR {
            Ptr: filter.as_ptr() as u64,
            Size: (filter.len() * size_of::<u16>()) as u32,
            Type: EVENT_FILTER_TYPE_EVENT_ID,
        };
        let params = ENABLE_TRACE_PARAMETERS {
            Version: ENABLE_TRACE_PARAMETERS_VERSION_2,
            EnableFilterDesc: &mut descriptor,
            FilterDescCount: 1,
            ..Default::default()
        };
        let params = (!ids.is_empty()).then_some(&params as *const ENABLE_TRACE_PARAMETERS);
        // SAFETY: `provider`, the filter and the parameters are valid for the
        // call; ETW copies the filter before returning.
        let code = unsafe {
            EnableTraceEx2(
                self.handle,
                provider,
                EVENT_CONTROL_CODE_ENABLE_PROVIDER.0,
                level,
                keywords,
                0,
                0,
                params,
            )
        };
        check(code)
    }

    /// Delivers every event to `callback` on a new thread until the session
    /// stops (it is dropped).
    pub fn consume(
        &self,
        callback: impl FnMut(&EVENT_RECORD) + Send + 'static,
    ) -> crate::Result<JoinHandle<()>> {
        let callback: Box<Callback> = Box::new(Box::new(callback));
        let context = Box::into_raw(callback);
        let mut name = self.name.clone();
        let mut logfile = EVENT_TRACE_LOGFILEW {
            LoggerName: PWSTR(name.as_mut_ptr()),
            Context: context.cast::<c_void>(),
            ..Default::default()
        };
        logfile.Anonymous1.ProcessTraceMode =
            PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
        logfile.Anonymous2.EventRecordCallback = Some(deliver);
        // SAFETY: `logfile` is fully initialised and its name buffer is alive
        // for the call; ETW keeps its own copy.
        let trace = unsafe { OpenTraceW(&mut logfile) };
        if trace.Value == INVALID_TRACE {
            let error = windows::core::Error::from_thread();
            // SAFETY: ETW never saw a working trace, so the pointer is still
            // exclusively ours.
            drop(unsafe { Box::from_raw(context) });
            return Err(error.into());
        }
        let context = SendPtr(context);
        let spawned = std::thread::Builder::new()
            .name("etw-consumer".into())
            .spawn(move || {
                let context = context;
                let handle = PROCESSTRACE_HANDLE { Value: trace.Value };
                // SAFETY: `handle` came from `OpenTraceW`; this blocks until
                // the session stops, then the handle is closed exactly once.
                unsafe {
                    let _ = ProcessTrace(&[handle], None, None);
                    let _ = CloseTrace(handle);
                }
                // SAFETY: the trace is closed, so ETW no longer calls back
                // with this pointer.
                drop(unsafe { Box::from_raw(context.0) });
            });
        match spawned {
            Ok(thread) => Ok(thread),
            Err(e) => {
                // The callback box is deliberately leaked: freeing it while a
                // trace that names it as context might still be open would be
                // worse than a few bytes lost on an out-of-threads failure.
                // SAFETY: `trace` is the open handle from `OpenTraceW`.
                let _ = unsafe { CloseTrace(PROCESSTRACE_HANDLE { Value: trace.Value }) };
                Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_OUTOFMEMORY,
                    e.to_string(),
                )
                .into())
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let mut props = Properties::new();
        // SAFETY: the handle and name belong to this session; stopping it
        // makes the consumer's `ProcessTrace` return.
        let _ = unsafe {
            ControlTraceW(
                self.handle,
                PCWSTR(self.name.as_ptr()),
                props.as_mut_ptr(),
                EVENT_TRACE_CONTROL_STOP,
            )
        };
    }
}

/// An `EVENT_FILTER_EVENT_ID` that lets only `ids` through, as UTF-16-sized
/// units: `FilterIn` + `Reserved`, `Count`, then the IDs.
fn event_id_filter(ids: &[u16]) -> Vec<u16> {
    let mut filter = Vec::with_capacity(2 + ids.len());
    filter.push(1); // FilterIn = TRUE, Reserved = 0 (little-endian bytes)
    filter.push(ids.len() as u16);
    filter.extend_from_slice(ids);
    filter
}

/// The event's payload bytes.
pub fn user_data(record: &EVENT_RECORD) -> &[u8] {
    if record.UserData.is_null() || record.UserDataLength == 0 {
        return &[];
    }
    // SAFETY: ETW guarantees `UserDataLength` readable bytes at `UserData`
    // for as long as the record is borrowed.
    unsafe {
        std::slice::from_raw_parts(
            record.UserData.cast::<u8>(),
            usize::from(record.UserDataLength),
        )
    }
}

/// The little-endian `u32` at byte offset `at`, if the payload is long enough.
pub fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    let bytes = d.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// The little-endian `u64` at byte offset `at`, if the payload is long enough.
pub fn u64_at(d: &[u8], at: usize) -> Option<u64> {
    let bytes = d.get(at..at.checked_add(8)?)?;
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_fields_within_bounds() {
        let d = [1, 0, 0, 0, 2, 0, 0, 0, 0xff];
        assert_eq!(u32_at(&d, 0), Some(1));
        assert_eq!(u32_at(&d, 4), Some(2));
        assert_eq!(u32_at(&d, 6), None);
        assert_eq!(u64_at(&d, 0), Some(0x2_0000_0001));
        assert_eq!(u64_at(&d, 2), None);
        assert_eq!(u32_at(&d, usize::MAX), None);
    }

    #[test]
    fn empty_record_has_no_payload() {
        assert!(user_data(&EVENT_RECORD::default()).is_empty());
    }

    #[test]
    fn parses_the_owner_from_session_names() {
        let owner = |name| owner(name, "RustWTM-Disk");
        assert_eq!(owner("RustWTM-Disk-1234"), Some((1234, None)));
        assert_eq!(owner("RustWTM-Disk-1234-names"), Some((1234, None)));
        assert_eq!(owner("RustWTM-Disk-1234.99"), Some((1234, Some(99))));
        assert_eq!(owner("RustWTM-Disk-1234.99-names"), Some((1234, Some(99))));
        for odd in [
            "RustWTM-Disk",
            "RustWTM-Disk-",
            "RustWTM-Disk-x1",
            "RustWTM-Disk-1.",
            "RustWTM-Disk-.5",
            "RustWTM-Disk-1.x",
            "RustWTM-Disk-99999999999",
            "RustWTM-Disk2-5",
            "RustWTM-GUI-Disk-5",
        ] {
            assert_eq!(owner(odd), None, "{odd}");
        }
    }

    #[test]
    fn a_session_outlives_its_name_only_with_its_owner() {
        let name = owned_name("RustWTM-Disk");
        let (pid, started) = owner(&name, "RustWTM-Disk").unwrap();
        assert_eq!(pid, std::process::id());
        assert!(started.is_some(), "{name}");
        assert!(!owner_gone(pid, started));
        assert!(!owner_gone(pid, None));
        // The same PID, started at another time: a later process.
        assert!(owner_gone(pid, started.map(|t| t + 1)));
        assert!(owner_gone(0xffff_fff1, None));
    }

    #[test]
    fn properties_reserve_room_for_both_names() {
        let mut props = Properties::new();
        let total = props.buf.len() * size_of::<u64>();
        let p = props.get();
        assert_eq!(p.Wnode.BufferSize as usize, total);
        let header = size_of::<EVENT_TRACE_PROPERTIES>();
        assert_eq!(p.LoggerNameOffset as usize, header);
        assert!(p.LogFileNameOffset as usize + NAME_UNITS * 2 <= total);
        assert_eq!(props.logger_name(), "");
    }

    #[test]
    fn event_id_filter_matches_the_documented_layout() {
        let f = event_id_filter(&[10, 11]);
        let bytes: Vec<u8> = f.iter().flat_map(|u| u.to_le_bytes()).collect();
        // FilterIn, Reserved, Count (2 bytes), then the IDs.
        assert_eq!(bytes, [1, 0, 2, 0, 10, 0, 11, 0]);
    }

    #[test]
    fn this_process_is_running_and_a_bogus_pid_is_not() {
        assert!(process_running(std::process::id()));
        // PIDs are multiples of 4; an odd huge one never exists.
        assert!(!process_running(0xffff_fff1));
    }

    #[test]
    fn stopping_orphans_with_an_unused_prefix_is_harmless() {
        stop_orphans(&format!("RustWTM-test-none-{}", std::process::id()));
    }

    #[test]
    fn needs_rights_or_runs_a_session() {
        // Sessions of crashed earlier runs would use up the kernel's slots.
        stop_orphans("RustWTM-test-etw");
        let name = format!("RustWTM-test-etw-{}", std::process::id());
        let elevated = crate::shell::is_elevated().unwrap_or(false);
        match Session::start(&name) {
            Ok(session) => {
                assert!(elevated);
                assert!(session_names().contains(&name));
                let thread = session.consume(|_| {}).unwrap();
                drop(session);
                thread.join().unwrap();
                assert!(!session_names().contains(&name));
            }
            Err(e) => {
                if elevated {
                    panic!("elevated but could not start a session: {e}");
                }
                assert!(e.is_access_denied(), "{e}");
            }
        }
    }
}

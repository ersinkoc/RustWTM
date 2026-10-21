//! Who reads and writes which file: kernel disk I/O events (ETW, needs
//! administrator rights) joined with file-name events, like Resource
//! Monitor. Windows' per-process disk counters say how much, never where.
//!
//! A disk I/O event names the file object and the thread that issued the
//! request; thread events map threads to processes and file-name events map
//! file objects to NT paths, which are shown with drive letters.
//!
//! The kernel names a file object when it is created, so files opened
//! before tracing began arrive unnamed; a helper thread now and then starts
//! and stops a second session, whose stop lists every open file object, and
//! keeps the names still missing.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use windows::Win32::Storage::FileSystem::QueryDosDeviceW;
use windows::Win32::System::Diagnostics::Etw::{
    DiskIoGuid, EVENT_HEADER_FLAG_32_BIT_HEADER, EVENT_RECORD, EVENT_TRACE_FLAG_DISK_FILE_IO,
    EVENT_TRACE_FLAG_DISK_IO, EVENT_TRACE_FLAG_THREAD, FileIoGuid, ThreadGuid,
};
use windows::Win32::System::Threading::{
    GetProcessIdOfThread, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
};
use windows::core::PCWSTR;

use crate::etw::{self, Session, u32_at, u64_at};
use crate::handle::OwnedHandle;

const OPCODE_READ: u8 = 10;
const OPCODE_WRITE: u8 = 11;
const OPCODE_FILE_NAME: u8 = 0;
const OPCODE_FILE_CREATE: u8 = 32;
const OPCODE_FILE_DELETE: u8 = 35;
const OPCODE_FILE_RUNDOWN: u8 = 36;
const OPCODE_THREAD_START: u8 = 1;
const OPCODE_THREAD_RUNDOWN: u8 = 3;
/// Names kept before starting over; the rundown recovers what is in use.
const MAX_NAMES: usize = 250_000;
/// Threads remembered before starting over; lookups refill the map.
const MAX_THREADS: usize = 200_000;
/// Rundowns cost about a second of kernel time; never more often than this.
const RUNDOWN_EVERY: Duration = Duration::from_secs(15);
/// How often the helper thread looks for unnamed files.
const HELPER_TICK: Duration = Duration::from_secs(1);
/// How long a rundown session runs before it is stopped.
const RUNDOWN_SETTLE: Duration = Duration::from_millis(100);
/// `DiskIo` Read/Write payload: `TransferSize` and `FileObject` offsets.
const DISK_SIZE_AT: usize = 8;
const DISK_FILE_AT: usize = 24;

/// Disk bytes one process moved for one file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileIo {
    pub pid: u32,
    /// `C:\...`; `None` while the file's name is not known yet.
    pub path: Option<String>,
    pub read: u64,
    pub write: u64,
}

/// What the consumer callbacks gather and [`DiskTracer::take`] drains.
#[derive(Default)]
struct Ledger {
    /// (process, file object) -> (read, write) since the last take.
    bytes: HashMap<(u32, u64), (u64, u64)>,
    /// File object -> NT path.
    names: HashMap<u64, Arc<str>>,
    /// File objects closed since the last take; forgotten after it, so the
    /// bytes they moved in the meantime still get their name.
    closed: HashSet<u64>,
    /// Thread -> owning process.
    threads: HashMap<u32, u32>,
    /// File objects that moved bytes but have no name yet.
    wanted: HashSet<u64>,
    /// Wanted objects a rundown did not name; not asked for again.
    given_up: HashSet<u64>,
}

impl Ledger {
    fn name(&mut self, file: u64, path: Arc<str>, overwrite: bool) {
        if self.names.len() >= MAX_NAMES && !self.names.contains_key(&file) {
            self.names.clear();
        }
        self.wanted.remove(&file);
        self.given_up.remove(&file);
        if overwrite {
            self.closed.remove(&file);
            self.names.insert(file, path);
        } else {
            self.names.entry(file).or_insert(path);
        }
    }

    fn owner(&mut self, thread: u32) -> Option<u32> {
        if let Some(&pid) = self.threads.get(&thread) {
            return Some(pid);
        }
        let pid = process_of_thread(thread)?;
        self.thread_started(thread, pid);
        Some(pid)
    }

    fn thread_started(&mut self, thread: u32, pid: u32) {
        if self.threads.len() >= MAX_THREADS {
            self.threads.clear();
        }
        self.threads.insert(thread, pid);
    }

    fn moved(&mut self, pid: u32, file: u64, size: u64, write: bool) {
        let (read, written) = self.bytes.entry((pid, file)).or_default();
        let total = if write { written } else { read };
        *total = total.saturating_add(size);
        if file != 0 && !self.names.contains_key(&file) && !self.given_up.contains(&file) {
            self.wanted.insert(file);
        }
    }

    /// The rows since the last call, busiest first; forgets closed files.
    fn drain(&mut self, devices: &[(String, String)]) -> Vec<FileIo> {
        let mut rows: HashMap<FileKey, (u64, u64)> = HashMap::new();
        for ((pid, file), (read, write)) in self.bytes.drain() {
            let name = self.names.get(&file).cloned();
            let row = rows.entry((pid, name)).or_default();
            row.0 = row.0.saturating_add(read);
            row.1 = row.1.saturating_add(write);
        }
        for file in self.closed.drain() {
            self.names.remove(&file);
            self.wanted.remove(&file);
            self.given_up.remove(&file);
        }
        let mut out: Vec<FileIo> = rows
            .into_iter()
            .map(|((pid, name), (read, write))| FileIo {
                pid,
                path: name.map(|n| dos_path(&n, devices)),
                read,
                write,
            })
            .collect();
        out.sort_by(|a, b| {
            (b.read.saturating_add(b.write))
                .cmp(&a.read.saturating_add(a.write))
                .then(a.pid.cmp(&b.pid))
                .then_with(|| a.path.cmp(&b.path))
        });
        out
    }
}

/// A process and the file it used (unnamed while not known yet).
type FileKey = (u32, Option<Arc<str>>);

fn lock(ledger: &Mutex<Ledger>) -> std::sync::MutexGuard<'_, Ledger> {
    ledger.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The process a still-running thread belongs to.
fn process_of_thread(thread: u32) -> Option<u32> {
    // SAFETY: plain call; the handle is owned and closed on return.
    let h = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, thread) }.ok()?;
    let h = OwnedHandle::new(h);
    // SAFETY: `h` is a valid thread handle with query rights.
    let pid = unsafe { GetProcessIdOfThread(h.raw()) };
    (pid != 0).then_some(pid)
}

/// Bytes in a pointer field of this event (kernel events log native ones).
fn pointer_size(record: &EVENT_RECORD) -> usize {
    if u32::from(record.EventHeader.Flags) & EVENT_HEADER_FLAG_32_BIT_HEADER != 0 {
        4
    } else {
        8
    }
}

fn pointer_at(d: &[u8], at: usize, size: usize) -> Option<u64> {
    if size == 4 {
        u32_at(d, at).map(u64::from)
    } else {
        u64_at(d, at)
    }
}

/// `DiskIo` Read/Write: (file object, bytes, issuing thread). The thread
/// follows `FileObject`, `Irp` (two pointers) and `HighResResponseTime`.
fn disk_transfer(d: &[u8], ptr: usize) -> Option<(u64, u64, u32)> {
    let size = u32_at(d, DISK_SIZE_AT)?;
    let file = pointer_at(d, DISK_FILE_AT, ptr)?;
    let thread = u32_at(d, DISK_FILE_AT + 2 * ptr + 8)?;
    Some((file, u64::from(size), thread))
}

/// `FileIo` Name/Create/Rundown: (file object, NT path).
fn file_name(d: &[u8], ptr: usize) -> Option<(u64, String)> {
    let file = pointer_at(d, 0, ptr)?;
    let units: Vec<u16> = d
        .get(ptr..)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_le_bytes(c))
        .take_while(|&c| c != 0)
        .collect();
    if units.is_empty() {
        return None;
    }
    Some((file, String::from_utf16_lossy(&units)))
}

/// `Thread` Start/DCStart: (thread, process).
fn thread_owner(d: &[u8]) -> Option<(u32, u32)> {
    Some((u32_at(d, 4)?, u32_at(d, 0)?))
}

/// Feeds one event of the main session into the ledger.
fn record_event(ledger: &Mutex<Ledger>, record: &EVENT_RECORD) {
    let header = &record.EventHeader;
    let opcode = header.EventDescriptor.Opcode;
    let d = etw::user_data(record);
    let ptr = pointer_size(record);
    if header.ProviderId == DiskIoGuid {
        if opcode != OPCODE_READ && opcode != OPCODE_WRITE {
            return;
        }
        let Some((file, size, thread)) = disk_transfer(d, ptr) else {
            return;
        };
        let mut l = lock(ledger);
        let pid = match l.owner(thread) {
            Some(pid) => pid,
            None if header.ProcessId != u32::MAX => header.ProcessId,
            None => return,
        };
        l.moved(pid, file, size, opcode == OPCODE_WRITE);
    } else if header.ProviderId == FileIoGuid {
        match opcode {
            OPCODE_FILE_NAME | OPCODE_FILE_CREATE | OPCODE_FILE_RUNDOWN => {
                if let Some((file, path)) = file_name(d, ptr) {
                    lock(ledger).name(file, path.into(), opcode != OPCODE_FILE_RUNDOWN);
                }
            }
            OPCODE_FILE_DELETE => {
                if let Some(file) = pointer_at(d, 0, ptr) {
                    lock(ledger).closed.insert(file);
                }
            }
            _ => {}
        }
    } else if header.ProviderId == ThreadGuid
        && (opcode == OPCODE_THREAD_START || opcode == OPCODE_THREAD_RUNDOWN)
        && let Some((thread, pid)) = thread_owner(d)
    {
        lock(ledger).thread_started(thread, pid);
    }
}

/// Runs a short second session; its stop rundown names the wanted files.
fn rundown(session_name: &str, ledger: &Arc<Mutex<Ledger>>) {
    let flags = EVENT_TRACE_FLAG_DISK_IO.0 | EVENT_TRACE_FLAG_DISK_FILE_IO.0;
    let Ok(session) = Session::start_kernel(session_name, flags) else {
        return;
    };
    let sink = Arc::clone(ledger);
    let consumer = session.consume(move |record| {
        let header = &record.EventHeader;
        if header.ProviderId != FileIoGuid {
            return;
        }
        let opcode = header.EventDescriptor.Opcode;
        if !matches!(
            opcode,
            OPCODE_FILE_NAME | OPCODE_FILE_CREATE | OPCODE_FILE_RUNDOWN
        ) {
            return;
        }
        let Some((file, path)) = file_name(etw::user_data(record), pointer_size(record)) else {
            return;
        };
        let mut l = lock(&sink);
        if l.wanted.contains(&file) {
            l.name(file, path.into(), false);
        }
    });
    std::thread::sleep(RUNDOWN_SETTLE);
    drop(session);
    if let Ok(consumer) = consumer {
        let _ = consumer.join();
    }
    let mut l = lock(ledger);
    let left: Vec<u64> = l.wanted.drain().collect();
    if l.given_up.len() + left.len() > MAX_NAMES {
        l.given_up.clear();
    }
    l.given_up.extend(left);
}

/// Waits for wanted names and runs rundowns, until `stop` hangs up.
fn helper(session_name: String, ledger: Arc<Mutex<Ledger>>, stop: mpsc::Receiver<()>) {
    let mut last: Option<Instant> = None;
    loop {
        match stop.recv_timeout(HELPER_TICK) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
        }
        if last.is_some_and(|t| t.elapsed() < RUNDOWN_EVERY) {
            continue;
        }
        if lock(&ledger).wanted.is_empty() {
            continue;
        }
        last = Some(Instant::now());
        rundown(&session_name, &ledger);
    }
}

/// (NT device, drive) pairs such as (`\Device\HarddiskVolume3`, `C:`).
fn dos_devices() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut target = vec![0u16; 1024];
    for letter in b'A'..=b'Z' {
        let drive = format!("{}:", char::from(letter));
        let name: Vec<u16> = drive.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `name` is NUL-terminated and `target` is a writable buffer
        // whose length is passed along.
        let len = unsafe { QueryDosDeviceW(PCWSTR(name.as_ptr()), Some(&mut target)) } as usize;
        if len == 0 {
            continue;
        }
        let first = &target[..len.min(target.len())];
        let end = first.iter().position(|&c| c == 0).unwrap_or(first.len());
        if end > 0 {
            out.push((String::from_utf16_lossy(&first[..end]), drive));
        }
    }
    out
}

/// `\Device\HarddiskVolume3\x` -> `C:\x` given `devices`; other paths as is.
fn dos_path(nt: &str, devices: &[(String, String)]) -> String {
    if let Some(rest) = nt.strip_prefix(r"\??\") {
        return rest.to_string();
    }
    for (device, drive) in devices {
        let Some(head) = nt.get(..device.len()) else {
            continue;
        };
        let rest = &nt[device.len()..];
        if head.eq_ignore_ascii_case(device) && (rest.is_empty() || rest.starts_with('\\')) {
            return format!("{drive}{rest}");
        }
    }
    nt.to_string()
}

/// Counts disk bytes per process and file while it lives.
pub struct DiskTracer {
    ledger: Arc<Mutex<Ledger>>,
    /// Dropping it tells the helper thread to finish.
    stop: Sender<()>,
    devices: Vec<(String, String)>,
    names_session: String,
    _session: Session,
}

impl std::fmt::Debug for DiskTracer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskTracer")
            .field("session", &self._session)
            .field("names_session", &self.names_session)
            .finish_non_exhaustive()
    }
}

impl DiskTracer {
    /// `name` names the kernel session; a second one, `{name}-names`,
    /// briefly runs for file names now and then.
    pub fn start(name: &str) -> crate::Result<Self> {
        let flags = EVENT_TRACE_FLAG_DISK_IO.0
            | EVENT_TRACE_FLAG_DISK_FILE_IO.0
            | EVENT_TRACE_FLAG_THREAD.0;
        let session = Session::start_kernel(name, flags)?;
        let ledger = Arc::new(Mutex::new(Ledger::default()));
        let sink = Arc::clone(&ledger);
        session.consume(move |record| record_event(&sink, record))?;

        let names_session = format!("{name}-names");
        let (stop, stopped) = mpsc::channel();
        let helper_name = names_session.clone();
        let helper_ledger = Arc::clone(&ledger);
        // Without the helper only names of files opened from now on are
        // known; tracing still works, so a failed spawn is not an error.
        let _ = std::thread::Builder::new()
            .name("disk-trace-names".into())
            .spawn(move || helper(helper_name, helper_ledger, stopped));

        Ok(Self {
            ledger,
            stop,
            devices: dos_devices(),
            names_session,
            _session: session,
        })
    }

    /// Bytes per process and file since the previous call, largest first.
    pub fn take(&self) -> Vec<FileIo> {
        lock(&self.ledger).drain(&self.devices)
    }

    /// The name of the short-lived second session that collects file names.
    pub fn names_session(&self) -> &str {
        &self.names_session
    }
}

impl Drop for DiskTracer {
    fn drop(&mut self) {
        // Dropping the sender wakes the helper, which then exits instead of
        // starting another rundown. One already running is cut short here
        // rather than left to finish on its own.
        let (hang_up, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.stop, hang_up));
        etw::stop_named(&self.names_session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices() -> Vec<(String, String)> {
        vec![
            (r"\Device\HarddiskVolume1".into(), "D:".into()),
            (r"\Device\HarddiskVolume10".into(), "C:".into()),
        ]
    }

    #[test]
    fn converts_device_paths_to_drive_letters() {
        let d = devices();
        assert_eq!(
            dos_path(r"\Device\HarddiskVolume10\Windows\a.dll", &d),
            r"C:\Windows\a.dll"
        );
        assert_eq!(dos_path(r"\device\harddiskvolume1\x", &d), r"D:\x");
        assert_eq!(dos_path(r"\Device\HarddiskVolume1", &d), "D:");
        assert_eq!(
            dos_path(r"\Device\HarddiskVolume2\x", &d),
            r"\Device\HarddiskVolume2\x"
        );
        assert_eq!(dos_path(r"\??\C:\pagefile.sys", &d), r"C:\pagefile.sys");
        assert_eq!(dos_path("", &d), "");
    }

    fn disk_payload(size: u32, file: u64, thread: u32) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&0u32.to_le_bytes()); // DiskNumber
        d.extend_from_slice(&0u32.to_le_bytes()); // IrpFlags
        d.extend_from_slice(&size.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes()); // Reserved
        d.extend_from_slice(&0u64.to_le_bytes()); // ByteOffset
        d.extend_from_slice(&file.to_le_bytes());
        d.extend_from_slice(&0u64.to_le_bytes()); // Irp
        d.extend_from_slice(&0u64.to_le_bytes()); // HighResResponseTime
        d.extend_from_slice(&thread.to_le_bytes());
        d
    }

    #[test]
    fn parses_disk_name_and_thread_payloads() {
        let d = disk_payload(4096, 0xffff_a000_0000_1230, 77);
        assert_eq!(
            disk_transfer(&d, 8),
            Some((0xffff_a000_0000_1230, 4096, 77))
        );
        assert_eq!(disk_transfer(&d[..40], 8), None);

        let mut n = 0x1234u32.to_le_bytes().to_vec();
        n.extend(r"\Device\X\a".encode_utf16().flat_map(u16::to_le_bytes));
        n.extend_from_slice(&[0, 0, 0x41, 0]);
        assert_eq!(file_name(&n, 4), Some((0x1234, r"\Device\X\a".to_string())));
        assert_eq!(file_name(&n[..4], 4), None);

        let t = [5, 0, 0, 0, 9, 0, 0, 0];
        assert_eq!(thread_owner(&t), Some((9, 5)));
    }

    #[test]
    fn drains_rows_merged_by_name_and_sorted() {
        let mut l = Ledger::default();
        l.thread_started(1, 100);
        l.name(0xa, r"\Device\HarddiskVolume10\big".into(), true);
        l.name(0xb, r"\Device\HarddiskVolume10\big".into(), true);
        l.name(0xc, r"\Device\HarddiskVolume1\small".into(), true);
        l.moved(100, 0xa, 1000, false);
        l.moved(100, 0xb, 500, true);
        l.moved(100, 0xc, 10, true);
        l.moved(200, 0xd, 20, false);
        assert!(l.wanted.contains(&0xd));
        l.closed.insert(0xc);

        let rows = l.drain(&devices());
        assert_eq!(
            rows,
            vec![
                FileIo {
                    pid: 100,
                    path: Some(r"C:\big".into()),
                    read: 1000,
                    write: 500
                },
                FileIo {
                    pid: 200,
                    path: None,
                    read: 20,
                    write: 0
                },
                FileIo {
                    pid: 100,
                    path: Some(r"D:\small".into()),
                    read: 0,
                    write: 10
                },
            ]
        );
        assert!(!l.names.contains_key(&0xc));
        assert!(l.drain(&devices()).is_empty());
    }

    #[test]
    fn rundown_names_do_not_replace_known_ones() {
        let mut l = Ledger::default();
        l.name(1, "new".into(), true);
        l.name(1, "old".into(), false);
        assert_eq!(&*l.names[&1], "new");
        l.wanted.insert(2);
        l.name(2, "found".into(), false);
        assert!(l.wanted.is_empty());
    }

    #[test]
    fn this_threads_process_is_found() {
        // SAFETY: plain call.
        let me = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        assert_eq!(process_of_thread(me), Some(std::process::id()));
    }

    #[test]
    fn system_drive_has_a_device() {
        let devices = dos_devices();
        assert!(
            devices
                .iter()
                .any(|(dev, drive)| { drive == "C:" && dev.starts_with(r"\Device\") })
        );
    }

    /// Needs an elevated test run; otherwise checks the error.
    #[test]
    fn traces_a_file_we_write_when_elevated() {
        // Sessions of crashed earlier runs would use up the kernel's slots.
        crate::etw::stop_orphans("RustWTM-test-disk");
        let name = format!("RustWTM-test-disk-{}", std::process::id());
        let elevated = crate::shell::is_elevated().unwrap_or(false);
        let tracer = match DiskTracer::start(&name) {
            Ok(t) => t,
            Err(e) => {
                if elevated {
                    panic!("elevated but could not trace: {e}");
                }
                assert!(e.is_access_denied(), "{e}");
                return;
            }
        };
        assert_eq!(tracer.names_session(), format!("{name}-names"));
        let path =
            std::env::temp_dir().join(format!("rustwtm-disktrace-{}.bin", std::process::id()));
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(&vec![7u8; 4 << 20]).unwrap();
            f.sync_all().unwrap();
        }
        let me = std::process::id();
        let wanted = path.to_string_lossy().to_lowercase();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut found = false;
        while !found && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(500));
            found = tracer.take().iter().any(|r| {
                r.pid == me
                    && r.write > 0
                    && r.path
                        .as_deref()
                        .is_some_and(|p| p.to_lowercase() == wanted)
            });
        }
        drop(tracer);
        let _ = std::fs::remove_file(&path);
        assert!(found, "no write seen for {wanted}");
    }
}

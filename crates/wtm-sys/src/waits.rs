//! Why a program does not respond.
//!
//! - [`hung_windows`]: top-level windows Windows itself marks as not
//!   responding (their thread took no message for five seconds). It reads
//!   window state only and opens no process, so it is cheap enough for every
//!   sampling tick.
//! - [`wait_chains`]: what the blocked threads of a process wait for, link by
//!   link and across processes (Wait Chain Traversal, the engine behind
//!   Resource Monitor's "Analyze wait chain"). Run it on request only.
//!
//! Wait Chain Traversal can crash the calling process when threads of the
//! process it walks come and go meanwhile, which is what our own process
//! does all the time. So it never walks our own process, and only one walk
//! runs at a time.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Diagnostics::Debug::{
    CloseThreadWaitChainSession, GetThreadWaitChain, OPEN_THREAD_WAIT_CHAIN_SESSION_FLAGS,
    OpenThreadWaitChainSession, WAIT_CHAIN_THREAD_OPTIONS, WAITCHAIN_NODE_INFO, WCT_MAX_NODE_COUNT,
    WCT_NETWORK_IO_FLAG, WCT_OBJECT_STATUS, WCT_OBJECT_TYPE, WCT_OUT_OF_PROC_CS_FLAG,
    WCT_OUT_OF_PROC_FLAG, WctAlpcType, WctComActivationType, WctComType, WctCriticalSectionType,
    WctMutexType, WctProcessWaitType, WctSendMessageType, WctSmbIoType, WctSocketIoType,
    WctStatusAbandoned, WctStatusBlocked, WctStatusNoAccess, WctStatusOwned, WctStatusRunning,
    WctThreadType, WctThreadWaitType,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_STYLE, GetWindow, GetWindowLongPtrW, GetWindowTextW,
    GetWindowThreadProcessId, IsHungAppWindow, IsWindowVisible, WS_CAPTION,
};
use windows::core::BOOL;

use crate::process::ProcessSnapshotter;

/// A window Windows marks as not responding.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HungWindow {
    pub pid: u32,
    pub tid: u32,
    pub title: String,
}

/// What a link of a chain waits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WaitKind {
    CriticalSection,
    SendMessage,
    Mutex,
    Alpc,
    Com,
    Thread,
    Process,
    ComActivation,
    Socket,
    Smb,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LinkStatus {
    Running,
    Blocked,
    Owned,
    Abandoned,
    NoAccess,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Link {
    Thread {
        pid: u32,
        tid: u32,
        status: LinkStatus,
    },
    Object {
        kind: WaitKind,
        name: String,
        status: LinkStatus,
    },
}

/// One thread's chain: the thread, what it waits on, who holds that, ...
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Chain {
    pub links: Vec<Link>,
    /// The chain loops back on itself: a deadlock.
    pub cycle: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WaitReport {
    /// Chains of blocked threads, deadlocks first, longest next.
    pub chains: Vec<Chain>,
    pub threads: usize,
    /// Threads asked about (blocked ones); the rest sleep or are idle.
    pub analyzed: usize,
    /// Threads Windows could not analyze in time.
    pub skipped: usize,
}

impl WaitReport {
    pub fn deadlocked(&self) -> bool {
        self.chains.iter().any(|c| c.cycle)
    }
}

/// Unowned application windows Windows marks as not responding, one per thread.
pub fn hung_windows() -> Vec<HungWindow> {
    let mut found: Vec<HungWindow> = Vec::new();
    // SAFETY: `collect` only borrows the `Vec` behind the pointer we pass and
    // runs synchronously for the duration of the call.
    unsafe {
        let _ = EnumWindows(
            Some(collect),
            LPARAM(&mut found as *mut Vec<HungWindow> as isize),
        );
    }
    // A program can own several hung windows on one thread; report it once.
    let mut seen = std::collections::HashSet::new();
    found.retain(|w| seen.insert(w.tid));
    found
}

/// The window enumeration callback: keep visible, unowned, titled application
/// windows that Windows reports as not responding.
unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` carries the `&mut Vec` set up by `hung_windows`, which
    // outlives the enumeration; the window handle is valid for query here.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }
        // A null owner means a genuine top-level window, not a dialog or popup.
        if !GetWindow(hwnd, GW_OWNER).unwrap_or_default().is_invalid() {
            return true.into();
        }
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        if style & WS_CAPTION.0 == 0 {
            return true.into();
        }
        if !IsHungAppWindow(hwnd).as_bool() {
            return true.into();
        }
        let mut pid = 0u32;
        let tid = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return true.into();
        }
        let mut text = [0u16; 256];
        let len = GetWindowTextW(hwnd, &mut text).max(0) as usize;
        let title = String::from_utf16_lossy(&text[..len]);
        let out = &mut *(lparam.0 as *mut Vec<HungWindow>);
        out.push(HungWindow { pid, tid, title });
    }
    true.into()
}

/// What the blocked threads of `pid` wait for. Windows has no timeout for a
/// chain that leads into a process that does not answer, so the analysis
/// runs on its own thread and gives up after `budget`.
pub fn wait_chains(pid: u32, budget: Duration) -> crate::Result<WaitReport> {
    if pid == std::process::id() {
        return Err(windows::core::Error::new(
            windows::Win32::Foundation::E_INVALIDARG,
            "RustWTM does not analyze its own wait chains",
        )
        .into());
    }
    let progress = Arc::new(Mutex::new(Progress::default()));
    let (tx, rx) = mpsc::channel();
    let worker = Arc::clone(&progress);
    // Detached on purpose: a GetThreadWaitChain call into a frozen process can
    // block forever, and abandoning this thread is how we honour `budget`.
    std::thread::spawn(move || {
        let report = analyze(pid, &worker);
        let _ = tx.send(report);
    });

    match rx.recv_timeout(budget) {
        Ok(report) => Ok(report),
        // The worker is still (or no longer) running; report what it reached.
        Err(RecvTimeoutError::Timeout) => Ok(progress.lock().unwrap().finish(false)),
        Err(RecvTimeoutError::Disconnected) => Ok(progress.lock().unwrap().finish(false)),
    }
}

/// Counts and chains gathered so far, shared with the caller so a partial
/// answer survives a timeout.
#[derive(Default)]
struct Progress {
    threads: usize,
    blocked: usize,
    /// Blocked threads not yet looked at; counted as skipped on a timeout.
    pending: usize,
    skipped: usize,
    chains: Vec<Chain>,
}

impl Progress {
    fn finish(&self, complete: bool) -> WaitReport {
        let mut chains = self.chains.clone();
        // Deadlocks first, then the chains with the most links.
        chains.sort_by(|a, b| {
            b.cycle
                .cmp(&a.cycle)
                .then(b.links.len().cmp(&a.links.len()))
        });
        let skipped = self.skipped + if complete { 0 } else { self.pending };
        WaitReport {
            chains,
            threads: self.threads,
            analyzed: self.blocked,
            skipped,
        }
    }
}

/// Runs the wait-chain traversal for every blocked thread of `pid`, updating
/// `progress` as it goes.
fn analyze(pid: u32, progress: &Mutex<Progress>) -> WaitReport {
    // One walk at a time in this process (see the module notes). A walk stuck
    // in a frozen process keeps the next one waiting, and that one's caller
    // then gets an empty, incomplete report when its budget runs out.
    static WALK: Mutex<()> = Mutex::new(());
    let _walk = WALK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let threads = ProcessSnapshotter::new().threads(pid).unwrap_or_default();
    let blocked: Vec<u32> = threads
        .iter()
        .filter(|t| t.blocked())
        .map(|t| t.tid)
        .collect();
    {
        let mut p = progress.lock().unwrap();
        p.threads = threads.len();
        p.blocked = blocked.len();
        p.pending = blocked.len();
    }

    // A synchronous session (no flags, no callback): every query blocks until
    // it has an answer, which is why this runs off the caller's thread.
    // SAFETY: plain FFI; the returned session is closed below.
    let session =
        unsafe { OpenThreadWaitChainSession(OPEN_THREAD_WAIT_CHAIN_SESSION_FLAGS(0), None) };
    if session.is_null() {
        return progress.lock().unwrap().finish(true);
    }

    // Follow critical sections, ALPC ports and socket waits across processes.
    let flags = WAIT_CHAIN_THREAD_OPTIONS(
        WCT_OUT_OF_PROC_FLAG.0 | WCT_OUT_OF_PROC_CS_FLAG.0 | WCT_NETWORK_IO_FLAG,
    );
    for tid in blocked {
        let chain = chain_for(session, flags, tid);
        let mut p = progress.lock().unwrap();
        match chain {
            // A lone node is just the thread itself: it waits on nothing worth
            // charting.
            Some(c) if c.links.len() > 1 => p.chains.push(c),
            Some(_) => {}
            None => p.skipped += 1,
        }
        p.pending = p.pending.saturating_sub(1);
    }

    // SAFETY: `session` came from OpenThreadWaitChainSession and is unused after.
    unsafe { CloseThreadWaitChainSession(session) };
    progress.lock().unwrap().finish(true)
}

/// One thread's wait chain, or `None` when Windows could not walk it.
fn chain_for(
    session: *mut core::ffi::c_void,
    flags: WAIT_CHAIN_THREAD_OPTIONS,
    tid: u32,
) -> Option<Chain> {
    let mut nodes = [WAITCHAIN_NODE_INFO::default(); WCT_MAX_NODE_COUNT as usize];
    let mut count = WCT_MAX_NODE_COUNT;
    let mut is_cycle = BOOL::default();
    // SAFETY: `nodes` holds `count` writable entries; the other out-pointers
    // reference live locals; `session` is an open WCT session.
    let result = unsafe {
        GetThreadWaitChain(
            session,
            None,
            flags,
            tid,
            &mut count,
            nodes.as_mut_ptr(),
            &mut is_cycle,
        )
    };
    result.ok()?;
    let count = (count as usize).min(nodes.len());
    let links = nodes[..count].iter().map(node_to_link).collect();
    Some(Chain {
        links,
        cycle: is_cycle.as_bool(),
    })
}

fn node_to_link(node: &WAITCHAIN_NODE_INFO) -> Link {
    let status = link_status(node.ObjectStatus);
    if node.ObjectType == WctThreadType {
        // SAFETY: a thread node selects the ThreadObject arm of the union.
        let t = unsafe { node.Anonymous.ThreadObject };
        Link::Thread {
            pid: t.ProcessId,
            tid: t.ThreadId,
            status,
        }
    } else {
        // SAFETY: any other object type selects the LockObject arm.
        let lock = unsafe { node.Anonymous.LockObject };
        Link::Object {
            kind: wait_kind(node.ObjectType),
            name: wide_to_string(&lock.ObjectName),
            status,
        }
    }
}

fn wait_kind(kind: WCT_OBJECT_TYPE) -> WaitKind {
    match kind {
        t if t == WctCriticalSectionType => WaitKind::CriticalSection,
        t if t == WctSendMessageType => WaitKind::SendMessage,
        t if t == WctMutexType => WaitKind::Mutex,
        t if t == WctAlpcType => WaitKind::Alpc,
        t if t == WctComType => WaitKind::Com,
        t if t == WctThreadWaitType => WaitKind::Thread,
        t if t == WctProcessWaitType => WaitKind::Process,
        t if t == WctComActivationType => WaitKind::ComActivation,
        t if t == WctSocketIoType => WaitKind::Socket,
        t if t == WctSmbIoType => WaitKind::Smb,
        _ => WaitKind::Unknown,
    }
}

fn link_status(status: WCT_OBJECT_STATUS) -> LinkStatus {
    match status {
        s if s == WctStatusRunning => LinkStatus::Running,
        s if s == WctStatusBlocked => LinkStatus::Blocked,
        s if s == WctStatusOwned => LinkStatus::Owned,
        s if s == WctStatusAbandoned => LinkStatus::Abandoned,
        s if s == WctStatusNoAccess => LinkStatus::NoAccess,
        _ => LinkStatus::Other,
    }
}

/// A fixed-width, NUL-terminated UTF-16 object name as a `String`.
fn wide_to_string(name: &[u16]) -> String {
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    String::from_utf16_lossy(&name[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadlock_is_reported_when_any_chain_cycles() {
        let thread = |tid| Link::Thread {
            pid: 1,
            tid,
            status: LinkStatus::Blocked,
        };
        let plain = WaitReport {
            chains: vec![Chain {
                links: vec![thread(1), thread(2)],
                cycle: false,
            }],
            threads: 4,
            analyzed: 1,
            skipped: 0,
        };
        assert!(!plain.deadlocked());

        let looped = WaitReport {
            chains: vec![Chain {
                links: vec![thread(1), thread(2), thread(1)],
                cycle: true,
            }],
            ..plain
        };
        assert!(looped.deadlocked());
    }

    #[test]
    fn object_and_status_codes_map_to_our_enums() {
        assert_eq!(wait_kind(WctMutexType), WaitKind::Mutex);
        assert_eq!(wait_kind(WctAlpcType), WaitKind::Alpc);
        assert_eq!(wait_kind(WctComActivationType), WaitKind::ComActivation);
        assert_eq!(wait_kind(WctSocketIoType), WaitKind::Socket);
        assert_eq!(wait_kind(WCT_OBJECT_TYPE(0)), WaitKind::Unknown);

        assert_eq!(link_status(WctStatusBlocked), LinkStatus::Blocked);
        assert_eq!(link_status(WctStatusOwned), LinkStatus::Owned);
        assert_eq!(link_status(WctStatusAbandoned), LinkStatus::Abandoned);
        assert_eq!(link_status(WCT_OBJECT_STATUS(0)), LinkStatus::Other);
    }

    #[test]
    fn a_fixed_width_name_stops_at_the_terminator() {
        let mut buf = [0u16; 128];
        for (slot, ch) in buf.iter_mut().zip("Lock".encode_utf16()) {
            *slot = ch;
        }
        assert_eq!(wide_to_string(&buf), "Lock");
        assert_eq!(wide_to_string(&[0u16; 8]), "");
    }

    #[test]
    fn chains_sort_deadlocks_then_length() {
        let link = Link::Thread {
            pid: 1,
            tid: 1,
            status: LinkStatus::Blocked,
        };
        let progress = Progress {
            threads: 10,
            blocked: 3,
            pending: 0,
            skipped: 1,
            chains: vec![
                Chain {
                    links: vec![link.clone(), link.clone()],
                    cycle: false,
                },
                Chain {
                    links: vec![link.clone(), link.clone(), link.clone(), link.clone()],
                    cycle: false,
                },
                Chain {
                    links: vec![link.clone(), link.clone()],
                    cycle: true,
                },
            ],
        };
        let report = progress.finish(true);
        assert!(report.chains[0].cycle, "deadlock first");
        assert_eq!(report.chains[1].links.len(), 4, "then the longest");
        assert_eq!(report.skipped, 1);
    }

    #[test]
    fn a_timeout_counts_the_unfinished_threads_as_skipped() {
        let progress = Progress {
            threads: 8,
            blocked: 5,
            pending: 2,
            skipped: 1,
            chains: Vec::new(),
        };
        assert_eq!(progress.finish(false).skipped, 3);
        assert_eq!(progress.finish(true).skipped, 1);
    }

    #[test]
    fn hung_windows_runs_and_reports_well_formed_entries() {
        // We cannot force a hang here, but the scan must never panic and must
        // never yield a zero pid or tid or a duplicate thread.
        let windows = hung_windows();
        let mut tids = std::collections::HashSet::new();
        for w in &windows {
            assert!(w.pid != 0 && w.tid != 0);
            assert!(tids.insert(w.tid), "one entry per thread");
        }
    }

    /// A child process whose threads sit waiting; ended when dropped.
    struct Waiting(std::process::Child);

    impl Drop for Waiting {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn wait_chains_analyzes_a_waiting_process() {
        let child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .map(Waiting)
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let report = wait_chains(child.0.id(), Duration::from_secs(10)).unwrap();
        assert!(report.threads >= 1, "at least its main thread");
        assert!(report.analyzed <= report.threads);
        // Every chain starts at a thread of some process.
        for chain in &report.chains {
            assert!(matches!(chain.links.first(), Some(Link::Thread { .. })));
        }
    }

    #[test]
    fn our_own_process_is_refused() {
        assert!(wait_chains(std::process::id(), Duration::from_secs(1)).is_err());
    }

    #[test]
    fn wait_chains_of_a_dead_pid_is_empty() {
        let report = wait_chains(0xffff_fffe, Duration::from_secs(5)).unwrap();
        assert_eq!(report.threads, 0);
        assert!(report.chains.is_empty());
    }
}

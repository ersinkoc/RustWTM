//! Why a process does not go away. Task Manager reports all of these as
//! "access is denied":
//! - It is already ending, but Windows cannot remove it while other
//!   processes hold handles to it.
//! - Windows protects it (a protected process), or a driver strips the right
//!   to end it from every handle (anti-cheat and security software).
//! - Ending it would crash Windows (a critical process).
//!

use std::collections::HashMap;
use std::ffi::c_void;

use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS};
use windows::Win32::Foundation::{HANDLE, NTSTATUS, STATUS_INFO_LENGTH_MISMATCH};
use windows::Win32::System::ProcessStatus::{EnumDeviceDrivers, GetDeviceDriverBaseNameW};

use crate::control::{Access, ProcessHandle};
use crate::process::ProcessSnapshotter;
use crate::{Error, Result};

windows::core::link!("ntdll.dll" "system" fn NtQueryInformationProcess(
    process_handle: HANDLE,
    class: u32,
    info: *mut core::ffi::c_void,
    len: u32,
    returned: *mut u32,
) -> NTSTATUS);

const SYSTEM_EXTENDED_HANDLE_INFORMATION: SYSTEM_INFORMATION_CLASS = SYSTEM_INFORMATION_CLASS(64);
const PROCESS_BASIC_INFORMATION: u32 = 0;
const PROCESS_PROTECTION_INFORMATION: u32 = 61;
const STILL_ACTIVE: i32 = 259;

/// `PROCESS_EXTENDED_BASIC_INFORMATION.Flags` bit for a process Windows has
/// begun tearing down.
const FLAG_IS_DELETING: u32 = 0x0000_0004;

/// `PROCESS_EXTENDED_BASIC_INFORMATION`: the basic information plus flags.
#[repr(C)]
#[derive(Default)]
#[allow(dead_code)]
struct ExtendedBasic {
    size: usize,
    exit_status: i32,
    peb: usize,
    affinity: usize,
    base_priority: i32,
    pid: usize,
    parent: usize,
    flags: u32,
}

#[repr(C)]
struct HandleTableHeader {
    count: usize,
    reserved: usize,
}

/// `SYSTEM_HANDLE_TABLE_ENTRY_INFO_EX`.
#[repr(C)]
#[allow(dead_code)]
struct HandleEntry {
    object: usize,
    pid: usize,
    value: usize,
    access: u32,
    back_trace: u16,
    type_index: u16,
    attributes: u32,
    reserved: u32,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<HandleEntry>() == 40);

/// A process holding handles to the one asked about.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Holder {
    pub pid: u32,
    pub name: String,
    pub handles: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EndReport {
    /// Windows has begun ending it.
    pub deleting: bool,
    /// It has an exit code: it has ended and only its object remains.
    pub exited: bool,
    pub threads: u32,
    /// Windows protects it, and how ("light protection, Antimalware").
    pub protection: Option<String>,
    /// Ending it would stop Windows with a blue screen.
    pub critical: bool,
    /// Whether a handle opened to end it came with that right; `None` when
    /// it could not be opened at all (`open_error`).
    pub can_end: Option<bool>,
    pub open_error: Option<String>,
    /// Other processes with handles to it; while they hold them, Windows
    /// cannot remove it. Empty without administrator rights on recent
    /// Windows, which hides object addresses.
    pub holders: Vec<Holder>,
    /// Whether the handle table showed object addresses at all.
    pub holders_known: bool,
    /// Loaded drivers known to withhold the right to end processes.
    pub guards: Vec<(String, String)>,
}

/// What kind of software a guarding driver belongs to.
#[derive(Clone, Copy)]
enum Kind {
    AntiCheat,
    Security,
}

/// Kernel drivers known to withhold the right to end the processes they
/// guard: driver file name prefix, product and kind.
const GUARDS: &[(&str, &str, Kind)] = &[
    ("vgk", "Riot Vanguard", Kind::AntiCheat),
    ("easyanticheat", "Easy Anti-Cheat", Kind::AntiCheat),
    ("bedaisy", "BattlEye", Kind::AntiCheat),
    ("faceit", "FACEIT", Kind::AntiCheat),
    ("eaanticheat", "EA Javelin", Kind::AntiCheat),
    ("ace-", "Tencent ACE", Kind::AntiCheat),
    ("mhyprot", "HoYoverse mhyprot", Kind::AntiCheat),
    ("xhunter", "XIGNCODE3", Kind::AntiCheat),
    ("npptnt", "nProtect GameGuard", Kind::AntiCheat),
    ("csagent", "CrowdStrike Falcon", Kind::Security),
    ("sentinelmonitor", "SentinelOne", Kind::Security),
    ("sophosed", "Sophos Endpoint", Kind::Security),
    ("klif", "Kaspersky", Kind::Security),
    ("ahnrghlh", "AhnLab", Kind::Security),
];

/// "Riot Vanguard anti-cheat", "Kaspersky security software".
fn product_label(product: &str, kind: Kind) -> String {
    match kind {
        Kind::AntiCheat if product.contains("Anti-Cheat") => product.to_owned(),
        Kind::AntiCheat => format!("{product} anti-cheat"),
        Kind::Security => format!("{product} security software"),
    }
}

/// What a `PS_PROTECTION` byte means.
pub fn protection_label(protection: u8) -> String {
    // Bits 0-2 are the protection level, bits 4-7 the signer.
    let level = match protection & 0x07 {
        1 => "light protection",
        2 => "full protection",
        _ => "no protection",
    };
    let signer = match (protection >> 4) & 0x0f {
        1 => "Authenticode",
        2 => "CodeGen",
        3 => "Antimalware",
        4 => "Lsa",
        5 => "Windows",
        6 => "WinTcb",
        7 => "WinSystem",
        8 => "App",
        _ => "",
    };
    if signer.is_empty() {
        level.to_owned()
    } else {
        format!("{level}, {signer}")
    }
}

/// Everything that explains why `pid` is still there. Opens it twice (a
/// limited query, and once with the right to end it, which is only checked,
/// never used) and reads the system handle table.
pub fn why_alive(pid: u32) -> crate::Result<EndReport> {
    let mut report = EndReport {
        deleting: false,
        exited: false,
        threads: 0,
        protection: None,
        critical: false,
        can_end: None,
        open_error: None,
        holders: Vec::new(),
        holders_known: false,
        guards: loaded_guards().unwrap_or_default(),
    };

    // The thread count comes from the system snapshot, so even a process we
    // cannot open still reports one.
    report.threads = ProcessSnapshotter::new()
        .threads(pid)
        .map(|t| t.len() as u32)
        .unwrap_or(0);

    let query = ProcessHandle::open(pid, Access::QUERY);
    if let Ok(handle) = &query {
        if let Ok(basic) = extended_basic(handle) {
            report.deleting = basic.flags & FLAG_IS_DELETING != 0;
            report.exited = basic.exit_status != STILL_ACTIVE;
        }
        report.critical = handle.is_critical().unwrap_or(false);
        report.protection = protection(handle);
        let (holders, known) = holders_of(handle, pid);
        report.holders = holders;
        report.holders_known = known;
    }

    // The right to end a process can be stripped from the handle even when the
    // process opens fine, which is exactly what anti-cheat drivers do.
    let terminate = ProcessHandle::open(pid, Access::TERMINATE);
    match (&query, &terminate) {
        (_, Ok(_)) => report.can_end = Some(true),
        (Ok(_), Err(_)) => report.can_end = Some(false),
        (Err(e), Err(_)) => {
            report.can_end = None;
            report.open_error = Some(e.to_string());
        }
    }

    Ok(report)
}

/// `PROCESS_EXTENDED_BASIC_INFORMATION` for an opened process.
fn extended_basic(handle: &ProcessHandle) -> Result<ExtendedBasic> {
    let mut info = ExtendedBasic {
        size: size_of::<ExtendedBasic>(),
        ..Default::default()
    };
    let mut returned = 0u32;
    // SAFETY: `info` is a correctly sized PROCESS_EXTENDED_BASIC_INFORMATION
    // and outlives the call; the handle carries query rights.
    let status = unsafe {
        NtQueryInformationProcess(
            handle.raw(),
            PROCESS_BASIC_INFORMATION,
            (&raw mut info).cast(),
            size_of::<ExtendedBasic>() as u32,
            &mut returned,
        )
    };
    nt_ok("NtQueryInformationProcess", status)?;
    Ok(info)
}

/// The protection label when Windows protects the process, else `None`.
fn protection(handle: &ProcessHandle) -> Option<String> {
    let mut byte = 0u8;
    let mut returned = 0u32;
    // SAFETY: PS_PROTECTION is a single byte; `byte` is writable.
    let status = unsafe {
        NtQueryInformationProcess(
            handle.raw(),
            PROCESS_PROTECTION_INFORMATION,
            (&raw mut byte).cast(),
            1,
            &mut returned,
        )
    };
    (status.is_ok() && byte & 0x07 != 0).then(|| protection_label(byte))
}

/// Processes other than the caller and the target that hold handles to the
/// target, plus whether the handle table exposed object addresses at all.
fn holders_of(handle: &ProcessHandle, target: u32) -> (Vec<Holder>, bool) {
    let table = match handle_table() {
        Ok(t) => t,
        Err(_) => return (Vec::new(), false),
    };
    let entries = parse_handle_table(&table);

    // Our own query handle points at the target's kernel object; its address
    // is how we recognise every other handle to the same object.
    let me = std::process::id();
    let my_value = handle.raw().0 as usize;
    let object = entries
        .iter()
        .find(|e| e.pid == me && e.value == my_value)
        .map(|e| e.object)
        .filter(|&o| o != 0);
    let object = match object {
        Some(o) => o,
        // No address: an unprivileged caller on a Windows that hides them.
        None => return (Vec::new(), false),
    };

    let mut counts: HashMap<u32, usize> = HashMap::new();
    for e in &entries {
        if e.object == object && e.pid != me && e.pid != target {
            *counts.entry(e.pid).or_default() += 1;
        }
    }

    let names: HashMap<u32, String> = ProcessSnapshotter::new()
        .snapshot()
        .unwrap_or_default()
        .into_iter()
        .map(|p| (p.pid, p.name))
        .collect();
    let mut holders: Vec<Holder> = counts
        .into_iter()
        .map(|(pid, handles)| Holder {
            pid,
            name: names.get(&pid).cloned().unwrap_or_default(),
            handles,
        })
        .collect();
    holders.sort_by(|a, b| b.handles.cmp(&a.handles).then(a.pid.cmp(&b.pid)));
    (holders, true)
}

/// One handle-table row reduced to the fields we match on.
struct Entry {
    object: usize,
    pid: u32,
    value: usize,
}

fn parse_handle_table(table: &[u64]) -> Vec<Entry> {
    let bytes = table.len() * 8;
    if bytes < size_of::<HandleTableHeader>() {
        return Vec::new();
    }
    let base = table.as_ptr().cast::<u8>();
    // SAFETY: the buffer begins with a SYSTEM_HANDLE_INFORMATION_EX header.
    let count = unsafe { &*base.cast::<HandleTableHeader>() }.count;
    let first = size_of::<HandleTableHeader>();
    let stride = size_of::<HandleEntry>();
    let mut out = Vec::with_capacity(count.min(bytes / stride.max(1)));
    for i in 0..count {
        let offset = first + i * stride;
        if offset + stride > bytes {
            break;
        }
        // SAFETY: the entry lies wholly within the valid bytes checked above.
        let e = unsafe { &*base.add(offset).cast::<HandleEntry>() };
        out.push(Entry {
            object: e.object,
            pid: e.pid as u32,
            value: e.value,
        });
    }
    out
}

/// Reads `SystemExtendedHandleInformation` into an 8-byte-aligned buffer.
fn handle_table() -> Result<Vec<u64>> {
    let mut buf = vec![0u64; (256 * 1024) / 8];
    loop {
        let capacity = buf.len() * 8;
        let mut needed = 0u32;
        // SAFETY: the buffer is valid for `capacity` bytes and outlives the call.
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_EXTENDED_HANDLE_INFORMATION,
                buf.as_mut_ptr().cast(),
                u32::try_from(capacity).unwrap_or(u32::MAX),
                &mut needed,
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            let wanted = (needed as usize).max(capacity * 2) + 64 * 1024;
            buf.resize(wanted.div_ceil(8), 0);
            continue;
        }
        nt_ok("NtQuerySystemInformation", status)?;
        return Ok(buf);
    }
}

/// Loaded kernel drivers that match the guard list, as `(file, product)`.
fn loaded_guards() -> Result<Vec<(String, String)>> {
    Ok(match_guards(&driver_base_names()?))
}

fn match_guards(names: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for name in names {
        let lower = name.to_ascii_lowercase();
        let Some(&(_, product, kind)) =
            GUARDS.iter().find(|(prefix, ..)| lower.starts_with(prefix))
        else {
            continue;
        };
        // One line per product, even if several of its drivers are loaded.
        let label = product_label(product, kind);
        if !out.iter().any(|(_, p)| *p == label) {
            out.push((name.clone(), label));
        }
    }
    out
}

/// The base file names of every loaded kernel-mode driver.
fn driver_base_names() -> Result<Vec<String>> {
    // Grow the array until it holds every driver base the kernel reports; a
    // zero-length probe is unreliable, so we ask with real room and retry if
    // it filled the buffer exactly.
    let mut bases: Vec<*mut c_void> = vec![std::ptr::null_mut(); 1024];
    let real = loop {
        let capacity = (bases.len() * size_of::<*mut c_void>()) as u32;
        let mut needed = 0u32;
        // SAFETY: `bases` is writable for `capacity` bytes; `needed` is live.
        unsafe { EnumDeviceDrivers(bases.as_mut_ptr(), capacity, &mut needed)? };
        let count = needed as usize / size_of::<*mut c_void>();
        if count > bases.len() {
            bases.resize(count, std::ptr::null_mut());
            continue;
        }
        break count;
    };

    let mut names = Vec::with_capacity(real);
    for &base in &bases[..real] {
        if base.is_null() {
            continue;
        }
        let mut buf = [0u16; 260];
        // SAFETY: `base` is a driver image base from the call above; `buf` is
        // writable and its length bounds the write.
        let len = unsafe { GetDeviceDriverBaseNameW(base, &mut buf) } as usize;
        if len > 0 {
            names.push(String::from_utf16_lossy(&buf[..len.min(buf.len())]));
        }
    }
    Ok(names)
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
    fn protection_labels_cover_level_and_signer() {
        // Level 1 (light) + signer 3 (Antimalware).
        assert_eq!(
            protection_label(0b0011_0001),
            "light protection, Antimalware"
        );
        // Level 2 (full) + signer 6 (WinTcb).
        assert_eq!(protection_label(0b0110_0010), "full protection, WinTcb");
        // No protection, no signer.
        assert_eq!(protection_label(0), "no protection");
        // A level with an unknown signer still names the level.
        assert_eq!(protection_label(0b0000_0010), "full protection");
    }

    #[test]
    fn match_guards_keys_on_the_file_prefix_once() {
        let drivers = vec![
            "ntoskrnl.exe".to_owned(),
            "vgk.sys".to_owned(),
            "EasyAntiCheat.sys".to_owned(),
            "VGK.SYS".to_owned(),
            "tcpip.sys".to_owned(),
        ];
        let guards = match_guards(&drivers);
        assert!(guards.contains(&("vgk.sys".to_owned(), "Riot Vanguard anti-cheat".to_owned())));
        assert!(guards.contains(&("EasyAntiCheat.sys".to_owned(), "Easy Anti-Cheat".to_owned())));
        assert_eq!(
            match_guards(&["CSAgent.sys".to_owned()]),
            [(
                "CSAgent.sys".to_owned(),
                "CrowdStrike Falcon security software".to_owned()
            )]
        );
        // No spurious matches from ordinary system drivers.
        assert!(!guards.iter().any(|(f, _)| f == "tcpip.sys"));
        // Duplicates by prefix are not repeated.
        assert_eq!(
            guards
                .iter()
                .filter(|(_, p)| p.contains("Vanguard"))
                .count(),
            1
        );
    }

    #[test]
    fn the_system_lists_drivers_and_matches_none_falsely() {
        // Enumeration always succeeds. The kernel exposes driver base
        // addresses (and thus names) only to administrators, so an
        // unprivileged run may see an empty list; when names do come back
        // they look like driver files, and an ordinary machine runs no
        // anti-cheat, so nothing in the guard list should match.
        let drivers = driver_base_names().unwrap();
        for name in &drivers {
            assert!(!name.is_empty());
        }
        if drivers
            .iter()
            .any(|d| d.eq_ignore_ascii_case("ntoskrnl.exe"))
        {
            let matched = match_guards(&drivers);
            assert!(matched.is_empty(), "unexpected guard drivers: {matched:?}");
        }
    }

    #[test]
    fn a_healthy_child_can_be_ended_and_is_not_guarded() {
        let child = spawn_ping();
        let report = why_alive(child.0.id()).unwrap();
        assert!(!report.deleting);
        assert!(!report.exited);
        assert!(report.threads >= 1);
        assert!(!report.critical);
        assert_eq!(report.can_end, Some(true));
        assert!(report.protection.is_none());
    }

    #[test]
    fn our_own_process_is_explained_without_error() {
        let report = why_alive(std::process::id()).unwrap();
        assert_eq!(report.can_end, Some(true));
        assert!(report.open_error.is_none());
        assert!(report.threads >= 1);
        // We hold our own query/terminate handles, so we never list ourselves.
        assert!(report.holders.iter().all(|h| h.pid != std::process::id()));
    }

    #[test]
    fn a_vanished_pid_reports_no_threads() {
        let report = why_alive(0xffff_fffe).unwrap();
        assert_eq!(report.threads, 0);
        assert!(report.holders.is_empty());
        // It could not be opened at all.
        assert_eq!(report.can_end, None);
        assert!(report.open_error.is_some());
    }
}

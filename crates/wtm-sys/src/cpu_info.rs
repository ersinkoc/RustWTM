//! What Windows Task Manager lists under the CPU graph: the model, base
//! and current speed, sockets, cores, caches and virtualization, plus the
//! scheduler activity tools like System Informer add (context switches,
//! system calls, queue length). Nothing here needs administrator rights.

use std::cell::RefCell;

use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW,
    PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
};
use windows::Win32::System::SystemInformation::{
    GetLogicalProcessorInformationEx, GetTickCount64, RelationAll,
};
use windows::Win32::System::Threading::{IsProcessorFeaturePresent, PROCESSOR_FEATURE_ID};
use windows::core::{PCWSTR, w};

use crate::Result;
use crate::gpu::pdh;

/// Facts about the processors that do not change while Windows runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CpuInfo {
    /// Model name, such as "13th Gen Intel(R) Core(TM) i7-1365U".
    pub name: Option<String>,
    /// Base (nominal) speed in MHz, as the firmware reports it.
    pub base_mhz: Option<u32>,
    pub layout: Layout,
    /// Virtualization is enabled in the firmware; `None` when unknown.
    pub virtualization: Option<bool>,
}

/// Packages, cores and caches, from `GetLogicalProcessorInformationEx`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Layout {
    pub sockets: u32,
    pub cores: u32,
    pub logical: u32,
    /// Total size of all caches of each level, in bytes (as Task Manager).
    pub l1: u64,
    pub l2: u64,
    pub l3: u64,
}

const RELATION_CORE: u32 = 0;
const RELATION_CACHE: u32 = 2;
const RELATION_PACKAGE: u32 = 3;
/// Largest record list accepted; real machines need a few kilobytes.
const MAX_LAYOUT_BYTES: u32 = 4 * 1024 * 1024;
/// `PF_VIRT_FIRMWARE_ENABLED`.
const VIRT_FIRMWARE_ENABLED: PROCESSOR_FEATURE_ID = PROCESSOR_FEATURE_ID(21);

/// Decodes a `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX` list. A record that
/// does not fit ends it; what was read before stays.
fn decode_layout(bytes: &[u8]) -> Layout {
    let mut layout = Layout::default();
    let mut offset = 0usize;
    let u32_at = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(
            bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
        ))
    };
    while let (Some(kind), Some(size)) = (u32_at(offset), u32_at(offset + 4)) {
        let size = size as usize;
        let Some(end) = offset
            .checked_add(size)
            .filter(|e| size >= 8 && *e <= bytes.len())
        else {
            break;
        };
        let record = &bytes[offset..end];
        match kind {
            RELATION_CORE => {
                layout.cores += 1;
                // PROCESSOR_RELATIONSHIP: GroupCount (u16) at 30, then
                // GROUP_AFFINITY entries of 16 bytes from 32 (mask first).
                let groups = record
                    .get(30..32)
                    .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as usize);
                for g in 0..groups.min(64) {
                    let at = 32 + g * 16;
                    if let Some(mask) = record.get(at..at + 8) {
                        let mask = u64::from_le_bytes(mask.try_into().unwrap_or_default());
                        layout.logical += mask.count_ones();
                    }
                }
            }
            RELATION_PACKAGE => layout.sockets += 1,
            RELATION_CACHE => {
                // CACHE_RELATIONSHIP: Level (u8) at 8, CacheSize (u32) at 12.
                if let (Some(&level), Some(size)) = (record.get(8), record.get(12..16)) {
                    let size = u64::from(u32::from_le_bytes(size.try_into().unwrap_or_default()));
                    match level {
                        1 => layout.l1 += size,
                        2 => layout.l2 += size,
                        3 => layout.l3 += size,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        offset = end;
    }
    layout
}

fn layout() -> Result<Layout> {
    let mut required = 0u32;
    // SAFETY: no buffer: asks for the required size only.
    let first = unsafe { GetLogicalProcessorInformationEx(RelationAll, None, &mut required) };
    if let Err(e) = first
        && e.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult()
    {
        return Err(e.into());
    }
    for _ in 0..3 {
        if required == 0 || required > MAX_LAYOUT_BYTES {
            return Ok(Layout::default());
        }
        // u64 elements keep the records aligned.
        let mut buf = vec![0u64; (required as usize).div_ceil(8)];
        let mut len = (buf.len() * 8) as u32;
        // SAFETY: `buf` is writable for `len` bytes.
        let result = unsafe {
            GetLogicalProcessorInformationEx(RelationAll, Some(buf.as_mut_ptr().cast()), &mut len)
        };
        match result {
            Ok(()) => {
                let len = (len as usize).min(buf.len() * 8);
                // SAFETY: the call initialized `len` bytes of `buf`, which
                // stays alive while the records are decoded and copied.
                let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), len) };
                return Ok(decode_layout(bytes));
            }
            Err(e) if e.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() => required = len,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(Layout::default())
}

/// Reads a value of the first processor's firmware description.
fn processor_value(name: &str) -> Option<crate::registry::Value> {
    let mut key = HKEY::default();
    // SAFETY: static, NUL-terminated path; `key` receives the handle,
    // which is closed below.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            w!(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
            None,
            KEY_READ,
            &mut key,
        )
    };
    if opened.is_err() {
        return None;
    }
    let value = crate::registry::value(key.0 as usize, name);
    // SAFETY: we opened the key above and close it once.
    let _ = unsafe { RegCloseKey(key) };
    value
}

/// The processor facts. Reads the registry and the processor list, so
/// call it once and keep the result.
pub fn info() -> CpuInfo {
    use crate::registry::Value;
    let name = match processor_value("ProcessorNameString") {
        Some(Value::Text(t)) if !t.trim().is_empty() => Some(t.trim().to_owned()),
        _ => None,
    };
    let base_mhz = match processor_value("~MHz") {
        Some(Value::Dword(mhz)) if mhz > 0 => Some(mhz),
        _ => None,
    };
    CpuInfo {
        name,
        base_mhz,
        layout: layout().unwrap_or_default(),
        // SAFETY: no pointers involved.
        virtualization: Some(unsafe { IsProcessorFeaturePresent(VIRT_FIRMWARE_ENABLED) }.as_bool()),
    }
}

/// Time since Windows started, including sleep.
pub fn uptime() -> std::time::Duration {
    // SAFETY: no pointers involved.
    std::time::Duration::from_millis(unsafe { GetTickCount64() })
}

/// Scheduler activity over the time since the previous call on this thread.
/// A value is `None` until it has two samples or when its counter is
/// missing.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Activity {
    /// The speed the processors run at now, in MHz (above the base speed
    /// with turbo).
    pub current_mhz: Option<f64>,
    pub context_switches: Option<f64>,
    pub system_calls: Option<f64>,
    /// Threads ready to run but waiting for a processor.
    pub queue_length: Option<f64>,
}

thread_local! {
    static COUNTERS: RefCell<Option<Counters>> = const { RefCell::new(None) };
}

/// Each logical processor's current speed in MHz, in processor order, as
/// of the latest [`activity`] call on this thread; empty before two calls
/// or where the per-processor counter is missing.
pub fn core_mhz() -> Vec<f64> {
    COUNTERS.with(|cell| {
        cell.try_borrow()
            .ok()
            .and_then(|c| c.as_ref().map(|c| c.cores.clone()))
            .unwrap_or_default()
    })
}

/// "group,index" of a per-processor instance; `None` for the totals
/// ("_Total", "0,_Total").
fn processor_instance(name: &str) -> Option<(u16, u16)> {
    let (group, index) = name.split_once(',')?;
    Some((group.parse().ok()?, index.parse().ok()?))
}

/// Most processors read.
const MAX_CORES: usize = 2048;

/// The latest activity. The first call on a thread opens the counter query
/// that later calls on that thread reuse; it returns no rates yet.
pub fn activity() -> Activity {
    COUNTERS.with(|cell| {
        let Ok(mut counters) = cell.try_borrow_mut() else {
            return Activity::default();
        };
        if counters.is_none() {
            *counters = Counters::open().ok();
        }
        counters.as_mut().map(Counters::read).unwrap_or_default()
    })
}

struct Counters {
    query: PDH_HQUERY,
    /// In the order of [`COUNTER_PATHS`]; a missing one stays `None`.
    counters: [Option<PDH_HCOUNTER>; 5],
    /// `% Processor Performance` of every processor.
    per_core: Option<PDH_HCOUNTER>,
    reader: crate::gpu::ArrayReader,
    /// The latest speed of each processor, in MHz.
    cores: Vec<f64>,
}

const COUNTER_PATHS: [PCWSTR; 5] = [
    w!(r"\Processor Information(_Total)\% Processor Performance"),
    w!(r"\Processor Information(_Total)\Processor Frequency"),
    w!(r"\System\Context Switches/sec"),
    w!(r"\System\System Calls/sec"),
    w!(r"\System\Processor Queue Length"),
];

impl Counters {
    fn open() -> Result<Self> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: real-time query; `query` receives the handle.
        pdh("PdhOpenQueryW", unsafe {
            PdhOpenQueryW(PCWSTR::null(), 0, &mut query)
        })?;
        // Owned from here on, so Drop closes the query.
        let mut c = Self {
            query,
            counters: [None; 5],
            per_core: None,
            reader: Default::default(),
            cores: Vec::new(),
        };
        let mut counter = PDH_HCOUNTER::default();
        // SAFETY: valid query; static, NUL-terminated counter path.
        if unsafe {
            PdhAddEnglishCounterW(
                query,
                w!(r"\Processor Information(*)\% Processor Performance"),
                0,
                &mut counter,
            )
        } == 0
        {
            c.per_core = Some(counter);
        }
        for (path, slot) in COUNTER_PATHS.iter().zip(&mut c.counters) {
            let mut counter = PDH_HCOUNTER::default();
            // SAFETY: valid query; static, NUL-terminated counter path.
            if unsafe { PdhAddEnglishCounterW(query, *path, 0, &mut counter) } == 0 {
                *slot = Some(counter);
            }
        }
        // Prime the rate counters.
        // SAFETY: valid query.
        pdh("PdhCollectQueryData", unsafe { PdhCollectQueryData(query) })?;
        Ok(c)
    }

    fn read(&mut self) -> Activity {
        // SAFETY: valid query.
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return Activity::default();
        }
        let value = |counter: Option<PDH_HCOUNTER>| -> Option<f64> {
            let mut v = PDH_FMT_COUNTERVALUE::default();
            // SAFETY: valid counter of this query; `v` is writable.
            let status =
                unsafe { PdhGetFormattedCounterValue(counter?, PDH_FMT_DOUBLE, None, &mut v) };
            // SAFETY: requested as PDH_FMT_DOUBLE.
            let v = unsafe { v.Anonymous.doubleValue };
            (status == 0 && v.is_finite() && v >= 0.0).then_some(v)
        };
        let [performance, frequency, switches, calls, queue] = self.counters;
        self.cores.clear();
        if let (Some(counter), Some(nominal)) = (self.per_core, value(frequency)) {
            let mut found: Vec<((u16, u16), f64)> = Vec::new();
            let _ = self.reader.read(counter, PDH_FMT_DOUBLE, |name, item| {
                // SAFETY: requested as PDH_FMT_DOUBLE.
                let v = unsafe { item.FmtValue.Anonymous.doubleValue };
                if let Some(id) = processor_instance(name)
                    && found.len() < MAX_CORES
                    && let Some(mhz) = current_mhz(Some(v), Some(nominal))
                {
                    found.push((id, mhz));
                }
            });
            found.sort_by_key(|(id, _)| *id);
            self.cores.extend(found.into_iter().map(|(_, mhz)| mhz));
        }
        Activity {
            current_mhz: current_mhz(value(performance), value(frequency)),
            context_switches: value(switches),
            system_calls: value(calls),
            queue_length: value(queue),
        }
    }
}

/// The current speed from the nominal frequency and the performance
/// percentage (which passes 100 with turbo), as Task Manager computes it.
fn current_mhz(performance: Option<f64>, frequency: Option<f64>) -> Option<f64> {
    let mhz = frequency? * performance? / 100.0;
    (mhz > 0.0 && mhz < 100_000.0).then_some(mhz)
}

impl Drop for Counters {
    fn drop(&mut self) {
        // SAFETY: we own the query; closing it also frees its counters.
        unsafe { PdhCloseQuery(self.query) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processor_instances_skip_the_totals() {
        assert_eq!(processor_instance("0,7"), Some((0, 7)));
        assert_eq!(processor_instance("1,63"), Some((1, 63)));
        assert_eq!(processor_instance("_Total"), None);
        assert_eq!(processor_instance("0,_Total"), None);
    }

    fn record(kind: u32, body: &[u8]) -> Vec<u8> {
        let mut r = Vec::new();
        r.extend_from_slice(&kind.to_le_bytes());
        r.extend_from_slice(&((8 + body.len()) as u32).to_le_bytes());
        r.extend_from_slice(body);
        r
    }

    fn core(mask: u64) -> Vec<u8> {
        let mut body = vec![0u8; 40];
        body[22..24].copy_from_slice(&1u16.to_le_bytes()); // GroupCount at 30
        body[24..32].copy_from_slice(&mask.to_le_bytes()); // Mask at 32
        record(RELATION_CORE, &body)
    }

    fn cache(level: u8, size: u32) -> Vec<u8> {
        let mut body = vec![0u8; 44];
        body[0] = level;
        body[4..8].copy_from_slice(&size.to_le_bytes()); // CacheSize at 12
        record(RELATION_CACHE, &body)
    }

    #[test]
    fn decodes_packages_cores_threads_and_caches() {
        let mut bytes = Vec::new();
        bytes.extend(record(RELATION_PACKAGE, &[0; 40]));
        bytes.extend(core(0b11));
        bytes.extend(core(0b1100));
        bytes.extend(core(0b1_0000));
        for (level, size) in [
            (1, 48 << 10),
            (1, 32 << 10),
            (1, 48 << 10),
            (2, 1 << 20),
            (3, 12 << 20),
        ] {
            bytes.extend(cache(level, size));
        }
        bytes.extend(record(7, &[0; 4])); // unknown kind: skipped
        let l = decode_layout(&bytes);
        assert_eq!(
            l,
            Layout {
                sockets: 1,
                cores: 3,
                logical: 5,
                l1: 128 << 10,
                l2: 1 << 20,
                l3: 12 << 20,
            }
        );
    }

    #[test]
    fn a_truncated_or_malformed_list_keeps_what_was_read() {
        let mut bytes = core(1);
        let mut broken = record(RELATION_PACKAGE, &[0; 40]);
        broken.truncate(20);
        bytes.extend(broken);
        assert_eq!(decode_layout(&bytes).cores, 1);
        assert_eq!(decode_layout(&bytes).sockets, 0);
        // A zero size would never advance.
        let mut zero = vec![0u8; 16];
        zero[0] = RELATION_PACKAGE as u8;
        assert_eq!(decode_layout(&zero), Layout::default());
        assert_eq!(decode_layout(&[]), Layout::default());
    }

    #[test]
    fn current_speed_follows_performance_and_rejects_nonsense() {
        assert_eq!(current_mhz(Some(150.0), Some(2000.0)), Some(3000.0));
        assert_eq!(current_mhz(None, Some(2000.0)), None);
        assert_eq!(current_mhz(Some(0.0), Some(2000.0)), None);
        assert_eq!(current_mhz(Some(1e9), Some(2000.0)), None);
    }

    #[test]
    fn reads_this_machines_processors() {
        let info = info();
        assert!(info.name.is_some(), "{info:?}");
        let l = info.layout;
        assert!(l.sockets >= 1 && l.cores >= 1, "{l:?}");
        assert!(l.logical >= l.cores, "{l:?}");
        assert!(l.l1 > 0, "{l:?}");
        assert!(uptime().as_secs() > 0);
        let _ = activity();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let a = activity();
        assert!(a.context_switches.is_some(), "{a:?}");
    }
}

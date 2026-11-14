//! Per-process GPU usage from the "GPU Engine" and "GPU Process Memory"
//! performance counters (the same source Windows Task Manager uses).

use std::collections::HashMap;

use windows::Win32::System::Performance::{
    PDH_FMT, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY,
    PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};
use windows::core::{PCWSTR, w};

use crate::fasthash::FastMap;
use crate::{Error, Result};

const PDH_CSTATUS_VALID_DATA: u32 = 0;
const PDH_CSTATUS_NEW_DATA: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuUsage {
    /// Busiest engine type (3D, copy, video, ...) for this process, 0..=100.
    pub percent: f32,
    /// Dedicated GPU memory in bytes.
    pub dedicated_bytes: u64,
    /// Shared (system) memory the GPU uses for this process, in bytes.
    pub shared_bytes: u64,
    /// The busiest engine while it is in use: the GPU's LUID (as
    /// [`GpuAdapterUsage::luid`]) and the engine type, such as "3D".
    pub engine: Option<(String, String)>,
}

#[derive(Debug, Default, Clone)]
pub struct GpuSample {
    pub per_process: HashMap<u32, GpuUsage>,
    /// Busiest engine type across all processes, 0..=100.
    pub total_percent: f32,
    /// Load of every engine type (3D, Copy, VideoDecode, ...), by name.
    pub engines: Vec<(String, f32)>,
    /// Each GPU's own load and memory, ordered by LUID.
    pub adapters: Vec<GpuAdapterUsage>,
}

/// The load and memory use of one GPU, as Task Manager shows per GPU.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GpuAdapterUsage {
    /// `{high:08x}_{low:08x}`, the same text as [`Adapter::luid`].
    pub luid: String,
    /// Busiest engine type of this GPU, 0..=100.
    pub percent: f32,
    /// Load of each engine type of this GPU, by name.
    pub engines: Vec<(String, f32)>,
    /// Dedicated (on-board) and shared (system) memory in use, in bytes;
    /// `None` without the "GPU Adapter Memory" counters.
    pub dedicated_used: Option<u64>,
    pub shared_used: Option<u64>,
}

/// Most GPUs reported; more are ignored.
const MAX_ADAPTERS: usize = 16;

/// The text of a LUID as [`Adapter::luid`] writes it.
fn luid_text(high: u32, low: u32) -> String {
    format!("{high:08x}_{low:08x}")
}

pub struct GpuSampler {
    query: PDH_HQUERY,
    engine: PDH_HCOUNTER,
    memory: PDH_HCOUNTER,
    /// Per process; `None` where Windows lacks the counter.
    shared_memory: Option<PDH_HCOUNTER>,
    /// Per GPU; `None` where Windows lacks the "GPU Adapter Memory" set.
    adapter_dedicated: Option<PDH_HCOUNTER>,
    adapter_shared: Option<PDH_HCOUNTER>,
    /// Kept between samples: there is one engine instance per process and
    /// engine, often thousands.
    reader: ArrayReader,
    totals: Totals,
}

impl std::fmt::Debug for GpuSampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GpuSampler")
    }
}

// SAFETY: PDH query handles are not tied to the creating thread; all access
// goes through `&mut self`, so they are never used concurrently.
unsafe impl Send for GpuSampler {}

pub(crate) fn pdh(call: &'static str, status: u32) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(Error::Nt { call, status })
    }
}

impl GpuSampler {
    /// Fails on systems without GPU performance counters.
    pub fn new() -> Result<Self> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: real-time query; `query` receives the handle.
        pdh("PdhOpenQueryW", unsafe {
            PdhOpenQueryW(PCWSTR::null(), 0, &mut query)
        })?;
        let mut sampler = Self {
            query,
            engine: PDH_HCOUNTER::default(),
            memory: PDH_HCOUNTER::default(),
            shared_memory: None,
            adapter_dedicated: None,
            adapter_shared: None,
            reader: ArrayReader::default(),
            totals: Totals::default(),
        };
        // SAFETY: valid query; static, NUL-terminated counter paths.
        unsafe {
            pdh(
                "PdhAddEnglishCounterW",
                PdhAddEnglishCounterW(
                    sampler.query,
                    w!(r"\GPU Engine(*)\Utilization Percentage"),
                    0,
                    &mut sampler.engine,
                ),
            )?;
            pdh(
                "PdhAddEnglishCounterW",
                PdhAddEnglishCounterW(
                    sampler.query,
                    w!(r"\GPU Process Memory(*)\Dedicated Usage"),
                    0,
                    &mut sampler.memory,
                ),
            )?;
        }
        for (path, slot) in [
            (
                w!(r"\GPU Process Memory(*)\Shared Usage"),
                &mut sampler.shared_memory,
            ),
            (
                w!(r"\GPU Adapter Memory(*)\Dedicated Usage"),
                &mut sampler.adapter_dedicated,
            ),
            (
                w!(r"\GPU Adapter Memory(*)\Shared Usage"),
                &mut sampler.adapter_shared,
            ),
        ] {
            let mut counter = PDH_HCOUNTER::default();
            // SAFETY: valid query; static, NUL-terminated counter path.
            // Optional: older builds have no per-adapter memory counters.
            if unsafe { PdhAddEnglishCounterW(sampler.query, path, 0, &mut counter) } == 0 {
                *slot = Some(counter);
            }
        }
        // Utilization is a rate counter: prime it so the next sample has a delta.
        // SAFETY: valid query.
        pdh("PdhCollectQueryData", unsafe {
            PdhCollectQueryData(sampler.query)
        })?;
        Ok(sampler)
    }

    pub fn sample(&mut self) -> Result<GpuSample> {
        // SAFETY: valid query.
        pdh("PdhCollectQueryData", unsafe {
            PdhCollectQueryData(self.query)
        })?;

        // Instance names are parsed as they are read, never collected.
        let mut acc = Aggregate::new(&mut self.totals);
        self.reader
            .read(self.engine, PDH_FMT_DOUBLE, |name, item| {
                // SAFETY: requested as PDH_FMT_DOUBLE.
                acc.engine(name, unsafe { item.FmtValue.Anonymous.doubleValue });
            })?;
        let mut out = acc.finish();
        self.reader.read(self.memory, PDH_FMT_LARGE, |name, item| {
            // SAFETY: requested as PDH_FMT_LARGE.
            add_memory(&mut out, name, false, unsafe {
                item.FmtValue.Anonymous.largeValue
            });
        })?;
        if let Some(counter) = self.shared_memory {
            // A failed read leaves shared memory unknown, not the sample.
            let _ = self.reader.read(counter, PDH_FMT_LARGE, |name, item| {
                // SAFETY: requested as PDH_FMT_LARGE.
                add_memory(&mut out, name, true, unsafe {
                    item.FmtValue.Anonymous.largeValue
                });
            });
        }
        for (counter, shared) in [(self.adapter_dedicated, false), (self.adapter_shared, true)] {
            let Some(counter) = counter else {
                continue;
            };
            // A failed read leaves that figure unknown, not the sample.
            let _ = self.reader.read(counter, PDH_FMT_LARGE, |name, item| {
                // SAFETY: requested as PDH_FMT_LARGE.
                add_adapter_memory(&mut out, name, shared, unsafe {
                    item.FmtValue.Anonymous.largeValue
                });
            });
        }
        Ok(out)
    }
}

/// Load per engine type across all processes. The names are kept between
/// samples so they are not allocated again; only a handful exist.
#[derive(Default)]
struct Totals(Vec<(String, f64)>);

impl Totals {
    /// The slot of `engtype`, added when new.
    fn slot(&mut self, engtype: &str) -> usize {
        if let Some(i) = self.0.iter().position(|(n, _)| n == engtype) {
            return i;
        }
        self.0.push((engtype.to_owned(), 0.0));
        self.0.len() - 1
    }
}

/// One engine of one GPU: the adapter's LUID, its physical index and the
/// engine's index, as named in a counter instance.
type EngineId = (u32, u32, u32, u32);

/// Builds a sample from the engine utilization instances, fed one at a
/// time. Instances whose names carry no PID are skipped.
///
/// As in Task Manager, a process shows its busiest engine and an engine
/// type its busiest engine: two video engines at 60% each are a type at
/// 60%, not 100%. An engine's load is what all processes put on it.
struct Aggregate<'a> {
    /// Busiest engine per process: its load, the engine and its type's slot.
    by_process: FastMap<u32, (f64, EngineId, usize)>,
    /// Summed over processes per engine, with its type's slot.
    by_engine: FastMap<EngineId, (usize, f64)>,
    totals: &'a mut Totals,
    /// Slots some instance reported this time.
    seen: Vec<bool>,
}

impl<'a> Aggregate<'a> {
    fn new(totals: &'a mut Totals) -> Self {
        for t in &mut totals.0 {
            t.1 = 0.0;
        }
        Self {
            by_process: FastMap::default(),
            by_engine: FastMap::default(),
            seen: vec![false; totals.0.len()],
            totals,
        }
    }

    fn engine(&mut self, name: &str, v: f64) {
        let Some((pid, engine, engtype)) = parse_engine_instance(name) else {
            return;
        };
        let slot = self.totals.slot(engtype);
        if slot >= self.seen.len() {
            self.seen.resize(slot + 1, false);
        }
        self.seen[slot] = true;
        let busiest = self.by_process.entry(pid).or_insert((v, engine, slot));
        if v > busiest.0 {
            *busiest = (v, engine, slot);
        }
        self.by_engine.entry(engine).or_insert((slot, 0.0)).1 += v;
    }

    /// Types nothing reported this time are left out.
    fn finish(self) -> GpuSample {
        let mut out = GpuSample::default();
        for ((high, low, _, _), (slot, v)) in self.by_engine {
            let total = &mut self.totals.0[slot].1;
            *total = total.max(v);
            let Some(adapter) = adapter_entry(&mut out.adapters, high, low) else {
                continue;
            };
            let name = &self.totals.0[slot].0;
            let v = v.min(100.0) as f32;
            match adapter.engines.iter_mut().find(|(n, _)| n == name) {
                Some(engine) => engine.1 = engine.1.max(v),
                None => adapter.engines.push((name.clone(), v)),
            }
        }
        for adapter in &mut out.adapters {
            adapter.engines.sort_by(|a, b| a.0.cmp(&b.0));
            adapter.percent = adapter.engines.iter().fold(0.0, |a, e| a.max(e.1));
        }
        out.per_process.reserve(self.by_process.len());
        for (pid, (v, (high, low, _, _), slot)) in self.by_process {
            let usage = out.per_process.entry(pid).or_default();
            usage.percent = v.clamp(0.0, 100.0) as f32;
            // Named only while busy, as Task Manager's GPU engine column.
            if v > 0.0 {
                usage.engine = Some((luid_text(high, low), self.totals.0[slot].0.clone()));
            }
        }
        let live = || {
            self.totals
                .0
                .iter()
                .zip(&self.seen)
                .filter(|(_, seen)| **seen)
                .map(|(t, _)| t)
        };
        out.total_percent = live().fold(0.0f64, |a, (_, b)| a.max(*b)).min(100.0) as f32;
        out.engines = live()
            .map(|(name, v)| (name.clone(), v.min(100.0) as f32))
            .collect();
        out.engines.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

/// The entry of one GPU, added in LUID order when new; `None` past
/// [`MAX_ADAPTERS`].
fn adapter_entry(
    adapters: &mut Vec<GpuAdapterUsage>,
    high: u32,
    low: u32,
) -> Option<&mut GpuAdapterUsage> {
    let luid = luid_text(high, low);
    let i = match adapters.binary_search_by(|a| a.luid.cmp(&luid)) {
        Ok(i) => i,
        Err(_) if adapters.len() >= MAX_ADAPTERS => return None,
        Err(i) => {
            adapters.insert(
                i,
                GpuAdapterUsage {
                    luid,
                    ..Default::default()
                },
            );
            i
        }
    };
    adapters.get_mut(i)
}

/// `luid_0x00000000_0x0000C9D3_phys_0` -> the LUID's two halves.
fn parse_adapter_instance(name: &str) -> Option<(u32, u32)> {
    let mut parts = name.strip_prefix("luid_")?.split('_');
    let hex = |p: Option<&str>| u32::from_str_radix(p?.strip_prefix("0x")?, 16).ok();
    Some((hex(parts.next())?, hex(parts.next())?))
}

/// Adds one instance of a "GPU Adapter Memory" counter to its GPU.
fn add_adapter_memory(out: &mut GpuSample, name: &str, shared: bool, v: i64) {
    let Some((high, low)) = parse_adapter_instance(name) else {
        return;
    };
    let Some(adapter) = adapter_entry(&mut out.adapters, high, low) else {
        return;
    };
    let slot = if shared {
        &mut adapter.shared_used
    } else {
        &mut adapter.dedicated_used
    };
    *slot = Some(slot.unwrap_or(0) + v.max(0) as u64);
}

/// Adds one instance of the dedicated or shared process memory counter.
fn add_memory(out: &mut GpuSample, name: &str, shared: bool, v: i64) {
    let Some(pid) = parse_pid(name) else {
        return;
    };
    let usage = out.per_process.entry(pid).or_default();
    if shared {
        usage.shared_bytes += v.max(0) as u64;
    } else {
        usage.dedicated_bytes += v.max(0) as u64;
    }
}

/// A sample from raw `(instance name, value)` pairs of the engine
/// utilization and dedicated memory counters.
#[cfg(test)]
fn aggregate(engine: &[(String, f64)], memory: &[(String, i64)]) -> GpuSample {
    let mut totals = Totals::default();
    let mut acc = Aggregate::new(&mut totals);
    for (name, v) in engine {
        acc.engine(name, *v);
    }
    let mut out = acc.finish();
    for (name, v) in memory {
        add_memory(&mut out, name, false, *v);
    }
    out
}

impl Drop for GpuSampler {
    fn drop(&mut self) {
        // SAFETY: we own the query; closing it also frees its counters.
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// Reads wildcard counter arrays into a buffer kept between calls.
#[derive(Default)]
pub(crate) struct ArrayReader {
    // u64 elements keep the item array aligned; names follow in the same buffer.
    buf: Vec<u64>,
    /// The name of the instance being reported, reused for every item.
    name: String,
}

/// Room added when an array outgrows the buffer: instances come and go.
const ARRAY_SLACK_BYTES: usize = 16 * 1024;

impl ArrayReader {
    /// Calls `f` for every valid instance value of a wildcard counter.
    ///
    /// PDH formats the whole array on every call, a size-only one included,
    /// so the kept buffer is offered straight away: one call per sample
    /// instead of two once it is large enough.
    pub(crate) fn read(
        &mut self,
        counter: PDH_HCOUNTER,
        format: PDH_FMT,
        mut f: impl FnMut(&str, &PDH_FMT_COUNTERVALUE_ITEM_W),
    ) -> Result<()> {
        let mut grown = 0;
        let count = loop {
            let mut size = u32::try_from(self.buf.len() * 8).unwrap_or(u32::MAX);
            let mut count = 0u32;
            let items = (!self.buf.is_empty())
                .then(|| self.buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>());
            // SAFETY: `items` is None with a zero size, or points to `buf`,
            // which holds `size` bytes.
            let status = unsafe {
                PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, items)
            };
            // The array can grow between calls; a few retries are plenty.
            if status == PDH_MORE_DATA && grown < 4 {
                grown += 1;
                let wanted = size as usize + ARRAY_SLACK_BYTES;
                self.buf.resize(wanted.div_ceil(8), 0);
                continue;
            }
            pdh("PdhGetFormattedCounterArrayW", status)?;
            // No instances (e.g. no process uses the GPU) is not an error.
            if items.is_none() {
                return Ok(());
            }
            break count as usize;
        };
        let items = self.buf.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        for i in 0..count {
            // SAFETY: the call wrote `count` items into `buf`.
            let item = unsafe { &*items.add(i) };
            if item.szName.is_null()
                || !matches!(
                    item.FmtValue.CStatus,
                    PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
                )
            {
                continue;
            }
            // SAFETY: instance names are NUL-terminated strings inside `buf`.
            let wide = unsafe { item.szName.as_wide() };
            self.name.clear();
            // Names that are not valid UTF-16 are skipped.
            let valid = char::decode_utf16(wide.iter().copied())
                .try_for_each(|c| c.map(|c| self.name.push(c)))
                .is_ok();
            if valid {
                f(&self.name, item);
            }
        }
        Ok(())
    }
}

/// `pid_1234_luid_0x0_0xC9D3_phys_0_eng_0_engtype_3D` -> (1234, "3D")
/// `pid_1234_luid_0x00000000_0x0000C9D3_phys_0_eng_0_engtype_3D`: the PID,
/// the engine and its type.
fn parse_engine_instance(name: &str) -> Option<(u32, EngineId, &str)> {
    let pid = parse_pid(name)?;
    let (head, engtype) = name.split_once("_engtype_")?;
    let mut parts = head.split_once("_luid_")?.1.split('_');
    let hex = |p: Option<&str>| u32::from_str_radix(p?.strip_prefix("0x")?, 16).ok();
    let (high, low) = (hex(parts.next())?, hex(parts.next())?);
    let mut after = |label: &str| match parts.next() {
        Some(l) if l == label => parts.next()?.parse::<u32>().ok(),
        _ => None,
    };
    let phys = after("phys")?;
    let eng = after("eng")?;
    Some((pid, (high, low, phys, eng), engtype))
}

fn parse_pid(name: &str) -> Option<u32> {
    name.strip_prefix("pid_")?.split('_').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_instance_names() {
        assert_eq!(
            parse_engine_instance("pid_1234_luid_0x00000000_0x0000C9D3_phys_0_eng_0_engtype_3D"),
            Some((1234, (0, 0xC9D3, 0, 0), "3D"))
        );
        assert_eq!(
            parse_engine_instance("pid_7_luid_0x0_0x1_phys_1_eng_5_engtype_VideoDecode"),
            Some((7, (0, 1, 1, 5), "VideoDecode"))
        );
        for odd in [
            "pid_7_luid_0x0_phys_0_eng_5_engtype_3D",
            "pid_7_luid_0x0_0x1_eng_0_phys_5_engtype_3D",
            "pid_7_luid_zz_0x1_phys_0_eng_5_engtype_3D",
            "pid_7_luid_0x0_0x1_phys_0_eng_5",
        ] {
            assert_eq!(parse_engine_instance(odd), None, "{odd}");
        }
        assert_eq!(parse_pid("pid_42_luid_0x0_0x1_phys_0"), Some(42));
        assert_eq!(parse_pid("_Total"), None);
    }

    #[test]
    fn aggregates_busiest_engine_and_memory_per_process() {
        let inst = |pid: u32, eng: u32, ty: &str| {
            format!("pid_{pid}_luid_0x0_0x1_phys_0_eng_{eng}_engtype_{ty}")
        };
        let engine = [
            // A process shows its busiest engine.
            (inst(1, 0, "3D"), 30.0),
            (inst(1, 1, "3D"), 20.0),
            (inst(1, 2, "Copy"), 10.0),
            // Engine 0 carries 30 + 70: the 3D type is at 100.
            (inst(2, 0, "3D"), 70.0),
            (inst(3, 3, "VideoDecode"), 150.0),
            // Two video encoders at 60% each: the type is at 60%, not 120%.
            (inst(5, 4, "VideoEncode"), 60.0),
            (inst(5, 5, "VideoEncode"), 60.0),
            ("_Total".to_owned(), 99.0),
        ];
        let memory = [
            ("pid_1_luid_0x0_0x1_phys_0".to_owned(), 1024),
            ("pid_4_luid_0x0_0x1_phys_0".to_owned(), -5),
            ("junk".to_owned(), 7),
        ];
        let mut s = aggregate(&engine, &memory);
        add_memory(&mut s, "pid_1_luid_0x0_0x1_phys_0", true, 4096);
        let get = |pid| s.per_process.get(&pid).cloned();
        assert_eq!(
            get(1),
            Some(GpuUsage {
                percent: 30.0,
                dedicated_bytes: 1024,
                shared_bytes: 4096,
                // The busiest engine names its GPU and type.
                engine: Some(("00000000_00000001".into(), "3D".into())),
            })
        );
        assert_eq!(
            get(3).and_then(|u| u.engine).map(|e| e.1).as_deref(),
            Some("VideoDecode")
        );
        assert_eq!(get(2).map(|u| u.percent), Some(70.0));
        // Per-process and total values are capped at 100%.
        assert_eq!(get(3).map(|u| u.percent), Some(100.0));
        assert_eq!(s.total_percent, 100.0);
        // Negative counter values clamp to zero.
        assert_eq!(get(4), Some(GpuUsage::default()));
        assert_eq!(get(5).map(|u| u.percent), Some(60.0));
        assert_eq!(s.per_process.len(), 5);
        assert_eq!(
            s.engines,
            [
                ("3D".to_owned(), 100.0),
                ("Copy".to_owned(), 10.0),
                ("VideoDecode".to_owned(), 100.0),
                ("VideoEncode".to_owned(), 60.0)
            ]
        );
    }

    #[test]
    fn pdh_failures_become_errors() {
        assert!(pdh("x", 0).is_ok());
        assert!(matches!(
            pdh("x", 0xC000_07D2),
            Err(Error::Nt {
                call: "x",
                status: 0xC000_07D2
            })
        ));
        // PDH validates handles, so a null counter is rejected cleanly.
        let mut calls = 0;
        let r = ArrayReader::default().read(PDH_HCOUNTER::default(), PDH_FMT_DOUBLE, |_, _| {
            calls += 1;
        });
        assert!(r.is_err());
        assert_eq!(calls, 0);
    }

    #[test]
    fn reads_wildcard_counter_arrays() {
        let mut query = PDH_HQUERY::default();
        // SAFETY: real-time query; `query` receives the handle.
        pdh("PdhOpenQueryW", unsafe {
            PdhOpenQueryW(PCWSTR::null(), 0, &mut query)
        })
        .unwrap();
        let mut counter = PDH_HCOUNTER::default();
        // SAFETY: valid query; static, NUL-terminated counter path. Unlike
        // the GPU counters, this one exists on every machine.
        let added = pdh("PdhAddEnglishCounterW", unsafe {
            PdhAddEnglishCounterW(
                query,
                w!(r"\Processor(*)\% Processor Time"),
                0,
                &mut counter,
            )
        });
        let collect = || {
            // SAFETY: valid query.
            pdh("PdhCollectQueryData", unsafe { PdhCollectQueryData(query) })
        };
        // A rate counter needs two samples before it has a value.
        let mut reader = ArrayReader::default();
        let first = collect();
        let before = reader.read(counter, PDH_FMT_DOUBLE, |_, _| {});
        std::thread::sleep(std::time::Duration::from_millis(100));
        let second = collect();
        let read = |reader: &mut ArrayReader| {
            let mut names = Vec::new();
            let r = reader.read(counter, PDH_FMT_DOUBLE, |name, item| {
                // SAFETY: requested as PDH_FMT_DOUBLE.
                let v = unsafe { item.FmtValue.Anonymous.doubleValue };
                names.push((name.to_owned(), v));
            });
            (r, names)
        };
        let (after, names) = read(&mut reader);
        // Again into the buffer kept from the first read, which now fits.
        let kept = reader.buf.len();
        let (repeat, again) = read(&mut reader);
        // SAFETY: we own the query; closing it also frees its counter.
        unsafe { PdhCloseQuery(query) };

        added.unwrap();
        first.unwrap();
        second.unwrap();
        // With a single sample PDH rejects the whole array.
        assert!(before.is_err());
        after.unwrap();
        assert!(names.iter().any(|(n, _)| n == "_Total"), "{names:?}");
        assert!(names.iter().all(|(_, v)| (0.0..=100.0).contains(v)));
        repeat.unwrap();
        assert_eq!(reader.buf.len(), kept, "a buffer that fits is kept");
        let instances = |v: &[(String, f64)]| v.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
        assert_eq!(instances(&again), instances(&names));
    }

    #[test]
    fn splits_load_and_memory_per_gpu() {
        let mut totals = Totals::default();
        let mut acc = Aggregate::new(&mut totals);
        // GPU 0xA: two 3D engines and a copy engine; GPU 0xB: video only.
        acc.engine(
            "pid_1_luid_0x00000000_0x0000000A_phys_0_eng_0_engtype_3D",
            30.0,
        );
        acc.engine(
            "pid_2_luid_0x00000000_0x0000000A_phys_0_eng_0_engtype_3D",
            20.0,
        );
        acc.engine(
            "pid_1_luid_0x00000000_0x0000000A_phys_0_eng_1_engtype_3D",
            40.0,
        );
        acc.engine(
            "pid_1_luid_0x00000000_0x0000000A_phys_0_eng_2_engtype_Copy",
            5.0,
        );
        acc.engine(
            "pid_3_luid_0x00000001_0x0000000B_phys_0_eng_0_engtype_VideoDecode",
            70.0,
        );
        let mut s = acc.finish();
        for (name, shared, v) in [
            ("luid_0x00000000_0x0000000A_phys_0", false, 1000),
            ("luid_0x00000000_0x0000000A_phys_1", false, 24),
            ("luid_0x00000000_0x0000000A_phys_0", true, 77),
            ("luid_0x00000002_0x0000000C_phys_0", false, -3),
            ("_Total", false, 5),
        ] {
            add_adapter_memory(&mut s, name, shared, v);
        }
        let luids: Vec<_> = s.adapters.iter().map(|a| a.luid.as_str()).collect();
        assert_eq!(
            luids,
            [
                "00000000_0000000a",
                "00000001_0000000b",
                "00000002_0000000c"
            ]
        );
        let a = &s.adapters[0];
        // Engine 0 carries 30 + 20; engine 1 is the busiest 3D engine.
        assert_eq!(
            a.engines,
            [("3D".to_owned(), 50.0), ("Copy".to_owned(), 5.0)]
        );
        assert_eq!(a.percent, 50.0);
        assert_eq!((a.dedicated_used, a.shared_used), (Some(1024), Some(77)));
        let b = &s.adapters[1];
        assert_eq!(b.percent, 70.0);
        assert_eq!((b.dedicated_used, b.shared_used), (None, None));
        // A GPU with memory but no engine instances still shows, idle.
        let c = &s.adapters[2];
        assert_eq!((c.percent, c.dedicated_used), (0.0, Some(0)));
        assert_eq!(s.total_percent, 70.0);
    }

    #[test]
    fn caps_the_number_of_gpus() {
        let mut s = GpuSample::default();
        for i in 0..(MAX_ADAPTERS as u32 + 5) {
            add_adapter_memory(&mut s, &format!("luid_0x0_0x{i:x}_phys_0"), false, 1);
        }
        assert_eq!(s.adapters.len(), MAX_ADAPTERS);
        assert_eq!(
            parse_adapter_instance("luid_0x1_0xff_phys_0"),
            Some((1, 255))
        );
        assert_eq!(parse_adapter_instance("pid_1_luid_0x1_0xff"), None);
    }

    #[test]
    fn engine_types_missing_from_a_sample_are_left_out() {
        let mut totals = Totals::default();
        let mut acc = Aggregate::new(&mut totals);
        acc.engine("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 40.0);
        acc.engine("pid_1_luid_0x0_0x1_phys_0_eng_1_engtype_Copy", 90.0);
        let first = acc.finish();
        assert_eq!(first.engines.len(), 2);
        assert_eq!(first.total_percent, 90.0);
        // The next sample reuses the kept names; Copy did not report.
        let mut acc = Aggregate::new(&mut totals);
        acc.engine("pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 10.0);
        let second = acc.finish();
        assert_eq!(second.engines, [("3D".to_owned(), 10.0)]);
        assert_eq!(second.total_percent, 10.0);
        assert_eq!(second.per_process.len(), 1);
    }

    #[test]
    fn debug_hides_raw_handles() {
        let s = GpuSampler {
            query: PDH_HQUERY::default(),
            engine: PDH_HCOUNTER::default(),
            memory: PDH_HCOUNTER::default(),
            shared_memory: None,
            adapter_dedicated: None,
            adapter_shared: None,
            reader: ArrayReader::default(),
            totals: Totals::default(),
        };
        assert_eq!(format!("{s:?}"), "GpuSampler");
        // Null handles: skip Drop so no close is attempted.
        std::mem::forget(s);
    }

    #[test]
    fn samples_without_error_when_counters_exist() {
        // Machines without a GPU driver have no counters; that is fine.
        let Ok(mut s) = GpuSampler::new() else {
            return;
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        let sample = s.sample().unwrap();
        assert!((0.0..=100.0).contains(&sample.total_percent));
        assert!(
            sample
                .per_process
                .values()
                .all(|u| (0.0..=100.0).contains(&u.percent))
        );
    }
}
/// A physical adapter's session LUID and DXGI identity. Re-enumerate after hotplug.
#[derive(Debug, Clone)]
pub struct Adapter {
    pub luid: String,
    pub name: String,
    pub dedicated_bytes: u64,
    pub shared_bytes: u64,
    /// PCI vendor and device IDs, matching the driver's hardware ID.
    pub vendor_id: u32,
    pub device_id: u32,
    /// The LUID as one number, as DirectX records it in the registry.
    pub luid_value: u64,
}

pub fn adapters() -> Result<Vec<Adapter>> {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND, IDXGIFactory1,
    };
    // SAFETY: the typed factory owns its COM reference; no pointers escape.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut result = Vec::new();
    for index in 0..32 {
        // SAFETY: the factory is live; enumeration returns an owned interface.
        let adapter = match unsafe { factory.EnumAdapters1(index) } {
            Ok(a) => a,
            Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(e) => return Err(e.into()),
        };
        // SAFETY: GetDesc1 returns an initialized value for this live adapter.
        let desc = unsafe { adapter.GetDesc1()? };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let end = desc
            .Description
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(desc.Description.len());
        let luid = format!(
            "{:08x}_{:08x}",
            desc.AdapterLuid.HighPart as u32, desc.AdapterLuid.LowPart
        );
        result.push(Adapter {
            luid,
            luid_value: (u64::from(desc.AdapterLuid.HighPart as u32) << 32)
                | u64::from(desc.AdapterLuid.LowPart),
            name: String::from_utf16_lossy(&desc.Description[..end]),
            dedicated_bytes: desc.DedicatedVideoMemory as u64,
            shared_bytes: desc.SharedSystemMemory as u64,
            vendor_id: desc.VendorId,
            device_id: desc.DeviceId,
        });
    }
    Ok(result)
}

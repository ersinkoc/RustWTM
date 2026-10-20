//! Per-process GPU usage from the "GPU Engine" and "GPU Process Memory"
//! performance counters (the same source Windows Task Manager uses).

use std::collections::HashMap;

use windows::Win32::System::Performance::{
    PDH_FMT, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY,
    PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};
use windows::core::{PCWSTR, w};

use crate::{Error, Result};

const PDH_CSTATUS_VALID_DATA: u32 = 0;
const PDH_CSTATUS_NEW_DATA: u32 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GpuUsage {
    /// Busiest engine type (3D, copy, video, ...) for this process, 0..=100.
    pub percent: f32,
    /// Dedicated GPU memory in bytes.
    pub dedicated_bytes: u64,
}

#[derive(Debug, Default, Clone)]
pub struct GpuSample {
    pub per_process: HashMap<u32, GpuUsage>,
    /// Busiest engine type across all processes, 0..=100.
    pub total_percent: f32,
    /// Load of every engine type (3D, Copy, VideoDecode, ...), by name.
    pub engines: Vec<(String, f32)>,
}

pub struct GpuSampler {
    query: PDH_HQUERY,
    engine: PDH_HCOUNTER,
    memory: PDH_HCOUNTER,
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

        let mut engine = Vec::new();
        read_array(self.engine, PDH_FMT_DOUBLE, |name, item| {
            // SAFETY: requested as PDH_FMT_DOUBLE.
            engine.push((name, unsafe { item.FmtValue.Anonymous.doubleValue }));
        })?;
        let mut memory = Vec::new();
        read_array(self.memory, PDH_FMT_LARGE, |name, item| {
            // SAFETY: requested as PDH_FMT_LARGE.
            memory.push((name, unsafe { item.FmtValue.Anonymous.largeValue }));
        })?;
        Ok(aggregate(&engine, &memory))
    }
}

/// Builds a sample from raw `(instance name, value)` pairs of the engine
/// utilization and dedicated memory counters. Instances whose names carry
/// no PID are skipped.
fn aggregate(engine: &[(String, f64)], memory: &[(String, i64)]) -> GpuSample {
    // Sum per (pid, engine type), then take the busiest type per process.
    let mut by_engine: HashMap<(u32, &str), f64> = HashMap::new();
    let mut totals: HashMap<&str, f64> = HashMap::new();
    for (name, v) in engine {
        let Some((pid, engtype)) = parse_engine_instance(name) else {
            continue;
        };
        *by_engine.entry((pid, engtype)).or_default() += v;
        *totals.entry(engtype).or_default() += v;
    }

    let mut out = GpuSample::default();
    for ((pid, _), v) in by_engine {
        let e = out.per_process.entry(pid).or_default();
        e.percent = e.percent.max(v.min(100.0) as f32);
    }
    out.total_percent = totals.values().fold(0.0f64, |a, b| a.max(*b)).min(100.0) as f32;
    out.engines = totals
        .iter()
        .map(|(name, v)| ((*name).to_owned(), v.min(100.0) as f32))
        .collect();
    out.engines.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, v) in memory {
        let Some(pid) = parse_pid(name) else {
            continue;
        };
        out.per_process.entry(pid).or_default().dedicated_bytes += (*v).max(0) as u64;
    }
    out
}

impl Drop for GpuSampler {
    fn drop(&mut self) {
        // SAFETY: we own the query; closing it also frees its counters.
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// Calls `f` for every valid instance value of a wildcard counter.
pub(crate) fn read_array(
    counter: PDH_HCOUNTER,
    format: PDH_FMT,
    mut f: impl FnMut(String, &PDH_FMT_COUNTERVALUE_ITEM_W),
) -> Result<()> {
    let mut size = 0u32;
    let mut count = 0u32;
    // SAFETY: size query without a buffer.
    let status =
        unsafe { PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, None) };
    if status != PDH_MORE_DATA {
        // No instances (e.g. no process uses the GPU) is not an error.
        return if status == 0 {
            Ok(())
        } else {
            pdh("PdhGetFormattedCounterArrayW", status)
        };
    }
    // u64 elements keep the item array aligned; names follow in the same buffer.
    let mut buf: Vec<u64> = vec![0; (size as usize).div_ceil(8)];
    let items = buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
    // SAFETY: `buf` holds `size` bytes.
    pdh("PdhGetFormattedCounterArrayW", unsafe {
        PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, Some(items))
    })?;
    for i in 0..count as usize {
        // SAFETY: the call wrote `count` items into `buf`.
        let item = unsafe { &*items.add(i) };
        if !matches!(
            item.FmtValue.CStatus,
            PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
        ) {
            continue;
        }
        // SAFETY: instance names are NUL-terminated strings inside `buf`.
        if let Ok(name) = unsafe { item.szName.to_string() } {
            f(name, item);
        }
    }
    Ok(())
}

/// `pid_1234_luid_0x0_0xC9D3_phys_0_eng_0_engtype_3D` -> (1234, "3D")
fn parse_engine_instance(name: &str) -> Option<(u32, &str)> {
    let pid = parse_pid(name)?;
    let engtype = name.split_once("_engtype_")?.1;
    Some((pid, engtype))
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
            Some((1234, "3D"))
        );
        assert_eq!(
            parse_engine_instance("pid_7_luid_0x0_0x1_phys_0_eng_5_engtype_VideoDecode"),
            Some((7, "VideoDecode"))
        );
        assert_eq!(parse_pid("pid_42_luid_0x0_0x1_phys_0"), Some(42));
        assert_eq!(parse_pid("_Total"), None);
    }

    #[test]
    fn aggregates_busiest_engine_and_memory_per_process() {
        let inst = |pid: u32, eng: u32, ty: &str| {
            format!("pid_{pid}_luid_0x0_0x1_phys_0_eng_{eng}_engtype_{ty}")
        };
        let engine = [
            // Two 3D engines of one process add up.
            (inst(1, 0, "3D"), 30.0),
            (inst(1, 1, "3D"), 20.0),
            (inst(1, 2, "Copy"), 10.0),
            (inst(2, 0, "3D"), 70.0),
            (inst(3, 0, "VideoDecode"), 150.0),
            ("_Total".to_owned(), 99.0),
        ];
        let memory = [
            ("pid_1_luid_0x0_0x1_phys_0".to_owned(), 1024),
            ("pid_4_luid_0x0_0x1_phys_0".to_owned(), -5),
            ("junk".to_owned(), 7),
        ];
        let s = aggregate(&engine, &memory);
        let get = |pid| s.per_process.get(&pid).copied();
        assert_eq!(
            get(1),
            Some(GpuUsage {
                percent: 50.0,
                dedicated_bytes: 1024
            })
        );
        assert_eq!(get(2).map(|u| u.percent), Some(70.0));
        // Per-process and total values are capped at 100%.
        assert_eq!(get(3).map(|u| u.percent), Some(100.0));
        assert_eq!(s.total_percent, 100.0);
        // Negative counter values clamp to zero.
        assert_eq!(get(4), Some(GpuUsage::default()));
        assert_eq!(s.per_process.len(), 4);
        assert_eq!(
            s.engines,
            [
                ("3D".to_owned(), 100.0),
                ("Copy".to_owned(), 10.0),
                ("VideoDecode".to_owned(), 100.0)
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
        let r = read_array(PDH_HCOUNTER::default(), PDH_FMT_DOUBLE, |_, _| calls += 1);
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
        let first = collect();
        let before = read_array(counter, PDH_FMT_DOUBLE, |_, _| {});
        std::thread::sleep(std::time::Duration::from_millis(100));
        let second = collect();
        let mut names = Vec::new();
        let after = read_array(counter, PDH_FMT_DOUBLE, |name, item| {
            // SAFETY: requested as PDH_FMT_DOUBLE.
            let v = unsafe { item.FmtValue.Anonymous.doubleValue };
            names.push((name, v));
        });
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
    }

    #[test]
    fn debug_hides_raw_handles() {
        let s = GpuSampler {
            query: PDH_HQUERY::default(),
            engine: PDH_HCOUNTER::default(),
            memory: PDH_HCOUNTER::default(),
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

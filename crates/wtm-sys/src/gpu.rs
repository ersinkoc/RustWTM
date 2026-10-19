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

fn pdh(call: &'static str, status: u32) -> Result<()> {
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

        // Sum per (pid, engine type), then take the busiest type per process.
        let mut by_engine: HashMap<(u32, String), f64> = HashMap::new();
        let mut totals: HashMap<String, f64> = HashMap::new();
        read_array(self.engine, PDH_FMT_DOUBLE, |name, item| {
            let Some((pid, engtype)) = parse_engine_instance(name) else {
                return;
            };
            // SAFETY: requested as PDH_FMT_DOUBLE.
            let v = unsafe { item.FmtValue.Anonymous.doubleValue };
            *by_engine.entry((pid, engtype.to_owned())).or_default() += v;
            *totals.entry(engtype.to_owned()).or_default() += v;
        })?;

        let mut out = GpuSample::default();
        for ((pid, _), v) in by_engine {
            let e = out.per_process.entry(pid).or_default();
            e.percent = e.percent.max(v.min(100.0) as f32);
        }
        out.total_percent = totals.values().fold(0.0f64, |a, b| a.max(*b)).min(100.0) as f32;

        read_array(self.memory, PDH_FMT_LARGE, |name, item| {
            let Some(pid) = parse_pid(name) else {
                return;
            };
            // SAFETY: requested as PDH_FMT_LARGE.
            let v = unsafe { item.FmtValue.Anonymous.largeValue }.max(0) as u64;
            out.per_process.entry(pid).or_default().dedicated_bytes += v;
        })?;
        Ok(out)
    }
}

impl Drop for GpuSampler {
    fn drop(&mut self) {
        // SAFETY: we own the query; closing it also frees its counters.
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// Calls `f` for every valid instance value of a wildcard counter.
fn read_array(
    counter: PDH_HCOUNTER,
    format: PDH_FMT,
    mut f: impl FnMut(&str, &PDH_FMT_COUNTERVALUE_ITEM_W),
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
            f(&name, item);
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

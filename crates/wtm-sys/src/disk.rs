//! Activity of each physical disk from the "PhysicalDisk" performance
//! counters (the same source Windows Task Manager uses).

use std::collections::BTreeMap;

use windows::Win32::System::Performance::{
    PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW, PdhCloseQuery,
    PdhCollectQueryData, PdhOpenQueryW,
};
use windows::core::{PCWSTR, w};

use crate::Result;
use crate::gpu::{ArrayReader, pdh};

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DiskUsage {
    /// Instance name: disk number and drive letters, e.g. `0 C: D:`.
    pub name: String,
    /// Share of the time the disk was busy, 0..=100.
    pub active_percent: f32,
    pub read_rate: f64,
    pub write_rate: f64,
    /// Average time one transfer took, in milliseconds.
    #[serde(default)]
    pub response_ms: f32,
    /// Average number of requests waiting or in service.
    #[serde(default)]
    pub queue_length: f32,
}

impl DiskUsage {
    /// The disk number at the start of the instance name.
    pub fn number(&self) -> Option<u32> {
        self.name.split_whitespace().next()?.parse().ok()
    }

    /// The drive letters in the instance name, e.g. `["C:", "D:"]`.
    pub fn letters(&self) -> impl Iterator<Item = &str> {
        self.name.split_whitespace().skip(1)
    }
}

/// Counters of one disk, in the order they are added to the query.
#[derive(Clone, Copy)]
enum Field {
    Idle,
    Read,
    Write,
    Response,
    Queue,
}

/// Puts one counter value into its disk's entry.
fn apply(d: &mut DiskUsage, field: Field, v: f64) {
    let v = if v.is_finite() { v } else { 0.0 };
    match field {
        Field::Idle => d.active_percent = (100.0 - v).clamp(0.0, 100.0) as f32,
        Field::Read => d.read_rate = v.max(0.0),
        Field::Write => d.write_rate = v.max(0.0),
        // Seconds per transfer.
        Field::Response => d.response_ms = (v * 1000.0).clamp(0.0, 1e7) as f32,
        Field::Queue => d.queue_length = v.clamp(0.0, 1e6) as f32,
    }
}

pub struct DiskSampler {
    query: PDH_HQUERY,
    idle: PDH_HCOUNTER,
    read: PDH_HCOUNTER,
    write: PDH_HCOUNTER,
    response: PDH_HCOUNTER,
    queue: PDH_HCOUNTER,
    reader: ArrayReader,
}

impl std::fmt::Debug for DiskSampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DiskSampler")
    }
}

// SAFETY: PDH query handles are not tied to the creating thread; all access
// goes through `&mut self`, so they are never used concurrently.
unsafe impl Send for DiskSampler {}

impl DiskSampler {
    pub fn new() -> Result<Self> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: real-time query; `query` receives the handle.
        pdh("PdhOpenQueryW", unsafe {
            PdhOpenQueryW(PCWSTR::null(), 0, &mut query)
        })?;
        let mut s = Self {
            query,
            idle: PDH_HCOUNTER::default(),
            read: PDH_HCOUNTER::default(),
            write: PDH_HCOUNTER::default(),
            response: PDH_HCOUNTER::default(),
            queue: PDH_HCOUNTER::default(),
            reader: ArrayReader::default(),
        };
        for (path, counter) in [
            (w!(r"\PhysicalDisk(*)\% Idle Time"), &mut s.idle),
            (w!(r"\PhysicalDisk(*)\Disk Read Bytes/sec"), &mut s.read),
            (w!(r"\PhysicalDisk(*)\Disk Write Bytes/sec"), &mut s.write),
            (
                w!(r"\PhysicalDisk(*)\Avg. Disk sec/Transfer"),
                &mut s.response,
            ),
            (w!(r"\PhysicalDisk(*)\Avg. Disk Queue Length"), &mut s.queue),
        ] {
            // SAFETY: valid query; static, NUL-terminated counter path.
            pdh("PdhAddEnglishCounterW", unsafe {
                PdhAddEnglishCounterW(query, path, 0, counter)
            })?;
        }
        // Rate counters: prime them so the next sample has a delta.
        // SAFETY: valid query.
        pdh("PdhCollectQueryData", unsafe { PdhCollectQueryData(query) })?;
        Ok(s)
    }

    /// One entry per physical disk, by name; `_Total` is left out.
    pub fn sample(&mut self) -> Result<Vec<DiskUsage>> {
        // SAFETY: valid query.
        pdh("PdhCollectQueryData", unsafe {
            PdhCollectQueryData(self.query)
        })?;
        let mut disks: BTreeMap<String, DiskUsage> = BTreeMap::new();
        for (counter, field) in [
            (self.idle, Field::Idle),
            (self.read, Field::Read),
            (self.write, Field::Write),
            (self.response, Field::Response),
            (self.queue, Field::Queue),
        ] {
            self.reader.read(counter, PDH_FMT_DOUBLE, |name, item| {
                // SAFETY: requested as PDH_FMT_DOUBLE.
                let v = unsafe { item.FmtValue.Anonymous.doubleValue };
                if name == "_Total" {
                    return;
                }
                // The counters report the same disks: name each once.
                if !disks.contains_key(name) {
                    let d = DiskUsage {
                        name: name.to_owned(),
                        ..Default::default()
                    };
                    disks.insert(name.to_owned(), d);
                }
                let Some(d) = disks.get_mut(name) else {
                    return;
                };
                apply(d, field, v);
            })?;
        }
        Ok(disks.into_values().collect())
    }
}

impl Drop for DiskSampler {
    fn drop(&mut self) {
        // SAFETY: we own the query; closing it also frees its counters.
        unsafe { PdhCloseQuery(self.query) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_counter_values_and_instance_names() {
        let mut d = DiskUsage {
            name: "1 D: E:".into(),
            ..Default::default()
        };
        apply(&mut d, Field::Idle, 75.0);
        apply(&mut d, Field::Read, -4.0);
        apply(&mut d, Field::Write, 2048.0);
        apply(&mut d, Field::Response, 0.0042);
        apply(&mut d, Field::Queue, f64::NAN);
        assert_eq!(d.active_percent, 25.0);
        assert_eq!((d.read_rate, d.write_rate), (0.0, 2048.0));
        assert!((d.response_ms - 4.2).abs() < 1e-4, "{}", d.response_ms);
        assert_eq!(d.queue_length, 0.0, "not a number reads as none");
        assert_eq!(d.number(), Some(1));
        assert_eq!(d.letters().collect::<Vec<_>>(), ["D:", "E:"]);
        let odd = DiskUsage {
            name: "HarddiskVolume".into(),
            ..Default::default()
        };
        assert_eq!(odd.number(), None);
    }

    #[test]
    fn samples_every_physical_disk() {
        let mut s = DiskSampler::new().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let disks = s.sample().unwrap();
        assert!(!disks.is_empty(), "this machine has a disk");
        for d in &disks {
            assert_ne!(d.name, "_Total");
            assert!((0.0..=100.0).contains(&d.active_percent), "{d:?}");
            assert!(d.read_rate >= 0.0 && d.write_rate >= 0.0);
            assert!(d.response_ms >= 0.0 && d.queue_length >= 0.0, "{d:?}");
        }
        assert!(format!("{s:?}") == "DiskSampler");
    }
}

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
}

pub struct DiskSampler {
    query: PDH_HQUERY,
    idle: PDH_HCOUNTER,
    read: PDH_HCOUNTER,
    write: PDH_HCOUNTER,
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
            reader: ArrayReader::default(),
        };
        for (path, counter) in [
            (w!(r"\PhysicalDisk(*)\% Idle Time"), &mut s.idle),
            (w!(r"\PhysicalDisk(*)\Disk Read Bytes/sec"), &mut s.read),
            (w!(r"\PhysicalDisk(*)\Disk Write Bytes/sec"), &mut s.write),
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
        for (counter, field) in [(self.idle, 0), (self.read, 1), (self.write, 2)] {
            self.reader.read(counter, PDH_FMT_DOUBLE, |name, item| {
                // SAFETY: requested as PDH_FMT_DOUBLE.
                let v = unsafe { item.FmtValue.Anonymous.doubleValue };
                if name == "_Total" {
                    return;
                }
                // The three counters report the same disks: name each once.
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
                match field {
                    0 => d.active_percent = (100.0 - v).clamp(0.0, 100.0) as f32,
                    1 => d.read_rate = v.max(0.0),
                    _ => d.write_rate = v.max(0.0),
                }
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
    fn samples_every_physical_disk() {
        let mut s = DiskSampler::new().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let disks = s.sample().unwrap();
        assert!(!disks.is_empty(), "this machine has a disk");
        for d in &disks {
            assert_ne!(d.name, "_Total");
            assert!((0.0..=100.0).contains(&d.active_percent), "{d:?}");
            assert!(d.read_rate >= 0.0 && d.write_rate >= 0.0);
        }
        assert!(format!("{s:?}") == "DiskSampler");
    }
}

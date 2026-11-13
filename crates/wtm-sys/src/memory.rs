//! Where physical memory goes: in use, modified, standby and free pages,
//! kernel pools and memory the hardware keeps for itself. Every source here
//! works without administrator rights.

use std::cell::RefCell;

use windows::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW,
    PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows::Win32::System::SystemInformation::GetPhysicallyInstalledSystemMemory;
use windows::core::{PCWSTR, w};

use crate::Result;
use crate::gpu::pdh;

/// A breakdown of physical memory, in bytes. Page lists are `None` when the
/// "Memory" performance counters are unavailable (disabled or corrupt).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Composition {
    /// Memory Windows can use.
    pub total: u64,
    pub available: u64,
    /// Memory installed in the PC, from the firmware tables.
    pub installed: Option<u64>,
    /// Kernel memory that can be paged out, and that must stay in RAM.
    pub paged_pool: u64,
    pub nonpaged_pool: u64,
    /// Changed pages that must be written to disk before reuse.
    pub modified: Option<u64>,
    /// Cached pages that are ready to be reused at once.
    pub standby: Option<u64>,
    /// Pages holding nothing.
    pub free: Option<u64>,
}

impl Composition {
    pub fn in_use(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    /// Installed memory that Windows cannot use: firmware, integrated
    /// graphics and devices keep it.
    pub fn hardware_reserved(&self) -> Option<u64> {
        self.installed
            .filter(|installed| *installed >= self.total)
            .map(|installed| installed - self.total)
    }

    /// Standby and modified pages: memory that holds cached data.
    pub fn cached(&self) -> Option<u64> {
        Some(self.standby? + self.modified?)
    }
}

/// The current breakdown. The first call on a thread opens the counter
/// query that later calls on that thread reuse.
pub fn composition() -> Result<Composition> {
    let mut info = PERFORMANCE_INFORMATION {
        cb: size_of::<PERFORMANCE_INFORMATION>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is writable for the size passed in `cb`.
    unsafe { GetPerformanceInfo(&mut info, info.cb)? };
    let page = info.PageSize as u64;
    let mut installed_kib = 0u64;
    // SAFETY: the out-pointer references a live u64. Fails on firmware
    // without memory tables; installed size is then unknown.
    let installed = unsafe { GetPhysicallyInstalledSystemMemory(&mut installed_kib) }
        .ok()
        .map(|()| installed_kib.saturating_mul(1024));
    let lists = PAGE_LISTS.with(|cell| {
        let mut lists = cell.try_borrow_mut().ok()?;
        if lists.is_none() {
            *lists = PageLists::open().ok();
        }
        lists.as_mut()?.read().ok()
    });
    Ok(Composition {
        total: (info.PhysicalTotal as u64).saturating_mul(page),
        available: (info.PhysicalAvailable as u64).saturating_mul(page),
        installed,
        paged_pool: (info.KernelPaged as u64).saturating_mul(page),
        nonpaged_pool: (info.KernelNonpaged as u64).saturating_mul(page),
        modified: lists.map(|l| l.modified),
        standby: lists.map(|l| l.standby),
        free: lists.map(|l| l.free),
    })
}

thread_local! {
    static PAGE_LISTS: RefCell<Option<PageLists>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy)]
struct ListSizes {
    modified: u64,
    standby: u64,
    free: u64,
}

/// The "Memory" counters for the page lists. They are sizes, not rates, so
/// one collection gives current values.
struct PageLists {
    query: PDH_HQUERY,
    modified: PDH_HCOUNTER,
    /// Windows splits the standby list by priority: reserve, normal and core.
    standby: [PDH_HCOUNTER; 3],
    free: PDH_HCOUNTER,
}

impl PageLists {
    fn open() -> Result<Self> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: real-time query; `query` receives the handle.
        pdh("PdhOpenQueryW", unsafe {
            PdhOpenQueryW(PCWSTR::null(), 0, &mut query)
        })?;
        // Owned from here on, so a failure below closes the query.
        let mut lists = Self {
            query,
            modified: PDH_HCOUNTER::default(),
            standby: [PDH_HCOUNTER::default(); 3],
            free: PDH_HCOUNTER::default(),
        };
        let [reserve, normal, core] = &mut lists.standby;
        for (path, counter) in [
            (w!(r"\Memory\Modified Page List Bytes"), &mut lists.modified),
            (w!(r"\Memory\Standby Cache Reserve Bytes"), reserve),
            (w!(r"\Memory\Standby Cache Normal Priority Bytes"), normal),
            (w!(r"\Memory\Standby Cache Core Bytes"), core),
            (w!(r"\Memory\Free & Zero Page List Bytes"), &mut lists.free),
        ] {
            // SAFETY: valid query; static, NUL-terminated counter path.
            pdh("PdhAddEnglishCounterW", unsafe {
                PdhAddEnglishCounterW(query, path, 0, counter)
            })?;
        }
        Ok(lists)
    }

    fn read(&mut self) -> Result<ListSizes> {
        // SAFETY: valid query.
        pdh("PdhCollectQueryData", unsafe {
            PdhCollectQueryData(self.query)
        })?;
        let value = |counter| -> Result<u64> {
            let mut v = PDH_FMT_COUNTERVALUE::default();
            // SAFETY: valid counter of this query; `v` is writable.
            pdh("PdhGetFormattedCounterValue", unsafe {
                PdhGetFormattedCounterValue(counter, PDH_FMT_LARGE, None, &mut v)
            })?;
            // SAFETY: requested as PDH_FMT_LARGE.
            Ok(u64::try_from(unsafe { v.Anonymous.largeValue }).unwrap_or(0))
        };
        let mut standby = 0u64;
        for counter in self.standby {
            standby = standby.saturating_add(value(counter)?);
        }
        Ok(ListSizes {
            modified: value(self.modified)?,
            standby,
            free: value(self.free)?,
        })
    }
}

impl Drop for PageLists {
    fn drop(&mut self) {
        // SAFETY: we own the query; closing it also frees its counters.
        unsafe { PdhCloseQuery(self.query) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_sizes_never_underflow_or_guess() {
        let c = Composition {
            total: 16 << 30,
            available: 6 << 30,
            installed: Some(17 << 30),
            modified: Some(1 << 28),
            standby: Some(4 << 30),
            ..Composition::default()
        };
        assert_eq!(c.in_use(), 10 << 30);
        assert_eq!(c.hardware_reserved(), Some(1 << 30));
        assert_eq!(c.cached(), Some((4 << 30) + (1 << 28)));
        let odd = Composition {
            total: 16 << 30,
            installed: Some(8 << 30),
            ..Composition::default()
        };
        assert_eq!(
            odd.hardware_reserved(),
            None,
            "never a negative reservation"
        );
        assert_eq!(odd.cached(), None, "unknown lists stay unknown");
    }

    #[test]
    fn this_pc_reports_a_consistent_breakdown() {
        let c = composition().unwrap();
        assert!(c.total > 0 && c.available <= c.total, "{c:?}");
        assert!(c.paged_pool > 0 && c.nonpaged_pool > 0, "{c:?}");
        if let (Some(standby), Some(free), Some(modified)) = (c.standby, c.free, c.modified) {
            // The page lists are parts of physical memory.
            assert!(standby + free + modified <= c.total + (64 << 20), "{c:?}");
        }
        // The second call reuses the open query.
        assert!(composition().is_ok());
    }
}

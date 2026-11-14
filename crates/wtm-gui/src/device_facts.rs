//! Per-device history and the facts the Performance pages list under each
//! graph, as Windows Task Manager does: what each disk, network adapter and
//! GPU is, and processor details. The sample carries each device's
//! readings; the facts are read by this window, on the worker thread, at a
//! slower pace than samples.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

pub use wtm_core::devices::GpuFacts;
use wtm_sys::cpu_info::{Activity, CpuInfo};
pub use wtm_sys::gpu_perf::GpuPerf;
use wtm_sys::net_info::{Addresses, WifiLink};
pub use wtm_sys::nvme_health::Health as NvmeHealth;
use wtm_sys::physical_disk::{DiskTemperature, PhysicalDisk};
use wtm_sys::smbios::MemoryModule;

/// The battery charge series.
pub const BATTERY_CHARGE: &str = "battery:charge";

/// Most series kept: a dozen devices of a few series each fit many times.
pub const MAX_SERIES: usize = 512;

/// One series per device measurement, aligned with `History::times`: a
/// device absent from a sample has a gap (NaN) there, never a guessed
/// value.
#[derive(Debug, Default)]
pub struct DeviceSeries {
    series: BTreeMap<String, VecDeque<f32>>,
}

impl DeviceSeries {
    pub fn get(&self, key: &str) -> Option<&VecDeque<f32>> {
        self.series.get(key)
    }

    pub fn clear(&mut self) {
        self.series.clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.series.len()
    }

    /// Adds one sample's readings after `times` grew to `len` entries.
    /// Series the sample lacks get a gap; series that hold only gaps (the
    /// device left the whole window) are dropped.
    pub fn record(&mut self, len: usize, readings: impl IntoIterator<Item = (String, f32)>) {
        let mut seen = std::collections::BTreeSet::new();
        for (key, value) in readings {
            if !self.series.contains_key(&key) && self.series.len() >= MAX_SERIES {
                continue;
            }
            let series = self.series.entry(key.clone()).or_default();
            align(series, len.saturating_sub(1));
            series.push_back(value);
            seen.insert(key);
        }
        self.series.retain(|key, series| {
            if !seen.contains(key) {
                align(series, len.saturating_sub(1));
                series.push_back(f32::NAN);
            }
            align(series, len);
            series.iter().any(|v| v.is_finite())
        });
    }
}

/// Makes `series` exactly `len` long: the oldest values go first, and
/// missing ones are gaps at the start.
fn align(series: &mut VecDeque<f32>, len: usize) {
    while series.len() > len {
        series.pop_front();
    }
    while series.len() < len {
        series.push_front(f32::NAN);
    }
}

pub fn disk_key(number: u32, measure: &str) -> String {
    format!("disk:{number}:{measure}")
}

pub fn net_key(luid: u64, measure: &str) -> String {
    format!("net:{luid:016x}:{measure}")
}

pub fn gpu_key(luid: &str, measure: &str) -> String {
    format!("gpu:{luid}:{measure}")
}

/// Every device reading of a sample, keyed for [`DeviceSeries`].
pub fn readings(s: &wtm_core::SystemStats) -> Vec<(String, f32)> {
    let mut out = Vec::new();
    for d in &s.disks {
        let Some(n) = d.number() else { continue };
        out.push((disk_key(n, "active"), d.active_percent));
        out.push((disk_key(n, "read"), d.read_rate as f32));
        out.push((disk_key(n, "write"), d.write_rate as f32));
    }
    for a in &s.net_adapters {
        out.push((net_key(a.luid, "rx"), a.rx_rate as f32));
        out.push((net_key(a.luid, "tx"), a.tx_rate as f32));
    }
    for g in &s.gpus {
        out.push((gpu_key(&g.luid, "load"), g.percent));
        for (engine, load) in &g.engines {
            out.push((gpu_key(&g.luid, &format!("engine:{engine}")), *load));
        }
        if let Some(bytes) = g.dedicated_used {
            out.push((gpu_key(&g.luid, "dedicated"), bytes as f32));
        }
        if let Some(bytes) = g.shared_used {
            out.push((gpu_key(&g.luid, "shared"), bytes as f32));
        }
    }
    out
}

/// Processor time spent on hardware, over the last sample.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CpuBreakdown {
    pub dpc_percent: f32,
    pub interrupt_percent: f32,
    pub interrupts_per_sec: Option<f64>,
}

/// DPC and interrupt shares of all processors' time between two readings
/// of the same processors, and the interrupt rate over `secs`.
pub fn breakdown(
    before: &[wtm_sys::system::CpuTimes],
    after: &[wtm_sys::system::CpuTimes],
    secs: f64,
) -> Option<CpuBreakdown> {
    if before.len() != after.len() || after.is_empty() {
        return None;
    }
    let (mut total, mut dpc, mut interrupt, mut count) = (0u64, 0u64, 0u64, 0u64);
    for (a, b) in before.iter().zip(after) {
        total += b.kernel.saturating_sub(a.kernel) + b.user.saturating_sub(a.user);
        dpc += b.dpc.saturating_sub(a.dpc);
        interrupt += b.interrupt.saturating_sub(a.interrupt);
        count += b.interrupts.saturating_sub(a.interrupts);
    }
    if total == 0 {
        return None;
    }
    let share = |part: u64| (part as f64 / total as f64 * 100.0).clamp(0.0, 100.0) as f32;
    Some(CpuBreakdown {
        dpc_percent: share(dpc),
        interrupt_percent: share(interrupt),
        interrupts_per_sec: (secs > 0.0).then(|| count as f64 / secs),
    })
}

/// The facts the Performance pages show beside the sample.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub cpu: Option<CpuInfo>,
    pub activity: Activity,
    pub breakdown: Option<CpuBreakdown>,
    pub uptime: Option<Duration>,
    /// Memory slots, empty ones included; `None` until read.
    pub memory: Option<Vec<MemoryModule>>,
    pub disks: Vec<PhysicalDisk>,
    pub volumes: Vec<wtm_core::Volume>,
    /// Drive of the Windows folder, e.g. `C:`.
    pub system_drive: Option<String>,
    /// Drives that hold a page file.
    pub page_file_drives: Vec<String>,
    pub addresses: Vec<Addresses>,
    pub wifi: Vec<WifiLink>,
    pub gpus: Vec<GpuFacts>,
    /// Each GPU's temperature, fan and memory clock, by LUID.
    pub gpu_perf: Vec<(String, GpuPerf)>,
    /// Temperature and wear of the disk whose page is open.
    pub disk_health: Option<DiskHealth>,
    /// The system battery, on PCs that have one.
    pub battery: Option<wtm_sys::power::BatteryStatus>,
    /// Each logical processor's current speed in MHz.
    pub core_mhz: Vec<f64>,
}

/// What a disk says about its own state.
#[derive(Debug, Clone)]
pub struct DiskHealth {
    pub number: u32,
    pub temperature: Option<DiskTemperature>,
    /// The NVMe health log, for NVMe drives.
    pub nvme: Option<NvmeHealth>,
}

/// The pages open in the window whose details are read only while shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Wanted {
    /// A Wi-Fi adapter's page: from Windows 11 24H2 reading its network may
    /// ask the user for location access.
    pub wifi: bool,
    /// A disk's page: reading its temperature may wake a sleeping drive.
    pub disk: Option<u32>,
}

impl Facts {
    pub fn disk(&self, number: u32) -> Option<&PhysicalDisk> {
        self.disks.iter().find(|d| d.number == number)
    }

    pub fn gpu(&self, luid: &str) -> Option<&GpuFacts> {
        self.gpus.iter().find(|g| g.luid == luid)
    }

    pub fn addresses(&self, luid: u64) -> Option<&Addresses> {
        self.addresses.iter().find(|a| a.luid == luid)
    }

    pub fn wifi(&self, luid: u64) -> Option<&WifiLink> {
        self.wifi.iter().find(|w| w.luid == luid)
    }

    pub fn gpu_perf(&self, luid: &str) -> Option<&GpuPerf> {
        self.gpu_perf
            .iter()
            .find(|(l, _)| l == luid)
            .map(|(_, p)| p)
    }

    pub fn disk_health(&self, number: u32) -> Option<&DiskHealth> {
        self.disk_health.as_ref().filter(|h| h.number == number)
    }
}

/// How often the slower facts are read again.
const ADDRESSES_EVERY: Duration = Duration::from_secs(5);
const VOLUMES_EVERY: Duration = Duration::from_secs(60);
const GPUS_EVERY: Duration = Duration::from_secs(60);
const DISK_HEALTH_EVERY: Duration = Duration::from_secs(5);

/// Reads the facts on the worker thread. Each source is read when due, and
/// again when the devices in the sample change (a disk plugged in).
#[derive(Default)]
pub struct Collector {
    facts: Facts,
    addresses_at: Option<Instant>,
    wifi_at: Option<Instant>,
    volumes_at: Option<Instant>,
    disks_for: Vec<u32>,
    gpus_at: Option<Instant>,
    gpus_for: Vec<String>,
    health_at: Option<Instant>,
}

/// Drives in Windows' list of page files in use
/// (`\??\C:\pagefile.sys` entries of `ExistingPageFiles`).
fn page_file_drives() -> Vec<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management")
        .and_then(|key| key.get_value::<Vec<String>, _>("ExistingPageFiles"))
        .map(|files| drives_of(&files))
        .unwrap_or_default()
}

fn drives_of(files: &[String]) -> Vec<String> {
    let mut drives: Vec<String> = files
        .iter()
        .take(26)
        .filter_map(|f| {
            let path = f.trim_start_matches(r"\??\");
            let mut chars = path.chars();
            match (chars.next(), chars.next()) {
                (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => {
                    Some(format!("{}:", letter.to_ascii_uppercase()))
                }
                _ => None,
            }
        })
        .collect();
    drives.dedup();
    drives
}

/// Whether a disk's temperature and wear may be read: only solid-state
/// drives on an internal bus answer at once. On a spinning drive the query
/// becomes an ATA command that can spin it up for seconds, on the thread
/// that also carries out commands; USB bridges rarely pass it on.
fn reads_health(disk: Option<&PhysicalDisk>) -> bool {
    disk.is_some_and(|d| d.bus == 17 || (d.ssd == Some(true) && d.bus != 7))
}

fn due(at: Option<Instant>, every: Duration, now: Instant) -> bool {
    at.is_none_or(|at| now.saturating_duration_since(at) >= every)
}

impl Collector {
    /// The facts, refreshed for a sample with these disks (number and drive
    /// letters) and GPU LUIDs. Wi-Fi details and a disk's temperature and
    /// wear are read only while their page is open (`wanted`).
    ///
    /// Everything here answers at once: identity of local disks only when the
    /// set of disks changes, the volumes of their letters only (never network
    /// drives), and the page files from the registry.
    pub fn collect(
        &mut self,
        disks: &[(u32, Vec<String>)],
        gpus: &[String],
        wanted: Wanted,
    ) -> Facts {
        let now = Instant::now();
        let f = &mut self.facts;
        if f.cpu.is_none() {
            f.cpu = Some(wtm_sys::cpu_info::info());
        }
        if f.memory.is_none() {
            f.memory = Some(wtm_sys::smbios::memory_modules().unwrap_or_default());
        }
        f.activity = wtm_sys::cpu_info::activity();
        f.core_mhz = wtm_sys::cpu_info::core_mhz();
        f.battery = wtm_sys::power::battery();
        f.uptime = Some(wtm_sys::cpu_info::uptime());
        if due(self.addresses_at, ADDRESSES_EVERY, now) {
            f.addresses = wtm_sys::net_info::addresses().unwrap_or_default();
            self.addresses_at = Some(now);
        }
        if wanted.wifi && due(self.wifi_at, ADDRESSES_EVERY, now) {
            f.wifi = wtm_sys::net_info::wifi_links();
            self.wifi_at = Some(now);
        }
        let numbers: Vec<u32> = disks.iter().map(|(n, _)| *n).take(64).collect();
        let changed = self.disks_for != numbers;
        if changed {
            f.disks = numbers
                .iter()
                .filter_map(|n| wtm_sys::physical_disk::read(*n).ok())
                .collect();
            f.system_drive = std::env::var("SystemDrive")
                .ok()
                .map(|d| d.trim_end_matches('\\').to_ascii_uppercase());
            f.page_file_drives = page_file_drives();
            self.disks_for = numbers;
        }
        if changed || due(self.volumes_at, VOLUMES_EVERY, now) {
            let roots: Vec<String> = disks
                .iter()
                .flat_map(|(_, letters)| letters.iter().map(|l| format!("{l}\\")))
                .collect();
            f.volumes = wtm_sys::volumes::volumes_at(&roots);
            self.volumes_at = Some(now);
        }
        if due(self.gpus_at, GPUS_EVERY, now) || self.gpus_for != gpus {
            f.gpus = wtm_core::devices::gpu_facts();
            self.gpus_at = Some(now);
            self.gpus_for = gpus.to_vec();
        }
        // A kernel query per GPU, answered by the driver at once.
        f.gpu_perf = gpus
            .iter()
            .take(16)
            .filter_map(|l| Some((l.clone(), wtm_sys::gpu_perf::read(l)?)))
            .collect();
        let wanted_disk = wanted
            .disk
            .filter(|n| self.disks_for.contains(n) && reads_health(f.disk(*n)));
        match wanted_disk {
            Some(number) => {
                let other = f.disk_health.as_ref().is_none_or(|h| h.number != number);
                if other || due(self.health_at, DISK_HEALTH_EVERY, now) {
                    let nvme = f.disk(number).is_some_and(|d| d.bus == 17);
                    f.disk_health = Some(DiskHealth {
                        number,
                        temperature: wtm_sys::physical_disk::temperature(number).ok(),
                        nvme: nvme
                            .then(|| wtm_sys::nvme_health::read(number).ok())
                            .flatten(),
                    });
                    self.health_at = Some(now);
                }
            }
            None => f.disk_health = None,
        }
        f.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(s: &DeviceSeries, key: &str) -> Vec<Option<f32>> {
        s.get(key)
            .unwrap()
            .iter()
            .map(|v| v.is_finite().then_some(*v))
            .collect()
    }

    #[test]
    fn device_series_stay_aligned_as_devices_come_and_go() {
        let mut s = DeviceSeries::default();
        s.record(1, [("a".to_owned(), 1.0)]);
        // "b" appears at the third sample: two gaps before it.
        s.record(2, [("a".to_owned(), 2.0)]);
        s.record(3, [("a".to_owned(), 3.0), ("b".to_owned(), 30.0)]);
        // "a" is missing once.
        s.record(4, [("b".to_owned(), 40.0)]);
        assert_eq!(values(&s, "a"), [Some(1.0), Some(2.0), Some(3.0), None]);
        assert_eq!(values(&s, "b"), [None, None, Some(30.0), Some(40.0)]);
        // The timeline is trimmed to 3: the oldest values go.
        s.record(3, [("a".to_owned(), 5.0)]);
        assert_eq!(values(&s, "a"), [Some(3.0), None, Some(5.0)]);
        assert_eq!(values(&s, "b"), [Some(30.0), Some(40.0), None]);
        // "b" has left the whole window: dropped.
        s.record(3, [("a".to_owned(), 6.0)]);
        s.record(3, [("a".to_owned(), 7.0)]);
        assert!(s.get("b").is_none());
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn only_internal_solid_state_drives_are_asked_for_their_health() {
        let disk = |bus, ssd| PhysicalDisk {
            bus,
            ssd,
            ..Default::default()
        };
        assert!(reads_health(Some(&disk(17, Some(true)))), "NVMe");
        assert!(
            reads_health(Some(&disk(17, None))),
            "NVMe without seek data"
        );
        assert!(reads_health(Some(&disk(11, Some(true)))), "SATA SSD");
        assert!(!reads_health(Some(&disk(11, Some(false)))), "SATA HDD");
        assert!(!reads_health(Some(&disk(11, None))), "unknown SATA drive");
        assert!(!reads_health(Some(&disk(7, Some(true)))), "USB SSD");
        assert!(!reads_health(None), "identity not read yet");
    }

    #[test]
    fn page_files_name_their_drives() {
        let files = [
            r"\??\C:\pagefile.sys".to_owned(),
            r"\??\d:\swap\pagefile.sys".to_owned(),
            "garbage".to_owned(),
        ];
        assert_eq!(drives_of(&files), ["C:", "D:"]);
        assert!(drives_of(&[]).is_empty());
    }

    #[test]
    fn device_series_are_capped() {
        let mut s = DeviceSeries::default();
        s.record(1, (0..MAX_SERIES + 20).map(|i| (format!("k{i}"), 1.0)));
        assert_eq!(s.len(), MAX_SERIES);
    }

    #[test]
    fn readings_key_every_device() {
        let s = wtm_core::SystemStats {
            disks: vec![
                wtm_core::DiskUsage {
                    name: "0 C:".into(),
                    active_percent: 12.0,
                    ..Default::default()
                },
                // No disk number: skipped.
                wtm_core::DiskUsage {
                    name: "odd".into(),
                    ..Default::default()
                },
            ],
            net_adapters: vec![wtm_core::NetAdapterUsage {
                luid: 0xAB,
                rx_rate: 10.0,
                ..Default::default()
            }],
            gpus: vec![wtm_sys::gpu::GpuAdapterUsage {
                luid: "00000000_0000c9d3".into(),
                percent: 40.0,
                engines: vec![("3D".into(), 40.0)],
                dedicated_used: Some(1024),
                shared_used: None,
            }],
            ..Default::default()
        };
        let r: BTreeMap<String, f32> = readings(&s).into_iter().collect();
        assert_eq!(r.get("disk:0:active"), Some(&12.0));
        assert_eq!(r.get("net:00000000000000ab:rx"), Some(&10.0));
        assert_eq!(r.get("gpu:00000000_0000c9d3:engine:3D"), Some(&40.0));
        assert_eq!(r.get("gpu:00000000_0000c9d3:dedicated"), Some(&1024.0));
        assert!(!r.contains_key("gpu:00000000_0000c9d3:shared"));
        assert_eq!(r.len(), 3 + 2 + 3);
    }

    #[test]
    fn breakdown_measures_dpc_interrupts_and_their_rate() {
        let t = |kernel, user, dpc, interrupt, interrupts| wtm_sys::system::CpuTimes {
            kernel,
            user,
            dpc,
            interrupt,
            interrupts,
            ..Default::default()
        };
        let before = [t(1000, 1000, 10, 5, 100), t(1000, 1000, 0, 0, 0)];
        let after = [t(1600, 1400, 30, 15, 300), t(1500, 1500, 20, 0, 100)];
        let b = breakdown(&before, &after, 2.0).unwrap();
        // 2000 units of processor time: 40 in DPCs, 10 in interrupts.
        assert_eq!(b.dpc_percent, 2.0);
        assert_eq!(b.interrupt_percent, 0.5);
        assert_eq!(b.interrupts_per_sec, Some(150.0));
        assert_eq!(breakdown(&before, &after[..1], 1.0), None);
        assert_eq!(breakdown(&before, &before, 1.0), None, "no time passed");
        assert_eq!(
            breakdown(&before, &after, 0.0).unwrap().interrupts_per_sec,
            None
        );
    }
}

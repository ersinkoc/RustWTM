use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 100 ns intervals between 1601-01-01 and 1970-01-01.
const FILETIME_UNIX_OFFSET: i64 = 116_444_736_000_000_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    /// Share of total machine capacity, 0..=100.
    pub cpu_percent: f32,
    pub cpu_time: Duration,
    pub working_set: u64,
    pub private_working_set: u64,
    pub private_bytes: u64,
    pub thread_count: u32,
    pub handle_count: u32,
    pub session_id: u32,
    pub base_priority: i32,
    /// Busiest GPU engine used by this process, 0..=100.
    #[serde(default)]
    pub gpu_percent: f32,
    /// Dedicated GPU memory.
    #[serde(default)]
    pub gpu_memory: u64,
    pub io_read_rate: f64,
    pub io_write_rate: f64,
    /// Network bytes per second, from an ETW trace that needs administrator
    /// rights; zero when the trace is not running (see
    /// [`Snapshot::net_traced`]).
    #[serde(default)]
    pub net_rx_rate: f64,
    #[serde(default)]
    pub net_tx_rate: f64,
    /// Raw FILETIME; together with `pid` it uniquely identifies a process.
    pub create_time: i64,
    /// Every thread is suspended: the process is frozen until resumed.
    #[serde(default)]
    pub suspended: bool,
}

impl ProcessInfo {
    pub fn start_time(&self) -> Option<SystemTime> {
        let unix_100ns = self.create_time.checked_sub(FILETIME_UNIX_OFFSET)?;
        let unix_100ns = u64::try_from(unix_100ns).ok()?;
        let since = Duration::new(
            unix_100ns / 10_000_000,
            (unix_100ns % 10_000_000) as u32 * 100,
        );
        UNIX_EPOCH.checked_add(since)
    }

    pub fn io_rate(&self) -> f64 {
        self.io_read_rate + self.io_write_rate
    }

    /// Network bytes per second, both directions.
    pub fn net_rate(&self) -> f64 {
        self.net_rx_rate + self.net_tx_rate
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemStats {
    pub cpu_percent: f32,
    pub logical_cpus: u32,
    pub memory_total: u64,
    pub memory_available: u64,
    pub commit_limit: u64,
    pub commit_available: u64,
    /// Busiest GPU engine across all processes, 0..=100.
    #[serde(default)]
    pub gpu_percent: f32,
    pub process_count: usize,
    pub thread_count: u64,
    pub handle_count: u64,
    /// Load of each logical processor, 0..=100.
    #[serde(default)]
    pub core_percent: Vec<f32>,
    /// Network adapter throughput, bytes per second.
    #[serde(default)]
    pub net_rx_rate: f64,
    #[serde(default)]
    pub net_tx_rate: f64,
    /// I/O of all processes together, bytes per second.
    #[serde(default)]
    pub io_read_rate: f64,
    #[serde(default)]
    pub io_write_rate: f64,
    /// Every physical disk; empty without disk performance counters.
    #[serde(default)]
    pub disks: Vec<wtm_sys::disk::DiskUsage>,
    /// Load of every GPU engine type, by name, 0..=100.
    #[serde(default)]
    pub gpu_engines: Vec<(String, f32)>,
}

impl SystemStats {
    pub fn memory_used(&self) -> u64 {
        self.memory_total.saturating_sub(self.memory_available)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Time since the previous sample; `None` for the first one (rates are zero).
    pub interval: Option<Duration>,
    pub system: SystemStats,
    pub processes: Vec<ProcessInfo>,
    /// Per-process network rates were measured; without the trace they are
    /// zero and should read as unknown.
    #[serde(default)]
    pub net_traced: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Cpu,
    Memory,
    Disk,
    Pid,
    Name,
}

/// Sorts "bigger is worse" metrics descending, identifiers ascending.
pub fn sort_processes(procs: &mut [ProcessInfo], key: SortKey) {
    if key == SortKey::Name {
        // Lower-cases each name once, not twice per comparison.
        procs.sort_by_cached_key(|p| (p.name.to_lowercase(), p.pid));
        return;
    }
    procs.sort_by(|a, b| {
        let primary = match key {
            SortKey::Cpu => b.cpu_percent.total_cmp(&a.cpu_percent),
            SortKey::Memory => b.private_working_set.cmp(&a.private_working_set),
            SortKey::Disk => b.io_rate().total_cmp(&a.io_rate()),
            SortKey::Pid => Ordering::Equal,
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        };
        primary.then(a.pid.cmp(&b.pid))
    });
}

/// Formats a byte count with binary units, e.g. `1.5 KiB`.
/// `45s`, `3m 20s`, `1h 05m`, `2d 3h`.
pub fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m {:02}s", s / 60, s % 60),
        3600..86400 => format!("{}h {:02}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d {}h", s / 86400, s % 86400 / 3600),
    }
}

pub fn format_bytes(n: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{v:.0} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, name: &str, cpu: f32) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: 0,
            name: name.to_owned(),
            cpu_percent: cpu,
            cpu_time: Duration::ZERO,
            working_set: 0,
            private_working_set: u64::from(pid),
            private_bytes: 0,
            thread_count: 0,
            handle_count: 0,
            session_id: 0,
            base_priority: 8,
            gpu_percent: 0.0,
            gpu_memory: 0,
            io_read_rate: 0.0,
            io_write_rate: 0.0,
            net_rx_rate: 0.0,
            net_tx_rate: 0.0,
            suspended: false,
            create_time: FILETIME_UNIX_OFFSET,
        }
    }

    fn pids(procs: &[ProcessInfo]) -> Vec<u32> {
        procs.iter().map(|p| p.pid).collect()
    }

    #[test]
    fn sorts_by_each_key() {
        let mut v = vec![proc(3, "b", 1.0), proc(1, "C", 5.0), proc(2, "a", 5.0)];
        sort_processes(&mut v, SortKey::Cpu);
        assert_eq!(pids(&v), [1, 2, 3]);
        sort_processes(&mut v, SortKey::Memory);
        assert_eq!(pids(&v), [3, 2, 1]);
        sort_processes(&mut v, SortKey::Name);
        assert_eq!(pids(&v), [2, 3, 1]);
        sort_processes(&mut v, SortKey::Pid);
        assert_eq!(pids(&v), [1, 2, 3]);
        v[0].io_read_rate = 10.0;
        v[2].io_write_rate = 30.0;
        sort_processes(&mut v, SortKey::Disk);
        assert_eq!(pids(&v), [3, 1, 2]);
        assert_eq!(v[0].io_rate(), 30.0);
    }

    #[test]
    fn memory_used_never_underflows() {
        let mut s = SystemStats {
            memory_total: 100,
            memory_available: 30,
            ..Default::default()
        };
        assert_eq!(s.memory_used(), 70);
        s.memory_available = 200;
        assert_eq!(s.memory_used(), 0);
    }

    #[test]
    fn formats_bytes_with_binary_units() {
        assert_eq!(format_bytes(0.0), "0 B");
        let d = Duration::from_secs;
        assert_eq!(format_duration(d(45)), "45s");
        assert_eq!(format_duration(d(200)), "3m 20s");
        assert_eq!(format_duration(d(3900)), "1h 05m");
        assert_eq!(format_duration(d(2 * 86400 + 3 * 3600 + 5)), "2d 3h");
        assert_eq!(format_bytes(1023.0), "1023 B");
        assert_eq!(format_bytes(1536.0), "1.5 KiB");
        assert_eq!(format_bytes(3.0 * 1024.0 * 1024.0 * 1024.0), "3.0 GiB");
        // Beyond TiB the unit stops growing.
        assert_eq!(format_bytes(2048.0 * 1024f64.powi(4)), "2048.0 TiB");
    }

    #[test]
    fn start_time_converts_filetime() {
        assert_eq!(proc(1, "x", 0.0).start_time(), Some(UNIX_EPOCH));
        let mut idle = proc(0, "idle", 0.0);
        idle.create_time = 0;
        assert_eq!(idle.start_time(), None);
        // One and a half seconds after 1970, and no overflow at the far end.
        idle.create_time = FILETIME_UNIX_OFFSET + 15_000_000;
        assert_eq!(
            idle.start_time(),
            Some(UNIX_EPOCH + Duration::from_millis(1500))
        );
        idle.create_time = i64::MAX;
        assert!(idle.start_time().is_some());
    }

    /// `cargo test --release -p wtm-core sort_speed -- --ignored --nocapture`
    #[test]
    #[ignore = "timing harness"]
    fn sort_speed() {
        let base: Vec<ProcessInfo> = (0..2000u32)
            .map(|i| {
                proc(
                    i * 4,
                    &format!("Process{}.exe", (i * 7919) % 2000),
                    i as f32,
                )
            })
            .collect();
        for key in [SortKey::Cpu, SortKey::Name] {
            let runs = 100;
            let mut total = Duration::ZERO;
            for _ in 0..runs {
                let mut v = base.clone();
                let started = std::time::Instant::now();
                sort_processes(&mut v, key);
                total += started.elapsed();
                std::hint::black_box(&v);
            }
            println!("{key:?}: {:?} per sort of 2000", total / runs);
        }
    }
}

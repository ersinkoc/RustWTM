use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use wtm_sys::disk::DiskSampler;
use wtm_sys::gpu::GpuSampler;
use wtm_sys::process::{ProcessSnapshotter, RawProcess};
use wtm_sys::system::{self, CpuTimes, NetCounters};

use crate::model::{ProcessInfo, Snapshot, SystemStats};

/// Turns successive raw snapshots into rates (CPU %, I/O bytes/s).
#[derive(Debug)]
pub struct Sampler {
    snapshotter: ProcessSnapshotter,
    /// None on systems without GPU performance counters.
    gpu: Option<GpuSampler>,
    /// None on systems without disk performance counters.
    disks: Option<DiskSampler>,
    logical_cpus: u32,
    prev: Option<Previous>,
    /// Network bytes per process; needs administrator rights.
    net_trace: Option<wtm_sys::nettrace::NetTracer>,
    /// The counter map of the sample before the previous one, emptied and
    /// refilled next time so steady-state sampling does not reallocate it.
    spare: HashMap<(u32, i64), Counters>,
}

#[derive(Debug)]
struct Previous {
    at: Instant,
    cpu: CpuTimes,
    cores: Vec<CpuTimes>,
    net: Option<Vec<(u64, NetCounters)>>,
    // Keyed by (pid, create_time) so a reused PID does not inherit stale counters.
    procs: HashMap<(u32, i64), Counters>,
}

#[derive(Debug, Clone, Copy)]
struct Counters {
    cpu_time: u64,
    io_read: u64,
    io_write: u64,
}

impl Counters {
    fn of(p: &RawProcess) -> Self {
        Self {
            cpu_time: p.user_time + p.kernel_time,
            io_read: p.io_read_bytes,
            io_write: p.io_write_bytes,
        }
    }
}

impl ProcessInfo {
    /// Converts a raw entry; rates are zero because they need two samples.
    fn from_raw(p: RawProcess) -> Self {
        Self {
            pid: p.pid,
            parent_pid: p.parent_pid,
            name: p.name,
            cpu_percent: 0.0,
            cpu_time: Duration::from_nanos((p.user_time + p.kernel_time) * 100),
            working_set: p.working_set,
            private_working_set: p.private_working_set,
            private_bytes: p.private_bytes,
            thread_count: p.thread_count,
            handle_count: p.handle_count,
            session_id: p.session_id,
            base_priority: p.base_priority,
            gpu_percent: 0.0,
            gpu_memory: 0,
            io_read_rate: 0.0,
            io_write_rate: 0.0,
            net_rx_rate: 0.0,
            net_tx_rate: 0.0,
            suspended: p.suspended,
            create_time: p.create_time,
        }
    }
}

/// Cheaply detects newly started processes between full samples.
#[derive(Debug, Default)]
pub struct StartWatcher {
    snapshotter: ProcessSnapshotter,
    seen: HashSet<(u32, i64)>,
    /// The set before `seen`, refilled by the next poll (four times a second
    /// in the service) instead of allocating a new one.
    spare: HashSet<(u32, i64)>,
    primed: bool,
}

impl StartWatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Processes that appeared since the previous call. The first call only
    /// records what is already running and returns nothing.
    pub fn poll(&mut self) -> crate::Result<Vec<ProcessInfo>> {
        let mut current = std::mem::take(&mut self.spare);
        current.clear();
        current.reserve(self.seen.len());
        // Only new processes are converted and named; the rest are just
        // recorded by key.
        let (primed, seen) = (self.primed, &self.seen);
        let raw = self.snapshotter.snapshot_where(|pid, create_time| {
            let key = (pid, create_time);
            current.insert(key);
            primed && !seen.contains(&key)
        })?;
        let started = raw.into_iter().map(ProcessInfo::from_raw).collect();
        self.spare = std::mem::replace(&mut self.seen, current);
        self.primed = true;
        Ok(started)
    }
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        Self {
            snapshotter: ProcessSnapshotter::new(),
            gpu: GpuSampler::new().ok(),
            disks: DiskSampler::new().ok(),
            logical_cpus: system::logical_cpu_count(),
            prev: None,
            net_trace: None,
            spare: HashMap::new(),
        }
    }

    /// Starts counting network bytes per process (an ETW session called
    /// `session`). Needs administrator rights; without them the rates stay 0.
    pub fn enable_network_trace(&mut self, session: &str) -> crate::Result<()> {
        if self.net_trace.is_none() {
            self.net_trace = Some(wtm_sys::nettrace::NetTracer::start(session)?);
        }
        Ok(())
    }

    pub fn network_traced(&self) -> bool {
        self.net_trace.is_some()
    }

    pub fn sample(&mut self) -> crate::Result<Snapshot> {
        let raw = self.snapshotter.snapshot()?;
        let now = Instant::now();
        let cpu = system::cpu_times()?;
        let mem = system::memory_status()?;
        let cores = system::per_cpu_times().unwrap_or_default();
        let net = system::net_adapters().ok();
        let mut gpu = self
            .gpu
            .as_mut()
            .and_then(|g| g.sample().ok())
            .unwrap_or_default();

        let elapsed = self.prev.as_ref().map(|p| now - p.at);
        let elapsed_secs = elapsed.map_or(0.0, |d| d.as_secs_f64());
        // Total CPU capacity of the interval, in 100 ns units.
        let capacity = elapsed_secs * 1e7 * f64::from(self.logical_cpus);

        let mut system = SystemStats {
            cpu_percent: self
                .prev
                .as_ref()
                .map_or(0.0, |p| system_cpu_percent(p.cpu, cpu)),
            logical_cpus: self.logical_cpus,
            memory_total: mem.total_phys,
            memory_available: mem.avail_phys,
            commit_limit: mem.commit_limit,
            commit_available: mem.commit_avail,
            gpu_percent: gpu.total_percent,
            gpu_engines: std::mem::take(&mut gpu.engines),
            disks: self
                .disks
                .as_mut()
                .and_then(|d| d.sample().ok())
                .unwrap_or_default(),
            process_count: raw.len(),
            ..Default::default()
        };
        if let Some(prev) = &self.prev {
            if prev.cores.len() == cores.len() {
                system.core_percent = prev
                    .cores
                    .iter()
                    .zip(&cores)
                    .map(|(&a, &b)| system_cpu_percent(a, b))
                    .collect();
            }
            if let (Some(a), Some(b)) = (&prev.net, &net)
                && elapsed_secs > 0.0
            {
                let moved = net_delta(a, b);
                system.net_rx_rate = moved.received as f64 / elapsed_secs;
                system.net_tx_rate = moved.sent as f64 / elapsed_secs;
            }
        }

        let traffic = self
            .net_trace
            .as_ref()
            .map(wtm_sys::nettrace::NetTracer::take)
            .unwrap_or_default();
        let mut counters = std::mem::take(&mut self.spare);
        counters.clear();
        counters.reserve(raw.len());
        let mut processes = Vec::with_capacity(raw.len());
        for p in raw {
            let now_c = Counters::of(&p);
            let prev_c = self
                .prev
                .as_ref()
                .and_then(|prev| prev.procs.get(&(p.pid, p.create_time)));

            let (cpu_percent, io_read_rate, io_write_rate) = match prev_c {
                Some(c) if elapsed_secs > 0.0 => (
                    (now_c.cpu_time.saturating_sub(c.cpu_time) as f64 / capacity * 100.0)
                        .clamp(0.0, 100.0) as f32,
                    now_c.io_read.saturating_sub(c.io_read) as f64 / elapsed_secs,
                    now_c.io_write.saturating_sub(c.io_write) as f64 / elapsed_secs,
                ),
                _ => (0.0, 0.0, 0.0),
            };

            system.thread_count += u64::from(p.thread_count);
            system.handle_count += u64::from(p.handle_count);
            system.io_read_rate += io_read_rate;
            system.io_write_rate += io_write_rate;
            counters.insert((p.pid, p.create_time), now_c);

            let mut info = ProcessInfo::from_raw(p);
            info.cpu_percent = cpu_percent;
            info.io_read_rate = io_read_rate;
            info.io_write_rate = io_write_rate;
            if elapsed_secs > 0.0
                && let Some((rx, tx)) = traffic.get(&info.pid)
            {
                info.net_rx_rate = *rx as f64 / elapsed_secs;
                info.net_tx_rate = *tx as f64 / elapsed_secs;
            }
            if let Some(g) = gpu.per_process.get(&info.pid) {
                info.gpu_percent = g.percent;
                info.gpu_memory = g.dedicated_bytes;
            }
            processes.push(info);
        }

        let replaced = self.prev.replace(Previous {
            at: now,
            cpu,
            cores,
            net,
            procs: counters,
        });
        if let Some(old) = replaced {
            self.spare = old.procs;
        }
        Ok(Snapshot {
            interval: elapsed,
            system,
            processes,
            net_traced: self.network_traced(),
            taken_at_ms: crate::hardware::unix_ms(),
        })
    }
}

/// Traffic between two readings of the adapters, over the adapters present
/// in both: one that has just come up would bring its whole count since
/// boot, and one that went away would take its count out of the total. A
/// counter that went down was reset and counts as no traffic.
fn net_delta(prev: &[(u64, NetCounters)], now: &[(u64, NetCounters)]) -> NetCounters {
    let mut moved = NetCounters::default();
    for (luid, b) in now {
        if let Some((_, a)) = prev.iter().find(|(l, _)| l == luid) {
            moved.received += b.received.saturating_sub(a.received);
            moved.sent += b.sent.saturating_sub(a.sent);
        }
    }
    moved
}

fn system_cpu_percent(prev: CpuTimes, now: CpuTimes) -> f32 {
    let idle = now.idle.saturating_sub(prev.idle);
    // Kernel time includes idle time.
    let total = now.kernel.saturating_sub(prev.kernel) + now.user.saturating_sub(prev.user);
    if total == 0 {
        return 0.0;
    }
    ((total.saturating_sub(idle)) as f64 / total as f64 * 100.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_traffic_is_taken_per_adapter() {
        let c = |received, sent| NetCounters { received, sent };
        let before = [(1, c(1_000, 500)), (2, c(9_000_000_000, 7_000_000_000))];
        // Adapter 2 went away, adapter 3 (up again with its old count) came
        // back, and adapter 1 moved 300 B in and 50 B out.
        let after = [(3, c(5_000_000_000, 4_000_000_000)), (1, c(1_300, 550))];
        assert_eq!(net_delta(&before, &after), c(300, 50));
        // A counter that went down was reset: no traffic, not a wrap.
        assert_eq!(net_delta(&[(1, c(800, 80))], &[(1, c(100, 10))]), c(0, 0));
        assert_eq!(net_delta(&[], &after), c(0, 0));
    }

    #[test]
    fn system_cpu_percent_math() {
        let a = CpuTimes {
            idle: 100,
            kernel: 200,
            user: 100,
        };
        let b = CpuTimes {
            idle: 175,
            kernel: 300,
            user: 200,
        };
        // total = 100 + 100, idle = 75 -> 62.5% busy
        assert_eq!(system_cpu_percent(a, b), 62.5);
        assert_eq!(system_cpu_percent(a, a), 0.0);
    }

    #[test]
    fn start_watcher_reports_new_processes_once() {
        let mut w = StartWatcher::new();
        assert!(w.poll().unwrap().is_empty(), "first poll only primes");

        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut found = false;
        while !found && Instant::now() < deadline {
            found = w.poll().unwrap().iter().any(|p| p.pid == pid);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(found, "child {pid} not reported");
        assert!(
            !w.poll().unwrap().iter().any(|p| p.pid == pid),
            "reported twice"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn second_sample_measures_busy_process() {
        let mut s = Sampler::default();
        let first = s.sample().unwrap();
        assert!(first.interval.is_none());

        let until = Instant::now() + Duration::from_millis(300);
        let mut x = 0u64;
        while Instant::now() < until {
            x = std::hint::black_box(x.wrapping_add(1));
        }

        let second = s.sample().unwrap();
        assert!(second.interval.is_some());
        let me = second
            .processes
            .iter()
            .find(|p| p.pid == std::process::id())
            .unwrap();
        assert!(me.cpu_percent > 0.0, "busy loop should register CPU usage");
        assert!(second.system.cpu_percent > 0.0);
        assert!(first.system.core_percent.is_empty());
        let cores = &second.system.core_percent;
        assert!(!cores.is_empty() && cores.iter().any(|&c| c > 0.0));
        assert!(cores.iter().all(|c| (0.0..=100.0).contains(c)));
        assert!(second.system.net_rx_rate >= 0.0 && second.system.io_read_rate >= 0.0);
        assert!(!second.system.disks.is_empty(), "this machine has a disk");
    }
}

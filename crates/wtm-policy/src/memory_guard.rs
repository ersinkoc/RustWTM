//! Memory guard: when the machine runs low on RAM for a while, the biggest
//! background apps have their working sets trimmed so the app in front of the
//! user gets the memory. Nothing is closed and nothing is lost: trimmed pages
//! move to the standby list and come back on demand.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde::Deserialize;
use wtm_core::control::ProcessRef;
use wtm_core::{Snapshot, protection_reason};

use crate::{Action, Decision, Key, Source, glob_match};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryGuardConfig {
    pub enabled: bool,
    /// Physical memory in use (percent) that counts as pressure...
    pub high_percent: f32,
    /// ...for this long.
    pub sustain_secs: u64,
    /// Trim at most this many apps per round, largest first.
    pub max_processes: usize,
    /// Ignore processes with a smaller working set.
    pub min_working_set_mb: u64,
    /// Do not trim the same process again within this time.
    pub cooldown_secs: u64,
    /// Image-name wildcards never trimmed.
    pub exempt: Vec<String>,
}

impl Default for MemoryGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            high_percent: 90.0,
            sustain_secs: 15,
            max_processes: 3,
            min_working_set_mb: 300,
            cooldown_secs: 300,
            exempt: Vec::new(),
        }
    }
}

impl MemoryGuardConfig {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !(1.0..=100.0).contains(&self.high_percent) {
            return Err("high_percent must be within 1..=100".into());
        }
        if self.max_processes == 0 {
            return Err("max_processes must be positive".into());
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct MemoryGuard {
    cfg: MemoryGuardConfig,
    high_since: Option<Instant>,
    trimmed: HashMap<Key, Instant>,
    foreground: HashSet<Key>,
}

impl MemoryGuard {
    pub fn new(cfg: MemoryGuardConfig) -> Self {
        Self {
            cfg,
            ..Self::default()
        }
    }

    pub fn set_config(&mut self, cfg: MemoryGuardConfig) {
        self.cfg = cfg;
    }

    /// The foreground app's processes, never trimmed.
    pub fn set_foreground(&mut self, keys: HashSet<Key>) {
        self.foreground = keys;
    }

    pub fn tick(&mut self, now: Instant, snap: &Snapshot) -> Vec<Decision> {
        let cooldown = Duration::from_secs(self.cfg.cooldown_secs);
        self.trimmed.retain(|_, at| now - *at < cooldown);
        let s = &snap.system;
        if !self.cfg.enabled || s.memory_total == 0 {
            self.high_since = None;
            return Vec::new();
        }
        let used = s.memory_used() as f64 / s.memory_total as f64 * 100.0;
        if used < f64::from(self.cfg.high_percent) {
            self.high_since = None;
            return Vec::new();
        }
        let since = *self.high_since.get_or_insert(now);
        if now - since < Duration::from_secs(self.cfg.sustain_secs) {
            return Vec::new();
        }
        let min = self.cfg.min_working_set_mb.saturating_mul(1 << 20);
        let mut candidates: Vec<_> = snap
            .processes
            .iter()
            .filter(|p| p.working_set >= min)
            .filter(|p| protection_reason(p.pid, &p.name).is_none())
            .filter(|p| !self.foreground.contains(&(p.pid, p.create_time)))
            .filter(|p| !self.trimmed.contains_key(&(p.pid, p.create_time)))
            .filter(|p| !self.cfg.exempt.iter().any(|e| glob_match(e, &p.name)))
            .collect();
        candidates.sort_by_key(|p| std::cmp::Reverse(p.working_set));
        candidates.truncate(self.cfg.max_processes);
        candidates
            .into_iter()
            .map(|p| {
                self.trimmed.insert((p.pid, p.create_time), now);
                Decision::Apply {
                    source: Source::MemoryGuard,
                    target: ProcessRef::from(p),
                    action: Action::TrimMemory,
                    reason: format!(
                        "memory {used:.0}% in use, process holds {} MiB",
                        p.working_set >> 20
                    ),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wtm_core::{ProcessInfo, SystemStats};

    fn proc(pid: u32, name: &str, ws_mb: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: 1,
            name: name.into(),
            cpu_percent: 0.0,
            cpu_time: Duration::ZERO,
            working_set: ws_mb << 20,
            private_working_set: ws_mb << 20,
            private_bytes: ws_mb << 20,
            thread_count: 1,
            handle_count: 1,
            session_id: 1,
            base_priority: 8,
            gpu_percent: 0.0,
            gpu_memory: 0,
            io_read_rate: 0.0,
            io_write_rate: 0.0,
            net_rx_rate: 0.0,
            net_tx_rate: 0.0,
            suspended: false,
            create_time: i64::from(pid),
        }
    }

    fn snap(used_percent: u64) -> Snapshot {
        Snapshot {
            interval: Some(Duration::from_secs(1)),
            system: SystemStats {
                memory_total: 100 << 30,
                memory_available: (100 - used_percent) << 30,
                ..Default::default()
            },
            processes: vec![
                proc(10, "big.exe", 4000),
                proc(11, "bigger.exe", 8000),
                proc(12, "small.exe", 100),
                proc(13, "front.exe", 9000),
                proc(14, "keep.exe", 7000),
                proc(4, "System", 9999),
            ],
            net_traced: false,
            taken_at_ms: 0,
        }
    }

    fn pids(d: &[Decision]) -> Vec<u32> {
        d.iter().map(|d| d.target().pid).collect()
    }

    #[test]
    fn trims_the_biggest_background_apps_under_sustained_pressure() {
        let mut g = MemoryGuard::new(MemoryGuardConfig {
            max_processes: 2,
            exempt: vec!["keep*".into()],
            ..MemoryGuardConfig::default()
        });
        g.set_foreground([(13, 13)].into());
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        assert!(g.tick(at(0), &snap(95)).is_empty(), "not sustained yet");
        let d = g.tick(at(20), &snap(95));
        assert_eq!(pids(&d), [11, 10]);
        assert!(matches!(
            &d[0],
            Decision::Apply {
                action: Action::TrimMemory,
                source: Source::MemoryGuard,
                ..
            }
        ));
        // Cooldown: the same processes are not trimmed again soon.
        assert!(g.tick(at(30), &snap(95)).is_empty());
        assert_eq!(pids(&g.tick(at(400), &snap(95))), [11, 10]);
        // Pressure gone: the timer resets.
        assert!(g.tick(at(401), &snap(50)).is_empty());
        assert!(g.tick(at(402), &snap(95)).is_empty());
    }

    #[test]
    fn disabled_or_unknown_memory_does_nothing() {
        let mut g = MemoryGuard::new(MemoryGuardConfig {
            enabled: false,
            sustain_secs: 0,
            ..MemoryGuardConfig::default()
        });
        assert!(g.tick(Instant::now(), &snap(99)).is_empty());
        g.set_config(MemoryGuardConfig {
            sustain_secs: 0,
            ..MemoryGuardConfig::default()
        });
        let mut empty = snap(99);
        empty.system.memory_total = 0;
        assert!(g.tick(Instant::now(), &empty).is_empty());
        assert_eq!(g.tick(Instant::now(), &snap(99)).len(), 3);

        let bad = MemoryGuardConfig {
            high_percent: 0.0,
            ..MemoryGuardConfig::default()
        };
        assert!(bad.validate().is_err());
        let bad = MemoryGuardConfig {
            max_processes: 0,
            ..MemoryGuardConfig::default()
        };
        assert!(bad.validate().is_err());
        assert!(MemoryGuardConfig::default().validate().is_ok());
    }
}

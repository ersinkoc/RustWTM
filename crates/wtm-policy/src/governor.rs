//! System-wide CPU governor: escalates against top consumers while the
//! machine is overloaded and rolls everything back once it recovers.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use wtm_core::control::ProcessRef;
use wtm_core::{Snapshot, protection_reason};

use crate::config::GovernorConfig;
use crate::{Decision, Key, Source, glob_match};

#[derive(Debug)]
pub struct Governor {
    cfg: GovernorConfig,
    high_since: Option<Instant>,
    low_since: Option<Instant>,
    throttled: HashMap<Key, Throttled>,
    /// The app the user is working in (and its child processes).
    foreground: HashSet<Key>,
}

#[derive(Debug)]
struct Throttled {
    target: ProcessRef,
    next_step: usize,
    /// CPU usage when first throttled; base for relative caps.
    baseline_cpu: f32,
    last_step: Instant,
}

impl Governor {
    pub fn new(cfg: GovernorConfig) -> Self {
        Self {
            cfg,
            high_since: None,
            low_since: None,
            throttled: HashMap::new(),
            foreground: HashSet::new(),
        }
    }

    /// The processes of the app in the foreground; with `protect_foreground`
    /// they are never throttled and lose any throttling right away.
    pub fn set_foreground(&mut self, keys: HashSet<Key>) {
        self.foreground = keys;
    }

    /// Replaces the configuration, keeping track of already throttled processes.
    pub fn set_config(&mut self, cfg: GovernorConfig) {
        self.cfg = cfg;
    }

    pub fn throttled_count(&self) -> usize {
        self.throttled.len()
    }

    pub fn tick(&mut self, now: Instant, snap: &Snapshot) -> Vec<Decision> {
        // Exited processes need no rollback.
        self.throttled.retain(|&(pid, created), _| {
            snap.processes
                .iter()
                .any(|p| p.pid == pid && p.create_time == created)
        });
        let mut out = Vec::new();
        if self.cfg.protect_foreground {
            let in_use: Vec<Key> = self
                .throttled
                .keys()
                .filter(|k| self.foreground.contains(k))
                .copied()
                .collect();
            for key in in_use {
                let t = self.throttled.remove(&key).expect("listed above");
                out.push(Decision::Restore {
                    source: Source::Governor,
                    target: t.target,
                    reason: "in use (foreground app)".into(),
                });
            }
        }
        out.extend(self.tick_load(now, snap));
        out
    }

    fn tick_load(&mut self, now: Instant, snap: &Snapshot) -> Vec<Decision> {
        if !self.cfg.enabled {
            return self.restore_all("governor disabled");
        }

        let cpu = snap.system.cpu_percent;
        if cpu >= self.cfg.high_cpu_percent {
            self.low_since = None;
            let since = *self.high_since.get_or_insert(now);
            if now - since >= secs(self.cfg.sustain_secs) {
                return self.escalate(now, snap, cpu);
            }
        } else if cpu <= self.cfg.recover_cpu_percent {
            self.high_since = None;
            let since = *self.low_since.get_or_insert(now);
            if now - since >= secs(self.cfg.recover_secs) {
                self.low_since = None;
                return self.restore_all(&format!("system CPU recovered to {cpu:.0}%"));
            }
        } else {
            // Between the thresholds: neither overload nor recovery is progressing.
            self.high_since = None;
            self.low_since = None;
        }
        Vec::new()
    }

    fn escalate(&mut self, now: Instant, snap: &Snapshot, system_cpu: f32) -> Vec<Decision> {
        let mut offenders: Vec<_> = snap
            .processes
            .iter()
            .filter(|p| p.cpu_percent >= self.cfg.offender_min_cpu_percent)
            .filter(|p| protection_reason(p.pid, &p.name).is_none())
            .filter(|p| !self.cfg.exempt.iter().any(|e| glob_match(e, &p.name)))
            .filter(|p| {
                !(self.cfg.protect_foreground && self.foreground.contains(&(p.pid, p.create_time)))
            })
            .collect();
        offenders.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        offenders.truncate(self.cfg.max_offenders);

        let cooldown = secs(self.cfg.step_cooldown_secs);
        let mut out = Vec::new();
        for p in offenders {
            let entry = self
                .throttled
                .entry((p.pid, p.create_time))
                .or_insert_with(|| Throttled {
                    target: p.into(),
                    next_step: 0,
                    baseline_cpu: p.cpu_percent,
                    last_step: now.checked_sub(cooldown).unwrap_or(now),
                });
            if entry.next_step >= self.cfg.ladder.len() || now - entry.last_step < cooldown {
                continue;
            }
            let action = self.cfg.ladder[entry.next_step].resolve(entry.baseline_cpu);
            out.push(Decision::Apply {
                source: Source::Governor,
                target: entry.target.clone(),
                action,
                reason: format!(
                    "system CPU {system_cpu:.0}%, process at {:.1}% (step {}/{})",
                    p.cpu_percent,
                    entry.next_step + 1,
                    self.cfg.ladder.len()
                ),
            });
            entry.next_step += 1;
            entry.last_step = now;
        }
        out
    }

    fn restore_all(&mut self, reason: &str) -> Vec<Decision> {
        self.throttled
            .drain()
            .map(|(_, t)| Decision::Restore {
                source: Source::Governor,
                target: t.target,
                reason: reason.to_owned(),
            })
            .collect()
    }
}

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, PriorityLevel};
    use wtm_core::{ProcessInfo, SystemStats};

    fn proc(pid: u32, name: &str, cpu: f32) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: 1,
            name: name.into(),
            cpu_percent: cpu,
            cpu_time: Duration::ZERO,
            working_set: 0,
            private_working_set: 0,
            private_bytes: 0,
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
            create_time: i64::from(pid) * 10,
        }
    }

    fn snap(system_cpu: f32, procs: Vec<ProcessInfo>) -> Snapshot {
        Snapshot {
            interval: Some(Duration::from_secs(1)),
            system: SystemStats {
                cpu_percent: system_cpu,
                ..Default::default()
            },
            processes: procs,
        }
    }

    fn cfg() -> GovernorConfig {
        GovernorConfig {
            sustain_secs: 3,
            recover_secs: 5,
            step_cooldown_secs: 2,
            max_offenders: 2,
            ladder: vec![
                Action::Priority(PriorityLevel::BelowNormal),
                Action::CpuCapRelative(0.5),
            ],
            exempt: vec!["exempt*.exe".into()],
            ..GovernorConfig::default()
        }
    }

    fn applied(ds: &[Decision]) -> Vec<(u32, Action)> {
        ds.iter()
            .filter_map(|d| match d {
                Decision::Apply { target, action, .. } => Some((target.pid, *action)),
                Decision::Restore { .. } => None,
            })
            .collect()
    }

    #[test]
    fn escalates_after_sustain_then_restores_after_recovery() {
        let mut g = Governor::new(cfg());
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let busy = || {
            vec![
                proc(10, "hog.exe", 40.0),
                proc(11, "medium.exe", 20.0),
                proc(12, "small.exe", 10.0),
                proc(13, "exempt-me.exe", 50.0),
                proc(4, "System", 60.0),
                proc(14, "idle.exe", 0.5),
            ]
        };

        // Overload must persist for sustain_secs.
        assert!(g.tick(at(0), &snap(95.0, busy())).is_empty());
        assert!(g.tick(at(2), &snap(95.0, busy())).is_empty());

        // Top two eligible offenders get step 1; exempt/protected/idle skipped.
        let d = g.tick(at(3), &snap(95.0, busy()));
        let below = Action::Priority(PriorityLevel::BelowNormal);
        assert_eq!(applied(&d), [(10, below), (11, below)]);

        // Cooldown blocks the next step.
        assert!(g.tick(at(4), &snap(95.0, busy())).is_empty());

        // Step 2: relative cap computed from the baseline.
        let d = g.tick(at(5), &snap(95.0, busy()));
        assert_eq!(
            applied(&d),
            [(10, Action::CpuCap(20.0)), (11, Action::CpuCap(10.0))]
        );

        // Ladder exhausted: nothing more.
        assert!(g.tick(at(7), &snap(95.0, busy())).is_empty());
        assert_eq!(g.throttled_count(), 2);

        // Recovery must persist for recover_secs, then everything is restored.
        assert!(g.tick(at(8), &snap(50.0, busy())).is_empty());
        let d = g.tick(at(13), &snap(50.0, busy()));
        assert_eq!(d.len(), 2);
        assert!(applied(&d).is_empty(), "recovery only restores");
        assert_eq!(g.throttled_count(), 0);
    }

    #[test]
    fn middle_zone_resets_timers() {
        let mut g = Governor::new(cfg());
        let t0 = Instant::now();
        let procs = || vec![proc(10, "hog.exe", 40.0)];
        g.tick(t0, &snap(95.0, procs()));
        g.tick(t0 + Duration::from_secs(2), &snap(80.0, procs()));
        // Overload restarted, so 3 more seconds are needed.
        assert!(
            g.tick(t0 + Duration::from_secs(3), &snap(95.0, procs()))
                .is_empty()
        );
        assert!(
            !g.tick(t0 + Duration::from_secs(6), &snap(95.0, procs()))
                .is_empty()
        );
    }

    #[test]
    fn forgets_exited_processes() {
        let mut g = Governor::new(GovernorConfig {
            sustain_secs: 0,
            ..cfg()
        });
        let t0 = Instant::now();
        g.tick(t0, &snap(95.0, vec![proc(10, "hog.exe", 40.0)]));
        assert_eq!(g.throttled_count(), 1);
        g.tick(t0, &snap(95.0, vec![]));
        assert_eq!(g.throttled_count(), 0);
    }

    #[test]
    fn disabling_restores_everything() {
        let mut g = Governor::new(GovernorConfig {
            sustain_secs: 0,
            ..cfg()
        });
        let t0 = Instant::now();
        let procs = || vec![proc(10, "hog.exe", 40.0)];
        g.tick(t0, &snap(95.0, procs()));
        g.set_config(GovernorConfig {
            enabled: false,
            ..cfg()
        });
        let d = g.tick(t0, &snap(95.0, procs()));
        assert!(matches!(d.as_slice(), [Decision::Restore { .. }]));
    }

    #[test]
    fn never_throttles_the_foreground_app_and_releases_it_at_once() {
        let mut g = Governor::new(GovernorConfig {
            sustain_secs: 0,
            ..cfg()
        });
        let now = Instant::now();
        let procs = || vec![proc(10, "game.exe", 50.0), proc(11, "miner.exe", 40.0)];
        g.set_foreground([(10, 100)].into());
        let d = g.tick(now, &snap(99.0, procs()));
        assert_eq!(applied(&d).iter().map(|a| a.0).collect::<Vec<_>>(), [11]);

        // The user switches to the miner window: it is released immediately.
        g.set_foreground([(11, 110)].into());
        let d = g.tick(now, &snap(99.0, procs()));
        assert!(matches!(
            &d[0],
            Decision::Restore { target, reason, .. }
                if target.pid == 11 && reason == "in use (foreground app)"
        ));
        assert_eq!(applied(&d).iter().map(|a| a.0).collect::<Vec<_>>(), [10]);

        // Without protection the foreground app is treated like any other.
        let mut g = Governor::new(GovernorConfig {
            sustain_secs: 0,
            protect_foreground: false,
            ..cfg()
        });
        g.set_foreground([(10, 100)].into());
        assert_eq!(applied(&g.tick(now, &snap(99.0, procs()))).len(), 2);
    }
}

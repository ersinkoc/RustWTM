//! Applies the rescue core: while the machine is overloaded, every ordinary
//! process is confined to the other cores with CPU Sets, so RustWTM and
//! Windows always have a core to run on. Released when the load recovers.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use wtm_core::Snapshot;
use wtm_core::control::{self, ProcessRef};
use wtm_core::cpu::RescueSplit;
use wtm_ipc::RescueStatus;
use wtm_policy::{RescueCore, RescueStep, ReserveConfig};

use crate::events::EventLog;

/// Processes confined per tick, so a huge process count cannot stall the
/// engine; the rest follow on the next ticks.
const PER_TICK: usize = 500;

type Key = (u32, i64);

pub struct Rescue {
    policy: RescueCore,
    split: Option<RescueSplit>,
    dry_run: bool,
    events: EventLog,
    confined: HashMap<Key, ProcessRef>,
    /// Refused, or already confined by their own choice: left alone.
    skipped: HashSet<Key>,
}

impl Rescue {
    pub fn new(cfg: ReserveConfig, dry_run: bool, events: EventLog) -> Self {
        let split = wtm_core::cpu::topology()
            .ok()
            .and_then(|t| t.rescue_split());
        if split.is_none() && cfg.rescue_core {
            events.warn(None, "rescue core unavailable on this processor layout");
        }
        Self {
            policy: RescueCore::new(cfg),
            split,
            dry_run,
            events,
            confined: HashMap::new(),
            skipped: HashSet::new(),
        }
    }

    pub fn configure(&mut self, cfg: ReserveConfig, dry_run: bool) {
        if dry_run != self.dry_run && self.policy.engaged() {
            // Hand the core back under the old mode, engage afresh under the new.
            self.policy.set_config(ReserveConfig {
                rescue_core: false,
                ..cfg.clone()
            });
            self.policy.tick(Instant::now(), 0.0);
            self.release_all("configuration changed");
        }
        self.dry_run = dry_run;
        self.policy.set_config(cfg);
    }

    /// What clients show while the rescue core is engaged.
    pub fn summary(&self) -> Option<RescueStatus> {
        let split = self.split.as_ref().filter(|_| self.policy.engaged())?;
        Some(RescueStatus {
            cpus: split.reserved_indices.clone(),
            moved: self.confined.len(),
        })
    }

    /// `live` holds the processes running in `snap`.
    pub fn tick(&mut self, now: Instant, snap: &Snapshot, live: &HashSet<Key>) {
        let Some(split) = &self.split else {
            return;
        };
        match self.policy.tick(now, snap.system.cpu_percent) {
            Some(RescueStep::Engage) => self.events.alert(
                None,
                format!(
                    "CPU overloaded: {} {} free for RustWTM and Windows",
                    cpu_names(split),
                    if self.dry_run {
                        "would be kept (dry run)"
                    } else {
                        "kept"
                    }
                ),
            ),
            Some(RescueStep::Release) => self.release_all("load recovered"),
            None => {}
        }
        self.confined.retain(|k, _| live.contains(k));
        self.skipped.retain(|k| live.contains(k));
        if !self.policy.engaged() || self.dry_run {
            return;
        }
        let Some(split) = &self.split else {
            return;
        };
        let cfg = self.policy.config();
        let fresh: Vec<_> = snap
            .processes
            .iter()
            .filter(|p| {
                let key = (p.pid, p.create_time);
                !self.confined.contains_key(&key)
                    && !self.skipped.contains(&key)
                    && cfg.confines(p.pid, &p.name)
            })
            .take(PER_TICK)
            .map(ProcessRef::from)
            .collect();
        for target in fresh {
            let key = (target.pid, target.create_time);
            match control::confine_cpu_sets(&target, &split.others) {
                Ok(true) => {
                    self.confined.insert(key, target);
                }
                Ok(false) | Err(_) => {
                    self.skipped.insert(key);
                }
            }
        }
    }

    /// Hands the core back to every process moved off it.
    pub fn release_all(&mut self, reason: &str) {
        self.skipped.clear();
        let Some(split) = &self.split else {
            return;
        };
        if self.confined.is_empty() {
            return;
        }
        let count = self.confined.len();
        for (_, target) in self.confined.drain() {
            let _ = control::release_cpu_sets(&target, &split.others);
        }
        self.events.info(
            None,
            format!("{reason}: {} handed back to {count} apps", cpu_names(split)),
        );
    }
}

fn cpu_names(split: &RescueSplit) -> String {
    let list: Vec<_> = split.reserved_indices.iter().map(u8::to_string).collect();
    format!(
        "{} {}",
        if list.len() == 1 { "CPU" } else { "CPUs" },
        list.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tick(r: &mut Rescue, now: Instant, snap: &Snapshot) {
        let live = snap
            .processes
            .iter()
            .map(|p| (p.pid, p.create_time))
            .collect();
        r.tick(now, snap, &live);
    }

    fn snapshot(cpu: f32, procs: Vec<wtm_core::ProcessInfo>) -> Snapshot {
        let mut s = wtm_core::Sampler::new().sample().unwrap();
        s.system.cpu_percent = cpu;
        s.processes = procs;
        s
    }

    #[test]
    fn engages_confines_new_processes_and_releases() {
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let live = wtm_core::Sampler::new().sample().unwrap();
        let ping = live
            .processes
            .iter()
            .find(|p| p.pid == pid)
            .unwrap()
            .clone();
        let events = EventLog::default();
        let cfg = ReserveConfig {
            sustain_secs: 0,
            release_secs: 0,
            ..ReserveConfig::default()
        };
        let mut r = Rescue::new(cfg.clone(), false, events.clone());
        if r.split.is_none() {
            let _ = child.kill();
            let _ = child.wait();
            return; // single-processor runner
        }
        let others = r.split.clone().unwrap().others;
        let sets = |pid| {
            let h = wtm_sys::control::ProcessHandle::open(pid, wtm_sys::control::Access::QUERY)
                .unwrap();
            h.default_cpu_sets().unwrap()
        };
        let t = Instant::now();
        tick(&mut r, t, &snapshot(99.0, vec![ping.clone()]));
        assert_eq!(sets(pid).len(), others.len());
        assert_eq!(r.summary().unwrap().moved, 1);

        tick(
            &mut r,
            t + Duration::from_secs(1),
            &snapshot(10.0, vec![ping.clone()]),
        );
        assert!(sets(pid).is_empty());
        assert!(r.summary().is_none());

        // Dry run only reports.
        r.configure(cfg, true);
        tick(
            &mut r,
            t + Duration::from_secs(2),
            &snapshot(99.0, vec![ping]),
        );
        assert!(sets(pid).is_empty());
        assert_eq!(r.summary().unwrap().moved, 0);
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn names_reserved_processors() {
        let one = RescueSplit {
            reserved: vec![7],
            others: vec![],
            reserved_indices: vec![7],
        };
        assert_eq!(cpu_names(&one), "CPU 7");
        let two = RescueSplit {
            reserved_indices: vec![14, 15],
            ..one
        };
        assert_eq!(cpu_names(&two), "CPUs 14, 15");
    }
}

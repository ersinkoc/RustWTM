//! Applies decisions and keeps every source's actions separate.
//!
//! For each process we store the actions requested per [`Source`] plus the
//! process's original priority / EcoQoS. The state actually applied is the
//! fold of all sources (lowest priority, tightest caps, any eco/suspend), so
//! when one source rolls back, the others' limits stay in force.

use std::collections::{HashMap, HashSet};
use std::mem::discriminant;

use wtm_core::Snapshot;
use wtm_core::control::{self, EcoQos, Limiter, Limits, PriorityClass, ProcessRef};
use wtm_ipc::Managed;
use wtm_policy::{Action, Decision, PriorityLevel, Source};

use crate::events::EventLog;

type Key = (u32, i64);

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Effective {
    priority: Option<PriorityLevel>,
    eco_qos: bool,
    cpu_cap: Option<f32>,
    memory_bytes: Option<u64>,
    suspended: bool,
}

impl Effective {
    fn fold<'a>(actions: impl IntoIterator<Item = &'a Action>) -> Self {
        let mut e = Self::default();
        for a in actions {
            match *a {
                Action::Priority(p) => e.priority = Some(e.priority.map_or(p, |q| q.min(p))),
                Action::EcoQos => e.eco_qos = true,
                Action::CpuCap(c) => e.cpu_cap = Some(e.cpu_cap.map_or(c, |d| d.min(c))),
                Action::MemoryCapMb(mb) => {
                    let b = mb.saturating_mul(1 << 20);
                    e.memory_bytes = Some(e.memory_bytes.map_or(b, |d| d.min(b)));
                }
                Action::Suspend => e.suspended = true,
                Action::Kill | Action::CpuCapRelative(_) => {}
            }
        }
        e
    }

    fn limits(&self) -> Limits {
        Limits {
            cpu_percent: self.cpu_cap,
            memory_bytes: self.memory_bytes,
        }
    }

    fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(p) = self.priority {
            parts.push(format!("{p:?}"));
        }
        if self.eco_qos {
            parts.push("efficiency mode".into());
        }
        if let Some(c) = self.cpu_cap {
            parts.push(format!("CPU ≤ {c:.2}%"));
        }
        if let Some(m) = self.memory_bytes {
            parts.push(format!("memory ≤ {} MiB", m >> 20));
        }
        if self.suspended {
            parts.push("suspended".into());
        }
        if parts.is_empty() {
            "none".into()
        } else {
            parts.join(", ")
        }
    }
}

#[derive(Debug)]
struct Touched {
    target: ProcessRef,
    original_priority: Option<PriorityClass>,
    original_eco: Option<EcoQos>,
    actions: HashMap<Source, Vec<Action>>,
    applied: Effective,
}

#[derive(Debug)]
pub struct Executor {
    dry_run: bool,
    limiter: Limiter,
    touched: HashMap<Key, Touched>,
    events: EventLog,
}

fn key(t: &ProcessRef) -> Key {
    (t.pid, t.create_time)
}

impl Executor {
    pub fn new(dry_run: bool, events: EventLog) -> Self {
        Self {
            dry_run,
            limiter: Limiter::new(),
            touched: HashMap::new(),
            events,
        }
    }

    /// Switching to dry-run rolls back automatic (governor/rule) actions;
    /// manual limits are kept.
    pub fn set_dry_run(&mut self, dry_run: bool) {
        if dry_run && !self.dry_run {
            self.restore_sources(
                &[Source::Governor, Source::Rule, Source::Insight],
                "dry-run enabled",
            );
        }
        self.dry_run = dry_run;
    }

    pub fn managed(&self) -> Vec<Managed> {
        let mut out: Vec<Managed> = self
            .touched
            .values()
            .map(|t| {
                let mut sources: Vec<Source> = t.actions.keys().copied().collect();
                sources.sort_by_key(|s| *s as u8);
                Managed {
                    target: t.target.clone(),
                    sources,
                    summary: t.applied.summary(),
                }
            })
            .collect();
        out.sort_by_key(|m| m.target.pid);
        out
    }

    pub fn forget_exited(&mut self, snap: &Snapshot) {
        let live: HashSet<Key> = snap
            .processes
            .iter()
            .map(|p| (p.pid, p.create_time))
            .collect();
        let Self {
            limiter, touched, ..
        } = self;
        touched.retain(|k, t| {
            let alive = live.contains(k);
            if !alive {
                limiter.forget(&t.target);
            }
            alive
        });
    }

    pub fn execute(&mut self, decision: &Decision) -> Result<(), String> {
        match decision {
            Decision::Apply {
                source,
                target,
                action,
                reason,
            } => {
                if self.dry_run && *source != Source::Manual {
                    self.events.info(
                        Some(target),
                        format!("dry-run: would apply {action} ({reason})"),
                    );
                    return Ok(());
                }
                self.apply(*source, target, *action, reason)
            }
            Decision::Restore {
                source,
                target,
                reason,
            } => {
                if self.dry_run && *source != Source::Manual {
                    self.events
                        .info(Some(target), format!("dry-run: would restore ({reason})"));
                    return Ok(());
                }
                self.restore(*source, target, reason)
            }
        }
    }

    fn apply(
        &mut self,
        source: Source,
        t: &ProcessRef,
        action: Action,
        reason: &str,
    ) -> Result<(), String> {
        // Notify on terminations and when the governor first steps in on a process.
        let mut notable = action == Action::Kill;
        let result = if action == Action::Kill {
            let r = control::kill(t).map_err(|e| e.to_string());
            if let Some(entry) = self.touched.remove(&key(t)) {
                self.limiter.forget(&entry.target);
            }
            r
        } else {
            let entry = self.touched.entry(key(t)).or_insert_with(|| {
                let details = control::inspect(t).ok();
                Touched {
                    target: t.clone(),
                    original_priority: details.as_ref().and_then(|d| d.priority),
                    original_eco: details.as_ref().and_then(|d| d.eco_qos),
                    actions: HashMap::new(),
                    applied: Effective::default(),
                }
            });
            notable |= source == Source::Governor && !entry.actions.contains_key(&source);
            // A newer action of the same kind from the same source replaces the old one.
            let list = entry.actions.entry(source).or_default();
            list.retain(|a| discriminant(a) != discriminant(&action));
            list.push(action);
            reconcile(&mut self.limiter, entry)
        };

        match &result {
            Ok(()) if notable => self
                .events
                .notable(Some(t), format!("{source:?}: {action} ({reason})")),
            Ok(()) => self
                .events
                .info(Some(t), format!("{source:?}: {action} ({reason})")),
            Err(e) => self.events.warn(
                Some(t),
                format!("{source:?}: {action} failed: {e} ({reason})"),
            ),
        }
        result
    }

    fn restore(&mut self, source: Source, t: &ProcessRef, reason: &str) -> Result<(), String> {
        let k = key(t);
        let Some(entry) = self.touched.get_mut(&k) else {
            return Ok(());
        };
        if entry.actions.remove(&source).is_none() {
            return Ok(());
        }
        let result = reconcile(&mut self.limiter, entry);
        if entry.actions.is_empty() {
            self.touched.remove(&k);
        }
        match &result {
            Ok(()) => self
                .events
                .info(Some(t), format!("{source:?}: restored ({reason})")),
            Err(e) => self.events.warn(
                Some(t),
                format!("{source:?}: restore incomplete: {e} ({reason})"),
            ),
        }
        result
    }

    fn restore_sources(&mut self, sources: &[Source], reason: &str) {
        let pending: Vec<(Source, ProcessRef)> = self
            .touched
            .values()
            .flat_map(|t| {
                sources
                    .iter()
                    .filter(|s| t.actions.contains_key(s))
                    .map(|s| (*s, t.target.clone()))
            })
            .collect();
        for (source, target) in pending {
            let _ = self.restore(source, &target, reason);
        }
    }

    /// Rolls back everything, from every source.
    pub fn restore_all(&mut self, reason: &str) {
        self.restore_sources(
            &[
                Source::Governor,
                Source::Rule,
                Source::Insight,
                Source::Manual,
            ],
            reason,
        );
    }
}

/// Brings the process from `applied` to the fold of all requested actions.
fn reconcile(limiter: &mut Limiter, t: &mut Touched) -> Result<(), String> {
    let new = Effective::fold(t.actions.values().flatten());
    let old = t.applied;
    let target = &t.target;
    let mut errors = Vec::new();

    if new.priority != old.priority {
        let priority = new
            .priority
            .map(PriorityClass::from)
            .or(t.original_priority);
        if let Some(p) = priority {
            errors.extend(control::set_priority(target, p).err());
        }
    }
    if new.eco_qos != old.eco_qos {
        let mode = if new.eco_qos {
            EcoQos::On
        } else {
            t.original_eco.unwrap_or(EcoQos::SystemManaged)
        };
        errors.extend(control::set_eco_qos(target, mode).err());
    }
    if new.limits() != old.limits() {
        let r = if new.limits() == Limits::default() {
            limiter.lift(target)
        } else {
            limiter.apply(target, new.limits())
        };
        errors.extend(r.err());
    }
    if new.suspended != old.suspended {
        let r = if new.suspended {
            control::suspend(target)
        } else {
            control::resume(target)
        };
        errors.extend(r.err());
    }

    t.applied = new;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_takes_the_most_restrictive_state() {
        let governor = [
            Action::Priority(PriorityLevel::BelowNormal),
            Action::CpuCap(20.0),
        ];
        let rule = [
            Action::Priority(PriorityLevel::Idle),
            Action::CpuCap(50.0),
            Action::EcoQos,
            Action::MemoryCapMb(512),
        ];
        let e = Effective::fold(governor.iter().chain(&rule));
        assert_eq!(e.priority, Some(PriorityLevel::Idle));
        assert_eq!(e.cpu_cap, Some(20.0));
        assert_eq!(e.memory_bytes, Some(512 << 20));
        assert!(e.eco_qos && !e.suspended);
        assert_eq!(
            e.summary(),
            "Idle, efficiency mode, CPU ≤ 20.00%, memory ≤ 512 MiB"
        );

        // Governor rolls back: the rule's limits remain.
        let e = Effective::fold(&rule);
        assert_eq!(e.priority, Some(PriorityLevel::Idle));
        assert_eq!(e.cpu_cap, Some(50.0));
        assert_eq!(Effective::fold(&[]).summary(), "none");
    }
}

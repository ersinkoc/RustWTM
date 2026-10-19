//! Applies per-application budgets ([`wtm_policy::AppLimit`]) with one job
//! object per budget. Processes started later by a member join the job
//! automatically (Windows puts children into their parent's job).

use std::collections::{HashMap, HashSet};

use wtm_core::ProcessInfo;
use wtm_core::control::ProcessRef;
use wtm_policy::{AppLimit, GroupId, GroupUsage, LimitPlanner, LimitScope};
use wtm_sys::control::{Access, ProcessHandle};
use wtm_sys::job::Job;

use crate::events::EventLog;

/// A job and the budgets currently set on it.
struct Budget {
    job: Job,
    cpu: Option<f32>,
    memory: Option<u64>,
}

impl Budget {
    fn apply(&mut self, cpu: Option<f32>, memory: Option<u64>) -> wtm_sys::Result<()> {
        // Windows rejects lifting a cap that was never set.
        if cpu != self.cpu && (cpu.is_some() || self.cpu.is_some()) {
            self.job.set_cpu_cap(cpu)?;
            self.cpu = cpu;
        }
        if memory != self.memory && (memory.is_some() || self.memory.is_some()) {
            self.job.set_job_memory_limit(memory)?;
            self.memory = memory;
        }
        Ok(())
    }
}

pub fn describe(limit: &AppLimit) -> String {
    let mut parts = Vec::new();
    if let Some(c) = limit.cpu_cores {
        parts.push(format!("{c} cores"));
    }
    if let Some(m) = limit.memory_gb {
        parts.push(format!("{m} GB"));
    }
    let scope = match limit.scope {
        LimitScope::Group => "all processes together",
        LimitScope::Each => "each instance",
    };
    format!("{} ({scope})", parts.join(", "))
}

pub struct AppLimiter {
    planner: LimitPlanner,
    budgets: HashMap<GroupId, Budget>,
    dry_run: bool,
    /// Groups already reported in dry-run.
    previewed: HashSet<GroupId>,
    /// Members that run in a job of their own (sandboxes, build tools) and
    /// cannot join the budget.
    outside: HashSet<(u32, i64)>,
    /// Apps whose outside members were already reported.
    reported_outside: HashSet<String>,
    events: EventLog,
}

impl AppLimiter {
    pub fn new(limits: Vec<AppLimit>, dry_run: bool, events: EventLog) -> Self {
        Self {
            planner: LimitPlanner::new(limits),
            budgets: HashMap::new(),
            dry_run,
            previewed: HashSet::new(),
            outside: HashSet::new(),
            reported_outside: HashSet::new(),
            events,
        }
    }

    /// Changes the limits or the mode; budgets of removed apps (or all, when
    /// switching to dry-run) are lifted.
    pub fn configure(&mut self, limits: Vec<AppLimit>, dry_run: bool) {
        if dry_run && !self.dry_run {
            self.planner.set_limits(Vec::new());
            self.lift_all("dry-run enabled");
        }
        self.dry_run = dry_run;
        for group in self.planner.set_limits(limits) {
            if let Some(b) = self.budgets.remove(&group) {
                self.lift(&group, b, "limit removed");
            }
        }
        self.previewed.clear();
    }

    /// Assigns new processes and keeps budgets in line with the limits.
    /// Returns the usage of every budget.
    pub fn tick(&mut self, procs: &[ProcessInfo], logical_cpus: u32) -> Vec<GroupUsage> {
        if self.dry_run {
            return self.preview(procs);
        }
        for (group, p) in self.planner.plan(procs) {
            let Some(limit) = self.planner.limit_of(&group).cloned() else {
                continue;
            };
            if !self.budgets.contains_key(&group) {
                match Job::create() {
                    Ok(job) => {
                        let target = ProcessRef::from(&p);
                        self.events.info(
                            Some(&target),
                            format!("limiting {}: {}", limit.name, describe(&limit)),
                        );
                        self.budgets.insert(
                            group.clone(),
                            Budget {
                                job,
                                cpu: None,
                                memory: None,
                            },
                        );
                    }
                    Err(e) => {
                        self.events
                            .warn(None, format!("cannot create a job for {}: {e}", limit.name));
                        continue;
                    }
                }
            }
            let budget = self.budgets.get_mut(&group).expect("inserted above");
            match add(&budget.job, &p) {
                Ok(()) => {}
                // Already in a job of its own that cannot nest in ours.
                Err(e) if e.is_access_denied() => {
                    self.outside.insert((p.pid, p.create_time));
                    if self.reported_outside.insert(group.app.clone()) {
                        self.events.info(
                            Some(&ProcessRef::from(&p)),
                            format!(
                                "some {} processes run in a job of their own (a sandbox or build \
                                 tool) or are protected, and stay outside its budget; new ones \
                                 join it",
                                limit.name
                            ),
                        );
                    }
                }
                // The process exited in the meantime.
                Err(e) if e.is_invalid_parameter() => {}
                Err(e) => self.events.warn(
                    Some(&ProcessRef::from(&p)),
                    format!("cannot apply the {} limit: {e}", limit.name),
                ),
            }
        }

        // Budgets follow edits; groups without live members are dropped.
        let live_keys: HashSet<(u32, i64)> = procs.iter().map(|p| (p.pid, p.create_time)).collect();
        self.outside.retain(|k| live_keys.contains(k));
        let mut usage = self.planner.usage(procs);
        for u in &mut usage {
            u.outside = u
                .members
                .iter()
                .filter(|m| self.outside.contains(m))
                .count();
        }
        let live: HashSet<&GroupId> = usage.iter().map(|u| &u.group).collect();
        self.budgets.retain(|g, _| live.contains(g));
        for u in &usage {
            if let Some(b) = self.budgets.get_mut(&u.group)
                && let Err(e) = b.apply(u.limit.cpu_percent(logical_cpus), u.limit.memory_bytes())
            {
                self.events
                    .warn(None, format!("cannot set the {} limit: {e}", u.limit.name));
            }
        }
        usage
    }

    /// Reports what would be limited without touching anything.
    fn preview(&mut self, procs: &[ProcessInfo]) -> Vec<GroupUsage> {
        let mut planner = LimitPlanner::new(self.planner.limits().to_vec());
        planner.plan(procs);
        let usage = planner.usage(procs);
        for u in &usage {
            if self.previewed.insert(u.group.clone()) {
                self.events.info(
                    None,
                    format!(
                        "dry-run: would limit {} ({} processes): {}",
                        u.limit.name,
                        u.processes,
                        describe(&u.limit)
                    ),
                );
            }
        }
        usage
    }

    fn lift(&self, group: &GroupId, mut budget: Budget, why: &str) {
        match budget.apply(None, None) {
            Ok(()) => self
                .events
                .info(None, format!("lifted the {} limit ({why})", group.app)),
            Err(e) => self
                .events
                .warn(None, format!("cannot lift the {} limit: {e}", group.app)),
        }
    }

    pub fn lift_all(&mut self, why: &str) {
        for (group, budget) in std::mem::take(&mut self.budgets) {
            self.lift(&group, budget, why);
        }
    }
}

/// Puts `p` into `job` unless it is already there (children inherit it).
fn add(job: &Job, p: &ProcessInfo) -> wtm_sys::Result<()> {
    let h = ProcessHandle::open(p.pid, Access::QUERY | Access::SET_QUOTA | Access::TERMINATE)?;
    // Same PID, different process: it exited and the PID was reused.
    if h.create_time()? != p.create_time || job.contains(&h)? {
        return Ok(());
    }
    job.assign(&h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;
    use wtm_core::Sampler;

    struct Guard(Child);

    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// A copy of cmd.exe with a unique name that starts a ping child.
    fn spawn_app(dir: &std::path::Path, name: &str) -> Guard {
        let exe = dir.join(name);
        if !exe.exists() {
            std::fs::copy(r"C:\Windows\System32\cmd.exe", &exe).unwrap();
        }
        Guard(
            Command::new(exe)
                .args(["/c", "ping -n 30 127.0.0.1 >nul"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    fn limit(name: &str, scope: LimitScope) -> AppLimit {
        AppLimit {
            name: name.into(),
            cpu_cores: Some(1.0),
            memory_gb: Some(1.0),
            scope,
        }
    }

    fn sample_with(pids: &[u32]) -> Vec<ProcessInfo> {
        let mut sampler = Sampler::new();
        crate::test_util::wait_for("children in the snapshot", || {
            let snap = sampler.sample().unwrap();
            let all = pids
                .iter()
                .all(|pid| snap.processes.iter().any(|p| p.parent_pid == *pid));
            all.then_some(snap.processes)
        })
    }

    fn messages(events: &EventLog) -> Vec<String> {
        events.after(0).into_iter().map(|e| e.message).collect()
    }

    #[test]
    fn limits_apps_with_their_children_and_lifts_on_change() {
        let dir = std::env::temp_dir().join(format!("wtm-applimit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("wtm-limited-{}.exe", std::process::id());
        let (a, b) = (spawn_app(&dir, &name), spawn_app(&dir, &name));
        let procs = sample_with(&[a.0.id(), b.0.id()]);

        let events = EventLog::default();
        let mut limiter =
            AppLimiter::new(vec![limit(&name, LimitScope::Each)], false, events.clone());
        let usage = limiter.tick(&procs, 8);
        assert_eq!(usage.len(), 2, "one budget per instance: {usage:?}");
        assert!(usage.iter().all(|u| u.processes >= 2), "{usage:?}");
        assert_eq!(limiter.budgets.len(), 2);
        let budget = limiter.budgets.values().next().unwrap();
        assert_eq!(budget.cpu, Some(12.5));
        assert_eq!(budget.memory, Some(1 << 30));
        // The children were put into (or inherited) the parent's job.
        let child = procs.iter().find(|p| p.parent_pid == a.0.id()).unwrap();
        let h = ProcessHandle::open(child.pid, Access::QUERY).unwrap();
        assert!(
            limiter
                .budgets
                .values()
                .any(|b| b.job.contains(&h).unwrap())
        );
        assert!(
            messages(&events)
                .iter()
                .any(|m| m.starts_with(&format!("limiting {name}")))
        );

        // Same app, new budget: changed in place.
        let mut cheaper = limit(&name, LimitScope::Each);
        cheaper.memory_gb = None;
        limiter.configure(vec![cheaper], false);
        limiter.tick(&procs, 8);
        assert!(
            limiter
                .budgets
                .values()
                .all(|b| b.memory.is_none() && b.cpu == Some(12.5))
        );

        // Removed: lifted.
        limiter.configure(Vec::new(), false);
        assert!(limiter.budgets.is_empty());
        assert!(
            messages(&events)
                .iter()
                .any(|m| m.contains("limit removed"))
        );

        // Dry-run only reports, once per budget.
        limiter.configure(vec![limit(&name, LimitScope::Group)], true);
        assert_eq!(limiter.tick(&procs, 8).len(), 1);
        limiter.tick(&procs, 8);
        let previews = messages(&events)
            .into_iter()
            .filter(|m| m.starts_with("dry-run: would limit"))
            .count();
        assert_eq!(previews, 1);
        assert!(limiter.budgets.is_empty());

        // Enforcing again, then switching to dry-run lifts everything.
        limiter.configure(vec![limit(&name, LimitScope::Group)], false);
        assert!(limiter.tick(&procs, 8)[0].processes >= 4);
        assert_eq!(limiter.budgets.len(), 1);
        limiter.configure(vec![limit(&name, LimitScope::Group)], true);
        assert!(limiter.budgets.is_empty());
        assert!(
            messages(&events)
                .iter()
                .any(|m| m.contains("dry-run enabled"))
        );

        // Orphaned children keep the budget; once all exit it is dropped.
        limiter.configure(vec![limit(&name, LimitScope::Group)], false);
        limiter.tick(&procs, 8);
        let children: Vec<u32> = procs
            .iter()
            .filter(|p| p.parent_pid == a.0.id() || p.parent_pid == b.0.id())
            // A console host may come and go with its parent; ping outlives it.
            .filter(|p| p.name.eq_ignore_ascii_case("ping.exe"))
            .map(|p| p.pid)
            .collect();
        drop((a, b));
        std::thread::sleep(Duration::from_millis(300));
        let now = Sampler::new().sample().unwrap().processes;
        let usage = limiter.tick(&now, 8);
        assert!(
            children
                .iter()
                .all(|c| usage[0].members.iter().any(|m| m.0 == *c)),
            "{usage:?}"
        );
        for pid in children {
            if let Ok(h) = ProcessHandle::open(pid, Access::TERMINATE | Access::SYNCHRONIZE) {
                let _ = h.terminate(1);
                let _ = h.wait_exit(Duration::from_secs(5));
            }
        }
        let now = Sampler::new().sample().unwrap().processes;
        assert!(limiter.tick(&now, 8).is_empty());
        assert!(limiter.budgets.is_empty());
        limiter.lift_all("test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failures_are_reported() {
        let events = EventLog::default();
        let mut limiter = AppLimiter::new(
            vec![limit("gone.exe", LimitScope::Group)],
            false,
            events.clone(),
        );
        // A process that exited before it could be added is skipped silently.
        let mut ghost = Sampler::new().sample().unwrap().processes.remove(0);
        ghost.pid = 0xFFFF_FFF0;
        ghost.parent_pid = 0;
        ghost.name = "gone.exe".into();
        limiter.tick(std::slice::from_ref(&ghost), 4);
        assert!(
            !messages(&events)
                .iter()
                .any(|m| m.starts_with("cannot apply the gone.exe limit")),
            "{:?}",
            messages(&events)
        );

        // Lifting a budget whose job rejects it is reported, not fatal.
        let budget = Budget {
            job: Job::create().unwrap(),
            cpu: Some(10.0),
            memory: None,
        };
        let group = GroupId {
            app: "x.exe".into(),
            scope: LimitScope::Group,
            root: None,
        };
        limiter.lift(&group, budget, "test");
        assert!(
            messages(&events)
                .iter()
                .any(|m| m.starts_with("cannot lift the x.exe limit"))
        );
        assert_eq!(
            describe(&limit("a", LimitScope::Group)),
            "1 cores, 1 GB (all processes together)"
        );
    }

    #[test]
    fn processes_in_foreign_jobs_are_counted_not_reported_one_by_one() {
        let dir = std::env::temp_dir().join(format!("wtm-applimit-foreign-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("wtm-foreign-{}.exe", std::process::id());
        let (a, b) = (spawn_app(&dir, &name), spawn_app(&dir, &name));
        // b already runs in a job of its own, like a sandboxed helper.
        let foreign = Job::create().unwrap();
        let hb = ProcessHandle::open(
            b.0.id(),
            Access::QUERY | Access::SET_QUOTA | Access::TERMINATE,
        )
        .unwrap();
        foreign.assign(&hb).unwrap();
        let procs = sample_with(&[a.0.id(), b.0.id()]);

        let events = EventLog::default();
        let mut limiter =
            AppLimiter::new(vec![limit(&name, LimitScope::Group)], false, events.clone());
        let usage = limiter.tick(&procs, 8);
        assert_eq!(usage.len(), 1);
        assert!(usage[0].outside >= 1, "{usage:?}");
        let log = messages(&events);
        let notes = log
            .iter()
            .filter(|m| m.contains("job of their own"))
            .count();
        assert_eq!(notes, 1, "{log:?}");
        assert!(
            !log.iter().any(|m| m.starts_with("cannot apply")),
            "{log:?}"
        );
        limiter.lift_all("test");
        drop((a, b));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

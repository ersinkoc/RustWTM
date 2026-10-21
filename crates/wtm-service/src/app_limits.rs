//! Applies per-application budgets ([`wtm_policy::AppLimit`]) with one job
//! object per budget. Processes started later by a member join the job
//! automatically (Windows puts children into their parent's job).

use std::collections::{HashMap, HashSet};

use wtm_core::control::ProcessRef;
use wtm_core::{ProcessInfo, format_bytes};
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
    /// Budgets that were reported as (nearly) out of memory.
    memory_full: HashSet<GroupId>,
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
            memory_full: HashSet::new(),
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
        if !self.outside.is_empty() {
            let live_keys: HashSet<(u32, i64)> =
                procs.iter().map(|p| (p.pid, p.create_time)).collect();
            self.outside.retain(|k| live_keys.contains(k));
        }
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
        self.memory_full.retain(|g| live.contains(g));
        for u in &usage {
            self.check_memory(u);
        }
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

    /// Warns once when a budget's memory is nearly used up (its processes
    /// then fail to allocate), and again only after it went below 85%.
    fn check_memory(&mut self, u: &GroupUsage) {
        let Some(limit) = u.limit.memory_bytes().filter(|&l| l > 0) else {
            return;
        };
        let fill = u.memory_bytes as f64 / limit as f64;
        if fill >= MEMORY_FULL {
            if self.memory_full.insert(u.group.clone()) {
                let instance = match u.group.root {
                    Some((pid, _)) => format!(" (instance PID {pid})"),
                    None => String::new(),
                };
                self.events.alert(
                    None,
                    format!(
                        "{}{instance} uses {} of its {} memory budget; its processes may fail \
                         to allocate memory. Raise the limit in the Limits tab if this is \
                         expected.",
                        u.limit.name,
                        format_bytes(u.memory_bytes as f64),
                        format_bytes(limit as f64)
                    ),
                );
            }
        } else if fill < MEMORY_OK {
            self.memory_full.remove(&u.group);
        }
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

/// Share of a memory budget that counts as full, and the share below which
/// it is reported again.
const MEMORY_FULL: f64 = 0.95;
const MEMORY_OK: f64 = 0.85;

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
    use std::os::windows::process::CommandExt;
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
        static NEXT_OUTPUT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let exe = dir.join(name);
        if !exe.exists() {
            std::fs::copy(r"C:\Windows\System32\cmd.exe", &exe).unwrap();
        }
        let output = dir.join(format!(
            "ping-{}.log",
            NEXT_OUTPUT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let log = std::fs::File::create(&output).unwrap();
        let mut child = Guard(
            Command::new(exe)
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .args(["/d", "/c", "ping -n 30 127.0.0.1"])
                .stdin(Stdio::null())
                .stdout(log)
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        // A process can appear in a snapshot before its initialization has
        // completed. Wait for an actual loopback reply before reassigning jobs
        // or ending its parent; neither a console host nor a PID is readiness.
        crate::test_util::wait_for("ping ready", || {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "fixture parent exited"
            );
            std::fs::read(&output)
                .unwrap_or_default()
                .windows(4)
                .any(|part| part == b"TTL=")
                .then_some(())
        });
        child
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
            let all = pids.iter().all(|pid| {
                snap.processes
                    .iter()
                    .any(|p| p.parent_pid == *pid && p.name.eq_ignore_ascii_case("ping.exe"))
            });
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
        assert_eq!(children.len(), 2, "both ping children must be ready");
        drop((a, b));
        let now = Sampler::new().sample().unwrap().processes;
        let usage = limiter.tick(&now, 8);
        assert!(
            children
                .iter()
                .all(|c| usage[0].members.iter().any(|m| m.0 == *c)),
            "expected children {children:?}; actual {usage:?}; live children {:?}",
            now.iter()
                .filter(|p| children.contains(&p.pid))
                .collect::<Vec<_>>()
        );
        // End everything still in the budget (console hosts included) until
        // it is empty; name what is left if it never empties.
        let mut usage = usage;
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while !usage.is_empty() {
            for &(pid, _) in usage.iter().flat_map(|u| &u.members) {
                if let Ok(h) = ProcessHandle::open(pid, Access::TERMINATE | Access::SYNCHRONIZE) {
                    let _ = h.terminate(1);
                    let _ = h.wait_exit(Duration::from_secs(2));
                }
            }
            let now = Sampler::new().sample().unwrap().processes;
            usage = limiter.tick(&now, 8);
            let left: Vec<_> = usage
                .iter()
                .flat_map(|u| &u.members)
                .map(|m| {
                    now.iter()
                        .find(|p| p.pid == m.0)
                        .map_or(format!("{}", m.0), |p| format!("{} {}", p.pid, p.name))
                })
                .collect();
            assert!(
                std::time::Instant::now() < deadline,
                "budget never emptied: {left:?}"
            );
        }
        assert!(limiter.budgets.is_empty());
        limiter.lift_all("test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_nearly_full_memory_budget_is_reported_once() {
        let events = EventLog::default();
        let mut limiter = AppLimiter::new(Vec::new(), false, events.clone());
        let usage = |memory_bytes: u64, root| GroupUsage {
            group: GroupId {
                app: "zen.exe".into(),
                scope: LimitScope::Each,
                root,
            },
            limit: limit("zen.exe", LimitScope::Each),
            processes: 3,
            members: Vec::new(),
            outside: 0,
            cpu_percent: 0.0,
            memory_bytes,
        };
        let alerts = |events: &EventLog| {
            events
                .after(0)
                .into_iter()
                .filter(|e| e.notify && e.level == wtm_ipc::EventLevel::Warn)
                .map(|e| e.message)
                .collect::<Vec<_>>()
        };
        let gb = 1u64 << 30;
        limiter.check_memory(&usage(gb / 2, Some((7, 1))));
        assert!(alerts(&events).is_empty());
        for _ in 0..3 {
            limiter.check_memory(&usage(gb * 96 / 100, Some((7, 1))));
        }
        let a = alerts(&events);
        assert_eq!(a.len(), 1, "{a:?}");
        assert!(
            a[0].starts_with("zen.exe (instance PID 7) uses "),
            "{}",
            a[0]
        );
        assert!(a[0].contains("of its 1.0 GiB memory budget"), "{}", a[0]);
        // Still high (between 85% and 95%): no repeat; after recovering, again.
        limiter.check_memory(&usage(gb * 90 / 100, Some((7, 1))));
        limiter.check_memory(&usage(gb * 96 / 100, Some((7, 1))));
        assert_eq!(alerts(&events).len(), 1);
        limiter.check_memory(&usage(gb / 2, Some((7, 1))));
        limiter.check_memory(&usage(gb, None));
        limiter.check_memory(&usage(gb, Some((7, 1))));
        let a = alerts(&events);
        assert_eq!(a.len(), 3, "{a:?}");
        assert!(a[1].starts_with("zen.exe uses "), "{}", a[1]);
        // Budgets without a memory limit are never full.
        let mut cpu_only = usage(u64::MAX, None);
        cpu_only.limit.memory_gb = None;
        limiter.check_memory(&cpu_only);
        assert_eq!(alerts(&events).len(), 3);
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

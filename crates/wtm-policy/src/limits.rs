//! Per-application limits: all processes of an app (and everything they
//! start) share one CPU and/or memory budget, either together (`group`) or per
//! top-level instance (`each`).
//!
//! [`LimitPlanner`] decides which job each process belongs to; applying the
//! limits with job objects is the service's job.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use wtm_core::{ProcessInfo, protection_reason};

use crate::{Key, glob_match};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LimitScope {
    /// Every matching process and its descendants share one budget.
    #[default]
    Group,
    /// Each top-level matching process shares a budget with its descendants.
    Each,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppLimit {
    /// Image-name wildcard, e.g. `node.exe`.
    pub name: String,
    /// CPU budget in logical processors (`2` = two fully busy cores).
    #[serde(default)]
    pub cpu_cores: Option<f32>,
    /// Committed-memory budget in GiB.
    #[serde(default)]
    pub memory_gb: Option<f32>,
    #[serde(default)]
    pub scope: LimitScope,
}

impl AppLimit {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("name must not be empty".into());
        }
        if self.cpu_cores.is_none() && self.memory_gb.is_none() {
            return Err("needs `cpu_cores` and/or `memory_gb`".into());
        }
        if self.cpu_cores.is_some_and(|c| !(c > 0.0 && c <= 4096.0)) {
            return Err("cpu_cores must be positive".into());
        }
        if self
            .memory_gb
            .is_some_and(|m| !(0.01..=65_536.0).contains(&m))
        {
            return Err("memory_gb must be at least 0.01".into());
        }
        Ok(())
    }

    /// The CPU budget as a share of the whole machine, capped at 100%.
    pub fn cpu_percent(&self, logical_cpus: u32) -> Option<f32> {
        self.cpu_cores
            .map(|c| (c / logical_cpus.max(1) as f32 * 100.0).min(100.0))
    }

    pub fn memory_bytes(&self) -> Option<u64> {
        self.memory_gb
            .map(|g| (f64::from(g) * f64::from(1u32 << 30)) as u64)
    }

    fn matches(&self, name: &str) -> bool {
        glob_match(&self.name, name)
    }

    /// Same app and scope; only the budgets may differ.
    fn same_target(&self, other: &AppLimit) -> bool {
        self.name.eq_ignore_ascii_case(&other.name) && self.scope == other.scope
    }
}

/// One budget: an app limit plus, for `each`, the top-level instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GroupId {
    pub app: String,
    pub scope: LimitScope,
    /// The top-level instance for `each`; `None` for `group`.
    pub root: Option<(u32, i64)>,
}

/// Current usage of one budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupUsage {
    pub group: GroupId,
    pub limit: AppLimit,
    pub processes: usize,
    /// Member processes as `(pid, create_time)`.
    pub members: Vec<(u32, i64)>,
    /// Share of the whole machine.
    pub cpu_percent: f32,
    /// Sum of private committed memory.
    pub memory_bytes: u64,
}

/// Tracks which budget each process belongs to. Membership is permanent for
/// a process instance: Windows cannot take a process out of a job.
#[derive(Debug, Default)]
pub struct LimitPlanner {
    limits: Vec<AppLimit>,
    members: HashMap<Key, GroupId>,
}

impl LimitPlanner {
    pub fn new(limits: Vec<AppLimit>) -> Self {
        Self {
            limits,
            members: HashMap::new(),
        }
    }

    pub fn limits(&self) -> &[AppLimit] {
        &self.limits
    }

    /// Replaces the limits. Returns the groups whose app or scope is gone;
    /// their budgets should be lifted. Groups of kept apps stay (with their
    /// possibly new budgets).
    pub fn set_limits(&mut self, limits: Vec<AppLimit>) -> Vec<GroupId> {
        let kept = |g: &GroupId| {
            limits
                .iter()
                .any(|l| l.name.eq_ignore_ascii_case(&g.app) && l.scope == g.scope)
        };
        let mut dropped: Vec<GroupId> = self
            .members
            .values()
            .filter(|g| !kept(g))
            .cloned()
            .collect();
        dropped.sort_by(|a, b| a.app.cmp(&b.app).then(a.root.cmp(&b.root)));
        dropped.dedup();
        self.members.retain(|_, g| kept(g));
        self.limits = limits;
        dropped
    }

    /// The limit governing `group`.
    pub fn limit_of(&self, group: &GroupId) -> Option<&AppLimit> {
        self.limits.iter().find(|l| {
            l.same_target(&AppLimit {
                name: group.app.clone(),
                scope: group.scope,
                cpu_cores: None,
                memory_gb: None,
            })
        })
    }

    /// Assigns new processes to budgets. Returns the processes to put into
    /// each group's job, parents before children. Exited processes are
    /// forgotten.
    pub fn plan(&mut self, procs: &[ProcessInfo]) -> Vec<(GroupId, ProcessInfo)> {
        let live: std::collections::HashSet<Key> =
            procs.iter().map(|p| (p.pid, p.create_time)).collect();
        self.members.retain(|k, _| live.contains(k));
        if self.limits.is_empty() {
            return Vec::new();
        }
        let index: HashMap<u32, usize> =
            procs.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
        // Ancestor chain, nearest first; a parent must predate its child.
        let chain = |mut i: usize| {
            let mut out = vec![i];
            while let Some(&parent) = index.get(&procs[i].parent_pid) {
                if parent == i
                    || procs[parent].create_time > procs[i].create_time
                    || out.contains(&parent)
                {
                    break;
                }
                out.push(parent);
                i = parent;
            }
            out
        };
        // Parents first, so a child joins its parent's group.
        let mut order: Vec<usize> = (0..procs.len()).collect();
        order.sort_by_key(|&i| procs[i].create_time);

        let mut out = Vec::new();
        for i in order {
            let p = &procs[i];
            let key = (p.pid, p.create_time);
            if self.members.contains_key(&key) || protection_reason(p.pid, &p.name).is_some() {
                continue;
            }
            let ancestors = chain(i);
            // An ancestor that already has a budget passes it on.
            let inherited = ancestors[1..]
                .iter()
                .find_map(|&a| self.members.get(&(procs[a].pid, procs[a].create_time)));
            let group = match inherited {
                Some(g) => Some(g.clone()),
                None => self.limits.iter().find_map(|l| {
                    // The topmost matching process in the chain is the root.
                    let root = ancestors
                        .iter()
                        .rev()
                        .find(|&&a| l.matches(&procs[a].name))?;
                    Some(GroupId {
                        app: l.name.to_lowercase(),
                        scope: l.scope,
                        root: match l.scope {
                            LimitScope::Group => None,
                            LimitScope::Each => Some((procs[*root].pid, procs[*root].create_time)),
                        },
                    })
                }),
            };
            if let Some(g) = group {
                self.members.insert(key, g.clone());
                out.push((g, p.clone()));
            }
        }
        out
    }

    /// Usage per group, sorted by app and root.
    pub fn usage(&self, procs: &[ProcessInfo]) -> Vec<GroupUsage> {
        // Members, CPU and memory per group.
        type Totals = (Vec<Key>, f32, u64);
        let mut by_group: HashMap<&GroupId, Totals> = HashMap::new();
        for p in procs {
            if let Some(g) = self.members.get(&(p.pid, p.create_time)) {
                let e = by_group.entry(g).or_default();
                e.0.push((p.pid, p.create_time));
                e.1 += p.cpu_percent;
                e.2 += p.private_bytes;
            }
        }
        let mut out: Vec<GroupUsage> = by_group
            .into_iter()
            .filter_map(|(g, (members, cpu, mem))| {
                Some(GroupUsage {
                    group: g.clone(),
                    limit: self.limit_of(g)?.clone(),
                    processes: members.len(),
                    members,
                    cpu_percent: cpu,
                    memory_bytes: mem,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            a.group
                .app
                .cmp(&b.group.app)
                .then(a.group.root.cmp(&b.group.root))
        });
        out
    }

    /// The group a process belongs to.
    pub fn group_of(&self, pid: u32, create_time: i64) -> Option<&GroupId> {
        self.members.get(&(pid, create_time))
    }
}

/// Adds `limit` to a configuration text, replacing an entry for the same app
/// and scope. Comments and formatting elsewhere are kept.
pub fn set_app_limit(text: &str, limit: &AppLimit) -> Result<String, String> {
    limit.validate()?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    let mut table = toml_edit::Table::new();
    table.insert("name", toml_edit::value(limit.name.clone()));
    if let Some(c) = limit.cpu_cores {
        table.insert("cpu_cores", toml_edit::value(f64::from(c)));
    }
    if let Some(m) = limit.memory_gb {
        table.insert("memory_gb", toml_edit::value(f64::from(m)));
    }
    let scope = match limit.scope {
        LimitScope::Group => "group",
        LimitScope::Each => "each",
    };
    table.insert("scope", toml_edit::value(scope));

    let limits = doc
        .entry("limit")
        .or_insert_with(|| toml_edit::Item::ArrayOfTables(Default::default()))
        .as_array_of_tables_mut()
        .ok_or("`limit` must be an array of tables ([[limit]])")?;
    let same = |t: &toml_edit::Table| {
        let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let s = t.get("scope").and_then(|v| v.as_str()).unwrap_or("group");
        name.eq_ignore_ascii_case(&limit.name) && s == scope
    };
    let found = limits.iter().position(same);
    match found {
        Some(i) => *limits.get_mut(i).expect("found above") = table,
        None => limits.push(table),
    }
    Ok(doc.to_string())
}

/// Removes the entry for `name` and `scope`; unchanged text if there is none.
pub fn remove_app_limit(text: &str, name: &str, scope: LimitScope) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    let scope = match scope {
        LimitScope::Group => "group",
        LimitScope::Each => "each",
    };
    if let Some(limits) = doc
        .get_mut("limit")
        .and_then(|i| i.as_array_of_tables_mut())
    {
        limits.retain(|t| {
            let n = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let s = t.get("scope").and_then(|v| v.as_str()).unwrap_or("group");
            !(n.eq_ignore_ascii_case(name) && s == scope)
        });
        if limits.is_empty() {
            doc.remove("limit");
        }
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn p(pid: u32, parent: u32, name: &str, created: i64) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: parent,
            name: name.into(),
            cpu_percent: 1.0,
            cpu_time: Duration::ZERO,
            working_set: 0,
            private_working_set: 0,
            private_bytes: 1 << 20,
            thread_count: 1,
            handle_count: 1,
            session_id: 1,
            base_priority: 8,
            gpu_percent: 0.0,
            gpu_memory: 0,
            io_read_rate: 0.0,
            io_write_rate: 0.0,
            create_time: created,
        }
    }

    fn limit(name: &str, scope: LimitScope) -> AppLimit {
        AppLimit {
            name: name.into(),
            cpu_cores: Some(2.0),
            memory_gb: Some(8.0),
            scope,
        }
    }

    fn pids(plan: &[(GroupId, ProcessInfo)]) -> Vec<(u32, Option<u32>)> {
        plan.iter()
            .map(|(g, p)| (p.pid, g.root.map(|r| r.0)))
            .collect()
    }

    /// explorer(1) -> zen(10) -> zen(11) -> helper(12); explorer -> zen(20);
    /// zen(20) -> crash(21); unrelated(30).
    fn tree() -> Vec<ProcessInfo> {
        vec![
            p(1, 0, "explorer.exe", 1),
            p(12, 11, "helper.exe", 13),
            p(10, 1, "zen.exe", 10),
            p(11, 10, "zen.exe", 11),
            p(20, 1, "Zen.exe", 20),
            p(21, 20, "crash.exe", 21),
            p(30, 1, "other.exe", 30),
            p(4, 0, "System", 0),
        ]
    }

    #[test]
    fn each_scope_groups_by_top_level_instance() {
        let mut planner = LimitPlanner::new(vec![limit("zen.exe", LimitScope::Each)]);
        let plan = planner.plan(&tree());
        assert_eq!(
            pids(&plan),
            [
                (10, Some(10)),
                (11, Some(10)),
                (12, Some(10)),
                (20, Some(20)),
                (21, Some(20))
            ]
        );
        // Already planned processes are not returned again.
        assert!(planner.plan(&tree()).is_empty());
        let usage = planner.usage(&tree());
        assert_eq!(usage.len(), 2);
        assert_eq!(usage[0].processes, 3);
        assert_eq!(usage[0].memory_bytes, 3 << 20);
        assert_eq!(usage[1].processes, 2);
        assert_eq!(planner.group_of(12, 13).unwrap().root, Some((10, 10)));
        assert!(planner.group_of(30, 30).is_none());
    }

    #[test]
    fn group_scope_shares_one_budget_and_children_inherit() {
        let mut planner = LimitPlanner::new(vec![limit("zen*", LimitScope::Group)]);
        let plan = planner.plan(&tree());
        assert_eq!(plan.len(), 5);
        assert!(
            plan.iter()
                .all(|(g, _)| g.root.is_none() && g.app == "zen*")
        );

        // A child started later joins its parent's group, whatever its name.
        let mut procs = tree();
        procs.push(p(40, 21, "late.exe", 40));
        let plan = planner.plan(&procs);
        assert_eq!(pids(&plan), [(40, None)]);

        // Exited processes are forgotten.
        procs.retain(|p| p.pid != 40);
        planner.plan(&procs);
        assert!(planner.group_of(40, 40).is_none());
        assert_eq!(planner.usage(&procs)[0].processes, 5);
    }

    #[test]
    fn set_limits_keeps_matching_groups_and_reports_dropped_ones() {
        let mut planner = LimitPlanner::new(vec![
            limit("zen.exe", LimitScope::Each),
            limit("other.exe", LimitScope::Group),
        ]);
        planner.plan(&tree());
        let mut changed = limit("ZEN.exe", LimitScope::Each);
        changed.cpu_cores = Some(1.0);
        let dropped = planner.set_limits(vec![changed.clone()]);
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].app, "other.exe");
        let g = planner.group_of(10, 10).unwrap().clone();
        assert_eq!(planner.limit_of(&g), Some(&changed));
        assert_eq!(planner.limits().len(), 1);

        // Without limits nothing is planned.
        assert_eq!(planner.set_limits(Vec::new()).len(), 2);
        assert!(planner.plan(&tree()).is_empty());
        assert!(planner.usage(&tree()).is_empty());
    }

    #[test]
    fn reused_parent_pids_and_cycles_do_not_confuse_the_chain() {
        let procs = [
            // Parent PID 5 was reused by a younger process.
            p(5, 0, "zen.exe", 50),
            p(6, 5, "child.exe", 10),
            // A cycle.
            p(7, 8, "a.exe", 1),
            p(8, 7, "a.exe", 1),
        ];
        let mut planner = LimitPlanner::new(vec![
            limit("zen.exe", LimitScope::Group),
            limit("a.exe", LimitScope::Each),
        ]);
        let plan = planner.plan(&procs);
        let got = pids(&plan);
        assert!(!got.iter().any(|(pid, _)| *pid == 6), "{got:?}");
        assert!(got.contains(&(5, None)));
        assert_eq!(
            got.iter().filter(|(pid, _)| *pid == 7 || *pid == 8).count(),
            2
        );
    }

    #[test]
    fn budgets_convert_and_validate() {
        let l = limit("node.exe", LimitScope::Group);
        assert_eq!(l.cpu_percent(16), Some(12.5));
        assert_eq!(l.cpu_percent(1), Some(100.0));
        assert_eq!(l.memory_bytes(), Some(8 << 30));
        assert!(l.validate().is_ok());
        let bad = |f: &dyn Fn(&mut AppLimit)| {
            let mut l = limit("x.exe", LimitScope::Group);
            f(&mut l);
            l.validate().unwrap_err()
        };
        assert_eq!(bad(&|l| l.name = " ".into()), "name must not be empty");
        assert_eq!(
            bad(&|l| {
                l.cpu_cores = None;
                l.memory_gb = None;
            }),
            "needs `cpu_cores` and/or `memory_gb`"
        );
        assert_eq!(
            bad(&|l| l.cpu_cores = Some(0.0)),
            "cpu_cores must be positive"
        );
        assert_eq!(
            bad(&|l| l.memory_gb = Some(0.0)),
            "memory_gb must be at least 0.01"
        );
    }

    #[test]
    fn edits_keep_comments_and_replace_entries() {
        let text = "# my config\ndry_run = false # enforce\n";
        let node = AppLimit {
            name: "node.exe".into(),
            cpu_cores: Some(2.0),
            memory_gb: None,
            scope: LimitScope::Group,
        };
        let one = set_app_limit(text, &node).unwrap();
        assert!(
            one.starts_with("# my config\ndry_run = false # enforce\n"),
            "{one}"
        );
        let parsed = crate::Config::from_toml(&one).unwrap();
        assert_eq!(parsed.limits, std::slice::from_ref(&node));

        let mut zen = limit("zen.exe", LimitScope::Each);
        let two = set_app_limit(&one, &zen).unwrap();
        zen.memory_gb = Some(4.0);
        let replaced = set_app_limit(&two, &zen).unwrap();
        let parsed = crate::Config::from_toml(&replaced).unwrap();
        assert_eq!(parsed.limits, [node.clone(), zen.clone()]);

        let removed = remove_app_limit(&replaced, "ZEN.EXE", LimitScope::Each).unwrap();
        assert_eq!(crate::Config::from_toml(&removed).unwrap().limits, [node]);
        let empty = remove_app_limit(&removed, "node.exe", LimitScope::Group).unwrap();
        assert!(!empty.contains("[[limit]]"), "{empty}");
        assert_eq!(remove_app_limit(text, "x", LimitScope::Each).unwrap(), text);

        assert!(set_app_limit("x = [", &zen).is_err());
        assert!(remove_app_limit("x = [", "a", LimitScope::Group).is_err());
        assert!(set_app_limit("limit = 3", &zen).is_err());
        zen.name.clear();
        assert!(set_app_limit(text, &zen).is_err());
    }
}

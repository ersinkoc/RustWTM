//! Per-process rules: match a process, optionally wait for a sustained
//! condition, then apply a list of actions once per process instance.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde::Deserialize;
use wtm_core::control::ProcessRef;
use wtm_core::{ProcessInfo, protection_reason};

use crate::{Action, Decision, Key, Source, glob_match};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    #[serde(rename = "match")]
    pub matcher: Matcher,
    #[serde(default)]
    pub when: Option<Condition>,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Matcher {
    /// Image-name wildcard, e.g. `chrome.exe`, `xmrig*`.
    pub name: Option<String>,
    /// Full image-path wildcard, e.g. `C:\Games\*\*.exe`.
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Condition {
    /// Share of total machine CPU.
    pub cpu_above: Option<f32>,
    /// Private committed memory.
    pub memory_above_mb: Option<u64>,
    /// How long all given thresholds must hold.
    pub for_secs: u64,
}

impl Rule {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.matcher.name.is_none() && self.matcher.path.is_none() {
            return Err("match needs `name` and/or `path`".into());
        }
        if self.actions.is_empty() {
            return Err("actions must not be empty".into());
        }
        if let Some(c) = &self.when
            && c.cpu_above.is_none()
            && c.memory_above_mb.is_none()
        {
            return Err("when needs `cpu_above` and/or `memory_above_mb`".into());
        }
        self.actions.iter().try_for_each(Action::validate)
    }
}

impl Matcher {
    fn matches(
        &self,
        p: &ProcessInfo,
        path_of: &mut dyn FnMut(&ProcessInfo) -> Option<String>,
    ) -> bool {
        if let Some(name) = &self.name
            && !glob_match(name, &p.name)
        {
            return false;
        }
        match &self.path {
            Some(pattern) => path_of(p).is_some_and(|path| glob_match(pattern, &path)),
            None => true,
        }
    }
}

impl Condition {
    fn holds(&self, p: &ProcessInfo) -> bool {
        self.cpu_above.is_none_or(|c| p.cpu_percent > c)
            && self
                .memory_above_mb
                .is_none_or(|mb| p.private_bytes > mb.saturating_mul(1 << 20))
    }
}

#[derive(Debug, Default)]
struct RuleState {
    since: Option<Instant>,
    fired: bool,
}

#[derive(Debug, Default)]
pub struct RuleEngine {
    rules: Vec<Rule>,
    state: HashMap<(usize, Key), RuleState>,
}

impl RuleEngine {
    pub fn new(rules: Vec<Rule>) -> Self {
        Self {
            rules,
            state: HashMap::new(),
        }
    }

    /// Replaces the rules; state is reset because rule indices change.
    pub fn set_rules(&mut self, rules: Vec<Rule>) {
        if rules != self.rules {
            self.rules = rules;
            self.state.clear();
        }
    }

    /// `path_of` resolves a process's image path; it is only called for rules
    /// that match on path, so callers can resolve lazily and cache.
    pub fn tick(
        &mut self,
        now: Instant,
        procs: &[ProcessInfo],
        path_of: &mut dyn FnMut(&ProcessInfo) -> Option<String>,
    ) -> Vec<Decision> {
        let out = self.evaluate(now, procs, path_of, false);
        let live: HashSet<Key> = procs.iter().map(|p| (p.pid, p.create_time)).collect();
        self.state.retain(|(_, key), _| live.contains(key));
        out
    }

    /// Fast path for processes that just started: evaluates only rules
    /// without a `when` condition (those need no usage history), so e.g. a
    /// `kill` rule acts within milliseconds instead of at the next tick.
    /// `procs` may be a partial list; no state is pruned.
    pub fn on_started(
        &mut self,
        now: Instant,
        procs: &[ProcessInfo],
        path_of: &mut dyn FnMut(&ProcessInfo) -> Option<String>,
    ) -> Vec<Decision> {
        self.evaluate(now, procs, path_of, true)
    }

    fn evaluate(
        &mut self,
        now: Instant,
        procs: &[ProcessInfo],
        path_of: &mut dyn FnMut(&ProcessInfo) -> Option<String>,
        unconditional_only: bool,
    ) -> Vec<Decision> {
        let mut out = Vec::new();
        for (idx, rule) in self.rules.iter().enumerate() {
            if unconditional_only && rule.when.is_some() {
                continue;
            }
            for p in procs {
                if protection_reason(p.pid, &p.name).is_some() {
                    continue;
                }
                let state = self.state.entry((idx, (p.pid, p.create_time))).or_default();
                if state.fired || !rule.matcher.matches(p, path_of) {
                    continue;
                }
                let ready = match &rule.when {
                    None => true,
                    Some(c) if c.holds(p) => {
                        let since = *state.since.get_or_insert(now);
                        now - since >= Duration::from_secs(c.for_secs)
                    }
                    Some(_) => {
                        state.since = None;
                        false
                    }
                };
                if !ready {
                    continue;
                }
                state.fired = true;
                let target = ProcessRef::from(p);
                out.extend(rule.actions.iter().map(|a| Decision::Apply {
                    source: Source::Rule,
                    target: target.clone(),
                    action: a.resolve(p.cpu_percent),
                    reason: format!("rule '{}'", rule.name),
                }));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, name: &str, cpu: f32, mem_mb: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: 1,
            name: name.into(),
            cpu_percent: cpu,
            cpu_time: Duration::ZERO,
            working_set: 0,
            private_working_set: 0,
            private_bytes: mem_mb << 20,
            thread_count: 1,
            handle_count: 1,
            session_id: 1,
            base_priority: 8,
            gpu_percent: 0.0,
            gpu_memory: 0,
            io_read_rate: 0.0,
            io_write_rate: 0.0,
            create_time: i64::from(pid) * 10,
        }
    }

    fn rule(name: Option<&str>, path: Option<&str>, when: Option<Condition>) -> Rule {
        Rule {
            name: "test".into(),
            matcher: Matcher {
                name: name.map(Into::into),
                path: path.map(Into::into),
            },
            when,
            actions: vec![Action::EcoQos, Action::CpuCapRelative(0.5)],
        }
    }

    fn no_path(_: &ProcessInfo) -> Option<String> {
        None
    }

    #[test]
    fn fires_once_after_sustained_condition() {
        let cond = Condition {
            cpu_above: Some(10.0),
            for_secs: 5,
            ..Default::default()
        };
        let mut e = RuleEngine::new(vec![rule(Some("hog*"), None, Some(cond))]);
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        let procs = |cpu| vec![proc(10, "hog.exe", cpu, 0), proc(11, "other.exe", 90.0, 0)];

        assert!(e.tick(at(0), &procs(20.0), &mut no_path).is_empty());
        // Condition broken: timer restarts.
        assert!(e.tick(at(3), &procs(5.0), &mut no_path).is_empty());
        assert!(e.tick(at(4), &procs(20.0), &mut no_path).is_empty());
        let d = e.tick(at(9), &procs(20.0), &mut no_path);
        assert_eq!(d.len(), 2);
        assert!(matches!(
            &d[1],
            Decision::Apply { action: Action::CpuCap(c), .. } if *c == 10.0
        ));
        // Already fired for this instance.
        assert!(e.tick(at(20), &procs(20.0), &mut no_path).is_empty());
    }

    #[test]
    fn unconditional_rule_and_memory_condition() {
        let mut e = RuleEngine::new(vec![
            rule(Some("a.exe"), None, None),
            rule(
                Some("b.exe"),
                None,
                Some(Condition {
                    memory_above_mb: Some(100),
                    ..Default::default()
                }),
            ),
        ]);
        let now = Instant::now();
        let procs = [
            proc(1, "A.EXE", 0.0, 0),
            proc(2, "b.exe", 0.0, 50),
            proc(3, "b.exe", 0.0, 200),
        ];
        let pids: Vec<u32> = e
            .tick(now, &procs, &mut no_path)
            .iter()
            .map(|d| d.target().pid)
            .collect();
        assert_eq!(pids, [1, 1, 3, 3]);
    }

    #[test]
    fn path_rules_resolve_lazily_and_skip_protected() {
        let mut e = RuleEngine::new(vec![rule(None, Some("c:\\bad\\*"), None)]);
        let mut calls = 0;
        let mut path_of = |p: &ProcessInfo| {
            calls += 1;
            Some(format!("C:\\bad\\{}", p.name))
        };
        let procs = [proc(10, "x.exe", 0.0, 0), proc(4, "System", 0.0, 0)];
        let d = e.tick(Instant::now(), &procs, &mut path_of);
        assert_eq!(d.len(), 2);
        assert_eq!(calls, 1, "protected processes are never resolved");
    }

    #[test]
    fn new_instance_with_reused_pid_fires_again() {
        let mut e = RuleEngine::new(vec![rule(Some("a.exe"), None, None)]);
        let now = Instant::now();
        let first = proc(10, "a.exe", 0.0, 0);
        assert_eq!(
            e.tick(now, std::slice::from_ref(&first), &mut no_path)
                .len(),
            2
        );
        let mut second = first.clone();
        second.create_time += 1;
        assert_eq!(e.tick(now, &[second], &mut no_path).len(), 2);
    }

    #[test]
    fn on_started_fires_unconditional_rules_only_and_once() {
        let cond = Condition {
            cpu_above: Some(0.0),
            ..Default::default()
        };
        let mut e = RuleEngine::new(vec![
            rule(Some("a.exe"), None, None),
            rule(Some("a.exe"), None, Some(cond)),
        ]);
        let now = Instant::now();
        let started = [proc(10, "a.exe", 5.0, 0)];
        let d = e.on_started(now, &started, &mut no_path);
        assert_eq!(d.len(), 2, "only the unconditional rule's two actions");

        // The next full tick must not fire the unconditional rule again, but
        // evaluates the conditional one.
        let others = [proc(10, "a.exe", 5.0, 0), proc(11, "b.exe", 0.0, 0)];
        assert_eq!(e.tick(now, &others, &mut no_path).len(), 2);
        assert!(e.tick(now, &others, &mut no_path).is_empty());
    }

    #[test]
    fn validate_rejects_empty_condition() {
        let r = rule(Some("a.exe"), None, Some(Condition::default()));
        assert_eq!(
            r.validate().unwrap_err(),
            "when needs `cpu_above` and/or `memory_above_mb`"
        );
        let mem_only = Condition {
            memory_above_mb: Some(1),
            ..Default::default()
        };
        assert!(rule(Some("a.exe"), None, Some(mem_only)).validate().is_ok());
    }

    #[test]
    fn unresolvable_path_never_matches() {
        let mut e = RuleEngine::new(vec![rule(None, Some("*"), None)]);
        let procs = [proc(10, "x.exe", 0.0, 0)];
        assert!(e.tick(Instant::now(), &procs, &mut no_path).is_empty());
    }

    #[test]
    fn set_rules_resets_state_only_when_rules_change() {
        let mut e = RuleEngine::new(vec![rule(Some("a.exe"), None, None)]);
        let now = Instant::now();
        let procs = [proc(10, "a.exe", 0.0, 0)];
        assert_eq!(e.tick(now, &procs, &mut no_path).len(), 2);

        // Same rules: the instance stays marked as fired.
        e.set_rules(vec![rule(Some("a.exe"), None, None)]);
        assert!(e.tick(now, &procs, &mut no_path).is_empty());

        // Different rules: state is cleared and the new rule fires.
        let mut kill = rule(Some("a.exe"), None, None);
        kill.actions = vec![Action::Kill];
        e.set_rules(vec![kill]);
        let d = e.tick(now, &procs, &mut no_path);
        assert!(matches!(
            d.as_slice(),
            [Decision::Apply {
                action: Action::Kill,
                ..
            }]
        ));
    }
}

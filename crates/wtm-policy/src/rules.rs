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
    /// Text the command line contains, or a wildcard for all of it.
    pub cmdline: Option<String>,
}

/// What a rule may need to know about a process beyond its snapshot entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    Path,
    CommandLine,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Condition {
    /// Share of total machine CPU.
    pub cpu_above: Option<f32>,
    /// Private committed memory.
    pub memory_above_mb: Option<u64>,
    /// GPU utilization of the process's busiest engine.
    pub gpu_above: Option<f32>,
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
        if self
            .actions
            .iter()
            .any(|a| matches!(a, Action::Kill | Action::Suspend))
            && let Some(name) = self.matcher.name.as_deref().map(str::trim)
        {
            if name == "*" {
                return Err(
                    "kill or suspend with `match.name = \"*\"` would hit every program".into(),
                );
            }
            if let Some(why) = wtm_core::automatic_end_blocked(name) {
                return Err(format!(
                    "{name} cannot be ended or suspended by a rule: {why}"
                ));
            }
        }
        // `*.exe`, `**` or a path of `*` hit every program as `*` does, unless
        // a command line or a narrower name or path picks some out.
        if self
            .actions
            .iter()
            .any(|a| matches!(a, Action::Kill | Action::Suspend))
            && self.matcher.cmdline.is_none()
            && self
                .matcher
                .name
                .as_deref()
                .is_none_or(|n| matches_every_program(n, &PROBE_NAMES))
            && self
                .matcher
                .path
                .as_deref()
                .is_none_or(|p| matches_every_program(p, &PROBE_PATHS))
        {
            return Err("kill or suspend with this match would hit every program".into());
        }
        if let Some(c) = &self.when
            && c.cpu_above.is_none()
            && c.memory_above_mb.is_none()
            && c.gpu_above.is_none()
        {
            return Err("when needs `cpu_above`, `memory_above_mb` and/or `gpu_above`".into());
        }
        // A share of the CPU the process used when the rule fired: without a
        // CPU threshold that can be nothing (a process just started, or idle
        // for a moment), and the cap would all but freeze it.
        if self.cpu_baseline().is_none()
            && self
                .actions
                .iter()
                .any(|a| matches!(a, Action::CpuCapRelative(_)))
        {
            return Err("cpu_cap_relative needs `when.cpu_above` in a rule".into());
        }
        self.actions.iter().try_for_each(Action::validate)
    }

    /// The least CPU a process has when this rule fires on it.
    fn cpu_baseline(&self) -> Option<f32> {
        self.when.as_ref().and_then(|c| c.cpu_above)
    }
}

/// Unrelated programs: a pattern matching all of them matches any program.
pub(crate) const PROBE_NAMES: [&str; 3] = ["a.exe", "explorer.exe", "zq9-x_y.exe"];
const PROBE_PATHS: [&str; 3] = [r"C:\a.exe", r"C:\Windows\explorer.exe", r"D:\zq9\x_y.exe"];

/// Whether `pattern` matches every one of `probes`.
pub(crate) fn matches_every_program(pattern: &str, probes: &[&str]) -> bool {
    probes.iter().all(|p| glob_match(pattern, p))
}

impl Matcher {
    /// The image-name part, which needs nothing but the snapshot entry.
    fn name_matches(&self, p: &ProcessInfo) -> bool {
        self.name
            .as_deref()
            .is_none_or(|name| glob_match(name, &p.name))
    }

    /// The path and command-line parts; `i` is the process's index.
    fn details_match(&self, i: usize, p: &ProcessInfo, details: &mut Details<'_>) -> bool {
        if let Some(pattern) = &self.path
            && !details
                .get(i, p, Detail::Path)
                .is_some_and(|path| glob_match(pattern, path))
        {
            return false;
        }
        self.cmdline.as_deref().is_none_or(|pattern| {
            details
                .get(i, p, Detail::CommandLine)
                .is_some_and(|line| crate::text_matches(pattern, line))
        })
    }
}

/// Paths and command lines resolved during one evaluation, so several rules
/// looking at the same process ask (and allocate) only once.
struct Details<'a> {
    lookup: &'a mut dyn FnMut(&ProcessInfo, Detail) -> Option<String>,
    procs: usize,
    /// Per process: path, command line; `None` while not asked yet. Only
    /// allocated once some rule needs a detail.
    resolved: Vec<[Option<Option<String>>; 2]>,
}

impl Details<'_> {
    fn get(&mut self, i: usize, p: &ProcessInfo, d: Detail) -> Option<&str> {
        if self.resolved.is_empty() {
            self.resolved.resize(self.procs, [None, None]);
        }
        let slot = match d {
            Detail::Path => 0,
            Detail::CommandLine => 1,
        };
        self.resolved[i][slot]
            .get_or_insert_with(|| (self.lookup)(p, d))
            .as_deref()
    }
}

impl Condition {
    fn holds(&self, p: &ProcessInfo) -> bool {
        self.cpu_above.is_none_or(|c| p.cpu_percent > c)
            && self
                .memory_above_mb
                .is_none_or(|mb| p.private_bytes > mb.saturating_mul(1 << 20))
            && self.gpu_above.is_none_or(|g| p.gpu_percent > g)
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

    /// `lookup` resolves a process's image path or command line; it is only
    /// called for rules that match on them, so callers can resolve lazily
    /// and cache.
    pub fn tick(
        &mut self,
        now: Instant,
        procs: &[ProcessInfo],
        lookup: &mut dyn FnMut(&ProcessInfo, Detail) -> Option<String>,
    ) -> Vec<Decision> {
        let out = self.evaluate(now, procs, lookup, false);
        if !self.state.is_empty() {
            let live: HashSet<Key> = procs.iter().map(|p| (p.pid, p.create_time)).collect();
            self.state.retain(|(_, key), _| live.contains(key));
        }
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
        lookup: &mut dyn FnMut(&ProcessInfo, Detail) -> Option<String>,
    ) -> Vec<Decision> {
        self.evaluate(now, procs, lookup, true)
    }

    fn evaluate(
        &mut self,
        now: Instant,
        procs: &[ProcessInfo],
        lookup: &mut dyn FnMut(&ProcessInfo, Detail) -> Option<String>,
        unconditional_only: bool,
    ) -> Vec<Decision> {
        let mut out = Vec::new();
        if self.rules.is_empty() {
            return out;
        }
        // Once per process, not once per rule and process.
        let protected: Vec<bool> = procs
            .iter()
            .map(|p| protection_reason(p.pid, &p.name).is_some())
            .collect();
        let mut details = Details {
            lookup,
            procs: procs.len(),
            resolved: Vec::new(),
        };
        for (idx, rule) in self.rules.iter().enumerate() {
            if unconditional_only && rule.when.is_some() {
                continue;
            }
            for (i, (p, &protected)) in procs.iter().zip(&protected).enumerate() {
                if protected || !rule.matcher.name_matches(p) {
                    continue;
                }
                let key = (idx, (p.pid, p.create_time));
                // State is only kept for processes a rule matches, not for
                // every rule and process pair.
                if self.state.get(&key).is_some_and(|s| s.fired)
                    || !rule.matcher.details_match(i, p, &mut details)
                {
                    continue;
                }
                let state = self.state.entry(key).or_default();
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
                    action: a.resolve(p.cpu_percent.max(rule.cpu_baseline().unwrap_or(0.0))),
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
            net_rx_rate: 0.0,
            net_tx_rate: 0.0,
            suspended: false,
            create_time: i64::from(pid) * 10,
        }
    }

    fn rule(name: Option<&str>, path: Option<&str>, when: Option<Condition>) -> Rule {
        Rule {
            name: "test".into(),
            matcher: Matcher {
                name: name.map(Into::into),
                path: path.map(Into::into),
                cmdline: None,
            },
            when,
            actions: vec![Action::EcoQos, Action::CpuCap(50.0)],
        }
    }

    fn no_path(_: &ProcessInfo, _: Detail) -> Option<String> {
        None
    }

    #[test]
    fn matches_on_the_command_line() {
        let mut r = rule(Some("node.exe"), None, None);
        r.matcher.cmdline = Some("vitest".into());
        let mut e = RuleEngine::new(vec![r]);
        let procs = [proc(10, "node.exe", 1.0, 0), proc(11, "node.exe", 1.0, 0)];
        let mut asked = Vec::new();
        let mut lookup = |p: &ProcessInfo, d: Detail| {
            asked.push((p.pid, d));
            (p.pid == 10).then(|| "node node_modules/vitest/vitest.mjs".to_owned())
        };
        let d = e.tick(Instant::now(), &procs, &mut lookup);
        assert!(d.iter().all(|d| d.target().pid == 10) && d.len() == 2);
        assert_eq!(
            asked,
            [(10, Detail::CommandLine), (11, Detail::CommandLine)]
        );
        // A command line alone is not enough to pick processes.
        let mut only = rule(None, None, None);
        only.matcher.cmdline = Some("x".into());
        assert!(only.validate().is_err());
    }

    #[test]
    fn fires_once_after_sustained_condition() {
        let cond = Condition {
            cpu_above: Some(10.0),
            for_secs: 5,
            ..Default::default()
        };
        let mut r = rule(Some("hog*"), None, Some(cond));
        r.actions[1] = Action::CpuCapRelative(0.5);
        assert!(r.validate().is_ok());
        let mut e = RuleEngine::new(vec![r]);
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
        let mut path_of = |p: &ProcessInfo, _: Detail| {
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
    fn relative_caps_need_a_cpu_threshold_and_never_fall_below_it() {
        let mut r = rule(Some("a.exe"), None, None);
        r.actions = vec![Action::CpuCapRelative(0.5)];
        assert_eq!(
            r.validate().unwrap_err(),
            "cpu_cap_relative needs `when.cpu_above` in a rule"
        );
        r.when = Some(Condition {
            memory_above_mb: Some(1),
            ..Default::default()
        });
        assert!(r.validate().is_err(), "memory alone is no CPU baseline");
        r.when = Some(Condition {
            cpu_above: Some(40.0),
            ..Default::default()
        });
        assert!(r.validate().is_ok());
        assert_eq!(r.cpu_baseline(), Some(40.0));
    }

    #[test]
    fn rules_never_end_everything_or_service_hosts() {
        for (name, action) in [
            ("*", Action::Kill),
            ("svchost.exe", Action::Suspend),
            ("wtm-service.exe", Action::Kill),
        ] {
            let r = Rule {
                actions: vec![action],
                ..rule(Some(name), None, None)
            };
            assert!(r.validate().is_err(), "{name}");
        }
        // Throttling them stays possible.
        assert!(rule(Some("*"), None, None).validate().is_ok());
    }

    #[test]
    fn validate_rejects_empty_condition() {
        let r = rule(Some("a.exe"), None, Some(Condition::default()));
        assert_eq!(
            r.validate().unwrap_err(),
            "when needs `cpu_above`, `memory_above_mb` and/or `gpu_above`"
        );
        let mem_only = Condition {
            memory_above_mb: Some(1),
            ..Default::default()
        };
        assert!(rule(Some("a.exe"), None, Some(mem_only)).validate().is_ok());
    }

    #[test]
    fn keeps_state_only_for_matches_and_resolves_details_once() {
        let mut e = RuleEngine::new(vec![
            rule(Some("a.exe"), None, None),
            rule(None, Some("c:\\x\\*"), None),
            rule(None, Some("c:\\y\\*"), None),
        ]);
        let procs: Vec<ProcessInfo> = (10..110).map(|pid| proc(pid, "b.exe", 0.0, 0)).collect();
        let mut calls = 0;
        let mut path_of = |_: &ProcessInfo, _: Detail| {
            calls += 1;
            Some("C:\\z\\b.exe".to_owned())
        };
        assert!(e.tick(Instant::now(), &procs, &mut path_of).is_empty());
        assert_eq!(calls, 100, "two path rules, one lookup per process");
        assert!(e.state.is_empty(), "nothing matched, nothing to remember");
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

    #[test]
    fn gpu_condition() {
        let cond = Condition {
            gpu_above: Some(50.0),
            for_secs: 2,
            ..Default::default()
        };
        let r = rule(Some("game.exe"), None, Some(cond));
        assert!(r.validate().is_ok());
        let mut e = RuleEngine::new(vec![r]);
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        let game = |gpu| ProcessInfo {
            gpu_percent: gpu,
            ..proc(10, "game.exe", 1.0, 0)
        };
        assert!(e.tick(at(0), &[game(80.0)], &mut no_path).is_empty());
        assert!(e.tick(at(1), &[game(10.0)], &mut no_path).is_empty());
        assert!(e.tick(at(2), &[game(80.0)], &mut no_path).is_empty());
        assert_eq!(e.tick(at(4), &[game(80.0)], &mut no_path).len(), 2);
    }

    #[test]
    fn ending_every_program_is_refused_however_spelled() {
        let rule = |m: &str| {
            crate::Config::from_toml(&format!(
                "[[rule]]\nname = \"r\"\nmatch = {{ {m} }}\nactions = [\"kill\"]\n"
            ))
        };
        let kill = |e: &str| crate::Config::from_toml(&format!("[[kill]]\n{e}\n"));
        for m in [r#"name = "*.exe""#, r#"name = "**""#, r#"path = "*""#] {
            assert!(rule(m).is_err(), "{m}");
        }
        assert!(kill(r#"name = "*.exe""#).is_err());
        for m in [
            r#"name = "xmrig*.exe""#,
            r#"name = "*.exe", cmdline = "vitest""#,
            r#"path = "*", name = "miner.exe""#,
        ] {
            assert!(rule(m).is_ok(), "{m}");
        }
        assert!(kill("name = \"*.exe\"\ncmdline = \"vitest\"").is_ok());
    }
}

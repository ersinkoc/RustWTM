//! The kill list: processes that may be ended when they misbehave. Each
//! entry matches by image name and, optionally, by command line (`node.exe`
//! running `vitest`, but not other Node programs). Entries with `cpu_above`
//! are ended automatically when they keep using that much CPU; every entry
//! is ended at once by the panic button.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use wtm_core::control::ProcessRef;
use wtm_core::{ProcessInfo, descendants, protection_reason};

use crate::{Action, Decision, Key, Source, glob_match};

fn default_for_secs() -> u64 {
    60
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KillEntry {
    /// Image-name wildcard, e.g. `node.exe`.
    pub name: String,
    /// Text the command line must contain (case-insensitive), or a wildcard
    /// pattern for the whole command line when it has `*` or `?`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmdline: Option<String>,
    /// End it automatically once it uses at least this share of the machine's
    /// CPU for `for_secs`; without it only the panic button ends it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_above: Option<f32>,
    #[serde(default = "default_for_secs")]
    pub for_secs: u64,
    /// Only while the whole machine is at least this busy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_cpu_above: Option<f32>,
    /// Also end the processes it started.
    #[serde(default = "yes")]
    pub tree: bool,
}

impl Default for KillEntry {
    fn default() -> Self {
        Self {
            name: String::new(),
            cmdline: None,
            cpu_above: None,
            for_secs: default_for_secs(),
            system_cpu_above: None,
            tree: true,
        }
    }
}

/// Whether `text` contains `pattern`, or matches it when it has wildcards.
pub fn text_matches(pattern: &str, text: &str) -> bool {
    if pattern.contains(['*', '?']) {
        glob_match(pattern, text)
    } else {
        text.to_lowercase().contains(&pattern.to_lowercase())
    }
}

impl KillEntry {
    /// `command_line` is only called when the name matches and the entry
    /// looks at the command line, so callers can resolve it lazily.
    pub fn matches(
        &self,
        p: &ProcessInfo,
        command_line: &mut dyn FnMut(&ProcessInfo) -> Option<String>,
    ) -> bool {
        glob_match(&self.name, &p.name)
            && self.cmdline.as_deref().is_none_or(|pattern| {
                command_line(p).is_some_and(|line| text_matches(pattern, &line))
            })
    }

    /// `node.exe` or `node.exe running "vitest"`.
    pub fn label(&self) -> String {
        match &self.cmdline {
            Some(c) => format!("{} running \"{c}\"", self.name),
            None => self.name.clone(),
        }
    }

    /// When it is ended, in words.
    pub fn describe(&self) -> String {
        let Some(cpu) = self.cpu_above else {
            return "only with the panic button".into();
        };
        let mut s = format!(
            "automatically after {} s at {cpu:.0}% CPU or more",
            self.for_secs
        );
        if let Some(sys) = self.system_cpu_above {
            s.push_str(&format!(" while the PC is above {sys:.0}%"));
        }
        s
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("name must not be empty".into());
        }
        if self.cmdline.as_deref().is_some_and(|c| c.trim().is_empty()) {
            return Err("cmdline must not be empty (leave it out to match any)".into());
        }
        for (key, v) in [
            ("cpu_above", self.cpu_above),
            ("system_cpu_above", self.system_cpu_above),
        ] {
            if let Some(v) = v
                && !(v > 0.0 && v <= 100.0)
            {
                return Err(format!("{key} must be in (0, 100], got {v}"));
            }
        }
        if self.cpu_above.is_some() && self.for_secs == 0 {
            return Err("for_secs must be at least 1".into());
        }
        Ok(())
    }
}

/// A process the panic button would end, with the entry that matched it.
#[derive(Debug, Clone)]
pub struct PanicTarget<'a> {
    pub process: &'a ProcessInfo,
    pub entry: usize,
    /// Started by a matching process rather than matching itself.
    pub child: bool,
}

/// Everything the panic button ends right now: matching processes and, for
/// entries with `tree`, what they started. Protected processes never.
pub fn panic_targets<'a>(
    entries: &[KillEntry],
    procs: &'a [ProcessInfo],
    command_line: &mut dyn FnMut(&ProcessInfo) -> Option<String>,
) -> Vec<PanicTarget<'a>> {
    let mut seen: HashSet<Key> = HashSet::new();
    let mut out = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        for p in procs {
            if protection_reason(p.pid, &p.name).is_some()
                || seen.contains(&(p.pid, p.create_time))
                || !e.matches(p, command_line)
            {
                continue;
            }
            seen.insert((p.pid, p.create_time));
            out.push(PanicTarget {
                process: p,
                entry: i,
                child: false,
            });
            if e.tree {
                for c in descendants(p.pid, p.create_time, procs) {
                    if protection_reason(c.pid, &c.name).is_none()
                        && seen.insert((c.pid, c.create_time))
                    {
                        out.push(PanicTarget {
                            process: c,
                            entry: i,
                            child: true,
                        });
                    }
                }
            }
        }
    }
    out
}

/// Ends kill-list processes that keep using too much CPU.
#[derive(Debug, Default)]
pub struct KillWatch {
    entries: Vec<KillEntry>,
    /// Since when a process is continuously above its entry's threshold.
    since: HashMap<(usize, Key), Instant>,
    /// Already ended (or asked to be), so each process is ended once.
    ended: HashSet<Key>,
}

impl KillWatch {
    pub fn new(entries: Vec<KillEntry>) -> Self {
        Self {
            entries,
            ..Self::default()
        }
    }

    pub fn set_entries(&mut self, entries: Vec<KillEntry>) {
        if entries != self.entries {
            self.entries = entries;
            self.since.clear();
        }
    }

    /// `system_cpu` is the whole machine's load; the command line is only
    /// looked up for processes that are over their CPU threshold.
    pub fn tick(
        &mut self,
        now: Instant,
        procs: &[ProcessInfo],
        system_cpu: f32,
        command_line: &mut dyn FnMut(&ProcessInfo) -> Option<String>,
    ) -> Vec<Decision> {
        let mut out = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            let Some(threshold) = e.cpu_above else {
                continue;
            };
            let busy_machine = e.system_cpu_above.is_none_or(|s| system_cpu >= s);
            for p in procs {
                let key = (p.pid, p.create_time);
                if !busy_machine
                    || p.cpu_percent < threshold
                    || self.ended.contains(&key)
                    || protection_reason(p.pid, &p.name).is_some()
                    || !e.matches(p, command_line)
                {
                    self.since.remove(&(i, key));
                    continue;
                }
                let since = *self.since.entry((i, key)).or_insert(now);
                if now - since < Duration::from_secs(e.for_secs) {
                    continue;
                }
                let reason = format!(
                    "kill list '{}': {:.0}% CPU for {} s",
                    e.label(),
                    p.cpu_percent,
                    e.for_secs
                );
                let mut victims = vec![p];
                if e.tree {
                    victims.extend(descendants(p.pid, p.create_time, procs));
                }
                for v in victims {
                    if protection_reason(v.pid, &v.name).is_none()
                        && self.ended.insert((v.pid, v.create_time))
                    {
                        out.push(Decision::Apply {
                            source: Source::Rule,
                            target: ProcessRef::from(v),
                            action: Action::Kill,
                            reason: reason.clone(),
                        });
                    }
                }
            }
        }
        let live: HashSet<Key> = procs.iter().map(|p| (p.pid, p.create_time)).collect();
        self.since.retain(|(_, k), _| live.contains(k));
        self.ended.retain(|k| live.contains(k));
        out
    }
}

fn entry_table(e: &KillEntry) -> toml_edit::Table {
    let mut t = toml_edit::Table::new();
    t.insert("name", toml_edit::value(e.name.clone()));
    if let Some(c) = &e.cmdline {
        t.insert("cmdline", toml_edit::value(c.clone()));
    }
    if let Some(c) = e.cpu_above {
        t.insert("cpu_above", toml_edit::value(f64::from(c)));
        t.insert("for_secs", toml_edit::value(e.for_secs as i64));
    }
    if let Some(s) = e.system_cpu_above {
        t.insert("system_cpu_above", toml_edit::value(f64::from(s)));
    }
    t.insert("tree", toml_edit::value(e.tree));
    t
}

/// Adds `entry` to the kill list, or replaces the entry at `index`.
pub fn set_kill_entry(
    text: &str,
    entry: &KillEntry,
    index: Option<usize>,
) -> Result<String, String> {
    entry.validate()?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    let list = doc
        .entry("kill")
        .or_insert_with(|| toml_edit::Item::ArrayOfTables(Default::default()))
        .as_array_of_tables_mut()
        .ok_or("`kill` must be an array of tables ([[kill]])")?;
    match index.and_then(|i| list.get_mut(i)) {
        Some(t) => *t = entry_table(entry),
        None => list.push(entry_table(entry)),
    }
    Ok(doc.to_string())
}

/// Removes the entry at `index`; unchanged text if there is none.
pub fn remove_kill_entry(text: &str, index: usize) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    if let Some(list) = doc.get_mut("kill").and_then(|i| i.as_array_of_tables_mut()) {
        if index < list.len() {
            list.remove(index);
        }
        if list.is_empty() {
            doc.remove("kill");
        }
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, parent: u32, name: &str, cpu: f32) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: parent,
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
            create_time: i64::from(pid) * 10,
        }
    }

    fn cmdlines(p: &ProcessInfo) -> Option<String> {
        match p.pid {
            100 => Some(r#""C:\nodejs\node.exe" C:\app\node_modules\vitest\vitest.mjs run"#.into()),
            101 => Some("node.exe worker.js".into()),
            200 => Some("node.exe server.js --port 3000".into()),
            _ => None,
        }
    }

    fn vitest(cpu_above: Option<f32>) -> KillEntry {
        KillEntry {
            name: "node.exe".into(),
            cmdline: Some("VITEST".into()),
            cpu_above,
            for_secs: 30,
            ..KillEntry::default()
        }
    }

    fn procs(cpu: f32) -> Vec<ProcessInfo> {
        vec![
            proc(100, 1, "node.exe", cpu),
            // A worker of the vitest run: ended with it (tree).
            proc(101, 100, "node.exe", 1.0),
            // Another Node program: left alone.
            proc(200, 1, "node.exe", 90.0),
            proc(4, 0, "System", 90.0),
        ]
    }

    #[test]
    fn command_line_patterns_contain_or_match_wildcards() {
        assert!(text_matches("vitest", "node C:\\x\\Vitest\\run"));
        assert!(!text_matches("jest", "node vitest"));
        assert!(text_matches("*--watch", "node vitest --watch"));
        assert!(!text_matches("*--watch", "node vitest --watch=false"));
        let e = vitest(None);
        let p = procs(0.0);
        assert!(e.matches(&p[0], &mut cmdlines));
        assert!(!e.matches(&p[2], &mut cmdlines));
        // No command line known: no match.
        assert!(!e.matches(&proc(300, 1, "node.exe", 0.0), &mut cmdlines));
        assert_eq!(e.label(), "node.exe running \"VITEST\"");
        assert_eq!(KillEntry { cmdline: None, ..e }.label(), "node.exe");
    }

    #[test]
    fn panic_targets_follow_the_tree_and_skip_protected_processes() {
        let p = procs(0.0);
        let entries = [
            vitest(None),
            KillEntry {
                name: "System".into(),
                ..KillEntry::default()
            },
        ];
        let t = panic_targets(&entries, &p, &mut cmdlines);
        let pids: Vec<(u32, bool)> = t.iter().map(|t| (t.process.pid, t.child)).collect();
        assert_eq!(pids, [(100, false), (101, true)]);
        let flat = KillEntry {
            tree: false,
            ..vitest(None)
        };
        assert_eq!(panic_targets(&[flat], &p, &mut cmdlines).len(), 1);
        // A child matching an entry of its own is listed once.
        let both = [
            vitest(None),
            KillEntry {
                name: "node.exe".into(),
                cmdline: Some("worker".into()),
                ..KillEntry::default()
            },
        ];
        assert_eq!(panic_targets(&both, &p, &mut cmdlines).len(), 2);
    }

    #[test]
    fn ends_a_process_that_keeps_eating_cpu_with_its_children_once() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut w = KillWatch::new(vec![vitest(Some(50.0))]);
        let mut lookups = 0;
        let mut look = |p: &ProcessInfo| {
            lookups += 1;
            cmdlines(p)
        };
        assert!(w.tick(at(0), &procs(80.0), 60.0, &mut look).is_empty());
        // It recovers in between: the clock restarts.
        assert!(w.tick(at(20), &procs(10.0), 60.0, &mut look).is_empty());
        assert!(w.tick(at(25), &procs(80.0), 60.0, &mut look).is_empty());
        assert!(w.tick(at(54), &procs(80.0), 60.0, &mut look).is_empty());
        let d = w.tick(at(55), &procs(80.0), 60.0, &mut look);
        let pids: Vec<u32> = d.iter().map(|d| d.target().pid).collect();
        assert_eq!(pids, [100, 101], "the vitest run and its worker");
        assert!(
            matches!(&d[0], Decision::Apply { action: Action::Kill, reason, .. }
            if reason == "kill list 'node.exe running \"VITEST\"': 80% CPU for 30 s")
        );
        // Once only, and the other busy node.exe is never touched.
        assert!(w.tick(at(56), &procs(80.0), 60.0, &mut look).is_empty());
        // Idle processes were never asked for their command line.
        drop(look);
        assert!(lookups > 0);

        // Gone processes are forgotten; a new run starts from zero.
        w.tick(at(57), &[], 60.0, &mut cmdlines);
        assert!(w.tick(at(58), &procs(80.0), 60.0, &mut cmdlines).is_empty());
    }

    #[test]
    fn can_wait_for_a_busy_machine_and_ignores_panic_only_entries() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut w = KillWatch::new(vec![
            KillEntry {
                system_cpu_above: Some(85.0),
                ..vitest(Some(50.0))
            },
            vitest(None),
        ]);
        assert!(w.tick(at(0), &procs(80.0), 40.0, &mut cmdlines).is_empty());
        assert!(
            w.tick(at(100), &procs(80.0), 40.0, &mut cmdlines)
                .is_empty()
        );
        assert!(
            w.tick(at(101), &procs(80.0), 90.0, &mut cmdlines)
                .is_empty()
        );
        assert_eq!(w.tick(at(131), &procs(80.0), 90.0, &mut cmdlines).len(), 2);
        // New entries reset the clocks.
        w.set_entries(vec![vitest(Some(10.0))]);
        assert!(
            w.tick(at(132), &procs(80.0), 90.0, &mut cmdlines)
                .is_empty()
        );
    }

    #[test]
    fn validates_and_describes_entries() {
        assert!(vitest(Some(50.0)).validate().is_ok());
        for bad in [
            KillEntry::default(),
            KillEntry {
                cmdline: Some(" ".into()),
                ..vitest(None)
            },
            vitest(Some(0.0)),
            vitest(Some(120.0)),
            KillEntry {
                for_secs: 0,
                ..vitest(Some(50.0))
            },
            KillEntry {
                system_cpu_above: Some(-1.0),
                ..vitest(None)
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        assert_eq!(vitest(None).describe(), "only with the panic button");
        assert_eq!(
            KillEntry {
                system_cpu_above: Some(85.0),
                ..vitest(Some(50.0))
            }
            .describe(),
            "automatically after 30 s at 50% CPU or more while the PC is above 85%"
        );
    }

    #[test]
    fn the_documented_examples_are_valid() {
        // The commented [[kill]] blocks of the default configuration.
        let lines: Vec<&str> = crate::DEFAULT_CONFIG_TOML.lines().collect();
        let start = lines.iter().position(|l| *l == "# [[kill]]").unwrap();
        let example: String = lines[start..]
            .iter()
            .take_while(|l| l.starts_with('#'))
            .map(|l| format!("{}\n", l.trim_start_matches('#').trim_start()))
            .collect();
        let c = crate::Config::from_toml(&example).unwrap();
        assert_eq!(c.kill.len(), 2);
        assert_eq!(c.kill[0].label(), "node.exe running \"vitest\"");
        assert_eq!(c.kill[1].cpu_above, None);
    }

    #[test]
    fn edits_the_kill_list_in_the_config_text() {
        let text = "# my config\ndry_run = false\n";
        let one = set_kill_entry(text, &vitest(Some(50.0)), None).unwrap();
        assert!(one.starts_with("# my config"));
        let c = crate::Config::from_toml(&one).unwrap();
        assert_eq!(c.kill, [vitest(Some(50.0))]);
        let two = set_kill_entry(&one, &vitest(None), None).unwrap();
        let replaced = set_kill_entry(
            &two,
            &KillEntry {
                name: "python.exe".into(),
                ..KillEntry::default()
            },
            Some(1),
        )
        .unwrap();
        let c = crate::Config::from_toml(&replaced).unwrap();
        assert_eq!(c.kill.len(), 2);
        assert_eq!(c.kill[1].name, "python.exe");
        assert!(set_kill_entry(text, &KillEntry::default(), None).is_err());
        assert!(set_kill_entry("kill = 1", &vitest(None), None).is_err());
        assert!(set_kill_entry("x = [", &vitest(None), None).is_err());

        let removed = remove_kill_entry(&replaced, 0).unwrap();
        assert_eq!(
            crate::Config::from_toml(&removed).unwrap().kill[0].name,
            "python.exe"
        );
        let empty = remove_kill_entry(&removed, 0).unwrap();
        assert!(!empty.contains("[[kill]]"));
        assert_eq!(remove_kill_entry(&empty, 5).unwrap(), empty);
        assert!(remove_kill_entry("x = [", 0).is_err());
    }
}

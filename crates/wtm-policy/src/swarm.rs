//! Swarms: hundreds of copies of one program, usually a script or tool that
//! keeps starting helpers and never waits for them (e.g. a status probe
//! launching `powershell.exe` every few seconds).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use wtm_core::ProcessInfo;

use crate::glob_match;

/// Programs that legitimately run in large numbers.
const NORMAL_CROWDS: [&str; 2] = ["svchost.exe", "RuntimeBroker.exe"];

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SwarmConfig {
    pub enabled: bool,
    /// Copies of one program that count as a swarm.
    pub min_count: usize,
    /// Image-name wildcards never reported.
    pub exempt: Vec<String>,
    /// End copies of a swarm that have run longer than this (minutes), with
    /// what they started; 0 leaves them alone. Respects `dry_run`.
    pub end_after_mins: u64,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_count: 150,
            exempt: Vec::new(),
            end_after_mins: 0,
        }
    }
}

impl SwarmConfig {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.min_count < 10 {
            return Err("min_count must be at least 10".into());
        }
        if self.end_after_mins > 7 * 24 * 60 {
            return Err("end_after_mins must be at most a week (10080)".into());
        }
        Ok(())
    }
}

/// Many running copies of one program.
#[derive(Debug, Clone)]
pub struct Swarm<'a> {
    /// Image name as the first copy spells it.
    pub name: String,
    pub members: Vec<&'a ProcessInfo>,
    /// Who started them: (parent name, copies), most first. Copies whose
    /// parent has exited are counted under `None`.
    pub parents: Vec<(Option<String>, usize)>,
    pub cpu_percent: f32,
    /// Private working set of all copies.
    pub memory: u64,
}

impl<'a> Swarm<'a> {
    /// Copies whose parent has exited: nothing waits for them any more.
    pub fn orphans(&self) -> usize {
        self.parents
            .iter()
            .filter(|(p, _)| p.is_none())
            .map(|(_, n)| n)
            .sum()
    }

    /// Copies that have run longer than `age` at `now`: a helper that
    /// should take seconds and is still there is stuck.
    pub fn older_than(&self, age: Duration, now: SystemTime) -> Vec<&'a ProcessInfo> {
        self.members
            .iter()
            .copied()
            .filter(|p| {
                p.start_time()
                    .and_then(|t| now.duration_since(t).ok())
                    .is_some_and(|d| d > age)
            })
            .collect()
    }

    /// e.g. "claude.exe (750), exited (378)".
    pub fn parents_text(&self) -> String {
        let parts: Vec<String> = self
            .parents
            .iter()
            .take(3)
            .map(|(p, n)| format!("{} ({n})", p.as_deref().unwrap_or("exited parent")))
            .collect();
        parts.join(", ")
    }
}

/// The parent of `p`, unless it exited (its PID may since have been reused
/// by a younger process).
pub fn live_parent<'a>(
    p: &ProcessInfo,
    by_pid: &HashMap<u32, &'a ProcessInfo>,
) -> Option<&'a ProcessInfo> {
    by_pid
        .get(&p.parent_pid)
        .copied()
        .filter(|parent| parent.pid != p.pid && parent.create_time <= p.create_time)
}

/// An image name that hashes and compares like its `to_lowercase()`, without
/// allocating one for ASCII names (nearly all of them).
#[derive(Debug)]
struct NameKey<'a>(Cow<'a, str>);

impl<'a> NameKey<'a> {
    fn new(name: &'a str) -> Self {
        Self(if name.is_ascii() {
            Cow::Borrowed(name)
        } else {
            Cow::Owned(name.to_lowercase())
        })
    }
}

impl PartialEq for NameKey<'_> {
    // Lowercased names hold no ASCII capitals, so ignoring ASCII case on
    // both sides compares the lowercase forms.
    fn eq(&self, other: &Self) -> bool {
        self.0.as_bytes().eq_ignore_ascii_case(other.0.as_bytes())
    }
}

impl Eq for NameKey<'_> {}

impl Hash for NameKey<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Equal keys have equal lengths, so they are fed in the same chunks.
        let mut buf = [0u8; 64];
        for chunk in self.0.as_bytes().chunks(buf.len()) {
            let lower = &mut buf[..chunk.len()];
            lower.copy_from_slice(chunk);
            lower.make_ascii_lowercase();
            state.write(lower);
        }
        state.write_u8(0xff);
    }
}

/// Programs with at least `cfg.min_count` running copies, largest first.
pub fn find_swarms<'a>(procs: &'a [ProcessInfo], cfg: &SwarmConfig) -> Vec<Swarm<'a>> {
    if !cfg.enabled {
        return Vec::new();
    }
    let mut groups: HashMap<NameKey, Vec<&ProcessInfo>> = HashMap::new();
    for p in procs.iter().filter(|p| p.pid > 4) {
        groups.entry(NameKey::new(&p.name)).or_default().push(p);
    }
    let by_pid: HashMap<u32, &ProcessInfo> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut swarms: Vec<Swarm<'a>> = groups
        .into_values()
        .filter(|m| m.len() >= cfg.min_count)
        .filter(|m| {
            let name = &m[0].name;
            !NORMAL_CROWDS.iter().any(|n| n.eq_ignore_ascii_case(name))
                && !cfg.exempt.iter().any(|e| glob_match(e, name))
                && wtm_core::protection_reason(m[0].pid, name).is_none()
        })
        .map(|members| {
            // Borrowed names while counting; one copy per distinct parent.
            let mut parents: HashMap<Option<&str>, usize> = HashMap::new();
            for p in &members {
                let parent = live_parent(p, &by_pid).map(|q| q.name.as_str());
                *parents.entry(parent).or_default() += 1;
            }
            let mut parents: Vec<(Option<String>, usize)> = parents
                .into_iter()
                .map(|(p, n)| (p.map(str::to_owned), n))
                .collect();
            parents.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            Swarm {
                name: members[0].name.clone(),
                cpu_percent: members.iter().map(|p| p.cpu_percent).sum(),
                memory: members.iter().map(|p| p.private_working_set).sum(),
                parents,
                members,
            }
        })
        .collect();
    swarms.sort_by_key(|s| std::cmp::Reverse(s.members.len()));
    swarms
}

/// Reports each swarm once, again only after it shrank below half the
/// threshold.
#[derive(Debug, Default)]
pub struct SwarmWatch {
    reported: HashSet<String>,
}

impl SwarmWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// The swarms to report now: (name, copies, parents text).
    pub fn tick(
        &mut self,
        procs: &[ProcessInfo],
        cfg: &SwarmConfig,
    ) -> Vec<(String, usize, String)> {
        let swarms = find_swarms(procs, cfg);
        // Only the few already reported names need counting.
        if !self.reported.is_empty() {
            let rearm = (cfg.min_count / 2).max(1);
            let mut counts: HashMap<NameKey, usize> = HashMap::new();
            for p in procs {
                *counts.entry(NameKey::new(&p.name)).or_default() += 1;
            }
            self.reported
                .retain(|name| counts.get(&NameKey::new(name)).is_some_and(|n| *n >= rearm));
        }
        swarms
            .into_iter()
            .filter(|s| self.reported.insert(s.name.to_lowercase()))
            .map(|s| (s.name.clone(), s.members.len(), s.parents_text()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn proc(pid: u32, parent: u32, name: &str, create: i64) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: parent,
            name: name.into(),
            cpu_percent: 0.5,
            cpu_time: Duration::ZERO,
            working_set: 0,
            private_working_set: 1 << 20,
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
            suspended: false,
            create_time: create,
        }
    }

    /// claude.exe (10) starts 30 probes, 20 more lost their parent, a young
    /// process reuses PID 11 of a parent that exited; 40 svchost.exe.
    fn procs() -> Vec<ProcessInfo> {
        let mut v = vec![proc(10, 1, "claude.exe", 100), proc(11, 1, "late.exe", 900)];
        for i in 0..30 {
            v.push(proc(1000 + i, 10, "powershell.exe", 200));
        }
        for i in 0..10 {
            v.push(proc(2000 + i, 99, "PowerShell.exe", 200));
            v.push(proc(3000 + i, 11, "powershell.exe", 200));
        }
        for i in 0..40 {
            v.push(proc(5000 + i, 1, "svchost.exe", 50));
        }
        v.push(proc(6000, 10, "node.exe", 300));
        v
    }

    fn cfg() -> SwarmConfig {
        SwarmConfig {
            min_count: 25,
            ..SwarmConfig::default()
        }
    }

    #[test]
    fn swarms_are_found_with_their_parents() {
        let procs = procs();
        let swarms = find_swarms(&procs, &cfg());
        assert_eq!(swarms.len(), 1, "svchost.exe is a normal crowd");
        let s = &swarms[0];
        assert_eq!(s.name, "powershell.exe");
        assert_eq!(s.members.len(), 50);
        assert_eq!(s.parents, [(Some("claude.exe".to_owned()), 30), (None, 20)]);
        assert_eq!(s.orphans(), 20);
        assert_eq!(s.parents_text(), "claude.exe (30), exited parent (20)");
        assert_eq!(s.memory, 50 << 20);
        assert!((s.cpu_percent - 25.0).abs() < 0.01);

        let exempt = SwarmConfig {
            exempt: vec!["power*".into()],
            ..cfg()
        };
        assert!(find_swarms(&procs, &exempt).is_empty());
        let off = SwarmConfig {
            enabled: false,
            ..cfg()
        };
        assert!(find_swarms(&procs, &off).is_empty());
    }

    #[test]
    fn names_group_like_their_lowercase_forms() {
        // The Kelvin sign lowercases to an ASCII `k`.
        let mut procs: Vec<ProcessInfo> = (0..30)
            .map(|i| {
                proc(
                    1000 + i,
                    1,
                    ["k.exe", "K.EXE", "\u{212A}.exe"][i as usize % 3],
                    1,
                )
            })
            .collect();
        procs.extend((0..30).map(|i| proc(2000 + i, 1, ["ÇAY.exe", "çay.EXE"][i as usize % 2], 1)));
        let mut swarms = find_swarms(&procs, &cfg());
        swarms.sort_by(|a, b| a.name.cmp(&b.name));
        let found: Vec<(&str, usize)> = swarms
            .iter()
            .map(|s| (s.name.as_str(), s.members.len()))
            .collect();
        assert_eq!(found, [("k.exe", 30), ("ÇAY.exe", 30)]);

        let mut watch = SwarmWatch::new();
        assert_eq!(watch.tick(&procs, &cfg()).len(), 2);
        assert!(watch.tick(&procs, &cfg()).is_empty(), "still reported");
    }

    #[test]
    fn a_swarm_is_reported_once_until_it_shrinks() {
        let mut procs = procs();
        let mut watch = SwarmWatch::new();
        let first = watch.tick(&procs, &cfg());
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].0, "powershell.exe");
        assert_eq!(first[0].1, 50);
        assert!(watch.tick(&procs, &cfg()).is_empty());
        // Down to 15 (above half of 25): still quiet when it grows again.
        procs.retain(|p| !(p.name.eq_ignore_ascii_case("powershell.exe") && p.pid >= 1015));
        assert!(watch.tick(&procs, &cfg()).is_empty());
        procs.retain(|p| !(p.name.eq_ignore_ascii_case("powershell.exe") && p.pid >= 1005));
        assert!(watch.tick(&procs, &cfg()).is_empty());
        let grown = self::procs();
        assert_eq!(watch.tick(&grown, &cfg()).len(), 1, "reported again");
    }

    #[test]
    fn stuck_copies_are_those_older_than_the_limit() {
        let mut procs = procs();
        // FILETIME of 2026-01-01 plus i minutes for the first ten probes.
        let base = 116_444_736_000_000_000 + 1_767_225_600 * 10_000_000;
        for (i, p) in procs
            .iter_mut()
            .filter(|p| p.pid >= 1000 && p.pid < 1010)
            .enumerate()
        {
            p.create_time = base + i as i64 * 60 * 10_000_000;
        }
        let swarms = find_swarms(&procs, &cfg());
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600 + 10 * 60);
        // Started 0..=9 minutes after base; at base+10 min, 5 are older than 5 min.
        let old = swarms[0].older_than(Duration::from_secs(5 * 60), now);
        assert_eq!(
            old.len(),
            5,
            "{:?}",
            old.iter().map(|p| p.pid).collect::<Vec<_>>()
        );
        assert!(old.iter().all(|p| p.pid < 1005));
        // Started 1..=9 minutes before now; the rest have no known start time
        // and are never treated as stuck.
        let all = swarms[0].older_than(Duration::from_secs(60), now);
        assert_eq!(all.len(), 9);
    }

    #[test]
    fn config_is_validated() {
        assert!(SwarmConfig::default().validate().is_ok());
        let bad = SwarmConfig {
            min_count: 3,
            ..SwarmConfig::default()
        };
        assert!(bad.validate().is_err());
        let too_long = SwarmConfig {
            end_after_mins: 20_000,
            ..SwarmConfig::default()
        };
        assert!(too_long.validate().is_err());
    }
}

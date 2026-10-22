//! Timing checks for the per-tick paths with a synthetic machine: 2000
//! processes against 50 rules and every other policy. Ignored by default;
//! run with `cargo test -p wtm-policy --release bench -- --ignored --nocapture`.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use wtm_core::{ProcessInfo, Snapshot, SystemStats};

use crate::{
    Action, AppLimit, Condition, Detail, Finding, FindingKind, GameMode, GameModeConfig, Governor,
    GovernorConfig, InsightPolicy, InsightsConfig, KillEntry, KillWatch, LimitPlanner, LimitScope,
    Matcher, MemoryGuard, MemoryGuardConfig, Rule, RuleEngine, SwarmConfig, SwarmWatch,
    find_swarms,
};

const PROCS: usize = 2000;
const RULES: usize = 50;
const ROUNDS: u32 = 50;

const NAMES: [&str; 24] = [
    "chrome.exe",
    "msedge.exe",
    "Code.exe",
    "svchost.exe",
    "RuntimeBroker.exe",
    "MsMpEng.exe",
    "node.exe",
    "conhost.exe",
    "explorer.exe",
    "SearchHost.exe",
    "Teams.exe",
    "OneDrive.exe",
    "powershell.exe",
    "WmiPrvSE.exe",
    "dllhost.exe",
    "steam.exe",
    "Discord.exe",
    "firefox.exe",
    "rust-analyzer.exe",
    "cargo.exe",
    "ShellExperienceHost.exe",
    "SecurityHealthService.exe",
    "audiodg.exe",
    "spoolsv.exe",
];

fn machine() -> Vec<ProcessInfo> {
    (0..PROCS)
        .map(|i| {
            let pid = 100 + i as u32 * 4;
            // A forest a few levels deep: every process but the first few
            // was started by an earlier one.
            let parent_pid = if i < 8 { 4 } else { 100 + (i as u32 / 3) * 4 };
            ProcessInfo {
                pid,
                parent_pid,
                name: NAMES[i % NAMES.len()].into(),
                cpu_percent: (i % 17) as f32 * 0.7,
                cpu_time: Duration::ZERO,
                working_set: (i as u64 % 40) << 24,
                private_working_set: (i as u64 % 40) << 23,
                private_bytes: (i as u64 % 40) << 24,
                thread_count: 10,
                handle_count: 200,
                session_id: 1,
                base_priority: 8,
                gpu_percent: 0.0,
                gpu_memory: 0,
                io_read_rate: 0.0,
                io_write_rate: 0.0,
                net_rx_rate: 0.0,
                net_tx_rate: 0.0,
                suspended: false,
                create_time: i as i64 * 10,
            }
        })
        .collect()
}

/// A mix of literal names, prefix and suffix wildcards, paths and command
/// lines; a few match running processes but never fire, so they are
/// evaluated on every tick.
fn rules() -> Vec<Rule> {
    (0..RULES)
        .map(|i| {
            let mut matcher = Matcher::default();
            let mut when = None;
            match i % 5 {
                0 => matcher.name = Some(format!("app{i}.exe")),
                1 => matcher.name = Some(format!("tool{i}*")),
                2 => matcher.name = Some(format!("*helper{i}.exe")),
                3 => {
                    matcher.name = Some(NAMES[i % NAMES.len()].to_uppercase());
                    when = Some(Condition {
                        cpu_above: Some(99.0),
                        ..Default::default()
                    });
                }
                _ => {
                    matcher.path = Some(format!("c:\\program files\\vendor{i}\\*\\*.exe"));
                    if i % 10 == 9 {
                        matcher.name = Some("node.exe".into());
                        matcher.cmdline = Some(format!("script{i}"));
                    }
                }
            }
            Rule {
                name: format!("rule {i}"),
                matcher,
                when,
                actions: vec![Action::EcoQos],
            }
        })
        .collect()
}

/// Prints the fastest of `ROUNDS` runs, which is the least disturbed by
/// whatever else the machine is doing.
fn time(label: &str, mut f: impl FnMut()) {
    f();
    let best = (0..ROUNDS)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .min()
        .unwrap_or_default();
    println!("{label:<24} {:>9.1} us/tick", best.as_secs_f64() * 1e6);
}

#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn per_tick_costs() {
    let procs = machine();
    let paths: HashMap<u32, String> = procs
        .iter()
        .map(|p| (p.pid, format!("C:\\Program Files\\App\\{}", p.name)))
        .collect();
    let lines: HashMap<u32, String> = procs
        .iter()
        .map(|p| {
            (
                p.pid,
                format!("\"C:\\x\\{}\" --type=renderer --lang=en-US", p.name),
            )
        })
        .collect();
    let mut lookup = |p: &ProcessInfo, d: Detail| match d {
        Detail::Path => paths.get(&p.pid).cloned(),
        Detail::CommandLine => lines.get(&p.pid).cloned(),
    };
    let now = Instant::now();

    let mut engine = RuleEngine::new(rules());
    time("rules", || {
        std::hint::black_box(engine.tick(now, &procs, &mut lookup));
    });

    let kill: Vec<KillEntry> = (0..10)
        .map(|i| KillEntry {
            name: NAMES[i].into(),
            cmdline: (i % 2 == 0).then(|| format!("vitest{i}")),
            cpu_above: Some(5.0),
            for_secs: 600,
            ..Default::default()
        })
        .collect();
    let mut watch = KillWatch::new(kill);
    time("kill watch", || {
        std::hint::black_box(watch.tick(now, &procs, 50.0, &mut |p| lines.get(&p.pid).cloned()));
    });

    let limits: Vec<AppLimit> = (0..5)
        .map(|i| AppLimit {
            name: format!("limited{i}.exe"),
            cpu_cores: Some(2.0),
            memory_gb: None,
            scope: LimitScope::Group,
        })
        .collect();
    let mut planner = LimitPlanner::new(limits);
    time("limits plan+usage", || {
        std::hint::black_box(planner.plan(&procs));
        std::hint::black_box(planner.usage(&procs));
    });

    let swarm = SwarmConfig {
        min_count: 50,
        ..Default::default()
    };
    time("find_swarms", || {
        std::hint::black_box(find_swarms(&procs, &swarm));
    });
    let mut swarm_watch = SwarmWatch::new();
    time("swarm watch", || {
        std::hint::black_box(swarm_watch.tick(&procs, &swarm));
    });

    let insights_cfg = InsightsConfig::default();
    let mut insights = InsightPolicy::new();
    time("insights (none flagged)", || {
        std::hint::black_box(insights.tick(&procs, &insights_cfg));
    });
    let mut flagged = InsightPolicy::new();
    flagged.update(
        vec![Finding {
            app: "Teams.exe".into(),
            kind: FindingKind::ChronicCpu,
            detail: String::new(),
        }],
        &insights_cfg,
    );
    time("insights (one flagged)", || {
        std::hint::black_box(flagged.tick(&procs, &insights_cfg));
    });

    let snap = Snapshot {
        interval: Some(Duration::from_secs(1)),
        system: SystemStats {
            cpu_percent: 99.0,
            memory_total: 16 << 30,
            memory_available: 1 << 30,
            ..Default::default()
        },
        processes: procs.clone(),
        net_traced: false,
        taken_at_ms: 0,
    };
    let mut governor = Governor::new(GovernorConfig {
        sustain_secs: 0,
        exempt: vec!["steam*.exe".into(), "discord.exe".into()],
        ..GovernorConfig::default()
    });
    let mut tick = 0;
    time("governor (overloaded)", || {
        tick += 1;
        std::hint::black_box(governor.tick(now + Duration::from_secs(tick * 60), &snap));
    });
    let mut guard = MemoryGuard::new(MemoryGuardConfig {
        sustain_secs: 0,
        exempt: vec!["code.exe".into()],
        ..MemoryGuardConfig::default()
    });
    time("memory guard", || {
        std::hint::black_box(guard.tick(now, &snap));
    });

    let mut game = GameMode::new(GameModeConfig::default());
    let keys: HashSet<(u32, i64)> = HashSet::from([(procs[0].pid, procs[0].create_time)]);
    time("game mode", || {
        std::hint::black_box(game.tick(Some(("game.exe".into(), keys.clone())), &procs));
    });
}

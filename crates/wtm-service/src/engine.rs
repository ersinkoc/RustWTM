//! The sampling / decision / enforcement loop shared by service and console mode.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context;
use tracing::{error, info, warn};
use wtm_core::control::{self, ControlResult, PriorityClass, ProcessRef};
use wtm_core::{ProcessInfo, Sampler, Snapshot, StartWatcher};
use wtm_ipc::{Command, Outcome, Status};
use wtm_policy::{
    Action, Config, DEFAULT_CONFIG_TOML, Decision, Governor, InsightPolicy, RuleEngine, Source,
    analyze,
};

use crate::executor::Executor;
use crate::history::{self, History};
use crate::ipc_server::{self, EngineMsg, Shared};

#[derive(Debug, Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub logs: PathBuf,
    pub history: PathBuf,
}

impl Paths {
    pub fn under(data_dir: &Path) -> Self {
        Self {
            config: data_dir.join("config.toml"),
            logs: data_dir.join("logs"),
            history: data_dir.join("history.sqlite"),
        }
    }
}

/// `%ProgramData%\RustWTM`.
pub fn default_data_dir() -> PathBuf {
    std::env::var_os("ProgramData")
        .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from)
        .join("RustWTM")
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn load_config(path: &Path) -> anyhow::Result<Config> {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, DEFAULT_CONFIG_TOML)
            .with_context(|| format!("writing default config to {}", path.display()))?;
        info!(path = %path.display(), "wrote default config");
    }
    match Config::load(path) {
        Ok(c) => Ok(c),
        Err(e) => {
            error!(error = %e, "config invalid; falling back to defaults (dry-run)");
            Ok(Config::default())
        }
    }
}

fn status(config: &Config, paths: &Paths, executor: &Executor) -> Status {
    Status {
        dry_run: config.dry_run,
        governor_enabled: config.governor.enabled,
        high_cpu_percent: config.governor.high_cpu_percent,
        recover_cpu_percent: config.governor.recover_cpu_percent,
        rules: config.rules.len(),
        config_path: paths.config.display().to_string(),
        managed: executor.managed(),
    }
}

/// Runs until [`EngineMsg::Stop`] arrives (or all senders are dropped), then
/// rolls back every applied limit. `tx` is handed to the IPC server.
pub fn run(paths: &Paths, tx: Sender<EngineMsg>, rx: &Receiver<EngineMsg>) -> anyhow::Result<()> {
    let mut config = load_config(&paths.config)?;
    let mut config_mtime = modified(&paths.config);

    if !control::enable_debug_privilege() {
        warn!("SeDebugPrivilege unavailable; elevated and service processes cannot be controlled");
    }
    if let Err(e) = control::set_own_priority(PriorityClass::High) {
        warn!(error = %e, "could not raise own priority");
    }

    let shared = Arc::new(Shared::default());
    let watcher_stop = Arc::new(AtomicBool::new(false));
    let watcher = spawn_watcher(tx.clone(), watcher_stop.clone());
    let server = match ipc_server::start(shared.clone(), tx) {
        Ok(s) => Some(s),
        Err(e) => {
            error!(error = %e, "IPC disabled");
            None
        }
    };

    let mut sampler = Sampler::new();
    let mut governor = Governor::new(config.governor.clone());
    let mut rules = RuleEngine::new(config.rules.clone());
    let mut executor = Executor::new(config.dry_run, shared.events.clone());
    let mut history = match History::open(&paths.history) {
        Ok(h) => {
            *shared
                .history_path
                .write()
                .unwrap_or_else(PoisonError::into_inner) = Some(paths.history.clone());
            Some(h)
        }
        Err(e) => {
            shared
                .events
                .warn(None, format!("usage history disabled: {e}"));
            None
        }
    };
    let mut insights = InsightPolicy::new();
    let mut next_insights = Instant::now() + Duration::from_secs(5);
    let mut next_prune = Instant::now();
    let mut paths_cache = PathCache::new();
    let mut last_snapshot: Option<Arc<Snapshot>> = None;

    shared.events.info(
        None,
        format!(
            "engine started (dry_run={}, governor={}, rules={})",
            config.dry_run,
            config.governor.enabled,
            config.rules.len()
        ),
    );

    let mut next_tick = Instant::now();
    'run: loop {
        // Serve control requests until the next sampling tick is due.
        loop {
            match rx.recv_timeout(next_tick.saturating_duration_since(Instant::now())) {
                Ok(EngineMsg::Stop) | Err(RecvTimeoutError::Disconnected) => break 'run,
                Ok(EngineMsg::Started(started)) => {
                    let decisions = rules.on_started(Instant::now(), &started, &mut |p| {
                        cached_path(&mut paths_cache, p)
                    });
                    for d in &decisions {
                        let _ = executor.execute(d);
                    }
                }
                Ok(EngineMsg::Control {
                    target,
                    command,
                    reply,
                }) => {
                    let procs = last_snapshot.as_deref().map_or(&[][..], |s| &s.processes);
                    let outcomes = handle_control(&mut executor, procs, &shared, &target, command);
                    let _ = reply.send(outcomes);
                    *shared
                        .status
                        .write()
                        .unwrap_or_else(PoisonError::into_inner) =
                        status(&config, paths, &executor);
                }
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        next_tick = Instant::now() + Duration::from_millis(config.sample_interval_ms);

        let mtime = modified(&paths.config);
        if mtime != config_mtime {
            config_mtime = mtime;
            match Config::load(&paths.config) {
                Ok(new) => {
                    governor.set_config(new.governor.clone());
                    rules.set_rules(new.rules.clone());
                    executor.set_dry_run(new.dry_run);
                    shared.events.info(
                        None,
                        format!(
                            "config reloaded (dry_run={}, rules={})",
                            new.dry_run,
                            new.rules.len()
                        ),
                    );
                    config = new;
                }
                Err(e) => shared
                    .events
                    .warn(None, format!("config reload failed, keeping previous: {e}")),
            }
        }

        let snap = match sampler.sample() {
            Ok(s) => Arc::new(s),
            Err(e) => {
                warn!(error = %e, "sampling failed");
                continue;
            }
        };
        executor.forget_exited(&snap);
        if let Some(h) = history.as_mut() {
            let unix = unix_now();
            if let Err(e) = h.record(&snap, unix) {
                warn!(error = %e, "history write failed");
            }
            if Instant::now() >= next_prune {
                next_prune = Instant::now() + Duration::from_secs(3600);
                let _ = h.prune(config.history_days, unix);
            }
            if Instant::now() >= next_insights {
                next_insights = Instant::now()
                    + Duration::from_secs(u64::from(config.insights.evaluate_every_minutes) * 60);
                evaluate_insights(
                    h,
                    &config,
                    &snap,
                    unix,
                    &mut insights,
                    &mut executor,
                    &shared,
                );
            }
        }
        paths_cache.retain(|&(pid, created), _| {
            snap.processes
                .iter()
                .any(|p| p.pid == pid && p.create_time == created)
        });

        let now = Instant::now();
        let mut decisions = governor.tick(now, &snap);
        if config.insights.enabled {
            decisions.extend(insights.tick(&snap.processes, &config.insights));
        }
        decisions.extend(rules.tick(now, &snap.processes, &mut |p| {
            cached_path(&mut paths_cache, p)
        }));
        for d in &decisions {
            let _ = executor.execute(d);
        }

        *shared
            .snapshot
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(snap.clone());
        *shared
            .status
            .write()
            .unwrap_or_else(PoisonError::into_inner) = status(&config, paths, &executor);
        last_snapshot = Some(snap);
    }

    shared
        .events
        .info(None, "stopping; restoring all processes");
    executor.restore_all("service stopping");
    if let Some(h) = history.as_mut() {
        let _ = h.flush();
    }
    watcher_stop.store(true, Ordering::SeqCst);
    let _ = watcher.join();
    if let Some(s) = server {
        s.shutdown();
    }
    Ok(())
}

fn handle_control(
    executor: &mut Executor,
    procs: &[wtm_core::ProcessInfo],
    shared: &Shared,
    target: &ProcessRef,
    command: Command,
) -> Vec<Outcome> {
    let outcome = |t: &ProcessRef, r: ControlResult<()>| Outcome {
        target: t.clone(),
        result: r.map_err(|e| e.to_string()),
    };
    let manual = |action| Decision::Apply {
        source: Source::Manual,
        target: target.clone(),
        action,
        reason: "manual".into(),
    };

    let outcomes = match command {
        Command::Kill => vec![outcome(target, control::kill(target))],
        Command::KillTree => control::kill_tree(target, procs)
            .into_iter()
            .map(|(t, r)| outcome(&t, r))
            .collect(),
        Command::Suspend => vec![outcome(target, control::suspend(target))],
        Command::Resume => vec![outcome(target, control::resume(target))],
        Command::Priority(p) => vec![outcome(target, control::set_priority(target, p))],
        Command::Affinity(mask) => vec![outcome(target, control::set_affinity(target, mask))],
        Command::EcoQos(mode) => vec![outcome(target, control::set_eco_qos(target, mode))],
        // Limits and unlimits go through the executor so they compose with
        // governor/rule limits and survive their rollbacks.
        Command::Limit(limits) => {
            let mut result = executor.execute(&Decision::Restore {
                source: Source::Manual,
                target: target.clone(),
                reason: "replaced by new manual limit".into(),
            });
            if let Some(c) = limits.cpu_percent {
                result = result.and(executor.execute(&manual(Action::CpuCap(c))));
            }
            if let Some(b) = limits.memory_bytes {
                let mb = b.div_ceil(1 << 20);
                result = result.and(executor.execute(&manual(Action::MemoryCapMb(mb))));
            }
            return vec![Outcome {
                target: target.clone(),
                result,
            }];
        }
        Command::Unlimit => {
            return vec![Outcome {
                target: target.clone(),
                result: executor.execute(&Decision::Restore {
                    source: Source::Manual,
                    target: target.clone(),
                    reason: "manual".into(),
                }),
            }];
        }
    };

    for o in &outcomes {
        match &o.result {
            Ok(()) => shared
                .events
                .info(Some(&o.target), format!("Manual: {command:?}")),
            Err(e) => shared
                .events
                .warn(Some(&o.target), format!("Manual: {command:?} failed: {e}")),
        }
    }
    outcomes
}

type PathCache = HashMap<(u32, i64), Option<String>>;

fn cached_path(cache: &mut PathCache, p: &ProcessInfo) -> Option<String> {
    cache
        .entry((p.pid, p.create_time))
        .or_insert_with(|| control::image_path(&ProcessRef::from(p)).ok())
        .clone()
}

const WATCH_INTERVAL: Duration = Duration::from_millis(250);

/// Polls for new processes between full samples so unconditional rules
/// (e.g. `kill`) act within a fraction of a second.
fn spawn_watcher(tx: Sender<EngineMsg>, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("start-watcher".into())
        .spawn(move || {
            let mut watcher = StartWatcher::new();
            let mut warned = false;
            while !stop.load(Ordering::SeqCst) {
                match watcher.poll() {
                    Ok(started) if !started.is_empty() => {
                        if tx.send(EngineMsg::Started(started)).is_err() {
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(e) if !warned => {
                        warn!(error = %e, "start watcher poll failed");
                        warned = true;
                    }
                    Err(_) => {}
                }
                std::thread::sleep(WATCH_INTERVAL);
            }
        })
        .expect("spawn start-watcher thread")
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Analyses the history window, raises new findings as notable events and
/// rolls back actions for findings that cleared.
fn evaluate_insights(
    history: &History,
    config: &Config,
    snap: &Snapshot,
    unix: i64,
    insights: &mut InsightPolicy,
    executor: &mut Executor,
    shared: &Shared,
) {
    let findings = if config.insights.enabled {
        let since = unix.div_euclid(60) - i64::from(config.insights.window_hours) * 60;
        match history::stats(history.connection(), since, snap.system.logical_cpus) {
            Ok(stats) => analyze(&stats, &config.insights),
            Err(e) => {
                warn!(error = %e, "history query failed");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let (raised, restores) = insights.update(findings, &config.insights);
    for f in raised {
        shared
            .events
            .notable(None, format!("{}: {:?} - {}", f.app, f.kind, f.detail));
    }
    for d in &restores {
        let _ = executor.execute(d);
    }
    *shared
        .findings
        .write()
        .unwrap_or_else(PoisonError::into_inner) = insights.findings();
}

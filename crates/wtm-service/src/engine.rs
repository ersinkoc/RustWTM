//! The sampling / decision / enforcement loop shared by service and console mode.

use std::collections::{HashMap, HashSet};
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
    Action, Config, DEFAULT_CONFIG_TOML, Decision, GameChange, GameMode, Governor, InsightPolicy,
    RuleEngine, Source, analyze,
};

use crate::app_limits::AppLimiter;
use crate::blocker::Blocker;
use crate::events::EventLog;
use crate::executor::Executor;
use crate::history::{self, History};
use crate::ipc_server::{self, EngineMsg, Shared};
use crate::rescue::Rescue;

#[derive(Debug, Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub logs: PathBuf,
    pub history: PathBuf,
    /// Telegram bot settings (holds the bot token).
    pub telegram: PathBuf,
    /// The power plan game mode replaced, while it is replaced.
    pub power_plan: PathBuf,
    /// Named pipe served for clients (overridden in tests).
    pub pipe: String,
    /// Where blocked programs are registered.
    pub block_root: BlockRoot,
    /// Command run instead of a blocked program; `None` disables blocking.
    pub block_debugger: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockRoot {
    /// The real Image File Execution Options key (needs elevation).
    System,
    /// A scratch key under HKCU (tests).
    #[cfg_attr(not(test), expect(dead_code))]
    User(String),
}

impl Paths {
    pub fn under(data_dir: &Path) -> Self {
        Self {
            config: data_dir.join("config.toml"),
            logs: data_dir.join("logs"),
            history: data_dir.join("history.sqlite"),
            telegram: data_dir.join("telegram.toml"),
            power_plan: data_dir.join("power-plan.txt"),
            pipe: wtm_ipc::PIPE_NAME.to_owned(),
            block_root: BlockRoot::System,
            block_debugger: block_debugger(),
        }
    }
}

/// `%ProgramData%\RustWTM`.
pub fn default_data_dir() -> PathBuf {
    std::env::var_os("ProgramData")
        .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from)
        .join("RustWTM")
}

/// The command Windows runs instead of a blocked program: the GUI next to
/// this executable, which tells the user and exits.
fn block_debugger() -> Option<String> {
    let gui = std::env::current_exe().ok()?.with_file_name("rustwtm.exe");
    gui.exists()
        .then(|| format!("\"{}\" --blocked", gui.display()))
}

fn open_blocker(root: &BlockRoot, debugger: Option<String>, events: &EventLog) -> Option<Blocker> {
    let Some(debugger) = debugger else {
        events.warn(
            None,
            "program blocking disabled: rustwtm.exe not found next to the service",
        );
        return None;
    };
    let opened = match root {
        BlockRoot::System => Blocker::system(debugger),
        BlockRoot::User(path) => Blocker::at(
            winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER),
            path,
            debugger,
        ),
    };
    opened
        .inspect_err(|e| events.warn(None, format!("program blocking disabled: {e}")))
        .ok()
}

/// Applies the block list (none in dry-run) and logs what changed.
fn sync_blocks(blocker: Option<&Blocker>, config: &Config, events: &EventLog) {
    let Some(blocker) = blocker else { return };
    let wanted: &[String] = if config.dry_run { &[] } else { &config.block };
    if config.dry_run && !config.block.is_empty() {
        events.info(
            None,
            format!("dry-run: would block {}", config.block.join(", ")),
        );
    }
    match blocker.sync(wanted) {
        Ok(r) => {
            for name in r.added {
                events.info(None, format!("blocked from starting: {name}"));
            }
            for name in r.removed {
                events.info(None, format!("no longer blocked: {name}"));
            }
            for name in r.conflicts {
                events.warn(
                    None,
                    format!("cannot block {name}: another program set its debugger"),
                );
            }
        }
        Err(e) => events.warn(None, format!("updating blocked programs failed: {e}")),
    }
}

/// Reports older than this are ignored (the GUI was closed).
const FOREGROUND_TTL: Duration = Duration::from_secs(5);

/// The foreground process and everything it started, plus its name.
fn foreground_keys(
    report: Option<(u32, Instant)>,
    procs: &[ProcessInfo],
) -> (HashSet<(u32, i64)>, Option<String>) {
    let Some(p) = report
        .filter(|(_, at)| at.elapsed() < FOREGROUND_TTL)
        .and_then(|(pid, _)| procs.iter().find(|p| p.pid == pid))
    else {
        return Default::default();
    };
    let mut keys: HashSet<_> = wtm_core::descendants(p.pid, p.create_time, procs)
        .iter()
        .map(|d| (d.pid, d.create_time))
        .collect();
    keys.insert((p.pid, p.create_time));
    (keys, Some(p.name.clone()))
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The configuration, and why the file could not be used when the
/// built-in defaults stand in for it.
fn load_config(path: &Path) -> anyhow::Result<(Config, Option<String>)> {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, DEFAULT_CONFIG_TOML)
            .with_context(|| format!("writing default config to {}", path.display()))?;
        info!(path = %path.display(), "wrote default config");
    }
    match Config::load(path) {
        Ok(c) => Ok((c, None)),
        Err(e) => {
            error!(error = %e, "config invalid; falling back to defaults (dry-run)");
            Ok((Config::default(), Some(e.to_string())))
        }
    }
}

fn status(
    config: &Config,
    paths: &Paths,
    executor: &Executor,
    app_limits: &[wtm_policy::GroupUsage],
    foreground: Option<&str>,
    rescue: &Rescue,
    game: &GameMode,
) -> Status {
    Status {
        dry_run: config.dry_run,
        governor_enabled: config.governor.enabled,
        high_cpu_percent: config.governor.high_cpu_percent,
        recover_cpu_percent: config.governor.recover_cpu_percent,
        rules: config.rules.len(),
        config_path: paths.config.display().to_string(),
        managed: executor.managed(),
        app_limits: app_limits.to_vec(),
        foreground: foreground.map(str::to_owned),
        rescue_core: rescue.summary(),
        game: game.game().map(|name| wtm_ipc::GameStatus {
            name: name.to_owned(),
            calmed: game.calmed(),
        }),
        config_error: None,
    }
}

/// The per-file disk trace stops this long after the last request.
const DISK_TRACE_IDLE: Duration = Duration::from_secs(30);

/// The hardware sensor helper stops this long after the last request,
/// unless a fan profile is active.
const HARDWARE_IDLE: Duration = Duration::from_secs(30);

/// Prefix of the ETW session that counts network bytes per process.
const NET_TRACE: &str = "RustWTM-Network";
/// How often trace sessions left by ended processes are cleared: Windows
/// allows 64 in all.
const TRACE_SWEEP_EVERY: Duration = Duration::from_secs(600);

/// Stops the trace sessions of RustWTM processes that are gone: earlier
/// service runs and windows closed without stopping theirs.
fn stop_trace_orphans() {
    for prefix in [
        NET_TRACE,
        crate::ipc_server::DISK_TRACE,
        wtm_core::GUI_DISK_TRACE,
        wtm_core::GUI_NET_TRACE,
    ] {
        wtm_sys::etw::stop_orphans(prefix);
    }
}

/// Runs until [`EngineMsg::Stop`] arrives (or all senders are dropped), then
/// rolls back every applied limit. `tx` is handed to the IPC server.
pub fn run(paths: &Paths, tx: Sender<EngineMsg>, rx: &Receiver<EngineMsg>) -> anyhow::Result<()> {
    let (mut config, mut config_error) = load_config(&paths.config)?;
    let mut config_mtime = modified(&paths.config);

    if !control::enable_debug_privilege() {
        warn!("SeDebugPrivilege unavailable; elevated and service processes cannot be controlled");
    }
    let floor = usize::try_from(config.reserve.working_set_mb << 20).unwrap_or(0);
    for failure in control::harden_self(PriorityClass::High, floor) {
        warn!(%failure, "could not reserve resources for the service");
    }

    let shared = Arc::new(Shared::default());
    if let Some(e) = &config_error {
        shared.events.warn(
            None,
            format!("configuration invalid, running on defaults (dry run): {e}"),
        );
    }
    shared.hardware.dry_run(config.dry_run);
    *shared
        .config
        .write()
        .unwrap_or_else(PoisonError::into_inner) = config.clone();
    let watcher_stop = Arc::new(AtomicBool::new(false));
    let watch_starts = Arc::new(AtomicBool::new(watches_starts(&config)));
    let watcher = spawn_watcher(tx.clone(), watcher_stop.clone(), watch_starts.clone());
    let server = match ipc_server::start(shared.clone(), tx.clone(), &paths.pipe) {
        Ok(s) => Some(s),
        Err(e) => {
            error!(error = %e, "IPC disabled");
            None
        }
    };

    let telegram_stop = Arc::new(AtomicBool::new(false));
    if let Err(e) = crate::telegram::spawn(
        paths.telegram.clone(),
        shared.clone(),
        tx.clone(),
        telegram_stop.clone(),
    ) {
        shared
            .events
            .warn(None, format!("Telegram bot unavailable: {e}"));
    }
    let mut sampler = Sampler::new();
    // Network bytes per process (ETW). The session name carries our PID so a
    // test run never stops the installed service's session.
    stop_trace_orphans();
    let mut next_sweep = Instant::now() + TRACE_SWEEP_EVERY;
    if let Err(e) = sampler.enable_network_trace(&wtm_sys::etw::owned_name(NET_TRACE)) {
        warn!(error = %e, "per-process network traffic unavailable");
    }
    let mut governor = Governor::new(config.governor.clone());
    let mut memory_guard = wtm_policy::MemoryGuard::new(config.memory_guard.clone());
    let mut rules = RuleEngine::new(config.rules.clone());
    let mut kill_watch = wtm_policy::KillWatch::new(config.kill.clone());
    let mut swarm_watch = wtm_policy::SwarmWatch::new();
    // Dry run: when each swarm's cleanup was last reported.
    let mut swarm_previewed: HashMap<String, Instant> = HashMap::new();
    let mut executor = Executor::new(config.dry_run, shared.events.clone());
    let mut rescue = Rescue::new(
        config.reserve.clone(),
        config.dry_run,
        shared.events.clone(),
    );
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
    let blocker = open_blocker(
        &paths.block_root,
        paths.block_debugger.clone(),
        &shared.events,
    );
    sync_blocks(blocker.as_ref(), &config, &shared.events);
    let mut insights = InsightPolicy::new();
    let mut next_insights = Instant::now() + Duration::from_secs(5);
    let mut next_prune = Instant::now();
    let mut paths_cache = PathCache::new();
    let mut cmdline_cache = PathCache::new();
    // The processes of the current sample, refilled each tick.
    let mut live: HashSet<(u32, i64)> = HashSet::new();
    let mut app_limiter =
        AppLimiter::new(config.limits.clone(), config.dry_run, shared.events.clone());
    let mut app_usage = Vec::new();
    // Last foreground report from a client, and the app it names.
    let mut foreground: Option<(u32, Instant)> = None;
    let mut foreground_app: Option<String> = None;
    // The foreground window covers its monitor.
    let mut fullscreen = false;
    let mut game_mode = GameMode::new(config.game_mode.clone());
    // The plan active before game mode switched it; one left by a crash is
    // put back first.
    restore_saved_plan(&paths.power_plan, &mut |line| {
        shared.events.info(None, line)
    });
    let mut saved_plan = SavedPlan {
        plan: None,
        file: Some(paths.power_plan.clone()),
    };
    let mut last_snapshot: Option<Arc<Snapshot>> = None;

    shared.events.info(
        None,
        format!(
            "engine started (dry_run={}, governor={}, rules={}, kill list={})",
            config.dry_run,
            config.governor.enabled,
            config.rules.len(),
            config.kill.len()
        ),
    );

    let mut next_tick = Instant::now();
    let started = (SystemTime::now(), Instant::now());
    'run: loop {
        // Serve control requests until the next sampling tick is due.
        loop {
            match rx.recv_timeout(next_tick.saturating_duration_since(Instant::now())) {
                Ok(EngineMsg::Stop) | Err(RecvTimeoutError::Disconnected) => break 'run,
                Ok(EngineMsg::Started(started)) => {
                    let decisions = rules.on_started(Instant::now(), &started, &mut |p, d| {
                        lookup(&mut paths_cache, &mut cmdline_cache, p, d)
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
                        .unwrap_or_else(PoisonError::into_inner) = Status {
                        config_error: config_error.clone(),
                        ..status(
                            &config,
                            paths,
                            &executor,
                            &app_usage,
                            foreground_app.as_deref(),
                            &rescue,
                            &game_mode,
                        )
                    };
                }
                Ok(EngineMsg::Foreground(pid, full)) => {
                    foreground = Some((pid, Instant::now()));
                    fullscreen = full;
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
                    // What a removed or edited rule applied is taken back;
                    // the rules as they are now fire again.
                    if new.rules != config.rules {
                        executor.restore_sources(&[Source::Rule], "rules changed");
                    }
                    // Likewise for the other policies: a process now exempt,
                    // or an action taken off a list, is let go at once, not
                    // whenever the policy happens to let go; they start
                    // over with the new settings.
                    if new.governor != config.governor {
                        executor.restore_sources(&[Source::Governor], "governor settings changed");
                        governor = Governor::new(new.governor.clone());
                    }
                    if new.memory_guard != config.memory_guard {
                        executor.restore_sources(
                            &[Source::MemoryGuard],
                            "memory guard settings changed",
                        );
                        memory_guard = wtm_policy::MemoryGuard::new(new.memory_guard.clone());
                    }
                    if new.insights != config.insights {
                        executor.restore_sources(&[Source::Insight], "insight settings changed");
                        insights = InsightPolicy::new();
                        next_insights = Instant::now();
                    }
                    if new.game_mode != config.game_mode {
                        executor.restore_sources(&[Source::GameMode], "game mode settings changed");
                        game_mode = GameMode::new(new.game_mode.clone());
                        // The new game mode never reports the game in front
                        // as ended: the plan goes back now unless it is
                        // still wanted.
                        if !(new.game_mode.enabled && new.game_mode.power_plan) {
                            restore_plan(&mut saved_plan, &shared.events);
                        }
                    }
                    governor.set_config(new.governor.clone());
                    memory_guard.set_config(new.memory_guard.clone());
                    rules.set_rules(new.rules.clone());
                    kill_watch.set_entries(new.kill.clone());
                    app_limiter.configure(new.limits.clone(), new.dry_run);
                    executor.set_dry_run(new.dry_run);
                    if new.dry_run != config.dry_run {
                        // The policies took their dry-run previews (or the
                        // actions now rolled back) as done; they start over,
                        // so enforcing acts on what is already running.
                        governor = Governor::new(new.governor.clone());
                        memory_guard = wtm_policy::MemoryGuard::new(new.memory_guard.clone());
                        rules = RuleEngine::new(new.rules.clone());
                        kill_watch = wtm_policy::KillWatch::new(new.kill.clone());
                        insights = InsightPolicy::new();
                        game_mode = GameMode::new(new.game_mode.clone());
                        if new.dry_run {
                            restore_plan(&mut saved_plan, &shared.events);
                        }
                    }
                    rescue.configure(new.reserve.clone(), new.dry_run);
                    game_mode.set_config(new.game_mode.clone());
                    watch_starts.store(watches_starts(&new), Ordering::Relaxed);
                    sync_blocks(blocker.as_ref(), &new, &shared.events);
                    shared.events.info(
                        None,
                        format!(
                            "config reloaded (dry_run={}, rules={}, kill list={})",
                            new.dry_run,
                            new.rules.len(),
                            new.kill.len()
                        ),
                    );
                    *shared
                        .config
                        .write()
                        .unwrap_or_else(PoisonError::into_inner) = new.clone();
                    config = new;
                    config_error = None;
                    shared.hardware.dry_run(config.dry_run);
                }
                Err(e) => {
                    shared
                        .events
                        .warn(None, format!("config reload failed, keeping previous: {e}"));
                    config_error = Some(e.to_string());
                }
            }
        }

        let snap = match sampler.sample() {
            Ok(s) => Arc::new(s),
            Err(e) => {
                warn!(error = %e, "sampling failed");
                continue;
            }
        };
        live.clear();
        live.extend(snap.processes.iter().map(|p| (p.pid, p.create_time)));
        executor.forget_exited(&live);
        rescue.tick(Instant::now(), &snap, &live);
        if snap.interval.is_some() {
            let mut recent = shared
                .recent
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            recent.push_back(wtm_ipc::SystemPoint::of(&snap));
            while recent.len() > RECENT_LEN {
                recent.pop_front();
            }
        }
        app_usage = app_limiter.tick(&snap.processes, snap.system.logical_cpus);
        if Instant::now() >= next_sweep {
            next_sweep = Instant::now() + TRACE_SWEEP_EVERY;
            stop_trace_orphans();
        }
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
        paths_cache.retain(|k, _| live.contains(k));
        cmdline_cache.retain(|k, _| live.contains(k));

        let now = Instant::now();
        let (keys, app) = foreground_keys(foreground, &snap.processes);
        memory_guard.set_foreground(keys.clone());
        let game = app
            .clone()
            .filter(|_| fullscreen)
            .map(|name| (name, keys.clone()));
        governor.set_foreground(keys);
        foreground_app = app;
        let mut decisions = governor.tick(now, &snap);
        decisions.extend(memory_guard.tick(now, &snap));
        if config.insights.enabled {
            decisions.extend(insights.tick(&snap.processes, &config.insights));
        }
        decisions.extend(rules.tick(now, &snap.processes, &mut |p, d| {
            lookup(&mut paths_cache, &mut cmdline_cache, p, d)
        }));
        decisions.extend(kill_watch.tick(
            now,
            &snap.processes,
            snap.system.cpu_percent,
            &mut |p| cached_command_line(&mut cmdline_cache, p),
        ));
        // The per-file disk trace runs only while a client keeps asking.
        {
            let mut trace = shared
                .disk_trace
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if trace.as_ref().is_some_and(|t| t.idle(DISK_TRACE_IDLE)) {
                *trace = None;
            }
        }
        // So does the sensor helper, unless it is driving a fan.
        shared.hardware.stop_if_idle(HARDWARE_IDLE);
        let (calm, change) = game_mode.tick(game, &snap.processes);
        decisions.extend(calm);
        if let Some(change) = change {
            game_changed(&change, &config, &mut saved_plan, &shared.events);
        }
        for d in &decisions {
            let _ = executor.execute(d);
        }
        for (name, count, parents) in swarm_watch.tick(&snap.processes, &config.swarm) {
            shared.events.alert(
                None,
                format!(
                    "{count} copies of {name} are running (started by {parents}); end them from RustWTM's Overview"
                ),
            );
        }

        if config.swarm.end_after_mins > 0 {
            end_stuck_swarms(
                &config,
                &snap.processes,
                &shared,
                &mut swarm_previewed,
                steady_now(started),
            );
        }

        *shared
            .snapshot
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(snap.clone());
        *shared
            .status
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Status {
            config_error: config_error.clone(),
            ..status(
                &config,
                paths,
                &executor,
                &app_usage,
                foreground_app.as_deref(),
                &rescue,
                &game_mode,
            )
        };
        last_snapshot = Some(snap);
    }

    shared
        .events
        .info(None, "stopping; restoring all processes");
    shared.hardware.stop();
    executor.restore_all("service stopping");
    *shared
        .disk_trace
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = None;
    restore_plan(&mut saved_plan, &shared.events);
    rescue.release_all("service stopping");
    app_limiter.lift_all("service stopping");
    let unblock = Config {
        dry_run: true,
        ..Config::default()
    };
    sync_blocks(blocker.as_ref(), &unblock, &shared.events);
    if let Some(h) = history.as_mut() {
        let _ = h.flush();
    }
    watcher_stop.store(true, Ordering::SeqCst);
    // Not joined: it may be waiting on Telegram for a few seconds.
    telegram_stop.store(true, Ordering::SeqCst);
    let _ = watcher.join();
    if let Some(s) = server {
        s.shutdown();
    }
    Ok(())
}

/// The power plan game mode replaced. Kept on disk too while it is
/// replaced, so that it is put back after a crash ([`restore_saved_plan`]
/// at the next start, or on uninstall) and the machine is not left on High
/// performance.
#[derive(Default)]
struct SavedPlan {
    plan: Option<String>,
    file: Option<PathBuf>,
}

/// Switches the power plan for a game that starts, and back when it ends.
fn game_changed(
    change: &GameChange,
    config: &Config,
    saved_plan: &mut SavedPlan,
    events: &EventLog,
) {
    match change {
        GameChange::Started(name) => {
            events.info(None, format!("game mode: {name} is in front"));
            if !config.game_mode.power_plan || saved_plan.plan.is_some() {
                return;
            }
            if config.dry_run {
                events.info(None, "game mode would switch to High performance (dry run)");
                return;
            }
            let Ok(current) = wtm_sys::power::active_scheme() else {
                return;
            };
            let current = current.trim_matches(['{', '}']).to_owned();
            // Written first: a crash right after switching still finds it.
            if let Some(file) = &saved_plan.file
                && let Err(e) = std::fs::write(file, &current)
            {
                events.warn(
                    None,
                    format!(
                        "game mode: power plan unchanged: cannot remember the current one: {e}"
                    ),
                );
                return;
            }
            match wtm_sys::power::set_active_scheme(wtm_sys::power::HIGH_PERFORMANCE) {
                Ok(()) => {
                    events.info(None, "game mode: High performance power plan");
                    saved_plan.plan = Some(current);
                }
                Err(e) => events.warn(None, format!("game mode: power plan unchanged: {e}")),
            }
        }
        GameChange::Ended(name) => {
            events.info(None, format!("game mode: {name} ended"));
            restore_plan(saved_plan, events);
        }
    }
}

fn restore_plan(saved_plan: &mut SavedPlan, events: &EventLog) {
    let Some(plan) = saved_plan.plan.take() else {
        return;
    };
    match wtm_sys::power::set_active_scheme(&plan) {
        // Left on disk otherwise, to be tried again at the next start.
        Ok(()) => {
            if let Some(file) = &saved_plan.file {
                let _ = std::fs::remove_file(file);
            }
        }
        Err(e) => events.warn(
            None,
            format!("could not restore the power plan {plan}: {e}"),
        ),
    }
}

/// Puts back the power plan game mode replaced before the service stopped
/// without restoring it (a crash), as remembered in `file`; reports to
/// `say`. Run at start and on uninstall.
pub fn restore_saved_plan(file: &Path, say: &mut dyn FnMut(String)) {
    let plan = match std::fs::read_to_string(file) {
        Ok(text) => text.trim().to_owned(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => return say(format!("could not read the saved power plan: {e}")),
    };
    let well_formed = plan.len() == 36
        && plan.char_indices().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        });
    let result = if well_formed {
        wtm_sys::power::set_active_scheme(&plan).map_err(|e| e.to_string())
    } else {
        Err("not a power plan".to_owned())
    };
    match result {
        Ok(()) => {
            say(format!(
                "restored the power plan {plan} game mode had replaced"
            ));
            let _ = std::fs::remove_file(file);
        }
        Err(e) if well_formed => say(format!("could not restore the power plan {plan}: {e}")),
        Err(e) => {
            say(format!("ignored the saved power plan: {e}"));
            let _ = std::fs::remove_file(file);
        }
    }
}

/// Copies of a swarm ended per sampling tick, so a huge one cannot stall
/// the engine; the rest follow on the next ticks.
const SWARM_END_PER_TICK: usize = 100;

/// The wall-clock time to judge process ages by, from `started` (the wall
/// clock and the monotonic one at the engine's start): never later than the
/// monotonic clock allows, so setting the clock forward cannot make every
/// copy of a crowd look old enough to end. A clock set back only makes
/// processes look younger.
fn steady_now(started: (SystemTime, Instant)) -> SystemTime {
    SystemTime::now().min(started.0 + started.1.elapsed())
}

/// Ends swarm copies that ran longer than `end_after_mins`, with what they
/// started (their console hosts); in dry run only reports them.
fn end_stuck_swarms(
    config: &Config,
    procs: &[ProcessInfo],
    shared: &Shared,
    previewed: &mut HashMap<String, Instant>,
    now: SystemTime,
) {
    let age = Duration::from_secs(config.swarm.end_after_mins * 60);
    let mut budget = SWARM_END_PER_TICK;
    for swarm in wtm_policy::find_swarms(procs, &config.swarm) {
        let stuck = swarm.older_than(age, now);
        if stuck.is_empty() || budget == 0 {
            continue;
        }
        let mins = config.swarm.end_after_mins;
        if config.dry_run {
            let due = previewed
                .get(&swarm.name)
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(600));
            if due {
                previewed.insert(swarm.name.clone(), Instant::now());
                shared.events.info(
                    None,
                    format!(
                        "dry run: would end {} copies of {} running longer than {mins} min",
                        stuck.len(),
                        swarm.name
                    ),
                );
            }
            continue;
        }
        let (mut ended, mut children, mut failed) = (0, 0, 0);
        for p in stuck.into_iter().take(budget) {
            budget -= 1;
            for (i, (_, r)) in control::kill_tree(&ProcessRef::from(p), procs)
                .into_iter()
                .enumerate()
            {
                match (i, r) {
                    (0, Ok(())) => ended += 1,
                    (_, Ok(())) => children += 1,
                    (_, Err(_)) => failed += 1,
                }
            }
        }
        if ended + failed > 0 {
            let extra = if children > 0 {
                format!(" and {children} processes they started")
            } else {
                String::new()
            };
            let failures = if failed > 0 {
                format!("; {failed} could not be ended")
            } else {
                String::new()
            };
            shared.events.alert(
                None,
                format!(
                    "Ended {ended} copies of {} that ran longer than {mins} min{extra}{failures}",
                    swarm.name
                ),
            );
        }
    }
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
        // A hand-picked priority or efficiency mode is what a later rollback
        // of RustWTM's own actions returns to.
        Command::Priority(p) => {
            let r = control::set_priority(target, p);
            if r.is_ok() {
                executor.manual_priority(target, p);
            }
            vec![outcome(target, r)]
        }
        Command::Affinity(mask) => vec![outcome(target, control::set_affinity(target, mask))],
        Command::AffinityTier(selection) => vec![outcome(
            target,
            control::set_core_affinity(target, selection),
        )],
        Command::EcoQos(mode) => {
            let r = control::set_eco_qos(target, mode);
            if r.is_ok() {
                executor.manual_eco(target, mode);
            }
            vec![outcome(target, r)]
        }
        Command::TrimMemory => vec![outcome(target, control::trim_memory(target))],
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
            let sources = executor.sources(target);
            let result = if sources.contains(&Source::Manual) {
                executor.execute(&Decision::Restore {
                    source: Source::Manual,
                    target: target.clone(),
                    reason: "manual".into(),
                })
            } else {
                Err(not_limited_by_hand(&sources))
            };
            return vec![Outcome {
                target: target.clone(),
                result,
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

/// Why "remove limits" did nothing: nothing set by hand to remove.
fn not_limited_by_hand(sources: &[Source]) -> String {
    if sources.is_empty() {
        return "not limited by RustWTM in this session".into();
    }
    let names: Vec<String> = sources.iter().map(|s| format!("{s:?}")).collect();
    format!(
        "no limits set by hand; the current ones come from {} and are lifted when it lets go",
        names.join(", ")
    )
}

type PathCache = HashMap<(u32, i64), Option<String>>;

fn cached_path(cache: &mut PathCache, p: &ProcessInfo) -> Option<String> {
    cache
        .entry((p.pid, p.create_time))
        .or_insert_with(|| control::image_path(&ProcessRef::from(p)).ok())
        .clone()
}

fn cached_command_line(cache: &mut PathCache, p: &ProcessInfo) -> Option<String> {
    cache
        .entry((p.pid, p.create_time))
        .or_insert_with(|| control::command_line(&ProcessRef::from(p)).ok())
        .clone()
}

/// What rules ask about a process, read once per process.
fn lookup(
    paths: &mut PathCache,
    cmdlines: &mut PathCache,
    p: &ProcessInfo,
    detail: wtm_policy::Detail,
) -> Option<String> {
    match detail {
        wtm_policy::Detail::Path => cached_path(paths, p),
        wtm_policy::Detail::CommandLine => cached_command_line(cmdlines, p),
    }
}

const WATCH_INTERVAL: Duration = Duration::from_millis(250);

/// Samples kept for clients' history graphs (five minutes at the default
/// interval).
const RECENT_LEN: usize = 300;

/// Only rules without a `when` act on processes as they start.
fn watches_starts(config: &Config) -> bool {
    config.rules.iter().any(|r| r.when.is_none())
}

/// Polls for new processes between full samples so unconditional rules
/// (e.g. `kill`) act within a fraction of a second. A poll reads the whole
/// process list, so it runs only while `wanted` (such a rule exists).
fn spawn_watcher(
    tx: Sender<EngineMsg>,
    stop: Arc<AtomicBool>,
    wanted: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("start-watcher".into())
        .spawn(move || {
            let mut watcher: Option<StartWatcher> = None;
            let mut warned = false;
            while !stop.load(Ordering::SeqCst) {
                if !wanted.load(Ordering::Relaxed) {
                    // Primed afresh when wanted again: what started
                    // meanwhile was seen by the full samples.
                    watcher = None;
                    std::thread::sleep(WATCH_INTERVAL);
                    continue;
                }
                match watcher.get_or_insert_with(StartWatcher::new).poll() {
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
            // Not "nothing found": that would lift every insight action, to
            // apply and announce them again once the history answers.
            Err(e) => {
                warn!(error = %e, "history query failed; findings kept");
                return;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command as Proc, Stdio};
    use std::sync::mpsc;

    use wtm_core::control::{Limits, PriorityClass};
    use wtm_ipc::{Client, Command, Request, Response};

    use crate::test_util::wait_for;

    struct Guard(Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn spawn(exe: &Path) -> Guard {
        Guard(
            Proc::new(exe)
                .raw_arg("/d /c ping -n 60 127.0.0.1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    fn connect(pipe: &str) -> Client {
        wait_for(pipe, || Client::connect_to(pipe).ok())
    }

    fn events(c: &mut Client) -> Vec<wtm_ipc::Event> {
        let r = c.call(&Request::Events { after: 0 }).unwrap();
        expect_reply!(r, Response::Events)
    }

    /// `None` until the engine has published its first sample.
    fn snapshot(c: &mut Client) -> Option<Snapshot> {
        let r = c.call(&Request::Snapshot).unwrap();
        (!matches!(r, Response::Error(_))).then(|| expect_reply!(r, Response::Snapshot))
    }

    #[test]
    fn rules_act_once_enforcing_and_are_taken_back_when_removed() {
        let dir = std::env::temp_dir().join(format!("wtm-engine-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let idler = dir.join("wtm-engine-idler.exe");
        std::fs::copy(r"C:\Windows\System32\cmd.exe", &idler).unwrap();
        let paths = Paths {
            pipe: format!(r"\\.\pipe\wtm-engine-reload-{}", std::process::id()),
            ..Paths::under(&dir)
        };
        let config = |dry_run: bool, rule: bool| {
            let mut text = format!(
                "dry_run = {dry_run}\nsample_interval_ms = 200\n\
                 [governor]\nenabled = false\n[game_mode]\nenabled = false\n"
            );
            if rule {
                text += "[[rule]]\nname = \"idle it\"\nmatch = { name = \"wtm-engine-idler.exe\" }\n\
                         actions = [{ priority = \"idle\" }]\n";
            }
            text
        };
        std::fs::write(&paths.config, config(true, true)).unwrap();
        let (tx, rx) = mpsc::channel();
        let stop = tx.clone();
        let engine_paths = paths.clone();
        let engine = std::thread::spawn(move || run(&engine_paths, tx, &rx));
        let mut client = connect(&paths.pipe);

        let child = spawn(&idler);
        let pid = child.0.id();
        let target = wait_for("idler sampled", || {
            snapshot(&mut client)?
                .processes
                .iter()
                .find(|p| p.pid == pid)
                .map(ProcessRef::from)
        });
        let priority = || wtm_core::control::inspect(&target).unwrap().priority;
        wait_for("dry-run preview", || {
            events(&mut client)
                .iter()
                .any(|e| e.message.contains("would") && e.pid == Some(pid))
                .then_some(())
        });
        assert_eq!(
            priority(),
            Some(PriorityClass::Normal),
            "dry run changes nothing"
        );

        // Enforcing acts on the process already running.
        std::thread::sleep(Duration::from_millis(1100)); // distinct mtime
        std::fs::write(&paths.config, config(false, true)).unwrap();
        wait_for("idle once enforcing", || {
            (priority() == Some(PriorityClass::Idle)).then_some(())
        });

        // Removing the rule takes back what it did.
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(&paths.config, config(false, false)).unwrap();
        wait_for("normal once the rule is gone", || {
            (priority() == Some(PriorityClass::Normal)).then_some(())
        });

        drop(client);
        stop.send(EngineMsg::Stop).unwrap();
        engine.join().unwrap().unwrap();
        drop(child);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engine_serves_clients_enforces_rules_and_reloads_config() {
        let dir = std::env::temp_dir().join(format!("wtm-engine-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let victim = dir.join("wtm-engine-victim.exe");
        std::fs::copy(r"C:\Windows\System32\cmd.exe", &victim).unwrap();
        let hog = dir.join("wtm-engine-hog.exe");
        std::fs::copy(r"C:\Windows\System32\cmd.exe", &hog).unwrap();
        let swarm = dir.join("wtm-engine-swarm.exe");
        std::fs::copy(r"C:\Windows\System32\cmd.exe", &swarm).unwrap();

        let block_key = format!(r"Software\RustWTM-test-engine-block-{}", std::process::id());
        let paths = Paths {
            pipe: format!(r"\\.\pipe\wtm-engine-test-{}", std::process::id()),
            block_root: BlockRoot::User(block_key.clone()),
            block_debugger: Some("blocked-test --blocked".into()),
            ..Paths::under(&dir)
        };
        let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
        let blocked = || {
            hkcu.open_subkey(&block_key)
                .map(|k| k.enum_keys().filter_map(Result::ok).collect::<Vec<_>>())
                .unwrap_or_default()
        };
        std::fs::write(
            &paths.config,
            "dry_run = false\nsample_interval_ms = 200\nblock = [\"wtm-blocked-test.exe\"]\n[governor]\nenabled = false\n\
             [[rule]]\nname = \"no victims\"\nmatch = { name = \"wtm-engine-victim.exe\" }\nactions = [\"kill\"]\n\
             [[kill]]\nname = \"wtm-engine-hog.exe\"\ncmdline = \"burn\"\ncpu_above = 1\nfor_secs = 1\n\
             [swarm]\nmin_count = 10\n",
        )
        .unwrap();

        let (tx, rx) = mpsc::channel();
        let stop = tx.clone();
        let engine_paths = paths.clone();
        let engine = std::thread::spawn(move || run(&engine_paths, tx, &rx));

        let mut client = connect(&paths.pipe);
        // Not the installed service, so the server cannot be verified.
        assert!(!client.is_verified());

        // A process matching an unconditional kill rule is terminated quickly.
        let mut v = spawn(&victim);
        let status = wait_for("victim killed", || v.0.try_wait().unwrap());
        assert_eq!(status.code(), Some(1));

        // Kill list: of two CPU-burning copies only the one whose command
        // line names "burn" is ended.
        let busy = |tag: &str| {
            Guard(
                Proc::new(&hog)
                    .raw_arg(format!("/d /c for /l %i in (0,0,1) do rem {tag}"))
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            )
        };
        let mut burner = busy("burn");
        let mut spared = busy("spare");
        let status = wait_for("kill list ended the burner", || {
            burner.0.try_wait().unwrap()
        });
        assert_eq!(status.code(), Some(1));
        assert!(
            spared.0.try_wait().unwrap().is_none(),
            "other command line kept"
        );
        drop(spared);

        // A swarm of idle copies is reported once, with who started them.
        let copies: Vec<Guard> = (0..12)
            .map(|_| {
                Guard(
                    Proc::new(&swarm)
                        .args(["/d", "/k"])
                        .stdin(Stdio::piped())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                        .unwrap(),
                )
            })
            .collect();
        let alert = wait_for("swarm reported", || {
            events(&mut client)
                .into_iter()
                .find(|e| e.message.contains("copies of wtm-engine-swarm.exe"))
        });
        assert!(alert.notify, "{alert:?}");
        // Reported as soon as at least 10 copies run, with their parent.
        let count: usize = alert.message.split(' ').next().unwrap().parse().unwrap();
        assert!((10..=12).contains(&count), "{alert:?}");
        assert!(
            alert.message.contains("started by wtm_service"),
            "{alert:?}"
        );
        drop(copies);

        // Read-only requests.
        let snap = wait_for("snapshot", || snapshot(&mut client));
        assert!(snap.processes.iter().any(|p| p.pid == std::process::id()));
        let s = expect_reply!(client.call(&Request::Status).unwrap(), Response::Status);
        assert!(!s.dry_run);
        assert_eq!(s.rules, 1);
        let log = events(&mut client);
        assert!(log.iter().any(|e| e.message.starts_with("engine started")));
        assert!(
            log.iter()
                .any(|e| e.message == "blocked from starting: wtm-blocked-test.exe"),
            "{log:?}"
        );
        assert_eq!(blocked(), ["wtm-blocked-test.exe"]);
        assert!(
            log.iter().any(|e| e.notify && e.message.contains("kill")),
            "{log:?}"
        );

        // Inspect runs with our own rights.
        let target = wait_for("helper in snapshot", || {
            let child = spawn(Path::new("cmd.exe"));
            let pid = child.0.id();
            std::thread::sleep(Duration::from_millis(500));
            let snap = snapshot(&mut client).unwrap();
            let found = snap.processes.iter().find(|p| p.pid == pid);
            found.map(|p| (child, ProcessRef::from(p)))
        });
        let (_helper, helper) = target;
        let details = expect_reply!(
            client.call(&Request::Inspect(helper.clone())).unwrap(),
            Response::Details
        );
        assert!(details.path.unwrap().to_lowercase().ends_with("cmd.exe"));

        // Control requests need an elevated client.
        let limit = Request::Control {
            target: helper.clone(),
            command: Command::Limit(Limits {
                cpu_percent: Some(50.0),
                memory_bytes: Some(1 << 30),
            }),
        };
        let r = client.call(&limit).unwrap();
        let applied = matches!(&r, Response::Control(o) if o.iter().all(|o| o.result.is_ok()));
        let refused = matches!(&r, Response::Error(e) if e.contains("administrator"));
        assert!(if client.is_admin() { applied } else { refused }, "{r:?}");
        for command in [
            Command::Priority(PriorityClass::BelowNormal),
            Command::Suspend,
            Command::Resume,
            Command::Unlimit,
            Command::TrimMemory,
            Command::KillTree,
        ] {
            let request = Request::Control {
                target: helper.clone(),
                command,
            };
            let r = client.call(&request).unwrap();
            let forwarded = matches!(r, Response::Control(_));
            assert_eq!(forwarded, client.is_admin(), "{command:?}: {r:?}");
        }

        // History is available (possibly still empty).
        assert!(matches!(
            client.call(&Request::History { hours: 1 }).unwrap(),
            Response::History { .. }
        ));
        assert!(matches!(
            client
                .call(&Request::AppSeries {
                    app: "cmd.exe".into(),
                    hours: 1
                })
                .unwrap(),
            Response::Series(_)
        ));
        assert!(matches!(
            client.call(&Request::Hello { version: 0 }).unwrap(),
            Response::Error(_)
        ));

        // Config changes are picked up; invalid ones are rejected.
        std::thread::sleep(Duration::from_millis(1100)); // distinct mtime
        std::fs::write(&paths.config, "dry_run = true\n").unwrap();
        wait_for("config reload", || {
            events(&mut client)
                .iter()
                .any(|e| e.message.starts_with("config reloaded"))
                .then_some(())
        });
        // Dry-run lifts the block.
        assert!(blocked().is_empty());
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(&paths.config, "dry_run = maybe\n").unwrap();
        wait_for("rejected config", || {
            events(&mut client)
                .iter()
                .any(|e| e.message.starts_with("config reload failed"))
                .then_some(())
        });

        drop(client);
        stop.send(EngineMsg::Stop).unwrap();
        engine.join().unwrap().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = hkcu.delete_subkey_all(&block_key);
    }

    #[test]
    fn block_list_follows_dry_run_and_reports_problems() {
        let key = format!(r"Software\RustWTM-test-engine-sync-{}", std::process::id());
        let root = BlockRoot::User(key.clone());
        let events = EventLog::default();
        let messages = || {
            events
                .after(0)
                .into_iter()
                .map(|e| e.message)
                .collect::<Vec<_>>()
        };
        assert!(open_blocker(&root, None, &events).is_none());
        sync_blocks(None, &Config::default(), &events);
        assert_eq!(
            messages(),
            ["program blocking disabled: rustwtm.exe not found next to the service"]
        );

        let b = open_blocker(&root, Some("x --blocked".into()), &events).unwrap();
        let mut config = Config {
            block: vec!["a.exe".into()],
            ..Config::default()
        };
        sync_blocks(Some(&b), &config, &events);
        assert_eq!(messages().last().unwrap(), "dry-run: would block a.exe");
        assert!(b.blocked().is_empty());

        config.dry_run = false;
        sync_blocks(Some(&b), &config, &events);
        assert_eq!(messages().last().unwrap(), "blocked from starting: a.exe");

        let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
        let (other, _) = hkcu.create_subkey(format!(r"{key}\b.exe")).unwrap();
        other.set_value("Debugger", &"someone-else.exe").unwrap();
        config.block = vec!["b.exe".into()];
        sync_blocks(Some(&b), &config, &events);
        let log = messages();
        assert!(
            log.contains(&"no longer blocked: a.exe".to_owned()),
            "{log:?}"
        );
        assert_eq!(
            log.last().unwrap(),
            "cannot block b.exe: another program set its debugger"
        );

        // A marked entry whose debugger vanished is still taken out.
        let (broken, _) = hkcu.create_subkey(format!(r"{key}\c.exe")).unwrap();
        broken.set_value("RustWTMBlock", &1u32).unwrap();
        sync_blocks(Some(&b), &Config::default(), &events);
        assert_eq!(messages().last().unwrap(), "no longer blocked: c.exe");

        // Registry key names are limited to 255 characters.
        config.block = vec![format!("{}.exe", "x".repeat(300))];
        sync_blocks(Some(&b), &config, &events);
        assert!(
            messages()
                .last()
                .unwrap()
                .starts_with("updating blocked programs failed")
        );
        hkcu.delete_subkey_all(&key).unwrap();

        // The real key needs elevation; without it blocking is disabled.
        let system = open_blocker(&BlockRoot::System, Some("x".into()), &events);
        assert_eq!(system.is_some(), Blocker::system("x".into()).is_ok());
    }

    #[test]
    fn missing_config_is_created_and_invalid_config_falls_back() {
        let dir = std::env::temp_dir().join(format!("wtm-config-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.toml");
        let (config, error) = load_config(&path).unwrap();
        assert!(config.dry_run);
        assert!(error.is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG_TOML);

        std::fs::write(&path, "nonsense = [").unwrap();
        let (config, error) = load_config(&path).unwrap();
        assert_eq!(config, Config::default());
        // The parser's message, for clients to show.
        assert!(error.is_some_and(|e| !e.is_empty()));
        let _ = std::fs::remove_dir_all(&dir);

        // Nowhere to write the default config.
        assert!(load_config(Path::new("")).is_err());
    }

    fn temp_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wtm-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn engine_without_ipc_or_history_still_serves_control_messages() {
        let dir = temp_dir("engine-degraded");
        let pipe = format!(r"\\.\pipe\wtm-engine-taken-{}", std::process::id());
        // Someone else owns the pipe name.
        let _squatter = wtm_sys::pipe::PipeListener::bind(&pipe, wtm_ipc::PIPE_SDDL).unwrap();
        let paths = Paths {
            pipe,
            history: dir.clone(), // a directory is no database
            ..Paths::under(&dir)
        };
        std::fs::write(
            &paths.config,
            "sample_interval_ms = 200\n[governor]\nenabled = false\n",
        )
        .unwrap();

        let (tx, rx) = mpsc::channel();
        let control = tx.clone();
        let engine = std::thread::spawn(move || run(&paths, tx, &rx));

        let child = spawn(Path::new("cmd.exe"));
        let target = wait_for("child sampled", || {
            let snap = Sampler::new().sample().unwrap();
            let found = snap.processes.iter().find(|p| p.pid == child.0.id());
            found.map(ProcessRef::from)
        });
        let (reply, outcomes) = mpsc::channel();
        control
            .send(EngineMsg::Control {
                target,
                command: Command::Priority(PriorityClass::BelowNormal),
                reply,
            })
            .unwrap();
        let outcomes = outcomes.recv_timeout(Duration::from_secs(30)).unwrap();
        assert!(outcomes[0].result.is_ok(), "{outcomes:?}");

        control.send(EngineMsg::Stop).unwrap();
        engine.join().unwrap().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engine_resolves_rule_paths_and_raises_history_insights() {
        let dir = temp_dir("engine-insights");
        let victim = dir.join("wtm-engine-path-victim.exe");
        std::fs::copy(r"C:\Windows\System32\cmd.exe", &victim).unwrap();
        let paths = Paths {
            pipe: format!(r"\\.\pipe\wtm-engine-insights-{}", std::process::id()),
            ..Paths::under(&dir)
        };
        // Dry-run: the rule is only logged. The temp dir may be an 8.3 path,
        // so match on the file name part of the full image path. Slow
        // sampling leaves new processes to the start watcher.
        std::fs::write(
            &paths.config,
            "dry_run = true\nsample_interval_ms = 3000\n[governor]\nenabled = false\n\
             [[rule]]\nname = \"path probe\"\nmatch = { path = '*\\wtm-engine-path-victim.exe' }\n\
             actions = [\"kill\"]\n",
        )
        .unwrap();
        // Ninety minutes of a busy app already on disk.
        let history = History::open(&paths.history).unwrap();
        let minute = unix_now() / 60;
        for m in 1..=90 {
            history
                .connection()
                .execute(
                    "INSERT INTO app_minute (app, minute, cpu_avg, cpu_peak, mem_avg, mem_peak, processes)
                     VALUES ('wtm-hog.exe', ?1, 40.0, 60.0, 1000, 2000, 1)",
                    [minute - m],
                )
                .unwrap();
        }
        drop(history);

        let (tx, rx) = mpsc::channel();
        let stop = tx.clone();
        let engine_paths = paths.clone();
        let engine = std::thread::spawn(move || run(&engine_paths, tx, &rx));
        let mut client = connect(&paths.pipe);
        // Started after the first sample, so the watcher sees it first.
        wait_for("first sample", || snapshot(&mut client));

        let v = spawn(&victim);
        let pid = v.0.id();
        wait_for("path rule", || {
            let log = events(&mut client);
            log.iter()
                .find(|e| e.pid == Some(pid) && e.message.starts_with("dry-run: would apply"))
                .map(|e| assert!(e.message.contains("path probe"), "{e:?}"))
        });
        // The first evaluation runs a few seconds after start.
        wait_for("insight", || {
            let log = events(&mut client);
            log.iter()
                .any(|e| e.notify && e.message.starts_with("wtm-hog.exe: ChronicCpu"))
                .then_some(())
        });
        let r = client.call(&Request::History { hours: 24 }).unwrap();
        let listed = |s: &[wtm_policy::AppStats]| s.iter().any(|s| s.app == "wtm-hog.exe");
        assert!(
            matches!(&r, Response::History { stats, findings } if listed(stats) && findings.len() == 1),
            "{r:?}"
        );

        drop(client);
        stop.send(EngineMsg::Stop).unwrap();
        engine.join().unwrap().unwrap();
        drop(v);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn process_ages_ignore_the_clock_set_forward() {
        let now = SystemTime::now();
        // Started an hour of wall clock ago, but only a second has passed.
        let started = (
            now - Duration::from_secs(3600),
            Instant::now() - Duration::from_secs(1),
        );
        let steady = steady_now(started);
        assert!(steady < now - Duration::from_secs(3000), "{steady:?}");
        // Without a jump the wall clock is used.
        let fair = (
            now - Duration::from_secs(5),
            Instant::now() - Duration::from_secs(5),
        );
        let d = SystemTime::now()
            .duration_since(steady_now(fair))
            .unwrap_or_default();
        assert!(d < Duration::from_secs(1), "{d:?}");
    }

    #[test]
    fn a_power_plan_left_by_a_crash_is_put_back_once() {
        let dir = std::env::temp_dir().join(format!("wtm-plan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("power-plan.txt");
        let mut said = Vec::new();
        restore_saved_plan(&file, &mut |l| said.push(l));
        assert!(said.is_empty(), "nothing saved, nothing done");

        // Setting the plan already active changes nothing on the machine.
        let current = wtm_sys::power::active_scheme().unwrap();
        std::fs::write(&file, current.trim_matches(['{', '}'])).unwrap();
        restore_saved_plan(&file, &mut |l| said.push(l));
        assert!(said[0].starts_with("restored"), "{said:?}");
        assert!(!file.exists());

        std::fs::write(&file, "../../not-a-plan").unwrap();
        restore_saved_plan(&file, &mut |l| said.push(l));
        assert!(said[1].starts_with("ignored"), "{said:?}");
        assert!(!file.exists());
        assert_eq!(wtm_sys::power::active_scheme().unwrap(), current);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn game_mode_leaves_the_power_plan_alone_in_dry_run_or_when_off() {
        let events = EventLog::default();
        let mut saved = SavedPlan::default();
        let mut config = Config {
            dry_run: true,
            ..Config::default()
        };
        config.game_mode.power_plan = true;
        game_changed(
            &GameChange::Started("game.exe".into()),
            &config,
            &mut saved,
            &events,
        );
        assert!(saved.plan.is_none());
        config.game_mode.power_plan = false;
        config.dry_run = false;
        game_changed(
            &GameChange::Started("game.exe".into()),
            &config,
            &mut saved,
            &events,
        );
        assert!(saved.plan.is_none());
        game_changed(
            &GameChange::Ended("game.exe".into()),
            &config,
            &mut saved,
            &events,
        );
        let log: Vec<String> = events.after(0).into_iter().map(|e| e.message).collect();
        assert_eq!(
            log,
            [
                "game mode: game.exe is in front",
                "game mode would switch to High performance (dry run)",
                "game mode: game.exe is in front",
                "game mode: game.exe ended",
            ]
        );
    }
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command as Proc, Stdio};

    use rusqlite::params;

    use crate::test_util::wait_for;
    use wtm_core::control::Limits;
    use wtm_ipc::Command;

    struct Guard(Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn spawn() -> Guard {
        let g = Guard(
            Proc::new("cmd.exe")
                .raw_arg("/d /c ping -n 60 127.0.0.1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        std::thread::sleep(Duration::from_millis(200));
        g
    }

    #[test]
    fn stuck_swarm_copies_are_previewed_in_dry_run_and_ended_when_enforcing() {
        let dir = std::env::temp_dir().join(format!("wtm-swarm-end-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("wtm-stuck-probe.exe");
        std::fs::copy(r"C:\Windows\System32\PING.EXE", &exe).unwrap();
        let _copies: Vec<Guard> = (0..12)
            .map(|_| {
                Guard(
                    Proc::new(&exe)
                        .args(["-n", "120", "127.0.0.1"])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                        .unwrap(),
                )
            })
            .collect();
        let probe = |p: &ProcessInfo| p.name.eq_ignore_ascii_case("wtm-stuck-probe.exe");
        // Only the probes are handed over: nothing else on this PC can be
        // taken for a swarm and ended.
        let procs: Vec<ProcessInfo> = wait_for("12 probes", || {
            let procs: Vec<ProcessInfo> = Sampler::new()
                .sample()
                .ok()?
                .processes
                .into_iter()
                .filter(probe)
                .collect();
            (procs.len() == 12).then_some(procs)
        });
        let mut config =
            Config::from_toml("dry_run = true\n[swarm]\nmin_count = 12\nend_after_mins = 1\n")
                .unwrap();
        let shared = Shared::default();
        let mut previewed = HashMap::new();
        let later = SystemTime::now() + Duration::from_secs(120);

        // Young copies are left alone.
        end_stuck_swarms(&config, &procs, &shared, &mut previewed, SystemTime::now());
        assert!(shared.events.after(0).is_empty());

        end_stuck_swarms(&config, &procs, &shared, &mut previewed, later);
        end_stuck_swarms(&config, &procs, &shared, &mut previewed, later);
        let events = shared.events.after(0);
        let previews: Vec<_> = events
            .iter()
            .filter(|e| e.message.contains("wtm-stuck-probe.exe"))
            .collect();
        assert_eq!(previews.len(), 1, "reported once: {events:?}");
        assert!(
            previews[0]
                .message
                .starts_with("dry run: would end 12 copies")
        );

        config.dry_run = false;
        end_stuck_swarms(&config, &procs, &shared, &mut previewed, later);
        let ended = shared
            .events
            .after(0)
            .into_iter()
            .find(|e| e.message.starts_with("Ended") && e.message.contains("wtm-stuck-probe.exe"))
            .expect("an alert about the ended copies");
        assert!(ended.notify);
        assert!(
            ended.message.starts_with("Ended 12 copies"),
            "{}",
            ended.message
        );
        wait_for("probes gone", || {
            let procs = Sampler::new().sample().ok()?.processes;
            procs.iter().all(|p| !probe(p)).then_some(())
        });
        drop(_copies);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn handle_control_runs_every_command() {
        let a = spawn();
        let b = spawn();
        let procs = Sampler::new().sample().unwrap().processes;
        let find = |pid| ProcessRef::from(procs.iter().find(|p| p.pid == pid).unwrap());
        let (ta, tb) = (find(a.0.id()), find(b.0.id()));
        let shared = Shared::default();
        // Dry-run must not affect manual commands.
        let mut executor = Executor::new(true, shared.events.clone());

        for command in [
            Command::Priority(PriorityClass::BelowNormal),
            Command::Affinity(1),
            Command::AffinityTier(wtm_core::cpu::CoreSelection::All),
            Command::EcoQos(wtm_core::control::EcoQos::On),
            Command::Suspend,
            Command::Resume,
            Command::Limit(Limits {
                cpu_percent: Some(50.0),
                memory_bytes: Some(1 << 30),
            }),
            Command::Limit(Limits {
                cpu_percent: Some(25.0),
                memory_bytes: None,
            }),
            Command::Unlimit,
            Command::TrimMemory,
            Command::KillTree,
        ] {
            let outcomes = handle_control(&mut executor, &procs, &shared, &ta, command);
            assert!(!outcomes.is_empty());
            assert!(
                outcomes.iter().all(|o| o.result.is_ok()),
                "{command:?}: {outcomes:?}"
            );
        }
        let killed = handle_control(&mut executor, &procs, &shared, &tb, Command::Kill);
        assert!(killed[0].result.is_ok());

        // Failures are reported, not panicked on.
        let system = ProcessRef {
            pid: 4,
            create_time: 0,
            name: "System".into(),
        };
        let refused = handle_control(&mut executor, &procs, &shared, &system, Command::Suspend);
        assert!(refused[0].result.is_err());
        let events = shared.events.after(0);
        assert!(
            events
                .iter()
                .any(|e| e.message.starts_with("Manual: Suspend failed"))
        );
    }

    #[test]
    fn manual_choices_outlast_rollbacks_and_unlimit_reports_nothing_to_do() {
        let a = spawn();
        let procs = Sampler::new().sample().unwrap().processes;
        let t = ProcessRef::from(procs.iter().find(|p| p.pid == a.0.id()).unwrap());
        let shared = Shared::default();
        let mut executor = Executor::new(false, shared.events.clone());
        let priority = || control::inspect(&t).unwrap().priority;

        // Nothing to remove: said so instead of reporting success.
        let r = handle_control(&mut executor, &procs, &shared, &t, Command::Unlimit);
        assert_eq!(
            r[0].result,
            Err("not limited by RustWTM in this session".into())
        );

        // The governor deprioritizes; the user then picks a priority by hand.
        executor
            .execute(&Decision::Apply {
                source: Source::Governor,
                target: t.clone(),
                action: Action::Priority(wtm_policy::PriorityLevel::Idle),
                reason: "test".into(),
            })
            .unwrap();
        let r = handle_control(&mut executor, &procs, &shared, &t, Command::Unlimit);
        let e = r[0].result.as_ref().unwrap_err();
        assert!(e.contains("Governor"), "{e}");
        let pick = Command::Priority(PriorityClass::AboveNormal);
        assert!(
            handle_control(&mut executor, &procs, &shared, &t, pick)[0]
                .result
                .is_ok()
        );
        // The governor letting go keeps the user's choice.
        executor
            .execute(&Decision::Restore {
                source: Source::Governor,
                target: t.clone(),
                reason: "test".into(),
            })
            .unwrap();
        assert_eq!(priority(), Some(PriorityClass::AboveNormal));
    }

    #[test]
    fn insights_raise_findings_and_clear_them() {
        let history = History::with(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
        let now_minute = 30_000_000i64;
        for m in 0..90 {
            history
                .connection()
                .execute(
                    "INSERT INTO app_minute (app, minute, cpu_avg, cpu_peak, mem_avg, mem_peak, processes)
                     VALUES ('hog.exe', ?1, 40.0, 60.0, 1000, 2000, 1)",
                    params![now_minute - m],
                )
                .unwrap();
        }
        let config = Config::default();
        let snap = Snapshot {
            interval: Some(Duration::from_secs(1)),
            system: wtm_core::SystemStats {
                logical_cpus: 4,
                ..Default::default()
            },
            processes: Vec::new(),
            net_traced: false,
            taken_at_ms: 0,
        };
        let shared = Shared::default();
        let mut insights = InsightPolicy::new();
        let mut executor = Executor::new(true, shared.events.clone());
        let unix = (now_minute + 1) * 60;

        evaluate_insights(
            &history,
            &config,
            &snap,
            unix,
            &mut insights,
            &mut executor,
            &shared,
        );
        let findings = shared.findings.read().unwrap().clone();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].app, "hog.exe");
        assert!(
            shared
                .events
                .after(0)
                .iter()
                .any(|e| e.notify && e.message.starts_with("hog.exe"))
        );

        // A running instance gets the finding's actions.
        let hog = wtm_core::ProcessInfo {
            pid: 999_990,
            parent_pid: 1,
            name: "hog.exe".into(),
            cpu_percent: 40.0,
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
            net_rx_rate: 0.0,
            net_tx_rate: 0.0,
            suspended: false,
            create_time: 1,
        };
        let applied = insights.tick(std::slice::from_ref(&hog), &config.insights);
        assert!(!applied.is_empty());

        // Disabled insights clear the findings and roll the actions back.
        let mut disabled = config.clone();
        disabled.insights.enabled = false;
        evaluate_insights(
            &history,
            &disabled,
            &snap,
            unix,
            &mut insights,
            &mut executor,
            &shared,
        );
        assert!(shared.findings.read().unwrap().is_empty());
        assert!(
            shared
                .events
                .after(0)
                .iter()
                .any(|e| e.pid == Some(hog.pid) && e.message.starts_with("dry-run: would restore"))
        );
    }

    #[test]
    fn unreadable_history_yields_no_findings() {
        let history = History::with(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
        history
            .connection()
            .execute_batch("DROP TABLE app_minute")
            .unwrap();
        let snap = Snapshot {
            interval: None,
            system: Default::default(),
            processes: Vec::new(),
            net_traced: false,
            taken_at_ms: 0,
        };
        let shared = Shared::default();
        let mut executor = Executor::new(true, shared.events.clone());
        evaluate_insights(
            &history,
            &Config::default(),
            &snap,
            unix_now(),
            &mut InsightPolicy::new(),
            &mut executor,
            &shared,
        );
        assert!(shared.findings.read().unwrap().is_empty());
    }

    #[test]
    fn watcher_exits_once_the_engine_is_gone() {
        let (tx, rx) = std::sync::mpsc::channel();
        drop(rx);
        let watcher = spawn_watcher(
            tx,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(true)),
        );
        // The first poll only records what is already running.
        std::thread::sleep(WATCH_INTERVAL * 2);
        let _child = spawn();
        wait_for("watcher exit", || watcher.is_finished().then_some(()));
        watcher.join().unwrap();
    }

    #[test]
    fn starts_are_watched_only_for_rules_without_a_condition() {
        assert!(!watches_starts(&Config::default()));
        let rule = |when: &str| {
            Config::from_toml(&format!(
                "[[rule]]\nname = \"r\"\nmatch = {{ name = \"a.exe\" }}\n{when}actions = [\"kill\"]\n"
            ))
            .unwrap()
        };
        assert!(!watches_starts(&rule("when = { cpu_above = 20 }\n")));
        assert!(watches_starts(&rule("")));
    }

    /// Timing: one start-watcher poll of this PC (four a second while it
    /// runs). `cargo test -p wtm-service --release -- --ignored
    /// start_watch_cost --nocapture`
    #[test]
    #[ignore = "timing"]
    fn start_watch_cost() {
        let mut watcher = StartWatcher::new();
        watcher.poll().unwrap();
        let n = 200;
        let t = Instant::now();
        for _ in 0..n {
            std::hint::black_box(watcher.poll().unwrap());
        }
        println!("start watcher poll: {:?}", t.elapsed() / n);

        // The running-process set a tick needs: built once and refilled,
        // where the executor, rescue core and engine each collected one.
        let procs = Sampler::new().sample().unwrap().processes;
        let keys = || procs.iter().map(|p| (p.pid, p.create_time));
        let n = 2000;
        let t = Instant::now();
        for _ in 0..n {
            for _ in 0..3 {
                std::hint::black_box(keys().collect::<HashSet<_>>());
            }
        }
        let before = t.elapsed() / n;
        let mut live = HashSet::new();
        let t = Instant::now();
        for _ in 0..n {
            live.clear();
            live.extend(keys());
            std::hint::black_box(&live);
        }
        println!(
            "live sets per tick ({} processes): three collected {before:?}, one refilled {:?}",
            procs.len(),
            t.elapsed() / n
        );
    }

    #[test]
    fn path_lookups_are_cached_per_process_instance() {
        let child = spawn();
        let procs = Sampler::new().sample().unwrap().processes;
        let p = procs.iter().find(|p| p.pid == child.0.id()).unwrap();
        let mut cache = PathCache::new();
        let path = cached_path(&mut cache, p).unwrap();
        assert!(path.to_lowercase().ends_with("cmd.exe"), "{path}");
        drop(child);
        // Served from the cache although the process is gone.
        assert_eq!(cached_path(&mut cache, p), Some(path));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn status_reflects_config_and_executor() {
        let config = Config::default();
        let paths = Paths::under(Path::new(r"C:\x"));
        let executor = Executor::new(true, Default::default());
        let rescue = Rescue::new(config.reserve.clone(), true, Default::default());
        let game = GameMode::new(config.game_mode.clone());
        let s = status(
            &config,
            &paths,
            &executor,
            &[],
            Some("game.exe"),
            &rescue,
            &game,
        );
        assert!(s.game.is_none());
        assert_eq!(s.foreground.as_deref(), Some("game.exe"));
        assert!(s.rescue_core.is_none());
        assert!(s.dry_run);
        assert_eq!(s.config_path, r"C:\x\config.toml");
        assert!(s.managed.is_empty());
        assert!(default_data_dir().ends_with("RustWTM"));
        assert!(unix_now() > 1_700_000_000);
    }

    #[test]
    fn foreground_report_covers_the_app_and_its_children_until_stale() {
        let snap = Sampler::new().sample().unwrap();
        let me = snap
            .processes
            .iter()
            .find(|p| p.pid == std::process::id())
            .unwrap();
        let (keys, app) = foreground_keys(Some((me.pid, Instant::now())), &snap.processes);
        assert!(keys.contains(&(me.pid, me.create_time)));
        assert_eq!(app.as_deref(), Some(me.name.as_str()));
        let stale = Instant::now().checked_sub(FOREGROUND_TTL * 2).unwrap();
        assert!(
            foreground_keys(Some((me.pid, stale)), &snap.processes)
                .0
                .is_empty()
        );
        assert!(
            foreground_keys(Some((0xFFFF_FFF0, Instant::now())), &snap.processes)
                .1
                .is_none()
        );
        assert!(foreground_keys(None, &snap.processes).0.is_empty());
    }
}

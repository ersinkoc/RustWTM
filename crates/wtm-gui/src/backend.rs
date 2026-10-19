//! Background worker: reads from the service when it runs, samples locally
//! otherwise. The UI thread only reads [`Data`] and sends [`UiCommand`]s.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;
use wtm_core::control::{self, ControlResult, Limiter, ProcessDetails, ProcessRef};
use wtm_core::startup::StartupEntry;
use wtm_core::{Connection, ServiceEntry, Session, StartType};
use wtm_core::{Sampler, Snapshot};
use wtm_policy::{AppStats, Finding, MinuteRow};

/// Per-app summaries plus the service's current long-term findings.
pub type AppHistory = (Vec<AppStats>, Vec<Finding>);

const NO_SERVICE: &str = "Usage history is recorded by the RustWTM service, which is not running.";
use wtm_ipc::{Client, Command, Event, EventLevel, Outcome, Request, Response, Status};

const TICK: Duration = Duration::from_secs(1);
const RECONNECT: Duration = Duration::from_secs(5);
pub const HISTORY_LEN: usize = 300;
const EVENTS_KEPT: usize = 2000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Starting,
    /// Connected to the engine; `admin` means it accepts control requests,
    /// `verified` that it is the installed service (not a console instance).
    Service { admin: bool, verified: bool },
    /// No service: sampling and controlling with this process's own rights.
    Local,
}

#[derive(Debug, Default)]
pub struct History {
    pub cpu: VecDeque<f32>,
    pub memory: VecDeque<f32>,
    pub gpu: VecDeque<f32>,
}

#[derive(Debug, Default)]
pub struct Data {
    pub mode: Mode,
    pub snapshot: Option<Arc<Snapshot>>,
    pub status: Option<Status>,
    pub events: VecDeque<Event>,
    pub history: History,
    pub details: Option<(ProcessRef, Result<ProcessDetails, String>)>,
    pub services: Option<Result<Vec<ServiceEntry>, String>>,
    pub sessions: Option<Result<Vec<Session>, String>>,
    pub connections: Option<Result<Vec<Connection>, String>>,
    pub startup: Option<Vec<StartupEntry>>,
    pub app_history: Option<Result<AppHistory, String>>,
    /// Per-minute rows for the app selected in App history.
    pub series: Option<(String, Vec<MinuteRow>)>,
    pub message: Option<(EventLevel, String)>,
}

pub enum UiCommand {
    Control {
        target: ProcessRef,
        command: Command,
    },
    Inspect(ProcessRef),
    /// Refresh one of the on-demand lists.
    Load(Panel),
    LoadHistory {
        hours: u32,
    },
    SetStartup {
        entry: StartupEntry,
        enabled: bool,
    },
    LoadSeries {
        app: String,
        hours: u32,
    },
    Service {
        name: String,
        op: ServiceOp,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Panel {
    Services,
    Users,
    Network,
    Startup,
    /// Only used for refresh timing; loaded via `LoadHistory`.
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceOp {
    Start,
    Stop,
    StartType(StartType),
}

pub struct Backend {
    data: Arc<Mutex<Data>>,
    tx: Sender<UiCommand>,
    /// Show desktop notifications for notable service events.
    pub notifications: Arc<AtomicBool>,
}

impl Backend {
    pub fn start(ctx: egui::Context) -> Self {
        let data = Arc::new(Mutex::new(Data::default()));
        let (tx, rx) = mpsc::channel();
        let notifications = Arc::new(AtomicBool::new(true));
        let mut worker = Worker {
            data: data.clone(),
            ctx,
            notifications: notifications.clone(),
            client: None,
            next_connect: Instant::now(),
            last_event: 0,
            sampler: Sampler::new(),
            limiter: Limiter::new(),
        };
        std::thread::Builder::new()
            .name("backend".into())
            .spawn(move || worker.run(&rx))
            .expect("spawn backend thread");
        Self {
            data,
            tx,
            notifications,
        }
    }

    pub fn data(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn send(&self, command: UiCommand) {
        let _ = self.tx.send(command);
    }
}

struct Worker {
    data: Arc<Mutex<Data>>,
    ctx: egui::Context,
    notifications: Arc<AtomicBool>,
    client: Option<Client>,
    next_connect: Instant,
    last_event: u64,
    sampler: Sampler,
    /// Job handles for limits applied in local mode.
    limiter: Limiter,
}

fn unexpected(r: Response) -> io::Error {
    io::Error::other(match r {
        Response::Error(e) => e,
        other => format!("unexpected response: {other:?}"),
    })
}

impl Worker {
    fn data(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn run(&mut self, rx: &Receiver<UiCommand>) {
        let mut next_tick = Instant::now();
        loop {
            match rx.recv_timeout(next_tick.saturating_duration_since(Instant::now())) {
                Ok(cmd) => {
                    self.handle(cmd);
                    self.ctx.request_repaint();
                    continue;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            next_tick = Instant::now() + TICK;
            self.tick();
            self.ctx.request_repaint();
        }
    }

    fn tick(&mut self) {
        if self.client.is_none() && Instant::now() >= self.next_connect {
            match Client::connect() {
                Ok(c) => {
                    self.client = Some(c);
                    self.last_event = 0;
                }
                Err(_) => self.next_connect = Instant::now() + RECONNECT,
            }
        }

        if let Some(client) = self.client.as_mut() {
            let admin = client.is_admin();
            let verified = client.is_verified();
            match fetch(client, self.last_event) {
                Ok((snapshot, status, events)) => {
                    // Skip the backlog fetched right after connecting.
                    if self.last_event != 0 && self.notifications.load(Ordering::Relaxed) {
                        events.iter().filter(|e| e.notify).for_each(notify);
                    }
                    if let Some(last) = events.last() {
                        self.last_event = last.id;
                    }
                    let mut data = self.data();
                    data.mode = Mode::Service { admin, verified };
                    data.status = Some(status);
                    data.events.extend(events);
                    if let Some(s) = snapshot {
                        publish(&mut data, s);
                    }
                    return;
                }
                Err(e) => {
                    self.client = None;
                    self.next_connect = Instant::now() + RECONNECT;
                    self.data().message =
                        Some((EventLevel::Warn, format!("Lost connection to service: {e}")));
                }
            }
        }

        match self.sampler.sample() {
            Ok(s) => {
                let mut data = self.data();
                data.mode = Mode::Local;
                data.status = None;
                publish(&mut data, s);
            }
            Err(e) => {
                self.data().message = Some((EventLevel::Error, format!("Sampling failed: {e}")));
            }
        }
    }

    fn handle(&mut self, cmd: UiCommand) {
        match cmd {
            UiCommand::Load(panel) => self.load(panel),
            UiCommand::SetStartup { entry, enabled } => {
                let verb = if enabled { "Enabled" } else { "Disabled" };
                let result = wtm_core::startup::set_startup_enabled(&entry, enabled);
                self.data().message = Some(match result {
                    Ok(()) => (
                        EventLevel::Info,
                        format!("{verb} startup entry {}", entry.name),
                    ),
                    Err(e) if e.kind() == io::ErrorKind::PermissionDenied => (
                        EventLevel::Warn,
                        format!(
                            "{}: access denied (all-users entries need administrator)",
                            entry.name
                        ),
                    ),
                    Err(e) => (EventLevel::Warn, format!("{}: {e}", entry.name)),
                });
                self.load(Panel::Startup);
            }
            UiCommand::LoadHistory { hours } => {
                let r = match self.client.as_mut() {
                    None => Err(NO_SERVICE.to_owned()),
                    Some(c) => match c.call(&Request::History { hours }) {
                        Ok(Response::History { stats, findings }) => Ok((stats, findings)),
                        Ok(other) => Err(unexpected(other).to_string()),
                        Err(e) => Err(e.to_string()),
                    },
                };
                self.data().app_history = Some(r);
            }
            UiCommand::LoadSeries { app, hours } => {
                if let Some(c) = self.client.as_mut()
                    && let Ok(Response::Series(rows)) = c.call(&Request::AppSeries {
                        app: app.clone(),
                        hours,
                    })
                {
                    self.data().series = Some((app, rows));
                }
            }
            UiCommand::Service { name, op } => {
                let result = match op {
                    ServiceOp::Start => wtm_core::start_service(&name),
                    ServiceOp::Stop => wtm_core::stop_service(&name),
                    ServiceOp::StartType(t) => wtm_core::set_start_type(&name, t),
                };
                self.data().message = Some(match result {
                    Ok(()) => (EventLevel::Info, format!("{op:?}: {name}")),
                    Err(e) if e.is_access_denied() => (
                        EventLevel::Warn,
                        format!("{op:?} {name}: access denied (run RustWTM as administrator)"),
                    ),
                    Err(e) => (EventLevel::Warn, format!("{op:?} {name} failed: {e}")),
                });
                self.load(Panel::Services);
            }
            UiCommand::Inspect(target) => {
                let result = match self.client.as_mut() {
                    Some(c) => match c.call(&Request::Inspect(target.clone())) {
                        Ok(Response::Details(d)) => Ok(d),
                        Ok(other) => Err(unexpected(other).to_string()),
                        Err(e) => Err(e.to_string()),
                    },
                    None => control::inspect(&target).map_err(|e| e.to_string()),
                };
                self.data().details = Some((target, result));
            }
            UiCommand::Control { target, command } => {
                let via_service = self.client.as_ref().is_some_and(Client::is_admin);
                let outcomes = if via_service {
                    let client = self.client.as_mut().expect("checked above");
                    match client.call(&Request::Control {
                        target: target.clone(),
                        command,
                    }) {
                        Ok(Response::Control(o)) => o,
                        Ok(other) => vec![failed(&target, unexpected(other).to_string())],
                        Err(e) => vec![failed(&target, e.to_string())],
                    }
                } else {
                    self.run_local(&target, command)
                };
                self.report(command, &outcomes, !via_service);
                // Refresh details so the side panel reflects the change.
                self.handle(UiCommand::Inspect(target));
            }
        }
    }

    fn load(&mut self, panel: Panel) {
        match panel {
            Panel::Services => {
                let r = wtm_core::list_services().map_err(|e| e.to_string());
                self.data().services = Some(r);
            }
            Panel::Users => {
                let r = wtm_core::sessions().map_err(|e| e.to_string());
                self.data().sessions = Some(r);
            }
            Panel::History => {}
            Panel::Startup => {
                self.data().startup = Some(wtm_core::startup::list_startup());
            }
            Panel::Network => {
                let r = wtm_core::connections().map_err(|e| e.to_string());
                self.data().connections = Some(r);
            }
        }
    }

    fn run_local(&mut self, t: &ProcessRef, command: Command) -> Vec<Outcome> {
        let one = |r: ControlResult<()>| {
            vec![Outcome {
                target: t.clone(),
                result: r.map_err(|e| e.to_string()),
            }]
        };
        match command {
            Command::Kill => one(control::kill(t)),
            Command::KillTree => {
                let snapshot = self.data().snapshot.clone();
                let procs = snapshot.as_deref().map_or(&[][..], |s| &s.processes);
                control::kill_tree(t, procs)
                    .into_iter()
                    .map(|(target, r)| Outcome {
                        target,
                        result: r.map_err(|e| e.to_string()),
                    })
                    .collect()
            }
            Command::Suspend => one(control::suspend(t)),
            Command::Resume => one(control::resume(t)),
            Command::Priority(p) => one(control::set_priority(t, p)),
            Command::Affinity(mask) => one(control::set_affinity(t, mask)),
            Command::EcoQos(mode) => one(control::set_eco_qos(t, mode)),
            Command::Limit(limits) => one(self.limiter.apply(t, limits)),
            Command::Unlimit => one(self.limiter.lift(t)),
        }
    }

    fn report(&self, command: Command, outcomes: &[Outcome], local: bool) {
        let mut data = self.data();
        let failures: Vec<&Outcome> = outcomes.iter().filter(|o| o.result.is_err()).collect();
        data.message = Some(match failures.first() {
            None => (
                EventLevel::Info,
                format!("{} — {}", describe(command), names(outcomes)),
            ),
            Some(o) => (
                EventLevel::Warn,
                format!(
                    "{} failed for {} ({}): {}",
                    describe(command),
                    o.target.name,
                    o.target.pid,
                    o.result.as_ref().err().map_or("", String::as_str)
                ),
            ),
        });
        if local {
            // The service logs its own events; record local actions ourselves.
            let unix_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            for o in outcomes {
                data.events.push_back(Event {
                    id: 0,
                    unix_ms,
                    level: if o.result.is_ok() {
                        EventLevel::Info
                    } else {
                        EventLevel::Warn
                    },
                    pid: Some(o.target.pid),
                    name: Some(o.target.name.clone()),
                    message: match &o.result {
                        Ok(()) => format!("Local: {}", describe(command)),
                        Err(e) => format!("Local: {} failed: {e}", describe(command)),
                    },
                    notify: false,
                });
            }
            trim(&mut data.events, EVENTS_KEPT);
        }
    }
}

fn failed(target: &ProcessRef, error: String) -> Outcome {
    Outcome {
        target: target.clone(),
        result: Err(error),
    }
}

fn names(outcomes: &[Outcome]) -> String {
    match outcomes {
        [one] => format!("{} ({})", one.target.name, one.target.pid),
        many => format!("{} processes", many.len()),
    }
}

pub fn describe(command: Command) -> String {
    match command {
        Command::Kill => "End task".into(),
        Command::KillTree => "End process tree".into(),
        Command::Suspend => "Suspend".into(),
        Command::Resume => "Resume".into(),
        Command::Priority(p) => format!("Priority {p:?}"),
        Command::Affinity(mask) => format!("Affinity {mask:#x}"),
        Command::EcoQos(mode) => format!("Efficiency mode {mode:?}"),
        Command::Limit(l) => {
            let mut parts = Vec::new();
            if let Some(c) = l.cpu_percent {
                parts.push(format!("CPU ≤ {c:.2}%"));
            }
            if let Some(m) = l.memory_bytes {
                parts.push(format!("memory ≤ {}", wtm_core::format_bytes(m as f64)));
            }
            format!("Limit {}", parts.join(", "))
        }
        Command::Unlimit => "Remove limits".into(),
    }
}

type Fetched = (Option<Snapshot>, Status, Vec<Event>);

fn fetch(client: &mut Client, after: u64) -> io::Result<Fetched> {
    let snapshot = match client.call(&Request::Snapshot)? {
        Response::Snapshot(s) => Some(s),
        // The service has not sampled yet.
        Response::Error(_) => None,
        other => return Err(unexpected(other)),
    };
    let status = match client.call(&Request::Status)? {
        Response::Status(s) => s,
        other => return Err(unexpected(other)),
    };
    let events = match client.call(&Request::Events { after })? {
        Response::Events(e) => e,
        other => return Err(unexpected(other)),
    };
    Ok((snapshot, status, events))
}

fn publish(data: &mut Data, snapshot: Snapshot) {
    let s = &snapshot.system;
    let mem = if s.memory_total > 0 {
        s.memory_used() as f32 / s.memory_total as f32 * 100.0
    } else {
        0.0
    };
    // The very first local sample has no interval and reads 0% everywhere.
    if snapshot.interval.is_some() {
        data.history.cpu.push_back(s.cpu_percent);
        data.history.memory.push_back(mem);
        data.history.gpu.push_back(s.gpu_percent);
        trim(&mut data.history.cpu, HISTORY_LEN);
        trim(&mut data.history.memory, HISTORY_LEN);
        trim(&mut data.history.gpu, HISTORY_LEN);
    }
    trim(&mut data.events, EVENTS_KEPT);
    data.snapshot = Some(Arc::new(snapshot));
}

fn trim<T>(q: &mut VecDeque<T>, max: usize) {
    while q.len() > max {
        q.pop_front();
    }
}

fn notify(e: &Event) {
    let title = match (&e.name, e.pid) {
        (Some(name), Some(pid)) => format!("RustWTM: {name} ({pid})"),
        _ => "RustWTM".to_owned(),
    };
    let _ = notify_rust::Notification::new()
        .app_id(APP_ID)
        .appname("RustWTM")
        .summary(&title)
        .body(&e.message)
        .show();
}

/// AppUserModelID used for toast notifications.
const APP_ID: &str = "RustWTM.TaskManager";

/// Registers the AppUserModelID under HKCU so notifications show "RustWTM"
/// as their source. Per-user, no elevation needed; failures are harmless.
pub fn register_app_id() {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let path = format!(r"Software\Classes\AppUserModelId\{APP_ID}");
    if let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(path) {
        let _ = key.set_value("DisplayName", &"RustWTM");
    }
}

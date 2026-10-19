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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command as Proc, Stdio};

    use wtm_core::control::Limits;
    use wtm_ipc::{PROTOCOL_VERSION, read_frame, write_frame};

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

    fn worker() -> Worker {
        Worker {
            data: Arc::new(Mutex::new(Data::default())),
            ctx: egui::Context::default(),
            notifications: Arc::new(AtomicBool::new(false)),
            client: None,
            // Never try the real service pipe from tests.
            next_connect: Instant::now() + Duration::from_secs(3600),
            last_event: 0,
            sampler: Sampler::new(),
            limiter: Limiter::new(),
        }
    }

    fn target_of(w: &Worker, pid: u32) -> ProcessRef {
        let data = w.data();
        let snap = data.snapshot.as_ref().unwrap();
        ProcessRef::from(snap.processes.iter().find(|p| p.pid == pid).unwrap())
    }

    fn message(w: &Worker) -> (EventLevel, String) {
        w.data().message.clone().unwrap()
    }

    #[test]
    fn local_mode_samples_loads_and_runs_commands() {
        let mut w = worker();
        w.tick();
        w.tick();
        {
            let d = w.data();
            assert_eq!(d.mode, Mode::Local);
            assert!(d.snapshot.is_some());
            assert_eq!(d.history.cpu.len(), 1, "first sample has no interval");
        }
        for panel in [
            Panel::Services,
            Panel::Users,
            Panel::Network,
            Panel::Startup,
            Panel::History,
        ] {
            w.handle(UiCommand::Load(panel));
        }
        {
            let d = w.data();
            assert!(matches!(d.services, Some(Ok(_))));
            assert!(matches!(d.sessions, Some(Ok(_))));
            assert!(matches!(d.connections, Some(Ok(_))));
            assert!(d.startup.is_some());
        }
        w.handle(UiCommand::LoadHistory { hours: 1 });
        assert!(matches!(&w.data().app_history, Some(Err(e)) if e == NO_SERVICE));
        w.handle(UiCommand::LoadSeries {
            app: "x".into(),
            hours: 1,
        });
        assert!(w.data().series.is_none());

        let child = spawn();
        w.tick();
        let t = target_of(&w, child.0.id());
        for command in [
            Command::Suspend,
            Command::Resume,
            Command::Priority(wtm_core::control::PriorityClass::BelowNormal),
            Command::Affinity(1),
            Command::EcoQos(wtm_core::control::EcoQos::On),
            Command::Limit(Limits {
                cpu_percent: Some(50.0),
                memory_bytes: Some(1 << 30),
            }),
            Command::Unlimit,
        ] {
            w.handle(UiCommand::Control {
                target: t.clone(),
                command,
            });
            assert_eq!(
                message(&w).0,
                EventLevel::Info,
                "{command:?}: {:?}",
                message(&w)
            );
        }
        assert!(matches!(&w.data().details, Some((r, Ok(_))) if *r == t));
        w.handle(UiCommand::Control {
            target: t.clone(),
            command: Command::KillTree,
        });
        assert_eq!(message(&w).0, EventLevel::Info);
        assert!(
            w.data()
                .events
                .iter()
                .any(|e| e.message.starts_with("Local: End process tree"))
        );

        // A protected process is refused with a warning.
        let system = ProcessRef {
            pid: 4,
            create_time: 0,
            name: "System".into(),
        };
        w.handle(UiCommand::Control {
            target: system,
            command: Command::Kill,
        });
        assert_eq!(message(&w).0, EventLevel::Warn);

        // Service operations without rights or on unknown services warn.
        w.handle(UiCommand::Service {
            name: "NoSuchServiceForRustWTMTests".into(),
            op: ServiceOp::Start,
        });
        assert_eq!(message(&w).0, EventLevel::Warn);
    }

    /// Answers like the service would, until the client disconnects.
    fn fake_server(pipe: String, snapshot: Snapshot) -> std::thread::JoinHandle<()> {
        let mut listener = wtm_sys::pipe::PipeListener::bind(&pipe, wtm_ipc::PIPE_SDDL).unwrap();
        std::thread::spawn(move || {
            let mut conn = listener.accept().unwrap();
            let target = ProcessRef {
                pid: 1,
                create_time: 1,
                name: "x.exe".into(),
            };
            while let Ok(request) = read_frame::<Request>(&mut conn) {
                let response = match request {
                    Request::Hello { .. } => Response::Hello {
                        version: PROTOCOL_VERSION,
                        admin: true,
                    },
                    Request::Snapshot => Response::Snapshot(snapshot.clone()),
                    Request::Status => Response::Status(Status::default()),
                    Request::Events { .. } => Response::Events(vec![Event {
                        id: 7,
                        unix_ms: 0,
                        level: EventLevel::Info,
                        pid: None,
                        name: None,
                        message: "from service".into(),
                        notify: true,
                    }]),
                    Request::Inspect(_) => Response::Error("denied".into()),
                    Request::Control { .. } => Response::Control(vec![Outcome {
                        target: target.clone(),
                        result: Ok(()),
                    }]),
                    Request::History { .. } => Response::History {
                        stats: Vec::new(),
                        findings: Vec::new(),
                    },
                    Request::AppSeries { .. } => Response::Series(Vec::new()),
                };
                if write_frame(&mut conn, &response).is_err() {
                    break;
                }
            }
        })
    }

    #[test]
    fn service_mode_fetches_controls_and_falls_back_when_lost() {
        let pipe = format!(r"\\.\pipe\wtm-gui-test-{}", std::process::id());
        let snapshot = Sampler::new().sample().unwrap();
        let server = fake_server(pipe.clone(), snapshot);

        let mut w = worker();
        w.client = Some(Client::connect_to(&pipe).unwrap());
        w.tick();
        {
            let d = w.data();
            assert_eq!(
                d.mode,
                Mode::Service {
                    admin: true,
                    verified: false
                }
            );
            assert!(d.status.is_some());
            assert_eq!(d.events.len(), 1);
        }
        assert_eq!(w.last_event, 7);

        let t = ProcessRef {
            pid: 1,
            create_time: 1,
            name: "x.exe".into(),
        };
        w.handle(UiCommand::Control {
            target: t.clone(),
            command: Command::Suspend,
        });
        assert_eq!(message(&w).0, EventLevel::Info);
        // Inspect after the command went through the service and was refused.
        assert!(matches!(&w.data().details, Some((_, Err(e))) if e == "denied"));
        w.handle(UiCommand::LoadHistory { hours: 24 });
        assert!(matches!(w.data().app_history, Some(Ok(_))));
        w.handle(UiCommand::LoadSeries {
            app: "x.exe".into(),
            hours: 24,
        });
        assert!(w.data().series.is_some());

        drop(w.client.take());
        server.join().unwrap();

        // A server that hangs up right after the handshake: the next tick
        // must fall back to local sampling and warn.
        let mut listener = wtm_sys::pipe::PipeListener::bind(&pipe, wtm_ipc::PIPE_SDDL).unwrap();
        let hangup = std::thread::spawn(move || {
            let mut conn = listener.accept().unwrap();
            let _ = read_frame::<Request>(&mut conn);
            let _ = write_frame(
                &mut conn,
                &Response::Hello {
                    version: PROTOCOL_VERSION,
                    admin: false,
                },
            );
        });
        w.client = Some(Client::connect_to(&pipe).unwrap());
        hangup.join().unwrap();
        w.tick();
        assert_eq!(w.data().mode, Mode::Local);
        assert!(w.client.is_none());
        let (level, text) = message(&w);
        assert_eq!(level, EventLevel::Warn);
        assert!(text.starts_with("Lost connection to service"), "{text}");
    }
}

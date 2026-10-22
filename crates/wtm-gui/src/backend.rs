//! Background worker: reads from the service when it runs, samples locally
//! otherwise. The UI thread only reads [`Data`] and sends [`UiCommand`]s.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;
use wtm_core::control::{self, ControlResult, Limiter, ProcessDetails, ProcessRef};
use wtm_core::startup::StartupEntry;
use wtm_core::{Connection, ServiceEntry, Session, StartType};
use wtm_core::{Sampler, Snapshot};
use wtm_policy::{AppStats, Finding, MinuteRow};

pub type CommandLines = std::collections::HashMap<(u32, i64), String>;
/// A process's modules with who signed each.
pub type ModuleList = Vec<(wtm_core::Module, wtm_core::Signature)>;
/// Processes with a window Windows marks as not responding, and its title.
pub type HungMap = std::collections::HashMap<u32, String>;

/// Per-app summaries plus the service's current long-term findings.
pub type AppHistory = (Vec<AppStats>, Vec<Finding>);

const NO_SERVICE: &str = "Usage history is recorded by the RustWTM service";
const NO_CONFIG: &str = "The configuration belongs to the RustWTM service";
const NO_TELEGRAM: &str = "The Telegram bot runs in the RustWTM service";

/// Why `what` (one of the constants above) is unavailable: the service is not
/// running, or it is another version that this window cannot talk to.
fn without_service(data: &Mutex<Data>, what: &str) -> String {
    let what = crate::i18n::tr(what);
    let data = data.lock().unwrap_or_else(PoisonError::into_inner);
    let error = data.service_error.as_deref().unwrap_or_default();
    match service_problem(data.service_error.as_deref()) {
        ServiceProblem::Older => {
            let __wtm_i18n_0 = &(env!("CARGO_PKG_VERSION"));
            crate::i18n::format(
                "{0}, but the installed service is a different version ({1}). Update it with the installer of version {2}.",
                &[
                    what.to_string(),
                    error.to_string(),
                    __wtm_i18n_0.to_string(),
                ],
            )
        }
        ServiceProblem::Newer => crate::i18n::format(
            "{0}, but the installed service is newer than this window ({1}). Restart RustWTM, or update it with the latest installer.",
            &[what.to_string(), error.to_string()],
        ),
        ServiceProblem::Impostor => crate::i18n::format(
            "{0}, but another program answers in its place, so RustWTM does not use it.",
            &[what.to_string()],
        ),
        ServiceProblem::NotRunning => {
            crate::i18n::format("{0}, which is not running.", &[what.to_string()])
        }
    }
}

/// Why the window works without the service, from the last connection error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceProblem {
    NotRunning,
    /// The service is an older version: update it.
    Older,
    /// The service is newer than this window: restart or update the window.
    Newer,
    /// Another process serves the pipe in place of the installed service.
    Impostor,
}

/// Reads the error of `wtm_ipc::Client::connect`; its handshake reports
/// "service speaks protocol {service}, client {ours}".
pub fn service_problem(error: Option<&str>) -> ServiceProblem {
    let Some(error) = error else {
        return ServiceProblem::NotRunning;
    };
    if error.contains("not served by the RustWTM service") {
        return ServiceProblem::Impostor;
    }
    if let Some(rest) = error.strip_prefix("service speaks protocol ")
        && let Some((service, client)) = rest.split_once(", client ")
        && let (Ok(service), Ok(client)) = (service.parse::<u32>(), client.parse::<u32>())
    {
        return if service > client {
            ServiceProblem::Newer
        } else {
            ServiceProblem::Older
        };
    }
    if error.contains("protocol") || error.contains("handshake") {
        return ServiceProblem::Older;
    }
    ServiceProblem::NotRunning
}
use wtm_ipc::{Client, Command, Event, EventLevel, Outcome, Request, Response, Status};

const TICK: Duration = Duration::from_secs(1);
/// The window's own disk trace stops when nothing read it for this long.
const LOCAL_DISK_TRACE_IDLE: Duration = Duration::from_secs(30);
/// How long closing the window waits for the worker to finish, so the trace
/// sessions it runs are stopped rather than left to the next sweep.
const WORKER_EXIT_WAIT: Duration = Duration::from_secs(3);
const RECONNECT: Duration = Duration::from_secs(5);
pub const HISTORY_LEN: usize = 300;
/// Seconds of CPU and memory kept for every running process.
pub const PROCESS_HISTORY_LEN: usize = 120;
/// Seconds of history in each logical processor's small graph.
pub const CORE_HISTORY_LEN: usize = 60;
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
    /// When each point was measured, in Unix milliseconds: one per entry of
    /// every series below, and of the last entries of `cores`.
    pub times: VecDeque<u64>,
    pub cpu: VecDeque<f32>,
    pub memory: VecDeque<f32>,
    pub gpu: VecDeque<f32>,
    /// Bytes per second.
    pub net_rx: VecDeque<f32>,
    pub net_tx: VecDeque<f32>,
    pub io_read: VecDeque<f32>,
    pub io_write: VecDeque<f32>,
    /// Load of every logical processor, last `CORE_HISTORY_LEN` samples.
    pub cores: Vec<VecDeque<f32>>,
}

/// Recent samples of one process, oldest first.
#[derive(Debug, Default)]
pub struct ProcessHistory {
    /// When each sample was measured, in Unix milliseconds.
    pub times: VecDeque<u64>,
    pub cpu: VecDeque<f32>,
    /// Private working set, bytes.
    pub memory: VecDeque<f32>,
}

pub type ProcessHistories = std::collections::HashMap<(u32, i64), ProcessHistory>;

#[derive(Debug, Default)]
pub struct Data {
    /// Receipt time in this GUI.
    pub sample_received_at: Option<Instant>,
    /// When the shown sample was measured, on this GUI's clock. A sample
    /// received again from the service's cache keeps its old time.
    pub sample_measured_at: Option<Instant>,
    /// Latest sampling failure; cleared only by a successful snapshot.
    pub sample_error: Option<String>,
    pub frequency: Option<Result<wtm_core::power::FrequencyReport, String>>,
    pub cpu_topology: Option<wtm_core::cpu::Topology>,
    pub hardware: wtm_core::hardware::HardwareReport,
    pub mode: Mode,
    pub snapshot: Option<Arc<Snapshot>>,
    pub status: Option<Status>,
    pub events: VecDeque<Event>,
    pub history: History,
    /// Per process, so a selected process shows its recent past at once.
    pub process_history: ProcessHistories,
    pub details: Option<(ProcessRef, Result<ProcessDetails, String>)>,
    /// Processes whose window does not respond, refreshed every sample.
    pub hung: HungMap,
    /// Counts the updates of the snapshot, status, command lines, image
    /// paths and hung windows, so the window can keep what it derives from
    /// them until the next one instead of redoing it every frame.
    pub revision: u64,
    /// The last wait chain analysis; `None` inside while it runs.
    pub wait_report: Option<(ProcessRef, Option<Result<wtm_core::WaitReport, String>>)>,
    /// The loaded modules of a process with their signatures; `None`
    /// inside while they are read.
    pub modules: Option<(ProcessRef, Option<Result<ModuleList, String>>)>,
    /// Programs blocked from the network with Windows Firewall.
    pub firewall: Option<Result<Vec<String>, String>>,
    /// Disk bytes per process and file over a few seconds.
    pub disk_activity: Option<Result<(Vec<wtm_core::FileIo>, f64), String>>,
    /// The last "why won't it end" report; `None` inside while it runs.
    pub end_report: Option<(ProcessRef, Option<Result<wtm_core::EndReport, String>>)>,
    pub services: Option<Result<Vec<ServiceEntry>, String>>,
    pub sessions: Option<Result<Vec<Session>, String>>,
    pub connections: Option<Result<Vec<Connection>, String>>,
    pub startup: Option<Vec<StartupEntry>>,
    pub app_history: Option<Result<AppHistory, String>>,
    /// Per-minute rows for the app selected in App history.
    pub series: Option<(String, Vec<MinuteRow>)>,
    /// Command lines per process instance, read with this window's own
    /// rights (empty when not readable). The service never sends them.
    pub command_lines: CommandLines,
    /// Image path per process instance (empty when not readable).
    pub image_paths: CommandLines,
    /// Small icon per image path; `None` when the file has none.
    pub icons: std::collections::HashMap<String, Option<Arc<wtm_core::IconImage>>>,
    /// The service's configuration file: (path, text).
    pub config: Option<Result<(String, String), String>>,
    /// Processes to announce with a notification when they exit, and since
    /// when.
    pub exit_watches: Vec<(ProcessRef, Instant)>,
    /// The service's Telegram bot settings (admins only).
    pub telegram: Option<Result<wtm_policy::TelegramConfig, String>>,
    pub message: Option<(EventLevel, String)>,
    /// The GUI runs with administrator rights.
    pub elevated: bool,
    /// Pops the window up when the CPU stays overloaded.
    pub alert_watch: crate::alert::AlertWatch,
    /// The service's kill list, from the last configuration loaded.
    pub kill_list: Vec<wtm_policy::KillEntry>,
    /// When many copies of one program count as a swarm (the service's
    /// setting once its configuration is loaded).
    pub swarm: wtm_policy::SwarmConfig,
    pub update: UpdateState,
    /// This copy was installed by the MSI, so an installer can update it.
    pub installable: bool,
    /// Why the last attempt to reach the service failed.
    pub service_error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SampleFreshness {
    Waiting,
    Current(Duration),
    Stale(Option<Duration>),
}

impl Data {
    pub fn sample_freshness(&self, now: Instant) -> SampleFreshness {
        let age = self
            .sample_measured_at
            .or(self.sample_received_at)
            .map(|at| now.saturating_duration_since(at));
        if self.sample_error.is_some() {
            return SampleFreshness::Stale(age);
        }
        let Some(age) = age else {
            return SampleFreshness::Waiting;
        };
        let allowance = self
            .snapshot
            .as_ref()
            .and_then(|s| s.interval)
            .unwrap_or(Duration::from_secs(1))
            .saturating_mul(3)
            .max(Duration::from_secs(5));
        if age >= allowance {
            SampleFreshness::Stale(Some(age))
        } else {
            SampleFreshness::Current(age)
        }
    }
}

pub enum UiCommand {
    SetFrequency(wtm_core::power::FrequencyRequest),
    RestoreFrequency,
    SetCooling {
        channel: String,
        profile: wtm_policy::CoolingProfile,
    },
    Control {
        target: ProcessRef,
        command: Command,
    },
    Inspect(ProcessRef),
    /// What the threads of a process wait for.
    WaitChain(ProcessRef),
    /// Why a process cannot be ended.
    WhyAlive(ProcessRef),
    /// The modules a process has loaded, with their signatures.
    Modules(ProcessRef),
    /// Blocks (`true`) or unblocks a program's network access.
    Firewall {
        path: String,
        block: bool,
    },
    /// Resets an IPv4 TCP connection.
    CloseConnection {
        local: std::net::SocketAddr,
        remote: std::net::SocketAddr,
    },
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
    /// Replace the service's configuration file (admin only).
    SaveConfig(String),
    /// Replace the Telegram bot settings (admin only).
    SaveTelegram(wtm_policy::TelegramConfig),
    /// Have the service send a test message to the allowed chats.
    TestTelegram,
    /// Write a full memory dump of the process to the Temp folder.
    CreateDump(ProcessRef),
    /// Notify when the process exits; again to stop watching.
    WatchExit(ProcessRef),
    /// Disconnect or sign out a logon session.
    Session {
        id: u32,
        op: SessionOp,
    },
    /// Show the process's executable in Explorer.
    OpenLocation(ProcessRef),
    /// Start a program like the Run dialog, optionally as administrator.
    Run {
        line: String,
        elevated: bool,
    },
    /// Start RustWTM again with administrator rights and close this window.
    RestartElevated,
    /// The user closed the CPU alert; stay quiet for this long.
    CloseAlert(Duration),
    /// Ask GitHub for the latest release.
    CheckUpdate,
    /// Download, verify and install this release, then restart.
    InstallUpdate(wtm_core::update::Release),
}

/// Where the update check stands.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum UpdateState {
    #[default]
    Unknown,
    Checking,
    UpToDate,
    Available(wtm_core::update::Release),
    /// Downloading and verifying, then the installer takes over.
    Installing(wtm_core::update::Release),
    Failed(String),
}

/// The steps of an update, replaceable in tests.
#[derive(Clone, Copy)]
pub struct Updater {
    pub latest: fn() -> Result<wtm_core::update::Release, String>,
    pub download: fn(
        &wtm_core::update::Release,
        &std::path::Path,
    ) -> Result<wtm_core::update::Download, String>,
    pub install: fn(&wtm_core::update::Download, &std::path::Path) -> Result<(), String>,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            latest: wtm_core::update::latest,
            download: wtm_core::update::download,
            install: wtm_core::update::install,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Panel {
    Sensors,
    Services,
    Users,
    Network,
    Startup,
    /// Only used for refresh timing; loaded via `LoadHistory`.
    History,
    /// The service's configuration file.
    Config,
    /// The Telegram bot settings.
    Telegram,
    /// Which files are read and written right now (Disks tab).
    DiskActivity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOp {
    Disconnect,
    SignOff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceOp {
    Start,
    Stop,
    Restart,
    StartType(StartType),
}

pub struct Backend {
    data: Arc<Mutex<Data>>,
    tx: Sender<UiCommand>,
    /// Show desktop notifications for notable service events.
    pub notifications: Arc<AtomicBool>,
    /// Native handle of the main window (0 until known), so it can be shown
    /// while the UI is hidden in the tray.
    pub window: Arc<AtomicIsize>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Backend {
    /// Hangs up on the worker and waits a little for it to end: its trace
    /// sessions are kernel objects that would outlive the window.
    fn drop(&mut self) {
        let (hang_up, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.tx, hang_up));
        if let Some(worker) = self.worker.take() {
            let deadline = Instant::now() + WORKER_EXIT_WAIT;
            while !worker.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}

impl Backend {
    pub fn start(ctx: egui::Context) -> Self {
        let data = Arc::new(Mutex::new(Data {
            elevated: wtm_core::launch::is_elevated(),
            installable: std::env::current_exe()
                .is_ok_and(|exe| wtm_core::update::installed_by_msi(&exe)),
            ..Data::default()
        }));
        let (tx, rx) = mpsc::channel();
        let notifications = Arc::new(AtomicBool::new(true));
        let window = Arc::new(AtomicIsize::new(0));
        let mut worker = Worker {
            local_hardware: None,
            data: data.clone(),
            ctx,
            notifications: notifications.clone(),
            window: window.clone(),
            raise: raise_window,
            calm: calm_window,
            updater: Updater::default(),
            client: None,
            pipe: wtm_ipc::PIPE_NAME.to_owned(),
            next_connect: Instant::now(),
            last_event: 0,
            notify,
            open: open_location,
            launch: wtm_core::launch::run,
            restart: wtm_core::launch::restart_elevated,
            foreground: wtm_core::foreground_fullscreen,
            sampler: local_sampler(),
            disk_trace: None,
            limiter: Limiter::new(),
            controls: None,
            sampler_paused: false,
        };
        let worker = std::thread::Builder::new()
            .name("backend".into())
            .spawn(move || worker.run(&rx))
            .expect("spawn backend thread");
        Self {
            data,
            tx,
            notifications,
            window,
            worker: Some(worker),
        }
    }

    pub fn data(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn send(&self, command: UiCommand) {
        let _ = self.tx.send(command);
    }

    /// A backend without a worker: shows `data` and hands sent commands to
    /// the returned receiver.
    #[cfg(test)]
    pub fn offline(data: Data) -> (Self, Receiver<UiCommand>) {
        let (tx, rx) = mpsc::channel();
        let backend = Self {
            data: Arc::new(Mutex::new(data)),
            tx,
            notifications: Arc::new(AtomicBool::new(true)),
            window: Arc::new(AtomicIsize::new(0)),
            worker: None,
        };
        (backend, rx)
    }
}

struct Worker {
    local_hardware: Option<wtm_core::hardware::LocalHardware>,
    data: Arc<Mutex<Data>>,
    ctx: egui::Context,
    notifications: Arc<AtomicBool>,
    window: Arc<AtomicIsize>,
    /// Puts the window in front of everything for an alert.
    raise: fn(isize),
    /// Undoes `raise` once the alert is closed.
    calm: fn(isize),
    updater: Updater,
    client: Option<Client>,
    /// Pipe of the service to connect to.
    pipe: String,
    next_connect: Instant,
    last_event: u64,
    /// Shows a desktop notification for an event.
    notify: fn(&Event),
    /// Shows a file in Explorer.
    open: fn(&str) -> io::Result<()>,
    /// Runs a command line, optionally elevated.
    launch: fn(&str, bool) -> Result<(), String>,
    /// Starts RustWTM elevated.
    restart: fn() -> Result<(), String>,
    /// The process owning the foreground window, and whether that window
    /// covers its monitor (game mode).
    foreground: fn() -> Option<(u32, bool)>,
    sampler: Sampler,
    /// Per-file disk trace when running elevated without the service, and
    /// when it was last read.
    disk_trace: Option<(wtm_core::DiskTracer, Instant)>,
    /// Job handles for limits applied in local mode.
    limiter: Limiter,
    /// Control requests for the service, sent one after another over one
    /// connection of their own (see [`Worker::control`]).
    controls: Option<Sender<(ProcessRef, Command)>>,
    /// The service measured since `sampler` last did: its next sample must
    /// not average over that time.
    sampler_paused: bool,
}

fn unexpected(r: Response) -> io::Error {
    io::Error::other(match r {
        Response::Error(e) => e,
        other => crate::i18n::format("unexpected response: {0}", &[format!("{:?}", other)]),
    })
}

/// Shown when the service hangs up, which it does every time it restarts
/// (an update, "Restart" in Services).
const RECONNECTING: &str = "The RustWTM service went away (it may be restarting); reconnecting…";

/// The warning for a connection that failed during a tick.
fn lost_connection(e: &io::Error) -> String {
    // ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED.
    let hung_up = matches!(
        e.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::UnexpectedEof
    ) || matches!(e.raw_os_error(), Some(109 | 232 | 233));
    if hung_up {
        crate::i18n::tr(RECONNECTING).to_owned()
    } else {
        crate::i18n::format("Lost connection to service: {0}", &[format!("{}", e)])
    }
}

/// Adds events fetched from the service. Right after connecting it sends its
/// whole backlog again, which replaces the service events already shown;
/// otherwise only events newer than the newest one shown are added. Local
/// events (id 0) are kept.
fn merge_events(events: &mut VecDeque<Event>, fetched: Vec<Event>, backlog: bool) {
    if backlog {
        events.retain(|e| e.id == 0);
    }
    let newest = events.iter().map(|e| e.id).max().unwrap_or(0);
    events.extend(fetched.into_iter().filter(|e| e.id > newest));
    if backlog {
        events.make_contiguous().sort_by_key(|e| e.unix_ms);
    }
    trim(events, EVENTS_KEPT);
}

/// Control errors are written for the command line ("try an elevated
/// prompt"); the window says what to do in the window instead.
fn gui_error(error: &str, elevated: bool) -> String {
    let denied = error
        .strip_prefix("access denied to process ")
        .and_then(|rest| rest.strip_suffix(" (try an elevated prompt)"));
    match denied {
        Some(pid) if elevated => crate::i18n::format(
            "access denied to process {0} (Windows protects it even from administrators)",
            &[pid.to_owned()],
        ),
        Some(pid) => crate::i18n::format(
            "access denied to process {0} (restart RustWTM as administrator)",
            &[pid.to_owned()],
        ),
        None => error.to_owned(),
    }
}

impl Worker {
    fn data(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn run(&mut self, rx: &Receiver<UiCommand>) {
        self.data().cpu_topology = wtm_core::cpu::topology().ok();
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
            self.stop_idle_disk_trace();
            self.tick();
            self.watch_cpu();
            self.check_exits();
            self.ctx.request_repaint();
        }
    }

    fn tick(&mut self) {
        if self.client.is_none() && Instant::now() >= self.next_connect {
            match Client::connect_to(&self.pipe) {
                Ok(c) => {
                    self.client = Some(c);
                    self.last_event = 0;
                    let mut data = self.data();
                    data.service_error = None;
                    // Back again: "reconnecting…" no longer applies.
                    if data.message.as_ref().map(|(_, m)| m.as_str())
                        == Some(crate::i18n::tr(RECONNECTING))
                    {
                        data.message = None;
                    }
                }
                Err(e) => {
                    self.next_connect = Instant::now() + RECONNECT;
                    self.data().service_error = Some(e.to_string());
                }
            }
        }

        if let Some(client) = self.client.as_mut() {
            let admin = client.is_admin();
            let verified = client.is_verified();
            // Lets the governor spare the app the user is working in; a
            // refusal (e.g. an exited process) is harmless.
            if let Some((pid, fullscreen)) = (self.foreground)() {
                let _ = client.call(&Request::Foreground { pid, fullscreen });
            }
            // Right after connecting: the service's last minutes, so the
            // graphs start full, and its event backlog.
            let fresh = self.last_event == 0;
            let backfill = fresh
                .then(|| match client.call(&Request::Recent) {
                    Ok(Response::Recent(points)) => Some(points),
                    _ => None,
                })
                .flatten();
            match fetch(client, self.last_event) {
                Ok((snapshot, status, events)) => {
                    // Skip the backlog fetched right after connecting.
                    if !fresh && self.notifications.load(Ordering::Relaxed) {
                        events.iter().filter(|e| e.notify).for_each(self.notify);
                    }
                    if let Some(last) = events.last() {
                        self.last_event = last.id;
                    }
                    let lookups = snapshot
                        .as_ref()
                        .map(|s| Lookups::fetch(&self.data, &s.processes))
                        .unwrap_or_default();
                    let hung = hung_map();
                    let mut data = self.data();
                    lookups.apply(&mut data);
                    data.hung = hung;
                    data.revision = data.revision.wrapping_add(1);
                    if let Some(points) = backfill
                        && data.history.cpu.len() < points.len()
                    {
                        fill_history(&mut data.history, &points);
                    }
                    data.mode = Mode::Service { admin, verified };
                    data.status = Some(status);
                    merge_events(&mut data.events, events, fresh);
                    if let Some(s) = snapshot {
                        publish(&mut data, s);
                    } else {
                        data.sample_error = Some("The service has not supplied a sample.".into());
                    }
                    drop(data);
                    refresh_icons(&self.data);
                    self.sampler_paused = true;
                    return;
                }
                Err(e) => {
                    self.client = None;
                    self.next_connect = Instant::now() + RECONNECT;
                    self.data().message = Some((EventLevel::Warn, lost_connection(&e)));
                }
            }
        }

        if std::mem::take(&mut self.sampler_paused) {
            self.sampler.restart();
        }
        let sample = self.sampler.sample();
        let lookups = sample
            .as_ref()
            .map(|s| Lookups::fetch(&self.data, &s.processes))
            .unwrap_or_default();
        let hung = hung_map();
        let mut data = self.data();
        lookups.apply(&mut data);
        data.hung = hung;
        data.revision = data.revision.wrapping_add(1);
        publish_local(&mut data, sample);
        drop(data);
        refresh_icons(&self.data);
    }

    /// Notifies about watched processes that are gone from the snapshot.
    fn check_exits(&mut self) {
        let gone: Vec<(ProcessRef, Instant)> = {
            let mut data = self.data();
            let Some(snap) = data.snapshot.clone() else {
                return;
            };
            let live: std::collections::HashSet<(u32, i64)> = snap
                .processes
                .iter()
                .map(|p| (p.pid, p.create_time))
                .collect();
            let (gone, kept) = std::mem::take(&mut data.exit_watches)
                .into_iter()
                .partition(|(t, _)| !live.contains(&(t.pid, t.create_time)));
            data.exit_watches = kept;
            gone
        };
        for (t, since) in gone {
            (self.notify)(&Event {
                id: 0,
                unix_ms: 0,
                level: EventLevel::Info,
                pid: Some(t.pid),
                name: Some(t.name.clone()),
                message: {
                    let __wtm_i18n_0 = &(t.name);
                    let __wtm_i18n_1 = &(wtm_core::format_duration(since.elapsed()));
                    crate::i18n::format(
                        "{0} has exited (watched for {1}).",
                        &[__wtm_i18n_0.to_string(), __wtm_i18n_1.to_string()],
                    )
                },
                notify: true,
            });
        }
    }

    /// Raises the CPU alert when the load stays too high.
    fn watch_cpu(&mut self) {
        let fired = {
            let mut data = self.data();
            match data.snapshot.as_ref() {
                Some(s) if s.interval.is_some() => {
                    let cpu = s.system.cpu_percent;
                    data.alert_watch.update(Instant::now(), cpu)
                }
                _ => false,
            }
        };
        if fired {
            (self.raise)(self.window.load(Ordering::Relaxed));
        }
    }

    /// Runs `job` on its own thread (it may wait for a UAC prompt) and shows
    /// the message it returns.
    fn in_background(&self, job: impl FnOnce() -> Option<(EventLevel, String)> + Send + 'static) {
        let (data, ctx) = (self.data.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            if let Some(message) = job() {
                data.lock().unwrap_or_else(PoisonError::into_inner).message = Some(message);
                ctx.request_repaint();
            }
        });
    }

    /// Runs `job` on its own thread with its own connection to the service,
    /// so a slow request does not hold up the samples of the main one.
    /// Sends a control request to the service. Requests go one after another
    /// over one connection: a burst (the panic button, ending a swarm)
    /// never exceeds the service's client limit and loses requests. Those
    /// that arrive together are reported together, failures first.
    fn control(&mut self, target: ProcessRef, command: Command) {
        let (pipe, data, ctx) = (self.pipe.clone(), self.data.clone(), self.ctx.clone());
        let controls = self.controls.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel::<(ProcessRef, Command)>();
            std::thread::spawn(move || {
                while let Ok(first) = rx.recv() {
                    // One connection per batch, closed after it: no client
                    // slot of the service is held while nothing is sent.
                    let mut client: Option<Client> = None;
                    // A batch lasts while requests keep coming in.
                    let mut pending = std::collections::VecDeque::from([first]);
                    let mut reports: Vec<(Command, Vec<Outcome>)> = Vec::new();
                    while let Some((target, command)) = pending.pop_front() {
                        // The service may wait up to seconds for the process.
                        let outcomes = run_on_service(&mut client, &pipe, &target, command);
                        let details = showing(
                            &data.lock().unwrap_or_else(PoisonError::into_inner),
                            &target,
                        )
                        .then(|| match client.as_mut() {
                            Some(c) => inspect_with(c, &target),
                            None => Err(crate::i18n::tr("The service is not reachable").into()),
                        });
                        if let Some(details) = details {
                            let mut data = data.lock().unwrap_or_else(PoisonError::into_inner);
                            if showing(&data, &target) {
                                data.details = Some((target, details));
                            }
                        }
                        match reports.last_mut() {
                            Some((c, all)) if *c == command => all.extend(outcomes),
                            _ => reports.push((command, outcomes)),
                        }
                        pending.extend(rx.try_iter());
                    }
                    // The last report sets the message: a failing one goes last.
                    reports.sort_by_key(|(_, o)| o.iter().any(|o| o.result.is_err()));
                    let mut data = data.lock().unwrap_or_else(PoisonError::into_inner);
                    for (command, outcomes) in reports {
                        report(&mut data, command, outcomes, false);
                    }
                    drop(data);
                    ctx.request_repaint();
                }
            });
            tx
        });
        let _ = controls.send((target, command));
    }

    fn aside(&self, job: impl FnOnce(io::Result<Client>, &Mutex<Data>) + Send + 'static) {
        let (pipe, data, ctx) = (self.pipe.clone(), self.data.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            job(Client::connect_to(&pipe), &data);
            ctx.request_repaint();
        });
    }

    fn handle(&mut self, cmd: UiCommand) {
        match cmd {
            UiCommand::SetFrequency(request) => {
                let result = wtm_core::power::set_frequency(&request);
                self.data().message=Some(match result {Ok(())=>(EventLevel::Info,crate::i18n::tr("Shared CPU frequency limit saved; original values are available in Restore").into()),Err(e)=>(EventLevel::Error,e)});
                self.load(Panel::Sensors);
            }
            UiCommand::RestoreFrequency => {
                let result = wtm_core::power::restore_frequency();
                self.data().message = Some(match result {
                    Ok(()) => (
                        EventLevel::Info,
                        crate::i18n::tr("Original CPU frequency settings restored").into(),
                    ),
                    Err(e) => (EventLevel::Error, e),
                });
                self.load(Panel::Sensors);
            }
            UiCommand::SetCooling { channel, profile } => {
                let result = match self.client.as_mut() {
                    Some(client) if client.is_admin() => {
                        match client.call(&Request::SetCooling { channel, profile }) {
                            Ok(Response::Done) => Ok(()),
                            Ok(other) => Err(unexpected(other).to_string()),
                            Err(e) => Err(e.to_string()),
                        }
                    }
                    _ => Err(crate::i18n::tr(
                        "Cooling needs the matching RustWTM service and an elevated window",
                    )
                    .into()),
                };
                self.data().message = Some(match result {
                    Ok(()) => (
                        EventLevel::Info,
                        crate::i18n::tr("Cooling request applied").into(),
                    ),
                    Err(e) => (EventLevel::Error, e),
                });
                self.load(Panel::Sensors);
            }
            UiCommand::Load(panel) => self.load(panel),
            UiCommand::SetStartup { entry, enabled } => {
                let result = wtm_core::startup::set_startup_enabled(&entry, enabled);
                self.data().message = Some(startup_message(&entry.name, enabled, result));
                self.load(Panel::Startup);
            }
            UiCommand::LoadHistory { hours } => {
                let r = match self.client.as_mut() {
                    None => Err(without_service(&self.data, NO_SERVICE)),
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
            UiCommand::Service {
                name,
                op: ServiceOp::Restart,
            } => {
                // Waits for the service to stop: off the worker thread.
                self.in_background(move || {
                    let result = wtm_core::restart_service(&name, Duration::from_secs(30));
                    Some(service_message(ServiceOp::Restart, &name, result))
                });
            }
            UiCommand::Service { name, op } => {
                let result = match op {
                    ServiceOp::Start => wtm_core::start_service(&name),
                    ServiceOp::Stop => wtm_core::stop_service(&name),
                    ServiceOp::Restart => unreachable!("handled above"),
                    ServiceOp::StartType(t) => wtm_core::set_start_type(&name, t),
                };
                self.data().message = Some(service_message(op, &name, result));
                self.load(Panel::Services);
            }
            UiCommand::WatchExit(target) => {
                let mut data = self.data();
                let before = data.exit_watches.len();
                data.exit_watches.retain(|(t, _)| *t != target);
                let text = if data.exit_watches.len() < before {
                    {
                        let __wtm_i18n_0 = &(target.name);
                        crate::i18n::format(
                            "No longer watching {0}",
                            std::slice::from_ref(__wtm_i18n_0),
                        )
                    }
                } else {
                    let text = {
                        let __wtm_i18n_0 = &(target.name);
                        let __wtm_i18n_1 = &(target.pid);
                        crate::i18n::format(
                            "You will be notified when {0} ({1}) exits",
                            &[__wtm_i18n_0.to_string(), format!("{}", __wtm_i18n_1)],
                        )
                    };
                    data.exit_watches.push((target, Instant::now()));
                    text
                };
                data.message = Some((EventLevel::Info, text));
            }
            UiCommand::CreateDump(target) => {
                self.in_background(move || {
                    Some(match control::create_dump(&target, &std::env::temp_dir()) {
                        Ok(path) => (EventLevel::Info, {
                            let __wtm_i18n_0 = &(target.name);
                            let __wtm_i18n_1 = &(path.display());
                            crate::i18n::format(
                                "Memory dump of {0} written to {1}",
                                &[__wtm_i18n_0.to_string(), format!("{}", __wtm_i18n_1)],
                            )
                        }),
                        Err(e) => (EventLevel::Error, {
                            let __wtm_i18n_0 = &(target.name);
                            crate::i18n::format(
                                "Cannot dump {0}: {1}",
                                &[__wtm_i18n_0.to_string(), format!("{}", e)],
                            )
                        }),
                    })
                });
            }
            UiCommand::Session { id, op } => {
                let result = match op {
                    SessionOp::Disconnect => wtm_core::disconnect_session(id),
                    SessionOp::SignOff => wtm_core::log_off_session(id),
                };
                let what = match op {
                    SessionOp::Disconnect => crate::i18n::tr("Disconnected"),
                    SessionOp::SignOff => crate::i18n::tr("Signed out"),
                };
                self.data().message = Some(match result {
                    Ok(()) => (EventLevel::Info, {
                        crate::i18n::format(
                            "{0} session {1}",
                            &[what.to_string(), format!("{}", id)],
                        )
                    }),
                    Err(e) if e.is_access_denied() => (EventLevel::Warn, {
                        crate::i18n::format(
                            "Session {0}: access denied (run RustWTM as administrator)",
                            &[format!("{}", id)],
                        )
                    }),
                    Err(e) => (EventLevel::Warn, {
                        crate::i18n::format(
                            "Session {0}: {1}",
                            &[format!("{}", id), format!("{}", e)],
                        )
                    }),
                });
                self.load(Panel::Users);
            }
            UiCommand::OpenLocation(target) => {
                let opened = control::image_path(&target)
                    .map_err(|e| e.to_string())
                    .and_then(|path| (self.open)(&path).map_err(|e| e.to_string()));
                if let Err(e) = opened {
                    self.data().message = Some((EventLevel::Error, {
                        let __wtm_i18n_0 = &(target.name);
                        crate::i18n::format(
                            "Cannot open the location of {0}: {1}",
                            &[__wtm_i18n_0.to_string(), e.to_string()],
                        )
                    }));
                }
            }
            UiCommand::Run { line, elevated } => {
                let launch = self.launch;
                self.in_background(move || match launch(&line, elevated) {
                    Ok(()) => Some((EventLevel::Info, {
                        crate::i18n::format("Started {0}", std::slice::from_ref(&line))
                    })),
                    Err(e) => Some((EventLevel::Error, {
                        crate::i18n::format(
                            "Cannot run {0}: {1}",
                            &[line.to_string(), e.to_string()],
                        )
                    })),
                });
            }
            UiCommand::RestartElevated => {
                let (restart, ctx) = (self.restart, self.ctx.clone());
                self.in_background(move || match restart() {
                    Ok(()) => {
                        crate::tray::request_exit(&ctx);
                        None
                    }
                    Err(e) => Some((EventLevel::Error, {
                        crate::i18n::format(
                            "Cannot restart as administrator: {0}",
                            std::slice::from_ref(&e),
                        )
                    })),
                });
            }
            UiCommand::CheckUpdate => {
                let (data, ctx, latest) =
                    (self.data.clone(), self.ctx.clone(), self.updater.latest);
                if updating(&data) {
                    return;
                }
                set_update(&data, UpdateState::Checking);
                std::thread::spawn(move || {
                    let state = match latest() {
                        Ok(r) if r.is_newer_than(wtm_core::update::current()) => {
                            UpdateState::Available(r)
                        }
                        Ok(_) => UpdateState::UpToDate,
                        Err(e) => UpdateState::Failed(e),
                    };
                    finish_check(&data, state);
                    ctx.request_repaint();
                });
            }
            UiCommand::InstallUpdate(release) => {
                let (data, ctx, updater) = (self.data.clone(), self.ctx.clone(), self.updater);
                if matches!(
                    data.lock().unwrap_or_else(PoisonError::into_inner).update,
                    UpdateState::Installing(_)
                ) {
                    return;
                }
                set_update(&data, UpdateState::Installing(release.clone()));
                std::thread::spawn(move || {
                    let dir = std::env::temp_dir().join("RustWTM-update");
                    let result = (updater.download)(&release, &dir).and_then(|msi| {
                        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                        (updater.install)(&msi, &exe)
                    });
                    match result {
                        // The installer replaces our files and starts us again.
                        Ok(()) => crate::tray::request_exit(&ctx),
                        Err(e) => {
                            set_update(&data, UpdateState::Failed(e));
                            ctx.request_repaint();
                        }
                    }
                });
            }
            UiCommand::CloseAlert(quiet) => {
                self.data().alert_watch.close(Instant::now(), quiet);
                (self.calm)(self.window.load(Ordering::Relaxed));
            }
            UiCommand::SaveConfig(toml) => {
                let result = match self.client.as_mut() {
                    None => Err(without_service(&self.data, NO_CONFIG)),
                    Some(c) => match c.call(&Request::SetConfig { toml }) {
                        Ok(Response::Done) => Ok(()),
                        Ok(other) => Err(unexpected(other).to_string()),
                        Err(e) => Err(e.to_string()),
                    },
                };
                self.data().message = Some(match result {
                    Ok(()) => (
                        EventLevel::Info,
                        crate::i18n::tr(
                            "Configuration saved; the service applies it within a second.",
                        )
                        .into(),
                    ),
                    Err(e) => (EventLevel::Error, {
                        crate::i18n::format(
                            "Saving the configuration failed: {0}",
                            std::slice::from_ref(&e),
                        )
                    }),
                });
                self.load(Panel::Config);
            }
            UiCommand::SaveTelegram(cfg) => {
                let result = match self.client.as_mut() {
                    None => Err(without_service(&self.data, NO_TELEGRAM)),
                    Some(c) => match c.call(&Request::SetTelegram(cfg)) {
                        Ok(Response::Done) => Ok(()),
                        Ok(other) => Err(unexpected(other).to_string()),
                        Err(e) => Err(e.to_string()),
                    },
                };
                self.data().message = Some(match result {
                    Ok(()) => (
                        EventLevel::Info,
                        crate::i18n::tr(
                            "Telegram settings saved; the bot follows them within seconds.",
                        )
                        .into(),
                    ),
                    Err(e) => (EventLevel::Error, {
                        crate::i18n::format(
                            "Saving the Telegram settings failed: {0}",
                            std::slice::from_ref(&e),
                        )
                    }),
                });
                self.load(Panel::Telegram);
            }
            UiCommand::TestTelegram => {
                let tested = |result: Result<String, String>| match result {
                    Ok(t) => (EventLevel::Info, t),
                    Err(e) => (EventLevel::Error, {
                        crate::i18n::format("Telegram test failed: {0}", std::slice::from_ref(&e))
                    }),
                };
                if self.client.is_none() {
                    self.data().message =
                        Some(tested(Err(without_service(&self.data, NO_TELEGRAM))));
                    return;
                }
                // Telegram may take half a minute to answer.
                self.aside(move |client, data| {
                    let result = client.map_err(|e| e.to_string()).and_then(|mut c| {
                        match c.call(&Request::TestTelegram) {
                            Ok(Response::Text(t)) => Ok(t),
                            Ok(other) => Err(unexpected(other).to_string()),
                            Err(e) => Err(e.to_string()),
                        }
                    });
                    data.lock().unwrap_or_else(PoisonError::into_inner).message =
                        Some(tested(result));
                });
            }
            UiCommand::WaitChain(target) => {
                self.data().wait_report = Some((target.clone(), None));
                let (data, ctx) = (self.data.clone(), self.ctx.clone());
                std::thread::spawn(move || {
                    let report = wtm_core::wait_chains(target.pid, WAIT_CHAIN_BUDGET)
                        .map_err(|e| e.to_string());
                    let mut d = data.lock().unwrap_or_else(PoisonError::into_inner);
                    // Unless another analysis was asked for meanwhile.
                    if d.wait_report.as_ref().is_some_and(|(t, _)| *t == target) {
                        d.wait_report = Some((target, Some(report)));
                    }
                    drop(d);
                    ctx.request_repaint();
                });
            }
            UiCommand::Firewall { path, block } => {
                let via_service = self.client.as_ref().is_some_and(Client::is_admin);
                let result = if via_service {
                    let client = self.client.as_mut().expect("checked above");
                    match client.call(&Request::Firewall {
                        path: path.clone(),
                        block,
                    }) {
                        Ok(Response::Done) => Ok(()),
                        Ok(other) => Err(unexpected(other).to_string()),
                        Err(e) => Err(e.to_string()),
                    }
                } else if block {
                    wtm_core::firewall::block(&path).map_err(|e| e.to_string())
                } else {
                    wtm_core::firewall::unblock(&path).map_err(|e| e.to_string())
                };
                self.data().message = Some(match (result, block) {
                    (Ok(()), true) => (
                        EventLevel::Info,
                        crate::i18n::format("Network blocked for {0}", std::slice::from_ref(&path)),
                    ),
                    (Ok(()), false) => (
                        EventLevel::Info,
                        crate::i18n::format(
                            "Network unblocked for {0}",
                            std::slice::from_ref(&path),
                        ),
                    ),
                    (Err(e), _) => (
                        EventLevel::Error,
                        crate::i18n::format(
                            "Windows Firewall refused the change for {0}: {1}",
                            &[path.clone(), e],
                        ),
                    ),
                });
                self.data().firewall = Some(firewall_blocked(self.client.as_mut()));
            }
            UiCommand::CloseConnection { local, remote } => {
                let via_service = self.client.as_ref().is_some_and(Client::is_admin);
                let result = if via_service {
                    let client = self.client.as_mut().expect("checked above");
                    match client.call(&Request::CloseConnection { local, remote }) {
                        Ok(Response::Done) => Ok(()),
                        Ok(other) => Err(unexpected(other).to_string()),
                        Err(e) => Err(e.to_string()),
                    }
                } else {
                    wtm_core::close_tcp(local, remote).map_err(|e| e.to_string())
                };
                self.data().message = Some(match result {
                    Ok(()) => (
                        EventLevel::Info,
                        crate::i18n::format(
                            "Connection {0} → {1} closed",
                            &[local.to_string(), remote.to_string()],
                        ),
                    ),
                    Err(e) => (
                        EventLevel::Error,
                        crate::i18n::format(
                            "Cannot close the connection {0} → {1}: {2}",
                            &[local.to_string(), remote.to_string(), e],
                        ),
                    ),
                });
                self.load(Panel::Network);
            }
            UiCommand::Modules(target) => {
                self.data().modules = Some((target.clone(), None));
                let (data, ctx) = (self.data.clone(), self.ctx.clone());
                std::thread::spawn(move || {
                    // Off the critical path: reading files and catalogs.
                    let _ = wtm_core::control::enter_background_mode();
                    let result = wtm_core::process_modules(target.pid)
                        .map(|mods| {
                            mods.into_iter()
                                .map(|m| {
                                    let s = cached_signature(&m.path);
                                    (m, s)
                                })
                                .collect()
                        })
                        .map_err(|e| e.to_string());
                    let mut d = data.lock().unwrap_or_else(PoisonError::into_inner);
                    if d.modules.as_ref().is_some_and(|(t, _)| *t == target) {
                        d.modules = Some((target, Some(result)));
                    }
                    drop(d);
                    ctx.request_repaint();
                });
            }
            UiCommand::WhyAlive(target) => {
                self.data().end_report = Some((target.clone(), None));
                let (data, ctx) = (self.data.clone(), self.ctx.clone());
                std::thread::spawn(move || {
                    let report = wtm_core::why_alive(target.pid).map_err(|e| e.to_string());
                    let mut d = data.lock().unwrap_or_else(PoisonError::into_inner);
                    if d.end_report.as_ref().is_some_and(|(t, _)| *t == target) {
                        d.end_report = Some((target, Some(report)));
                    }
                    drop(d);
                    ctx.request_repaint();
                });
            }
            UiCommand::Inspect(target) => {
                let result = match self.client.as_mut() {
                    Some(c) => inspect_with(c, &target),
                    None => control::inspect(&target).map_err(|e| e.to_string()),
                };
                self.data().details = Some((target, result));
            }
            UiCommand::Control { target, command } => {
                // A limit this window set while it ran without the service is
                // held by its own job handle: only it can change or lift it.
                let ours = matches!(command, Command::Limit(_) | Command::Unlimit)
                    && self.limiter.is_limited(&target);
                if !ours && self.client.as_ref().is_some_and(Client::is_admin) {
                    self.control(target, command);
                } else {
                    let outcomes = self.run_local(&target, command);
                    report(&mut self.data(), command, outcomes, true);
                    // The side panel, if it shows this process, reflects the change.
                    if showing(&self.data(), &target) {
                        self.handle(UiCommand::Inspect(target));
                    }
                }
            }
        }
    }

    fn load(&mut self, panel: Panel) {
        match panel {
            Panel::Sensors => {
                let frequency = wtm_core::power::frequency_report();
                self.data().frequency = Some(frequency);
                if let Some(client) = self.client.as_mut() {
                    match client.call(&Request::Hardware) {
                        Ok(Response::Hardware(report)) => {
                            self.data().hardware = report;
                            self.local_hardware = None;
                        }
                        Ok(other) => self.data().hardware.notice = unexpected(other).to_string(),
                        Err(e) => self.data().hardware.notice = e.to_string(),
                    }
                } else {
                    let report = self
                        .local_hardware
                        .get_or_insert_with(wtm_core::hardware::LocalHardware::start)
                        .snapshot();
                    self.data().hardware = report;
                }
            }
            Panel::Services => {
                let r = wtm_core::list_services().map_err(|e| e.to_string());
                self.data().services = Some(r);
            }
            Panel::Users => {
                let r = wtm_core::sessions().map_err(|e| e.to_string());
                self.data().sessions = Some(r);
            }
            Panel::History => {}
            Panel::Config => {
                let r = match self.client.as_mut() {
                    None => Err(without_service(&self.data, NO_CONFIG)),
                    Some(c) => match c.call(&Request::GetConfig) {
                        Ok(Response::Config { toml, path }) => Ok((path, toml)),
                        Ok(other) => Err(unexpected(other).to_string()),
                        Err(e) => Err(e.to_string()),
                    },
                };
                let mut data = self.data();
                // Kept parsed for the panic button, which every tab offers.
                let parsed = r
                    .as_ref()
                    .ok()
                    .and_then(|(_, text)| wtm_policy::Config::from_toml(text).ok());
                data.kill_list = parsed.as_ref().map(|c| c.kill.clone()).unwrap_or_default();
                data.swarm = parsed.map(|c| c.swarm).unwrap_or_default();
                data.config = Some(r);
            }
            Panel::Telegram => {
                let r = match self.client.as_mut() {
                    None => Err(without_service(&self.data, NO_TELEGRAM)),
                    Some(c) => match c.call(&Request::GetTelegram) {
                        Ok(Response::Telegram(cfg)) => Ok(cfg),
                        Ok(other) => Err(unexpected(other).to_string()),
                        Err(e) => Err(e.to_string()),
                    },
                };
                self.data().telegram = Some(r);
            }
            Panel::Startup => {
                self.data().startup = Some(wtm_core::startup::list_startup());
            }
            Panel::DiskActivity => {
                let result = match self.client.as_mut() {
                    Some(c) => match c.call(&Request::DiskActivity) {
                        Ok(Response::DiskActivity { files, seconds }) => Ok((files, seconds)),
                        Ok(other) => Err(unexpected(other).to_string()),
                        Err(e) => Err(e.to_string()),
                    },
                    None if wtm_core::launch::is_elevated() => self.local_disk_activity(),
                    None => Err(crate::i18n::tr(
                        "Which files are read and written needs the RustWTM service or administrator rights.",
                    )
                    .to_owned()),
                };
                self.data().disk_activity = Some(result);
            }
            Panel::Network => {
                let r = wtm_core::connections().map_err(|e| e.to_string());
                self.data().connections = Some(r);
                // Rules change rarely: read once, then after each change.
                if self.data().firewall.is_none() {
                    let blocked = firewall_blocked(self.client.as_mut());
                    self.data().firewall = Some(blocked);
                }
            }
        }
    }

    /// Stops the window's own disk trace when nothing reads it any more, or
    /// once the service, which runs its own, answers instead.
    fn stop_idle_disk_trace(&mut self) {
        if self.disk_trace.as_ref().is_some_and(|(_, taken)| {
            self.client.is_some() || taken.elapsed() >= LOCAL_DISK_TRACE_IDLE
        }) {
            self.disk_trace = None;
        }
    }

    /// Reads the window's own per-file disk trace, starting it if needed.
    fn local_disk_activity(&mut self) -> Result<(Vec<wtm_core::FileIo>, f64), String> {
        if self.disk_trace.is_none() {
            wtm_core::stop_trace_orphans(wtm_core::GUI_DISK_TRACE);
            let name = wtm_core::trace_session_name(wtm_core::GUI_DISK_TRACE);
            let tracer = wtm_core::DiskTracer::start(&name).map_err(|e| e.to_string())?;
            self.disk_trace = Some((tracer, Instant::now()));
        }
        let (tracer, taken) = self.disk_trace.as_mut().expect("started above");
        let seconds = taken.elapsed().as_secs_f64();
        *taken = Instant::now();
        Ok((tracer.take(), seconds))
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
            Command::AffinityTier(selection) => one(control::set_core_affinity(t, selection)),
            Command::EcoQos(mode) => one(control::set_eco_qos(t, mode)),
            Command::Limit(limits) => one(self.limiter.apply(t, limits)),
            Command::Unlimit => one(self.limiter.lift(t)),
            Command::TrimMemory => one(control::trim_memory(t)),
        }
    }
}

/// Shows what a control command did; `local` ones are also logged as events
/// (the service logs its own).
fn report(data: &mut Data, command: Command, mut outcomes: Vec<Outcome>, local: bool) {
    // Through the service the request already had administrator rights.
    let privileged = data.elevated || !local;
    for o in &mut outcomes {
        if let Err(e) = &mut o.result {
            *e = gui_error(e, privileged);
        }
    }
    let outcomes = &outcomes[..];
    {
        let failures: Vec<&Outcome> = outcomes.iter().filter(|o| o.result.is_err()).collect();
        data.message = Some(match failures.first() {
            None => (
                EventLevel::Info,
                format!("{} — {}", describe(command), names(outcomes)),
            ),
            Some(o) => (EventLevel::Warn, {
                let __wtm_i18n_0 = &(describe(command));
                let __wtm_i18n_1 = &(o.target.name);
                let __wtm_i18n_2 = &(o.target.pid);
                let __wtm_i18n_3 = &(o.result.as_ref().err().map_or("", String::as_str));
                crate::i18n::format(
                    "{0} failed for {1} ({2}): {3}",
                    &[
                        __wtm_i18n_0.to_string(),
                        __wtm_i18n_1.to_string(),
                        format!("{}", __wtm_i18n_2),
                        __wtm_i18n_3.to_string(),
                    ],
                )
            }),
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
                        Ok(()) => {
                            let __wtm_i18n_0 = &(describe(command));
                            crate::i18n::format("Local: {0}", std::slice::from_ref(__wtm_i18n_0))
                        }
                        Err(e) => {
                            let __wtm_i18n_0 = &(describe(command));
                            crate::i18n::format(
                                "Local: {0} failed: {1}",
                                &[__wtm_i18n_0.to_string(), e.to_string()],
                            )
                        }
                    },
                    notify: false,
                });
            }
            trim(&mut data.events, EVENTS_KEPT);
        }
    }
}

fn startup_message(name: &str, enabled: bool, result: io::Result<()>) -> (EventLevel, String) {
    let verb = if enabled {
        crate::i18n::tr("Enabled")
    } else {
        crate::i18n::tr("Disabled")
    };
    match result {
        Ok(()) => (EventLevel::Info, {
            crate::i18n::format(
                "{0} startup entry {1}",
                &[verb.to_string(), name.to_string()],
            )
        }),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => (EventLevel::Warn, {
            crate::i18n::format(
                "{0}: access denied (all-users entries need administrator)",
                &[name.to_string()],
            )
        }),
        Err(e) => (EventLevel::Warn, format!("{name}: {e}")),
    }
}

fn service_message(
    op: ServiceOp,
    name: &str,
    result: wtm_core::Result<()>,
) -> (EventLevel, String) {
    match result {
        Ok(()) => (EventLevel::Info, format!("{op:?}: {name}")),
        Err(e) if e.is_access_denied() => (EventLevel::Warn, {
            crate::i18n::format(
                "{0} {1}: access denied (run RustWTM as administrator)",
                &[format!("{:?}", op), name.to_string()],
            )
        }),
        Err(e) => (EventLevel::Warn, {
            crate::i18n::format(
                "{0} {1} failed: {2}",
                &[format!("{:?}", op), name.to_string(), format!("{}", e)],
            )
        }),
    }
}

fn inspect_with(client: &mut Client, target: &ProcessRef) -> Result<ProcessDetails, String> {
    match client.call(&Request::Inspect(target.clone())) {
        Ok(Response::Details(d)) => Ok(d),
        Ok(other) => Err(unexpected(other).to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Whether the side panel shows `target` (its details are loaded or loading).
fn showing(data: &Data, target: &ProcessRef) -> bool {
    data.details.as_ref().is_some_and(|(r, _)| r == target)
}

/// One control request over `client`, connecting first (again, after a
/// failure) when there is no connection.
fn run_on_service(
    client: &mut Option<Client>,
    pipe: &str,
    target: &ProcessRef,
    command: Command,
) -> Vec<Outcome> {
    if client.is_none() {
        match Client::connect_to(pipe) {
            Ok(c) => *client = Some(c),
            Err(e) => return vec![failed(target, e.to_string())],
        }
    }
    let Some(c) = client.as_mut() else {
        return Vec::new();
    };
    let request = Request::Control {
        target: target.clone(),
        command,
    };
    match c.call(&request) {
        Ok(Response::Control(o)) => o,
        Ok(other) => vec![failed(target, unexpected(other).to_string())],
        Err(e) => {
            *client = None;
            vec![failed(target, e.to_string())]
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
        many => {
            let __wtm_i18n_0 = &(many.len());
            crate::i18n::format("{0} processes", &[format!("{}", __wtm_i18n_0)])
        }
    }
}

pub fn describe(command: Command) -> String {
    match command {
        Command::Kill => crate::i18n::tr("End task").into(),
        Command::KillTree => crate::i18n::tr("End process tree").into(),
        Command::Suspend => crate::i18n::tr("Suspend").into(),
        Command::Resume => crate::i18n::tr("Resume").into(),
        Command::Priority(p) => crate::i18n::format("Priority {0}", &[format!("{:?}", p)]),
        Command::Affinity(mask) => crate::i18n::format("Affinity {0}", &[format!("{:#x}", mask)]),
        Command::AffinityTier(selection) => {
            let __wtm_i18n_0 = &(crate::i18n::tr(selection.label()));
            crate::i18n::format("Run on {0}", &[__wtm_i18n_0.to_string()])
        }
        Command::EcoQos(mode) => {
            crate::i18n::format("Efficiency mode {0}", &[format!("{:?}", mode)])
        }
        Command::Limit(l) => {
            let mut parts = Vec::new();
            if let Some(c) = l.cpu_percent {
                parts.push(crate::i18n::format("CPU ≤ {0}%", &[format!("{:.2}", c)]));
            }
            if let Some(m) = l.memory_bytes {
                parts.push({
                    let __wtm_i18n_0 = &(wtm_core::format_bytes(m as f64));
                    crate::i18n::format("memory ≤ {0}", std::slice::from_ref(__wtm_i18n_0))
                });
            }
            {
                let __wtm_i18n_0 = &(parts.join(", "));
                crate::i18n::format("Limit {0}", std::slice::from_ref(__wtm_i18n_0))
            }
        }
        Command::Unlimit => crate::i18n::tr("Remove limits").into(),
        Command::TrimMemory => crate::i18n::tr("Free memory").into(),
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

/// Shows a local sample, or why sampling failed.
fn publish_local(data: &mut Data, sample: wtm_core::Result<Snapshot>) {
    data.mode = Mode::Local;
    data.status = None;
    match sample {
        Ok(s) => {
            publish(data, s);
        }
        Err(e) => {
            data.sample_error = Some(e.to_string());
        }
    }
}

/// Replaces the history with the service's recent points, oldest first.
fn fill_history(h: &mut History, points: &[wtm_ipc::SystemPoint]) {
    let points = &points[points.len().saturating_sub(HISTORY_LEN)..];
    let series = |f: fn(&wtm_ipc::SystemPoint) -> f32| points.iter().map(f).collect();
    h.times = points.iter().map(|p| p.at_ms).collect();
    h.cpu = series(|p| p.cpu);
    h.memory = series(|p| p.memory);
    h.gpu = series(|p| p.gpu);
    h.net_rx = series(|p| p.net_rx);
    h.net_tx = series(|p| p.net_tx);
    h.io_read = series(|p| p.io_read);
    h.io_write = series(|p| p.io_write);
    let recent = &points[points.len().saturating_sub(CORE_HISTORY_LEN)..];
    let cores = recent.last().map_or(0, |p| p.cores.len());
    h.cores = (0..cores)
        .map(|i| {
            recent
                .iter()
                .map(|p| p.cores.get(i).copied().unwrap_or_default())
                .collect()
        })
        .collect();
}

fn publish(data: &mut Data, snapshot: Snapshot) {
    let (now, unix_now) = (Instant::now(), wtm_core::hardware::unix_ms());
    // The service answers from its latest sample; asked again before it
    // measures anew, it returns the same one, which adds nothing to graphs.
    let new = snapshot.taken_at_ms == 0
        || data
            .snapshot
            .as_ref()
            .is_none_or(|s| s.taken_at_ms != snapshot.taken_at_ms);
    data.sample_received_at = Some(now);
    data.sample_measured_at = Some(measured_at(now, unix_now, snapshot.taken_at_ms));
    data.sample_error = None;
    let at_ms = match snapshot.taken_at_ms {
        0 => unix_now,
        t => t,
    };
    let s = &snapshot.system;
    let mem = if s.memory_total > 0 {
        s.memory_used() as f32 / s.memory_total as f32 * 100.0
    } else {
        0.0
    };
    // The very first local sample has no interval and reads 0% everywhere.
    if snapshot.interval.is_some() && new {
        let h = &mut data.history;
        h.times.push_back(at_ms);
        trim(&mut h.times, HISTORY_LEN);
        for (series, value) in [
            (&mut h.cpu, s.cpu_percent),
            (&mut h.memory, mem),
            (&mut h.gpu, s.gpu_percent),
            (&mut h.net_rx, s.net_rx_rate as f32),
            (&mut h.net_tx, s.net_tx_rate as f32),
            (&mut h.io_read, s.io_read_rate as f32),
            (&mut h.io_write, s.io_write_rate as f32),
        ] {
            series.push_back(value);
            trim(series, HISTORY_LEN);
        }
        h.cores.resize_with(s.core_percent.len(), VecDeque::new);
        for (series, &load) in h.cores.iter_mut().zip(&s.core_percent) {
            series.push_back(load);
            trim(series, CORE_HISTORY_LEN);
        }
        update_process_history(&mut data.process_history, &snapshot.processes, at_ms);
    }
    trim(&mut data.events, EVENTS_KEPT);
    update_command_lines(&mut data.command_lines, &snapshot.processes);
    update_image_paths(&mut data.image_paths, &snapshot.processes);
    data.snapshot = Some(Arc::new(snapshot));
}

/// `taken_at_ms` (Unix milliseconds) on the monotonic clock, given that it
/// is `unix_now` at `now`; `now` when unknown or ahead of this clock.
fn measured_at(now: Instant, unix_now: u64, taken_at_ms: u64) -> Instant {
    if taken_at_ms == 0 {
        return now;
    }
    now.checked_sub(Duration::from_millis(unix_now.saturating_sub(taken_at_ms)))
        .unwrap_or(now)
}

fn set_update(data: &Mutex<Data>, state: UpdateState) {
    data.lock().unwrap_or_else(PoisonError::into_inner).update = state;
}

/// An update is being checked for or installed.
fn updating(data: &Mutex<Data>) -> bool {
    matches!(
        data.lock().unwrap_or_else(PoisonError::into_inner).update,
        UpdateState::Checking | UpdateState::Installing(_)
    )
}

/// Shows the outcome of a check, unless an install began while it ran.
fn finish_check(data: &Mutex<Data>, state: UpdateState) {
    let mut data = data.lock().unwrap_or_else(PoisonError::into_inner);
    if data.update == UpdateState::Checking {
        data.update = state;
    }
}

/// Shows the window above everything and keeps RustWTM responsive while
/// the machine is overloaded.
fn raise_window(window: isize) {
    let _ = control::set_own_priority(wtm_core::control::PriorityClass::High);
    if window != 0 {
        wtm_core::window::bring_to_front(window);
    }
}

fn calm_window(window: isize) {
    // Back to the standing priority set at startup, not below it.
    let _ = control::set_own_priority(wtm_core::control::PriorityClass::AboveNormal);
    if window != 0 {
        wtm_core::window::release_topmost(window);
    }
}

/// Appends the latest sample of every process and forgets exited ones.
fn update_process_history(
    histories: &mut ProcessHistories,
    procs: &[wtm_core::ProcessInfo],
    at_ms: u64,
) {
    let live: std::collections::HashSet<(u32, i64)> =
        procs.iter().map(|p| (p.pid, p.create_time)).collect();
    histories.retain(|k, _| live.contains(k));
    for p in procs {
        let h = histories.entry((p.pid, p.create_time)).or_default();
        h.times.push_back(at_ms);
        h.cpu.push_back(p.cpu_percent);
        h.memory.push_back(p.private_working_set as f32);
        trim(&mut h.times, PROCESS_HISTORY_LEN);
        trim(&mut h.cpu, PROCESS_HISTORY_LEN);
        trim(&mut h.memory, PROCESS_HISTORY_LEN);
    }
}

/// The sampler used without the service. Started as administrator it also
/// counts network bytes per process (an ETW session that needs the rights).
fn local_sampler() -> Sampler {
    let mut sampler = Sampler::new();
    if wtm_core::launch::is_elevated() {
        wtm_core::stop_trace_orphans(wtm_core::GUI_NET_TRACE);
        let _ =
            sampler.enable_network_trace(&wtm_core::trace_session_name(wtm_core::GUI_NET_TRACE));
    }
    sampler
}

/// Who signed a file, remembered per path: the same system DLLs appear in
/// nearly every process.
fn cached_signature(path: &str) -> wtm_core::Signature {
    static CACHE: std::sync::LazyLock<
        Mutex<std::collections::HashMap<String, wtm_core::Signature>>,
    > = std::sync::LazyLock::new(Mutex::default);
    let key = path.to_lowercase();
    if let Some(s) = CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return s.clone();
    }
    let s = wtm_core::signature(path);
    CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, s.clone());
    s
}

/// Programs blocked from the network, from the service when connected.
fn firewall_blocked(client: Option<&mut Client>) -> Result<Vec<String>, String> {
    match client {
        Some(c) => match c.call(&Request::FirewallBlocked) {
            Ok(Response::Blocked(list)) => Ok(list),
            Ok(other) => Err(unexpected(other).to_string()),
            Err(e) => Err(e.to_string()),
        },
        None => wtm_core::firewall::blocked().map_err(|e| e.to_string()),
    }
}

/// A wait chain analysis gives up after this long.
const WAIT_CHAIN_BUDGET: Duration = Duration::from_secs(15);

/// Processes with a not-responding window. Reads window state only.
fn hung_map() -> HungMap {
    wtm_core::hung_windows()
        .into_iter()
        .map(|w| (w.pid, w.title))
        .collect()
}

/// Command lines and image paths of processes not seen before. Each one is a
/// call into the process, so they are read without holding the data lock the
/// window needs for every frame.
#[derive(Debug, Default)]
struct Lookups {
    command_lines: Vec<((u32, i64), String)>,
    image_paths: Vec<((u32, i64), String)>,
}

impl Lookups {
    fn fetch(data: &Mutex<Data>, procs: &[wtm_core::ProcessInfo]) -> Self {
        let (known_lines, known_paths): (
            std::collections::HashSet<_>,
            std::collections::HashSet<_>,
        ) = {
            let d = data.lock().unwrap_or_else(PoisonError::into_inner);
            (
                d.command_lines.keys().copied().collect(),
                d.image_paths.keys().copied().collect(),
            )
        };
        let mut lookups = Self::default();
        for p in procs {
            let key = (p.pid, p.create_time);
            let target = || ProcessRef::from(p);
            if !known_lines.contains(&key) {
                let line = control::command_line(&target()).unwrap_or_default();
                lookups.command_lines.push((key, line));
            }
            if !known_paths.contains(&key) {
                let path = control::image_path(&target()).unwrap_or_default();
                lookups.image_paths.push((key, path));
            }
        }
        lookups
    }

    /// Stores the results; `publish` then finds every live process known.
    fn apply(self, data: &mut Data) {
        data.command_lines.extend(self.command_lines);
        data.image_paths.extend(self.image_paths);
    }
}

/// Reads the image path of every new process once.
fn update_image_paths(cache: &mut CommandLines, procs: &[wtm_core::ProcessInfo]) {
    let live: std::collections::HashSet<(u32, i64)> =
        procs.iter().map(|p| (p.pid, p.create_time)).collect();
    cache.retain(|k, _| live.contains(k));
    for p in procs {
        cache
            .entry((p.pid, p.create_time))
            .or_insert_with(|| control::image_path(&ProcessRef::from(p)).unwrap_or_default());
    }
}

/// Extracts the icons of image paths seen for the first time. Runs without
/// holding the data lock: icon extraction touches the disk.
fn refresh_icons(data: &Mutex<Data>) {
    let missing: Vec<String> = {
        let d = data.lock().unwrap_or_else(PoisonError::into_inner);
        let mut paths: Vec<String> = d
            .image_paths
            .values()
            .filter(|p| !p.is_empty() && !d.icons.contains_key(*p))
            .cloned()
            .collect();
        paths.sort();
        paths.dedup();
        paths
    };
    if missing.is_empty() {
        return;
    }
    let found: Vec<_> = missing
        .into_iter()
        .map(|p| {
            let icon = wtm_core::small_icon(&p).map(Arc::new);
            (p, icon)
        })
        .collect();
    data.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .icons
        .extend(found);
}

/// Reads the command line of every new process once; a process's command
/// line never changes. Exited processes are forgotten.
fn update_command_lines(cache: &mut CommandLines, procs: &[wtm_core::ProcessInfo]) {
    let live: std::collections::HashSet<(u32, i64)> =
        procs.iter().map(|p| (p.pid, p.create_time)).collect();
    cache.retain(|k, _| live.contains(k));
    for p in procs {
        cache
            .entry((p.pid, p.create_time))
            .or_insert_with(|| control::command_line(&ProcessRef::from(p)).unwrap_or_default());
    }
}

fn trim<T>(q: &mut VecDeque<T>, max: usize) {
    while q.len() > max {
        q.pop_front();
    }
}

fn notification(e: &Event) -> notify_rust::Notification {
    let title = match (&e.name, e.pid) {
        (Some(name), Some(pid)) => format!("RustWTM: {name} ({pid})"),
        _ => "RustWTM".to_owned(),
    };
    notify_rust::Notification::new()
        .app_id(APP_ID)
        .appname("RustWTM")
        .summary(&title)
        .body(&crate::i18n::message(&e.message))
        .finalize()
}

fn notify(e: &Event) {
    let _ = notification(e).show();
}

/// The toast shown when Windows starts us instead of a blocked program.
pub fn blocked_notification(name: &str) -> notify_rust::Notification {
    notify_rust::Notification::new()
        .app_id(APP_ID)
        .appname("RustWTM")
        .summary(crate::i18n::tr("Program blocked"))
        .body(&{ crate::i18n::format("RustWTM prevented {0} from starting.", &[name.to_string()]) })
        .finalize()
}

pub fn notify_blocked(name: &str) {
    let _ = blocked_notification(name).show();
}

/// Opens Explorer with `path` (a file or a folder) selected.
pub fn reveal(path: &std::path::Path) -> io::Result<()> {
    open_location(&path.display().to_string())
}

/// Opens Explorer with `path` selected.
fn open_location(path: &str) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    // Explorer parses its own command line and needs the path quoted.
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{path}\""))
        .spawn()
        .map(drop)
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

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "RustWTM";

/// The command Windows runs at sign-in: this program, hidden in the tray.
fn autostart_command() -> io::Result<String> {
    Ok(format!(
        "\"{}\" --background",
        std::env::current_exe()?.display()
    ))
}

/// Whether RustWTM starts (in the tray) when the user signs in.
pub fn autostart_enabled() -> bool {
    autostart_enabled_in(RUN_KEY)
}

pub fn set_autostart(on: bool) -> io::Result<()> {
    set_autostart_in(RUN_KEY, on)
}

fn autostart_enabled_in(key: &str) -> bool {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(key)
        .and_then(|k| k.get_value::<String, _>(RUN_VALUE))
        .is_ok()
}

fn set_autostart_in(key: &str, on: bool) -> io::Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let (run, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(key)?;
    if on {
        run.set_value(RUN_VALUE, &autostart_command()?)
    } else {
        match run.delete_value(RUN_VALUE) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
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

    #[test]
    fn closing_the_window_ends_its_worker() {
        let (tx, rx) = mpsc::channel::<UiCommand>();
        let ended = Arc::new(AtomicBool::new(false));
        let flag = ended.clone();
        let worker = std::thread::spawn(move || {
            // Like `Worker::run`: serve until the window hangs up.
            while rx.recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(50));
            flag.store(true, Ordering::SeqCst);
        });
        let backend = Backend {
            data: Arc::new(Mutex::new(Data::default())),
            tx,
            notifications: Arc::new(AtomicBool::new(true)),
            window: Arc::new(AtomicIsize::new(0)),
            worker: Some(worker),
        };
        backend.send(UiCommand::RestoreFrequency);
        drop(backend);
        assert!(ended.load(Ordering::SeqCst), "the worker finished first");
    }

    fn worker() -> Worker {
        Worker {
            local_hardware: None,
            data: Arc::new(Mutex::new(Data::default())),
            ctx: egui::Context::default(),
            notifications: Arc::new(AtomicBool::new(false)),
            window: Arc::new(AtomicIsize::new(0)),
            raise: |_| {},
            calm: |_| {},
            updater: Updater {
                latest: || Err("offline".into()),
                download: |_, _| Err("offline".into()),
                install: |_, _| Err("offline".into()),
            },
            client: None,
            pipe: String::new(),
            // Never try the real service pipe from tests.
            next_connect: Instant::now() + Duration::from_secs(3600),
            last_event: 0,
            // Not called while `notifications` is off.
            notify,
            open: |_| Ok(()),
            launch: |_, _| Ok(()),
            restart: || Ok(()),
            foreground: || None,
            sampler: Sampler::new(),
            disk_trace: None,
            limiter: Limiter::new(),
            controls: None,
            sampler_paused: false,
        }
    }

    fn target_of(w: &Worker, pid: u32) -> ProcessRef {
        let data = w.data();
        let snap = data.snapshot.as_ref().unwrap();
        ProcessRef::from(snap.processes.iter().find(|p| p.pid == pid).unwrap())
    }

    fn message(w: &Worker) -> (EventLevel, String) {
        w.data().message.take().unwrap()
    }

    #[test]
    fn process_history_follows_running_processes() {
        let mut h = ProcessHistories::new();
        let mut a = crate::ui_tests::proc(10, 0, "a.exe", 5.0);
        let b = crate::ui_tests::proc(11, 0, "b.exe", 1.0);
        for i in 0..PROCESS_HISTORY_LEN + 5 {
            a.cpu_percent = i as f32;
            update_process_history(&mut h, &[a.clone(), b.clone()], i as u64 * 1000);
        }
        let ha = &h[&(10, a.create_time)];
        assert_eq!(ha.cpu.len(), PROCESS_HISTORY_LEN);
        assert_eq!(ha.times.len(), PROCESS_HISTORY_LEN);
        assert_eq!(
            ha.times.back(),
            Some(&((PROCESS_HISTORY_LEN + 4) as u64 * 1000))
        );
        assert_eq!(ha.cpu.back(), Some(&((PROCESS_HISTORY_LEN + 4) as f32)));
        assert_eq!(ha.memory.len(), PROCESS_HISTORY_LEN);
        // b exits.
        update_process_history(&mut h, &[a.clone()], 0);
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn a_sample_received_again_adds_no_point_and_keeps_its_age() {
        let mut d = Data::default();
        let measured = wtm_core::hardware::unix_ms() - 30_000;
        let sample = |taken_at_ms, cpu| Snapshot {
            interval: Some(Duration::from_secs(2)),
            taken_at_ms,
            system: wtm_core::SystemStats {
                cpu_percent: cpu,
                ..Default::default()
            },
            ..crate::ui_tests::snapshot()
        };
        publish(&mut d, sample(measured, 10.0));
        publish(&mut d, sample(measured, 10.0));
        assert_eq!(d.history.cpu, [10.0]);
        assert_eq!(d.history.times, [measured]);
        // Received just now, measured half a minute ago: out of date.
        let now = Instant::now();
        assert!(d.sample_received_at.unwrap() <= now);
        assert!(matches!(
            d.sample_freshness(now),
            SampleFreshness::Stale(Some(age)) if age >= Duration::from_secs(29)
        ));

        publish(&mut d, sample(measured + 2000, 20.0));
        assert_eq!(d.history.cpu, [10.0, 20.0]);
        assert_eq!(d.history.times, [measured, measured + 2000]);
    }

    #[test]
    fn measurement_times_map_onto_the_monotonic_clock() {
        let now = Instant::now();
        assert_eq!(measured_at(now, 10_000, 0), now, "unknown");
        assert_eq!(measured_at(now, 10_000, 12_000), now, "clock behind");
        assert_eq!(
            now.duration_since(measured_at(now, 10_000, 7_500)),
            Duration::from_millis(2500)
        );
    }

    #[test]
    fn history_is_filled_from_the_services_recent_points() {
        let point = |i: usize| wtm_ipc::SystemPoint {
            at_ms: i as u64 * 2000,
            cpu: i as f32,
            memory: 1.0,
            gpu: 2.0,
            net_rx: 3.0,
            net_tx: 4.0,
            io_read: 5.0,
            io_write: 6.0,
            cores: vec![i as f32; 2],
        };
        let mut h = History::default();
        fill_history(
            &mut h,
            &(0..HISTORY_LEN + 10).map(point).collect::<Vec<_>>(),
        );
        assert_eq!(h.cpu.len(), HISTORY_LEN);
        assert_eq!(h.cpu.front(), Some(&10.0), "the oldest are dropped");
        assert_eq!(h.times.len(), HISTORY_LEN);
        assert_eq!(h.times.front(), Some(&20_000), "times match the points");
        assert_eq!((h.net_tx[0], h.io_write[0]), (4.0, 6.0));
        assert_eq!(h.cores.len(), 2);
        assert_eq!(h.cores[1].len(), CORE_HISTORY_LEN);
        assert_eq!(h.cores[1].back(), Some(&((HISTORY_LEN + 9) as f32)));
        fill_history(&mut h, &[]);
        assert!(h.cpu.is_empty() && h.cores.is_empty());
    }

    #[test]
    fn watched_processes_are_announced_once_when_they_exit() {
        static EXITED: AtomicIsize = AtomicIsize::new(0);
        let mut w = worker();
        w.notify = |e| {
            assert!(e.message.starts_with("b.exe has exited"), "{}", e.message);
            EXITED.fetch_add(1, Ordering::Relaxed);
        };
        let a = crate::ui_tests::proc(10, 0, "a.exe", 5.0);
        let b = crate::ui_tests::proc(11, 0, "b.exe", 1.0);
        let snapshot = |procs: Vec<wtm_core::ProcessInfo>| {
            Some(Arc::new(Snapshot {
                interval: None,
                system: wtm_core::SystemStats::default(),
                processes: procs,
                net_traced: false,
                taken_at_ms: 0,
            }))
        };
        w.data().snapshot = snapshot(vec![a.clone(), b.clone()]);
        w.handle(UiCommand::WatchExit(ProcessRef::from(&a)));
        w.handle(UiCommand::WatchExit(ProcessRef::from(&b)));
        assert!(message(&w).1.contains("notified when b.exe (11) exits"));
        w.check_exits();
        assert_eq!(EXITED.load(Ordering::Relaxed), 0, "both still run");

        w.data().snapshot = snapshot(vec![a.clone()]);
        w.check_exits();
        w.check_exits();
        assert_eq!(EXITED.load(Ordering::Relaxed), 1);
        assert_eq!(w.data().exit_watches.len(), 1, "a.exe is still watched");
        w.handle(UiCommand::WatchExit(ProcessRef::from(&a)));
        assert!(
            w.data().exit_watches.is_empty(),
            "a second request stops watching"
        );
        assert_eq!(message(&w).1, "No longer watching a.exe");
    }

    #[test]
    fn an_overload_raises_the_window_and_closing_calms_it() {
        static RAISED: AtomicIsize = AtomicIsize::new(0);
        static CALMED: AtomicIsize = AtomicIsize::new(0);
        let mut w = worker();
        w.window.store(42, Ordering::Relaxed);
        w.raise = |h| RAISED.store(h, Ordering::Relaxed);
        w.calm = |h| CALMED.store(h, Ordering::Relaxed);
        w.data().alert_watch.settings.seconds = 0;

        // No snapshot yet, then a first one without an interval: nothing.
        w.watch_cpu();
        let mut snap = crate::ui_tests::snapshot();
        snap.interval = None;
        snap.system.cpu_percent = 99.0;
        w.data().snapshot = Some(Arc::new(snap.clone()));
        w.watch_cpu();
        assert_eq!(RAISED.load(Ordering::Relaxed), 0);

        snap.interval = Some(Duration::from_secs(1));
        w.data().snapshot = Some(Arc::new(snap));
        w.watch_cpu();
        assert_eq!(RAISED.load(Ordering::Relaxed), 42);
        assert_eq!(w.data().alert_watch.active.map(|a| a.cpu), Some(99.0));

        w.handle(UiCommand::CloseAlert(Duration::from_secs(60)));
        assert!(w.data().alert_watch.active.is_none());
        assert_eq!(CALMED.load(Ordering::Relaxed), 42);
        // Quiet now, even though the load is still high.
        RAISED.store(0, Ordering::Relaxed);
        w.watch_cpu();
        assert_eq!(RAISED.load(Ordering::Relaxed), 0);

        // The real helpers: our own priority goes up and back; no window.
        raise_window(0);
        calm_window(0);
    }

    fn release(major: u32) -> wtm_core::update::Release {
        wtm_core::update::Release {
            version: (major, 0, 0),
            tag: format!("v{major}.0.0"),
            page: "https://example.invalid/r".into(),
            msi: Some((
                "https://example.invalid/a.msi".into(),
                "https://example.invalid/a.sha256".into(),
            )),
        }
    }

    /// Waits until the update state leaves `Checking` / `Installing`.
    fn settled(w: &Worker) -> UpdateState {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let s = w.data().update.clone();
            if !matches!(s, UpdateState::Checking | UpdateState::Installing(_)) {
                return s;
            }
            assert!(Instant::now() < deadline, "update never settled");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_check_never_disturbs_an_install_under_way() {
        let mut w = worker();
        w.updater.latest = || panic!("no check while installing");
        w.updater.download = |_, _| panic!("no second install");
        set_update(&w.data, UpdateState::Installing(release(99)));
        w.handle(UiCommand::CheckUpdate);
        w.handle(UiCommand::InstallUpdate(release(98)));
        assert_eq!(w.data().update, UpdateState::Installing(release(99)));

        // A check that was already running when the install began.
        finish_check(&w.data, UpdateState::Available(release(99)));
        assert_eq!(w.data().update, UpdateState::Installing(release(99)));
        set_update(&w.data, UpdateState::Checking);
        finish_check(&w.data, UpdateState::UpToDate);
        assert_eq!(w.data().update, UpdateState::UpToDate);
    }

    #[test]
    fn checks_for_and_installs_updates_in_the_background() {
        let mut w = worker();
        w.updater.latest = || Ok(release(99));
        w.handle(UiCommand::CheckUpdate);
        assert_eq!(settled(&w), UpdateState::Available(release(99)));
        w.updater.latest = || Ok(release(0));
        w.handle(UiCommand::CheckUpdate);
        assert_eq!(settled(&w), UpdateState::UpToDate);
        w.updater.latest = || Err("offline".into());
        w.handle(UiCommand::CheckUpdate);
        assert_eq!(settled(&w), UpdateState::Failed("offline".into()));

        // A failed download is shown and nothing is installed.
        w.updater.download = |_, _| Err("checksum mismatch".into());
        w.updater.install = |_, _| panic!("must not install");
        w.handle(UiCommand::InstallUpdate(release(99)));
        assert_eq!(settled(&w), UpdateState::Failed("checksum mismatch".into()));

        // Success hands over to the installer and exits.
        let ctx = egui::Context::default();
        w.ctx = ctx.clone();
        w.updater.download = |_, dir| {
            Ok(wtm_core::update::Download {
                path: dir.join("a.msi"),
                sha256: [7; 32],
            })
        };
        w.updater.install = |msi, exe| {
            assert!(msi.path.ends_with("RustWTM-update\\a.msi"));
            assert_eq!(msi.sha256, [7; 32]);
            assert_eq!(exe, std::env::current_exe().unwrap());
            Ok(())
        };
        w.handle(UiCommand::InstallUpdate(release(99)));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !crate::tray::exit_requested(&ctx) {
            assert!(Instant::now() < deadline, "did not exit for the installer");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(Updater::default().latest as usize != 0);
    }

    #[test]
    fn autostart_is_written_and_removed() {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        let key = format!(r"Software\RustWTM-test-autostart-{}", std::process::id());
        assert!(!autostart_enabled_in(&key));
        set_autostart_in(&key, true).unwrap();
        assert!(autostart_enabled_in(&key));
        let value: String = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(&key)
            .unwrap()
            .get_value(RUN_VALUE)
            .unwrap();
        assert!(
            value.ends_with("\" --background") && value.starts_with('"'),
            "{value}"
        );
        set_autostart_in(&key, false).unwrap();
        assert!(!autostart_enabled_in(&key));
        // Turning off twice is fine.
        set_autostart_in(&key, false).unwrap();
        RegKey::predef(HKEY_CURRENT_USER)
            .delete_subkey_all(&key)
            .unwrap();
        // The real key can be read without changing it.
        let _ = autostart_enabled();
    }

    /// Waits for a message from a background job.
    fn background_message(w: &Worker) -> (EventLevel, String) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(m) = w.data().message.take() {
                return m;
            }
            assert!(Instant::now() < deadline, "no message from the job");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn runs_tasks_and_restarts_elevated_in_the_background() {
        let mut w = worker();
        w.handle(UiCommand::Run {
            line: "notepad a.txt".into(),
            elevated: true,
        });
        assert_eq!(
            background_message(&w),
            (EventLevel::Info, "Started notepad a.txt".into())
        );
        w.launch = |_, _| Err("cancelled at the administrator prompt".into());
        w.handle(UiCommand::Run {
            line: "regedit".into(),
            elevated: true,
        });
        assert_eq!(
            background_message(&w).1,
            "Cannot run regedit: cancelled at the administrator prompt"
        );

        w.restart = || Err("no".into());
        w.handle(UiCommand::RestartElevated);
        assert_eq!(
            background_message(&w).1,
            "Cannot restart as administrator: no"
        );

        // Success closes the window.
        let ctx = egui::Context::default();
        ctx.run_pass(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        w.ctx = ctx.clone();
        w.restart = || Ok(());
        w.handle(UiCommand::RestartElevated);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut output = ctx.run_pass(egui::RawInput::default(), |_| {});
            output.textures_delta.clear();
            let commands = output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .map(|v| v.commands.clone())
                .unwrap_or_default();
            if commands.contains(&egui::ViewportCommand::Close) {
                break;
            }
            assert!(Instant::now() < deadline, "window not closed");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(w.data().message.is_none());
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
        let no_history = format!("{NO_SERVICE}, which is not running.");
        let no_config = format!("{NO_CONFIG}, which is not running.");
        assert!(matches!(&w.data().app_history, Some(Err(e)) if *e == no_history));
        w.handle(UiCommand::SaveConfig(String::new()));
        assert!(message(&w).1.ends_with(&no_config));
        w.handle(UiCommand::TestTelegram);
        assert!(message(&w).1.contains(NO_TELEGRAM));
        w.handle(UiCommand::Load(Panel::Telegram));
        assert!(matches!(&w.data().telegram, Some(Err(e)) if e.starts_with(NO_TELEGRAM)));
        assert!(matches!(&w.data().config, Some(Err(e)) if *e == no_config));
        // Another version of the service is named as the reason.
        w.data().service_error = Some("service speaks protocol 6, client 7".into());
        w.handle(UiCommand::Load(Panel::Config));
        let text = match &w.data().config {
            Some(Err(e)) => e.clone(),
            other => panic!("{other:?}"),
        };
        assert!(
            text.starts_with(&format!(
                "{NO_CONFIG}, but the installed service is a different version (service speaks \
                 protocol 6, client 7). Update it with the installer of version"
            )),
            "{text}"
        );
        w.data().service_error = None;
        w.handle(UiCommand::LoadSeries {
            app: "x".into(),
            hours: 1,
        });
        assert!(w.data().series.is_none());

        let child = spawn();
        w.tick();
        let t = target_of(&w, child.0.id());
        // Shown in the side panel, so its details follow every command.
        w.handle(UiCommand::Inspect(t.clone()));
        for command in [
            Command::Suspend,
            Command::Resume,
            Command::Priority(wtm_core::control::PriorityClass::BelowNormal),
            Command::Affinity(1),
            Command::AffinityTier(wtm_core::cpu::CoreSelection::All),
            Command::EcoQos(wtm_core::control::EcoQos::On),
            Command::Limit(Limits {
                cpu_percent: Some(50.0),
                memory_bytes: Some(1 << 30),
            }),
            Command::Unlimit,
            Command::TrimMemory,
        ] {
            w.handle(UiCommand::Control {
                target: t.clone(),
                command,
            });
            let (level, text) = message(&w);
            assert_eq!(level, EventLevel::Info, "{command:?}: {text}");
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
        for op in [
            ServiceOp::Start,
            ServiceOp::Stop,
            ServiceOp::StartType(StartType::Manual),
        ] {
            w.handle(UiCommand::Service {
                name: "NoSuchServiceForRustWTMTests".into(),
                op,
            });
            assert_eq!(message(&w).0, EventLevel::Warn, "{op:?}");
        }
    }

    /// Answers every request with `respond` on `connections` connections
    /// (slow requests use one of their own), each until its client
    /// disconnects.
    fn serve(
        pipe: &str,
        connections: usize,
        respond: impl FnMut(Request) -> Response + Send + 'static,
    ) -> std::thread::JoinHandle<()> {
        let mut listener = wtm_sys::pipe::PipeListener::bind(pipe, wtm_ipc::PIPE_SDDL).unwrap();
        let respond = Arc::new(Mutex::new(respond));
        std::thread::spawn(move || {
            let served: Vec<_> = (0..connections)
                .map(|_| {
                    let mut conn = listener.accept().unwrap();
                    let respond = respond.clone();
                    std::thread::spawn(move || {
                        while let Ok(request) = read_frame::<Request>(&mut conn) {
                            let response = respond.lock().unwrap()(request);
                            let _ = write_frame(&mut conn, &response);
                        }
                    })
                })
                .collect();
            for s in served {
                s.join().unwrap();
            }
        })
    }

    fn hello() -> Response {
        Response::Hello {
            version: PROTOCOL_VERSION,
            admin: true,
        }
    }

    /// Connects to a server that hangs up right after the handshake.
    fn hang_up_after_hello(pipe: &str, admin: bool) -> Client {
        let mut listener = wtm_sys::pipe::PipeListener::bind(pipe, wtm_ipc::PIPE_SDDL).unwrap();
        let server = std::thread::spawn(move || {
            let mut conn = listener.accept().unwrap();
            let _ = read_frame::<Request>(&mut conn);
            let _ = write_frame(
                &mut conn,
                &Response::Hello {
                    version: PROTOCOL_VERSION,
                    admin,
                },
            );
        });
        let client = Client::connect_to(pipe).unwrap();
        server.join().unwrap();
        client
    }

    fn test_pipe(name: &str) -> String {
        format!(r"\\.\pipe\wtm-gui-test-{name}-{}", std::process::id())
    }

    /// Answers like the service would, until the client disconnects.
    fn fake_server(
        pipe: String,
        connections: usize,
        snapshot: Snapshot,
    ) -> std::thread::JoinHandle<()> {
        let target = ProcessRef {
            pid: 1,
            create_time: 1,
            name: "x.exe".into(),
        };
        serve(&pipe, connections, move |request| match request {
            Request::Hello { .. } => hello(),
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
            Request::GetConfig => Response::Config {
                toml: "dry_run = true\n".into(),
                path: r"C:\cfg.toml".into(),
            },
            Request::SetConfig { .. } => Response::Done,
            Request::Recent => Response::Recent(
                (0..400)
                    .map(|i| wtm_ipc::SystemPoint {
                        at_ms: 1 + i * 1000,
                        cpu: i as f32 / 4.0,
                        memory: 50.0,
                        gpu: 1.0,
                        net_rx: 10.0,
                        net_tx: 20.0,
                        io_read: 30.0,
                        io_write: 40.0,
                        cores: vec![i as f32 / 4.0; 4],
                    })
                    .collect(),
            ),
            Request::GetTelegram => Response::Telegram(wtm_policy::TelegramConfig::default()),
            Request::SetTelegram(_) => Response::Done,
            Request::TestTelegram => Response::Text("sent".into()),
            Request::Hardware => Response::Hardware(wtm_core::hardware::HardwareReport::default()),
            Request::SetCooling { .. } => Response::Done,
            Request::CloseConnection { .. } => Response::Done,
            Request::Firewall { .. } => Response::Done,
            Request::FirewallBlocked => Response::Blocked(Vec::new()),
            Request::DiskActivity => Response::DiskActivity {
                files: Vec::new(),
                seconds: 1.0,
            },
            Request::Foreground { .. } => Response::Done,
        })
    }

    #[test]
    fn a_limit_set_without_the_service_is_lifted_here_once_it_runs() {
        let child = spawn();
        let mut w = worker();
        w.tick();
        let t = target_of(&w, child.0.id());
        let limit = Limits {
            cpu_percent: Some(50.0),
            memory_bytes: Some(1 << 30),
        };
        w.handle(UiCommand::Control {
            target: t.clone(),
            command: Command::Limit(limit),
        });
        assert_eq!(message(&w).0, EventLevel::Info);
        assert!(w.limiter.is_limited(&t));
        // The service is now connected (any request would be refused).
        w.client = Some(hang_up_after_hello(&test_pipe("limit-lift"), true));
        w.handle(UiCommand::Control {
            target: t.clone(),
            command: Command::Unlimit,
        });
        assert_eq!(message(&w).0, EventLevel::Info, "lifted here, not sent");
        assert!(!w.limiter.is_limited(&t));
    }

    #[test]
    fn local_sampling_after_the_service_starts_afresh() {
        let mut w = worker();
        w.tick();
        w.tick();
        let interval = |w: &Worker| w.data().snapshot.as_ref().unwrap().interval;
        assert!(interval(&w).is_some());
        // The service measured for a while; then the connection was lost.
        w.sampler_paused = true;
        w.tick();
        assert!(
            interval(&w).is_none(),
            "no rates averaged over the service's time"
        );
        w.tick();
        assert!(interval(&w).is_some());
    }

    #[test]
    fn a_burst_of_controls_goes_through_one_connection_and_reports_failures() {
        let pipe = test_pipe("burst");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        // The main connection and the one for control requests: more would
        // be refused by a real service once 16 are open.
        let server = serve(&pipe, 2, move |request| match request {
            Request::Hello { .. } => hello(),
            Request::Control { target, .. } => {
                log.lock().unwrap().push(target.pid);
                let result = if target.pid == 13 {
                    Err("access denied".into())
                } else {
                    Ok(())
                };
                Response::Control(vec![Outcome { target, result }])
            }
            Request::Inspect(_) => Response::Error("denied".into()),
            other => panic!("{other:?}"),
        });
        let mut w = worker();
        w.pipe = pipe.clone();
        w.client = Some(Client::connect_to(&pipe).unwrap());
        // Something else is selected: its panel is left alone.
        let selected = ProcessRef {
            pid: 999,
            create_time: 1,
            name: "selected.exe".into(),
        };
        w.data().details = Some((selected.clone(), Err("kept".into())));
        for pid in 1..=40 {
            w.handle(UiCommand::Control {
                target: ProcessRef {
                    pid,
                    create_time: 1,
                    name: format!("node{pid}.exe"),
                },
                command: Command::Kill,
            });
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while seen.lock().unwrap().len() < 40 {
            assert!(Instant::now() < deadline, "{:?}", seen.lock().unwrap());
            std::thread::sleep(Duration::from_millis(10));
        }
        let (level, text) = background_message(&w);
        assert_eq!(level, EventLevel::Warn, "the failure is not hidden: {text}");
        assert!(text.contains("node13.exe"), "{text}");
        assert_eq!(*seen.lock().unwrap(), (1..=40).collect::<Vec<_>>());
        assert!(matches!(&w.data().details, Some((r, Err(e))) if *r == selected && e == "kept"));
        drop(w);
        server.join().unwrap();
    }

    #[test]
    fn service_mode_fetches_controls_and_falls_back_when_lost() {
        let pipe = test_pipe("service");
        let snapshot = Sampler::new().sample().unwrap();
        // The main connection, then one each for Control and TestTelegram.
        let server = fake_server(pipe.clone(), 3, snapshot);

        let mut w = worker();
        w.pipe = pipe.clone();
        w.client = Some(Client::connect_to(&pipe).unwrap());
        w.foreground = || Some((1, false));
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
            // The graphs start with the service's last minutes.
            assert_eq!(d.history.cpu.len(), HISTORY_LEN);
            assert_eq!(d.history.cpu.back(), Some(&(399.0 / 4.0)));
            assert_eq!(d.history.io_write.front(), Some(&40.0));
            assert_eq!(d.history.cores.len(), 4);
            assert_eq!(d.history.cores[0].len(), CORE_HISTORY_LEN);
        }
        assert_eq!(w.last_event, 7);

        let t = ProcessRef {
            pid: 1,
            create_time: 1,
            name: "x.exe".into(),
        };
        // The side panel shows it.
        w.data().details = Some((t.clone(), Err("loading".into())));
        w.handle(UiCommand::Control {
            target: t.clone(),
            command: Command::Suspend,
        });
        assert_eq!(background_message(&w).0, EventLevel::Info);
        // Inspect after the command went through the service and was refused.
        assert!(matches!(&w.data().details, Some((_, Err(e))) if e == "denied"));
        w.handle(UiCommand::LoadHistory { hours: 24 });
        assert!(matches!(w.data().app_history, Some(Ok(_))));
        w.handle(UiCommand::SaveConfig("dry_run = true\n".into()));
        assert_eq!(message(&w).0, EventLevel::Info);
        assert!(
            matches!(&w.data().config, Some(Ok((path, toml))) if path == r"C:\cfg.toml" && toml.starts_with("dry_run"))
        );
        w.handle(UiCommand::LoadSeries {
            app: "x.exe".into(),
            hours: 24,
        });
        assert!(w.data().series.is_some());
        w.handle(UiCommand::SaveTelegram(
            wtm_policy::TelegramConfig::default(),
        ));
        assert_eq!(message(&w).0, EventLevel::Info);
        assert!(matches!(w.data().telegram, Some(Ok(_))));
        w.handle(UiCommand::TestTelegram);
        assert_eq!(
            background_message(&w),
            (EventLevel::Info, "sent".to_owned())
        );

        drop(w.client.take());
        server.join().unwrap();

        // A server that hangs up right after the handshake (as a restarting
        // service does): the next tick falls back to local sampling and
        // says that it reconnects.
        w.client = Some(hang_up_after_hello(&pipe, false));
        w.tick();
        assert_eq!(w.data().mode, Mode::Local);
        assert!(w.client.is_none());
        assert_eq!(message(&w), (EventLevel::Warn, RECONNECTING.to_owned()));
    }

    static NOTIFIED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn count_notification(_: &Event) {
        NOTIFIED.fetch_add(1, Ordering::Relaxed);
    }

    fn x_exe() -> ProcessRef {
        ProcessRef {
            pid: 1,
            create_time: 1,
            name: "x.exe".into(),
        }
    }

    #[test]
    fn connects_notifies_and_handles_odd_answers() {
        let pipe = test_pipe("odd");
        let mut next_event = 0;
        let server = serve(&pipe, 3, move |request| match request {
            Request::Hello { .. } => hello(),
            // The service has not sampled yet.
            Request::Snapshot => Response::Error("no sample yet".into()),
            Request::Status => Response::Status(Status::default()),
            Request::Events { .. } => {
                next_event += 1;
                Response::Events(vec![Event {
                    id: next_event,
                    unix_ms: 0,
                    level: EventLevel::Warn,
                    pid: Some(1),
                    name: Some("x.exe".into()),
                    message: "limited".into(),
                    notify: true,
                }])
            }
            Request::Inspect(_) => Response::Details(ProcessDetails {
                path: None,
                command_line: None,
                user: None,
                priority: None,
                affinity: None,
                eco_qos: None,
                critical: None,
                protected: None,
            }),
            Request::Control { .. } => Response::Error("refused".into()),
            Request::History { .. } => Response::Series(Vec::new()),
            Request::AppSeries { .. } => Response::Error("unknown app".into()),
            Request::GetConfig => Response::Done,
            Request::SetConfig { .. } => Response::Error("invalid configuration: x".into()),
            Request::Recent | Request::GetTelegram | Request::SetTelegram(_) => Response::Done,
            Request::TestTelegram => Response::Error("chat 1: chat not found".into()),
            Request::Hardware => Response::Hardware(wtm_core::hardware::HardwareReport::default()),
            Request::SetCooling { .. } => Response::Done,
            Request::CloseConnection { .. } => Response::Done,
            Request::Firewall { .. } => Response::Done,
            Request::FirewallBlocked => Response::Blocked(Vec::new()),
            Request::DiskActivity => Response::DiskActivity {
                files: Vec::new(),
                seconds: 1.0,
            },
            Request::Foreground { .. } => Response::Error("not a process of your session".into()),
        });

        let mut w = worker();
        w.pipe = pipe;
        w.next_connect = Instant::now();
        w.notify = count_notification;
        w.foreground = || Some((1, false));
        w.notifications.store(true, Ordering::Relaxed);
        w.tick();
        assert!(w.client.is_some(), "tick connects to the pipe");
        assert!(w.data().snapshot.is_none());
        assert_eq!(
            w.data().sample_freshness(Instant::now()),
            SampleFreshness::Stale(None)
        );
        // The backlog fetched right after connecting is not announced.
        assert_eq!(NOTIFIED.load(Ordering::Relaxed), 0);
        w.tick();
        assert_eq!(NOTIFIED.load(Ordering::Relaxed), 1);
        assert_eq!(w.last_event, 2);

        w.data().details = Some((x_exe(), Err("loading".into())));
        w.handle(UiCommand::Control {
            target: x_exe(),
            command: Command::Resume,
        });
        let (level, text) = background_message(&w);
        assert_eq!(level, EventLevel::Warn);
        assert!(text.ends_with("refused"), "{text}");
        assert!(matches!(&w.data().details, Some((_, Ok(_)))));
        w.handle(UiCommand::LoadHistory { hours: 1 });
        assert!(
            matches!(&w.data().app_history, Some(Err(e)) if e.starts_with("unexpected response"))
        );
        w.handle(UiCommand::LoadSeries {
            app: "x.exe".into(),
            hours: 1,
        });
        assert!(w.data().series.is_none());
        w.handle(UiCommand::SaveConfig(String::new()));
        let (level, text) = message(&w);
        assert_eq!(level, EventLevel::Error);
        assert!(text.ends_with("invalid configuration: x"), "{text}");
        assert!(matches!(&w.data().config, Some(Err(e)) if e.starts_with("unexpected response")));
        w.handle(UiCommand::TestTelegram);
        assert!(background_message(&w).1.ends_with("chat not found"));
        w.handle(UiCommand::Load(Panel::Telegram));
        assert!(matches!(&w.data().telegram, Some(Err(e)) if e.starts_with("unexpected response")));

        drop(w.client.take());
        server.join().unwrap();
    }

    #[test]
    fn requests_on_a_dead_connection_fail_cleanly() {
        let mut w = worker();
        w.pipe = test_pipe("dead");
        w.client = Some(hang_up_after_hello(&w.pipe, true));
        w.handle(UiCommand::LoadHistory { hours: 1 });
        assert!(matches!(w.data().app_history, Some(Err(_))));
        w.handle(UiCommand::SaveConfig(String::new()));
        assert_eq!(message(&w).0, EventLevel::Error);
        assert!(matches!(w.data().config, Some(Err(_))));
        // The connection for the request cannot be made either.
        w.data().details = Some((x_exe(), Err("loading".into())));
        w.handle(UiCommand::Control {
            target: x_exe(),
            command: Command::Kill,
        });
        assert_eq!(background_message(&w).0, EventLevel::Warn);
        assert!(matches!(&w.data().details, Some((_, Err(_)))));
    }

    fn event(id: u64, unix_ms: u64) -> Event {
        Event {
            id,
            unix_ms,
            level: EventLevel::Info,
            pid: None,
            name: None,
            message: format!("event {id}"),
            notify: false,
        }
    }

    fn ids(events: &VecDeque<Event>) -> Vec<u64> {
        events.iter().map(|e| e.id).collect()
    }

    #[test]
    fn reconnecting_does_not_repeat_service_events() {
        let mut events = VecDeque::new();
        merge_events(&mut events, vec![event(1, 10), event(2, 20)], true);
        merge_events(&mut events, vec![event(3, 30)], false);
        // A local action between the service's events.
        events.push_back(event(0, 35));
        // Events already shown are not added twice.
        merge_events(&mut events, vec![event(3, 30), event(4, 40)], false);
        assert_eq!(ids(&events), [1, 2, 3, 0, 4]);
        // After reconnecting the backlog replaces the service events and the
        // local one keeps its place in time.
        merge_events(
            &mut events,
            (1..=5).map(|i| event(i, i * 10)).collect(),
            true,
        );
        assert_eq!(ids(&events), [1, 2, 3, 0, 4, 5]);
        // A restarted service may count from 1 again.
        merge_events(&mut events, vec![event(1, 100)], true);
        assert_eq!(ids(&events), [0, 1]);
        // Never more than are kept.
        merge_events(
            &mut events,
            (1..=EVENTS_KEPT as u64 + 10)
                .map(|i| event(i, 100 + i))
                .collect(),
            true,
        );
        assert_eq!(events.len(), EVENTS_KEPT);
        assert_eq!(events.back().map(|e| e.id), Some(EVENTS_KEPT as u64 + 10));
    }

    #[test]
    fn the_service_problem_is_read_from_the_connection_error() {
        use ServiceProblem::*;
        assert_eq!(service_problem(None), NotRunning);
        assert_eq!(
            service_problem(Some(
                "The system cannot find the file specified. (os error 2)"
            )),
            NotRunning
        );
        assert_eq!(
            service_problem(Some("service speaks protocol 12, client 13")),
            Older
        );
        assert_eq!(
            service_problem(Some("service speaks protocol 14, client 13")),
            Newer
        );
        assert_eq!(
            service_problem(Some("unexpected handshake reply: Done")),
            Older
        );
        assert_eq!(
            service_problem(Some("the pipe is not served by the RustWTM service")),
            Impostor
        );
        let data = Mutex::new(Data {
            service_error: Some("service speaks protocol 14, client 13".into()),
            ..Data::default()
        });
        let text = without_service(&data, NO_CONFIG);
        assert!(text.contains("newer than this window"), "{text}");
        assert!(text.contains("Restart RustWTM"), "{text}");
        data.lock().unwrap().service_error =
            Some("the pipe is not served by the RustWTM service".into());
        assert!(without_service(&data, NO_CONFIG).contains("another program"));
    }

    #[test]
    fn errors_are_worded_for_the_window() {
        let denied = "access denied to process 42 (try an elevated prompt)";
        assert_eq!(
            gui_error(denied, false),
            "access denied to process 42 (restart RustWTM as administrator)"
        );
        assert_eq!(
            gui_error(denied, true),
            "access denied to process 42 (Windows protects it even from administrators)"
        );
        assert_eq!(
            gui_error("process 42 no longer exists", false),
            "process 42 no longer exists"
        );
        let mut data = Data::default();
        report(
            &mut data,
            Command::Kill,
            vec![failed(&x_exe(), denied.into())],
            true,
        );
        let text = data.message.unwrap().1;
        assert!(
            text.ends_with("(restart RustWTM as administrator)"),
            "{text}"
        );
        assert!(data.events[0].message.ends_with("as administrator)"));

        let broken = io::Error::from_raw_os_error(109);
        assert_eq!(lost_connection(&broken), RECONNECTING);
        let eof = io::Error::from(io::ErrorKind::UnexpectedEof);
        assert_eq!(lost_connection(&eof), RECONNECTING);
        let odd = io::Error::other("unexpected response: Done");
        assert_eq!(
            lost_connection(&odd),
            "Lost connection to service: unexpected response: Done"
        );
    }

    #[test]
    fn slow_requests_do_not_hold_up_the_worker() {
        let pipe = test_pipe("slow");
        let snapshot = Sampler::new().sample().unwrap();
        // The main connection answers at once; the Telegram test takes long.
        let server = serve(&pipe, 2, move |request| match request {
            Request::Hello { .. } => hello(),
            Request::Snapshot => Response::Snapshot(snapshot.clone()),
            Request::Status => Response::Status(Status::default()),
            Request::Events { .. } => Response::Events(Vec::new()),
            Request::TestTelegram => {
                std::thread::sleep(Duration::from_millis(300));
                Response::Text("sent".into())
            }
            _ => Response::Done,
        });
        let mut w = worker();
        w.pipe = pipe.clone();
        w.client = Some(Client::connect_to(&pipe).unwrap());
        let started = Instant::now();
        w.handle(UiCommand::TestTelegram);
        assert!(started.elapsed() < Duration::from_millis(250));
        assert_eq!(background_message(&w).1, "sent");
        drop(w.client.take());
        server.join().unwrap();
    }

    #[test]
    fn fetch_rejects_unexpected_answers() {
        for stage in 0..3 {
            let pipe = test_pipe(&format!("fetch{stage}"));
            let server = serve(&pipe, 1, move |request| match (request, stage) {
                (Request::Hello { .. }, _) => hello(),
                (Request::Snapshot, 0) | (Request::Status, 1) | (Request::Events { .. }, 2) => {
                    Response::Series(Vec::new())
                }
                (Request::Snapshot, _) => Response::Error("no sample yet".into()),
                _ => Response::Status(Status::default()),
            });
            let mut w = worker();
            w.client = Some(Client::connect_to(&pipe).unwrap());
            w.tick();
            assert!(w.client.is_none(), "stage {stage}");
            let (_, text) = message(&w);
            assert!(
                text.starts_with("Lost connection to service: unexpected response"),
                "{text}"
            );
            server.join().unwrap();
        }
    }

    #[test]
    fn startup_and_service_messages() {
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        let missing = io::Error::from(io::ErrorKind::NotFound);
        assert_eq!(
            startup_message("App", true, Ok(())),
            (EventLevel::Info, "Enabled startup entry App".into())
        );
        assert_eq!(
            startup_message("App", false, Ok(())).1,
            "Disabled startup entry App"
        );
        assert!(
            startup_message("App", false, Err(denied))
                .1
                .contains("access denied")
        );
        assert_eq!(
            startup_message("App", false, Err(missing)).0,
            EventLevel::Warn
        );

        let nt = |status| wtm_core::Error::Nt { call: "x", status };
        assert_eq!(
            service_message(ServiceOp::Stop, "Fax", Ok(())),
            (EventLevel::Info, "Stop: Fax".into())
        );
        let (level, text) = service_message(ServiceOp::Start, "Fax", Err(nt(0xC000_0022)));
        assert_eq!(level, EventLevel::Warn);
        assert!(text.contains("access denied"), "{text}");
        let (_, text) = service_message(ServiceOp::Start, "Fax", Err(nt(0xC000_0001)));
        assert!(text.starts_with("Start Fax failed"), "{text}");
    }

    #[test]
    fn toggling_a_startup_entry_reports_and_reloads() {
        use winreg::RegKey;
        use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
        const APPROVED_RUN: &str =
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
        // The approval flag is written even without a matching Run value;
        // remove it again whatever happens.
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER)
                    .open_subkey_with_flags(APPROVED_RUN, KEY_SET_VALUE)
                {
                    let _ = key.delete_value(&self.0);
                }
            }
        }
        let name = format!("RustWTM-gui-test-{}", std::process::id());
        let _cleanup = Cleanup(name.clone());
        let mut w = worker();
        w.handle(UiCommand::SetStartup {
            entry: StartupEntry {
                name: name.clone(),
                command: "notepad.exe".into(),
                scope: wtm_core::startup::StartupScope::CurrentUser,
                source: wtm_core::startup::StartupSource::Run,
                enabled: true,
            },
            enabled: false,
        });
        assert_eq!(
            message(&w),
            (EventLevel::Info, format!("Disabled startup entry {name}"))
        );
        assert!(w.data().startup.is_some());
    }

    #[test]
    fn sampling_failure_keeps_the_last_sample_and_recovers_on_success() {
        let mut d = Data::default();
        publish_local(&mut d, Ok(crate::ui_tests::snapshot()));
        let received = d.sample_received_at;
        assert!(received.is_some());
        let snapshot = d.snapshot.clone().unwrap();
        let history = d.history.cpu.clone();
        let action_message = Some((EventLevel::Info, "Independent action result".into()));
        d.message = action_message.clone();
        d.mode = Mode::Service {
            admin: true,
            verified: true,
        };
        publish_local(
            &mut d,
            Err(wtm_core::Error::Nt {
                call: "sample",
                status: 1,
            }),
        );
        assert_eq!(d.mode, Mode::Local);
        assert!(d.sample_error.is_some());
        assert_eq!(d.sample_received_at, received);
        assert!(Arc::ptr_eq(d.snapshot.as_ref().unwrap(), &snapshot));
        assert_eq!(d.history.cpu, history);
        assert_eq!(d.message, action_message);
        assert!(matches!(
            d.sample_freshness(received.unwrap()),
            SampleFreshness::Stale(Some(_))
        ));
        publish_local(&mut d, Ok(crate::ui_tests::snapshot()));
        assert!(d.sample_error.is_none());
        assert_eq!(d.message, action_message);
        assert!(matches!(
            d.sample_freshness(d.sample_received_at.unwrap()),
            SampleFreshness::Current(_)
        ));
    }

    #[test]
    fn freshness_uses_elapsed_time_and_allows_slow_sampling() {
        let now = Instant::now();
        let mut d = Data::default();
        assert_eq!(d.sample_freshness(now), SampleFreshness::Waiting);
        d.sample_error = Some("unavailable".into());
        assert_eq!(d.sample_freshness(now), SampleFreshness::Stale(None));
        d.sample_error = None;
        d.sample_received_at = Some(now);
        d.snapshot = Some(Arc::new(crate::ui_tests::snapshot()));
        assert_eq!(
            d.sample_freshness(now + Duration::from_millis(4999)),
            SampleFreshness::Current(Duration::from_millis(4999))
        );
        assert_eq!(
            d.sample_freshness(now + Duration::from_secs(5)),
            SampleFreshness::Stale(Some(Duration::from_secs(5)))
        );
        Arc::make_mut(d.snapshot.as_mut().unwrap()).interval = Some(Duration::from_secs(10));
        assert_eq!(
            d.sample_freshness(now + Duration::from_secs(29)),
            SampleFreshness::Current(Duration::from_secs(29))
        );
        assert_eq!(
            d.sample_freshness(now + Duration::from_secs(30)),
            SampleFreshness::Stale(Some(Duration::from_secs(30)))
        );
    }

    #[test]
    fn local_samples_history_and_notifications() {
        let mut d = Data::default();
        publish_local(
            &mut d,
            Err(wtm_core::Error::Nt {
                call: "x",
                status: 1,
            }),
        );
        assert!(d.sample_error.as_ref().unwrap().contains('x'));

        let mut s = crate::ui_tests::snapshot();
        s.system.memory_total = 0;
        publish_local(&mut d, Ok(s));
        assert_eq!(d.mode, Mode::Local);
        assert_eq!(d.history.memory, [0.0]);
        // One small history per logical processor of the sample.
        assert_eq!(d.history.cores.len(), 4);
        assert_eq!(d.history.cores[0], [95.0]);

        let mut q: VecDeque<u32> = (0..5).collect();
        trim(&mut q, 3);
        assert_eq!(q, [2, 3, 4]);

        assert_eq!(unexpected(Response::Error("e".into())).to_string(), "e");
        let mut e = Event {
            id: 1,
            unix_ms: 0,
            level: EventLevel::Info,
            pid: Some(5),
            name: Some("x.exe".into()),
            message: "body".into(),
            notify: true,
        };
        let n = notification(&e);
        assert_eq!(
            (n.summary.as_str(), n.body.as_str()),
            ("RustWTM: x.exe (5)", "body")
        );
        e.pid = None;
        assert_eq!(notification(&e).summary, "RustWTM");
    }

    static OPENED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    #[test]
    fn open_location_shows_the_image_or_reports_why_not() {
        let mut w = worker();
        w.open = |path| {
            OPENED.lock().unwrap().push(path.to_owned());
            Ok(())
        };
        let me = ProcessRef {
            pid: std::process::id(),
            create_time: wtm_sys::control::ProcessHandle::open(
                std::process::id(),
                wtm_sys::control::Access::QUERY,
            )
            .unwrap()
            .create_time()
            .unwrap(),
            name: "me.exe".into(),
        };
        w.handle(UiCommand::OpenLocation(me.clone()));
        let exe = std::env::current_exe().unwrap();
        assert_eq!(
            OPENED.lock().unwrap().as_slice(),
            [exe.display().to_string()]
        );
        assert!(w.data().message.is_none());

        w.open = |_| Err(io::Error::other("no explorer"));
        w.handle(UiCommand::OpenLocation(me));
        assert!(message(&w).1.ends_with("no explorer"));

        w.handle(UiCommand::OpenLocation(ProcessRef {
            pid: 0xFFFF_FFF0,
            create_time: 1,
            name: "gone.exe".into(),
        }));
        let (level, text) = message(&w);
        assert_eq!(level, EventLevel::Error);
        assert!(
            text.starts_with("Cannot open the location of gone.exe"),
            "{text}"
        );
    }

    #[test]
    fn blocked_notification_names_the_program() {
        let n = blocked_notification("xmrig.exe");
        assert_eq!(n.summary, "Program blocked");
        assert_eq!(n.body, "RustWTM prevented xmrig.exe from starting.");
    }

    #[test]
    fn notifications_follow_language_and_preserve_program_names() {
        use crate::i18n::{Language, scope};
        for (language, title) in [
            (Language::Turkish, "Program engellendi"),
            (Language::German, "Programm blockiert"),
            (Language::French, "Programme bloqué"),
            (Language::Spanish, "Programa bloqueado"),
        ] {
            let _language = scope(language);
            let n = blocked_notification("çğİıöşü-{0}.exe");
            assert_eq!(n.summary, title);
            assert!(n.body.contains("çğİıöşü-{0}.exe"));
            let event = Event {
                id: 1,
                unix_ms: 0,
                level: EventLevel::Info,
                pid: Some(5),
                name: Some("çğİıöşü.exe".into()),
                message: "blocked from starting: çğİıöşü.exe".into(),
                notify: true,
            };
            let n = notification(&event);
            assert!(n.summary.contains("çğİıöşü.exe (5)"));
            assert_ne!(n.body, event.message);
            assert!(n.body.contains("çğİıöşü.exe"));
        }
    }

    #[test]
    fn command_lines_are_read_once_and_forgotten_on_exit() {
        let snap = Sampler::new().sample().unwrap();
        let mut cache = CommandLines::new();
        update_command_lines(&mut cache, &snap.processes);
        let me = snap
            .processes
            .iter()
            .find(|p| p.pid == std::process::id())
            .unwrap();
        assert!(!cache[&(me.pid, me.create_time)].is_empty());
        // Cached: an entry is not read again.
        cache.insert((me.pid, me.create_time), "cached".into());
        update_command_lines(&mut cache, &snap.processes);
        assert_eq!(cache[&(me.pid, me.create_time)], "cached");
        update_command_lines(&mut cache, &[]);
        assert!(cache.is_empty());
    }

    #[test]
    fn new_processes_are_looked_up_outside_the_lock_once() {
        let snap = Sampler::new().sample().unwrap();
        let data = Mutex::new(Data::default());
        let lookups = Lookups::fetch(&data, &snap.processes);
        assert_eq!(lookups.command_lines.len(), snap.processes.len());
        assert_eq!(lookups.image_paths.len(), snap.processes.len());
        lookups.apply(&mut data.lock().unwrap());
        let me = (std::process::id(), {
            let p = snap.processes.iter().find(|p| p.pid == std::process::id());
            p.unwrap().create_time
        });
        let d = data.lock().unwrap();
        assert!(d.image_paths[&me].to_lowercase().ends_with(".exe"));
        assert!(!d.command_lines[&me].is_empty());
        drop(d);
        let again = Lookups::fetch(&data, &snap.processes);
        assert!(again.command_lines.is_empty() && again.image_paths.is_empty());
    }

    #[test]
    fn image_paths_and_icons_are_loaded_once() {
        let snap = Sampler::new().sample().unwrap();
        let data = Mutex::new(Data::default());
        update_image_paths(&mut data.lock().unwrap().image_paths, &snap.processes);
        let me = snap
            .processes
            .iter()
            .find(|p| p.pid == std::process::id())
            .unwrap();
        let path = data.lock().unwrap().image_paths[&(me.pid, me.create_time)].clone();
        assert!(path.to_lowercase().ends_with(".exe"), "{path}");
        refresh_icons(&data);
        assert!(data.lock().unwrap().icons.contains_key(&path));
        // Nothing new: no further work.
        let before = data.lock().unwrap().icons.len();
        refresh_icons(&data);
        assert_eq!(data.lock().unwrap().icons.len(), before);
        update_image_paths(&mut data.lock().unwrap().image_paths, &[]);
        assert!(data.lock().unwrap().image_paths.is_empty());
    }
}

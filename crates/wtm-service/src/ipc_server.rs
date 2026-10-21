//! Named-pipe server: answers read-only requests from shared state and
//! forwards control requests (admin only) to the engine thread.

use std::fs::OpenOptions;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Context;
use tracing::{debug, warn};
use wtm_core::Snapshot;
use wtm_core::control::{self, ProcessRef};
use wtm_ipc::{
    Command, Outcome, PIPE_SDDL, PROTOCOL_VERSION, Request, Response, Status, read_frame,
    write_frame,
};
use wtm_sys::pipe::{PipeConnection, PipeListener};

use crate::events::EventLog;

const MAX_CLIENTS: usize = 16;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);

/// State published by the engine for clients.
#[derive(Debug, Default)]
pub struct Shared {
    pub hardware: crate::hardware::HardwareHub,
    pub snapshot: RwLock<Option<Arc<Snapshot>>>,
    pub status: RwLock<Status>,
    pub events: EventLog,
    /// Current long-term findings.
    pub findings: RwLock<Vec<wtm_policy::Finding>>,
    /// System-wide load of the last `RECENT_LEN` samples, oldest first.
    pub recent: RwLock<std::collections::VecDeque<wtm_ipc::SystemPoint>>,
    /// The configuration in force.
    pub config: RwLock<wtm_policy::Config>,
    /// History database, opened read-only per request.
    pub history_path: RwLock<Option<std::path::PathBuf>>,
    /// Per-file disk tracing while clients ask for it.
    pub disk_trace: std::sync::Mutex<Option<DiskTrace>>,
}

/// A running per-file disk trace and when it was last read.
#[derive(Debug)]
pub struct DiskTrace {
    tracer: wtm_core::DiskTracer,
    taken: std::time::Instant,
}

impl DiskTrace {
    /// No client asked for this long.
    pub fn idle(&self, after: std::time::Duration) -> bool {
        self.taken.elapsed() >= after
    }
}

/// Prefix of the per-file disk trace session.
pub const DISK_TRACE: &str = "RustWTM-Disk";

fn disk_activity(shared: &Shared) -> Response {
    let mut trace = shared
        .disk_trace
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if trace.is_none() {
        let name = format!("{DISK_TRACE}-{}", std::process::id());
        match wtm_core::DiskTracer::start(&name) {
            Ok(tracer) => {
                *trace = Some(DiskTrace {
                    tracer,
                    taken: std::time::Instant::now(),
                });
            }
            Err(e) => return Response::Error(e.to_string()),
        }
    }
    let t = trace.as_mut().expect("started above");
    let seconds = t.taken.elapsed().as_secs_f64();
    t.taken = std::time::Instant::now();
    Response::DiskActivity {
        files: t.tracer.take(),
        seconds,
    }
}

pub enum EngineMsg {
    Stop,
    /// Processes that appeared since the last watcher poll.
    Started(Vec<wtm_core::ProcessInfo>),
    Control {
        target: ProcessRef,
        command: Command,
        reply: Sender<Vec<Outcome>>,
    },
    /// The client's foreground process (already checked to be in its
    /// session), and whether it is full screen.
    Foreground(u32, bool),
}

pub struct Server {
    stop: Arc<AtomicBool>,
    pipe: String,
    thread: Option<JoinHandle<()>>,
}

pub fn start(shared: Arc<Shared>, engine: Sender<EngineMsg>, pipe: &str) -> anyhow::Result<Server> {
    let mut listener = PipeListener::bind(pipe, PIPE_SDDL)
        .with_context(|| format!("binding {pipe} (is another instance running?)"))?;
    let stop = Arc::new(AtomicBool::new(false));
    let clients = Arc::new(AtomicUsize::new(0));

    let stop_flag = stop.clone();
    let thread = std::thread::Builder::new()
        .name("ipc-accept".into())
        .spawn(move || {
            loop {
                let conn = listener.accept();
                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }
                let conn = match conn {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(error = %e, "pipe accept failed");
                        std::thread::sleep(Duration::from_secs(1));
                        continue;
                    }
                };
                if clients.fetch_add(1, Ordering::SeqCst) >= MAX_CLIENTS {
                    clients.fetch_sub(1, Ordering::SeqCst);
                    warn!("too many IPC clients; connection dropped");
                    continue;
                }
                let (shared, engine, clients) = (shared.clone(), engine.clone(), clients.clone());
                let spawned =
                    std::thread::Builder::new()
                        .name("ipc-client".into())
                        .spawn(move || {
                            if let Err(e) = serve(conn, &shared, &engine) {
                                debug!(error = %e, "IPC client disconnected");
                            }
                            clients.fetch_sub(1, Ordering::SeqCst);
                        });
                if spawned.is_err() {
                    warn!("could not spawn IPC client thread");
                }
            }
        })?;

    Ok(Server {
        stop,
        pipe: pipe.to_owned(),
        thread: Some(thread),
    })
}

impl Server {
    pub fn shutdown(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop with a throwaway connection.
        let _ = OpenOptions::new().read(true).write(true).open(&self.pipe);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn read_lock<T: Clone>(lock: &RwLock<T>) -> T {
    lock.read().unwrap_or_else(PoisonError::into_inner).clone()
}

fn serve(mut conn: PipeConnection, shared: &Shared, engine: &Sender<EngineMsg>) -> io::Result<()> {
    let Request::Hello { version } = read_frame(&mut conn)? else {
        return write_frame(&mut conn, &Response::Error("expected Hello".into()));
    };
    let admin = conn.client_is_admin().unwrap_or(false);
    write_frame(
        &mut conn,
        &Response::Hello {
            version: PROTOCOL_VERSION,
            admin,
        },
    )?;
    if version != PROTOCOL_VERSION {
        return Ok(());
    }

    loop {
        let request: Request = match read_frame(&mut conn) {
            Ok(r) => r,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        let response = match request {
            Request::Hello { .. } => Response::Error("already connected".into()),
            Request::Snapshot => match read_lock(&shared.snapshot) {
                Some(s) => Response::Snapshot((*s).clone()),
                None => Response::Error("no snapshot yet".into()),
            },
            Request::Status => Response::Status(read_lock(&shared.status)),
            Request::Hardware => Response::Hardware(shared.hardware.snapshot()),
            Request::SetCooling { .. } if !admin => {
                Response::Error("administrator rights required".into())
            }
            Request::SetCooling { channel, profile } => {
                let description = format!("Cooling {channel}: {:?}", profile.mode);
                match shared.hardware.set(channel, profile) {
                    Ok(()) => {
                        shared.events.info(None, format!("{description} applied"));
                        Response::Done
                    }
                    Err(e) => {
                        shared
                            .events
                            .warn(None, format!("{description} rejected: {e}"));
                        Response::Error(e)
                    }
                }
            }
            Request::Recent => Response::Recent(read_lock(&shared.recent).into()),
            Request::Events { after } => Response::Events(shared.events.after(after)),
            // Inspected with the client's rights, not the service's.
            Request::Inspect(target) => match conn.as_client(|| control::inspect(&target)) {
                Ok(Ok(d)) => Response::Details(d),
                Ok(Err(e)) => Response::Error(e.to_string()),
                Err(e) => Response::Error(e.to_string()),
            },
            Request::History { hours } => {
                match history_query(shared, hours, |conn, since, cpus| {
                    crate::history::stats(conn, since, cpus)
                }) {
                    Ok(stats) => Response::History {
                        stats,
                        findings: read_lock(&shared.findings),
                    },
                    Err(e) => Response::Error(e),
                }
            }
            Request::AppSeries { app, hours } => {
                match history_query(shared, hours, |conn, since, _| {
                    crate::history::series(conn, &app, since)
                }) {
                    Ok(rows) => Response::Series(rows),
                    Err(e) => Response::Error(e),
                }
            }
            Request::GetConfig => get_config(shared),
            Request::Foreground { pid, fullscreen } => {
                foreground(shared, engine, pid, fullscreen, conn.client_session_id())
            }
            Request::SetConfig { .. } if !admin => {
                Response::Error("administrator rights required (run the client elevated)".into())
            }
            Request::SetConfig { toml } => set_config(shared, &toml),
            Request::GetTelegram | Request::SetTelegram(_) | Request::TestTelegram if !admin => {
                Response::Error("administrator rights required (run the client elevated)".into())
            }
            Request::GetTelegram => match telegram_path(shared) {
                Some(path) => match wtm_policy::TelegramConfig::load(&path) {
                    Ok(c) => Response::Telegram(c),
                    Err(e) => Response::Error(e),
                },
                None => Response::Error("configuration is not available yet".into()),
            },
            Request::SetTelegram(cfg) => set_telegram(shared, &cfg),
            Request::TestTelegram => match telegram_path(shared)
                .ok_or_else(|| "configuration is not available yet".to_owned())
                .and_then(|p| wtm_policy::TelegramConfig::load(&p))
                .and_then(|c| crate::telegram::test(&c))
            {
                Ok(text) => Response::Text(text),
                Err(e) => Response::Error(e),
            },
            Request::DiskActivity => disk_activity(shared),
            Request::FirewallBlocked => match wtm_core::firewall::blocked() {
                Ok(list) => Response::Blocked(list),
                Err(e) => Response::Error(e.to_string()),
            },
            Request::Firewall { .. } if !admin => {
                Response::Error("administrator rights required (run the client elevated)".into())
            }
            Request::Firewall { path, block } => {
                let result = if block {
                    wtm_core::firewall::block(&path)
                } else {
                    wtm_core::firewall::unblock(&path)
                };
                match result {
                    Ok(()) => {
                        shared.events.info(
                            None,
                            format!(
                                "network {} for {path}",
                                if block { "blocked" } else { "unblocked" }
                            ),
                        );
                        Response::Done
                    }
                    Err(e) => Response::Error(e.to_string()),
                }
            }
            Request::CloseConnection { .. } if !admin => {
                Response::Error("administrator rights required (run the client elevated)".into())
            }
            Request::CloseConnection { local, remote } => {
                match wtm_core::close_tcp(local, remote) {
                    Ok(()) => {
                        shared
                            .events
                            .info(None, format!("connection {local} -> {remote} closed"));
                        Response::Done
                    }
                    Err(e) => Response::Error(e.to_string()),
                }
            }
            Request::Control { .. } if !admin => {
                Response::Error("administrator rights required (run the client elevated)".into())
            }
            Request::Control { target, command } => {
                forward_control(engine, target, command, CONTROL_TIMEOUT)
            }
        };
        write_frame(&mut conn, &response)?;
    }
}

/// Hands a control request to the engine thread and waits for its outcomes.
fn forward_control(
    engine: &Sender<EngineMsg>,
    target: ProcessRef,
    command: Command,
    timeout: Duration,
) -> Response {
    let (reply, rx) = mpsc::channel();
    if engine
        .send(EngineMsg::Control {
            target,
            command,
            reply,
        })
        .is_err()
    {
        return Response::Error("service is stopping".into());
    }
    match rx.recv_timeout(timeout) {
        Ok(outcomes) => Response::Control(outcomes),
        Err(_) => Response::Error("service did not answer in time".into()),
    }
}

/// Forwards a foreground report if `pid` runs in the client's session, so a
/// client cannot shield processes of other users.
fn foreground(
    shared: &Shared,
    engine: &Sender<EngineMsg>,
    pid: u32,
    fullscreen: bool,
    client_session: wtm_sys::Result<u32>,
) -> Response {
    let session = read_lock(&shared.snapshot).and_then(|s| {
        s.processes
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| p.session_id)
    });
    match (session, client_session) {
        (Some(s), Ok(c)) if s == c => {
            let _ = engine.send(EngineMsg::Foreground(pid, fullscreen));
            Response::Done
        }
        (None, _) => Response::Error(format!("process {pid} not found")),
        _ => Response::Error("not a process of your session".into()),
    }
}

fn config_path(shared: &Shared) -> Option<std::path::PathBuf> {
    let path = read_lock(&shared.status).config_path;
    (!path.is_empty()).then(|| path.into())
}

fn get_config(shared: &Shared) -> Response {
    let Some(path) = config_path(shared) else {
        return Response::Error("configuration is not available yet".into());
    };
    match std::fs::read_to_string(&path) {
        Ok(toml) => Response::Config {
            toml,
            path: path.display().to_string(),
        },
        Err(e) => Response::Error(format!("reading {}: {e}", path.display())),
    }
}

/// Validates `toml` and replaces the file atomically, so the engine never
/// reads a half-written configuration.
fn set_config(shared: &Shared, toml: &str) -> Response {
    if let Err(e) = wtm_policy::Config::from_toml(toml) {
        return Response::Error(format!("invalid configuration: {e}"));
    }
    let Some(path) = config_path(shared) else {
        return Response::Error("configuration is not available yet".into());
    };
    let tmp = path.with_extension("toml.new");
    let written = std::fs::write(&tmp, toml).and_then(|()| std::fs::rename(&tmp, &path));
    match written {
        Ok(()) => {
            shared.events.info(None, "configuration replaced via IPC");
            Response::Done
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Response::Error(format!("writing {}: {e}", path.display()))
        }
    }
}

/// `telegram.toml` next to the configuration file.
fn telegram_path(shared: &Shared) -> Option<std::path::PathBuf> {
    config_path(shared).map(|p| p.with_file_name("telegram.toml"))
}

/// Validates `cfg` and replaces `telegram.toml`, readable only by SYSTEM
/// and administrators before it gets its name.
fn set_telegram(shared: &Shared, cfg: &wtm_policy::TelegramConfig) -> Response {
    if let Err(e) = cfg.validate() {
        return Response::Error(e);
    }
    let Some(path) = telegram_path(shared) else {
        return Response::Error("configuration is not available yet".into());
    };
    let tmp = path.with_extension("toml.new");
    let written = std::fs::write(&tmp, cfg.to_toml())
        .and_then(|()| {
            wtm_sys::security::set_dacl(&tmp, wtm_policy::TELEGRAM_SDDL)
                .map_err(|e| io::Error::other(e.to_string()))
        })
        .and_then(|()| std::fs::rename(&tmp, &path));
    match written {
        Ok(()) => {
            shared
                .events
                .info(None, "Telegram settings replaced via IPC");
            Response::Done
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Response::Error(format!("writing {}: {e}", path.display()))
        }
    }
}

/// Runs `f` on a fresh read-only connection with the window start minute.
fn history_query<T>(
    shared: &Shared,
    hours: u32,
    f: impl FnOnce(&rusqlite::Connection, i64, u32) -> rusqlite::Result<T>,
) -> Result<T, String> {
    let path = read_lock(&shared.history_path).ok_or("usage history is not available")?;
    let conn =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
    let cpus = read_lock(&shared.snapshot).map_or(1, |s| s.system.logical_cpus);
    let now_minute = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64 / 60);
    let since = now_minute - i64::from(hours.clamp(1, 24 * 90)) * 60;
    f(&conn, since, cpus).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::sync::mpsc::Receiver;
    use wtm_core::control::PriorityClass;
    use wtm_policy::DEFAULT_CONFIG_TOML;

    use crate::test_util::wait_for;

    /// A server on a pipe unique to this test; the engine end is returned.
    fn server(test: &str) -> (Server, String, Receiver<EngineMsg>) {
        let pipe = format!(r"\\.\pipe\wtm-ipc-{test}-{}", std::process::id());
        let (tx, rx) = mpsc::channel();
        let server = start(Arc::new(Shared::default()), tx, &pipe).unwrap();
        (server, pipe, rx)
    }

    /// A raw client: the tests speak the protocol by hand.
    fn open(pipe: &str) -> File {
        // Busy while the server creates the next instance.
        wait_for(pipe, || {
            OpenOptions::new().read(true).write(true).open(pipe).ok()
        })
    }

    fn call(f: &mut File, request: &Request) -> io::Result<Response> {
        write_frame(f, request)?;
        read_frame(f)
    }

    fn hello(pipe: &str) -> (File, bool) {
        let mut f = open(pipe);
        let hello = Request::Hello {
            version: PROTOCOL_VERSION,
        };
        let r = call(&mut f, &hello).unwrap();
        let admin = matches!(r, Response::Hello { admin: true, .. });
        assert!(
            matches!(r, Response::Hello { version, .. } if version == PROTOCOL_VERSION),
            "{r:?}"
        );
        (f, admin)
    }

    fn error(r: Response) -> String {
        expect_reply!(r, Response::Error)
    }

    #[test]
    fn answers_requests_before_the_engine_has_published_anything() {
        let (server, pipe, rx) = server("empty");
        drop(rx); // the engine is gone
        let (mut f, admin) = hello(&pipe);

        let again = Request::Hello { version: 0 };
        assert_eq!(error(call(&mut f, &again).unwrap()), "already connected");
        assert_eq!(
            error(call(&mut f, &Request::Snapshot).unwrap()),
            "no snapshot yet"
        );
        let status = expect_reply!(call(&mut f, &Request::Status).unwrap(), Response::Status);
        assert_eq!(status, Status::default());
        let events = call(&mut f, &Request::Events { after: 0 }).unwrap();
        assert!(expect_reply!(events, Response::Events).is_empty());

        let gone = ProcessRef {
            pid: 0xFFFF_FFF0,
            create_time: 1,
            name: "gone.exe".into(),
        };
        assert!(!error(call(&mut f, &Request::Inspect(gone.clone())).unwrap()).is_empty());
        let history = Request::History { hours: 1 };
        let unavailable = "usage history is not available";
        assert_eq!(error(call(&mut f, &history).unwrap()), unavailable);
        let series = Request::AppSeries {
            app: "a.exe".into(),
            hours: 1,
        };
        assert_eq!(error(call(&mut f, &series).unwrap()), unavailable);
        let no_config = "configuration is not available yet";
        assert_eq!(error(call(&mut f, &Request::GetConfig).unwrap()), no_config);
        let set = Request::SetConfig {
            toml: DEFAULT_CONFIG_TOML.into(),
        };
        let refusal = error(call(&mut f, &set).unwrap());
        let expected = if admin { no_config } else { "administrator" };
        assert!(refusal.contains(expected), "{refusal}");

        let control = Request::Control {
            target: gone,
            command: Command::Kill,
        };
        let refusal = error(call(&mut f, &control).unwrap());
        let expected = if admin { "stopping" } else { "administrator" };
        assert!(refusal.contains(expected), "{refusal}");
        let expected = if admin { no_config } else { "administrator" };
        let set = Request::SetTelegram(wtm_policy::TelegramConfig::default());
        for request in [Request::GetTelegram, set, Request::TestTelegram] {
            let refusal = error(call(&mut f, &request).unwrap());
            assert!(refusal.contains(expected), "{request:?}: {refusal}");
        }
        let profile = wtm_policy::CoolingProfile {
            mode: wtm_policy::CoolingMode::Manual(50.0),
            sensor_id: "temp".into(),
            ..Default::default()
        };
        let request = Request::SetCooling {
            channel: "fan".into(),
            profile,
        };
        let refusal = error(call(&mut f, &request).unwrap());
        assert!(
            refusal.contains(if admin { "monitoring" } else { "administrator" }),
            "{refusal}"
        );

        // A malformed frame ends the session.
        f.write_all(&u32::MAX.to_le_bytes()).unwrap();
        assert!(read_frame::<Response>(&mut f).is_err());
        server.shutdown();
    }

    #[test]
    fn rejects_sessions_without_a_matching_handshake() {
        let (server, pipe, _rx) = server("handshake");
        let mut f = open(&pipe);
        assert_eq!(
            error(call(&mut f, &Request::Status).unwrap()),
            "expected Hello"
        );
        assert!(read_frame::<Response>(&mut f).is_err(), "closed");

        // A different protocol version gets the server's version, then EOF.
        let mut f = open(&pipe);
        let r = call(&mut f, &Request::Hello { version: 0 }).unwrap();
        assert!(
            matches!(r, Response::Hello { version, .. } if version == PROTOCOL_VERSION),
            "{r:?}"
        );
        assert!(call(&mut f, &Request::Status).is_err(), "closed");

        // A client that vanishes before the handshake reply does no harm.
        let mut f = open(&pipe);
        write_frame(
            &mut f,
            &Request::Hello {
                version: PROTOCOL_VERSION,
            },
        )
        .unwrap();
        drop(f);
        hello(&pipe);
        server.shutdown();
    }

    #[test]
    fn limits_concurrent_clients() {
        let (server, pipe, _rx) = server("limit");
        let idle: Vec<File> = (0..MAX_CLIENTS).map(|_| open(&pipe)).collect();
        // One too many: dropped without a word.
        let mut extra = open(&pipe);
        let mut buf = [0u8; 1];
        assert!(!matches!(extra.read(&mut buf), Ok(n) if n > 0));

        // Slots free up as soon as clients leave.
        drop(idle);
        let hello = Request::Hello {
            version: PROTOCOL_VERSION,
        };
        wait_for("a free slot", || call(&mut open(&pipe), &hello).ok());
        server.shutdown();
    }

    #[test]
    fn binding_a_taken_pipe_fails() {
        let (server, pipe, _rx) = server("taken");
        let (tx, _rx2) = mpsc::channel();
        assert!(start(Arc::new(Shared::default()), tx, &pipe).is_err());
        server.shutdown();
    }

    #[test]
    fn control_requests_are_forwarded_to_the_engine() {
        let target = ProcessRef {
            pid: 1234,
            create_time: 5,
            name: "app.exe".into(),
        };
        let command = Command::Priority(PriorityClass::Idle);
        let forward = |engine: &Sender<EngineMsg>, timeout| {
            forward_control(engine, target.clone(), command, timeout)
        };

        let (tx, rx) = mpsc::channel();
        let engine = std::thread::spawn(move || {
            if let Ok(EngineMsg::Control {
                target,
                command,
                reply,
            }) = rx.recv()
            {
                assert_eq!(command, Command::Priority(PriorityClass::Idle));
                reply
                    .send(vec![Outcome {
                        target,
                        result: Ok(()),
                    }])
                    .unwrap();
            }
        });
        let outcomes = expect_reply!(forward(&tx, CONTROL_TIMEOUT), Response::Control);
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].target, target);
        engine.join().unwrap();

        // The engine is busy and never answers.
        let (tx, busy) = mpsc::channel();
        let late = error(forward(&tx, Duration::from_millis(50)));
        assert_eq!(late, "service did not answer in time");
        drop(busy);
        // The engine has stopped.
        assert_eq!(error(forward(&tx, CONTROL_TIMEOUT)), "service is stopping");
    }

    #[test]
    fn config_is_read_validated_and_replaced_atomically() {
        let dir = std::env::temp_dir().join(format!("wtm-ipc-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, DEFAULT_CONFIG_TOML).unwrap();
        let shared = Shared::default();
        shared.status.write().unwrap().config_path = path.display().to_string();

        let (toml, shown) = match get_config(&shared) {
            Response::Config { toml, path } => (toml, path),
            other => panic!("{other:?}"),
        };
        assert_eq!(toml, DEFAULT_CONFIG_TOML);
        assert_eq!(shown, path.display().to_string());

        // Invalid text is rejected and the file is left alone.
        let invalid = error(set_config(&shared, "dry_run = 3"));
        assert!(invalid.starts_with("invalid configuration"), "{invalid}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG_TOML);

        let new = DEFAULT_CONFIG_TOML.replace("dry_run = true", "dry_run = false");
        assert!(matches!(set_config(&shared, &new), Response::Done));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), new);
        assert!(!path.with_extension("toml.new").exists());
        assert!(
            shared
                .events
                .after(0)
                .iter()
                .any(|e| e.message == "configuration replaced via IPC")
        );

        // A directory in place of the file: reading and writing fail cleanly.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(error(get_config(&shared)).starts_with("reading"));
        assert!(error(set_config(&shared, DEFAULT_CONFIG_TOML)).starts_with("writing"));
        assert!(!path.with_extension("toml.new").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn foreground_reports_are_limited_to_the_clients_session() {
        let shared = Shared::default();
        let snap = wtm_core::Sampler::new().sample().unwrap();
        let me = snap
            .processes
            .iter()
            .find(|p| p.pid == std::process::id())
            .unwrap()
            .clone();
        *shared.snapshot.write().unwrap() = Some(Arc::new(snap));
        let (tx, rx) = mpsc::channel();
        assert!(matches!(
            foreground(&shared, &tx, me.pid, true, Ok(me.session_id)),
            Response::Done
        ));
        assert!(matches!(rx.try_recv(), Ok(EngineMsg::Foreground(pid, true)) if pid == me.pid));
        let other = error(foreground(
            &shared,
            &tx,
            me.pid,
            false,
            Ok(me.session_id + 1),
        ));
        assert_eq!(other, "not a process of your session");
        let failed = wtm_sys::Error::Nt {
            call: "x",
            status: 1,
        };
        assert!(error(foreground(&shared, &tx, me.pid, false, Err(failed))).contains("session"));
        assert!(error(foreground(&shared, &tx, 0xFFFF_FFF0, false, Ok(0))).contains("not found"));
        assert!(rx.try_recv().is_err(), "rejected reports are not forwarded");
    }
}

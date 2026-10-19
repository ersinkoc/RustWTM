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
    Command, Outcome, PIPE_NAME, PIPE_SDDL, PROTOCOL_VERSION, Request, Response, Status,
    read_frame, write_frame,
};
use wtm_sys::pipe::{PipeConnection, PipeListener};

use crate::events::EventLog;

const MAX_CLIENTS: usize = 16;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);

/// State published by the engine for clients.
#[derive(Debug, Default)]
pub struct Shared {
    pub snapshot: RwLock<Option<Arc<Snapshot>>>,
    pub status: RwLock<Status>,
    pub events: EventLog,
    /// Current long-term findings.
    pub findings: RwLock<Vec<wtm_policy::Finding>>,
    /// History database, opened read-only per request.
    pub history_path: RwLock<Option<std::path::PathBuf>>,
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
}

pub struct Server {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

pub fn start(shared: Arc<Shared>, engine: Sender<EngineMsg>) -> anyhow::Result<Server> {
    let mut listener = PipeListener::bind(PIPE_NAME, PIPE_SDDL)
        .with_context(|| format!("binding {PIPE_NAME} (is another instance running?)"))?;
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
        thread: Some(thread),
    })
}

impl Server {
    pub fn shutdown(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop with a throwaway connection.
        let _ = OpenOptions::new().read(true).write(true).open(PIPE_NAME);
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
            Request::Control { .. } if !admin => {
                Response::Error("administrator rights required (run the client elevated)".into())
            }
            Request::Control { target, command } => {
                let (reply, rx) = mpsc::channel();
                if engine
                    .send(EngineMsg::Control {
                        target,
                        command,
                        reply,
                    })
                    .is_err()
                {
                    Response::Error("service is stopping".into())
                } else {
                    match rx.recv_timeout(CONTROL_TIMEOUT) {
                        Ok(outcomes) => Response::Control(outcomes),
                        Err(_) => Response::Error("service did not answer in time".into()),
                    }
                }
            }
        };
        write_frame(&mut conn, &response)?;
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

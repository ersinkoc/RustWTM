//! Protocol between the RustWTM service and its clients (GUI, CLI).
//!
//! Transport: the local named pipe [`PIPE_NAME`]; every message is a
//! little-endian `u32` length followed by a postcard-encoded body. The client
//! opens with [`Request::Hello`]. Read-only requests are open to interactive
//! users; [`Request::Control`] requires an elevated administrator.

#![forbid(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use wtm_core::Snapshot;
use wtm_core::control::{EcoQos, Limits, PriorityClass, ProcessDetails, ProcessRef};
use wtm_policy::{AppStats, Finding, MinuteRow, Source};

pub const PIPE_NAME: &str = r"\\.\pipe\RustWTM";

/// Name of the Windows service that owns [`PIPE_NAME`].
pub const SERVICE_NAME: &str = "RustWTM";

/// SYSTEM and Administrators: full; interactive users: read/write (requests
/// are authorized individually by the server).
pub const PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)";

pub const PROTOCOL_VERSION: u32 = 11;

const MAX_FRAME: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    Hello {
        version: u32,
    },
    Snapshot,
    Status,
    /// Events with an id greater than `after`.
    Events {
        after: u64,
    },
    /// Evaluated with the client's own access rights.
    Inspect(ProcessRef),
    Control {
        target: ProcessRef,
        command: Command,
    },
    /// Per-app usage summaries over the last `hours`.
    History {
        hours: u32,
    },
    /// Per-minute rows of one app over the last `hours`.
    AppSeries {
        app: String,
        hours: u32,
    },
    /// The service's configuration file as text.
    GetConfig,
    /// Validates and replaces the configuration file (admin only). The service
    /// reloads it within one sampling interval.
    SetConfig {
        toml: String,
    },
    /// The last few minutes of system-wide load, one point per sample.
    Recent,
    /// The Telegram bot settings, token included (admin only).
    GetTelegram,
    /// Validates and replaces the Telegram bot settings (admin only).
    SetTelegram(wtm_policy::TelegramConfig),
    /// Sends a test message to every allowed chat with the saved settings
    /// (admin only).
    TestTelegram,
    /// The process owning the client's foreground window; the governor
    /// leaves that app alone. Accepted only for processes in the client's
    /// own session.
    Foreground {
        pid: u32,
    },
    Hardware,
    SetCooling {
        channel: String,
        profile: wtm_policy::CoolingProfile,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Command {
    Kill,
    KillTree,
    Suspend,
    Resume,
    Priority(PriorityClass),
    Affinity(u64),
    EcoQos(EcoQos),
    /// Manual job-object limits, kept until `Unlimit` or process exit.
    Limit(Limits),
    Unlimit,
    /// Move the working set to the standby list (frees RAM; nothing is lost).
    TrimMemory,
    AffinityTier(wtm_core::cpu::CoreSelection),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Hello {
        version: u32,
        admin: bool,
    },
    Snapshot(Snapshot),
    Status(Status),
    Events(Vec<Event>),
    Details(ProcessDetails),
    Control(Vec<Outcome>),
    History {
        stats: Vec<AppStats>,
        findings: Vec<Finding>,
    },
    Series(Vec<MinuteRow>),
    Config {
        toml: String,
        path: String,
    },
    Telegram(wtm_policy::TelegramConfig),
    Recent(Vec<SystemPoint>),
    /// The request succeeded and has no payload.
    Done,
    /// The request succeeded; a line to show the user.
    Text(String),
    Error(String),
    Hardware(wtm_core::hardware::HardwareReport),
}

/// System-wide load at one sample, for history graphs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemPoint {
    pub cpu: f32,
    /// Share of physical memory in use, 0..=100.
    pub memory: f32,
    pub gpu: f32,
    /// Bytes per second.
    pub net_rx: f32,
    pub net_tx: f32,
    pub io_read: f32,
    pub io_write: f32,
    /// Load of each logical processor.
    pub cores: Vec<f32>,
}

impl SystemPoint {
    pub fn of(s: &wtm_core::SystemStats) -> Self {
        Self {
            cpu: s.cpu_percent,
            memory: s.memory_used() as f32 * 100.0 / s.memory_total.max(1) as f32,
            gpu: s.gpu_percent,
            net_rx: s.net_rx_rate as f32,
            net_tx: s.net_tx_rate as f32,
            io_read: s.io_read_rate as f32,
            io_write: s.io_write_rate as f32,
            cores: s.core_percent.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub target: ProcessRef,
    pub result: Result<(), String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub dry_run: bool,
    pub governor_enabled: bool,
    pub high_cpu_percent: f32,
    pub recover_cpu_percent: f32,
    pub rules: usize,
    pub config_path: String,
    pub managed: Vec<Managed>,
    /// Per-application budgets and their current usage.
    pub app_limits: Vec<wtm_policy::GroupUsage>,
    /// The foreground app protected from the governor, if any.
    pub foreground: Option<String>,
}

/// A process the service currently holds limits on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Managed {
    pub target: ProcessRef,
    pub sources: Vec<Source>,
    /// Human-readable effective state, e.g. "BelowNormal, eco, CPU ≤ 10%".
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: u64,
    pub unix_ms: u64,
    pub level: EventLevel,
    pub pid: Option<u32>,
    pub name: Option<String>,
    pub message: String,
    /// Worth a desktop notification (process terminated by a rule, governor
    /// started throttling a process).
    pub notify: bool,
}

fn invalid(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

pub fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let body = postcard::to_stdvec(msg).map_err(invalid)?;
    if body.len() > MAX_FRAME {
        return Err(invalid("frame too large"));
    }
    w.write_all(&(body.len() as u32).to_le_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

pub fn read_frame<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(invalid("frame too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    postcard::from_bytes(&body).map_err(invalid)
}

/// Blocking client for the service pipe.
#[derive(Debug)]
pub struct Client {
    pipe: File,
    admin: bool,
    verified: bool,
}

/// `ERROR_PIPE_BUSY`: all instances are momentarily in use.
const ERROR_PIPE_BUSY: i32 = 231;

impl Client {
    pub fn connect() -> io::Result<Self> {
        Self::connect_to(PIPE_NAME)
    }

    pub fn connect_to(name: &str) -> io::Result<Self> {
        Self::connect_checked(name, wtm_sys::services::service_pid(SERVICE_NAME))
    }

    /// Connects to `name`, requiring its server to be process `service`
    /// when the service is running.
    fn connect_checked(name: &str, service: Option<u32>) -> io::Result<Self> {
        let mut attempts = 0;
        let pipe = loop {
            match OpenOptions::new().read(true).write(true).open(name) {
                Ok(p) => break p,
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempts < 10 => {
                    attempts += 1;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e),
            }
        };
        let verified = verify_server(&pipe, service)?;
        let mut client = Self {
            pipe,
            admin: false,
            verified,
        };
        match client.call(&Request::Hello {
            version: PROTOCOL_VERSION,
        })? {
            Response::Hello { version, admin } if version == PROTOCOL_VERSION => {
                client.admin = admin;
                Ok(client)
            }
            Response::Hello { version, .. } => Err(invalid(format!(
                "service speaks protocol {version}, client {PROTOCOL_VERSION}"
            ))),
            other => Err(invalid(format!("unexpected handshake reply: {other:?}"))),
        }
    }

    /// Whether the service will accept `Control` requests from us.
    pub fn is_admin(&self) -> bool {
        self.admin
    }

    /// Whether the pipe server is the installed RustWTM service.
    pub fn is_verified(&self) -> bool {
        self.verified
    }

    pub fn call(&mut self, request: &Request) -> io::Result<Response> {
        write_frame(&mut self.pipe, request)?;
        read_frame(&mut self.pipe)
    }
}

/// Makes sure we talk to the installed service (PID `service`) and not to a
/// process that grabbed the pipe name. Without a running service (console
/// mode during development) the server cannot be verified: `Ok(false)`.
fn verify_server(pipe: &File, service: Option<u32>) -> io::Result<bool> {
    match service {
        Some(service) if wtm_sys::pipe::server_pid(pipe).ok() == Some(service) => Ok(true),
        Some(_) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the pipe is not served by the RustWTM service",
        )),
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn hello_keeps_its_wire_tag_across_protocol_versions() {
        let mut request = Vec::new();
        write_frame(
            &mut request,
            &Request::Hello {
                version: PROTOCOL_VERSION,
            },
        )
        .unwrap();
        assert_eq!(
            request[4], 0,
            "old services must understand the handshake before rejecting the version"
        );
        let mut response = Vec::new();
        write_frame(
            &mut response,
            &Response::Hello {
                version: 9,
                admin: false,
            },
        )
        .unwrap();
        assert!(matches!(
            read_frame::<Response>(&mut Cursor::new(response)).unwrap(),
            Response::Hello { version: 9, .. }
        ));
    }

    #[test]
    fn frames_round_trip() {
        let target = ProcessRef {
            pid: 42,
            create_time: 7,
            name: "a.exe".into(),
        };
        let requests = [
            Request::Hello { version: 1 },
            Request::Events { after: 9 },
            Request::Control {
                target: target.clone(),
                command: Command::Limit(Limits {
                    cpu_percent: Some(12.5),
                    memory_bytes: None,
                }),
            },
            Request::Control {
                target: target.clone(),
                command: Command::Priority(PriorityClass::Idle),
            },
            Request::Control {
                target,
                command: Command::AffinityTier(wtm_core::cpu::CoreSelection::LowPower),
            },
        ];
        let mut buf = Vec::new();
        for r in &requests {
            write_frame(&mut buf, r).unwrap();
        }
        let mut cursor = Cursor::new(buf);
        for r in &requests {
            assert_eq!(&read_frame::<Request>(&mut cursor).unwrap(), r);
        }
    }

    /// The wire format has no field names, so every field must always be
    /// written; optional budgets of app limits once broke Status replies.
    #[test]
    fn status_with_partial_app_limits_round_trips() {
        let limit = wtm_policy::AppLimit {
            name: "zen.exe".into(),
            cpu_cores: None,
            memory_gb: Some(8.0),
            scope: wtm_policy::LimitScope::Each,
        };
        let status = Status {
            app_limits: vec![wtm_policy::GroupUsage {
                group: wtm_policy::GroupId {
                    app: "zen.exe".into(),
                    scope: wtm_policy::LimitScope::Each,
                    root: Some((1, 2)),
                },
                limit,
                processes: 1,
                members: vec![(1, 2)],
                outside: 0,
                cpu_percent: 1.0,
                memory_bytes: 3,
            }],
            ..Default::default()
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &Response::Status(status.clone())).unwrap();
        match read_frame::<Response>(&mut Cursor::new(buf)).unwrap() {
            Response::Status(s) => assert_eq!(s, status),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_oversized_frames() {
        let mut buf = (u32::MAX).to_le_bytes().to_vec();
        buf.extend([0; 8]);
        assert!(read_frame::<Request>(&mut Cursor::new(buf)).is_err());

        let huge = Request::AppSeries {
            app: "x".repeat(MAX_FRAME + 1),
            hours: 1,
        };
        let e = write_frame(&mut io::sink(), &huge).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_truncated_and_garbled_frames() {
        // Length promises more bytes than follow.
        let mut buf = 8u32.to_le_bytes().to_vec();
        buf.extend([1, 2]);
        let e = read_frame::<Request>(&mut Cursor::new(buf)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);

        // A body that is not a valid `Request`.
        let mut buf = 1u32.to_le_bytes().to_vec();
        buf.push(0xFF);
        let e = read_frame::<Request>(&mut Cursor::new(buf)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    use std::sync::atomic::{AtomicU32, Ordering};
    use std::thread::JoinHandle;
    use wtm_sys::pipe::PipeListener;

    /// A fresh pipe name per test so tests (and a real service) never collide.
    fn pipe_name() -> String {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        format!(
            r"\\.\pipe\wtm-ipc-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn listen(name: &str) -> PipeListener {
        PipeListener::bind(
            name,
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)(A;;GA;;;OW)",
        )
        .unwrap()
    }

    /// Serves one client: answers the handshake with `hello`, then every
    /// further request with `reply`, until the client hangs up.
    fn serve(name: &str, hello: Response, reply: Response) -> JoinHandle<Vec<Request>> {
        let mut listener = listen(name);
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            let mut conn = listener.accept().unwrap();
            let mut answer = hello;
            // Ends when the client hangs up.
            while let Ok(request) = read_frame::<Request>(&mut conn) {
                seen.push(request);
                let _ = write_frame(&mut conn, &answer);
                answer = reply.clone();
            }
            seen
        })
    }

    #[test]
    fn handshake_then_calls() {
        let name = pipe_name();
        let server = serve(
            &name,
            Response::Hello {
                version: PROTOCOL_VERSION,
                admin: true,
            },
            Response::Status(Status {
                rules: 3,
                ..Status::default()
            }),
        );
        // Pretend the test process is the service: it serves the pipe.
        let mut client = Client::connect_checked(&name, Some(std::process::id())).unwrap();
        assert!(client.is_admin());
        assert!(client.is_verified());
        let reply = client.call(&Request::Status).unwrap();
        assert!(
            matches!(reply, Response::Status(Status { rules: 3, .. })),
            "{reply:?}"
        );
        drop(client);
        let seen = server.join().unwrap();
        assert_eq!(
            seen,
            [
                Request::Hello {
                    version: PROTOCOL_VERSION
                },
                Request::Status
            ]
        );
    }

    #[test]
    fn handshake_rejects_wrong_version_and_replies() {
        let name = pipe_name();
        let _server = serve(
            &name,
            Response::Hello {
                version: PROTOCOL_VERSION + 1,
                admin: false,
            },
            Response::Error(String::new()),
        );
        let e = Client::connect_checked(&name, None).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert!(e.to_string().contains("protocol"), "{e}");

        let name = pipe_name();
        let _server = serve(
            &name,
            Response::Error("go away".into()),
            Response::Error(String::new()),
        );
        let e = Client::connect_checked(&name, None).unwrap_err();
        assert!(e.to_string().contains("unexpected handshake reply"), "{e}");
    }

    #[test]
    fn connect_fails_without_server() {
        let e = Client::connect_to(&pipe_name()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn connect_gives_up_while_the_pipe_stays_busy() {
        let name = pipe_name();
        // The only instance is taken by a client that is never accepted.
        let _listener = listen(&name);
        let _holder = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .unwrap();
        let e = Client::connect_checked(&name, None).unwrap_err();
        assert_eq!(e.raw_os_error(), Some(ERROR_PIPE_BUSY));
    }

    #[test]
    fn verifies_the_pipe_server() {
        let name = pipe_name();
        let _listener = listen(&name);
        let pipe = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .unwrap();
        // The test process itself serves the pipe.
        assert!(verify_server(&pipe, Some(std::process::id())).unwrap());
        let e = verify_server(&pipe, Some(std::process::id().wrapping_add(4))).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        assert!(!verify_server(&pipe, None).unwrap());
    }

    #[test]
    fn events_compare_and_clone() {
        let e = Event {
            id: 1,
            unix_ms: 2,
            level: EventLevel::Warn,
            pid: Some(3),
            name: None,
            message: "m".into(),
            notify: true,
        };
        assert_eq!(e.clone(), e);
        assert!(format!("{e:?}").contains("Warn"));
    }
}

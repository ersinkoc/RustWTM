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

pub const PROTOCOL_VERSION: u32 = 3;

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
    Error(String),
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
        // Make sure we talk to the installed service and not to a process
        // that grabbed the pipe name. Without a running service (console
        // mode during development) the server cannot be verified.
        let verified = match wtm_sys::services::service_pid(SERVICE_NAME) {
            Some(service) if wtm_sys::pipe::server_pid(&pipe).ok() == Some(service) => true,
            Some(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "the pipe is not served by the RustWTM service",
                ));
            }
            None => false,
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

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
                target,
                command: Command::Priority(PriorityClass::Idle),
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

    #[test]
    fn rejects_oversized_frames() {
        let mut buf = (u32::MAX).to_le_bytes().to_vec();
        buf.extend([0; 8]);
        assert!(read_frame::<Request>(&mut Cursor::new(buf)).is_err());
    }
}

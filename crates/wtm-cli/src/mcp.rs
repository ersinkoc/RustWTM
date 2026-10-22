//! `wtm mcp`: a Model Context Protocol server on standard input and output,
//! so AI assistants (Claude Desktop, Claude Code, ...) can ask what this PC
//! is doing. Nothing listens on the network: the assistant starts this
//! program and talks to it over a pipe. Read-only unless started with
//! `--allow-control`, which adds tools that end, suspend or deprioritize
//! processes with the rights of whoever started it.

use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wtm_core::control::{self, EcoQos, PriorityClass, ProcessRef};
use wtm_core::{ProcessInfo, Sampler, Snapshot, format_bytes};

/// Protocol revisions this server speaks, newest first.
const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Rates compare two samples; one older than this would average CPU, disk
/// and network over the whole pause between two tool calls.
const FRESH: Duration = Duration::from_secs(2);
/// The window measured when the last sample is too old.
const WINDOW: Duration = Duration::from_millis(500);

pub struct Server {
    sampler: Sampler,
    /// When the sampler last sampled.
    sampled: Option<Instant>,
    /// Ask the RustWTM service for its latest sample first: it also measures
    /// network traffic per process.
    use_service: bool,
    allow_control: bool,
}

impl Server {
    pub fn new(allow_control: bool) -> Self {
        Self {
            sampler: Sampler::new(),
            sampled: None,
            use_service: true,
            allow_control,
        }
    }

    /// A sample with current rates: unless the previous sample is recent,
    /// a fresh one is taken first to compare against.
    fn snapshot(&mut self) -> Result<Snapshot, String> {
        if self.use_service
            && let Some(snap) = service_snapshot()
        {
            return Ok(snap);
        }
        self.local_snapshot()
    }

    /// A snapshot taken here, never the service's.
    fn local_snapshot(&mut self) -> Result<Snapshot, String> {
        if self.sampled.is_none_or(|at| at.elapsed() > FRESH) {
            self.sampler.sample().map_err(|e| e.to_string())?;
            std::thread::sleep(WINDOW);
        }
        let snap = self.sampler.sample().map_err(|e| e.to_string())?;
        self.sampled = Some(Instant::now());
        Ok(snap)
    }

    /// Answers one JSON-RPC message; `None` for notifications.
    pub fn handle(&mut self, message: &Value) -> Option<Value> {
        if !message.is_object() {
            return Some(error_reply(
                -32600,
                "invalid request: expected one JSON-RPC object (batches are not supported)",
            ));
        }
        // A message without a method is a client's answer (or junk): never
        // answered, as JSON-RPC requires.
        let method = message.get("method")?;
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let id = id?;
        let result = match method.as_str() {
            _ if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") => Err((
                -32600,
                "invalid request: `jsonrpc` must be \"2.0\"".to_owned(),
            )),
            None => Err((
                -32600,
                "invalid request: `method` must be a string".to_owned(),
            )),
            Some("initialize") => Ok(self.initialize(&params)),
            Some("ping") => Ok(json!({})),
            Some("tools/list") => Ok(json!({ "tools": self.tools() })),
            Some("tools/call") => self.call(&params),
            Some(method) => Err((-32601, format!("method not found: {method}"))),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": message },
            }),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let asked = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(PROTOCOLS[0]);
        let version = PROTOCOLS
            .iter()
            .find(|v| **v == asked)
            .copied()
            .unwrap_or(PROTOCOLS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "rustwtm", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "RustWTM sees this Windows PC: processes, CPU, memory, disks, \
                network, hung programs and why a program will not end. Start with \
                system_overview.",
        })
    }

    fn tools(&self) -> Vec<Value> {
        let pid = json!({ "type": "object", "properties": { "pid": { "type": "integer" } }, "required": ["pid"] });
        // Control tools also take the creation time list_processes reports,
        // so a PID reused meanwhile by another program is refused.
        let identity = |extra: Value| {
            let mut properties = json!({
                "pid": { "type": "integer" },
                "create_time": { "type": "string", "description": "The create_time from list_processes, as text (it is too large for a JSON number to keep exactly); refuses a PID that now belongs to another process" },
            });
            if let (Some(p), Some(e)) = (properties.as_object_mut(), extra.as_object()) {
                p.extend(e.clone());
            }
            properties
        };
        let target = |extra: Value, required: &[&str]| json!({ "type": "object", "properties": identity(extra), "required": required });
        let mut tools = vec![
            tool(
                "system_overview",
                "CPU, memory and disk load, the busiest programs and programs that are not responding.",
                json!({ "type": "object", "properties": {} }),
            ),
            tool(
                "list_processes",
                "Processes sorted by cpu, memory, disk or network, optionally filtered by name. Each line ends with the process's create_time, which control tools accept to make sure they act on the same process.",
                json!({
                    "type": "object",
                    "properties": {
                        "sort": { "type": "string", "enum": ["cpu", "memory", "disk", "network"] },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
                        "name": { "type": "string", "description": "Part of the image name" },
                    },
                }),
            ),
            tool(
                "process_details",
                "Path, command line, user, priority and efficiency mode of one process.",
                pid.clone(),
            ),
            tool(
                "why_not_responding",
                "Whether a program's window hangs, and what its blocked threads wait for (wait chains, deadlocks).",
                pid.clone(),
            ),
            tool(
                "why_wont_it_end",
                "Why a process cannot be ended: protection, drivers that withhold the right, or other programs holding it open.",
                pid.clone(),
            ),
            tool(
                "network_connections",
                "TCP and UDP endpoints, optionally of one process.",
                json!({ "type": "object", "properties": { "pid": { "type": "integer" } } }),
            ),
            tool(
                "drives",
                "Every drive with its free space.",
                json!({ "type": "object", "properties": {} }),
            ),
            tool(
                "recent_events",
                "What RustWTM's service did recently (limits, alerts, blocks).",
                json!({ "type": "object", "properties": { "limit": { "type": "integer", "minimum": 1, "maximum": 200 } } }),
            ),
        ];
        if self.allow_control {
            tools.extend([
                tool("end_process", "Ends a process (unsaved data in it is lost).", target(json!({}), &["pid", "create_time"])),
                tool("suspend_process", "Freezes every thread of a process.", target(json!({}), &["pid", "create_time"])),
                tool("resume_process", "Resumes a suspended process.", target(json!({}), &["pid", "create_time"])),
                tool("set_priority", "Changes a process's priority class.", target(
                    json!({ "priority": { "type": "string", "enum": ["idle", "below_normal", "normal", "above_normal", "high"] } }),
                    &["pid", "create_time", "priority"],
                )),
                tool("efficiency_mode", "Turns efficiency mode (EcoQoS) on or off for a process.", target(
                    json!({ "on": { "type": "boolean" } }),
                    &["pid", "create_time", "on"],
                )),
            ]);
        }
        tools
    }

    /// Runs a tool; an unknown one is a JSON-RPC error, as MCP has it.
    fn call(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let known = CONTROL_TOOLS.contains(&name)
            || self
                .tools()
                .iter()
                .any(|t| t["name"].as_str() == Some(name));
        if !known {
            return Err((-32602, format!("unknown tool: {name}")));
        }
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        let control_tool = CONTROL_TOOLS.contains(&name);
        let result = if control_tool && !self.allow_control {
            Err("control tools are off; start `wtm mcp --allow-control` to enable them".into())
        } else {
            self.run_tool(name, &args)
        };
        Ok(match result {
            Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
            Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
        })
    }

    fn target(&mut self, args: &Value) -> Result<ProcessRef, String> {
        let pid = args
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|p| u32::try_from(p).ok())
            .ok_or("`pid` is required")?;
        let find = |snap: Snapshot| {
            snap.processes
                .iter()
                .find(|p| p.pid == pid)
                .map(ProcessRef::from)
        };
        // The service publishes every few seconds; a process started since
        // is only in a snapshot taken now.
        let target = match find(self.snapshot()?) {
            Some(t) => Some(t),
            None if self.use_service => find(self.local_snapshot()?),
            None => None,
        }
        .ok_or_else(|| format!("no process with PID {pid}"))?;
        check_identity(&target, args.get("create_time"))?;
        Ok(target)
    }

    /// [`Self::target`] for a tool that changes the process: its
    /// `create_time` is required, so a process that took the PID since the
    /// listing is never the one changed.
    fn control_target(&mut self, args: &Value) -> Result<ProcessRef, String> {
        if args.get("create_time").is_none_or(Value::is_null) {
            return Err(
                "`create_time` (from list_processes) is required, so that a process that took \
                 the PID since is never changed"
                    .into(),
            );
        }
        self.target(args)
    }

    fn run_tool(&mut self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "system_overview" => self.overview(),
            "list_processes" => {
                let sort = args.get("sort").and_then(Value::as_str).unwrap_or("cpu");
                let limit = args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(20)
                    .clamp(1, 200);
                let filter = args
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_lowercase();
                let snap = self.snapshot()?;
                let mut procs: Vec<&ProcessInfo> = snap
                    .processes
                    .iter()
                    .filter(|p| p.pid != 0 && p.name.to_lowercase().contains(&filter))
                    .collect();
                let key = |p: &ProcessInfo| match sort {
                    "memory" => p.private_working_set as f64,
                    "disk" => p.io_rate(),
                    "network" => p.net_rate(),
                    _ => f64::from(p.cpu_percent),
                };
                procs.sort_by(|a, b| key(b).total_cmp(&key(a)));
                let mut lines: Vec<String> = procs
                    .into_iter()
                    .take(limit as usize)
                    .map(|p| process_line(p, snap.net_traced))
                    .collect();
                if !snap.net_traced {
                    lines.push(NET_UNTRACED.into());
                }
                Ok(lines.join("\n"))
            }
            "process_details" => {
                let t = self.target(args)?;
                let d = control::inspect(&t).map_err(|e| e.to_string())?;
                Ok(format!(
                    "{} (PID {})\npath: {}\ncommand line: {}\nuser: {}\npriority: {}\nefficiency mode: {}",
                    t.name,
                    t.pid,
                    d.path.unwrap_or_default(),
                    d.command_line.unwrap_or_default(),
                    d.user.unwrap_or_default(),
                    d.priority.map(|p| format!("{p:?}")).unwrap_or_default(),
                    d.eco_qos.map(|e| format!("{e:?}")).unwrap_or_default(),
                ))
            }
            "why_not_responding" => {
                let t = self.target(args)?;
                let hung = wtm_core::hung_windows()
                    .into_iter()
                    .find(|w| w.pid == t.pid);
                let report = wtm_core::wait_chains(t.pid, Duration::from_secs(15))
                    .map_err(|e| e.to_string())?;
                let mut out = match hung {
                    Some(w) => format!("Window \"{}\" is not responding.\n", w.title),
                    None => "No window of it is marked as not responding.\n".to_owned(),
                };
                if report.deadlocked() {
                    out.push_str(
                        "DEADLOCK: threads wait for each other and will never continue.\n",
                    );
                }
                out.push_str(&format!(
                    "{} threads, {} blocked, {} with a wait chain.\n",
                    report.threads,
                    report.analyzed,
                    report.chains.len()
                ));
                if report.skipped > 0 {
                    out.push_str(&format!(
                        "Analysis incomplete: {} blocked threads were not examined (other \
                         analyses are running, or it took too long); ask again shortly.\n",
                        report.skipped
                    ));
                }
                for c in report.chains.iter().take(10) {
                    let links: Vec<String> = c.links.iter().map(link_text).collect();
                    out.push_str(&links.join(" -> "));
                    out.push('\n');
                }
                Ok(out)
            }
            "why_wont_it_end" => {
                let t = self.target(args)?;
                let r = wtm_core::why_alive(t.pid, t.create_time).map_err(|e| e.to_string())?;
                Ok(serde_json::to_string_pretty(&r).unwrap_or_default())
            }
            "network_connections" => {
                let pid = match args.get("pid") {
                    None | Some(Value::Null) => None,
                    Some(pid) => Some(
                        pid.as_u64()
                            .and_then(|p| u32::try_from(p).ok())
                            .ok_or("`pid` must be a process ID")?,
                    ),
                };
                let snap = self.snapshot()?;
                let names: std::collections::HashMap<u32, &str> = snap
                    .processes
                    .iter()
                    .map(|p| (p.pid, p.name.as_str()))
                    .collect();
                let conns = wtm_core::connections().map_err(|e| e.to_string())?;
                Ok(conns
                    .iter()
                    .filter(|c| pid.is_none_or(|p| c.pid == p))
                    .take(300)
                    .map(|c| {
                        format!(
                            "{:?} {} -> {} {:?} {} ({})",
                            c.protocol,
                            c.local,
                            c.remote
                                .map(|r| r.to_string())
                                .unwrap_or_else(|| "*".into()),
                            c.state,
                            names.get(&c.pid).copied().unwrap_or("?"),
                            c.pid
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            "drives" => Ok(wtm_core::volumes()
                .iter()
                .map(|v| {
                    format!(
                        "{} {} ({}): {} free of {} ({:.0}% used)",
                        v.root,
                        v.label,
                        v.file_system,
                        format_bytes(v.free as f64),
                        format_bytes(v.total as f64),
                        v.used_percent()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")),
            "recent_events" => {
                let limit = args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(30)
                    .clamp(1, 200);
                let mut client = wtm_ipc::Client::connect()
                    .map_err(|e| format!("the RustWTM service is not reachable: {e}"))?;
                // Their text reaches the assistant: only the real service's.
                if !client.is_verified() {
                    return Err("the RustWTM pipe is not served by the installed service".into());
                }
                match client.call(&wtm_ipc::Request::Events { after: 0 }) {
                    Ok(wtm_ipc::Response::Events(events)) => Ok(events
                        .iter()
                        .rev()
                        .take(limit as usize)
                        .map(|e| format!("{:?}: {}", e.level, e.message))
                        .collect::<Vec<_>>()
                        .join("\n")),
                    Ok(other) => Err(format!("unexpected answer: {other:?}")),
                    Err(e) => Err(e.to_string()),
                }
            }
            "end_process" => {
                let t = self.control_target(args)?;
                control::kill(&t).map_err(|e| e.to_string())?;
                Ok(format!("{} (PID {}) ended", t.name, t.pid))
            }
            "suspend_process" => {
                let t = self.control_target(args)?;
                control::suspend(&t).map_err(|e| e.to_string())?;
                Ok(format!("{} (PID {}) suspended", t.name, t.pid))
            }
            "resume_process" => {
                let t = self.control_target(args)?;
                control::resume(&t).map_err(|e| e.to_string())?;
                Ok(format!("{} (PID {}) resumed", t.name, t.pid))
            }
            "set_priority" => {
                let t = self.control_target(args)?;
                let priority =
                    match args.get("priority").and_then(Value::as_str) {
                        Some("idle") => PriorityClass::Idle,
                        Some("below_normal") => PriorityClass::BelowNormal,
                        Some("normal") => PriorityClass::Normal,
                        Some("above_normal") => PriorityClass::AboveNormal,
                        Some("high") => PriorityClass::High,
                        _ => return Err(
                            "`priority` must be idle, below_normal, normal, above_normal or high"
                                .into(),
                        ),
                    };
                control::set_priority(&t, priority).map_err(|e| e.to_string())?;
                Ok(format!(
                    "{} (PID {}) now runs at {priority:?}",
                    t.name, t.pid
                ))
            }
            "efficiency_mode" => {
                let t = self.control_target(args)?;
                let on = args
                    .get("on")
                    .and_then(Value::as_bool)
                    .ok_or("`on` is required")?;
                let mode = if on {
                    EcoQos::On
                } else {
                    EcoQos::SystemManaged
                };
                control::set_eco_qos(&t, mode).map_err(|e| e.to_string())?;
                Ok(format!(
                    "efficiency mode {} for {} (PID {})",
                    if on { "on" } else { "off" },
                    t.name,
                    t.pid
                ))
            }
            _ => Err(format!("unknown tool: {name}")),
        }
    }

    fn overview(&mut self) -> Result<String, String> {
        let snap = self.snapshot()?;
        let s = &snap.system;
        let mut out = format!(
            "CPU {:.0}% of {} logical processors\nmemory {} of {} in use\n",
            s.cpu_percent,
            s.logical_cpus,
            format_bytes(s.memory_used() as f64),
            format_bytes(s.memory_total as f64)
        );
        for d in &s.disks {
            out.push_str(&format!(
                "disk {}: {:.0}% busy, read {}/s, write {}/s\n",
                d.name,
                d.active_percent,
                format_bytes(d.read_rate),
                format_bytes(d.write_rate)
            ));
        }
        let mut by_cpu: Vec<&ProcessInfo> = snap.processes.iter().filter(|p| p.pid != 0).collect();
        by_cpu.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        out.push_str("busiest by CPU:\n");
        for p in by_cpu.iter().take(5) {
            out.push_str(&format!("  {}\n", process_line(p, snap.net_traced)));
        }
        let mut by_mem = by_cpu.clone();
        by_mem.sort_by_key(|p| std::cmp::Reverse(p.private_working_set));
        out.push_str("largest by memory:\n");
        for p in by_mem.iter().take(5) {
            out.push_str(&format!("  {}\n", process_line(p, snap.net_traced)));
        }
        let hung = wtm_core::hung_windows();
        let mut seen = HashSet::new();
        for w in hung.iter().filter(|w| seen.insert(w.pid)) {
            let name = snap
                .processes
                .iter()
                .find(|p| p.pid == w.pid)
                .map_or("?", |p| p.name.as_str());
            out.push_str(&format!(
                "NOT RESPONDING: {name} (PID {}) \"{}\"\n",
                w.pid, w.title
            ));
        }
        Ok(out)
    }
}

/// Tools that change a process; offered with `--allow-control` only.
const CONTROL_TOOLS: [&str; 5] = [
    "end_process",
    "suspend_process",
    "resume_process",
    "set_priority",
    "efficiency_mode",
];

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": schema })
}

/// The installed service's latest sample, while it runs. Only a verified
/// service is trusted: a program squatting on the pipe could otherwise
/// misname processes for the control tools.
fn service_snapshot() -> Option<Snapshot> {
    let mut client = wtm_ipc::Client::connect().ok()?;
    if !client.is_verified() {
        return None;
    }
    match client.call(&wtm_ipc::Request::Snapshot).ok()? {
        wtm_ipc::Response::Snapshot(s) if s.interval.is_some() => Some(s),
        _ => None,
    }
}

/// Said once under a listing whose network rates were not measured.
const NET_UNTRACED: &str =
    "(network per process not measured: that needs the RustWTM service running)";

/// One process; the network rate only when it was measured (`net`), since
/// otherwise it would read as an idle 0 B/s.
fn process_line(p: &ProcessInfo, net: bool) -> String {
    let network = if net {
        format!(", network {}/s", format_bytes(p.net_rate()))
    } else {
        String::new()
    };
    let suspended = if p.suspended { ", SUSPENDED" } else { "" };
    format!(
        "{} (PID {}): CPU {:.1}%, memory {}, disk {}/s{network}{suspended}, create_time {}",
        p.name,
        p.pid,
        p.cpu_percent,
        format_bytes(p.private_working_set as f64),
        format_bytes(p.io_rate()),
        p.create_time
    )
}

/// Refuses `target` when the caller named a different instance of its PID
/// (`create_time` from an earlier list_processes).
///
/// Creation times count 100 ns since 1601, past what a JSON number keeps
/// exactly in JavaScript clients, which round them. Text compares exactly;
/// a number matches at the precision it can have, a window of about 2 µs
/// in which the same PID is not reused.
fn check_identity(target: &ProcessRef, create_time: Option<&Value>) -> Result<(), String> {
    let same = match create_time {
        None | Some(Value::Null) => return Ok(()),
        Some(Value::String(text)) => match text.trim().parse::<i64>() {
            Ok(t) => t == target.create_time,
            Err(_) => return Err("`create_time` must be the number from list_processes".into()),
        },
        Some(v) => match (v.as_i64(), v.as_f64()) {
            (Some(t), _) if t == target.create_time => true,
            (_, Some(t)) => t == target.create_time as f64,
            _ => return Err("`create_time` must be the number from list_processes".into()),
        },
    };
    if same {
        Ok(())
    } else {
        Err(format!(
            "PID {} now belongs to another process ({}); list the processes again",
            target.pid, target.name
        ))
    }
}

fn link_text(link: &wtm_core::Link) -> String {
    match link {
        wtm_core::Link::Thread { pid, tid, status } => {
            format!("thread {tid} (PID {pid}, {status:?})")
        }
        wtm_core::Link::Object { kind, name, .. } if name.is_empty() => format!("{kind:?}"),
        wtm_core::Link::Object { kind, name, .. } => format!("{kind:?} \"{name}\""),
    }
}

/// Serves one client until its input ends.
pub fn run(
    input: impl BufRead,
    mut output: impl Write,
    allow_control: bool,
) -> std::io::Result<()> {
    let mut server = Server::new(allow_control);
    // Raw lines: one that is not UTF-8 is a parse error, not the end.
    for line in input.split(b'\n') {
        let line = line?;
        if line.trim_ascii().is_empty() {
            continue;
        }
        let reply = match serde_json::from_slice::<Value>(&line) {
            Ok(message) => server.handle(&message),
            Err(e) => Some(error_reply(-32700, &format!("parse error: {e}"))),
        };
        if let Some(reply) = reply {
            writeln!(output, "{reply}")?;
            output.flush()?;
        }
    }
    Ok(())
}

/// An error for a message whose id could not be read.
fn error_reply(code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_lines_and_non_objects_are_answered_and_the_server_goes_on() {
        let ping = json!({"jsonrpc":"2.0","id":1,"method":"ping"});
        let mut input = b"\xff\xfe not utf-8\n[1, 2]\n42\n".to_vec();
        input.extend(format!("{ping}\r\n").into_bytes());
        let mut out = Vec::new();
        run(&input[..], &mut out, false).unwrap();
        let replies: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(replies.len(), 4, "{replies:#?}");
        assert_eq!(replies[0]["error"]["code"], -32700);
        assert_eq!(replies[1]["error"]["code"], -32600);
        assert_eq!(replies[2]["error"]["code"], -32600);
        assert_eq!(replies[3]["id"], 1, "still serving after bad input");
        assert!(replies[3]["result"].is_object());
    }

    #[test]
    fn connections_refuse_a_pid_that_is_not_one() {
        for pid in [json!("4"), json!(-1), json!(1u64 << 33)] {
            let call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"network_connections","arguments":{"pid":pid}}});
            let replies = talk(&[call], false);
            assert_eq!(replies[0]["result"]["isError"], true, "{pid}");
            let text = replies[0]["result"]["content"][0]["text"].as_str().unwrap();
            assert!(text.contains("`pid` must be"), "{text}");
        }
    }

    fn talk(lines: &[Value], allow_control: bool) -> Vec<Value> {
        let input: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let mut out = Vec::new();
        run(input.as_bytes(), &mut out, allow_control).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn initializes_lists_tools_and_answers_calls() {
        let replies = talk(
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
                json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_processes","arguments":{"sort":"memory","limit":3}}}),
                json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"process_details","arguments":{"pid":std::process::id()}}}),
                json!({"jsonrpc":"2.0","id":5,"method":"nope"}),
                json!({"jsonrpc":"2.0","id":6,"method":"ping"}),
            ],
            false,
        );
        // The notification gets no answer.
        assert_eq!(replies.len(), 6, "{replies:#?}");
        assert_eq!(replies[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(replies[0]["result"]["serverInfo"]["name"], "rustwtm");
        let names: Vec<&str> = replies[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"system_overview") && names.contains(&"why_wont_it_end"));
        assert!(!names.contains(&"end_process"), "read-only by default");
        let listing = replies[2]["result"]["content"][0]["text"].as_str().unwrap();
        // Three processes, then (without the service) the note that network
        // was not measured.
        let untraced = usize::from(listing.ends_with(NET_UNTRACED));
        assert_eq!(listing.lines().count(), 3 + untraced, "{listing}");
        let details = replies[3]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            details.contains(&format!("PID {}", std::process::id())),
            "{details}"
        );
        assert_eq!(replies[4]["error"]["code"], -32601);
        assert_eq!(replies[5]["result"], json!({}));
    }

    #[test]
    fn control_tools_need_the_flag() {
        let call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"end_process","arguments":{"pid":1}}});
        let replies = talk(std::slice::from_ref(&call), false);
        assert_eq!(replies[0]["result"]["isError"], true);
        assert!(
            replies[0]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("--allow-control")
        );
        let list = json!({"jsonrpc":"2.0","id":2,"method":"tools/list"});
        let replies = talk(&[list], true);
        assert!(
            replies[0]["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["name"] == "end_process")
        );
        // A missing process is an error result, not a crash.
        let missing = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"suspend_process","arguments":{"pid":4294967280u64}}});
        let replies = talk(&[missing], true);
        assert_eq!(replies[0]["result"]["isError"], true);
    }

    #[test]
    fn only_requests_are_answered_and_malformed_ones_are_named() {
        let replies = talk(
            &[
                // A client's answer, or a message without a method: silence.
                json!({"jsonrpc":"2.0","id":1,"result":{}}),
                json!({"jsonrpc":"1.0","id":2,"method":"ping"}),
                json!({"jsonrpc":"2.0","id":3,"method":5}),
                json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"no_such_tool"}}),
                json!({"jsonrpc":"2.0","id":5,"method":"ping"}),
            ],
            false,
        );
        let codes: Vec<(i64, Value)> = replies
            .iter()
            .map(|r| (r["id"].as_i64().unwrap(), r["error"]["code"].clone()))
            .collect();
        assert_eq!(
            codes,
            [
                (2, json!(-32600)),
                (3, json!(-32600)),
                (4, json!(-32602)),
                (5, Value::Null)
            ]
        );
    }

    #[test]
    fn control_tools_check_the_process_identity() {
        let t = ProcessRef {
            pid: 7,
            create_time: 1234,
            name: "app.exe".into(),
        };
        assert!(check_identity(&t, None).is_ok());
        assert!(check_identity(&t, Some(&Value::Null)).is_ok());
        assert!(check_identity(&t, Some(&json!(1234))).is_ok());
        let reused = check_identity(&t, Some(&json!(99))).unwrap_err();
        assert!(reused.contains("another process"), "{reused}");
        assert!(check_identity(&t, Some(&json!("x"))).is_err());
        assert!(check_identity(&t, Some(&json!(true))).is_err());
        assert!(check_identity(&t, Some(&json!("1234"))).is_ok());
        assert!(check_identity(&t, Some(&json!("99"))).is_err());

        // A real creation time, as a JavaScript client rounds it.
        let real = ProcessRef {
            create_time: 134_051_234_567_890_123,
            ..t.clone()
        };
        let rounded: f64 = serde_json::from_str("134051234567890123").unwrap();
        assert_ne!(rounded as i64, real.create_time, "the number was rounded");
        assert!(check_identity(&real, Some(&json!(rounded))).is_ok());
        assert!(check_identity(&real, Some(&json!("134051234567890123"))).is_ok());
        assert!(check_identity(&real, Some(&json!("134051234567890124"))).is_err());
        assert!(check_identity(&real, Some(&json!(rounded + 100_000.0))).is_err());

        // Through the server: a stale creation time is refused before acting.
        let me = std::process::id();
        let call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"suspend_process","arguments":{"pid":me,"create_time":1}}});
        let replies = talk(&[call], true);
        assert_eq!(replies[0]["result"]["isError"], true);
        let text = replies[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("another process"), "{text}");
        let list = json!({"jsonrpc":"2.0","id":2,"method":"tools/list"});
        let replies = talk(&[list], true);
        let tools = replies[0]["result"]["tools"].as_array().unwrap();
        let end = tools.iter().find(|t| t["name"] == "end_process").unwrap();
        assert!(end["inputSchema"]["properties"]["create_time"].is_object());
        let priority = tools.iter().find(|t| t["name"] == "set_priority").unwrap();
        assert_eq!(
            priority["inputSchema"]["required"],
            json!(["pid", "create_time", "priority"])
        );
        // Without its start time a PID may name a later process: refused.
        let call = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"suspend_process","arguments":{"pid":me}}});
        let replies = talk(&[call], true);
        assert_eq!(replies[0]["result"]["isError"], true);
        let text = replies[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("`create_time`"), "{text}");
        assert!(priority["inputSchema"]["properties"]["priority"].is_object());
    }

    #[test]
    fn lines_show_network_only_when_measured() {
        let snap = Sampler::new().sample().unwrap();
        let me = snap
            .processes
            .iter()
            .find(|p| p.pid == std::process::id())
            .unwrap();
        let line = process_line(me, false);
        assert!(!line.contains("network"), "{line}");
        assert!(
            line.ends_with(&format!("create_time {}", me.create_time)),
            "{line}"
        );
        assert!(process_line(me, true).contains("network 0 B/s"));
    }

    #[test]
    fn a_stale_sample_is_refreshed_before_measuring() {
        let mut server = Server::new(false);
        server.use_service = false;
        let first = server.snapshot().unwrap();
        assert!(
            first.interval.is_some_and(|d| d >= WINDOW),
            "rates need a window"
        );
        // Right after: the previous sample is recent enough to compare with.
        let started = Instant::now();
        let second = server.snapshot().unwrap();
        assert!(started.elapsed() < WINDOW);
        assert!(second.interval.is_some_and(|d| d < FRESH));
        // Long after: measured afresh, not averaged over the pause.
        server.sampled = Some(Instant::now() - FRESH * 5);
        let third = server.snapshot().unwrap();
        assert!(third.interval.is_some_and(|d| d < FRESH));
    }

    #[test]
    fn bad_json_is_a_parse_error() {
        let mut out = Vec::new();
        run("{not json\n".as_bytes(), &mut out, false).unwrap();
        let reply: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(reply["error"]["code"], -32700);
    }
}

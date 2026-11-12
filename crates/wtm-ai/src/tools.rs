//! What RustWTM can tell about this PC, as named tools with JSON
//! arguments and plain-text results. The MCP server (`wtm mcp`) offers
//! them to AI assistants; the built-in assistant gathers its facts with
//! the same code. Read-only unless control is allowed, which adds tools
//! that end, suspend or deprioritize processes.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};
use wtm_core::control::{self, EcoQos, PriorityClass, ProcessRef};
use wtm_core::{ProcessInfo, Sampler, Snapshot, format_bytes, format_duration};

/// Rates compare two samples; one older than this would average CPU, disk
/// and network over the whole pause between two tool calls.
pub const FRESH: Duration = Duration::from_secs(2);
/// The window measured when the last sample is too old.
pub const WINDOW: Duration = Duration::from_millis(500);

/// Tools that change a process; offered with control allowed only.
pub const CONTROL_TOOLS: [&str; 5] = [
    "end_process",
    "suspend_process",
    "resume_process",
    "set_priority",
    "efficiency_mode",
];

/// The tools the built-in assistant may call: reading only, never control.
pub const READ_ONLY_TOOLS: [&str; 14] = [
    "system_overview",
    "list_processes",
    "process_details",
    "process_report",
    "process_tree",
    "process_modules",
    "why_not_responding",
    "why_wont_it_end",
    "network_connections",
    "services",
    "startup_items",
    "app_history",
    "drives",
    "recent_events",
];

/// Said once under a listing whose network rates were not measured.
pub const NET_UNTRACED: &str =
    "(network per process not measured: that needs the RustWTM service running)";

pub struct Toolbox {
    sampler: Sampler,
    /// When the sampler last sampled.
    sampled: Option<Instant>,
    /// Ask the RustWTM service for its latest sample first: it also measures
    /// network traffic per process.
    pub use_service: bool,
    allow_control: bool,
    /// Include command lines in results. Off for the built-in assistant
    /// when the user chose not to send them.
    pub command_lines: bool,
}

impl Toolbox {
    pub fn new(allow_control: bool) -> Self {
        Self {
            sampler: Sampler::new(),
            sampled: None,
            use_service: true,
            allow_control,
            command_lines: true,
        }
    }

    pub fn allows_control(&self) -> bool {
        self.allow_control
    }

    /// A sample with current rates: unless the previous sample is recent,
    /// a fresh one is taken first to compare against.
    pub fn snapshot(&mut self) -> Result<Snapshot, String> {
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

    /// MCP tool descriptors: name, description and input schema.
    pub fn tools(&self) -> Vec<Value> {
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
        let none = || json!({ "type": "object", "properties": {} });
        let mut tools = vec![
            tool(
                "system_overview",
                "CPU, memory and disk load, the busiest programs and programs that are not responding.",
                none(),
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
                "process_report",
                "Everything known about one process in one go: usage, path, command line, user, \
                 digital signature of its executable, parent and child processes, Windows services \
                 it hosts, its network ports, whether it starts with Windows, protection and hung \
                 windows. Use it to explain what a process is and whether it is safe.",
                pid.clone(),
            ),
            tool(
                "process_tree",
                "The ancestors (who started it) and all descendants of one process.",
                pid.clone(),
            ),
            tool(
                "process_modules",
                "DLLs and other modules loaded into one process; modules outside the Windows \
                 folder are checked for a valid digital signature.",
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
                "services",
                "Windows services with state, start type, account and hosting PID, optionally \
                 filtered by part of the name or by PID.",
                json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Part of the service or display name" },
                        "pid": { "type": "integer" },
                        "running_only": { "type": "boolean" },
                    },
                }),
            ),
            tool(
                "startup_items",
                "Programs that start when Windows starts (Run keys and Startup folders), enabled or not.",
                none(),
            ),
            tool(
                "app_history",
                "Per-application CPU, GPU and memory over the last hours, with sustained problems \
                 the RustWTM service found (chronic CPU, memory, handle or thread growth). Needs \
                 the service.",
                json!({ "type": "object", "properties": { "hours": { "type": "integer", "minimum": 1, "maximum": 720 } } }),
            ),
            tool("drives", "Every drive with its free space.", none()),
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

    /// Descriptors of [`READ_ONLY_TOOLS`], for a model to call.
    pub fn read_only_tools(&self) -> Vec<Value> {
        self.tools()
            .into_iter()
            .filter(|t| READ_ONLY_TOOLS.contains(&t["name"].as_str().unwrap_or("")))
            .collect()
    }

    /// Whether `name` is a tool at all (control tools count even when off).
    pub fn knows(&self, name: &str) -> bool {
        CONTROL_TOOLS.contains(&name)
            || self
                .tools()
                .iter()
                .any(|t| t["name"].as_str() == Some(name))
    }

    /// Runs a tool; control tools only when allowed.
    pub fn run(&mut self, name: &str, args: &Value) -> Result<String, String> {
        if CONTROL_TOOLS.contains(&name) && !self.allow_control {
            return Err(
                "control tools are off; start `wtm mcp --allow-control` to enable them".into(),
            );
        }
        self.run_tool(name, args)
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
                Ok(listing(&snap, sort, limit as usize, &filter))
            }
            "process_details" => {
                let t = self.target(args)?;
                let d = control::inspect(&t).map_err(|e| e.to_string())?;
                let command_line = if self.command_lines {
                    d.command_line.unwrap_or_default()
                } else {
                    "(not shared)".to_owned()
                };
                Ok(format!(
                    "{} (PID {})\npath: {}\ncommand line: {}\nuser: {}\npriority: {}\nefficiency mode: {}",
                    t.name,
                    t.pid,
                    d.path.unwrap_or_default(),
                    command_line,
                    d.user.unwrap_or_default(),
                    d.priority.map(|p| format!("{p:?}")).unwrap_or_default(),
                    d.eco_qos.map(|e| format!("{e:?}")).unwrap_or_default(),
                ))
            }
            "process_report" => {
                let t = self.target(args)?;
                let snap = self.snapshot()?;
                Ok(process_report(&t, &snap, self.command_lines))
            }
            "process_tree" => {
                let t = self.target(args)?;
                let snap = self.snapshot()?;
                Ok(process_tree(&t, &snap.processes))
            }
            "process_modules" => {
                let t = self.target(args)?;
                process_modules(&t)
            }
            "why_not_responding" => {
                let t = self.target(args)?;
                why_not_responding(&t)
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
                let names: HashMap<u32, &str> = snap
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
            "services" => {
                let filter = args
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_lowercase();
                let pid = args.get("pid").and_then(Value::as_u64);
                let running_only = args.get("running_only").and_then(Value::as_bool) == Some(true);
                let list = wtm_core::list_services().map_err(|e| e.to_string())?;
                let lines: Vec<String> = list
                    .iter()
                    .filter(|s| {
                        filter.is_empty()
                            || s.name.to_lowercase().contains(&filter)
                            || s.display_name.to_lowercase().contains(&filter)
                    })
                    .filter(|s| pid.is_none_or(|p| u64::from(s.pid) == p))
                    .filter(|s| !running_only || s.pid != 0)
                    .take(300)
                    .map(|s| service_line(s, self.command_lines))
                    .collect();
                Ok(if lines.is_empty() {
                    "no matching service".into()
                } else {
                    lines.join("\n")
                })
            }
            "startup_items" => Ok(startup_text(self.command_lines)),
            "app_history" => {
                let hours = args
                    .get("hours")
                    .and_then(Value::as_u64)
                    .unwrap_or(24)
                    .clamp(1, 720) as u32;
                app_history(hours)
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
                let mut client = verified_service()?;
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
        Ok(overview_text(&snap))
    }
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": schema })
}

/// A connection to the installed service, or why there is none. Only the
/// verified service is trusted: a program squatting on the pipe could
/// otherwise feed made-up text to the assistant.
fn verified_service() -> Result<wtm_ipc::Client, String> {
    let client = wtm_ipc::Client::connect()
        .map_err(|e| format!("the RustWTM service is not reachable: {e}"))?;
    if !client.is_verified() {
        return Err("the RustWTM pipe is not served by the installed service".into());
    }
    Ok(client)
}

/// The installed service's latest sample, while it runs. Only a verified
/// service is trusted: a program squatting on the pipe could otherwise
/// misname processes for the control tools.
fn service_snapshot() -> Option<Snapshot> {
    let mut client = verified_service().ok()?;
    match client.call(&wtm_ipc::Request::Snapshot).ok()? {
        wtm_ipc::Response::Snapshot(s) if s.interval.is_some() => Some(s),
        _ => None,
    }
}

/// CPU, memory and disk load of the whole machine, no process named.
pub fn totals_text(snap: &Snapshot) -> String {
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
    out
}

/// CPU, memory, disks, the busiest processes and hung windows.
pub fn overview_text(snap: &Snapshot) -> String {
    let mut out = totals_text(snap);
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
    out
}

/// Processes sorted by `sort`, at most `limit`, names containing `filter`.
pub fn listing(snap: &Snapshot, sort: &str, limit: usize, filter: &str) -> String {
    let mut procs: Vec<&ProcessInfo> = snap
        .processes
        .iter()
        .filter(|p| p.pid != 0 && p.name.to_lowercase().contains(filter))
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
        .take(limit)
        .map(|p| process_line(p, snap.net_traced))
        .collect();
    if !snap.net_traced {
        lines.push(NET_UNTRACED.into());
    }
    lines.join("\n")
}

/// One process; the network rate only when it was measured (`net`), since
/// otherwise it would read as an idle 0 B/s.
pub fn process_line(p: &ProcessInfo, net: bool) -> String {
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

/// The parent of `p` in `procs`: the process with its parent PID that was
/// started before it (a later one only took a freed PID).
fn parent_of<'a>(p: &ProcessInfo, procs: &'a [ProcessInfo]) -> Option<&'a ProcessInfo> {
    procs
        .iter()
        .find(|q| q.pid == p.parent_pid && q.pid != p.pid && q.create_time <= p.create_time)
}

fn signature_text(path: &str) -> String {
    match wtm_core::signature(path) {
        wtm_core::Signature::Signed { signer, catalog } => format!(
            "signed by {}{}",
            signer.as_deref().unwrap_or("an unnamed publisher"),
            if catalog { " (Windows catalog)" } else { "" }
        ),
        wtm_core::Signature::Unsigned => "NOT SIGNED".into(),
        wtm_core::Signature::Invalid(why) => format!("INVALID SIGNATURE: {why}"),
    }
}

/// How long ago `p` started.
fn uptime(p: &ProcessInfo) -> Option<String> {
    let started = p.start_time()?;
    let age = SystemTime::now().duration_since(started).ok()?;
    Some(format_duration(age))
}

/// Everything about one process, for an assistant to explain it.
pub fn process_report(t: &ProcessRef, snap: &Snapshot, command_lines: bool) -> String {
    let mut out = Vec::new();
    let me = snap
        .processes
        .iter()
        .find(|p| p.pid == t.pid && p.create_time == t.create_time);
    out.push(format!("process: {} (PID {})", t.name, t.pid));
    if let Some(p) = me {
        if let Some(age) = uptime(p) {
            out.push(format!("running for: {age}"));
        }
        out.push(format!(
            "usage: CPU {:.1}%, private memory {}, working set {}, disk {}/s{}, GPU {:.0}%, {} threads, {} handles{}",
            p.cpu_percent,
            format_bytes(p.private_working_set as f64),
            format_bytes(p.working_set as f64),
            format_bytes(p.io_rate()),
            if snap.net_traced {
                format!(", network {}/s", format_bytes(p.net_rate()))
            } else {
                String::new()
            },
            p.gpu_percent,
            p.thread_count,
            p.handle_count,
            if p.suspended { ", SUSPENDED" } else { "" },
        ));
        out.push(format!("session: {}", p.session_id));
    }
    let details = control::inspect(t).ok();
    let path = details.as_ref().and_then(|d| d.path.clone());
    if let Some(d) = &details {
        out.push(format!(
            "path: {}",
            d.path.as_deref().unwrap_or("(unknown)")
        ));
        if command_lines {
            out.push(format!(
                "command line: {}",
                d.command_line.as_deref().unwrap_or("(unknown)")
            ));
        }
        out.push(format!(
            "user: {}",
            d.user.as_deref().unwrap_or("(unknown)")
        ));
        if let Some(pr) = d.priority {
            out.push(format!("priority: {pr:?}"));
        }
        if let Some(e) = d.eco_qos {
            out.push(format!("efficiency mode: {e:?}"));
        }
        if d.critical == Some(true) {
            out.push("critical: ending it stops Windows".into());
        }
        if let Some(prot) = &d.protected {
            out.push(format!("protected process: {prot}"));
        }
    } else {
        out.push("details: not readable with these rights".into());
    }
    if let Some(path) = path.as_deref().filter(|p| !p.is_empty()) {
        out.push(format!("executable signature: {}", signature_text(path)));
    }
    if let Some(reason) = wtm_core::protection_reason(t.pid, &t.name) {
        out.push(format!("RustWTM protects it: {reason}"));
    }
    if let Some(p) = me {
        match parent_of(p, &snap.processes) {
            Some(parent) => out.push(format!("started by: {} (PID {})", parent.name, parent.pid)),
            None => out.push(format!(
                "started by: PID {} (no longer running)",
                p.parent_pid
            )),
        }
        let children = wtm_core::descendants(t.pid, t.create_time, &snap.processes);
        if !children.is_empty() {
            let mut counts: Vec<(String, usize)> = Vec::new();
            for c in &children {
                match counts.iter_mut().find(|(n, _)| *n == c.name) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((c.name.clone(), 1)),
                }
            }
            let names: Vec<String> = counts
                .iter()
                .take(15)
                .map(|(n, k)| {
                    if *k > 1 {
                        format!("{n} ×{k}")
                    } else {
                        n.clone()
                    }
                })
                .collect();
            out.push(format!(
                "descendants: {} ({})",
                children.len(),
                names.join(", ")
            ));
        }
    }
    if let Ok(services) = wtm_core::list_services() {
        let hosted: Vec<String> = services
            .iter()
            .filter(|s| s.pid == t.pid)
            .map(|s| format!("{} ({})", s.name, s.display_name))
            .collect();
        if !hosted.is_empty() {
            out.push(format!("hosts services: {}", hosted.join(", ")));
        }
    }
    if let Ok(conns) = wtm_core::connections() {
        let mine: Vec<&wtm_core::Connection> = conns.iter().filter(|c| c.pid == t.pid).collect();
        if !mine.is_empty() {
            let mut listening: Vec<String> = mine
                .iter()
                .filter(|c| {
                    c.remote.is_none()
                        && (c.state.is_none() || c.state == Some(wtm_core::TcpState::Listen))
                })
                .map(|c| format!("{:?} {}", c.protocol, c.local))
                .collect();
            listening.sort();
            listening.dedup();
            let mut remotes: Vec<String> = mine
                .iter()
                .filter_map(|c| c.remote)
                .map(|r| r.to_string())
                .collect();
            remotes.sort();
            remotes.dedup();
            out.push(format!(
                "network: {} endpoints; listening on {}; talking to {}",
                mine.len(),
                if listening.is_empty() {
                    "nothing".to_owned()
                } else {
                    listening
                        .iter()
                        .take(15)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                },
                if remotes.is_empty() {
                    "no one".to_owned()
                } else {
                    remotes
                        .iter()
                        .take(12)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ));
        }
    }
    if let Some(exe) = path.as_deref().and_then(|p| p.rsplit('\\').next()) {
        let exe = exe.to_lowercase();
        let starts: Vec<String> = wtm_core::startup::list_startup()
            .into_iter()
            .filter(|e| e.command.to_lowercase().contains(&exe))
            .map(|e| {
                format!(
                    "{} ({:?}, {:?}, {})",
                    e.name,
                    e.scope,
                    e.source,
                    if e.enabled { "enabled" } else { "disabled" }
                )
            })
            .collect();
        if !starts.is_empty() {
            out.push(format!("starts with Windows as: {}", starts.join(", ")));
        }
    }
    if let Some(w) = wtm_core::hung_windows()
        .into_iter()
        .find(|w| w.pid == t.pid)
    {
        out.push(format!("NOT RESPONDING: window \"{}\"", w.title));
    }
    out.join("\n")
}

/// Who started `t`, up to the top, and everything it started.
pub fn process_tree(t: &ProcessRef, procs: &[ProcessInfo]) -> String {
    let Some(me) = procs
        .iter()
        .find(|p| p.pid == t.pid && p.create_time == t.create_time)
    else {
        return format!("PID {} is no longer running", t.pid);
    };
    let mut chain = vec![me];
    while chain.len() < 16 {
        match parent_of(chain[chain.len() - 1], procs) {
            Some(parent) if !chain.iter().any(|c| c.pid == parent.pid) => chain.push(parent),
            _ => break,
        }
    }
    let mut out = String::from("ancestors (oldest first):\n");
    for (depth, p) in chain.iter().rev().enumerate() {
        out.push_str(&format!(
            "{}{} (PID {})\n",
            "  ".repeat(depth),
            p.name,
            p.pid
        ));
    }
    let descendants = wtm_core::descendants(t.pid, t.create_time, procs);
    if descendants.is_empty() {
        out.push_str("no child processes\n");
    } else {
        out.push_str(&format!("descendants ({}):\n", descendants.len()));
        for d in descendants.iter().take(200) {
            out.push_str(&format!(
                "  {} (PID {}, parent {}): CPU {:.1}%, memory {}\n",
                d.name,
                d.pid,
                d.parent_pid,
                d.cpu_percent,
                format_bytes(d.private_working_set as f64)
            ));
        }
    }
    out
}

/// Modules of `t`; those outside the Windows folder are signature-checked
/// (at most a few dozen: each check reads the file).
fn process_modules(t: &ProcessRef) -> Result<String, String> {
    const CHECKED: usize = 40;
    let modules = wtm_core::process_modules(t.pid, t.create_time).map_err(|e| e.to_string())?;
    let windir = std::env::var("SystemRoot")
        .unwrap_or_else(|_| r"C:\Windows".into())
        .to_lowercase();
    let mut out = format!("{} modules in {} (PID {})\n", modules.len(), t.name, t.pid);
    let mut checked = 0;
    let mut windows = 0;
    for m in &modules {
        if m.path.to_lowercase().starts_with(&windir) {
            windows += 1;
            continue;
        }
        let sig = if checked < CHECKED {
            checked += 1;
            signature_text(&m.path)
        } else {
            "not checked".into()
        };
        out.push_str(&format!(
            "{} ({}): {}\n",
            m.path,
            format_bytes(m.size as f64),
            sig
        ));
    }
    out.push_str(&format!("{windows} more from the Windows folder\n"));
    Ok(out)
}

fn why_not_responding(t: &ProcessRef) -> Result<String, String> {
    let hung = wtm_core::hung_windows()
        .into_iter()
        .find(|w| w.pid == t.pid);
    let report =
        wtm_core::wait_chains(t.pid, Duration::from_secs(15)).map_err(|e| e.to_string())?;
    let mut out = match hung {
        Some(w) => format!("Window \"{}\" is not responding.\n", w.title),
        None => "No window of it is marked as not responding.\n".to_owned(),
    };
    if report.deadlocked() {
        out.push_str("DEADLOCK: threads wait for each other and will never continue.\n");
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

fn service_line(s: &wtm_core::ServiceEntry, command_lines: bool) -> String {
    let binary = if command_lines {
        s.binary_path
            .as_deref()
            .map(|b| format!(", binary {b}"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    format!(
        "{} ({}): {:?}, start {}, account {}{}{binary}",
        s.name,
        s.display_name,
        s.state,
        s.start_type
            .map(|t| format!("{t:?}"))
            .unwrap_or_else(|| "?".into()),
        s.account.as_deref().unwrap_or("?"),
        if s.pid != 0 {
            format!(", PID {}", s.pid)
        } else {
            String::new()
        },
    )
}

/// The startup entries, one per line.
pub fn startup_text(command_lines: bool) -> String {
    let entries = wtm_core::startup::list_startup();
    if entries.is_empty() {
        return "no startup programs".into();
    }
    entries
        .iter()
        .map(|e| {
            let command = if command_lines {
                format!(": {}", e.command)
            } else {
                String::new()
            };
            format!(
                "{} [{:?}, {:?}, {}]{command}",
                e.name,
                e.scope,
                e.source,
                if e.enabled { "enabled" } else { "disabled" }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The service's per-app summary over `hours`, busiest first, and findings.
pub fn app_history(hours: u32) -> Result<String, String> {
    let mut client = verified_service()?;
    match client.call(&wtm_ipc::Request::History { hours }) {
        Ok(wtm_ipc::Response::History { stats, findings }) => {
            let mut stats = stats;
            stats.sort_by(|a, b| b.cpu_avg.total_cmp(&a.cpu_avg));
            let mut out = format!("last {hours} h, busiest applications first:\n");
            for s in stats.iter().take(20) {
                out.push_str(&format!(
                    "  {}: running {} min, CPU avg {:.1}% (p95 {:.1}%, peak {:.0}%), GPU avg {:.1}%, memory avg {} (peak {}, growth {}/h)\n",
                    s.app,
                    s.minutes,
                    s.cpu_avg,
                    s.cpu_p95,
                    s.cpu_peak,
                    s.gpu_avg,
                    format_bytes(s.mem_avg as f64),
                    format_bytes(s.mem_peak as f64),
                    format_bytes(s.mem_growth_per_hour),
                ));
            }
            if findings.is_empty() {
                out.push_str("no sustained problems found\n");
            } else {
                out.push_str("sustained problems:\n");
                for f in &findings {
                    out.push_str(&format!("  {} {:?}: {}\n", f.app, f.kind, f.detail));
                }
            }
            Ok(out)
        }
        Ok(other) => Err(format!("unexpected answer: {other:?}")),
        Err(e) => Err(e.to_string()),
    }
}

/// Refuses `target` when the caller named a different instance of its PID
/// (`create_time` from an earlier list_processes).
///
/// Creation times count 100 ns since 1601, past what a JSON number keeps
/// exactly in JavaScript clients, which round them. Text compares exactly;
/// a number matches at the precision it can have, a window of about 2 µs
/// in which the same PID is not reused.
pub fn check_identity(target: &ProcessRef, create_time: Option<&Value>) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut tools = Toolbox::new(false);
        tools.use_service = false;
        let first = tools.snapshot().unwrap();
        assert!(
            first.interval.is_some_and(|d| d >= WINDOW),
            "rates need a window"
        );
        // Right after: the previous sample is recent enough to compare with.
        let started = Instant::now();
        let second = tools.snapshot().unwrap();
        assert!(started.elapsed() < WINDOW);
        assert!(second.interval.is_some_and(|d| d < FRESH));
        // Long after: measured afresh, not averaged over the pause.
        tools.sampled = Some(Instant::now() - FRESH * 5);
        let third = tools.snapshot().unwrap();
        assert!(third.interval.is_some_and(|d| d < FRESH));
    }

    #[test]
    fn reports_and_trees_describe_this_process() {
        let mut tools = Toolbox::new(false);
        tools.use_service = false;
        let me = json!({ "pid": std::process::id() });
        let report = tools.run("process_report", &me).unwrap();
        assert!(
            report.contains(&format!("(PID {})", std::process::id())),
            "{report}"
        );
        assert!(report.contains("path: "), "{report}");
        assert!(report.contains("command line: "), "{report}");
        assert!(report.contains("started by: "), "{report}");
        // Without command lines, none is shown.
        tools.command_lines = false;
        let report = tools.run("process_report", &me).unwrap();
        assert!(!report.contains("command line: "), "{report}");
        let details = tools.run("process_details", &me).unwrap();
        assert!(details.contains("command line: (not shared)"), "{details}");

        let tree = tools.run("process_tree", &me).unwrap();
        assert!(tree.starts_with("ancestors (oldest first):"), "{tree}");
        assert!(
            tree.contains(&format!("(PID {})", std::process::id())),
            "{tree}"
        );
    }

    #[test]
    fn modules_services_and_startup_answer() {
        let mut tools = Toolbox::new(false);
        tools.use_service = false;
        let modules = tools
            .run("process_modules", &json!({ "pid": std::process::id() }))
            .unwrap();
        assert!(
            modules.contains("more from the Windows folder"),
            "{modules}"
        );
        let services = tools
            .run("services", &json!({ "name": "eventlog" }))
            .unwrap();
        assert!(services.to_lowercase().contains("eventlog"), "{services}");
        let none = tools
            .run("services", &json!({ "name": "no-such-service-xyz" }))
            .unwrap();
        assert_eq!(none, "no matching service");
        assert!(tools.run("startup_items", &json!({})).is_ok());
    }

    #[test]
    fn control_tools_are_gated() {
        let mut tools = Toolbox::new(false);
        assert!(tools.knows("end_process") && tools.knows("process_report"));
        assert!(!tools.knows("format_disk"));
        let e = tools.run("end_process", &json!({ "pid": 1 })).unwrap_err();
        assert!(e.contains("--allow-control"), "{e}");
        assert!(
            !tools
                .tools()
                .iter()
                .any(|t| CONTROL_TOOLS.contains(&t["name"].as_str().unwrap()))
        );
        // The assistant's list is exactly the read-only one, even with
        // control allowed.
        let names: Vec<String> = Toolbox::new(true)
            .read_only_tools()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(names.len(), READ_ONLY_TOOLS.len());
        assert!(names.iter().all(|n| !CONTROL_TOOLS.contains(&n.as_str())));
    }
}

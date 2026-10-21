//! `wtm mcp`: a Model Context Protocol server on standard input and output,
//! so AI assistants (Claude Desktop, Claude Code, ...) can ask what this PC
//! is doing. Nothing listens on the network: the assistant starts this
//! program and talks to it over a pipe. Read-only unless started with
//! `--allow-control`, which adds tools that end, suspend or deprioritize
//! processes with the rights of whoever started it.

use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::time::Duration;

use serde_json::{Value, json};
use wtm_core::control::{self, EcoQos, PriorityClass, ProcessRef};
use wtm_core::{ProcessInfo, Sampler, Snapshot, format_bytes};

/// Protocol revisions this server speaks, newest first.
const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

pub struct Server {
    sampler: Sampler,
    primed: bool,
    allow_control: bool,
}

impl Server {
    pub fn new(allow_control: bool) -> Self {
        Self {
            sampler: Sampler::new(),
            primed: false,
            allow_control,
        }
    }

    /// A sample with rates: the first one needs a second sample to compare.
    fn snapshot(&mut self) -> Result<Snapshot, String> {
        if !self.primed {
            self.sampler.sample().map_err(|e| e.to_string())?;
            std::thread::sleep(Duration::from_millis(400));
            self.primed = true;
        }
        self.sampler.sample().map_err(|e| e.to_string())
    }

    /// Answers one JSON-RPC message; `None` for notifications.
    pub fn handle(&mut self, message: &Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let id = id?;
        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": self.tools() })),
            "tools/call" => Ok(self.call(&params)),
            _ => Err((-32601, format!("method not found: {method}"))),
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
        let mut tools = vec![
            tool(
                "system_overview",
                "CPU, memory and disk load, the busiest programs and programs that are not responding.",
                json!({ "type": "object", "properties": {} }),
            ),
            tool(
                "list_processes",
                "Processes sorted by cpu, memory, disk or network, optionally filtered by name.",
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
                tool("end_process", "Ends a process (unsaved data in it is lost).", pid.clone()),
                tool("suspend_process", "Freezes every thread of a process.", pid.clone()),
                tool("resume_process", "Resumes a suspended process.", pid.clone()),
                tool("set_priority", "Changes a process's priority class.", json!({
                    "type": "object",
                    "properties": {
                        "pid": { "type": "integer" },
                        "priority": { "type": "string", "enum": ["idle", "below_normal", "normal", "above_normal", "high"] },
                    },
                    "required": ["pid", "priority"],
                })),
                tool("efficiency_mode", "Turns efficiency mode (EcoQoS) on or off for a process.", json!({
                    "type": "object",
                    "properties": { "pid": { "type": "integer" }, "on": { "type": "boolean" } },
                    "required": ["pid", "on"],
                })),
            ]);
        }
        tools
    }

    fn call(&mut self, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        let control_tool = [
            "end_process",
            "suspend_process",
            "resume_process",
            "set_priority",
            "efficiency_mode",
        ]
        .contains(&name);
        let result = if control_tool && !self.allow_control {
            Err("control tools are off; start `wtm mcp --allow-control` to enable them".into())
        } else {
            self.run_tool(name, &args)
        };
        match result {
            Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
            Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
        }
    }

    fn target(&mut self, args: &Value) -> Result<ProcessRef, String> {
        let pid = args
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|p| u32::try_from(p).ok())
            .ok_or("`pid` is required")?;
        let snap = self.snapshot()?;
        snap.processes
            .iter()
            .find(|p| p.pid == pid)
            .map(ProcessRef::from)
            .ok_or_else(|| format!("no process with PID {pid}"))
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
                Ok(procs
                    .into_iter()
                    .take(limit as usize)
                    .map(process_line)
                    .collect::<Vec<_>>()
                    .join("\n"))
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
                for c in report.chains.iter().take(10) {
                    let links: Vec<String> = c.links.iter().map(link_text).collect();
                    out.push_str(&links.join(" -> "));
                    out.push('\n');
                }
                Ok(out)
            }
            "why_wont_it_end" => {
                let t = self.target(args)?;
                let r = wtm_core::why_alive(t.pid).map_err(|e| e.to_string())?;
                Ok(serde_json::to_string_pretty(&r).unwrap_or_default())
            }
            "network_connections" => {
                let pid = args.get("pid").and_then(Value::as_u64);
                let snap = self.snapshot()?;
                let names: std::collections::HashMap<u32, &str> = snap
                    .processes
                    .iter()
                    .map(|p| (p.pid, p.name.as_str()))
                    .collect();
                let conns = wtm_core::connections().map_err(|e| e.to_string())?;
                Ok(conns
                    .iter()
                    .filter(|c| pid.is_none_or(|p| u64::from(c.pid) == p))
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
                let t = self.target(args)?;
                control::kill(&t).map_err(|e| e.to_string())?;
                Ok(format!("{} (PID {}) ended", t.name, t.pid))
            }
            "suspend_process" => {
                let t = self.target(args)?;
                control::suspend(&t).map_err(|e| e.to_string())?;
                Ok(format!("{} (PID {}) suspended", t.name, t.pid))
            }
            "resume_process" => {
                let t = self.target(args)?;
                control::resume(&t).map_err(|e| e.to_string())?;
                Ok(format!("{} (PID {}) resumed", t.name, t.pid))
            }
            "set_priority" => {
                let t = self.target(args)?;
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
                let t = self.target(args)?;
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
            out.push_str(&format!("  {}\n", process_line(p)));
        }
        let mut by_mem = by_cpu.clone();
        by_mem.sort_by_key(|p| std::cmp::Reverse(p.private_working_set));
        out.push_str("largest by memory:\n");
        for p in by_mem.iter().take(5) {
            out.push_str(&format!("  {}\n", process_line(p)));
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

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": schema })
}

fn process_line(p: &ProcessInfo) -> String {
    format!(
        "{} (PID {}): CPU {:.1}%, memory {}, disk {}/s, network {}/s",
        p.name,
        p.pid,
        p.cpu_percent,
        format_bytes(p.private_working_set as f64),
        format_bytes(p.io_rate()),
        format_bytes(p.net_rate())
    )
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
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(message) => server.handle(&message),
            Err(e) => Some(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32700, "message": format!("parse error: {e}") },
            })),
        };
        if let Some(reply) = reply {
            writeln!(output, "{reply}")?;
            output.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(listing.lines().count(), 3, "{listing}");
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
    fn bad_json_is_a_parse_error() {
        let mut out = Vec::new();
        run("{not json\n".as_bytes(), &mut out, false).unwrap();
        let reply: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(reply["error"]["code"], -32700);
    }
}

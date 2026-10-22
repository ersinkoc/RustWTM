//! Telegram bot: answers commands from allowed chats (status, busiest
//! processes, ports, services, ending and suspending processes) and
//! forwards alerts. Runs on its own thread and long-polls the Bot API.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tracing::{debug, warn};
use wtm_core::control::{self, ProcessRef};
use wtm_core::ports::{PortUse, end_blocked, relay_hint};
use wtm_core::{ProcessInfo, ServiceEntry, ServiceState, Snapshot, format_bytes};
use wtm_ipc::{Command, Event, EventLevel, Outcome, Status};
use wtm_policy::{Config, TelegramConfig};

use crate::ipc_server::{EngineMsg, Shared};

const USER_AGENT: &str = concat!("RustWTM/", env!("CARGO_PKG_VERSION"));
/// How long `getUpdates` waits for a message; also how often alerts go out.
const POLL_SECS: u64 = 2;
/// Telegram's limit is 4096 characters per message.
const MAX_TEXT: usize = 3800;
/// Unanswered confirmations expire.
const PENDING_TTL: Duration = Duration::from_secs(300);
/// Chats that are not allowed are told their ID at most this often.
const TELL_ID_EVERY: Duration = Duration::from_secs(600);

/// The menu Telegram shows next to the message box.
const MENU: [(&str, &str); 19] = [
    ("status", "CPU, memory, disks and network"),
    ("top", "Busiest processes by CPU: /top 15"),
    ("mem", "Processes using the most memory"),
    ("find", "Processes whose name contains text: /find chrome"),
    ("info", "Details of a process: /info 1234"),
    ("kill", "End a process by PID or name (asks first)"),
    ("tree", "End a process and everything it started"),
    ("suspend", "Pause a process: /suspend 1234"),
    ("resume", "Let a paused process run again"),
    ("wait", "Message me when a process exits: /wait 1234"),
    ("unwait", "Stop every /wait of this chat"),
    ("ports", "Listening ports and the programs behind them"),
    ("free", "End what uses a port: /free 3000"),
    ("services", "Find services: /services sql"),
    ("restartsvc", "Restart a service: /restartsvc Spooler"),
    ("swarms", "Hundreds of copies of one program"),
    ("panic", "End everything on the kill list (asks first)"),
    ("events", "Recent service events"),
    ("mute", "Pause alerts: /mute 60 (minutes); /unmute"),
];

#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    /// The token was rejected or the bot deleted.
    Unauthorized,
    /// Something else polls updates with this token.
    Conflict,
    Other(String),
}

/// The Telegram Bot API: `method` with a JSON body, returning `result`.
pub trait Api {
    fn call(&mut self, method: &str, body: Value, timeout: Duration) -> Result<Value, ApiError>;
}

pub struct HttpApi {
    base: String,
}

impl HttpApi {
    pub fn new(token: &str) -> Self {
        Self {
            base: format!("https://api.telegram.org/bot{token}"),
        }
    }
}

impl Api for HttpApi {
    fn call(&mut self, method: &str, body: Value, timeout: Duration) -> Result<Value, ApiError> {
        let (status, reply) = wtm_sys::http::post(
            &format!("{}/{method}", self.base),
            "application/json",
            body.to_string().as_bytes(),
            USER_AGENT,
            timeout.as_millis().min(i32::MAX as u128) as i32,
        )
        .map_err(|e| ApiError::Other(e.to_string()))?;
        api_result(status, &reply)
    }
}

/// Telegram answers `{"ok": true, "result": …}` or a description.
fn api_result(status: u16, body: &[u8]) -> Result<Value, ApiError> {
    match status {
        401 | 404 => return Err(ApiError::Unauthorized),
        409 => return Err(ApiError::Conflict),
        _ => {}
    }
    let reply: Value =
        serde_json::from_slice(body).map_err(|e| ApiError::Other(format!("HTTP {status}: {e}")))?;
    if reply["ok"] == true {
        Ok(reply["result"].clone())
    } else {
        Err(ApiError::Other(
            reply["description"]
                .as_str()
                .unwrap_or("request failed")
                .to_owned(),
        ))
    }
}

/// What the bot reads and changes on this PC.
pub trait Host {
    fn snapshot(&self) -> Option<Arc<Snapshot>>;
    fn status(&self) -> Status;
    fn config(&self) -> Config;
    fn events_after(&self, id: u64) -> Vec<Event>;
    fn control(&self, target: &ProcessRef, command: Command) -> Vec<Outcome>;
    fn command_line(&self, p: &ProcessInfo) -> Option<String>;
    fn image_path(&self, p: &ProcessInfo) -> Option<String>;
    fn port_uses(&self) -> Result<Vec<PortUse>, String>;
    fn services(&self) -> Result<Vec<ServiceEntry>, String>;
    fn set_service(&self, name: &str, op: ServiceOp) -> Result<(), String>;
    /// Records a line in the service event log.
    fn note(&self, message: String);
}

pub struct ServiceHost {
    shared: Arc<Shared>,
    engine: Sender<EngineMsg>,
}

impl Host for ServiceHost {
    fn snapshot(&self) -> Option<Arc<Snapshot>> {
        self.shared
            .snapshot
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn status(&self) -> Status {
        self.shared
            .status
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn config(&self) -> Config {
        self.shared
            .config
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn events_after(&self, id: u64) -> Vec<Event> {
        self.shared.events.after(id)
    }

    fn control(&self, target: &ProcessRef, command: Command) -> Vec<Outcome> {
        let (reply, rx) = mpsc::channel();
        let failed = |e: &str| {
            vec![Outcome {
                target: target.clone(),
                result: Err(e.to_owned()),
            }]
        };
        if self
            .engine
            .send(EngineMsg::Control {
                target: target.clone(),
                command,
                reply,
            })
            .is_err()
        {
            return failed("the service is stopping");
        }
        rx.recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| failed("the service did not answer in time"))
    }

    fn command_line(&self, p: &ProcessInfo) -> Option<String> {
        control::command_line(&ProcessRef::from(p)).ok()
    }

    fn image_path(&self, p: &ProcessInfo) -> Option<String> {
        control::image_path(&ProcessRef::from(p)).ok()
    }

    fn port_uses(&self) -> Result<Vec<PortUse>, String> {
        wtm_core::connections()
            .map(|c| wtm_core::ports::port_uses(&c))
            .map_err(|e| e.to_string())
    }

    fn services(&self) -> Result<Vec<ServiceEntry>, String> {
        wtm_core::list_services().map_err(|e| e.to_string())
    }

    fn set_service(&self, name: &str, op: ServiceOp) -> Result<(), String> {
        match op {
            ServiceOp::Start => wtm_core::start_service(name),
            ServiceOp::Stop => wtm_core::stop_service(name),
            ServiceOp::Restart => wtm_core::restart_service(name, Duration::from_secs(30)),
        }
        .map_err(|e| e.to_string())
    }

    fn note(&self, message: String) {
        self.shared.events.info(None, message);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceOp {
    Start,
    Stop,
    Restart,
}

/// A process a chat wants to hear about when it exits.
struct Wait {
    chat: i64,
    target: ProcessRef,
    since: Instant,
}

/// At most this many `/wait`s across all chats.
const MAX_WAITS: usize = 50;

/// A message to send: HTML text and inline buttons (label, callback data).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reply {
    pub text: String,
    pub buttons: Vec<Vec<(String, String)>>,
}

impl Reply {
    fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            buttons: Vec::new(),
        }
    }
}

/// What a button press leads to.
#[derive(Debug, Clone, PartialEq)]
enum Pressed {
    /// Replace the message that held the button.
    Edit(Reply),
    /// Send a new message.
    Send(Reply),
    /// Only the short notice Telegram shows on top of the chat.
    Toast(String),
}

/// Processes waiting for "End" to be confirmed.
struct Pending {
    chat: i64,
    targets: Vec<ProcessRef>,
    /// Suspend the targets on confirmation instead of ending them.
    suspend: bool,
    created: Instant,
}

pub struct Bot<A, H> {
    api: A,
    host: H,
    cfg: TelegramConfig,
    /// Shown in every message, for people with several PCs on one bot.
    computer: String,
    offset: i64,
    last_event: u64,
    pending: HashMap<u64, Pending>,
    next_pending: u64,
    muted_until: Option<Instant>,
    told: HashMap<i64, Instant>,
    waits: Vec<Wait>,
    /// The bot's own @name, to ignore commands meant for other bots.
    username: Option<String>,
    /// Swarm names behind the buttons of /swarms: button data is limited
    /// to 64 bytes, which a long image name would exceed.
    swarm_names: HashMap<u64, (String, Instant)>,
    /// Updates waiting from before this bot started were dropped: they
    /// may be hours old or already handled, and are never acted on.
    caught_up: bool,
}

/// `&`, `<` and `>` as HTML entities.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Lines in a monospace block, cut to fit one message.
fn pre(lines: &[String]) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if out.len() + line.len() > MAX_TEXT - 200 {
            out.push_str(&format!("… {} more\n", lines.len() - i));
            break;
        }
        out.push_str(&esc(line));
        out.push('\n');
    }
    format!("<pre>{}</pre>", out.trim_end())
}

/// At most `n` characters, with an ellipsis when cut.
fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        let cut: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

fn ago(unix_ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let secs = now.saturating_sub(unix_ms) / 1000;
    match secs {
        0..60 => format!("{secs}s ago"),
        60..3600 => format!("{}m ago", secs / 60),
        _ => format!("{}h ago", secs / 3600),
    }
}

fn rate(bytes_per_sec: f64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

fn mem(p: &ProcessInfo) -> String {
    format_bytes(p.private_working_set as f64)
}

/// `chrome.exe (1234)`.
fn who(name: &str, pid: u32) -> String {
    format!("{name} ({pid})")
}

/// Splits `/top@MyBot 15` into (`top`, `15`).
fn parse_command(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let rest = text.strip_prefix('/')?;
    let (cmd, arg) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let cmd = cmd.split('@').next().unwrap_or(cmd).to_ascii_lowercase();
    Some((cmd, arg.trim().to_owned()))
}

/// Why `p` must not be suspended (resuming is always safe). Suspending is
/// held to what ending is: a frozen svchost takes the network, and with it
/// the way to resume it from here.
fn pause_blocked(p: &ProcessInfo, command: Command) -> Option<String> {
    match command {
        Command::Suspend => end_blocked(p.pid, &p.name),
        _ => wtm_core::protection_reason(p.pid, &p.name).map(|r| format!("protected: {r}")),
    }
}

/// Whether `text` is for this bot: not a command addressed to another one
/// (`/kill@OtherBot`). Without its own name known, no addressed command is.
fn addressed_to(text: &str, me: Option<&str>) -> bool {
    let Some(rest) = text.trim().strip_prefix('/') else {
        return true;
    };
    let cmd = rest.split(char::is_whitespace).next().unwrap_or(rest);
    match cmd.split_once('@') {
        None => true,
        Some((_, to)) => me.is_some_and(|me| me.eq_ignore_ascii_case(to)),
    }
}

/// A count from `arg`, `default` when empty, at most `max`.
fn count(arg: &str, default: usize, max: usize) -> usize {
    arg.parse::<usize>().map_or(default, |n| n.clamp(1, max))
}

const CONTROL_OFF: &str = "Control from Telegram is turned off in RustWTM → Settings → Telegram.";

impl<A: Api, H: Host> Bot<A, H> {
    pub fn new(api: A, host: H, cfg: TelegramConfig, computer: String) -> Self {
        // Alerts from before the bot started are not news.
        let last_event = host.events_after(0).last().map_or(0, |e| e.id);
        Self {
            api,
            host,
            cfg,
            computer,
            offset: 0,
            last_event,
            pending: HashMap::new(),
            next_pending: 1,
            muted_until: None,
            told: HashMap::new(),
            waits: Vec::new(),
            username: None,
            swarm_names: HashMap::new(),
            caught_up: false,
        }
    }

    /// New settings; what removed chats asked for (exit notices, pending
    /// confirmations) goes with them.
    pub fn set_config(&mut self, cfg: TelegramConfig) {
        self.waits.retain(|w| cfg.chats.contains(&w.chat));
        self.pending.retain(|_, p| cfg.chats.contains(&p.chat));
        self.cfg = cfg;
    }

    /// Learns the bot's name and drops the updates that were waiting when
    /// it started; until both are done, no update is handled.
    fn catch_up(&mut self) -> Result<(), ApiError> {
        if self.username.is_none() {
            let me = self.call("getMe", json!({}))?;
            self.username = Some(me["username"].as_str().unwrap_or_default().to_owned());
        }
        if !self.caught_up {
            // The latest update alone, which also confirms all before it.
            let last = self.call("getUpdates", json!({ "offset": -1, "timeout": 0 }))?;
            if let Some(id) = last
                .as_array()
                .and_then(|u| u.last())
                .and_then(|u| u["update_id"].as_i64())
            {
                self.offset = id + 1;
            }
            self.caught_up = true;
        }
        Ok(())
    }

    /// Sends `reply` to every allowed chat. A chat that cannot be reached
    /// (it blocked the bot, left a group, or is rate limited) does not keep
    /// the others from getting it.
    fn send_all(&mut self, reply: &Reply) -> Result<(), ApiError> {
        for chat in self.cfg.chats.clone() {
            match self.send(chat, reply) {
                Err(ApiError::Other(e)) => debug!(chat, error = %e, "telegram send failed"),
                other => other?,
            }
        }
        Ok(())
    }

    pub fn token(&self) -> &str {
        &self.cfg.token
    }

    /// Fills the command menu and greets the allowed chats.
    pub fn introduce(&mut self) -> Result<(), ApiError> {
        let commands: Vec<Value> = MENU
            .iter()
            .map(|(c, d)| json!({"command": c, "description": d}))
            .collect();
        self.call("setMyCommands", json!({ "commands": commands }))?;
        let hello = Reply::text(format!(
            "✅ RustWTM is connected on <b>{}</b>. Send /help for what I can do.",
            esc(&self.computer)
        ));
        self.send_all(&hello)
    }

    /// One round: forward new alerts, then wait briefly for messages.
    pub fn tick(&mut self) -> Result<(), ApiError> {
        self.catch_up()?;
        self.forward_alerts()?;
        self.report_exits()?;
        let updates = self.api.call(
            "getUpdates",
            json!({
                "offset": self.offset,
                "timeout": POLL_SECS,
                "allowed_updates": ["message", "callback_query"],
            }),
            Duration::from_secs(POLL_SECS + 15),
        )?;
        for update in updates.as_array().into_iter().flatten() {
            if let Some(id) = update["update_id"].as_i64() {
                self.offset = self.offset.max(id + 1);
            }
            match self.handle(update) {
                Err(ApiError::Other(e)) => debug!(error = %e, "telegram reply failed"),
                other => other?,
            }
        }
        let now = Instant::now();
        self.pending
            .retain(|_, p| now.duration_since(p.created) < PENDING_TTL);
        self.told
            .retain(|_, t| now.duration_since(*t) < TELL_ID_EVERY);
        self.swarm_names
            .retain(|_, (_, t)| now.duration_since(*t) < PENDING_TTL);
        Ok(())
    }

    fn call(&mut self, method: &str, body: Value) -> Result<Value, ApiError> {
        self.api.call(method, body, Duration::from_secs(20))
    }

    fn markup(reply: &Reply) -> Value {
        let rows: Vec<Vec<Value>> = reply
            .buttons
            .iter()
            .map(|row| {
                row.iter()
                    .map(|(text, data)| json!({"text": text, "callback_data": data}))
                    .collect()
            })
            .collect();
        json!({ "inline_keyboard": rows })
    }

    fn send(&mut self, chat: i64, reply: &Reply) -> Result<(), ApiError> {
        self.call(
            "sendMessage",
            json!({
                "chat_id": chat,
                "text": reply.text,
                "parse_mode": "HTML",
                "link_preview_options": {"is_disabled": true},
                "reply_markup": Self::markup(reply),
            }),
        )
        .map(drop)
    }

    fn allowed(&self, chat: i64) -> bool {
        self.cfg.chats.contains(&chat)
    }

    fn handle(&mut self, update: &Value) -> Result<(), ApiError> {
        if let Some(q) = update.get("callback_query") {
            return self.handle_press(q);
        }
        let msg = &update["message"];
        let Some(chat) = msg["chat"]["id"].as_i64() else {
            return Ok(());
        };
        let text = msg["text"].as_str().unwrap_or_default();
        if !addressed_to(text, self.username.as_deref()) {
            return Ok(());
        }
        // In a group only commands are for the bot; other talk is not.
        let private = msg["chat"]["type"].as_str().is_none_or(|t| t == "private");
        if !private && parse_command(text).is_none() {
            return Ok(());
        }
        if !self.allowed(chat) {
            return self.tell_id(chat, &msg["from"]);
        }
        let reply = self.command(chat, text);
        self.send(chat, &reply)
    }

    /// Tells a chat that is not allowed how to allow it.
    fn tell_id(&mut self, chat: i64, from: &Value) -> Result<(), ApiError> {
        let now = Instant::now();
        if self
            .told
            .get(&chat)
            .is_some_and(|t| now.duration_since(*t) < TELL_ID_EVERY)
        {
            return Ok(());
        }
        self.told.insert(chat, now);
        let user = from["username"]
            .as_str()
            .map(|u| format!("@{u}"))
            .or_else(|| from["first_name"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "someone".into());
        self.host.note(format!(
            "Telegram: chat {chat} ({user}) asked for access; add the ID in Settings → Telegram to allow it"
        ));
        self.send(
            chat,
            &Reply::text(format!(
                "This chat is not allowed to use this bot.\nIts ID is <code>{chat}</code>: add it in RustWTM → Settings → Telegram to allow it."
            )),
        )
    }

    fn forward_alerts(&mut self) -> Result<(), ApiError> {
        let events = self.host.events_after(self.last_event);
        let Some(last) = events.last() else {
            return Ok(());
        };
        self.last_event = last.id;
        let muted = self.muted_until.is_some_and(|t| Instant::now() < t);
        if !self.cfg.notify || muted || self.cfg.chats.is_empty() {
            return Ok(());
        }
        let alerts: Vec<&Event> = events.iter().filter(|e| e.notify).collect();
        if alerts.is_empty() {
            return Ok(());
        }
        let mut text = format!("🖥 <b>{}</b>\n", esc(&self.computer));
        for e in alerts.iter().take(10) {
            let icon = match e.level {
                EventLevel::Info => "ℹ️",
                EventLevel::Warn => "⚠️",
                EventLevel::Error => "❌",
            };
            let subject = match (&e.name, e.pid) {
                (Some(name), Some(pid)) => format!("{}: ", esc(&who(name, pid))),
                _ => String::new(),
            };
            text.push_str(&format!(
                "{icon} {subject}{}\n",
                esc(&clip(&e.message, 400))
            ));
        }
        if alerts.len() > 10 {
            text.push_str(&format!("…and {} more\n", alerts.len() - 10));
        }
        let reply = Reply::text(text.trim_end());
        self.send_all(&reply)
    }

    /// The answer to a text message.
    fn command(&mut self, chat: i64, text: &str) -> Reply {
        let Some((cmd, arg)) = parse_command(text) else {
            return Reply::text("Send /help for the list of commands.");
        };
        let controls = matches!(
            cmd.as_str(),
            "kill"
                | "end"
                | "tree"
                | "suspend"
                | "resume"
                | "free"
                | "panic"
                | "start_service"
                | "stop_service"
                | "startsvc"
                | "stopsvc"
                | "restartsvc"
        );
        if controls && !self.cfg.control {
            return Reply::text(CONTROL_OFF);
        }
        let Some(snap) = self.host.snapshot() else {
            return Reply::text("RustWTM is still starting; try again in a few seconds.");
        };
        let procs = &snap.processes;
        match cmd.as_str() {
            "start" | "help" => self.help(),
            "status" => self.status(&snap),
            "top" => self.top(procs, count(&arg, 10, 30), false),
            "mem" => self.top(procs, count(&arg, 10, 30), true),
            "find" => self.find(procs, &arg),
            "info" => self.info(procs, &arg),
            "kill" | "end" => self.end_cmd(chat, procs, &arg, false),
            "tree" => self.end_cmd(chat, procs, &arg, true),
            "suspend" => self.pause_cmd(chat, procs, &arg, Command::Suspend),
            "resume" => self.pause_cmd(chat, procs, &arg, Command::Resume),
            "ports" => self.ports(procs),
            "free" => self.free(chat, procs, &arg),
            "services" => self.services(&arg),
            "startsvc" | "start_service" => self.service_cmd(&arg, ServiceOp::Start),
            "stopsvc" | "stop_service" => self.service_cmd(&arg, ServiceOp::Stop),
            "restartsvc" => self.service_cmd(&arg, ServiceOp::Restart),
            "wait" => self.wait_cmd(chat, procs, &arg),
            "unwait" => {
                let before = self.waits.len();
                self.waits.retain(|w| w.chat != chat);
                Reply::text(format!("Stopped {} wait(s).", before - self.waits.len()))
            }
            "swarms" => self.swarms(procs),
            "panic" => self.panic(chat, procs),
            "events" => self.events(count(&arg, 10, 50)),
            "mute" => {
                let minutes = count(&arg, 60, 24 * 60);
                self.muted_until = Some(Instant::now() + Duration::from_secs(minutes as u64 * 60));
                Reply::text(format!(
                    "🔕 Alerts paused for {minutes} min. /unmute to resume."
                ))
            }
            "unmute" => {
                self.muted_until = None;
                Reply::text("🔔 Alerts are on again.")
            }
            _ => Reply::text(format!(
                "Unknown command /{}. Send /help for the list.",
                esc(&cmd)
            )),
        }
    }

    fn help(&self) -> Reply {
        let mut text = format!("<b>RustWTM</b> on <b>{}</b>\n\n", esc(&self.computer));
        for (c, d) in MENU {
            text.push_str(&format!("/{c} – {}\n", esc(d)));
        }
        text.push_str("\nProcesses can be named by PID or by name (chrome or chrome.exe).");
        if !self.cfg.control {
            text.push_str(&format!("\n\n{CONTROL_OFF}"));
        }
        Reply::text(text)
    }

    fn status(&self, snap: &Snapshot) -> Reply {
        let s = &snap.system;
        let used = s.memory_total.saturating_sub(s.memory_available);
        let mem_pct = if s.memory_total > 0 {
            used as f64 * 100.0 / s.memory_total as f64
        } else {
            0.0
        };
        let commit_used = s.commit_limit.saturating_sub(s.commit_available);
        let mut lines = vec![
            format!(
                "CPU      {:.0}% of {} logical processors",
                s.cpu_percent, s.logical_cpus
            ),
            format!(
                "Memory   {} of {} ({mem_pct:.0}%)",
                format_bytes(used as f64),
                format_bytes(s.memory_total as f64)
            ),
            format!(
                "Commit   {} of {}",
                format_bytes(commit_used as f64),
                format_bytes(s.commit_limit as f64)
            ),
            format!("GPU      {:.0}%", s.gpu_percent),
            format!(
                "Network  ↓ {}  ↑ {}",
                rate(s.net_rx_rate),
                rate(s.net_tx_rate)
            ),
            format!(
                "Disk I/O read {}  write {}",
                rate(s.io_read_rate),
                rate(s.io_write_rate)
            ),
        ];
        for d in &s.disks {
            lines.push(format!(
                "Disk {:<4} {:.0}% active",
                d.name, d.active_percent
            ));
        }
        lines.push(format!(
            "{} processes · {} threads · {} handles",
            s.process_count, s.thread_count, s.handle_count
        ));
        let status = self.host.status();
        let governor = if !status.governor_enabled {
            "off".to_owned()
        } else if status.dry_run {
            "on, dry run (only logs)".to_owned()
        } else {
            format!("on, holding {} process(es)", status.managed.len())
        };
        lines.push(format!("Governor {governor}"));
        let mut busiest: Vec<&ProcessInfo> = snap.processes.iter().filter(|p| p.pid > 4).collect();
        busiest.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        let top: Vec<String> = busiest
            .iter()
            .take(3)
            .map(|p| format!("{} {:.0}%", p.name, p.cpu_percent))
            .collect();
        lines.push(format!("Busiest  {}", top.join(", ")));
        Reply::text(format!("🖥 <b>{}</b>\n{}", esc(&self.computer), pre(&lines)))
    }

    /// Rows of "End" buttons for the first few processes.
    fn end_buttons(procs: &[&ProcessInfo]) -> Vec<Vec<(String, String)>> {
        procs
            .iter()
            .take(8)
            .map(|p| {
                (
                    format!("✖ {}", clip(&who(&p.name, p.pid), 28)),
                    format!("e:{}:{}", p.pid, p.create_time),
                )
            })
            .collect::<Vec<_>>()
            .chunks(2)
            .map(<[_]>::to_vec)
            .collect()
    }

    fn table(procs: &[&ProcessInfo]) -> Vec<String> {
        let mut lines = vec![format!(
            "{:<22} {:>6} {:>5} {:>9}",
            "Name", "PID", "CPU", "Memory"
        )];
        lines.extend(procs.iter().map(|p| {
            format!(
                "{:<22} {:>6} {:>4.0}% {:>9}",
                clip(&p.name, 22),
                p.pid,
                p.cpu_percent,
                mem(p)
            )
        }));
        lines
    }

    fn top(&self, procs: &[ProcessInfo], n: usize, by_memory: bool) -> Reply {
        let mut list: Vec<&ProcessInfo> = procs.iter().filter(|p| p.pid > 4).collect();
        if by_memory {
            list.sort_by_key(|p| std::cmp::Reverse(p.private_working_set));
        } else {
            list.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        }
        list.truncate(n);
        let title = if by_memory { "Most memory" } else { "Busiest" };
        let mut reply = Reply::text(format!(
            "<b>{title}</b> on {}\n{}",
            esc(&self.computer),
            pre(&Self::table(&list))
        ));
        if self.cfg.control {
            reply.buttons = Self::end_buttons(&list);
        }
        reply
    }

    fn find(&self, procs: &[ProcessInfo], arg: &str) -> Reply {
        if arg.is_empty() {
            return Reply::text("Which processes? e.g. /find chrome");
        }
        let needle = arg.to_lowercase();
        let mut list: Vec<&ProcessInfo> = procs
            .iter()
            .filter(|p| p.pid > 0 && p.name.to_lowercase().contains(&needle))
            .collect();
        if list.is_empty() {
            return Reply::text(format!("No process name contains “{}”.", esc(arg)));
        }
        list.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        let total = list.len();
        let memory: u64 = list.iter().map(|p| p.private_working_set).sum();
        let cpu: f32 = list.iter().map(|p| p.cpu_percent).sum();
        list.truncate(25);
        let mut reply = Reply::text(format!(
            "<b>{total}</b> matching “{}” · {cpu:.0}% CPU · {}\n{}",
            esc(arg),
            format_bytes(memory as f64),
            pre(&Self::table(&list))
        ));
        if self.cfg.control {
            reply.buttons = Self::end_buttons(&list);
        }
        reply
    }

    /// Processes named by PID, or by image name with or without `.exe`.
    fn resolve<'a>(procs: &'a [ProcessInfo], arg: &str) -> Result<Vec<&'a ProcessInfo>, String> {
        if arg.is_empty() {
            return Err("Which process? Give a PID or a name, e.g. 1234 or chrome.".into());
        }
        if let Ok(pid) = arg.parse::<u32>() {
            return procs
                .iter()
                .find(|p| p.pid == pid)
                .map(|p| vec![p])
                .ok_or_else(|| format!("No process has PID {pid}."));
        }
        let with_exe = format!("{arg}.exe");
        let list: Vec<&ProcessInfo> = procs
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case(arg) || p.name.eq_ignore_ascii_case(&with_exe))
            .collect();
        if list.is_empty() {
            Err(format!(
                "No process is named {}. Try /find {}",
                esc(arg),
                esc(arg)
            ))
        } else {
            Ok(list)
        }
    }

    fn info(&self, procs: &[ProcessInfo], arg: &str) -> Reply {
        let list = match Self::resolve(procs, arg) {
            Ok(l) => l,
            Err(e) => return Reply::text(e),
        };
        if list.len() > 1 {
            return self.find(procs, arg);
        }
        let p = list[0];
        let parent = procs
            .iter()
            .find(|q| q.pid == p.parent_pid && q.create_time <= p.create_time)
            .map_or_else(
                || format!("{} (exited)", p.parent_pid),
                |q| who(&q.name, q.pid),
            );
        let mut lines = vec![
            format!("PID      {}", p.pid),
            format!("Parent   {parent}"),
            format!("CPU      {:.1}%", p.cpu_percent),
            format!("Memory   {} (private)", mem(p)),
            format!("Threads  {} · handles {}", p.thread_count, p.handle_count),
            format!(
                "Disk     read {}  write {}",
                rate(p.io_read_rate),
                rate(p.io_write_rate)
            ),
        ];
        if p.gpu_percent > 0.0 {
            lines.push(format!("GPU      {:.0}%", p.gpu_percent));
        }
        if let Some(path) = self.host.image_path(p) {
            lines.push(format!("Path     {path}"));
        }
        if let Some(cmd) = self.host.command_line(p) {
            lines.push(format!("Command  {}", clip(&cmd, 600)));
        }
        let mut reply = Reply::text(format!("<b>{}</b>\n{}", esc(&p.name), pre(&lines)));
        let key = format!("{}:{}", p.pid, p.create_time);
        reply.buttons = vec![vec![(
            "🔔 Tell me when it exits".into(),
            format!("x:{key}"),
        )]];
        if self.cfg.control {
            reply.buttons.extend([
                vec![
                    ("✖ End".into(), format!("e:{key}")),
                    ("🌳 End tree".into(), format!("t:{key}")),
                ],
                vec![
                    ("⏸ Suspend".into(), format!("s:{key}")),
                    ("▶ Resume".into(), format!("r:{key}")),
                ],
            ]);
        }
        reply
    }

    /// Starts watching `target` for `chat`; the reply text.
    fn add_wait(&mut self, chat: i64, target: ProcessRef) -> String {
        if self
            .waits
            .iter()
            .any(|w| w.chat == chat && w.target == target)
        {
            return format!("Already waiting for {}.", who(&target.name, target.pid));
        }
        if self.waits.len() >= MAX_WAITS {
            return "Too many waits; /unwait some first.".into();
        }
        let text = format!(
            "🔔 I will tell you when {} exits.",
            who(&target.name, target.pid)
        );
        self.waits.push(Wait {
            chat,
            target,
            since: Instant::now(),
        });
        text
    }

    fn wait_cmd(&mut self, chat: i64, procs: &[ProcessInfo], arg: &str) -> Reply {
        let list = match Self::resolve(procs, arg) {
            Ok(l) => l,
            Err(e) => return Reply::text(e),
        };
        let lines: Vec<String> = list
            .iter()
            .take(10)
            .map(|p| self.add_wait(chat, ProcessRef::from(*p)))
            .collect();
        Reply::text(esc(&lines.join("\n")))
    }

    /// Tells chats about the processes they wait for that have exited.
    fn report_exits(&mut self) -> Result<(), ApiError> {
        if self.waits.is_empty() {
            return Ok(());
        }
        let Some(snap) = self.host.snapshot() else {
            return Ok(());
        };
        let live: std::collections::HashSet<(u32, i64)> = snap
            .processes
            .iter()
            .map(|p| (p.pid, p.create_time))
            .collect();
        let (gone, kept): (Vec<Wait>, Vec<Wait>) = std::mem::take(&mut self.waits)
            .into_iter()
            .partition(|w| !live.contains(&(w.target.pid, w.target.create_time)));
        self.waits = kept;
        for w in gone {
            let text = format!(
                "✅ <b>{}</b> has exited on {} (waited {}).",
                esc(&who(&w.target.name, w.target.pid)),
                esc(&self.computer),
                wtm_core::format_duration(w.since.elapsed())
            );
            match self.send(w.chat, &Reply::text(text)) {
                Err(ApiError::Other(e)) => {
                    debug!(chat = w.chat, error = %e, "telegram send failed")
                }
                other => other?,
            }
        }
        Ok(())
    }

    /// Asks before ending `procs`; protected ones are left out with a reason.
    fn ask_end(
        &mut self,
        chat: i64,
        procs: &[&ProcessInfo],
        tree: bool,
        notes: Vec<String>,
    ) -> Reply {
        let mut notes = notes;
        let mut targets = Vec::new();
        for p in procs {
            match end_blocked(p.pid, &p.name) {
                Some(reason) => notes.push(format!("Not ended: {}: {reason}", who(&p.name, p.pid))),
                None => targets.push(*p),
            }
        }
        // A reply past Telegram's 4096 characters would not be sent at all.
        let mut notes_text: String = notes
            .iter()
            .take(10)
            .map(|n| format!("\n{}", esc(n)))
            .collect();
        if notes.len() > 10 {
            notes_text.push_str(&format!("\n…and {} more", notes.len() - 10));
        }
        if targets.is_empty() {
            return Reply::text(format!("Nothing to end.{notes_text}"));
        }
        let id = self.next_pending;
        self.next_pending += 1;
        let what = if targets.len() == 1 {
            let p = targets[0];
            format!(
                "End <b>{}</b>? It uses {:.0}% CPU and {}.",
                esc(&who(&p.name, p.pid)),
                p.cpu_percent,
                mem(p)
            )
        } else {
            let memory: u64 = targets.iter().map(|p| p.private_working_set).sum();
            let names: Vec<String> = targets
                .iter()
                .take(8)
                .map(|p| who(&p.name, p.pid))
                .collect();
            let more = if targets.len() > 8 {
                format!(" and {} more", targets.len() - 8)
            } else {
                String::new()
            };
            format!(
                "End these <b>{}</b> processes ({})? {}{more}",
                targets.len(),
                format_bytes(memory as f64),
                esc(&names.join(", "))
            )
        };
        let tree_text = if tree {
            "\nWhat they started is ended too."
        } else {
            ""
        };
        self.pending.insert(
            id,
            Pending {
                chat,
                targets: targets.iter().map(|p| ProcessRef::from(*p)).collect(),
                suspend: false,
                created: Instant::now(),
            },
        );
        let mut first = vec![("✅ End".to_owned(), format!("y:{id}:{}", u8::from(tree)))];
        if !tree {
            first.push(("🌳 With children".to_owned(), format!("y:{id}:1")));
        }
        Reply {
            text: format!("{what}{tree_text}{notes_text}"),
            buttons: vec![first, vec![("Cancel".into(), format!("n:{id}"))]],
        }
    }

    fn end_cmd(&mut self, chat: i64, procs: &[ProcessInfo], arg: &str, tree: bool) -> Reply {
        match Self::resolve(procs, arg) {
            Ok(list) => self.ask_end(chat, &list, tree, Vec::new()),
            Err(e) => Reply::text(e),
        }
    }

    /// Ends the confirmed processes.
    fn end(&mut self, chat: i64, targets: &[ProcessRef], tree: bool) -> String {
        let command = if tree {
            Command::KillTree
        } else {
            Command::Kill
        };
        let mut ended = Vec::new();
        let mut failed = Vec::new();
        for t in targets {
            for o in self.host.control(t, command) {
                match o.result {
                    Ok(()) => ended.push(who(&o.target.name, o.target.pid)),
                    Err(e) => failed.push(format!("{}: {e}", who(&o.target.name, o.target.pid))),
                }
            }
        }
        self.host.note(format!(
            "Telegram: chat {chat} ended {} process(es){}",
            ended.len(),
            if failed.is_empty() {
                String::new()
            } else {
                format!(", {} failed", failed.len())
            }
        ));
        let mut text = match ended.len() {
            0 => "Nothing was ended.".to_owned(),
            1 => format!("✅ Ended {}.", esc(&ended[0])),
            n => format!(
                "✅ Ended {n} processes: {}.",
                esc(&clip(&ended.join(", "), 1500))
            ),
        };
        for f in failed.iter().take(10) {
            text.push_str(&format!("\n❌ {}", esc(f)));
        }
        text
    }

    fn pause(&mut self, target: &ProcessRef, command: Command) -> String {
        let verb = if command == Command::Suspend {
            "Suspended"
        } else {
            "Resumed"
        };
        let outcomes = self.host.control(target, command);
        match outcomes.first().map(|o| &o.result) {
            Some(Ok(())) => format!("{verb} {}", who(&target.name, target.pid)),
            Some(Err(e)) => format!("Failed: {e}"),
            None => "Failed".into(),
        }
    }

    /// Suspends or resumes what `arg` names. Suspending several processes is
    /// asked first, like ending them.
    fn pause_cmd(
        &mut self,
        chat: i64,
        procs: &[ProcessInfo],
        arg: &str,
        command: Command,
    ) -> Reply {
        let list = match Self::resolve(procs, arg) {
            Ok(l) => l,
            Err(e) => return Reply::text(e),
        };
        let mut notes = Vec::new();
        let mut targets = Vec::new();
        for p in &list {
            match pause_blocked(p, command) {
                Some(reason) => notes.push(format!("{}: not done: {reason}", who(&p.name, p.pid))),
                None => targets.push(*p),
            }
        }
        let mut notes_text: String = notes
            .iter()
            .take(10)
            .map(|n| format!("\n{}", esc(n)))
            .collect();
        if notes.len() > 10 {
            notes_text.push_str(&format!("\n…and {} more", notes.len() - 10));
        }
        if command == Command::Suspend && targets.len() > 1 {
            let id = self.next_pending;
            self.next_pending += 1;
            let names: Vec<String> = targets
                .iter()
                .take(8)
                .map(|p| who(&p.name, p.pid))
                .collect();
            let more = if targets.len() > 8 {
                format!(" and {} more", targets.len() - 8)
            } else {
                String::new()
            };
            let text = format!(
                "Suspend these <b>{}</b> processes? {}{more}{notes_text}",
                targets.len(),
                esc(&names.join(", "))
            );
            self.pending.insert(
                id,
                Pending {
                    chat,
                    targets: targets.iter().map(|p| ProcessRef::from(*p)).collect(),
                    suspend: true,
                    created: Instant::now(),
                },
            );
            return Reply {
                text,
                buttons: vec![vec![
                    ("⏸ Suspend".to_owned(), format!("y:{id}:s")),
                    ("Cancel".to_owned(), format!("n:{id}")),
                ]],
            };
        }
        let lines: Vec<String> = targets
            .iter()
            .take(50)
            .map(|p| esc(&self.pause(&ProcessRef::from(*p), command)))
            .collect();
        Reply::text(
            format!("{}{notes_text}", lines.join("\n"))
                .trim()
                .to_owned(),
        )
    }

    fn ports(&self, procs: &[ProcessInfo]) -> Reply {
        let uses = match self.host.port_uses() {
            Ok(u) => u,
            Err(e) => return Reply::text(format!("Cannot read ports: {}", esc(&e))),
        };
        let name = |pid: u32| {
            procs
                .iter()
                .find(|p| p.pid == pid)
                .map_or_else(|| "?".to_owned(), |p| p.name.clone())
        };
        let tcp: Vec<&PortUse> = uses
            .iter()
            .filter(|u| u.protocol == wtm_core::Protocol::Tcp)
            .collect();
        let mut lines = vec![format!(
            "{:>5} {:<16} {:<18} {:>6} {}",
            "Port", "Known as", "Program", "PID", "Reach"
        )];
        lines.extend(tcp.iter().map(|u| {
            format!(
                "{:>5} {:<16} {:<18} {:>6} {}",
                u.port,
                clip(u.known().map_or("", |k| k.service), 16),
                clip(&name(u.pid), 18),
                u.pid,
                u.exposure().label()
            )
        }));
        Reply::text(format!(
            "<b>{}</b> listening TCP ports on {}\n{}\n/free PORT ends what uses one.",
            tcp.len(),
            esc(&self.computer),
            pre(&lines)
        ))
    }

    fn free(&mut self, chat: i64, procs: &[ProcessInfo], arg: &str) -> Reply {
        let Some(port) = wtm_core::ports::parse_port(arg) else {
            return Reply::text("Which port? e.g. /free 3000");
        };
        let uses = match self.host.port_uses() {
            Ok(u) => u,
            Err(e) => return Reply::text(format!("Cannot read ports: {}", esc(&e))),
        };
        let owners = wtm_core::ports::port_owners(&uses, port);
        if owners.is_empty() {
            return Reply::text(format!("Nothing uses port {port}; it is free."));
        }
        let list: Vec<&ProcessInfo> = procs.iter().filter(|p| owners.contains(&p.pid)).collect();
        let notes: Vec<String> = list
            .iter()
            .filter_map(|p| relay_hint(&p.name).map(|h| format!("{}: {h}", p.name)))
            .collect();
        self.ask_end(chat, &list, false, notes)
    }

    fn services(&self, arg: &str) -> Reply {
        let all = match self.host.services() {
            Ok(s) => s,
            Err(e) => return Reply::text(format!("Cannot list services: {}", esc(&e))),
        };
        let needle = arg.to_lowercase();
        let mut list: Vec<&ServiceEntry> = all
            .iter()
            .filter(|s| {
                needle.is_empty() && s.state == ServiceState::Running
                    || !needle.is_empty()
                        && (s.name.to_lowercase().contains(&needle)
                            || s.display_name.to_lowercase().contains(&needle))
            })
            .collect();
        list.sort_by_key(|s| s.name.to_lowercase());
        if list.is_empty() {
            return Reply::text(format!("No service matches “{}”.", esc(arg)));
        }
        let total = list.len();
        list.truncate(40);
        let lines: Vec<String> = list
            .iter()
            .map(|s| {
                format!(
                    "{:<8} {:<24} {}",
                    format!("{:?}", s.state),
                    clip(&s.name, 24),
                    clip(&s.display_name, 34)
                )
            })
            .collect();
        let title = if needle.is_empty() {
            format!("<b>{total}</b> running services")
        } else {
            format!("<b>{total}</b> services matching “{}”", esc(arg))
        };
        Reply::text(format!(
            "{title}\n{}\n/startsvc NAME · /stopsvc NAME · /restartsvc NAME",
            pre(&lines)
        ))
    }

    fn service_cmd(&mut self, name: &str, op: ServiceOp) -> Reply {
        if name.is_empty() {
            return Reply::text("Which service? Use its short name from /services.");
        }
        if op != ServiceOp::Start && name.eq_ignore_ascii_case(wtm_ipc::SERVICE_NAME) {
            return Reply::text(
                "Stopping RustWTM itself would also stop this bot; do it on the PC.",
            );
        }
        let verb = match op {
            ServiceOp::Start => "Started",
            ServiceOp::Stop => "Stopped",
            ServiceOp::Restart => "Restarted",
        };
        match self.host.set_service(name, op) {
            Ok(()) => {
                self.host
                    .note(format!("Telegram: {} service {name}", verb.to_lowercase()));
                Reply::text(format!("✅ {verb} {}.", esc(name)))
            }
            Err(e) => Reply::text(format!("❌ {}: {}", esc(name), esc(&e))),
        }
    }

    fn swarms(&mut self, procs: &[ProcessInfo]) -> Reply {
        let cfg = self.host.config().swarm;
        let swarms = wtm_policy::find_swarms(procs, &cfg);
        if swarms.is_empty() {
            return Reply::text(format!("No program runs {} or more copies.", cfg.min_count));
        }
        let mut text = String::new();
        let mut buttons = Vec::new();
        for s in swarms.iter().take(5) {
            text.push_str(&format!(
                "⚠️ <b>{}</b> copies of <b>{}</b> · {:.0}% CPU · {}\nStarted by {}\n",
                s.members.len(),
                esc(&s.name),
                s.cpu_percent,
                format_bytes(s.memory as f64),
                esc(&s.parents_text())
            ));
            if self.cfg.control {
                let id = self.next_pending;
                self.next_pending += 1;
                self.swarm_names
                    .insert(id, (s.name.clone(), Instant::now()));
                buttons.push(vec![(
                    format!("✖ End {} × {}", s.members.len(), clip(&s.name, 20)),
                    format!("w:{id}"),
                )]);
                if s.orphans() > 0 && s.orphans() < s.members.len() {
                    buttons.push(vec![(
                        format!("✖ End the {} whose parent exited", s.orphans()),
                        format!("o:{id}"),
                    )]);
                }
            }
        }
        Reply {
            text: text.trim_end().to_owned(),
            buttons,
        }
    }

    fn panic(&mut self, chat: i64, procs: &[ProcessInfo]) -> Reply {
        let config = self.host.config();
        if config.kill.is_empty() {
            return Reply::text("The kill list is empty; add programs in RustWTM → Kill list.");
        }
        let host = &self.host;
        let targets = wtm_policy::panic_targets(&config.kill, procs, &mut |p| host.command_line(p));
        if targets.is_empty() {
            return Reply::text("Nothing on the kill list is running.");
        }
        let list: Vec<&ProcessInfo> = targets.iter().map(|t| t.process).collect();
        self.ask_end(chat, &list, false, Vec::new())
    }

    fn events(&self, n: usize) -> Reply {
        let events = self.host.events_after(0);
        if events.is_empty() {
            return Reply::text("No events yet.");
        }
        let lines: Vec<String> = events
            .iter()
            .rev()
            .take(n)
            .rev()
            .map(|e| {
                let subject = match (&e.name, e.pid) {
                    (Some(name), Some(pid)) => format!("{}: ", who(name, pid)),
                    _ => String::new(),
                };
                format!("{:>7} {subject}{}", ago(e.unix_ms), clip(&e.message, 200))
            })
            .collect();
        Reply::text(pre(&lines))
    }

    fn handle_press(&mut self, q: &Value) -> Result<(), ApiError> {
        let id = q["id"].as_str().unwrap_or_default().to_owned();
        let chat = q["message"]["chat"]["id"].as_i64().unwrap_or_default();
        let message_id = q["message"]["message_id"].as_i64();
        let data = q["data"].as_str().unwrap_or_default();
        let pressed = if self.allowed(chat) {
            self.press(chat, data)
        } else {
            Pressed::Toast("This chat is not allowed.".into())
        };
        let toast = match &pressed {
            Pressed::Toast(t) => t.clone(),
            _ => String::new(),
        };
        self.call(
            "answerCallbackQuery",
            json!({"callback_query_id": id, "text": clip(&toast, 190)}),
        )?;
        match pressed {
            Pressed::Edit(reply) => match message_id {
                Some(message_id) => self
                    .call(
                        "editMessageText",
                        json!({
                            "chat_id": chat,
                            "message_id": message_id,
                            "text": reply.text,
                            "parse_mode": "HTML",
                            "reply_markup": Self::markup(&reply),
                        }),
                    )
                    .map(drop),
                None => self.send(chat, &reply),
            },
            Pressed::Send(reply) => self.send(chat, &reply),
            Pressed::Toast(_) => Ok(()),
        }
    }

    /// What a button with callback `data` does.
    fn press(&mut self, chat: i64, data: &str) -> Pressed {
        let (kind, rest) = data.split_once(':').unwrap_or((data, ""));
        if kind == "n" {
            if let Ok(id) = rest.parse::<u64>()
                && self.pending.get(&id).is_some_and(|p| p.chat == chat)
            {
                self.pending.remove(&id);
            }
            return Pressed::Edit(Reply::text("Cancelled; nothing was ended."));
        }
        if kind == "x" {
            let mut parts = rest.split(':');
            let pid = parts.next().and_then(|v| v.parse::<u32>().ok());
            let created = parts.next().and_then(|v| v.parse::<i64>().ok());
            let found = self.host.snapshot().and_then(|s| {
                s.processes
                    .iter()
                    .find(|p| Some(p.pid) == pid && Some(p.create_time) == created)
                    .map(ProcessRef::from)
            });
            return Pressed::Toast(match found {
                Some(t) => self.add_wait(chat, t),
                None => "That process has already exited.".into(),
            });
        }
        if !self.cfg.control {
            return Pressed::Toast(CONTROL_OFF.into());
        }
        if kind == "y" {
            let (id, tree) = rest.split_once(':').unwrap_or((rest, "0"));
            let pending = id
                .parse::<u64>()
                .ok()
                .and_then(|id| self.pending.remove(&id))
                .filter(|p| p.chat == chat);
            return match pending {
                Some(p) if p.suspend => {
                    let lines: Vec<String> = p
                        .targets
                        .iter()
                        .map(|t| self.pause(t, Command::Suspend))
                        .collect();
                    Pressed::Edit(Reply::text(esc(&lines.join("\n"))))
                }
                Some(p) => Pressed::Edit(Reply::text(self.end(chat, &p.targets, tree == "1"))),
                None => Pressed::Edit(Reply::text(
                    "This question expired; nothing was ended. Ask again.",
                )),
            };
        }
        let Some(snap) = self.host.snapshot() else {
            return Pressed::Toast("RustWTM is still starting.".into());
        };
        let procs = &snap.processes;
        match kind {
            // Swarm: every copy, or only those whose parent exited.
            "w" | "o" => {
                let cfg = self.host.config().swarm;
                let swarms = wtm_policy::find_swarms(procs, &cfg);
                let name = rest
                    .parse::<u64>()
                    .ok()
                    .and_then(|id| self.swarm_names.get(&id))
                    .map(|(name, _)| name.clone());
                let Some(s) = swarms.iter().find(|s| Some(&s.name) == name.as_ref()) else {
                    return Pressed::Toast("That swarm is gone.".into());
                };
                let by_pid: HashMap<u32, &ProcessInfo> = procs.iter().map(|p| (p.pid, p)).collect();
                let members: Vec<&ProcessInfo> = s
                    .members
                    .iter()
                    .copied()
                    .filter(|p| kind == "w" || wtm_policy::live_parent(p, &by_pid).is_none())
                    .collect();
                Pressed::Send(self.ask_end(chat, &members, false, Vec::new()))
            }
            _ => {
                let mut parts = rest.split(':');
                let pid = parts.next().and_then(|v| v.parse::<u32>().ok());
                let created = parts.next().and_then(|v| v.parse::<i64>().ok());
                let Some(p) = procs
                    .iter()
                    .find(|p| Some(p.pid) == pid && Some(p.create_time) == created)
                else {
                    return Pressed::Toast("That process has already exited.".into());
                };
                match kind {
                    "e" => Pressed::Send(self.ask_end(chat, &[p], false, Vec::new())),
                    "t" => Pressed::Send(self.ask_end(chat, &[p], true, Vec::new())),
                    "s" | "r" => {
                        let command = if kind == "s" {
                            Command::Suspend
                        } else {
                            Command::Resume
                        };
                        if let Some(reason) = pause_blocked(p, command) {
                            return Pressed::Toast(format!("Not suspended: {reason}"));
                        }
                        Pressed::Toast(self.pause(&ProcessRef::from(p), command))
                    }
                    _ => Pressed::Toast("Unknown button.".into()),
                }
            }
        }
    }
}

fn computer_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "this PC".into())
}

fn describe(e: ApiError) -> String {
    match e {
        ApiError::Unauthorized => "Telegram rejected the bot token".into(),
        ApiError::Conflict => "another program is reading this bot's messages".into(),
        ApiError::Other(e) => e,
    }
}

/// Checks the token and sends a test message to every allowed chat.
pub fn test(cfg: &TelegramConfig) -> Result<String, String> {
    if !wtm_policy::token_looks_valid(&cfg.token) {
        return Err("no valid bot token is saved".into());
    }
    test_with(&mut HttpApi::new(&cfg.token), cfg, &computer_name())
}

fn test_with(api: &mut impl Api, cfg: &TelegramConfig, computer: &str) -> Result<String, String> {
    let timeout = Duration::from_secs(20);
    let me = api.call("getMe", json!({}), timeout).map_err(describe)?;
    let bot = format!("@{}", me["username"].as_str().unwrap_or("bot"));
    if cfg.chats.is_empty() {
        return Ok(format!(
            "The token works ({bot}). Send the bot a message: it answers with your chat ID to add here."
        ));
    }
    let mut failed = Vec::new();
    for chat in &cfg.chats {
        let body = json!({
            "chat_id": chat,
            "text": format!("🔔 Test message from RustWTM on {computer}."),
        });
        if let Err(e) = api.call("sendMessage", body, timeout) {
            let e = describe(e);
            let hint = if e.contains("chat not found") {
                " (send the bot a message first)"
            } else {
                ""
            };
            failed.push(format!("chat {chat}: {e}{hint}"));
        }
    }
    if failed.is_empty() {
        Ok(format!(
            "{bot} sent a test message to {} chat(s).",
            cfg.chats.len()
        ))
    } else {
        Err(failed.join("; "))
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Runs the bot while `telegram.toml` turns it on, until `stop` is set.
pub fn spawn(
    path: PathBuf,
    shared: Arc<Shared>,
    engine: Sender<EngineMsg>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("telegram".into())
        .spawn(move || run(&path, &shared, &engine, &stop))
}

fn run(path: &Path, shared: &Arc<Shared>, engine: &Sender<EngineMsg>, stop: &AtomicBool) {
    let computer = computer_name();
    let events = &shared.events;
    let mut seen: Option<Option<SystemTime>> = None;
    let mut bot: Option<Bot<HttpApi, ServiceHost>> = None;
    let mut backoff = Duration::from_secs(5);
    let mut conflict_told = false;
    while !stop.load(Ordering::SeqCst) {
        let mtime = modified(path);
        if seen != Some(mtime) {
            seen = Some(mtime);
            if mtime.is_some() {
                // The token is a secret: only SYSTEM and administrators.
                if let Err(e) = wtm_sys::security::set_dacl(path, wtm_policy::TELEGRAM_SDDL) {
                    warn!(error = %e, "could not restrict telegram.toml");
                }
                seen = Some(modified(path));
            }
            match TelegramConfig::load(path) {
                Ok(cfg) if cfg.enabled => {
                    let chats = cfg.chats.len();
                    match bot.as_mut().filter(|b| b.token() == cfg.token) {
                        Some(b) => b.set_config(cfg),
                        None => {
                            let host = ServiceHost {
                                shared: shared.clone(),
                                engine: engine.clone(),
                            };
                            let mut b =
                                Bot::new(HttpApi::new(&cfg.token), host, cfg, computer.clone());
                            if let Err(e) = b.introduce() {
                                debug!(error = ?e, "telegram introduction failed");
                            }
                            bot = Some(b);
                            conflict_told = false;
                        }
                    }
                    events.info(None, format!("Telegram bot on ({chats} allowed chat(s))"));
                }
                Ok(_) => {
                    if bot.take().is_some() {
                        events.info(None, "Telegram bot off");
                    }
                }
                Err(e) => {
                    bot = None;
                    events.warn(None, format!("Telegram settings invalid, bot off: {e}"));
                }
            }
        }
        let Some(b) = bot.as_mut() else {
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        match b.tick() {
            Ok(()) => backoff = Duration::from_secs(5),
            Err(ApiError::Unauthorized) => {
                events.warn(
                    None,
                    "Telegram rejected the bot token; check it in Settings → Telegram",
                );
                bot = None;
            }
            Err(ApiError::Conflict) => {
                if !conflict_told {
                    conflict_told = true;
                    events.warn(
                        None,
                        "Another program is reading this Telegram bot's messages; give RustWTM its own bot",
                    );
                }
                std::thread::sleep(Duration::from_secs(10));
            }
            Err(ApiError::Other(e)) => {
                debug!(error = %e, "telegram unreachable");
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// Every API call: method and body.
    type Calls = Rc<RefCell<Vec<(String, Value)>>>;

    #[derive(Default)]
    struct FakeApi {
        calls: Calls,
        updates: VecDeque<Value>,
        /// What waited before the bot started.
        backlog: Vec<Value>,
        /// Chats whose messages fail.
        unreachable: Vec<i64>,
    }

    impl Api for FakeApi {
        fn call(&mut self, method: &str, body: Value, _: Duration) -> Result<Value, ApiError> {
            self.calls
                .borrow_mut()
                .push((method.to_owned(), body.clone()));
            match method {
                "getMe" => Ok(json!({"username": "RustWtmBot"})),
                "getUpdates" if body["offset"] == -1 => Ok(Value::Array(
                    self.backlog.last().cloned().into_iter().collect(),
                )),
                "getUpdates" => Ok(Value::Array(self.updates.drain(..).collect())),
                "sendMessage"
                    if self
                        .unreachable
                        .contains(&body["chat_id"].as_i64().unwrap()) =>
                {
                    Err(ApiError::Other(
                        "Forbidden: bot was blocked by the user".into(),
                    ))
                }
                _ => Ok(json!(true)),
            }
        }
    }

    #[derive(Default)]
    struct FakeHost {
        procs: Vec<ProcessInfo>,
        config: Config,
        events: RefCell<Vec<Event>>,
        controls: RefCell<Vec<(u32, Command)>>,
    }

    impl Host for FakeHost {
        fn snapshot(&self) -> Option<Arc<Snapshot>> {
            Some(Arc::new(Snapshot {
                interval: None,
                system: wtm_core::SystemStats::default(),
                processes: self.procs.clone(),
                net_traced: false,
                taken_at_ms: 0,
            }))
        }
        fn status(&self) -> Status {
            Status::default()
        }
        fn config(&self) -> Config {
            self.config.clone()
        }
        fn events_after(&self, id: u64) -> Vec<Event> {
            self.events
                .borrow()
                .iter()
                .filter(|e| e.id > id)
                .cloned()
                .collect()
        }
        fn control(&self, target: &ProcessRef, command: Command) -> Vec<Outcome> {
            self.controls.borrow_mut().push((target.pid, command));
            vec![Outcome {
                target: target.clone(),
                result: if target.pid == 666 {
                    Err("access denied".into())
                } else {
                    Ok(())
                },
            }]
        }
        fn command_line(&self, p: &ProcessInfo) -> Option<String> {
            Some(format!("{} --flag", p.name))
        }
        fn image_path(&self, _: &ProcessInfo) -> Option<String> {
            None
        }
        fn port_uses(&self) -> Result<Vec<PortUse>, String> {
            Ok(vec![PortUse {
                protocol: wtm_core::Protocol::Tcp,
                port: 3000,
                addresses: vec!["127.0.0.1".parse().unwrap()],
                pid: 200,
                connections: 0,
            }])
        }
        fn services(&self) -> Result<Vec<ServiceEntry>, String> {
            Ok(vec![ServiceEntry {
                name: "MSSQLSERVER".into(),
                display_name: "SQL Server".into(),
                state: ServiceState::Running,
                pid: 1,
                start_type: None,
                binary_path: None,
                account: None,
            }])
        }
        fn set_service(&self, name: &str, _: ServiceOp) -> Result<(), String> {
            if name == "Nope" {
                Err("no such service".into())
            } else {
                Ok(())
            }
        }
        fn note(&self, message: String) {
            let mut events = self.events.borrow_mut();
            let id = events.len() as u64 + 1;
            events.push(event(id, &message, false));
        }
    }

    fn event(id: u64, message: &str, notify: bool) -> Event {
        Event {
            id,
            unix_ms: 0,
            level: EventLevel::Warn,
            pid: None,
            name: None,
            message: message.into(),
            notify,
        }
    }

    fn proc(pid: u32, name: &str, cpu: f32) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: 1,
            name: name.into(),
            cpu_percent: cpu,
            cpu_time: Duration::ZERO,
            working_set: 0,
            private_working_set: 100 << 20,
            private_bytes: 0,
            thread_count: 1,
            handle_count: 1,
            session_id: 1,
            base_priority: 8,
            gpu_percent: 0.0,
            gpu_memory: 0,
            io_read_rate: 0.0,
            io_write_rate: 0.0,
            net_rx_rate: 0.0,
            net_tx_rate: 0.0,
            suspended: false,
            create_time: i64::from(pid) * 10,
        }
    }

    const ME: i64 = 42;

    fn bot(control: bool) -> (Bot<FakeApi, FakeHost>, Calls) {
        let api = FakeApi::default();
        let calls = api.calls.clone();
        let host = FakeHost {
            procs: vec![
                proc(4, "System", 1.0),
                proc(100, "chrome.exe", 30.0),
                proc(101, "chrome.exe", 5.0),
                proc(200, "node.exe", 50.0),
                proc(300, "csrss.exe", 0.0),
                proc(666, "stubborn.exe", 1.0),
            ],
            ..FakeHost::default()
        };
        host.events.borrow_mut().push(event(1, "old alert", true));
        let cfg = TelegramConfig {
            enabled: true,
            token: "1:x".into(),
            chats: vec![ME],
            notify: true,
            control,
        };
        (Bot::new(api, host, cfg, "DESK".into()), calls)
    }

    fn message(update: i64, chat: i64, text: &str) -> Value {
        json!({"update_id": update, "message": {"chat": {"id": chat}, "from": {"username": "ersin"}, "text": text}})
    }

    fn press(update: i64, chat: i64, data: &str) -> Value {
        json!({"update_id": update, "callback_query": {"id": "q", "data": data, "message": {"message_id": 7, "chat": {"id": chat}}}})
    }

    /// Texts sent or edited, in order.
    fn texts(calls: &RefCell<Vec<(String, Value)>>) -> Vec<String> {
        calls
            .borrow()
            .iter()
            .filter(|(m, _)| m == "sendMessage" || m == "editMessageText")
            .map(|(_, b)| b["text"].as_str().unwrap().to_owned())
            .collect()
    }

    fn last_buttons(calls: &RefCell<Vec<(String, Value)>>) -> Vec<String> {
        let calls = calls.borrow();
        let (_, body) = calls
            .iter()
            .rev()
            .find(|(m, _)| m == "sendMessage")
            .unwrap();
        body["reply_markup"]["inline_keyboard"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|row| row.as_array().unwrap().iter())
            .map(|b| b["callback_data"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn what_waited_before_the_start_is_never_run() {
        let (mut b, calls) = bot(true);
        b.api.backlog = vec![
            message(7, ME, "/kill chrome"),
            message(8, ME, "/suspend node"),
        ];
        b.tick().unwrap();
        assert!(
            texts(&calls)
                .iter()
                .all(|t| !t.contains("chrome") && !t.contains("node"))
        );
        // Polling goes on after the last waiting update.
        let polls: Vec<Value> = calls
            .borrow()
            .iter()
            .filter(|(m, b)| m == "getUpdates" && b["offset"] != -1)
            .map(|(_, b)| b["offset"].clone())
            .collect();
        assert_eq!(polls, [json!(9)]);
    }

    #[test]
    fn commands_for_other_bots_and_group_chatter_are_ignored() {
        let (mut b, calls) = bot(true);
        b.api
            .updates
            .push_back(message(1, ME, "/kill@OtherBot chrome"));
        b.api.updates.push_back(json!({"update_id": 2, "message": {
            "chat": {"id": ME, "type": "group"}, "from": {}, "text": "lunch?"}}));
        b.tick().unwrap();
        assert!(texts(&calls).is_empty(), "{:?}", texts(&calls));
        b.api.updates.push_back(message(3, ME, "/top@rustwtmbot"));
        b.tick().unwrap();
        assert_eq!(texts(&calls).len(), 1, "its own name, any case");
        assert!(addressed_to("/top", None) && !addressed_to("/top@x", None));
    }

    #[test]
    fn an_unreachable_chat_does_not_silence_the_others() {
        let (mut b, calls) = bot(true);
        b.cfg.chats = vec![7, ME];
        b.api.unreachable = vec![7];
        b.host.events.borrow_mut().push(event(2, "disk full", true));
        b.tick().unwrap();
        let sent = calls.borrow();
        assert!(
            sent.iter()
                .any(|(m, b)| m == "sendMessage" && b["chat_id"] == ME)
        );
        assert!(
            sent.iter()
                .any(|(m, b)| m == "getUpdates" && b["offset"] != -1),
            "still polls"
        );
    }

    #[test]
    fn suspending_several_is_asked_first_and_svchost_never() {
        let (mut b, calls) = bot(true);
        b.host.procs.push(proc(500, "svchost.exe", 1.0));
        b.api.updates.push_back(message(1, ME, "/suspend svchost"));
        b.tick().unwrap();
        assert!(b.host.controls.borrow().is_empty());
        assert!(
            texts(&calls).last().unwrap().contains("not done"),
            "{:?}",
            texts(&calls)
        );

        b.api.updates.push_back(message(2, ME, "/suspend chrome"));
        b.tick().unwrap();
        assert!(
            b.host.controls.borrow().is_empty(),
            "two copies: asked first"
        );
        let (id, _) = b
            .pending
            .iter()
            .next()
            .map(|(id, p)| (*id, p.suspend))
            .unwrap();
        b.api.updates.push_back(press(3, ME, &format!("y:{id}:s")));
        b.tick().unwrap();
        let done: Vec<(u32, Command)> = b.host.controls.borrow().clone();
        assert_eq!(done, [(100, Command::Suspend), (101, Command::Suspend)]);
    }

    #[test]
    fn swarm_buttons_fit_whatever_the_name() {
        let (mut b, calls) = bot(true);
        let long = format!("{}.exe", "x".repeat(80));
        b.host.config.swarm.min_count = 10;
        b.host
            .procs
            .extend((0..12).map(|i| proc(1000 + i, &long, 1.0)));
        b.api.updates.push_back(message(1, ME, "/swarms"));
        b.tick().unwrap();
        let data = last_buttons(&calls);
        assert!(!data.is_empty());
        assert!(data.iter().all(|d| d.len() <= 64), "{data:?}");
        // The button still finds the swarm.
        b.api.updates.push_back(press(2, ME, &data[0]));
        b.tick().unwrap();
        let last = texts(&calls).last().unwrap().clone();
        assert!(last.contains("End these"), "{last}");
    }

    #[test]
    fn removed_chats_lose_their_waits_and_confirmations() {
        let (mut b, _) = bot(true);
        b.api.updates.push_back(message(1, ME, "/kill chrome"));
        b.tick().unwrap();
        assert!(!b.pending.is_empty());
        let mut cfg = b.cfg.clone();
        cfg.chats = vec![99];
        b.set_config(cfg);
        assert!(b.pending.is_empty());
    }

    #[test]
    fn commands_are_parsed() {
        assert_eq!(
            parse_command("/top@RustWtmBot 15"),
            Some(("top".into(), "15".into()))
        );
        assert_eq!(
            parse_command(" /KILL  chrome "),
            Some(("kill".into(), "chrome".into()))
        );
        assert_eq!(parse_command("hello"), None);
        assert_eq!(count("", 10, 30), 10);
        assert_eq!(count("99", 10, 30), 30);
        assert_eq!(esc("<a&b>"), "&lt;a&amp;b&gt;");
        assert_eq!(clip("abcdef", 4), "abc…");
        assert_eq!(
            api_result(200, br#"{"ok":true,"result":[1]}"#),
            Ok(json!([1]))
        );
        assert_eq!(api_result(401, b""), Err(ApiError::Unauthorized));
        assert_eq!(api_result(409, b""), Err(ApiError::Conflict));
        assert_eq!(
            api_result(400, br#"{"ok":false,"description":"Bad Request"}"#),
            Err(ApiError::Other("Bad Request".into()))
        );
    }

    #[test]
    fn strangers_are_told_their_chat_id_once_and_nothing_else() {
        let (mut b, calls) = bot(true);
        b.api
            .updates
            .extend([message(1, 7, "/kill chrome"), message(2, 7, "/top")]);
        b.tick().unwrap();
        let sent = texts(&calls);
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(sent[0].contains("<code>7</code>"), "{}", sent[0]);
        assert!(b.host.controls.borrow().is_empty());
        assert!(
            b.host
                .events
                .borrow()
                .iter()
                .any(|e| e.message.contains("chat 7 (@ersin)"))
        );
        assert_eq!(b.offset, 3, "updates are acknowledged");
    }

    #[test]
    fn status_top_find_and_info_answer() {
        let (mut b, calls) = bot(true);
        b.api.updates.extend([
            message(1, ME, "/status"),
            message(2, ME, "/top 2"),
            message(3, ME, "/find CHROME"),
            message(4, ME, "/info 200"),
            message(5, ME, "/help"),
            message(6, ME, "/nonsense"),
        ]);
        b.tick().unwrap();
        let sent = texts(&calls);
        assert!(
            sent[0].contains("DESK") && sent[0].contains("Busiest  node.exe 50%"),
            "{}",
            sent[0]
        );
        assert!(
            sent[1].contains("node.exe") && sent[1].contains("chrome.exe"),
            "{}",
            sent[1]
        );
        assert!(!sent[1].contains("stubborn"), "only the top 2: {}", sent[1]);
        assert!(sent[2].contains("<b>2</b> matching"), "{}", sent[2]);
        assert!(sent[3].contains("node.exe --flag"), "{}", sent[3]);
        assert!(sent[4].contains("/kill"), "{}", sent[4]);
        assert!(sent[5].contains("Unknown command"), "{}", sent[5]);
        assert_eq!(
            last_buttons(&calls).len(),
            0,
            "the last reply has no buttons"
        );
    }

    #[test]
    fn ending_by_name_asks_first_and_skips_protected_processes() {
        let (mut b, calls) = bot(true);
        b.api.updates.push_back(message(1, ME, "/kill chrome"));
        b.tick().unwrap();
        let ask = texts(&calls).pop().unwrap();
        assert!(ask.contains("End these <b>2</b> processes"), "{ask}");
        assert!(
            b.host.controls.borrow().is_empty(),
            "nothing ended before confirming"
        );
        let buttons = last_buttons(&calls);
        assert_eq!(buttons, ["y:1:0", "y:1:1", "n:1"]);

        // Another chat cannot confirm it; this one can, once.
        b.api.updates.extend([
            press(2, 99, "y:1:0"),
            press(3, ME, "y:1:0"),
            press(4, ME, "y:1:0"),
        ]);
        b.tick().unwrap();
        assert_eq!(
            *b.host.controls.borrow(),
            [(100, Command::Kill), (101, Command::Kill)]
        );
        let sent = texts(&calls);
        assert!(
            sent[sent.len() - 2].contains("Ended 2 processes"),
            "{sent:?}"
        );
        assert!(sent[sent.len() - 1].contains("expired"), "{sent:?}");

        b.api.updates.push_back(message(5, ME, "/kill csrss"));
        b.tick().unwrap();
        let reply = texts(&calls).pop().unwrap();
        assert!(
            reply.contains("Nothing to end") && reply.contains("protected"),
            "{reply}"
        );
    }

    #[test]
    fn buttons_end_trees_suspend_and_report_failures() {
        let (mut b, calls) = bot(true);
        b.api.updates.extend([
            press(1, ME, "t:200:2000"),
            press(2, ME, "y:1:1"),
            press(3, ME, "s:100:1000"),
            press(4, ME, "s:100:999"),
            press(5, ME, "e:666:6660"),
            press(6, ME, "y:2:0"),
            press(7, ME, "e:100:1000"),
            press(8, ME, "n:3"),
        ]);
        b.tick().unwrap();
        assert_eq!(
            *b.host.controls.borrow(),
            [
                (200, Command::KillTree),
                (100, Command::Suspend),
                (666, Command::Kill)
            ]
        );
        let sent = texts(&calls);
        assert!(
            sent.iter()
                .any(|t| t.contains("What they started is ended too")),
            "{sent:?}"
        );
        assert!(
            sent.iter()
                .any(|t| t.contains("❌ stubborn.exe (666): access denied")),
            "{sent:?}"
        );
        assert!(sent.last().unwrap().contains("Cancelled"), "{sent:?}");
        let toasts: Vec<String> = calls
            .borrow()
            .iter()
            .filter(|(m, _)| m == "answerCallbackQuery")
            .map(|(_, b)| b["text"].as_str().unwrap().to_owned())
            .collect();
        assert!(
            toasts.contains(&"Suspended chrome.exe (100)".to_owned()),
            "{toasts:?}"
        );
        assert!(
            toasts.contains(&"That process has already exited.".to_owned()),
            "{toasts:?}"
        );
    }

    #[test]
    fn read_only_mode_refuses_control() {
        let (mut b, calls) = bot(false);
        b.api.updates.extend([
            message(1, ME, "/kill 100"),
            press(2, ME, "e:100:1000"),
            message(3, ME, "/top"),
        ]);
        b.tick().unwrap();
        assert!(b.host.controls.borrow().is_empty());
        assert!(texts(&calls)[0].contains("turned off"));
        assert!(
            last_buttons(&calls).is_empty(),
            "no End buttons in read-only mode"
        );
    }

    #[test]
    fn ports_free_services_and_panic() {
        let (mut b, calls) = bot(true);
        b.host.config.kill = vec![wtm_policy::KillEntry {
            name: "node.exe".into(),
            ..wtm_policy::KillEntry::default()
        }];
        b.api.updates.extend([
            message(1, ME, "/ports"),
            message(2, ME, "/free 3000"),
            message(3, ME, "/free 9999"),
            message(4, ME, "/services sql"),
            message(5, ME, "/stopsvc RustWTM"),
            message(6, ME, "/startsvc Nope"),
            message(7, ME, "/panic"),
        ]);
        b.tick().unwrap();
        let sent = texts(&calls);
        assert!(
            sent[0].contains(" 3000 ") && sent[0].contains("node.exe"),
            "{}",
            sent[0]
        );
        assert!(sent[1].contains("End <b>node.exe (200)</b>"), "{}", sent[1]);
        assert!(sent[2].contains("it is free"), "{}", sent[2]);
        assert!(sent[3].contains("MSSQLSERVER"), "{}", sent[3]);
        assert!(sent[4].contains("would also stop this bot"), "{}", sent[4]);
        assert!(sent[5].contains("no such service"), "{}", sent[5]);
        assert!(sent[6].contains("End <b>node.exe (200)</b>"), "{}", sent[6]);
    }

    #[test]
    fn waits_report_exits_once_even_in_read_only_mode() {
        let (mut b, calls) = bot(false);
        b.api.updates.extend([
            message(1, ME, "/wait chrome"),
            message(2, ME, "/wait 100"),
            press(3, ME, "x:200:2000"),
            press(4, ME, "x:200:1"),
        ]);
        b.tick().unwrap();
        let sent = texts(&calls);
        assert!(
            sent[0].contains("when chrome.exe (100) exits") && sent[0].contains("(101)"),
            "{}",
            sent[0]
        );
        assert!(sent[1].contains("Already waiting"), "{}", sent[1]);
        assert_eq!(b.waits.len(), 3);

        // chrome.exe 100 and node.exe exit; 101 still runs.
        b.host.procs.retain(|p| p.pid != 100 && p.pid != 200);
        b.tick().unwrap();
        b.tick().unwrap();
        let sent = texts(&calls);
        let exits: Vec<&String> = sent.iter().filter(|t| t.contains("has exited")).collect();
        assert_eq!(exits.len(), 2, "{sent:?}");
        assert!(
            exits[0].contains("chrome.exe (100)") && exits[0].contains("DESK"),
            "{}",
            exits[0]
        );
        assert_eq!(b.waits.len(), 1);

        b.api.updates.extend([
            message(5, ME, "/unwait"),
            message(6, ME, "/restartsvc Spooler"),
        ]);
        b.tick().unwrap();
        let sent = texts(&calls);
        assert!(sent[sent.len() - 2].contains("Stopped 1 wait"), "{sent:?}");
        assert!(
            sent[sent.len() - 1].contains("turned off"),
            "restart is control: {sent:?}"
        );
        assert!(b.waits.is_empty());
    }

    #[test]
    fn services_restart_but_rustwtm_does_not() {
        let (mut b, calls) = bot(true);
        b.api.updates.extend([
            message(1, ME, "/restartsvc Spooler"),
            message(2, ME, "/restartsvc rustwtm"),
            message(3, ME, "/info 100"),
        ]);
        b.tick().unwrap();
        let sent = texts(&calls);
        assert_eq!(sent[0], "✅ Restarted Spooler.");
        assert!(sent[1].contains("would also stop this bot"), "{}", sent[1]);
        assert_eq!(
            last_buttons(&calls)[0],
            "x:100:1000",
            "info offers a wait first"
        );
    }

    #[test]
    fn alerts_are_forwarded_once_and_can_be_muted() {
        let (mut b, calls) = bot(true);
        b.tick().unwrap();
        assert!(
            texts(&calls).is_empty(),
            "alerts from before the bot started stay quiet"
        );
        b.host
            .events
            .borrow_mut()
            .extend([event(2, "CPU at 99%", true), event(3, "routine", false)]);
        b.tick().unwrap();
        b.tick().unwrap();
        let sent = texts(&calls);
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(
            sent[0].contains("DESK") && sent[0].contains("⚠️ CPU at 99%"),
            "{}",
            sent[0]
        );
        assert!(!sent[0].contains("routine"));

        b.api.updates.push_back(message(1, ME, "/mute 5"));
        b.tick().unwrap();
        b.host
            .events
            .borrow_mut()
            .push(event(4, "CPU at 98%", true));
        b.tick().unwrap();
        assert!(!texts(&calls).iter().any(|t| t.contains("98%")), "muted");
        b.api.updates.push_back(message(2, ME, "/unmute"));
        b.tick().unwrap();
        b.host
            .events
            .borrow_mut()
            .push(event(5, "CPU at 97%", true));
        b.tick().unwrap();
        assert!(texts(&calls).last().unwrap().contains("97%"));
    }

    #[test]
    fn the_test_message_checks_the_token_and_every_chat() {
        struct Answers;
        impl Api for Answers {
            fn call(&mut self, method: &str, body: Value, _: Duration) -> Result<Value, ApiError> {
                match (method, body["chat_id"].as_i64()) {
                    ("getMe", _) => Ok(json!({"username": "DeskBot"})),
                    ("sendMessage", Some(1)) => Ok(json!({})),
                    _ => Err(ApiError::Other("Bad Request: chat not found".into())),
                }
            }
        }
        let mut cfg = TelegramConfig {
            token: "1:x".into(),
            ..TelegramConfig::default()
        };
        assert!(
            test_with(&mut Answers, &cfg, "DESK")
                .unwrap()
                .contains("@DeskBot")
        );
        cfg.chats = vec![1];
        assert_eq!(
            test_with(&mut Answers, &cfg, "DESK").unwrap(),
            "@DeskBot sent a test message to 1 chat(s)."
        );
        cfg.chats = vec![1, 2];
        let e = test_with(&mut Answers, &cfg, "DESK").unwrap_err();
        assert!(e.contains("chat 2") && e.contains("message first"), "{e}");
        assert!(test(&TelegramConfig::default()).is_err());
    }

    /// Needs the internet: `cargo test -p wtm-service -- --ignored real_api`.
    #[test]
    #[ignore]
    fn real_api_rejects_a_made_up_token() {
        let mut api = HttpApi::new("123456789:AAFh3kq_Zx-9yLmnopQRstuVWxyz012345");
        let r = api.call("getMe", json!({}), Duration::from_secs(20));
        assert_eq!(r, Err(ApiError::Unauthorized));
    }

    #[test]
    fn introduction_sets_the_menu_and_greets() {
        let (mut b, calls) = bot(true);
        b.introduce().unwrap();
        let calls = calls.borrow();
        assert_eq!(calls[0].0, "setMyCommands");
        assert_eq!(calls[0].1["commands"].as_array().unwrap().len(), MENU.len());
        assert_eq!(calls[1].1["chat_id"], ME);
    }
}

//! Optional hardware provider and a driver-only NVIDIA fallback. Blocking I/O
//! belongs on a dedicated worker, never the GUI or the governor's sample loop.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const STALE_MS: u64 = 10_000;
const MAX_REPLY: usize = 2 * 1024 * 1024;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorKind {
    Temperature,
    Fan,
    Control,
    Power,
    Voltage,
    Clock,
    Load,
}

impl SensorKind {
    pub fn unit(self) -> &'static str {
        match self {
            Self::Temperature => "°C",
            Self::Fan => "RPM",
            Self::Control | Self::Load => "%",
            Self::Power => "W",
            Self::Voltage => "V",
            Self::Clock => "MHz",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sensor {
    pub id: String,
    pub device: String,
    pub name: String,
    pub kind: SensorKind,
    pub value: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FanChannel {
    pub id: String,
    pub device: String,
    pub name: String,
    pub min_percent: f32,
    pub max_percent: f32,
    pub writable: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HardwareReport {
    pub provider: String,
    pub sampled_at_ms: u64,
    pub sensors: Vec<Sensor>,
    pub channels: Vec<FanChannel>,
    pub notice: String,
    #[serde(default)]
    pub cooling: Vec<(String, String)>,
}

impl HardwareReport {
    pub fn fresh(&self, now: u64) -> bool {
        self.sampled_at_ms != 0
            && self.sampled_at_ms <= now.saturating_add(1000)
            && now.saturating_sub(self.sampled_at_ms) <= STALE_MS
    }

    pub fn validate(&mut self) {
        for s in &mut self.sensors {
            if s.value.is_some_and(|v| !v.is_finite()) {
                s.value = None;
            }
        }
        for c in &mut self.channels {
            if !c.min_percent.is_finite()
                || !c.max_percent.is_finite()
                || c.min_percent < 30.0
                || c.max_percent != 100.0
                || c.min_percent > c.max_percent
            {
                c.writable = false;
                c.reason = "Channel must support valid bounds and 100% emergency duty".into();
                c.min_percent = 30.0;
                c.max_percent = 100.0;
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct Reply {
    protocol: u32,
    #[serde(default)]
    request_id: u64,
    #[serde(default)]
    report: Option<HardwareReport>,
    #[serde(default)]
    error: Option<String>,
}

struct Bridge {
    child: Child,
    input: ChildStdin,
    replies: Receiver<Result<Vec<u8>, String>>,
    next_id: u64,
}

impl Bridge {
    fn start(path: PathBuf, writable: bool) -> Result<Self, String> {
        let mut child = Command::new(path)
            .arg(if writable { "--control" } else { "--read-only" })
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let input = child.stdin.take().ok_or("Provider stdin unavailable")?;
        let output = child.stdout.take().ok_or("Provider stdout unavailable")?;
        let (tx, replies) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let mut bytes = Vec::new();
                let result = (&mut reader)
                    .take((MAX_REPLY + 1) as u64)
                    .read_until(b'\n', &mut bytes);
                match result {
                    Ok(0) => break,
                    Ok(_) if bytes.len() <= MAX_REPLY => {
                        if tx.send(Ok(bytes)).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {
                        let _ = tx.send(Err("Hardware provider reply too large".into()));
                        break;
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                        break;
                    }
                }
            }
        });
        Ok(Self {
            child,
            input,
            replies,
            next_id: 0,
        })
    }

    fn call(&mut self, mut request: serde_json::Value, timeout: Duration) -> Result<Reply, String> {
        self.next_id += 1;
        let request_id = self.next_id;
        request["request_id"] = serde_json::json!(request_id);
        serde_json::to_writer(&mut self.input, &request).map_err(|e| e.to_string())?;
        self.input
            .write_all(b"\n")
            .and_then(|()| self.input.flush())
            .map_err(|e| e.to_string())?;
        receive_reply(&self.replies, request_id, timeout)
    }
}

fn receive_reply(
    replies: &Receiver<Result<Vec<u8>, String>>,
    request_id: u64,
    timeout: Duration,
) -> Result<Reply, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let bytes = replies
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|e| format!("Hardware provider: {e}"))??;
        let reply: Reply = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if reply.protocol != 2 {
            return Err("Hardware provider protocol mismatch; rebuild/update the provider".into());
        }
        // A late sample/control reply must never acknowledge a newer
        // request (especially rollback after a timed-out operation).
        if reply.request_id != request_id && !(reply.request_id == 0 && reply.error.is_some()) {
            continue;
        }
        if let Some(e) = reply.error {
            return Err(e);
        }
        return Ok(reply);
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // Normal shutdown requests defaults first. If the provider is stuck,
        // its leases can only help while its watchdog/device API is responsive.
        let _ = self.call(
            serde_json::json!({"op":"restore_all"}),
            Duration::from_secs(2),
        );
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn helper_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.join("hardware/wtm-hardware.exe");
    if path.is_file() {
        return Some(path);
    }
    if cfg!(debug_assertions) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug/hardware/wtm-hardware.exe");
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

pub struct HardwareReader {
    bridge: Option<Bridge>,
    writable: bool,
    attempted: bool,
}

/// Read-only GUI fallback, independent of the regular process-sampling worker.
pub struct LocalHardware {
    report: std::sync::Arc<std::sync::RwLock<HardwareReport>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl LocalHardware {
    pub fn start() -> Self {
        let report = std::sync::Arc::new(std::sync::RwLock::new(HardwareReport::default()));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (published, stopped) = (report.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut reader = HardwareReader::new(false);
            while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
                let result = reader.sample();
                let mut data = published
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match result {
                    Ok(r) => *data = r,
                    Err(e) => data.notice = format!("Sensor collection failed: {e}"),
                }
                drop(data);
                for _ in 0..20 {
                    if stopped.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        });
        Self { report, stop }
    }
    pub fn snapshot(&self) -> HardwareReport {
        self.report
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}
impl Drop for LocalHardware {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl HardwareReader {
    pub fn new(writable: bool) -> Self {
        Self {
            bridge: None,
            writable,
            attempted: false,
        }
    }

    pub fn sample(&mut self) -> Result<HardwareReport, String> {
        if !self.attempted {
            self.attempted = true;
            if let Some(path) = helper_path() {
                self.bridge = Some(Bridge::start(path, self.writable)?);
            }
        }
        if let Some(bridge) = &mut self.bridge {
            let result = bridge.call(serde_json::json!({"op":"sample"}), Duration::from_secs(20));
            match result {
                Ok(reply) => {
                    let mut report = reply.report.ok_or("Provider omitted its report")?;
                    report.validate();
                    return Ok(report);
                }
                Err(e) => {
                    self.bridge = None;
                    return Err(e);
                }
            }
        }
        nvidia_snapshot()
    }

    pub fn set(&mut self, channel: &str, duty: Option<f32>) -> Result<(), String> {
        if !self.writable {
            return Err("This provider is read-only".into());
        }
        if duty.is_some_and(|v| !v.is_finite() || !(30.0..=100.0).contains(&v)) {
            return Err("Fan duty must be between 30 and 100 percent".into());
        }
        let bridge = self
            .bridge
            .as_mut()
            .ok_or("Fan controls need the hardware provider")?;
        bridge.call(
            serde_json::json!({"op":"set","channel":channel,"percent":duty}),
            Duration::from_secs(4),
        )?;
        Ok(())
    }
}

fn nvidia_snapshot() -> Result<HardwareReport, String> {
    let windows = std::env::var_os("WINDIR").ok_or("Windows directory is unavailable")?;
    let exe = PathBuf::from(windows).join("System32/nvidia-smi.exe");
    if !exe.is_file() {
        return Ok(HardwareReport { notice: "Build/install the optional hardware provider for CPU, motherboard, storage and fan sensors. No NVIDIA sensor provider was found.".into(), ..Default::default() });
    }
    let mut child = Command::new(exe).args(["--query-gpu=uuid,name,temperature.gpu,fan.speed,power.draw,clocks.current.graphics,clocks.current.memory", "--format=csv,noheader,nounits"])
        .creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    let output = child
        .stdout
        .take()
        .ok_or("GPU provider stdout unavailable")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let r = output
            .take(MAX_REPLY as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(r);
    });
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            if !status.success() {
                return Err("NVIDIA sensor query failed".into());
            }
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("NVIDIA sensor query timed out".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let bytes = rx
        .recv_timeout(Duration::from_secs(1))
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    parse_nvidia(&String::from_utf8_lossy(&bytes), unix_ms())
}

fn parse_nvidia(text: &str, sampled_at_ms: u64) -> Result<HardwareReport, String> {
    let mut report = HardwareReport { provider: "NVIDIA driver".into(), sampled_at_ms,
        notice: "GPU telemetry is read-only. Add the hardware provider for CPU/motherboard sensors and supported cooling channels.".into(), ..Default::default() };
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if fields.len() != 7 {
            return Err("Unexpected NVIDIA sensor reply".into());
        }
        for (i, name, kind) in [
            (2, "GPU temperature", SensorKind::Temperature),
            (3, "Fan duty (not RPM)", SensorKind::Control),
            (4, "Board power", SensorKind::Power),
            (5, "Graphics clock", SensorKind::Clock),
            (6, "Memory clock", SensorKind::Clock),
        ] {
            let value = fields[i].parse::<f32>().ok().filter(|v| v.is_finite());
            report.sensors.push(Sensor {
                id: format!("nvidia/{}/{i}", fields[0]),
                device: fields[1].into(),
                name: name.into(),
                kind,
                value,
            });
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_replies_are_correlated_and_timeouts_cannot_acknowledge_rollback() {
        let (tx, rx) = mpsc::channel();
        for id in [1, 2] {
            tx.send(Ok(
                format!("{{\"protocol\":2,\"request_id\":{id}}}").into_bytes()
            ))
            .unwrap();
        }
        assert_eq!(
            receive_reply(&rx, 2, Duration::from_millis(10))
                .unwrap()
                .request_id,
            2
        );
        tx.send(Ok(br#"{"protocol":2,"request_id":2}"#.to_vec()))
            .unwrap();
        assert!(receive_reply(&rx, 3, Duration::from_millis(1)).is_err());
        tx.send(Ok(br#"{"protocol":1,"request_id":3}"#.to_vec()))
            .unwrap();
        assert!(
            receive_reply(&rx, 3, Duration::from_millis(10))
                .unwrap_err()
                .contains("protocol mismatch")
        );
        tx.send(Ok(
            br#"{"protocol":2,"request_id":0,"error":"startup failed"}"#.to_vec(),
        ))
        .unwrap();
        assert_eq!(
            receive_reply(&rx, 4, Duration::from_millis(10)).unwrap_err(),
            "startup failed"
        );
    }
    #[test]
    fn missing_gpu_values_stay_unknown_and_fan_percent_is_not_rpm() {
        let r = parse_nvidia("GPU-1, RTX Test, 50, N/A, 42.5, 900, 15000\n", 2000).unwrap();
        assert_eq!(r.sensors[0].value, Some(50.0));
        assert_eq!(r.sensors[1].value, None);
        assert_eq!(r.sensors[1].kind.unit(), "%");
        assert!(r.channels.is_empty());
        assert!(r.fresh(12_000));
        assert!(!r.fresh(12_001));
        assert!(!r.fresh(0));
        assert!(parse_nvidia("broken", 1).is_err());
    }
}

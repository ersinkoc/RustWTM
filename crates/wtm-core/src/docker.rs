//! Read-only Docker Engine monitoring. No CLI subprocesses, daemon writes or
//! privileged service proxy: local pipes are opened as the interactive user.
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

pub const MAX_CONTAINERS: usize = 512;
pub const HISTORY: usize = 60;
const MAX_STATS: usize = 16;
const REQUEST_TIME: Duration = Duration::from_millis(750);
pub const STALE: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub context: String,
    pub pipe: String,
}
impl Endpoint {
    pub fn from_host(context: String, host: &str) -> Result<Self, String> {
        let suffix = host
            .strip_prefix("npipe:////./pipe/")
            .or_else(|| host.strip_prefix(r"npipe://\\.\pipe\"))
            .ok_or("Only local Windows Docker named-pipe contexts are supported")?;
        if suffix.is_empty()
            || suffix.len() > 128
            || !suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err("Invalid local Docker pipe".into());
        }
        Ok(Self {
            context,
            pipe: format!(r"\\.\pipe\{suffix}"),
        })
    }
    pub fn discover() -> Result<Self, String> {
        let home = std::env::var_os("DOCKER_CONFIG")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("USERPROFILE").map(|p| Path::new(&p).join(".docker")))
            .ok_or("Docker user configuration directory is unavailable")?;
        Self::resolve(
            &home,
            std::env::var("DOCKER_CONTEXT")
                .ok()
                .filter(|s| !s.is_empty()),
            std::env::var("DOCKER_HOST").ok().filter(|s| !s.is_empty()),
        )
    }
    fn resolve(
        home: &Path,
        explicit_context: Option<String>,
        host: Option<String>,
    ) -> Result<Self, String> {
        if explicit_context.is_none()
            && let Some(host) = host
        {
            return Self::from_host("DOCKER_HOST".into(), &host);
        }
        let config = if home
            .join("config.json")
            .try_exists()
            .map_err(|_| "Docker configuration is unreadable")?
        {
            json_file(&home.join("config.json"))?
        } else {
            Value::Null
        };
        let context = explicit_context
            .or_else(|| config["currentContext"].as_str().map(str::to_owned))
            .filter(|s| !s.is_empty())
            .unwrap_or("default".into());
        if context == "default" {
            return Self::from_host(context, "npipe:////./pipe/docker_engine");
        }
        let entries = std::fs::read_dir(home.join("contexts/meta"))
            .map_err(|_| "Docker context metadata is unavailable")?;
        for entry in entries.take(256) {
            let entry = entry.map_err(|e| e.to_string())?;
            let Ok(meta) = json_file(&entry.path().join("meta.json")) else {
                continue;
            };
            if meta["Name"].as_str() == Some(&context) {
                return Self::from_host(
                    context,
                    meta["Endpoints"]["docker"]["Host"]
                        .as_str()
                        .ok_or("Docker context has no engine endpoint")?,
                );
            }
        }
        Err("Docker context was not found among the first 256 local contexts".into())
    }
    pub fn get(&self, path: &str, limit: usize) -> Result<Vec<u8>, String> {
        let raw = wtm_sys::pipe_http::get(&self.pipe, path, REQUEST_TIME, limit)
            .map_err(|e| format!("Docker is unavailable: {e}"))?;
        http_body(&raw)
    }
}
fn json_file(path: &Path) -> Result<Value, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(128 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 128 * 1024 {
        return Err("Docker configuration exceeds its size limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Docker configuration is not valid JSON".into())
}

/// HTTP framing is decoded separately from JSON, including Docker's chunked
/// replies and multiplexed log payloads. All sizes are bounded by transport.
pub fn http_body(raw: &[u8]) -> Result<Vec<u8>, String> {
    if raw.len() > 4 * 1024 * 1024 {
        return Err("Docker response exceeds its size limit".into());
    }
    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("Incomplete Docker HTTP headers")?;
    if header_end > 16 * 1024 {
        return Err("Docker HTTP headers exceed their size limit".into());
    }
    let header =
        std::str::from_utf8(&raw[..header_end]).map_err(|_| "Invalid Docker HTTP headers")?;
    let mut lines = header.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or("Invalid Docker HTTP status")?;
    let mut chunked = false;
    let mut length = None;
    for line in lines {
        let (key, value) = line.split_once(':').ok_or("Invalid Docker HTTP header")?;
        if key.eq_ignore_ascii_case("transfer-encoding") {
            if !value.trim().eq_ignore_ascii_case("chunked") {
                return Err("Unsupported Docker transfer encoding".into());
            }
            chunked = true;
        }
        if key.eq_ignore_ascii_case("content-length") {
            let n = value
                .trim()
                .parse::<usize>()
                .map_err(|_| "Invalid Docker content length")?;
            if length.is_some_and(|old| old != n) {
                return Err("Conflicting Docker content lengths".into());
            }
            length = Some(n);
        }
    }
    if chunked && length.is_some() {
        return Err("Ambiguous Docker HTTP framing".into());
    }
    let mut body = &raw[header_end + 4..];
    let decoded = if chunked {
        let mut out = Vec::new();
        loop {
            let end = body
                .windows(2)
                .position(|w| w == b"\r\n")
                .ok_or("Incomplete Docker chunk")?;
            let line = std::str::from_utf8(&body[..end]).map_err(|_| "Invalid Docker chunk")?;
            let size = usize::from_str_radix(line.split(';').next().unwrap_or(""), 16)
                .map_err(|_| "Invalid Docker chunk size")?;
            body = &body[end + 2..];
            if size == 0 {
                if body.len() < 2 {
                    return Err("Incomplete Docker chunk terminator".into());
                }
                break;
            }
            if size > body.len().saturating_sub(2) || body.get(size..size + 2) != Some(b"\r\n") {
                return Err("Incomplete Docker chunk data".into());
            }
            out.extend_from_slice(&body[..size]);
            body = &body[size + 2..];
        }
        out
    } else {
        if length.is_some_and(|n| n != body.len()) {
            return Err("Incomplete Docker response body".into());
        }
        body.to_vec()
    };
    if !(200..300).contains(&status) {
        return Err(format!("Docker HTTP {status}"));
    }
    Ok(decoded)
}

#[derive(Clone, Debug, Default)]
pub struct Metrics {
    pub cpu: Option<f32>,
    pub memory: Option<u64>,
    pub limit: Option<u64>,
    pub rx_rate: Option<f64>,
    pub tx_rate: Option<f64>,
    pub read_rate: Option<f64>,
    pub write_rate: Option<f64>,
    pub measured: Option<Instant>,
    pub cpu_history: VecDeque<f32>,
    pub memory_history: VecDeque<f32>,
}
impl Metrics {
    pub fn fresh(&self, now: Instant) -> bool {
        self.measured
            .is_some_and(|t| now.saturating_duration_since(t) <= STALE)
    }
}
#[derive(Clone, Debug, Default)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub project: String,
    pub state: String,
    pub status: String,
    pub created: i64,
    pub ports: Vec<String>,
    pub metrics: Metrics,
    pub metrics_error: Option<String>,
}
#[derive(Clone, Debug)]
pub struct Detail {
    pub id: String,
    pub health: String,
    pub started: String,
    pub restarts: u64,
    pub logs: Result<String, String>,
}
#[derive(Clone, Debug)]
pub struct Report {
    pub endpoint: Endpoint,
    pub containers: Vec<Container>,
    pub detail: Option<Result<Detail, String>>,
    pub detail_id: Option<String>,
    pub sampled: Instant,
    pub truncated: bool,
}
#[derive(Clone, Debug, Default)]
pub struct State {
    pub report: Option<Report>,
    pub error: Option<String>,
}

struct ContainerList(Vec<RawContainer>);
impl<'de> Deserialize<'de> for ContainerList {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = ContainerList;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a Docker container array")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut rows = Vec::new();
                while rows.len() <= MAX_CONTAINERS {
                    match seq.next_element::<RawContainer>()? {
                        Some(row) => rows.push(row),
                        None => return Ok(ContainerList(rows)),
                    }
                }
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(ContainerList(rows))
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawContainer {
    #[serde(default)]
    id: String,
    #[serde(default)]
    names: Vec<String>,
    #[serde(default)]
    image: String,
    #[serde(default)]
    labels: HashMap<String, String>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    created: i64,
    #[serde(default)]
    ports: Vec<RawPort>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawPort {
    #[serde(default)]
    private_port: u16,
    #[serde(default)]
    public_port: Option<u16>,
    #[serde(default, rename = "IP")]
    ip: Option<String>,
    #[serde(default, rename = "Type")]
    kind: String,
}
fn short(s: &str) -> String {
    s.chars().take(512).collect()
}
fn valid_id(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn number(v: &Value, path: &str) -> Option<u64> {
    v.pointer(path)?.as_u64()
}

#[derive(Default)]
pub struct Sampler {
    endpoint: Option<Endpoint>,
    version: Option<String>,
    previous: HashMap<String, Previous>,
    errors: HashMap<String, String>,
    cursor: usize,
}
struct Previous {
    time: Instant,
    sample: String,
    cpu: Option<u64>,
    system: Option<u64>,
    rx: Option<u64>,
    tx: Option<u64>,
    read: Option<u64>,
    write: Option<u64>,
    metrics: Metrics,
}
impl Sampler {
    pub fn sample(
        &mut self,
        selected: Option<&str>,
        stop: &std::sync::atomic::AtomicBool,
    ) -> Result<Report, String> {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Monitoring stopped".into());
        }
        let endpoint = Endpoint::discover()?;
        if self.endpoint.as_ref() != Some(&endpoint) {
            self.previous.clear();
            self.errors.clear();
            self.version = None;
            self.cursor = 0;
        }
        self.endpoint = Some(endpoint.clone());
        let result = self.collect(&endpoint, selected, stop, &mut |path, limit| {
            endpoint.get(path, limit)
        });
        if result.is_err() {
            self.version = None;
        }
        result
    }
    fn collect(
        &mut self,
        endpoint: &Endpoint,
        selected: Option<&str>,
        stop: &std::sync::atomic::AtomicBool,
        get: &mut impl FnMut(&str, usize) -> Result<Vec<u8>, String>,
    ) -> Result<Report, String> {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Monitoring stopped".into());
        }
        if self.version.is_none() {
            let v: Value = serde_json::from_slice(&get("/version", 32 * 1024)?)
                .map_err(|_| "Invalid Docker version response")?;
            let api = v["ApiVersion"]
                .as_str()
                .ok_or("Docker API version is missing")?;
            let (major, minor) = api.split_once('.').ok_or("Invalid Docker API version")?;
            let pair = (
                major
                    .parse::<u32>()
                    .map_err(|_| "Invalid Docker API version")?,
                minor
                    .parse::<u32>()
                    .map_err(|_| "Invalid Docker API version")?,
            );
            if pair < (1, 41) || pair.0 != 1 {
                return Err("Docker Engine API 1.41 or newer is required".into());
            }
            self.version = Some(format!("v1.{}", pair.1.min(53)));
        }
        let base = self.version.clone().unwrap();
        let bytes = get(&format!("/{base}/containers/json?all=1"), 2 * 1024 * 1024)?;
        let raw: ContainerList =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid Docker container list")?;
        let truncated = raw.0.len() > MAX_CONTAINERS;
        let mut containers: Vec<_> = raw
            .0
            .into_iter()
            .take(MAX_CONTAINERS)
            .filter(|r| valid_id(&r.id))
            .map(|r| {
                let ports = r
                    .ports
                    .into_iter()
                    .take(128)
                    .map(|p| match p.public_port {
                        Some(public) => {
                            let ip =
                                p.ip.as_deref()
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or("0.0.0.0");
                            let binding = ip
                                .parse::<std::net::IpAddr>()
                                .map(|ip| std::net::SocketAddr::new(ip, public).to_string())
                                .unwrap_or_else(|_| format!("{}:{public}", short(ip)));
                            format!("{binding} → {}/{}", p.private_port, short(&p.kind))
                        }
                        None => format!("{}/{}", p.private_port, p.kind),
                    })
                    .collect();
                let metrics = self
                    .previous
                    .get(&r.id)
                    .map(|p| p.metrics.clone())
                    .unwrap_or_default();
                let metrics_error = self.errors.get(&r.id).cloned();
                let name = short(
                    r.names
                        .first()
                        .map(|s| s.trim_start_matches('/'))
                        .filter(|name| !name.is_empty())
                        .unwrap_or(&r.id[..12]),
                );
                Container {
                    id: r.id,
                    name,
                    image: short(&r.image),
                    project: short(
                        r.labels
                            .get("com.docker.compose.project")
                            .map(String::as_str)
                            .unwrap_or(""),
                    ),
                    state: r.state,
                    status: short(&r.status),
                    created: r.created,
                    ports,
                    metrics,
                    metrics_error,
                }
            })
            .collect();
        containers.sort_by(|a, b| a.id.cmp(&b.id));
        self.previous.retain(|id, _| {
            containers
                .iter()
                .any(|c| &c.id == id && c.state == "running")
        });
        self.errors.retain(|id, _| {
            containers
                .iter()
                .any(|c| &c.id == id && c.state == "running")
        });
        for c in containers.iter_mut().filter(|c| c.state != "running") {
            c.metrics = Metrics::default();
            c.metrics_error = None;
        }
        let count = containers.len();
        let budget = Instant::now() + Duration::from_secs(2);
        let mut attempts = 0;
        let start_cursor = self.cursor;
        let selected_index = selected.and_then(|id| containers.iter().position(|c| c.id == id));
        let order = selected_index.into_iter().chain(
            (0..count)
                .map(|offset| (start_cursor + offset) % count)
                .filter(|index| Some(*index) != selected_index),
        );
        for index in order {
            if stop.load(std::sync::atomic::Ordering::Relaxed)
                || Instant::now() >= budget
                || attempts >= MAX_STATS
            {
                break;
            }
            let c = &mut containers[index];
            if Some(index) != selected_index {
                self.cursor = (index + 1) % count;
            }
            if c.state != "running" {
                continue;
            }
            attempts += 1;
            match get(
                &format!(
                    "/{base}/containers/{}/stats?stream=false&one-shot=true",
                    c.id
                ),
                128 * 1024,
            )
            .and_then(|b| {
                serde_json::from_slice::<Value>(&b).map_err(|_| "Invalid Docker statistics".into())
            }) {
                Ok(value) => {
                    c.metrics = self.measure(&c.id, &value, Instant::now());
                    c.metrics_error = None;
                    self.errors.remove(&c.id);
                }
                Err(error) => {
                    self.errors.insert(c.id.clone(), error.clone());
                    c.metrics_error = Some(error);
                }
            }
        }
        let detail_id = selected
            .filter(|id| valid_id(id) && containers.iter().any(|c| c.id == *id))
            .map(str::to_owned);
        let detail = selected
            .filter(|id| {
                !stop.load(std::sync::atomic::Ordering::Relaxed)
                    && valid_id(id)
                    && containers.iter().any(|c| c.id == *id)
            })
            .map(|id| {
                let value: Value = serde_json::from_slice(&get(
                    &format!("/{base}/containers/{id}/json"),
                    512 * 1024,
                )?)
                .map_err(|_| "Invalid Docker container details")?;
                let tty = value["Config"]["Tty"].as_bool().unwrap_or(false);
                let logs = if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    Err("Monitoring stopped".into())
                } else {
                    get(
                        &format!(
                            "/{base}/containers/{id}/logs?stdout=1&stderr=1&timestamps=1&tail=200"
                        ),
                        256 * 1024,
                    )
                    .and_then(|b| decode_logs(&b, tty))
                };
                Ok(Detail {
                    id: id.into(),
                    health: value["State"]["Health"]["Status"]
                        .as_str()
                        .map(short)
                        .unwrap_or_default(),
                    started: value["State"]["StartedAt"]
                        .as_str()
                        .map(short)
                        .unwrap_or_default(),
                    restarts: value["RestartCount"].as_u64().unwrap_or(0),
                    logs,
                })
            });
        Ok(Report {
            endpoint: endpoint.clone(),
            containers,
            detail,
            detail_id,
            sampled: Instant::now(),
            truncated,
        })
    }
    fn measure(&mut self, id: &str, v: &Value, now: Instant) -> Metrics {
        let sample = v["read"].as_str().unwrap_or("").to_owned();
        if !sample.is_empty()
            && let Some(old) = self.previous.get(id)
            && old.sample == sample
        {
            return old.metrics.clone();
        }
        let cpu = number(v, "/cpu_stats/cpu_usage/total_usage");
        let system = number(v, "/cpu_stats/system_cpu_usage");
        let sum_net = |key: &str| {
            v["networks"].as_object().map(|net| {
                net.values()
                    .filter_map(|n| n[key].as_u64())
                    .fold(0u64, u64::saturating_add)
            })
        };
        let sum_io = |op: &str| {
            v.pointer("/blkio_stats/io_service_bytes_recursive")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter(|r| r["op"].as_str().is_some_and(|s| s.eq_ignore_ascii_case(op)))
                        .filter_map(|r| r["value"].as_u64())
                        .fold(0u64, u64::saturating_add)
                })
        };
        let (rx, tx, read, write) = (
            sum_net("rx_bytes"),
            sum_net("tx_bytes"),
            sum_io("read").or_else(|| number(v, "/storage_stats/read_size_bytes")),
            sum_io("write").or_else(|| number(v, "/storage_stats/write_size_bytes")),
        );
        let usage = number(v, "/memory_stats/usage")
            .or_else(|| number(v, "/memory_stats/privateworkingset"));
        let cache = number(v, "/memory_stats/stats/total_inactive_file")
            .or_else(|| number(v, "/memory_stats/stats/inactive_file"))
            .unwrap_or(0);
        let mut m = Metrics {
            memory: usage.map(|n| if cache < n { n - cache } else { n }),
            limit: number(v, "/memory_stats/limit").filter(|n| *n > 0),
            measured: Some(now),
            ..Metrics::default()
        };
        if let Some(old) = self.previous.get(id) {
            m.cpu_history = old.metrics.cpu_history.clone();
            m.memory_history = old.metrics.memory_history.clone();
            let dt = now.saturating_duration_since(old.time).as_secs_f64();
            if dt > 0.0 && dt <= STALE.as_secs_f64() {
                let cores = number(v, "/cpu_stats/online_cpus").or_else(|| {
                    v.pointer("/cpu_stats/cpu_usage/percpu_usage")
                        .and_then(Value::as_array)
                        .map(|a| a.len() as u64)
                });
                if let (Some(delta), Some(cores)) = (
                    system
                        .zip(old.system)
                        .and_then(|(n, o)| n.checked_sub(o))
                        .filter(|n| *n > 0),
                    cores.filter(|n| *n > 0),
                ) {
                    m.cpu = cpu
                        .zip(old.cpu)
                        .and_then(|(n, o)| n.checked_sub(o))
                        .map(|n| (n as f64 / delta as f64 * cores as f64 * 100.0) as f32)
                        .filter(|n| n.is_finite());
                } else if system.is_none()
                    && let Some(cores) = number(v, "/num_procs").filter(|n| *n > 0)
                {
                    m.cpu = cpu
                        .zip(old.cpu)
                        .and_then(|(n, o)| n.checked_sub(o))
                        .map(|n| (n as f64 / (dt * 10_000_000.0 * cores as f64) * 100.0) as f32)
                        .filter(|n| n.is_finite());
                }
                let rate = |n: Option<u64>, o: Option<u64>| {
                    n.zip(o)
                        .and_then(|(n, o)| n.checked_sub(o))
                        .map(|n| n as f64 / dt)
                };
                if v["networks"].is_object() {
                    m.rx_rate = rate(rx, old.rx);
                    m.tx_rate = rate(tx, old.tx);
                }
                if v.pointer("/blkio_stats/io_service_bytes_recursive")
                    .is_some_and(Value::is_array)
                    || v["storage_stats"].is_object()
                {
                    m.read_rate = rate(read, old.read);
                    m.write_rate = rate(write, old.write);
                }
            } else {
                m.cpu_history.clear();
                m.memory_history.clear();
            }
        }
        if let Some(cpu) = m.cpu {
            m.cpu_history.push_back(cpu);
        } else {
            m.cpu_history.clear();
        }
        if let Some(memory) = m.memory {
            m.memory_history.push_back(memory as f32);
        } else {
            m.memory_history.clear();
        }
        while m.cpu_history.len() > HISTORY {
            m.cpu_history.pop_front();
        }
        while m.memory_history.len() > HISTORY {
            m.memory_history.pop_front();
        }
        self.previous.insert(
            id.into(),
            Previous {
                time: now,
                sample,
                cpu,
                system,
                rx,
                tx,
                read,
                write,
                metrics: m.clone(),
            },
        );
        m
    }
}
pub fn decode_logs(mut bytes: &[u8], tty: bool) -> Result<String, String> {
    let mut out = Vec::new();
    if tty {
        out.extend_from_slice(bytes);
    } else {
        while !bytes.is_empty() {
            if bytes.len() < 8 || ![1, 2].contains(&bytes[0]) || bytes[1..4] != [0, 0, 0] {
                return Err("Invalid Docker log stream".into());
            }
            let len = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
            bytes = &bytes[8..];
            if len > bytes.len() {
                return Err("Incomplete Docker log frame".into());
            }
            out.extend_from_slice(&bytes[..len]);
            bytes = &bytes[len..];
        }
    }
    Ok(String::from_utf8_lossy(&out)
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicBool;
    fn endpoint() -> Endpoint {
        Endpoint::from_host("test".into(), "npipe:////./pipe/docker_engine").unwrap()
    }
    fn stats(cpu: u64, system: u64, bytes: u64, sample: &str) -> Value {
        json!({"read":sample,"cpu_stats":{"cpu_usage":{"total_usage":cpu},"system_cpu_usage":system,"online_cpus":4},"memory_stats":{"usage":1000,"limit":4000,"stats":{"inactive_file":100}},"networks":{"eth0":{"rx_bytes":bytes,"tx_bytes":bytes*2}},"blkio_stats":{"io_service_bytes_recursive":[{"op":"Read","value":bytes*3},{"op":"Write","value":bytes*4}]}})
    }
    #[test]
    fn docker_endpoint_is_local_and_request_safe() {
        assert_eq!(endpoint().pipe, r"\\.\pipe\docker_engine");
        for host in [
            "tcp://localhost:2375",
            "ssh://host",
            "npipe:////remote/pipe/docker",
            "npipe:////./pipe/bad/name",
            "npipe:////./pipe/",
        ] {
            assert!(Endpoint::from_host("test".into(), host).is_err());
        }
    }
    #[test]
    fn docker_context_resolution_respects_user_overrides_without_global_env_changes() {
        let root = std::env::temp_dir().join(format!("wtm-docker-context-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let result = std::panic::catch_unwind(|| {
            std::fs::create_dir_all(root.join("contexts/meta/fixture")).unwrap();
            std::fs::write(root.join("config.json"),br#"{"currentContext":"desktop-linux","auths":{"private-registry":{"auth":"never logged"}}}"#).unwrap();
            std::fs::write(root.join("contexts/meta/fixture/meta.json"),br#"{"Name":"desktop-linux","Endpoints":{"docker":{"Host":"npipe:////./pipe/dockerDesktopLinuxEngine"}}}"#).unwrap();
            assert_eq!(
                Endpoint::resolve(&root, None, None).unwrap().context,
                "desktop-linux"
            );
            assert_eq!(
                Endpoint::resolve(
                    &root,
                    Some("default".into()),
                    Some("tcp://remote:2375".into())
                )
                .unwrap()
                .context,
                "default"
            );
            assert!(Endpoint::resolve(&root, None, Some("tcp://remote:2375".into())).is_err());
            assert!(Endpoint::resolve(&root, Some("missing".into()), None).is_err());
        });
        std::fs::remove_dir_all(&root).unwrap();
        result.unwrap();
    }

    #[test]
    fn docker_container_count_is_capped_before_deserializing_unbounded_rows() {
        let mut rows: Vec<_> = (0..MAX_CONTAINERS + 4)
            .map(|i| json!({"Id":format!("{i:064x}"),"Names":["/test"],"State":"exited"}))
            .collect();
        rows[0]["Ports"] = json!([{"IP":"::","PublicPort":8080,"PrivatePort":80,"Type":"tcp"}]);
        let mut sampler = Sampler::default();
        let report = sampler
            .collect(
                &endpoint(),
                None,
                &AtomicBool::new(false),
                &mut |path, _| {
                    Ok(serde_json::to_vec(&if path == "/version" {
                        json!({"ApiVersion":"1.53"})
                    } else {
                        json!(rows)
                    })
                    .unwrap())
                },
            )
            .unwrap();
        assert!(report.truncated);
        assert_eq!(report.containers.len(), MAX_CONTAINERS);
        assert_eq!(report.containers[0].ports[0], "[::]:8080 → 80/tcp");
    }

    #[test]
    fn docker_http_framing_is_checked_before_json() {
        assert_eq!(
            http_body(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap(),
            b"{}"
        );
        assert_eq!(http_body(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\n{\r\n1;ext=x\r\n}\r\n0\r\n\r\n").unwrap(),b"{}");
        for raw in [
            b"HTTP/1.1 500 Error\r\nContent-Length: 0\r\n\r\n".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n{}",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffff\r\nx",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 0\r\n\r\n0\r\n\r\n",
        ] {
            assert!(http_body(raw).is_err());
        }
    }
    #[test]
    fn docker_rates_use_deltas_and_never_invent_a_first_sample() {
        let mut sampler = Sampler::default();
        let t = Instant::now();
        let first = sampler.measure("a", &stats(100, 1000, 100, "one"), t);
        assert_eq!(first.cpu, None);
        assert_eq!(first.rx_rate, None);
        assert_eq!(first.memory, Some(900));
        let second = sampler.measure(
            "a",
            &stats(200, 1400, 300, "two"),
            t + Duration::from_secs(2),
        );
        assert_eq!(second.cpu, Some(100.0));
        assert_eq!(second.rx_rate, Some(100.0));
        assert_eq!(second.tx_rate, Some(200.0));
        assert_eq!(second.read_rate, Some(300.0));
        let repeated = sampler.measure(
            "a",
            &stats(200, 1400, 300, "two"),
            t + Duration::from_secs(3),
        );
        assert_eq!(repeated.measured, second.measured);
        assert_eq!(repeated.cpu_history.len(), 1);
        let reset = sampler.measure("a", &stats(1, 1, 1, "reset"), t + Duration::from_secs(4));
        assert_eq!(reset.cpu, None);
        assert_eq!(reset.rx_rate, None);
        let gap = sampler.measure(
            "a",
            &stats(200, 1400, 300, "gap"),
            t + Duration::from_secs(40),
        );
        assert_eq!(gap.cpu, None);
        assert_eq!(gap.rx_rate, None);
        assert!(!second.fresh(t + Duration::from_secs(40)));
    }
    #[test]
    fn docker_missing_fields_and_cache_bounds_remain_unknown() {
        let mut s = Sampler::default();
        let t = Instant::now();
        let missing = s.measure("a", &json!({}), t);
        assert_eq!(missing.memory, None);
        let measured = s.measure(
            "a",
            &stats(100, 1000, 100, "next"),
            t + Duration::from_secs(1),
        );
        assert_eq!(measured.cpu, None);
        assert_eq!(measured.rx_rate, None);
        let mut v = stats(200, 1200, 200, "last");
        v["memory_stats"]["stats"]["inactive_file"] = json!(2000);
        assert_eq!(
            s.measure("a", &v, t + Duration::from_secs(2)).memory,
            Some(1000)
        );
        for i in 3..100 {
            s.measure(
                "a",
                &stats(i * 100, i * 1000, i * 10, &i.to_string()),
                t + Duration::from_secs(i),
            );
        }
        assert!(s.previous["a"].metrics.cpu_history.len() <= HISTORY);
        assert_eq!(s.previous["a"].metrics.memory_history.len(), HISTORY);
    }
    #[test]
    fn docker_windows_stats_use_available_cpu_memory_and_storage_fields() {
        let mut s = Sampler::default();
        let t = Instant::now();
        let stats = |cpu, read, sample| json!({"read":sample,"num_procs":4,"cpu_stats":{"cpu_usage":{"total_usage":cpu}},"memory_stats":{"privateworkingset":900,"limit":4096},"storage_stats":{"read_size_bytes":read,"write_size_bytes":read*2}});
        assert_eq!(s.measure("win", &stats(0u64, 0u64, "one"), t).cpu, None);
        let second = s.measure(
            "win",
            &stats(20_000_000u64, 512u64, "two"),
            t + Duration::from_secs(1),
        );
        assert_eq!(second.cpu, Some(50.0));
        assert_eq!(second.memory, Some(900));
        assert_eq!(second.read_rate, Some(512.0));
        assert_eq!(second.write_rate, Some(1024.0));
    }

    #[test]
    fn docker_multiplexed_and_tty_logs_are_bounded_and_decoded() {
        let bytes = [
            1, 0, 0, 0, 0, 0, 0, 3, b'o', b'k', b'\n', 2, 0, 0, 0, 0, 0, 0, 3, b'e', b'r', b'r',
        ];
        assert_eq!(decode_logs(&bytes, false).unwrap(), "ok\nerr");
        assert_eq!(decode_logs(b"plain\n", true).unwrap(), "plain\n");
        assert!(decode_logs(&bytes[..9], false).is_err());
        assert_eq!(decode_logs(b"", false).unwrap(), "");
    }
    #[test]
    fn docker_collection_is_fair_bounded_and_failure_isolation_keeps_rows() {
        let endpoint = endpoint();
        let mut s = Sampler::default();
        let stop = AtomicBool::new(false);
        let rows:Vec<_>=(0..20).map(|i|json!({"Id":format!("{i:064x}"),"Names":[format!("/app-{i}")],"Image":"demo","State":"running","Labels":{"com.docker.compose.project":"suite"},"Ports":[{"PrivatePort":80,"PublicPort":8080,"IP":"127.0.0.1","Type":"tcp"}]})).collect();
        let mut ids = Vec::new();
        let mut get = |path: &str, _| {
            if path == "/version" {
                return Ok(serde_json::to_vec(&json!({"ApiVersion":"1.99"})).unwrap());
            }
            if path.ends_with("/containers/json?all=1") {
                return Ok(serde_json::to_vec(&rows).unwrap());
            }
            ids.push(path.to_owned());
            Err("injected stats failure".into())
        };
        let one = s.collect(&endpoint, None, &stop, &mut get).unwrap();
        let two = s.collect(&endpoint, None, &stop, &mut get).unwrap();
        assert_eq!(one.containers.len(), 20);
        assert_eq!(two.containers[0].project, "suite");
        assert_eq!(
            one.containers[0].ports[0], "127.0.0.1:8080 → 80/tcp",
            "preserve the Engine's published IP address"
        );
        assert_eq!(ids.len(), 32);
        assert_ne!(ids[0], ids[16]);
        assert_eq!(
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            20
        );
        assert!(one.containers.iter().any(|c| c.metrics_error.is_some()));
        assert!(
            two.containers.iter().all(|c| c.metrics_error.is_some()),
            "failures persist until that row is sampled successfully"
        );
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(
            s.collect(&endpoint, None, &stop, &mut |_, _| panic!(
                "stopped sampler must not request"
            ))
            .is_err()
        );
    }
    #[test]
    #[ignore = "requires the interactive user's local Docker Engine"]
    fn docker_live_read_only_sample() {
        let mut s = Sampler::default();
        let r = s.sample(None, &AtomicBool::new(false)).unwrap();
        println!(
            "Docker context: {}; containers: {}",
            r.endpoint.context,
            r.containers.len()
        );
        assert!(r.endpoint.pipe.starts_with(r"\\.\pipe\"));
        if let Some(first) = r.containers.iter().find(|c| c.state == "running") {
            let id = first.id.clone();
            std::thread::sleep(Duration::from_secs(1));
            let second = s.sample(Some(&id), &AtomicBool::new(false)).unwrap();
            let row = second
                .containers
                .iter()
                .find(|c| c.id == id)
                .expect("selected container is still listed");
            assert!(row.metrics.measured.is_some());
            assert!(
                row.metrics.cpu.is_some(),
                "a repeated live sample must expose CPU when Linux counters are available"
            );
            assert!(row.metrics.memory.is_some());
            let detail = second.detail.as_ref().unwrap().as_ref().unwrap();
            assert_eq!(detail.id, id);
            println!(
                "Live selected sample: CPU available, memory available, detail identity verified; logs readable: {}",
                detail.logs.is_ok()
            );
        }
    }
}

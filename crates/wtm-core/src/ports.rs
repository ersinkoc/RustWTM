//! Ports in use: which program listens where, and the usual ports of
//! developer tools and servers.

use std::collections::BTreeMap;
use std::net::IpAddr;

use crate::{Connection, Protocol, TcpState};

/// A port that a well-known tool or server uses by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownPort {
    pub port: u16,
    pub service: &'static str,
    pub category: &'static str,
}

const fn known(port: u16, service: &'static str, category: &'static str) -> KnownPort {
    KnownPort {
        port,
        service,
        category,
    }
}

pub const DEV_SERVER: &str = "Dev servers";
pub const DATABASE: &str = "Databases";
pub const CACHE: &str = "Caches & queues";
pub const SEARCH: &str = "Search";
pub const CONTAINERS: &str = "Containers & cloud";
pub const MONITORING: &str = "Monitoring";
pub const AI: &str = "AI & notebooks";
pub const DEBUG: &str = "Debuggers & tools";
pub const WEB: &str = "Web";
pub const SYSTEM: &str = "Windows & remote access";

/// The catalog, by category (the order the GUI shows them in).
pub const KNOWN_PORTS: &[KnownPort] = &[
    known(3000, "Node / React / Next.js / Grafana", DEV_SERVER),
    known(3001, "Node / React (second app)", DEV_SERVER),
    known(4200, "Angular CLI", DEV_SERVER),
    known(5173, "Vite", DEV_SERVER),
    known(5174, "Vite (second app)", DEV_SERVER),
    known(4173, "Vite preview", DEV_SERVER),
    known(8080, "HTTP alt / webpack / Tomcat / Spring", DEV_SERVER),
    known(8000, "Django / Python http.server / Laravel", DEV_SERVER),
    known(5000, "Flask / ASP.NET / Docker registry", DEV_SERVER),
    known(5001, "ASP.NET (HTTPS)", DEV_SERVER),
    known(7071, "Azure Functions", DEV_SERVER),
    known(8081, "Metro (React Native)", DEV_SERVER),
    known(8787, "Wrangler (Cloudflare Workers)", DEV_SERVER),
    known(19000, "Expo", DEV_SERVER),
    known(6006, "Storybook", DEV_SERVER),
    known(4321, "Astro", DEV_SERVER),
    known(4000, "Phoenix / Jekyll / Gatsby", DEV_SERVER),
    known(1313, "Hugo", DEV_SERVER),
    known(51204, "Vitest UI", DEV_SERVER),
    known(9323, "Playwright report", DEV_SERVER),
    known(5432, "PostgreSQL", DATABASE),
    known(3306, "MySQL / MariaDB", DATABASE),
    known(1433, "SQL Server", DATABASE),
    known(1521, "Oracle", DATABASE),
    known(27017, "MongoDB", DATABASE),
    known(9042, "Cassandra / ScyllaDB", DATABASE),
    known(7474, "Neo4j", DATABASE),
    known(8086, "InfluxDB", DATABASE),
    known(8123, "ClickHouse (HTTP)", DATABASE),
    known(5984, "CouchDB", DATABASE),
    known(26257, "CockroachDB", DATABASE),
    known(6333, "Qdrant", DATABASE),
    known(8529, "ArangoDB", DATABASE),
    known(6379, "Redis / Valkey", CACHE),
    known(11211, "Memcached", CACHE),
    known(5672, "RabbitMQ", CACHE),
    known(15672, "RabbitMQ management", CACHE),
    known(9092, "Kafka", CACHE),
    known(2181, "ZooKeeper", CACHE),
    known(4222, "NATS", CACHE),
    known(1883, "MQTT", CACHE),
    known(9200, "Elasticsearch / OpenSearch", SEARCH),
    known(5601, "Kibana", SEARCH),
    known(7700, "Meilisearch", SEARCH),
    known(8108, "Typesense", SEARCH),
    known(2375, "Docker API", CONTAINERS),
    known(2376, "Docker API (TLS)", CONTAINERS),
    known(6443, "Kubernetes API", CONTAINERS),
    known(8001, "kubectl proxy", CONTAINERS),
    known(9000, "MinIO / Portainer / PHP-FPM", CONTAINERS),
    known(4566, "LocalStack", CONTAINERS),
    known(10000, "Azurite (Azure storage)", CONTAINERS),
    known(8200, "Vault", CONTAINERS),
    known(8500, "Consul", CONTAINERS),
    known(9090, "Prometheus", MONITORING),
    known(9100, "Prometheus node exporter", MONITORING),
    known(16686, "Jaeger", MONITORING),
    known(4317, "OpenTelemetry (gRPC)", MONITORING),
    known(4318, "OpenTelemetry (HTTP)", MONITORING),
    known(11434, "Ollama", AI),
    known(1234, "LM Studio", AI),
    known(7860, "Gradio / Stable Diffusion web UI", AI),
    known(8188, "ComfyUI", AI),
    known(8888, "Jupyter", AI),
    known(9229, "Node.js inspector", DEBUG),
    known(9222, "Chrome DevTools", DEBUG),
    known(5037, "Android Debug Bridge", DEBUG),
    known(8025, "Mailpit / MailHog (web)", DEBUG),
    known(1025, "Mailpit / MailHog (SMTP)", DEBUG),
    known(80, "HTTP", WEB),
    known(443, "HTTPS", WEB),
    known(8443, "HTTPS alt", WEB),
    known(22, "SSH", SYSTEM),
    known(3389, "Remote Desktop", SYSTEM),
    known(5900, "VNC", SYSTEM),
    known(5985, "WinRM", SYSTEM),
    known(445, "File sharing (SMB)", SYSTEM),
    known(135, "Windows RPC", SYSTEM),
];

/// The catalog entry for `port`.
pub fn known_port(port: u16) -> Option<&'static KnownPort> {
    KNOWN_PORTS.iter().find(|k| k.port == port)
}

/// Who can reach a listening socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Exposure {
    /// Only programs on this PC (127.0.0.1 / ::1).
    ThisPc,
    /// One specific network address.
    Network,
    /// Every network adapter (0.0.0.0 / ::).
    AllNetworks,
}

impl Exposure {
    pub fn label(self) -> &'static str {
        match self {
            Self::ThisPc => "This PC only",
            Self::Network => "One network",
            Self::AllNetworks => "All networks",
        }
    }
}

/// One program listening on (TCP) or bound to (UDP) a local port.
#[derive(Debug, Clone, PartialEq)]
pub struct PortUse {
    pub protocol: Protocol,
    pub port: u16,
    /// Local addresses, IPv4 and IPv6, without duplicates.
    pub addresses: Vec<IpAddr>,
    pub pid: u32,
    /// Established TCP connections to this port.
    pub connections: usize,
}

impl PortUse {
    pub fn exposure(&self) -> Exposure {
        self.addresses
            .iter()
            .map(|a| {
                if a.is_unspecified() {
                    Exposure::AllNetworks
                } else if a.is_loopback() {
                    Exposure::ThisPc
                } else {
                    Exposure::Network
                }
            })
            .max()
            .unwrap_or(Exposure::ThisPc)
    }

    pub fn known(&self) -> Option<&'static KnownPort> {
        known_port(self.port)
    }

    /// The addresses as text, e.g. `0.0.0.0, ::`.
    pub fn address_text(&self) -> String {
        let parts: Vec<String> = self.addresses.iter().map(IpAddr::to_string).collect();
        parts.join(", ")
    }
}

/// Every listening TCP port and bound UDP port, one entry per protocol,
/// port and process; sorted by port.
pub fn port_uses(conns: &[Connection]) -> Vec<PortUse> {
    let mut uses: BTreeMap<(u16, u8, u32), PortUse> = BTreeMap::new();
    for c in conns {
        let listening = match c.protocol {
            Protocol::Tcp => c.state == Some(TcpState::Listen),
            Protocol::Udp => true,
        };
        if !listening {
            continue;
        }
        let key = (c.local.port(), c.protocol as u8, c.pid);
        let entry = uses.entry(key).or_insert_with(|| PortUse {
            protocol: c.protocol,
            port: c.local.port(),
            addresses: Vec::new(),
            pid: c.pid,
            connections: 0,
        });
        if !entry.addresses.contains(&c.local.ip()) {
            entry.addresses.push(c.local.ip());
        }
    }
    for c in conns {
        if c.protocol == Protocol::Tcp && c.state == Some(TcpState::Established) {
            let key = (c.local.port(), Protocol::Tcp as u8, c.pid);
            if let Some(u) = uses.get_mut(&key) {
                u.connections += 1;
            }
        }
    }
    uses.into_values().collect()
}

/// The processes that hold `port` open for listening (TCP) or bound (UDP);
/// what has to end to free the port. Sorted, without duplicates.
pub fn port_owners(uses: &[PortUse], port: u16) -> Vec<u32> {
    let mut pids: Vec<u32> = uses
        .iter()
        .filter(|u| u.port == port && u.pid != 0)
        .map(|u| u.pid)
        .collect();
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// A warning for programs that only relay a port for something else:
/// ending them stops more than the one port.
pub fn relay_hint(process_name: &str) -> Option<&'static str> {
    match process_name.to_ascii_lowercase().as_str() {
        "wslrelay.exe" => Some(
            "Forwarded from WSL: ending it cuts the forwarding; stop the server inside WSL instead",
        ),
        "com.docker.backend.exe" | "vpnkit.exe" | "com.docker.proxy.exe" => Some(
            "Forwarded from a Docker container: ending it stops Docker Desktop; stop the container instead",
        ),
        _ => None,
    }
}

/// Why the program behind a port may not be ended from the port views.
pub fn end_blocked(pid: u32, process_name: &str) -> Option<String> {
    if let Some(reason) = crate::protection_reason(pid, process_name) {
        let what = if pid == 4 {
            " (Windows serves this port itself: http.sys or file sharing)"
        } else {
            ""
        };
        return Some(format!("protected: {reason}{what}"));
    }
    process_name
        .eq_ignore_ascii_case("svchost.exe")
        .then(|| "it hosts Windows services; stop the service in the Services tab instead".into())
}

/// Parses `3000`, `:3000` or `localhost:3000` into a port number.
pub fn parse_port(text: &str) -> Option<u16> {
    let text = text.trim();
    let digits = text.rsplit(':').next().unwrap_or(text).trim();
    digits.parse::<u16>().ok().filter(|p| *p != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    fn tcp(local: &str, state: TcpState, pid: u32) -> Connection {
        Connection {
            protocol: Protocol::Tcp,
            local: local.parse().unwrap(),
            remote: None,
            state: Some(state),
            pid,
        }
    }

    fn udp(local: &str, pid: u32) -> Connection {
        Connection {
            protocol: Protocol::Udp,
            local: local.parse().unwrap(),
            remote: None,
            state: None,
            pid,
        }
    }

    #[test]
    fn catalog_has_no_duplicate_ports() {
        let mut ports: Vec<u16> = KNOWN_PORTS.iter().map(|k| k.port).collect();
        ports.sort_unstable();
        let before = ports.len();
        ports.dedup();
        assert_eq!(ports.len(), before);
        assert_eq!(known_port(5173).unwrap().service, "Vite");
        assert_eq!(known_port(5432).unwrap().category, DATABASE);
        assert!(known_port(1).is_none());
    }

    #[test]
    fn listeners_are_grouped_per_port_and_process() {
        let conns = vec![
            tcp("0.0.0.0:3000", TcpState::Listen, 10),
            tcp("[::]:3000", TcpState::Listen, 10),
            tcp("127.0.0.1:3000", TcpState::Established, 10),
            tcp("127.0.0.1:3000", TcpState::Established, 10),
            tcp("127.0.0.1:51000", TcpState::Established, 11),
            tcp("127.0.0.1:5432", TcpState::Listen, 20),
            tcp("127.0.0.1:5432", TcpState::TimeWait, 0),
            udp("0.0.0.0:5353", 30),
            udp("0.0.0.0:5353", 31),
        ];
        let uses = port_uses(&conns);
        let ports: Vec<(u16, u32)> = uses.iter().map(|u| (u.port, u.pid)).collect();
        assert_eq!(ports, [(3000, 10), (5353, 30), (5353, 31), (5432, 20)]);
        let node = &uses[0];
        assert_eq!(
            node.addresses,
            [
                IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                IpAddr::V6(Ipv6Addr::UNSPECIFIED)
            ]
        );
        assert_eq!(node.connections, 2);
        assert_eq!(node.exposure(), Exposure::AllNetworks);
        assert_eq!(node.address_text(), "0.0.0.0, ::");
        assert_eq!(uses[3].exposure(), Exposure::ThisPc);
        assert_eq!(uses[3].known().unwrap().service, "PostgreSQL");
        assert_eq!(port_owners(&uses, 5353), [30, 31]);
        assert_eq!(port_owners(&uses, 9999), Vec::<u32>::new());
    }

    #[test]
    fn exposure_of_a_specific_address() {
        let u = PortUse {
            protocol: Protocol::Tcp,
            port: 80,
            addresses: vec![
                SocketAddr::from(([127, 0, 0, 1], 80)).ip(),
                SocketAddr::from(([192, 168, 1, 5], 80)).ip(),
            ],
            pid: 1,
            connections: 0,
        };
        assert_eq!(u.exposure(), Exposure::Network);
    }

    #[test]
    fn ports_are_parsed_from_typical_text() {
        assert_eq!(parse_port("3000"), Some(3000));
        assert_eq!(parse_port(" :5173 "), Some(5173));
        assert_eq!(parse_port("localhost:8080"), Some(8080));
        assert_eq!(parse_port("0"), None);
        assert_eq!(parse_port("70000"), None);
        assert_eq!(parse_port("abc"), None);
    }

    #[test]
    fn relays_are_explained_and_windows_is_left_alone() {
        assert!(relay_hint("wslrelay.exe").unwrap().contains("WSL"));
        assert!(
            relay_hint("com.docker.backend.exe")
                .unwrap()
                .contains("Docker")
        );
        assert!(relay_hint("node.exe").is_none());
        assert!(end_blocked(4, "System").unwrap().contains("http.sys"));
        assert!(
            end_blocked(900, "svchost.exe")
                .unwrap()
                .contains("Services tab")
        );
        assert!(end_blocked(900, "node.exe").is_none());
    }

    #[test]
    fn real_ports_can_be_read() {
        // A listener of our own must show up with this process as owner.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let uses = port_uses(&crate::connections().unwrap());
        assert_eq!(port_owners(&uses, port), [std::process::id()]);
    }
}

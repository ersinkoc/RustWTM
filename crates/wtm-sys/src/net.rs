//! TCP/UDP endpoints with their owning process.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR, WIN32_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, GetExtendedUdpTable, MIB_TCP_STATE_DELETE_TCB, MIB_TCP6ROW_OWNER_PID,
    MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_LH, MIB_TCPROW_LH_0, MIB_TCPROW_OWNER_PID,
    MIB_TCPTABLE_OWNER_PID, MIB_UDP6ROW_OWNER_PID, MIB_UDP6TABLE_OWNER_PID, MIB_UDPROW_OWNER_PID,
    MIB_UDPTABLE_OWNER_PID, SetTcpEntry, TCP_TABLE_OWNER_PID_ALL, TCP_TABLE_OWNER_PID_LISTENER,
    UDP_TABLE_OWNER_PID,
};

use crate::Result;

const AF_INET: u32 = 2;
const AF_INET6: u32 = 23;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Protocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
    DeleteTcb,
    Unknown(u32),
}

impl TcpState {
    fn from_raw(raw: u32) -> Self {
        match raw {
            1 => Self::Closed,
            2 => Self::Listen,
            3 => Self::SynSent,
            4 => Self::SynReceived,
            5 => Self::Established,
            6 => Self::FinWait1,
            7 => Self::FinWait2,
            8 => Self::CloseWait,
            9 => Self::Closing,
            10 => Self::LastAck,
            11 => Self::TimeWait,
            12 => Self::DeleteTcb,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub protocol: Protocol,
    pub local: SocketAddr,
    /// `None` for UDP and listening TCP sockets.
    pub remote: Option<SocketAddr>,
    pub state: Option<TcpState>,
    pub pid: u32,
}

fn port(raw: u32) -> u16 {
    // Ports are stored in network byte order in the low 16 bits.
    u16::from_be(raw as u16)
}

fn v4(raw: u32) -> IpAddr {
    // Stored in network byte order, i.e. the in-memory bytes are the octets.
    IpAddr::V4(Ipv4Addr::from(raw.to_ne_bytes()))
}

// Windows keeps IPv6 scope identifiers separately from the address octets.
fn scoped_v6(address: [u8; 16], raw_port: u32, scope_id: u32) -> SocketAddr {
    SocketAddr::V6(SocketAddrV6::new(
        Ipv6Addr::from(address),
        port(raw_port),
        0,
        scope_id,
    ))
}

/// Bytes the TCPv4, TCPv6, UDPv4 and UDPv6 tables needed last time.
static SIZE_HINTS: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];
const MAX_TABLE_BYTES: usize = 16 * 1024 * 1024;
const TABLE_ATTEMPTS: usize = 4;
type TcpEndpoint = (SocketAddr, SocketAddr, u32);
fn key4(r: &MIB_TCPROW_OWNER_PID) -> TcpEndpoint {
    (
        SocketAddr::new(v4(r.dwLocalAddr), port(r.dwLocalPort)),
        SocketAddr::new(v4(r.dwRemoteAddr), port(r.dwRemotePort)),
        r.dwOwningPid,
    )
}
fn key6(r: &MIB_TCP6ROW_OWNER_PID) -> TcpEndpoint {
    (
        scoped_v6(r.ucLocalAddr, r.dwLocalPort, r.dwLocalScopeId),
        scoped_v6(r.ucRemoteAddr, r.dwRemotePort, r.dwRemoteScopeId),
        r.dwOwningPid,
    )
}
fn confirmed_state(raw: u32, key: &TcpEndpoint, listeners: &HashSet<TcpEndpoint>) -> TcpState {
    // Some hosts report raw state zero even for a real listener. Only the
    // documented LISTENER table can establish that role; never guess from zero
    // or from an empty remote address. Keep every unconfirmed value unchanged.
    if raw == 0 && listeners.contains(key) {
        TcpState::Listen
    } else {
        TcpState::from_raw(raw)
    }
}
fn table_limit() -> crate::Error {
    windows::core::Error::from(ERROR_INSUFFICIENT_BUFFER.to_hresult()).into()
}

/// Calls a `GetExtended*Table` function until the buffer is large enough.
///
/// Every call walks the whole kernel table, a size-only one included, so
/// the first call already offers a buffer as large as the one that sufficed
/// last time (`hint`, in bytes): one call per table instead of two.
fn fetch(
    hint: &AtomicUsize,
    mut call: impl FnMut(Option<*mut core::ffi::c_void>, &mut u32) -> u32,
) -> Result<Vec<u64>> {
    let hinted = hint.load(Ordering::Relaxed);
    if hinted > MAX_TABLE_BYTES {
        return Err(table_limit());
    }
    let mut buf: Vec<u64> = vec![0; hinted.div_ceil(8)];
    let mut size = (buf.len() * 8) as u32;
    for _ in 0..TABLE_ATTEMPTS {
        let ptr = (!buf.is_empty()).then(|| buf.as_mut_ptr().cast());
        let rc = WIN32_ERROR(call(ptr, &mut size));
        if rc == NO_ERROR {
            hint.store(buf.len() * 8, Ordering::Relaxed);
            return Ok(buf);
        }
        if rc != ERROR_INSUFFICIENT_BUFFER {
            return Err(windows::core::Error::from(rc.to_hresult()).into());
        }
        // Tables can grow between calls; leave headroom.
        if size as usize > MAX_TABLE_BYTES {
            return Err(table_limit());
        }
        buf.resize(
            (size as usize)
                .saturating_add(4096)
                .min(MAX_TABLE_BYTES)
                .div_ceil(8),
            0,
        );
        size = (buf.len() * 8) as u32;
    }
    Err(table_limit())
}

/// # Safety
/// `buf` must hold a `T`-table: a `u32` count followed by that many `R` rows
/// at offset `rows_offset`.
unsafe fn rows<R: Copy>(buf: &[u64], rows_offset: usize) -> Vec<R> {
    if buf.is_empty() {
        return Vec::new();
    }
    let base = buf.as_ptr().cast::<u8>();
    // SAFETY: guaranteed by the caller.
    unsafe {
        let count = *base.cast::<u32>() as usize;
        let first = base.add(rows_offset).cast::<R>();
        (0..count).map(|i| *first.add(i)).collect()
    }
}

/// Resets an IPv4 TCP connection, as Resource Monitor and TCPView do: both
/// ends see it closed at once. Needs administrator rights. Windows offers
/// no way to close an IPv6 connection from outside its owner.
pub fn close_tcp(local: SocketAddr, remote: SocketAddr) -> Result<()> {
    let (SocketAddr::V4(l), SocketAddr::V4(r)) = (local, remote) else {
        return Err(windows::core::Error::from(
            windows::Win32::Foundation::ERROR_NOT_SUPPORTED.to_hresult(),
        )
        .into());
    };
    let row = MIB_TCPROW_LH {
        Anonymous: MIB_TCPROW_LH_0 {
            dwState: MIB_TCP_STATE_DELETE_TCB.0 as u32,
        },
        dwLocalAddr: u32::from_ne_bytes(l.ip().octets()),
        dwLocalPort: u32::from(l.port().to_be()),
        dwRemoteAddr: u32::from_ne_bytes(r.ip().octets()),
        dwRemotePort: u32::from(r.port().to_be()),
    };
    // SAFETY: `row` is a fully initialized MIB_TCPROW.
    let rc = WIN32_ERROR(unsafe { SetTcpEntry(&row) });
    if rc == NO_ERROR {
        Ok(())
    } else {
        Err(windows::core::Error::from(rc.to_hresult()).into())
    }
}

pub fn connections() -> Result<Vec<Connection>> {
    let mut out = Vec::new();

    let tcp4 = fetch(&SIZE_HINTS[0], |p, size| {
        // SAFETY: `p` is None or a buffer of `*size` bytes.
        unsafe { GetExtendedTcpTable(p, size, false, AF_INET, TCP_TABLE_OWNER_PID_ALL, 0) }
    })?;
    // SAFETY: filled by GetExtendedTcpTable(AF_INET, OWNER_PID_ALL).
    let tcp4_rows = unsafe {
        rows::<MIB_TCPROW_OWNER_PID>(&tcp4, std::mem::offset_of!(MIB_TCPTABLE_OWNER_PID, table))
    };
    let mut listeners = HashSet::new();
    if tcp4_rows.iter().any(|r| r.dwState == 0) {
        // A failed role query leaves the original unknown state visible.
        if let Ok(table) = fetch(&SIZE_HINTS[4], |p, size| {
            // SAFETY: p is absent or points to size writable bytes; same
            // documented row layout as OWNER_PID_ALL, filtered by the OS.
            unsafe { GetExtendedTcpTable(p, size, false, AF_INET, TCP_TABLE_OWNER_PID_LISTENER, 0) }
        }) {
            // SAFETY: successful IPv4 OWNER_PID_LISTENER query above.
            listeners.extend(
                unsafe {
                    rows::<MIB_TCPROW_OWNER_PID>(
                        &table,
                        std::mem::offset_of!(MIB_TCPTABLE_OWNER_PID, table),
                    )
                }
                .iter()
                .map(key4),
            );
        }
    }
    for r in tcp4_rows {
        let state = confirmed_state(r.dwState, &key4(&r), &listeners);
        out.push(Connection {
            protocol: Protocol::Tcp,
            local: SocketAddr::new(v4(r.dwLocalAddr), port(r.dwLocalPort)),
            remote: (state != TcpState::Listen)
                .then(|| SocketAddr::new(v4(r.dwRemoteAddr), port(r.dwRemotePort))),
            state: Some(state),
            pid: r.dwOwningPid,
        });
    }

    let tcp6 = fetch(&SIZE_HINTS[1], |p, size| {
        // SAFETY: as above.
        unsafe { GetExtendedTcpTable(p, size, false, AF_INET6, TCP_TABLE_OWNER_PID_ALL, 0) }
    })?;
    // SAFETY: filled by GetExtendedTcpTable(AF_INET6, OWNER_PID_ALL).
    let tcp6_rows = unsafe {
        rows::<MIB_TCP6ROW_OWNER_PID>(&tcp6, std::mem::offset_of!(MIB_TCP6TABLE_OWNER_PID, table))
    };
    listeners.clear();
    if tcp6_rows.iter().any(|r| r.dwState == 0)
        && let Ok(table) = fetch(&SIZE_HINTS[5], |p, size| {
            // SAFETY: as above, using the IPv6 owner-PID row layout.
            unsafe {
                GetExtendedTcpTable(p, size, false, AF_INET6, TCP_TABLE_OWNER_PID_LISTENER, 0)
            }
        })
    {
        // SAFETY: successful IPv6 OWNER_PID_LISTENER query above.
        listeners.extend(
            unsafe {
                rows::<MIB_TCP6ROW_OWNER_PID>(
                    &table,
                    std::mem::offset_of!(MIB_TCP6TABLE_OWNER_PID, table),
                )
            }
            .iter()
            .map(key6),
        );
    }
    for r in tcp6_rows {
        let state = confirmed_state(r.dwState, &key6(&r), &listeners);
        out.push(Connection {
            protocol: Protocol::Tcp,
            local: scoped_v6(r.ucLocalAddr, r.dwLocalPort, r.dwLocalScopeId),
            remote: (state != TcpState::Listen)
                .then(|| scoped_v6(r.ucRemoteAddr, r.dwRemotePort, r.dwRemoteScopeId)),
            state: Some(state),
            pid: r.dwOwningPid,
        });
    }

    let udp4 = fetch(&SIZE_HINTS[2], |p, size| {
        // SAFETY: as above.
        unsafe { GetExtendedUdpTable(p, size, false, AF_INET, UDP_TABLE_OWNER_PID, 0) }
    })?;
    // SAFETY: filled by GetExtendedUdpTable(AF_INET, OWNER_PID).
    for r in unsafe {
        rows::<MIB_UDPROW_OWNER_PID>(&udp4, std::mem::offset_of!(MIB_UDPTABLE_OWNER_PID, table))
    } {
        out.push(Connection {
            protocol: Protocol::Udp,
            local: SocketAddr::new(v4(r.dwLocalAddr), port(r.dwLocalPort)),
            remote: None,
            state: None,
            pid: r.dwOwningPid,
        });
    }

    let udp6 = fetch(&SIZE_HINTS[3], |p, size| {
        // SAFETY: as above.
        unsafe { GetExtendedUdpTable(p, size, false, AF_INET6, UDP_TABLE_OWNER_PID, 0) }
    })?;
    // SAFETY: filled by GetExtendedUdpTable(AF_INET6, OWNER_PID).
    for r in unsafe {
        rows::<MIB_UDP6ROW_OWNER_PID>(&udp6, std::mem::offset_of!(MIB_UDP6TABLE_OWNER_PID, table))
    } {
        out.push(Connection {
            protocol: Protocol::Udp,
            local: scoped_v6(r.ucLocalAddr, r.dwLocalPort, r.dwLocalScopeId),
            remote: None,
            state: None,
            pid: r.dwOwningPid,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {

    #[test]
    fn closing_needs_ipv4_and_rights() {
        let v6: SocketAddr = "[::1]:5000".parse().unwrap();
        let v4: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        assert!(close_tcp(v6, v6).is_err(), "IPv6 cannot be reset");
        // No such connection (or no rights): refused, never a crash.
        assert!(close_tcp(v4, "127.0.0.1:5001".parse().unwrap()).is_err());
    }

    use super::*;
    use std::net::TcpListener;
    #[test]
    fn zero_state_needs_exact_os_listener_confirmation() {
        let key = (
            "127.0.0.1:8080".parse().unwrap(),
            "0.0.0.0:0".parse().unwrap(),
            100,
        );
        let confirmed = HashSet::from([key]);
        assert_eq!(TcpState::from_raw(0), TcpState::Unknown(0));
        assert_eq!(confirmed_state(0, &key, &confirmed), TcpState::Listen);
        assert_eq!(
            confirmed_state(0, &key, &HashSet::new()),
            TcpState::Unknown(0)
        );
        assert_eq!(
            confirmed_state(0, &(key.0, key.1, 101), &confirmed),
            TcpState::Unknown(0)
        );
        assert_eq!(
            confirmed_state(
                0,
                &(key.0, "127.0.0.1:9000".parse().unwrap(), 100),
                &confirmed
            ),
            TcpState::Unknown(0)
        );
        assert_eq!(confirmed_state(5, &key, &confirmed), TcpState::Established);
        assert_eq!(confirmed_state(13, &key, &confirmed), TcpState::Unknown(13));
        let scoped = (
            "[fe80::1%7]:8080".parse().unwrap(),
            "[::]:0".parse().unwrap(),
            100,
        );
        assert_eq!(
            confirmed_state(
                0,
                &("[fe80::1%8]:8080".parse().unwrap(), scoped.1, 100),
                &HashSet::from([scoped])
            ),
            TcpState::Unknown(0)
        );
    }
    #[test]
    fn table_growth_is_bounded_without_allocating_an_advertised_huge_buffer() {
        let hint = AtomicUsize::new(0);
        let mut calls = 0;
        let result = fetch(&hint, |_, size| {
            calls += 1;
            *size = u32::MAX;
            ERROR_INSUFFICIENT_BUFFER.0
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
        assert_eq!(hint.load(Ordering::Relaxed), 0);
        calls = 0;
        assert!(
            fetch(&hint, |_, size| {
                calls += 1;
                *size = 16;
                if calls > TABLE_ATTEMPTS {
                    0
                } else {
                    ERROR_INSUFFICIENT_BUFFER.0
                }
            })
            .is_err()
        );
        assert_eq!(calls, TABLE_ATTEMPTS);
    }

    #[test]
    fn finds_own_listening_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let conns = connections().unwrap();
        let mine = conns
            .iter()
            .find(|c| c.protocol == Protocol::Tcp && c.local == addr)
            .expect("own listener not found");
        assert_eq!(mine.pid, std::process::id());
        assert_eq!(mine.state, Some(TcpState::Listen));
        assert!(mine.remote.is_none());
    }

    /// Opens a loopback connection on `ip` and checks both ends are listed
    /// as established with their remote address. Skips when `ip` is not
    /// configured (e.g. IPv6 disabled).
    fn established_loopback(ip: &str) {
        let Ok(listener) = TcpListener::bind((ip, 0)) else {
            return;
        };
        let server = listener.local_addr().unwrap();
        let stream = std::net::TcpStream::connect(server).unwrap();
        let client = stream.local_addr().unwrap();
        let (_accepted, _) = listener.accept().unwrap();

        let conns = connections().unwrap();
        let outgoing = conns
            .iter()
            .find(|c| c.protocol == Protocol::Tcp && c.local == client)
            .expect("client end not found");
        assert_eq!(outgoing.state, Some(TcpState::Established));
        assert_eq!(outgoing.remote, Some(server));
        assert_eq!(outgoing.pid, std::process::id());
    }

    #[test]
    fn finds_established_connections() {
        established_loopback("127.0.0.1");
        established_loopback("::1");
    }

    #[test]
    fn finds_own_udp_sockets() {
        let v4 = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let v6 = std::net::UdpSocket::bind("[::1]:0").ok();
        let conns = connections().unwrap();
        for addr in std::iter::once(v4.local_addr().unwrap())
            .chain(v6.iter().map(|s| s.local_addr().unwrap()))
        {
            let mine = conns
                .iter()
                .find(|c| c.protocol == Protocol::Udp && c.local == addr)
                .unwrap_or_else(|| panic!("{addr} not found"));
            assert_eq!(mine.pid, std::process::id());
            assert!(mine.remote.is_none() && mine.state.is_none());
        }
    }

    #[test]
    fn ipv6_endpoints_preserve_interface_scopes() {
        let loopback = Ipv6Addr::LOCALHOST.octets();
        assert_eq!(
            scoped_v6(loopback, u32::from(8080u16.to_be()), 0),
            "[::1]:8080".parse().unwrap(),
            "control: unscoped loopback"
        );
        for scope in [1, 7, u32::MAX] {
            let address = "fe80::1234".parse::<Ipv6Addr>().unwrap().octets();
            let expected: SocketAddr = format!("[fe80::1234%{scope}]:443").parse().unwrap();
            let actual = scoped_v6(address, u32::from(443u16.to_be()), scope);
            eprintln!("EXPECTED: {expected}; ACTUAL: {actual}");
            assert_eq!(
                actual, expected,
                "interface identity must survive TCP/UDP table decoding"
            );
        }
        assert_eq!(scoped_v6(loopback, 0, 0).port(), 0);
    }

    #[test]
    fn maps_every_tcp_state() {
        let states: Vec<TcpState> = (1..=13).map(TcpState::from_raw).collect();
        assert_eq!(
            states,
            [
                TcpState::Closed,
                TcpState::Listen,
                TcpState::SynSent,
                TcpState::SynReceived,
                TcpState::Established,
                TcpState::FinWait1,
                TcpState::FinWait2,
                TcpState::CloseWait,
                TcpState::Closing,
                TcpState::LastAck,
                TcpState::TimeWait,
                TcpState::DeleteTcb,
                TcpState::Unknown(13),
            ]
        );
    }

    #[test]
    fn fetch_reports_errors_and_empty_tables() {
        const ERROR_INVALID_PARAMETER: u32 = 87;
        let hint = AtomicUsize::new(0);
        let err = fetch(&hint, |_, _| ERROR_INVALID_PARAMETER).unwrap_err();
        assert!(err.is_invalid_parameter(), "{err}");

        // Success on the first (size-only) call leaves the buffer empty.
        let buf = fetch(&hint, |_, _| 0).unwrap();
        assert!(buf.is_empty());
        // SAFETY: an empty buffer is never dereferenced.
        let parsed = unsafe { rows::<MIB_TCPROW_OWNER_PID>(&buf, 4) };
        assert!(parsed.is_empty());
    }

    #[test]
    fn fetch_offers_the_size_that_sufficed_last_time() {
        let hint = AtomicUsize::new(0);
        let mut calls = Vec::new();
        // A table of 100 bytes: too small a buffer is refused with its size.
        let mut table = |p: Option<*mut core::ffi::c_void>, size: &mut u32| {
            calls.push((p.is_some(), *size));
            if *size < 100 {
                *size = 100;
                ERROR_INSUFFICIENT_BUFFER.0
            } else {
                0
            }
        };
        let first = fetch(&hint, &mut table).unwrap();
        let second = fetch(&hint, &mut table).unwrap();
        assert_eq!(first.len(), second.len());
        // Size query and fill the first time; a single call afterwards.
        assert_eq!(calls.len(), 3, "{calls:?}");
        assert!(!calls[0].0 && calls[1].0 && calls[2].0);
        assert_eq!(hint.load(Ordering::Relaxed), first.len() * 8);
    }
}

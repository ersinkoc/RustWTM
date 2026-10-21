//! TCP/UDP endpoints with their owning process.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR, WIN32_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, GetExtendedUdpTable, MIB_TCP_STATE_DELETE_TCB, MIB_TCP6ROW_OWNER_PID,
    MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_LH, MIB_TCPROW_LH_0, MIB_TCPROW_OWNER_PID,
    MIB_TCPTABLE_OWNER_PID, MIB_UDP6ROW_OWNER_PID, MIB_UDP6TABLE_OWNER_PID, MIB_UDPROW_OWNER_PID,
    MIB_UDPTABLE_OWNER_PID, SetTcpEntry, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
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

/// Bytes the TCPv4, TCPv6, UDPv4 and UDPv6 tables needed last time.
static SIZE_HINTS: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

/// Calls a `GetExtended*Table` function until the buffer is large enough.
///
/// Every call walks the whole kernel table, a size-only one included, so
/// the first call already offers a buffer as large as the one that sufficed
/// last time (`hint`, in bytes): one call per table instead of two.
fn fetch(
    hint: &AtomicUsize,
    mut call: impl FnMut(Option<*mut core::ffi::c_void>, &mut u32) -> u32,
) -> Result<Vec<u64>> {
    let mut buf: Vec<u64> = vec![0; hint.load(Ordering::Relaxed).div_ceil(8)];
    let mut size = (buf.len() * 8) as u32;
    loop {
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
        buf.resize((size as usize + 4096).div_ceil(8), 0);
        size = (buf.len() * 8) as u32;
    }
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
    for r in unsafe {
        rows::<MIB_TCPROW_OWNER_PID>(&tcp4, std::mem::offset_of!(MIB_TCPTABLE_OWNER_PID, table))
    } {
        let state = TcpState::from_raw(r.dwState);
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
    for r in unsafe {
        rows::<MIB_TCP6ROW_OWNER_PID>(&tcp6, std::mem::offset_of!(MIB_TCP6TABLE_OWNER_PID, table))
    } {
        let state = TcpState::from_raw(r.dwState);
        out.push(Connection {
            protocol: Protocol::Tcp,
            local: SocketAddr::new(
                IpAddr::V6(Ipv6Addr::from(r.ucLocalAddr)),
                port(r.dwLocalPort),
            ),
            remote: (state != TcpState::Listen).then(|| {
                SocketAddr::new(
                    IpAddr::V6(Ipv6Addr::from(r.ucRemoteAddr)),
                    port(r.dwRemotePort),
                )
            }),
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
            local: SocketAddr::new(
                IpAddr::V6(Ipv6Addr::from(r.ucLocalAddr)),
                port(r.dwLocalPort),
            ),
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

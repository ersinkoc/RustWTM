//! Network bytes per process from the kernel's TCP/IP and UDP/IP events
//! (a system logger session). Needs administrator rights (the service has
//! them).
//!
//! The manifest-based Microsoft-Windows-Kernel-Network provider no longer
//! reports sends and receives on current Windows builds, so the classic
//! kernel events are used. Each send and receive event starts with the
//! owning process ID and the byte count, for IPv4 and IPv6 alike; the event
//! header's own process ID is whoever was running when the packet was
//! handled, so the payload's is used.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use windows::Win32::System::Diagnostics::Etw::{
    EVENT_RECORD, EVENT_TRACE_FLAG_NETWORK_TCPIP, TcpIpGuid, UdpIpGuid,
};

use crate::etw::{self, Session};
use crate::fasthash::FastMap;

/// Send and receive opcodes, the same for TCP and UDP: IPv4, then IPv6.
const OPCODE_SEND: u8 = 10;
const OPCODE_RECEIVE: u8 = 11;
const OPCODE_SEND_V6: u8 = 26;
const OPCODE_RECEIVE_V6: u8 = 27;

/// Bytes received and sent per process since the last [`NetTracer::take`].
pub type Traffic = HashMap<u32, (u64, u64)>;

/// What the consumer counts into: updated on every packet, so hashed with
/// the cheap hasher, and copied into a [`Traffic`] once per take.
type Counts = FastMap<u32, (u64, u64)>;

/// Which way a packet went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Received,
    Sent,
}

/// Process, byte count and direction of a TCP or UDP transfer event.
fn transfer(opcode: u8, payload: &[u8]) -> Option<(u32, u64, Direction)> {
    let direction = match opcode {
        OPCODE_SEND | OPCODE_SEND_V6 => Direction::Sent,
        OPCODE_RECEIVE | OPCODE_RECEIVE_V6 => Direction::Received,
        _ => return None,
    };
    let pid = etw::u32_at(payload, 0)?;
    let bytes = etw::u32_at(payload, 4)?;
    Some((pid, u64::from(bytes), direction))
}

fn count(traffic: &mut Counts, pid: u32, bytes: u64, direction: Direction) {
    let (received, sent) = traffic.entry(pid).or_default();
    let total = match direction {
        Direction::Received => received,
        Direction::Sent => sent,
    };
    *total = total.saturating_add(bytes);
}

/// Counts network bytes per process while it lives.
#[derive(Debug)]
pub struct NetTracer {
    traffic: Arc<Mutex<Counts>>,
    _session: Session,
}

impl NetTracer {
    /// `name` names the ETW session (one per process at a time).
    pub fn start(name: &str) -> crate::Result<Self> {
        let session = Session::start_kernel(name, EVENT_TRACE_FLAG_NETWORK_TCPIP.0)?;
        let traffic = Arc::new(Mutex::new(Counts::default()));
        let sink = Arc::clone(&traffic);
        session.consume(move |record: &EVENT_RECORD| {
            let header = &record.EventHeader;
            if header.ProviderId != TcpIpGuid && header.ProviderId != UdpIpGuid {
                return;
            }
            let opcode = header.EventDescriptor.Opcode;
            if let Some((pid, bytes, direction)) = transfer(opcode, etw::user_data(record)) {
                let mut traffic = sink.lock().unwrap_or_else(PoisonError::into_inner);
                count(&mut traffic, pid, bytes, direction);
            }
        })?;
        Ok(Self {
            traffic,
            _session: session,
        })
    }

    /// Everything counted since the previous call.
    pub fn take(&self) -> Traffic {
        let mut counts = self.traffic.lock().unwrap_or_else(PoisonError::into_inner);
        // Draining keeps the map's table for the next interval's packets.
        let mut out = Traffic::with_capacity(counts.len());
        out.extend(counts.drain());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(pid: u32, size: u32) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&pid.to_le_bytes());
        d.extend_from_slice(&size.to_le_bytes());
        d.extend_from_slice(&[0; 16]); // addresses and ports follow
        d
    }

    #[test]
    fn classifies_sends_and_receives() {
        let d = payload(1234, 1500);
        for opcode in [OPCODE_SEND, OPCODE_SEND_V6] {
            assert_eq!(transfer(opcode, &d), Some((1234, 1500, Direction::Sent)));
        }
        for opcode in [OPCODE_RECEIVE, OPCODE_RECEIVE_V6] {
            assert_eq!(
                transfer(opcode, &d),
                Some((1234, 1500, Direction::Received))
            );
        }
        // Connect, disconnect and the receive-side copy carry no new bytes.
        for opcode in [12, 13, 18] {
            assert_eq!(transfer(opcode, &d), None);
        }
        assert_eq!(transfer(10, &d[..6]), None);
    }

    #[test]
    fn counts_per_process_and_direction() {
        let mut t = Counts::default();
        count(&mut t, 7, 100, Direction::Received);
        count(&mut t, 7, 50, Direction::Sent);
        count(&mut t, 7, 1, Direction::Received);
        count(&mut t, 9, u64::MAX, Direction::Sent);
        count(&mut t, 9, 1, Direction::Sent);
        assert_eq!(t[&7], (101, 50));
        assert_eq!(t[&9], (0, u64::MAX));
    }

    /// 64 KiB over a loopback TCP connection of our own.
    fn loopback_traffic() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            use std::io::Write;
            let mut s = std::net::TcpStream::connect(addr).unwrap();
            s.write_all(&[0; 64 * 1024]).unwrap();
        });
        let (mut server, _) = listener.accept().unwrap();
        let mut sink = Vec::new();
        std::io::Read::read_to_end(&mut server, &mut sink).unwrap();
        client.join().unwrap();
    }

    /// Needs an elevated test run to see traffic; otherwise checks the error.
    #[test]
    fn starts_when_elevated() {
        // Sessions of crashed earlier runs would use up the kernel's slots.
        crate::etw::stop_orphans("RustWTM-test-net");
        let name = format!("RustWTM-test-net-{}", std::process::id());
        let elevated = crate::shell::is_elevated().unwrap_or(false);
        // Windows runs at most eight kernel loggers; in a full workspace run
        // the other trace tests and the engine tests may hold them briefly.
        let busy = windows::Win32::Foundation::ERROR_NO_SYSTEM_RESOURCES.to_hresult();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let started = loop {
            match NetTracer::start(&name) {
                Err(crate::Error::Win32(e))
                    if e.code() == busy && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                other => break other,
            }
        };
        match started {
            Ok(tracer) => {
                assert!(elevated);
                // Real-time events reach us only once the consumer is
                // attached, so traffic is made again until it shows up.
                let me = std::process::id();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                let mut seen = (0, 0);
                std::thread::sleep(std::time::Duration::from_millis(500));
                while std::time::Instant::now() < deadline && (seen.0 == 0 || seen.1 == 0) {
                    loopback_traffic();
                    // A datagram that leaves the machine (TEST-NET-1, discard port).
                    if let Ok(udp) = std::net::UdpSocket::bind("0.0.0.0:0") {
                        let _ = udp.send_to(&[0; 512], "192.0.2.1:9");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let (r, s) = tracer.take().get(&me).copied().unwrap_or_default();
                    seen = (seen.0 + r, seen.1 + s);
                }
                assert!(seen.0 > 0 && seen.1 > 0, "{seen:?}");
            }
            Err(e) => {
                if elevated {
                    panic!("elevated but could not trace: {e}");
                }
                assert!(e.is_access_denied(), "{e}");
            }
        }
    }
}

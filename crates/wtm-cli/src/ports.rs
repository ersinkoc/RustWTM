//! `wtm ports` and `wtm free-port`: who listens on which port, and ending
//! whatever keeps a port busy.

use std::io::Write;
use std::process::ExitCode;

use wtm_core::control::ProcessRef;
use wtm_core::ports::{self, PortUse};
use wtm_core::{ProcessInfo, Protocol};

fn process(procs: &[ProcessInfo], pid: u32) -> Option<&ProcessInfo> {
    procs.iter().find(|p| p.pid == pid)
}

/// The listening ports as a table; `udp` adds bound UDP ports, `known` keeps
/// only the ports of well-known tools and servers.
pub fn render(
    uses: &[PortUse],
    procs: &[ProcessInfo],
    udp: bool,
    known: bool,
    out: &mut impl Write,
) -> std::io::Result<()> {
    let shown: Vec<&PortUse> = uses
        .iter()
        .filter(|u| udp || u.protocol == Protocol::Tcp)
        .filter(|u| !known || u.known().is_some())
        .collect();
    if shown.is_empty() {
        return writeln!(out, "No listening ports.");
    }
    writeln!(
        out,
        "{:>6} {:<5} {:>7}  {:<26} {:<36} {:<13} {:>7}  ADDRESS",
        "PORT", "PROTO", "PID", "PROGRAM", "KNOWN AS", "REACH", "CLIENTS"
    )?;
    for u in shown {
        let name = process(procs, u.pid).map_or("?", |p| p.name.as_str());
        let clients = match u.protocol {
            Protocol::Tcp => u.connections.to_string(),
            Protocol::Udp => String::new(),
        };
        writeln!(
            out,
            "{:>6} {:<5} {:>7}  {:<26} {:<36} {:<13} {:>7}  {}",
            u.port,
            format!("{:?}", u.protocol).to_uppercase(),
            u.pid,
            name,
            u.known().map_or("", |k| k.service),
            u.exposure().label(),
            clients,
            u.address_text()
        )?;
    }
    Ok(())
}

/// Ends the processes that hold `port` (only lists them without `yes`).
pub fn free_port(
    port: u16,
    yes: bool,
    uses: &[PortUse],
    procs: &[ProcessInfo],
    end: &mut dyn FnMut(&ProcessRef) -> Result<(), String>,
    out: &mut impl Write,
) -> anyhow::Result<ExitCode> {
    let owners = ports::port_owners(uses, port);
    if owners.is_empty() {
        writeln!(out, "Port {port} is free.")?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut failed = 0;
    let mut endable = 0;
    for pid in &owners {
        let Some(p) = process(procs, *pid) else {
            writeln!(out, "{pid:>7}  ?  (exited)")?;
            continue;
        };
        let result = if let Some(reason) = ports::end_blocked(p.pid, &p.name) {
            failed += 1;
            format!("not ended: {reason}")
        } else if yes {
            match end(&ProcessRef::from(p)) {
                Ok(()) => "ended".to_owned(),
                Err(e) => {
                    failed += 1;
                    format!("failed: {e}")
                }
            }
        } else {
            endable += 1;
            match ports::relay_hint(&p.name) {
                Some(hint) => format!("would be ended. {hint}"),
                None => "would be ended".to_owned(),
            }
        };
        writeln!(out, "{:>7}  {:<28} {result}", p.pid, p.name)?;
    }
    if !yes && endable > 0 {
        writeln!(
            out,
            "\nRun `wtm free-port {port} --yes` to end {} now.",
            if endable == 1 { "it" } else { "them" }
        )?;
    }
    Ok(if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wtm_core::{Connection, TcpState};

    fn proc(pid: u32, name: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: 0,
            name: name.into(),
            cpu_percent: 0.0,
            cpu_time: std::time::Duration::ZERO,
            working_set: 0,
            private_working_set: 0,
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
            create_time: 1,
        }
    }

    fn uses() -> Vec<PortUse> {
        let listen = |local: &str, pid| Connection {
            protocol: Protocol::Tcp,
            local: local.parse().unwrap(),
            remote: None,
            state: Some(TcpState::Listen),
            pid,
        };
        ports::port_uses(&[
            listen("0.0.0.0:5173", 10),
            listen("127.0.0.1:41000", 10),
            listen("0.0.0.0:445", 4),
            Connection {
                protocol: Protocol::Udp,
                local: "0.0.0.0:5353".parse().unwrap(),
                remote: None,
                state: None,
                pid: 20,
            },
        ])
    }

    fn procs() -> Vec<ProcessInfo> {
        vec![
            proc(4, "System"),
            proc(10, "node.exe"),
            proc(20, "chrome.exe"),
        ]
    }

    fn text(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut out = Vec::new();
        f(&mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn ports_are_listed_with_their_programs() {
        let all = text(|o| render(&uses(), &procs(), false, false, o).unwrap());
        assert!(all.contains("5173 TCP"), "{all}");
        assert!(all.contains("Vite"), "{all}");
        assert!(all.contains("All networks"), "{all}");
        assert!(all.contains("41000"), "{all}");
        assert!(!all.contains("5353"), "{all}");
        let udp = text(|o| render(&uses(), &procs(), true, false, o).unwrap());
        assert!(udp.contains("5353 UDP"), "{udp}");
        let known = text(|o| render(&uses(), &procs(), false, true, o).unwrap());
        assert!(!known.contains("41000"), "{known}");
        let none = text(|o| render(&[], &[], false, false, o).unwrap());
        assert_eq!(none.trim(), "No listening ports.");
    }

    #[test]
    fn a_port_is_freed_only_with_yes() {
        let mut ended = Vec::new();
        let mut end = |t: &ProcessRef| {
            ended.push(t.pid);
            Ok(())
        };
        let preview = text(|o| {
            let code = free_port(5173, false, &uses(), &procs(), &mut end, o).unwrap();
            assert_eq!(code, ExitCode::SUCCESS);
        });
        assert!(preview.contains("node.exe"), "{preview}");
        assert!(preview.contains("would be ended"), "{preview}");
        let done = text(|o| {
            free_port(5173, true, &uses(), &procs(), &mut end, o).unwrap();
        });
        assert!(done.contains("ended"), "{done}");
        assert_eq!(ended, [10]);

        let free = text(|o| {
            free_port(9999, true, &uses(), &procs(), &mut |_| Ok(()), o).unwrap();
        });
        assert_eq!(free.trim(), "Port 9999 is free.");
        let kernel = text(|o| {
            let code = free_port(445, true, &uses(), &procs(), &mut |_| Ok(()), o).unwrap();
            assert_eq!(code, ExitCode::FAILURE);
        });
        assert!(kernel.contains("not ended: protected"), "{kernel}");
        let failing = text(|o| {
            let code = free_port(
                5173,
                true,
                &uses(),
                &procs(),
                &mut |_| Err("denied".into()),
                o,
            )
            .unwrap();
            assert_eq!(code, ExitCode::FAILURE);
        });
        assert!(failing.contains("failed: denied"), "{failing}");
    }
}

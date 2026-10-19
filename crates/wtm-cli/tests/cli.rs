//! Runs the real `wtm` binary against short-lived child processes.

use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_ping() -> Guard {
    let child = Guard(
        Command::new("cmd.exe")
            .raw_arg("/d /c ping -n 60 127.0.0.1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    // Give the process a moment to show up in snapshots.
    std::thread::sleep(Duration::from_millis(200));
    child
}

fn wtm(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wtm"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn list_prints_summary_header_and_rows() {
    let o = wtm(&["list", "--sample-ms", "100", "-n", "5", "--sort", "mem"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("CPU"), "{out}");
    assert!(lines[1].contains("PID") && lines[1].contains("NAME"));
    assert_eq!(lines.len(), 7, "{out}");
}

#[test]
fn info_shows_details() {
    let child = spawn_ping();
    let pid = child.0.id().to_string();
    let o = wtm(&["info", &pid]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains(&format!("PID          {pid}")), "{out}");
    assert!(out.to_lowercase().contains("cmd.exe"));
    assert!(out.contains("Protected    no"));
}

#[test]
fn control_commands_round_trip() {
    let child = spawn_ping();
    let pid = child.0.id().to_string();
    for args in [
        vec!["priority", &pid, "below-normal"],
        vec!["affinity", &pid, "0"],
        vec!["affinity", &pid],
        vec!["eco", &pid, "on"],
        vec!["eco", &pid, "auto"],
        vec!["suspend", &pid],
        vec!["resume", &pid],
        vec!["limit", &pid, "--cpu", "50", "--memory", "1G", "--detach"],
    ] {
        let o = wtm(&args);
        assert!(o.status.success(), "{args:?}: {}{}", stdout(&o), stderr(&o));
    }
    let info = stdout(&wtm(&["info", &pid]));
    assert!(info.contains("Priority     BelowNormal"), "{info}");
    assert!(
        info.contains("Affinity     0\n") || info.contains("Affinity     0\r\n"),
        "{info}"
    );
    assert!(info.contains("EcoQoS       SystemManaged"), "{info}");
}

#[test]
fn kill_tree_terminates_the_child() {
    let mut child = spawn_ping();
    let pid = child.0.id().to_string();
    let o = wtm(&["kill", &pid, "--tree"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("kill-tree"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "child still running");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn refusals_and_errors_fail_with_messages() {
    // Protected processes are refused.
    let o = wtm(&["kill", "csrss"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("protected"), "{}", stderr(&o));

    // Unknown targets.
    let o = wtm(&["suspend", "no-such-process-xyz"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("no process matches"));

    // Invalid arguments are rejected by the parser.
    assert!(!wtm(&["limit", "4", "--cpu", "0"]).status.success());
    assert!(!wtm(&["affinity", "4", "64"]).status.success());

    // limit without any limit.
    let child = spawn_ping();
    let o = wtm(&["limit", &child.0.id().to_string()]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--cpu and/or --memory"));
}

#[test]
fn top_and_history_fail_gracefully_without_terminal_or_service() {
    // Detached from any console, the TUI cannot enter raw mode and must
    // error out instead of waiting for keys. Guarded by a timeout.
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let mut top = Guard(
        Command::new(env!("CARGO_BIN_EXE_wtm"))
            .arg("top")
            .creation_flags(DETACHED_PROCESS)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(s) = top.0.try_wait().unwrap() {
            break s;
        }
        assert!(
            Instant::now() < deadline,
            "wtm top did not exit without a console"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(!status.success());
    // History needs the service (not running in tests).
    let o = wtm(&["history"]);
    if !o.status.success() {
        assert!(stderr(&o).contains("service"), "{}", stderr(&o));
    }
}

//! End-to-end tests against real child processes.

use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use wtm_core::control::{self, ControlError, EcoQos, Limiter, Limits, PriorityClass, ProcessRef};
use wtm_core::{ProcessInfo, Sampler};

/// Kills the child if a test fails half-way.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_cmd(script: &str) -> ChildGuard {
    let child = Command::new("cmd.exe")
        .raw_arg(format!("/d /c {script}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cmd.exe");
    ChildGuard(child)
}

fn snapshot() -> Vec<ProcessInfo> {
    Sampler::new().sample().unwrap().processes
}

fn find(pid: u32) -> ProcessRef {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(p) = snapshot().iter().find(|p| p.pid == pid) {
            return p.into();
        }
        assert!(Instant::now() < deadline, "process {pid} never appeared");
        sleep(Duration::from_millis(50));
    }
}

fn is_running(pid: u32, create_time: i64) -> bool {
    snapshot()
        .iter()
        .any(|p| p.pid == pid && p.create_time == create_time)
}

#[test]
fn suspend_resume_and_kill() {
    let mut child = spawn_cmd("ping -n 60 127.0.0.1");
    let target = find(child.0.id());

    control::suspend(&target).unwrap();
    control::resume(&target).unwrap();
    control::kill(&target).unwrap();

    let status = child.0.wait().unwrap();
    assert_eq!(status.code(), Some(1));
}

#[test]
fn priority_affinity_and_eco_round_trip() {
    let child = spawn_cmd("ping -n 60 127.0.0.1");
    let target = find(child.0.id());

    control::set_priority(&target, PriorityClass::BelowNormal).unwrap();
    control::set_affinity(&target, 0b1).unwrap();
    control::set_eco_qos(&target, EcoQos::On).unwrap();

    let d = control::inspect(&target).unwrap();
    assert_eq!(d.priority, Some(PriorityClass::BelowNormal));
    assert_eq!(d.affinity.map(|(m, _)| m), Some(0b1));
    assert_eq!(d.eco_qos, Some(EcoQos::On));
    assert!(d.path.unwrap().to_lowercase().ends_with("cmd.exe"));
    assert_eq!(d.critical, Some(false));

    control::set_eco_qos(&target, EcoQos::SystemManaged).unwrap();
    assert_eq!(
        control::inspect(&target).unwrap().eco_qos,
        Some(EcoQos::SystemManaged)
    );
}

#[test]
fn refuses_protected_and_stale_targets() {
    let procs = snapshot();
    for name in ["System", "csrss.exe", "lsass.exe"] {
        let p = procs
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .unwrap();
        let r = control::kill(&p.into());
        assert!(
            matches!(r, Err(ControlError::Protected { .. })),
            "{name}: {r:?}"
        );
    }
    let me = procs.iter().find(|p| p.pid == std::process::id()).unwrap();
    assert!(matches!(
        control::suspend(&me.into()),
        Err(ControlError::Protected { .. })
    ));

    let child = spawn_cmd("ping -n 60 127.0.0.1");
    let mut stale = find(child.0.id());
    stale.create_time += 1;
    assert!(matches!(
        control::kill(&stale),
        Err(ControlError::Stale { .. })
    ));
}

#[test]
fn kill_tree_kills_descendants() {
    let root = spawn_cmd("ping -n 60 127.0.0.1");
    let root_ref = find(root.0.id());

    let deadline = Instant::now() + Duration::from_secs(5);
    let ping = loop {
        let procs = snapshot();
        if let Some(p) = wtm_core::descendants(root_ref.pid, root_ref.create_time, &procs).first() {
            break ProcessRef::from(*p);
        }
        assert!(Instant::now() < deadline, "ping child never appeared");
        sleep(Duration::from_millis(50));
    };
    assert!(ping.name.eq_ignore_ascii_case("PING.EXE"));

    let results = control::kill_tree(&root_ref, &snapshot());
    assert!(results.iter().all(|(_, r)| r.is_ok()), "{results:?}");
    sleep(Duration::from_millis(300));
    assert!(!is_running(ping.pid, ping.create_time));
    assert!(!is_running(root_ref.pid, root_ref.create_time));
}

fn measure_cpu(target: &ProcessRef, window: Duration) -> f32 {
    let mut s = Sampler::new();
    s.sample().unwrap();
    sleep(window);
    s.sample()
        .unwrap()
        .processes
        .iter()
        .find(|p| p.pid == target.pid)
        .map_or(0.0, |p| p.cpu_percent)
}

#[test]
fn job_hard_cap_throttles_busy_process() {
    let child = spawn_cmd("for /l %i in (0,0,1) do @rem");
    let target = find(child.0.id());

    let uncapped = measure_cpu(&target, Duration::from_millis(1000));
    assert!(uncapped > 0.2, "spinner should be busy, got {uncapped}%");

    let cap = (uncapped / 4.0).max(0.05);
    let mut limiter = Limiter::new();
    limiter
        .apply(
            &target,
            Limits {
                cpu_percent: Some(cap),
                memory_bytes: None,
            },
        )
        .unwrap();
    let capped = measure_cpu(&target, Duration::from_millis(1500));
    assert!(
        capped < uncapped / 2.0,
        "uncapped {uncapped}%, cap {cap}%, measured {capped}%"
    );

    limiter.lift(&target).unwrap();
    let lifted = measure_cpu(&target, Duration::from_millis(1000));
    assert!(
        lifted > capped,
        "lifted {lifted}% should exceed capped {capped}%"
    );
}

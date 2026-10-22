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

    // Threads enter the suspended state asynchronously, once scheduled.
    let becomes = |frozen: bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let now = snapshot()
                .iter()
                .find(|p| p.pid == target.pid)
                .map(|p| p.suspended);
            if now == Some(frozen) {
                return;
            }
            assert!(Instant::now() < deadline, "suspended is {now:?}");
            sleep(Duration::from_millis(20));
        }
    };
    becomes(false);
    // Suspensions stack; one resume still thaws the process.
    control::suspend(&target).unwrap();
    control::suspend(&target).unwrap();
    control::suspend(&target).unwrap();
    becomes(true);
    control::resume(&target).unwrap();
    becomes(false);
    // Resuming a running process is harmless.
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

    control::set_core_affinity(&target, wtm_core::cpu::CoreSelection::All).unwrap();
    let restored = control::inspect(&target).unwrap().affinity.unwrap();
    assert_eq!(restored.0, restored.1);

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

    // The name a caller gives is only a claim: a process whose image has a
    // protected name stays protected under another name, and without a
    // creation time. A copy of cmd.exe stands in for the real thing.
    let dir = std::env::temp_dir().join(format!("wtm-impostor-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let image = dir.join("fontdrvhost.exe");
    std::fs::copy(r"C:\Windows\System32\cmd.exe", &image).unwrap();
    let impostor = ChildGuard(
        Command::new(&image)
            .raw_arg("/d /c ping -n 60 127.0.0.1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let claimed = ProcessRef {
        pid: impostor.0.id(),
        create_time: 0,
        name: "notepad.exe".into(),
    };
    let r = control::kill(&claimed);
    assert!(matches!(r, Err(ControlError::Protected { .. })), "{r:?}");
    drop(impostor);
    let _ = std::fs::remove_dir_all(&dir);

    let child = spawn_cmd("ping -n 60 127.0.0.1");
    let mut stale = find(child.0.id());
    stale.create_time += 1;
    assert!(matches!(
        control::kill(&stale),
        Err(ControlError::Stale { .. })
    ));
    assert!(matches!(
        control::set_core_affinity(&stale, wtm_core::cpu::CoreSelection::All),
        Err(ControlError::Stale { .. })
    ));
    assert!(matches!(
        control::set_core_affinity(&me.into(), wtm_core::cpu::CoreSelection::All),
        Err(ControlError::Protected { .. })
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

#[test]
fn kill_tree_spares_the_children_of_a_reused_pid() {
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

    // The client still holds an earlier process that had this PID; the
    // PID's current owner and its child are not that process's tree.
    let earlier = ProcessRef {
        create_time: root_ref.create_time - 1,
        ..root_ref.clone()
    };
    let results = control::kill_tree(&earlier, &snapshot());
    assert!(
        matches!(results[..], [(_, Err(ControlError::Stale { .. }))]),
        "{results:?}"
    );
    assert!(is_running(ping.pid, ping.create_time));
    control::kill(&ping).unwrap();
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

#[test]
fn missing_pid_is_not_found() {
    // PIDs are multiples of 4 and never get anywhere near this value.
    let ghost = ProcessRef {
        pid: u32::MAX - 3,
        create_time: 0,
        name: "ghost.exe".into(),
    };
    let r = control::resume(&ghost);
    assert!(
        matches!(r, Err(ControlError::NotFound { pid }) if pid == ghost.pid),
        "{r:?}"
    );
    assert!(matches!(
        control::image_path(&ghost),
        Err(ControlError::NotFound { .. })
    ));
}

#[test]
fn image_path_without_identity_check() {
    let child = spawn_cmd("ping -n 60 127.0.0.1");
    let mut target = find(child.0.id());
    // create_time 0 skips the PID-reuse check.
    target.create_time = 0;
    let path = control::image_path(&target).unwrap();
    assert!(path.to_lowercase().ends_with("cmd.exe"), "{path}");
}

#[test]
fn invalid_affinity_is_a_sys_error() {
    let child = spawn_cmd("ping -n 60 127.0.0.1");
    let target = find(child.0.id());
    let r = control::set_affinity(&target, 0);
    assert!(matches!(r, Err(ControlError::Sys { .. })), "{r:?}");
}

#[test]
fn own_priority_and_debug_privilege() {
    // Re-applies the current priority, so the test process is unchanged.
    let me = ProcessRef {
        pid: std::process::id(),
        create_time: 0,
        name: String::new(),
    };
    let current = control::inspect(&me).unwrap().priority.unwrap();
    control::set_own_priority(current).unwrap();
    // True only when elevated; either way it must not panic.
    let _ = control::enable_debug_privilege();
}

#[test]
fn limiter_tracks_lifts_and_forgets_targets() {
    let a = spawn_cmd("ping -n 60 127.0.0.1");
    let b = spawn_cmd("ping -n 60 127.0.0.1");
    let (a_ref, b_ref) = (find(a.0.id()), find(b.0.id()));
    let mut limiter = Limiter::new();
    // Nothing applied yet: lifting is refused, not reported as done.
    assert!(matches!(
        limiter.lift(&a_ref),
        Err(ControlError::NotLimited { .. })
    ));

    let memory_only = Limits {
        cpu_percent: None,
        memory_bytes: Some(512 << 20),
    };
    let both = Limits {
        cpu_percent: Some(50.0),
        ..memory_only
    };
    limiter.apply(&a_ref, memory_only).unwrap();
    limiter.apply(&b_ref, both).unwrap();
    assert_eq!(limiter.limited().count(), 2);

    // Forgetting keeps the limits but drops the bookkeeping.
    limiter.forget(&b_ref);
    let lifted = limiter.lift_all();
    assert_eq!(lifted.len(), 1);
    assert_eq!(lifted[0].0, a_ref);
    assert!(lifted[0].1.is_ok(), "{lifted:?}");
    assert_eq!(limiter.limited().count(), 0);
}

#[test]
fn memory_only_limit_and_lift() {
    let child = spawn_cmd("ping -n 60 127.0.0.1");
    let target = find(child.0.id());
    let mut limiter = Limiter::new();
    let only_memory = Limits {
        cpu_percent: None,
        memory_bytes: Some(512 << 20),
    };
    limiter.apply(&target, only_memory).unwrap();
    // Switch to CPU only, then lift everything.
    limiter
        .apply(
            &target,
            Limits {
                cpu_percent: Some(25.0),
                memory_bytes: None,
            },
        )
        .unwrap();
    limiter.lift(&target).unwrap();
}

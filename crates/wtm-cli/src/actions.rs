//! Process control subcommands.

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use anyhow::bail;
use wtm_core::control::{
    self, ControlResult, EcoQos, Limiter, Limits, PriorityClass, ProcessDetails, ProcessRef,
};
use wtm_core::{ProcessInfo, Sampler};

use crate::{parse, table};

fn snapshot() -> anyhow::Result<Vec<ProcessInfo>> {
    Ok(Sampler::new().sample()?.processes)
}

/// Resolves PIDs or image names (`.exe` optional, case-insensitive).
fn resolve(specs: &[String], procs: &[ProcessInfo]) -> anyhow::Result<Vec<ProcessRef>> {
    Ok(matching(specs, procs)?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// The processes `specs` select, each once, in order of first mention.
fn matching<'a>(
    specs: &[String],
    procs: &'a [ProcessInfo],
) -> anyhow::Result<Vec<&'a ProcessInfo>> {
    let mut out: Vec<&ProcessInfo> = Vec::new();
    for spec in specs {
        let matches: Vec<&ProcessInfo> = match spec.parse::<u32>() {
            Ok(pid) => procs.iter().filter(|p| p.pid == pid).collect(),
            Err(_) => {
                // Case-insensitive beyond ASCII too: `çizim` is Çizim.exe.
                let spec = spec.to_lowercase();
                let with_exe = format!("{spec}.exe");
                procs
                    .iter()
                    .filter(|p| {
                        let name = p.name.to_lowercase();
                        name == spec || name == with_exe
                    })
                    .collect()
            }
        };
        if matches.is_empty() {
            bail!("no process matches '{spec}'");
        }
        for p in matches {
            if !out.iter().any(|r| r.pid == p.pid) {
                out.push(p);
            }
        }
    }
    Ok(out)
}

/// Prints one result line; returns whether it succeeded.
fn report(verb: &str, target: &ProcessRef, result: ControlResult<()>) -> bool {
    match result {
        Ok(()) => {
            println!("OK    {verb:<10} {:>7}  {}", target.pid, target.name);
            true
        }
        Err(e) => {
            eprintln!("FAIL  {verb:<10} {:>7}  {}: {e}", target.pid, target.name);
            false
        }
    }
}

fn exit_code(all_ok: bool) -> ExitCode {
    if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Runs `action` on every resolved target.
fn for_each(
    specs: &[String],
    verb: &str,
    action: impl Fn(&ProcessRef) -> ControlResult<()>,
) -> anyhow::Result<ExitCode> {
    let targets = resolve(specs, &snapshot()?)?;
    let mut ok = true;
    for t in &targets {
        ok &= report(verb, t, action(t));
    }
    Ok(exit_code(ok))
}

pub fn kill(specs: &[String], tree: bool) -> anyhow::Result<ExitCode> {
    if !tree {
        return for_each(specs, "kill", control::kill);
    }
    let procs = snapshot()?;
    let mut ok = true;
    for root in outermost(resolve(specs, &procs)?, &procs) {
        for (t, r) in control::kill_tree(&root, &procs) {
            ok &= report("kill-tree", &t, r);
        }
    }
    Ok(exit_code(ok))
}

/// The `roots` that are no descendant of another one: ending a tree ends
/// the matches inside it, which would otherwise be ended (and reported as
/// failing) a second time.
fn outermost(roots: Vec<ProcessRef>, procs: &[ProcessInfo]) -> Vec<ProcessRef> {
    let inside: std::collections::HashSet<(u32, i64)> = roots
        .iter()
        .flat_map(|r| wtm_core::descendants(r.pid, r.create_time, procs))
        .map(|p| (p.pid, p.create_time))
        .collect();
    roots
        .into_iter()
        .filter(|r| !inside.contains(&(r.pid, r.create_time)))
        .collect()
}

pub fn suspend(specs: &[String]) -> anyhow::Result<ExitCode> {
    for_each(specs, "suspend", control::suspend)
}

pub fn resume(specs: &[String]) -> anyhow::Result<ExitCode> {
    for_each(specs, "resume", control::resume)
}

pub fn dump(specs: &[String], dir: &std::path::Path) -> anyhow::Result<ExitCode> {
    let mut ok = true;
    for t in resolve(specs, &snapshot()?)? {
        match control::create_dump(&t, dir) {
            Ok(path) => println!(
                "OK    dump       {:>7}  {}: {}",
                t.pid,
                t.name,
                path.display()
            ),
            Err(e) => {
                eprintln!("FAIL  dump       {:>7}  {}: {e}", t.pid, t.name);
                ok = false;
            }
        }
    }
    Ok(exit_code(ok))
}

pub fn trim(specs: &[String]) -> anyhow::Result<ExitCode> {
    for_each(specs, "free memory", control::trim_memory)
}

pub fn priority(specs: &[String], class: PriorityClass) -> anyhow::Result<ExitCode> {
    for_each(specs, "priority", |t| control::set_priority(t, class))
}

pub fn eco(specs: &[String], mode: EcoQos) -> anyhow::Result<ExitCode> {
    for_each(specs, "eco", |t| control::set_eco_qos(t, mode))
}

pub fn affinity(specs: &[String], cpus: Option<&str>) -> anyhow::Result<ExitCode> {
    let Some(cpus) = cpus else {
        let mut ok = true;
        for t in resolve(specs, &snapshot()?)? {
            match affinity_line(&t, control::inspect(&t).map(|d| d.affinity)) {
                Ok(line) => println!("{line}"),
                Err(line) => {
                    eprintln!("{line}");
                    ok = false;
                }
            }
        }
        return Ok(exit_code(ok));
    };
    let mask = parse::cpu_list(cpus).map_err(anyhow::Error::msg)?;
    for_each(specs, "affinity", |t| control::set_affinity(t, mask))
}

pub fn core_affinity(
    specs: &[String],
    selection: wtm_core::cpu::CoreSelection,
) -> anyhow::Result<ExitCode> {
    for_each(specs, selection.label(), |t| {
        control::set_core_affinity(t, selection)
    })
}

/// Report line for `affinity` without a CPU list: `Ok` for stdout, `Err` for stderr.
fn affinity_line(
    t: &ProcessRef,
    affinity: ControlResult<Option<(u64, u64)>>,
) -> Result<String, String> {
    match affinity {
        Ok(Some((mask, system))) => Ok(format!(
            "{:>7}  {:<24} cpus {} (system {})",
            t.pid,
            t.name,
            parse::format_cpu_list(mask),
            parse::format_cpu_list(system)
        )),
        Ok(None) => Err(format!("{:>7}  {}: affinity not readable", t.pid, t.name)),
        Err(e) => Err(format!("{:>7}  {}: {e}", t.pid, t.name)),
    }
}

pub fn info(specs: &[String], json: bool) -> anyhow::Result<ExitCode> {
    let procs = snapshot()?;
    if json {
        let (value, ok) = info_json(matching(specs, &procs)?, |p| control::inspect(&p.into()));
        serde_json::to_writer_pretty(std::io::stdout().lock(), &value)?;
        println!();
        return Ok(exit_code(ok));
    }
    let mut ok = true;
    for p in matching(specs, &procs)? {
        println!("PID          {}", p.pid);
        println!("Name         {}", p.name);
        println!("Parent PID   {}", p.parent_pid);
        println!("Session      {}", p.session_id);
        if let Some(up) = p
            .start_time()
            .and_then(|s| SystemTime::now().duration_since(s).ok())
        {
            println!("Uptime       {}", uptime(up));
        }
        println!("CPU time     {:.1}s", p.cpu_time.as_secs_f64());
        println!(
            "Memory       {} private WS, {} commit",
            table::bytes(p.private_working_set as f64),
            table::bytes(p.private_bytes as f64)
        );
        println!("Threads      {}", p.thread_count);
        println!("Handles      {}", p.handle_count);
        match details(control::inspect(&p.into())) {
            Ok(text) => print!("{text}"),
            Err(text) => {
                eprintln!("{text}");
                ok = false;
            }
        }
        println!();
    }
    Ok(exit_code(ok))
}

/// `info --json`: each process with its details, or why they could not be
/// read; and whether all could.
fn info_json(
    procs: Vec<&ProcessInfo>,
    inspect: impl Fn(&ProcessInfo) -> ControlResult<ProcessDetails>,
) -> (serde_json::Value, bool) {
    let mut ok = true;
    let rows = procs
        .into_iter()
        .map(|p| match inspect(p) {
            Ok(d) => serde_json::json!({ "process": p, "details": d }),
            Err(e) => {
                ok = false;
                serde_json::json!({ "process": p, "error": e.to_string() })
            }
        })
        .collect();
    (serde_json::Value::Array(rows), ok)
}

/// The `info` lines from [`control::inspect`]: `Ok` for stdout, `Err` for stderr.
fn details(d: ControlResult<ProcessDetails>) -> Result<String, String> {
    let d = d.map_err(|e| format!("Details      {e}"))?;
    let na = || "n/a".to_owned();
    let lines = [
        ("Path", d.path.unwrap_or_else(na)),
        ("Command", d.command_line.unwrap_or_else(na)),
        ("User", d.user.unwrap_or_else(na)),
        ("Priority", d.priority.map_or_else(na, |p| format!("{p:?}"))),
        (
            "Affinity",
            d.affinity
                .map_or_else(na, |(m, _)| parse::format_cpu_list(m)),
        ),
        ("EcoQoS", d.eco_qos.map_or_else(na, |e| format!("{e:?}"))),
        ("Critical", d.critical.map_or_else(na, |c| yes_no(c).into())),
        ("Protected", d.protected.unwrap_or_else(|| "no".into())),
    ];
    Ok(lines
        .iter()
        .map(|(label, value)| format!("{label:<13}{value}\n"))
        .collect())
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn uptime(d: Duration) -> String {
    let s = d.as_secs();
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h >= 24 {
        format!("{}d {:02}h {m:02}m", h / 24, h % 24)
    } else {
        format!("{h}h {m:02}m {s:02}s")
    }
}

pub fn limit(
    specs: &[String],
    cpu_percent: Option<f32>,
    memory_bytes: Option<u64>,
    detach: bool,
) -> anyhow::Result<ExitCode> {
    if cpu_percent.is_none() && memory_bytes.is_none() {
        bail!("specify --cpu and/or --memory");
    }
    let limits = Limits {
        cpu_percent,
        memory_bytes,
    };
    let mut limiter = Limiter::new();
    let mut ok = true;
    for t in resolve(specs, &snapshot()?)? {
        ok &= report("limit", &t, limiter.apply(&t, limits));
    }
    let targets: Vec<ProcessRef> = limiter.limited().cloned().collect();
    if targets.is_empty() {
        return Ok(exit_code(false));
    }
    if detach {
        println!("Limits stay in force until the processes exit.");
        return Ok(exit_code(ok));
    }

    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))?;
    println!("Holding limits. Press Ctrl+C to lift them and exit.");
    ok &= hold(&mut limiter, &targets, cpu_percent, &stop)?;
    Ok(exit_code(ok))
}

/// Prints the limited processes every second until they have all exited or
/// `stop` is set; then lifts the limits. Returns whether lifting succeeded.
fn hold(
    limiter: &mut Limiter,
    targets: &[ProcessRef],
    cpu_percent: Option<f32>,
    stop: &AtomicBool,
) -> anyhow::Result<bool> {
    let mut sampler = Sampler::new();
    sampler.sample()?;
    let cap = cpu_percent.map_or_else(|| "none".to_owned(), |c| format!("{c:.1}%"));
    while !stop.load(Ordering::SeqCst) {
        let tick = Instant::now() + Duration::from_secs(1);
        while Instant::now() < tick && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let snap = sampler.sample()?;
        let mut alive = 0;
        for t in targets {
            if let Some(p) = snap
                .processes
                .iter()
                .find(|p| p.pid == t.pid && p.create_time == t.create_time)
            {
                alive += 1;
                println!(
                    "{:>7}  {:<24} cpu {:5.1}% (cap {cap})  mem {}",
                    p.pid,
                    p.name,
                    p.cpu_percent,
                    table::bytes(p.private_bytes as f64)
                );
            }
        }
        if alive == 0 {
            println!("All limited processes have exited.");
            return Ok(true);
        }
    }

    let mut ok = true;
    for (t, r) in limiter.lift_all() {
        ok &= report("unlimit", &t, r);
    }
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};
    use wtm_core::control::ControlError;

    fn procs(entries: &[(u32, &str)]) -> Vec<ProcessInfo> {
        let base = snapshot().unwrap().swap_remove(0);
        entries
            .iter()
            .map(|&(pid, name)| ProcessInfo {
                pid,
                name: name.into(),
                ..base.clone()
            })
            .collect()
    }

    fn specs(s: &[&str]) -> Vec<String> {
        s.iter().map(|&s| s.to_owned()).collect()
    }

    #[test]
    fn a_tree_inside_another_match_is_ended_once() {
        let mut list = procs(&[(10, "node.exe"), (11, "node.exe"), (12, "node.exe")]);
        for p in &mut list {
            p.create_time = i64::from(p.pid);
        }
        // 11 is 10's child; 12 stands alone.
        list[1].parent_pid = 10;
        list[2].parent_pid = 1;
        let roots = resolve(&specs(&["node"]), &list).unwrap();
        let pids: Vec<u32> = outermost(roots, &list).iter().map(|r| r.pid).collect();
        assert_eq!(pids, [10, 12]);
    }

    #[test]
    fn resolves_pids_and_names_once_each() {
        let procs = procs(&[
            (10, "Foo.exe"),
            (11, "bar.exe"),
            (12, "foo.exe"),
            (13, "baz"),
        ]);
        let pids = |s: &[&str]| -> Vec<u32> {
            resolve(&specs(s), &procs)
                .unwrap()
                .iter()
                .map(|r| r.pid)
                .collect()
        };
        assert_eq!(pids(&["foo"]), [10, 12]);
        assert_eq!(pids(&["BAR.EXE", "11", "baz"]), [11, 13]);
        // Case-insensitive beyond ASCII too.
        let tr = self::procs(&[(20, "Çizim.exe")]);
        assert_eq!(resolve(&specs(&["çizim"]), &tr).unwrap()[0].pid, 20);
        assert_eq!(pids(&["12", "foo"]), [12, 10]);
        let e = resolve(&specs(&["99"]), &procs).unwrap_err();
        assert_eq!(e.to_string(), "no process matches '99'");
        assert!(resolve(&specs(&["foo", "nope"]), &procs).is_err());
    }

    fn target() -> ProcessRef {
        ProcessRef {
            pid: 42,
            create_time: 1,
            name: "app.exe".into(),
        }
    }

    #[test]
    fn affinity_lines() {
        assert_eq!(
            affinity_line(&target(), Ok(Some((0b101, 0xFF)))).unwrap(),
            "     42  app.exe                  cpus 0,2 (system 0-7)"
        );
        assert_eq!(
            affinity_line(&target(), Ok(None)).unwrap_err(),
            "     42  app.exe: affinity not readable"
        );
        let e = affinity_line(&target(), Err(ControlError::NotFound { pid: 42 })).unwrap_err();
        assert_eq!(e, "     42  app.exe: process 42 no longer exists");
    }

    #[test]
    fn detail_lines() {
        let full = ProcessDetails {
            path: Some(r"C:\app.exe".into()),
            command_line: Some(r#""C:\app.exe" --fast"#.into()),
            user: Some(r"PC\ana".into()),
            priority: Some(PriorityClass::High),
            affinity: Some((0b11, 0xF)),
            eco_qos: Some(EcoQos::On),
            critical: Some(true),
            protected: Some("system process".into()),
        };
        assert_eq!(
            details(Ok(full)).unwrap(),
            "Path         C:\\app.exe\n\
             Command      \"C:\\app.exe\" --fast\n\
             User         PC\\ana\n\
             Priority     High\n\
             Affinity     0-1\n\
             EcoQoS       On\n\
             Critical     yes\n\
             Protected    system process\n"
        );
        let empty = ProcessDetails {
            path: None,
            command_line: None,
            user: None,
            priority: None,
            affinity: None,
            eco_qos: None,
            critical: Some(false),
            protected: None,
        };
        let text = details(Ok(empty)).unwrap();
        assert!(
            text.starts_with(
                "Path         n/a\nCommand      n/a\nUser         n/a\nPriority     n/a\n"
            ),
            "{text}"
        );
        assert!(
            text.ends_with("Critical     no\nProtected    no\n"),
            "{text}"
        );
        assert_eq!(
            details(Err(ControlError::AccessDenied { pid: 4 })).unwrap_err(),
            "Details      access denied to process 4 (try an elevated prompt)"
        );
    }

    #[test]
    fn info_json_carries_details_or_the_error() {
        let procs = snapshot().unwrap();
        let me = procs.iter().find(|p| p.pid == std::process::id()).unwrap();
        let (value, ok) = info_json(vec![me], |p| control::inspect(&p.into()));
        assert!(ok);
        assert_eq!(value[0]["process"]["pid"], me.pid);
        assert!(value[0]["details"]["path"].is_string());
        let (value, ok) = info_json(vec![me], |p| Err(ControlError::AccessDenied { pid: p.pid }));
        assert!(!ok);
        assert!(
            value[0]["error"]
                .as_str()
                .unwrap()
                .contains("access denied")
        );
        assert!(value[0].get("details").is_none());
    }

    #[test]
    fn formats_uptime() {
        assert_eq!(
            uptime(Duration::from_secs(3 * 3600 + 4 * 60 + 5)),
            "3h 04m 05s"
        );
        assert_eq!(
            uptime(Duration::from_secs(2 * 86_400 + 5 * 3600 + 7 * 60 + 9)),
            "2d 05h 07m"
        );
        assert_eq!(yes_no(true), "yes");
        assert_eq!(yes_no(false), "no");
    }

    struct Guard(Child);

    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn hold_lifts_limits_when_stopped() {
        let child = Guard(
            Command::new("ping")
                .args(["-n", "60", "127.0.0.1"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        // A spawned process is listed as soon as `spawn` returns.
        let pid = child.0.id().to_string();
        let target = resolve(&[pid], &snapshot().unwrap()).unwrap().remove(0);
        let mut limiter = Limiter::new();
        let limits = Limits {
            cpu_percent: Some(50.0),
            memory_bytes: None,
        };
        limiter.apply(&target, limits).unwrap();
        let stop = AtomicBool::new(true);
        assert!(hold(&mut limiter, &[target], Some(50.0), &stop).unwrap());
        assert_eq!(limiter.limited().count(), 0);
    }
}

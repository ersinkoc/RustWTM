//! Process control subcommands.

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use anyhow::bail;
use wtm_core::control::{self, ControlResult, EcoQos, Limiter, Limits, PriorityClass, ProcessRef};
use wtm_core::{ProcessInfo, Sampler};

use crate::{parse, table};

fn snapshot() -> anyhow::Result<Vec<ProcessInfo>> {
    Ok(Sampler::new().sample()?.processes)
}

/// Resolves PIDs or image names (`.exe` optional, case-insensitive).
fn resolve(specs: &[String], procs: &[ProcessInfo]) -> anyhow::Result<Vec<ProcessRef>> {
    let mut out: Vec<ProcessRef> = Vec::new();
    for spec in specs {
        let matches: Vec<&ProcessInfo> = match spec.parse::<u32>() {
            Ok(pid) => procs.iter().filter(|p| p.pid == pid).collect(),
            Err(_) => {
                let with_exe = format!("{spec}.exe");
                procs
                    .iter()
                    .filter(|p| {
                        p.name.eq_ignore_ascii_case(spec) || p.name.eq_ignore_ascii_case(&with_exe)
                    })
                    .collect()
            }
        };
        if matches.is_empty() {
            bail!("no process matches '{spec}'");
        }
        for p in matches {
            if !out.iter().any(|r| r.pid == p.pid) {
                out.push(p.into());
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
    for root in resolve(specs, &procs)? {
        for (t, r) in control::kill_tree(&root, &procs) {
            ok &= report("kill-tree", &t, r);
        }
    }
    Ok(exit_code(ok))
}

pub fn suspend(specs: &[String]) -> anyhow::Result<ExitCode> {
    for_each(specs, "suspend", control::suspend)
}

pub fn resume(specs: &[String]) -> anyhow::Result<ExitCode> {
    for_each(specs, "resume", control::resume)
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
            match control::inspect(&t).map(|d| d.affinity) {
                Ok(Some((mask, system))) => println!(
                    "{:>7}  {:<24} cpus {} (system {})",
                    t.pid,
                    t.name,
                    parse::format_cpu_list(mask),
                    parse::format_cpu_list(system)
                ),
                Ok(None) => {
                    eprintln!("{:>7}  {}: affinity not readable", t.pid, t.name);
                    ok = false;
                }
                Err(e) => {
                    eprintln!("{:>7}  {}: {e}", t.pid, t.name);
                    ok = false;
                }
            }
        }
        return Ok(exit_code(ok));
    };
    let mask = parse::cpu_list(cpus).map_err(anyhow::Error::msg)?;
    for_each(specs, "affinity", |t| control::set_affinity(t, mask))
}

pub fn info(specs: &[String]) -> anyhow::Result<ExitCode> {
    let procs = snapshot()?;
    let mut ok = true;
    for t in resolve(specs, &procs)? {
        let Some(p) = procs.iter().find(|p| p.pid == t.pid) else {
            continue;
        };
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
        match control::inspect(&t) {
            Ok(d) => {
                let na = || "n/a".to_owned();
                println!("Path         {}", d.path.unwrap_or_else(na));
                println!(
                    "Priority     {}",
                    d.priority.map_or_else(na, |p| format!("{p:?}"))
                );
                println!(
                    "Affinity     {}",
                    d.affinity
                        .map_or_else(na, |(m, _)| parse::format_cpu_list(m))
                );
                println!(
                    "EcoQoS       {}",
                    d.eco_qos.map_or_else(na, |e| format!("{e:?}"))
                );
                println!(
                    "Critical     {}",
                    d.critical.map_or_else(na, |c| yes_no(c).into())
                );
                println!("Protected    {}", d.protected.as_deref().unwrap_or("no"));
            }
            Err(e) => {
                eprintln!("Details      {e}");
                ok = false;
            }
        }
        println!();
    }
    Ok(exit_code(ok))
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
        for t in &targets {
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
            return Ok(exit_code(ok));
        }
    }

    for (t, r) in limiter.lift_all() {
        ok &= report("unlimit", &t, r);
    }
    Ok(exit_code(ok))
}

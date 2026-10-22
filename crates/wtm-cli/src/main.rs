#![forbid(unsafe_code)]

mod actions;
mod config;
mod kill;
mod mcp;
mod parse;
mod ports;
mod power;
mod table;
mod top;

use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use wtm_core::control::{self, EcoQos, PriorityClass};
use wtm_core::{Sampler, SortKey, sort_processes};

#[derive(Parser)]
#[command(name = "wtm", version, about = "RustWTM - Windows task manager")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Targets are PIDs or image names (`.exe` optional, case-insensitive);
/// a name applies to every process with that name.
#[derive(Subcommand)]
enum Command {
    /// Read or explicitly change shared Windows power-plan frequency limits
    Power {
        #[command(subcommand)]
        action: Option<power::PowerCommand>,
    },
    /// List CPU Sets, processor groups and relative P/E/LP-E tiers
    Cores {
        #[arg(long)]
        json: bool,
    },
    /// Temperatures, fan RPM/duty, power, voltage and clocks (read-only)
    Sensors {
        #[arg(long)]
        json: bool,
    },
    /// Print a one-shot process listing
    List {
        #[arg(long, short, value_enum, default_value_t)]
        sort: SortArg,
        /// Show only the first N processes
        #[arg(long, short = 'n')]
        limit: Option<usize>,
        /// Sampling window used to measure CPU and disk rates
        #[arg(long, default_value_t = 1000, value_name = "MS")]
        sample_ms: u64,
        /// Also show each process's command line
        #[arg(long, short = 'c')]
        cmdline: bool,
        /// Print the sample as JSON (network rates are null when unmeasured)
        #[arg(long)]
        json: bool,
    },
    /// Model Context Protocol server on stdin/stdout, for AI assistants
    /// (Claude Desktop, Claude Code). Read-only unless --allow-control.
    Mcp {
        /// Also offer tools that end, suspend or deprioritize processes
        #[arg(long)]
        allow_control: bool,
    },
    /// Live, auto-refreshing process monitor
    Top {
        #[arg(long, short, value_enum, default_value_t)]
        sort: SortArg,
        /// Refresh interval
        #[arg(long, short, default_value_t = 1000, value_name = "MS")]
        interval_ms: u64,
    },
    /// Show details of processes
    Info {
        #[arg(required = true)]
        targets: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Terminate processes
    Kill {
        #[arg(required = true)]
        targets: Vec<String>,
        /// Also terminate all descendants
        #[arg(long, short)]
        tree: bool,
    },
    /// Write a full memory dump file of processes, for debugging
    Dump {
        #[arg(required = true)]
        targets: Vec<String>,
        /// Folder for the .DMP files (default: your Temp folder)
        #[arg(long, short)]
        dir: Option<std::path::PathBuf>,
    },
    /// Free RAM: move working sets to the standby list (nothing is lost)
    Trim {
        #[arg(required = true)]
        targets: Vec<String>,
    },
    /// Freeze all threads of processes
    Suspend {
        #[arg(required = true)]
        targets: Vec<String>,
    },
    /// Resume suspended processes
    Resume {
        #[arg(required = true)]
        targets: Vec<String>,
    },
    /// Change the priority class
    Priority {
        target: String,
        /// Realtime needs elevation, otherwise Windows silently uses High
        #[arg(value_enum)]
        class: PriorityArg,
    },
    /// Show or set the CPU affinity, e.g. `0-3,6` or `0xF`
    Affinity {
        target: String,
        #[arg(conflicts_with_all=["pcores","ecores","lp_ecores","all_cores"])]
        cpus: Option<String>,
        #[arg(long,conflicts_with_all=["ecores","lp_ecores","all_cores"])]
        pcores: bool,
        #[arg(long,conflicts_with_all=["lp_ecores","all_cores"])]
        ecores: bool,
        #[arg(long, conflicts_with = "all_cores")]
        lp_ecores: bool,
        #[arg(long)]
        all_cores: bool,
    },
    /// Toggle efficiency mode (EcoQoS power throttling)
    Eco {
        target: String,
        #[arg(value_enum, default_value_t = EcoArg::On)]
        mode: EcoArg,
    },
    /// Long-term per-app usage recorded by the service, plus findings
    History {
        /// Analysis window
        #[arg(long, default_value_t = 24)]
        hours: u32,
        /// Show only the first N apps
        #[arg(long, short = 'n', default_value_t = 20)]
        limit: usize,
    },
    /// Per-app CPU / memory budgets enforced by the service (persistent)
    AppLimit {
        #[command(subcommand)]
        action: config::LimitAction,
    },
    /// The kill list: programs the service may end (automatically when they
    /// keep eating CPU, or all at once with `wtm panic`)
    KillList {
        #[command(subcommand)]
        action: kill::KillAction,
    },
    /// End everything on the kill list that is running (preview without --yes)
    Panic {
        #[arg(long, short)]
        yes: bool,
    },
    /// Listening ports and the programs behind them
    Ports {
        /// Include bound UDP ports
        #[arg(long, short)]
        udp: bool,
        /// Only ports of well-known developer tools and servers
        #[arg(long, short)]
        known: bool,
        #[arg(long)]
        json: bool,
    },
    /// End whatever keeps a port busy, e.g. `wtm free-port 3000` (preview
    /// without --yes)
    FreePort {
        #[arg(value_parser = parse_port)]
        port: u16,
        #[arg(long, short)]
        yes: bool,
        /// Also end the processes it started
        #[arg(long, short)]
        tree: bool,
    },
    /// Show, check or replace the service's configuration
    Config {
        #[command(subcommand)]
        action: config::Action,
    },
    /// Hard-cap CPU and/or memory using a job object
    Limit {
        #[arg(required = true)]
        targets: Vec<String>,
        /// CPU cap as a share of the whole machine, e.g. `20` or `12.5%`
        #[arg(long, value_parser = parse::percent, value_name = "PERCENT")]
        cpu: Option<f32>,
        /// Committed memory cap per process, e.g. `512M`, `2G`
        #[arg(long, value_parser = parse::size, value_name = "SIZE")]
        memory: Option<u64>,
        /// Exit immediately; limits then last until the processes exit
        #[arg(long)]
        detach: bool,
    },
}

#[derive(Clone, Copy, Default, ValueEnum)]
enum SortArg {
    #[default]
    Cpu,
    Mem,
    Disk,
    Pid,
    Name,
}

impl From<SortArg> for SortKey {
    fn from(s: SortArg) -> Self {
        match s {
            SortArg::Cpu => SortKey::Cpu,
            SortArg::Mem => SortKey::Memory,
            SortArg::Disk => SortKey::Disk,
            SortArg::Pid => SortKey::Pid,
            SortArg::Name => SortKey::Name,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum PriorityArg {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
    Realtime,
}

impl From<PriorityArg> for PriorityClass {
    fn from(p: PriorityArg) -> Self {
        match p {
            PriorityArg::Idle => PriorityClass::Idle,
            PriorityArg::BelowNormal => PriorityClass::BelowNormal,
            PriorityArg::Normal => PriorityClass::Normal,
            PriorityArg::AboveNormal => PriorityClass::AboveNormal,
            PriorityArg::High => PriorityClass::High,
            PriorityArg::Realtime => PriorityClass::Realtime,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum EcoArg {
    /// Always throttle (efficiency mode)
    On,
    /// Never throttle
    Off,
    /// Let Windows decide (default for all processes)
    Auto,
}

impl From<EcoArg> for EcoQos {
    fn from(e: EcoArg) -> Self {
        match e {
            EcoArg::On => EcoQos::On,
            EcoArg::Off => EcoQos::Off,
            EcoArg::Auto => EcoQos::SystemManaged,
        }
    }
}

/// Ends the kill list: through the service when this prompt is elevated
/// (it can end anything), otherwise directly with our own rights.
fn run_panic(yes: bool) -> anyhow::Result<ExitCode> {
    let mut client = wtm_ipc::Client::connect()
        .map_err(|e| anyhow::anyhow!("RustWTM service not reachable ({e})"))?;
    // The kill list decides what is ended, perhaps with our administrator
    // rights: only the real service may hand it over, not a program that
    // took the pipe's name while the service is not running.
    anyhow::ensure!(
        client.is_verified(),
        "the RustWTM pipe is not served by the installed service; its kill list is not used"
    );
    let text = kill::fetch(&mut client)?;
    let entries = wtm_policy::Config::from_toml(&text)?.kill;
    let mut sampler = Sampler::new();
    sampler.sample()?;
    std::thread::sleep(Duration::from_millis(300));
    let procs = sampler.sample()?.processes;
    let admin = client.is_admin();
    let mut end = |t: &control::ProcessRef| -> Result<(), String> {
        if !admin {
            return control::kill(t).map_err(|e| e.to_string());
        }
        let request = wtm_ipc::Request::Control {
            target: t.clone(),
            command: wtm_ipc::Command::Kill,
        };
        match client.call(&request) {
            Ok(wtm_ipc::Response::Control(outcomes)) => {
                outcomes.into_iter().try_for_each(|o| o.result)
            }
            Ok(other) => Err(format!("unexpected response: {other:?}")),
            Err(e) => Err(e.to_string()),
        }
    };
    kill::panic(
        yes,
        &entries,
        &procs,
        &mut |p| control::command_line(&control::ProcessRef::from(p)).ok(),
        &mut end,
        &mut io::stdout().lock(),
    )
}

fn parse_port(s: &str) -> Result<u16, String> {
    wtm_core::ports::parse_port(s).ok_or_else(|| format!("not a port number: {s}"))
}

/// The ports in use and the processes, for the names behind them.
fn port_uses() -> anyhow::Result<(Vec<wtm_core::ports::PortUse>, Vec<wtm_core::ProcessInfo>)> {
    let read_at = std::time::SystemTime::now();
    let uses = wtm_core::ports::port_uses(&wtm_core::connections()?);
    let mut sampler = Sampler::new();
    sampler.sample()?;
    std::thread::sleep(Duration::from_millis(200));
    Ok((uses, owners_then(sampler.sample()?.processes, read_at)))
}

/// `procs` without those started after `read_at`, when the ports were read:
/// such a process got the PID of an owner that has exited since, and must
/// neither be named for the port nor ended for it.
fn owners_then(
    mut procs: Vec<wtm_core::ProcessInfo>,
    read_at: std::time::SystemTime,
) -> Vec<wtm_core::ProcessInfo> {
    procs.retain(|p| p.start_time().is_none_or(|t| t <= read_at));
    procs
}

fn main() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();
    // Lets an elevated prompt control services and other users' processes.
    control::enable_debug_privilege();

    match cli.command {
        Command::Power { action } => {
            power::run(action.unwrap_or(power::PowerCommand::Show { json: false }))
        }
        Command::Cores { json } => {
            let topology = wtm_core::cpu::topology()?;
            if json {
                serde_json::to_writer_pretty(io::stdout().lock(), &topology)?;
                println!();
            } else {
                println!("GROUP  CPU  CORE  CLASS  TIER     CPU SET  STATE");
                for c in &topology.cpus {
                    println!(
                        "{:>5}  {:>3}  {:>4}  {:>5}  {:<7}  {:>7}  {}",
                        c.group,
                        c.index,
                        c.core,
                        c.efficiency_class,
                        topology.tier(c.efficiency_class),
                        c.id,
                        if c.reserved_elsewhere {
                            "reserved"
                        } else if c.parked {
                            "parked"
                        } else {
                            "available"
                        }
                    );
                }
                println!(
                    "Tiers are relative Windows EfficiencyClass values; higher classes are faster. Indices are group-relative."
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::List {
            sort,
            limit,
            sample_ms,
            cmdline,
            json,
        } => list(sort.into(), limit, sample_ms, cmdline, json).map(|()| ExitCode::SUCCESS),
        Command::Mcp { allow_control } => {
            mcp::run(io::stdin().lock(), io::stdout().lock(), allow_control)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Top { sort, interval_ms } => {
            top::run(sort.into(), Duration::from_millis(interval_ms.max(100)))
                .map(|()| ExitCode::SUCCESS)
        }
        Command::Info { targets, json } => actions::info(&targets, json),
        Command::History { hours, limit } => history(
            wtm_ipc::Client::connect(),
            hours,
            limit,
            &mut io::stdout().lock(),
        )
        .map(|()| ExitCode::SUCCESS),
        Command::AppLimit { action } => {
            config::app_limit(action, wtm_ipc::Client::connect, &mut io::stdout().lock())
        }
        Command::Config { action } => {
            config::run(action, wtm_ipc::Client::connect, &mut io::stdout().lock())
        }
        Command::KillList { action } => {
            kill::kill_list(action, wtm_ipc::Client::connect, &mut io::stdout().lock())
        }
        Command::Panic { yes } => run_panic(yes),
        Command::Sensors { json } => {
            let mut reader = wtm_core::hardware::HardwareReader::new(false);
            let report = reader.sample().map_err(anyhow::Error::msg)?;
            let mut out = io::stdout().lock();
            if json {
                serde_json::to_writer_pretty(&mut out, &report)?;
                writeln!(out)?;
            } else {
                writeln!(out, "{}", report.provider)?;
                writeln!(out, "{}", report.notice)?;
                for sensor in &report.sensors {
                    let value = sensor.value.map_or_else(
                        || "unavailable".into(),
                        |v| format!("{v:.1} {}", sensor.kind.unit()),
                    );
                    writeln!(out, "{} | {} | {value}", sensor.device, sensor.name)?;
                }
                for channel in &report.channels {
                    writeln!(
                        out,
                        "Control: {} | {} | {}",
                        channel.device, channel.name, channel.reason
                    )?;
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Ports { udp, known, json } => {
            let (uses, procs) = port_uses()?;
            let mut out = io::stdout().lock();
            if json {
                serde_json::to_writer_pretty(&mut out, &ports::json(&uses, &procs, udp, known))?;
                writeln!(out)?;
            } else {
                ports::render(&uses, &procs, udp, known, &mut out)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::FreePort { port, yes, tree } => {
            let (uses, procs) = port_uses()?;
            let mut end = |t: &control::ProcessRef| {
                if tree {
                    let failed: Vec<String> = control::kill_tree(t, &procs)
                        .into_iter()
                        .filter_map(|(_, r)| r.err().map(|e| e.to_string()))
                        .collect();
                    if failed.is_empty() {
                        Ok(())
                    } else {
                        Err(failed.join("; "))
                    }
                } else {
                    control::kill(t).map_err(|e| e.to_string())
                }
            };
            ports::free_port(port, yes, &uses, &procs, &mut end, &mut io::stdout().lock())
        }
        Command::Kill { targets, tree } => actions::kill(&targets, tree),
        Command::Suspend { targets } => actions::suspend(&targets),
        Command::Trim { targets } => actions::trim(&targets),
        Command::Dump { targets, dir } => {
            actions::dump(&targets, &dir.unwrap_or_else(std::env::temp_dir))
        }
        Command::Resume { targets } => actions::resume(&targets),
        Command::Priority { target, class } => actions::priority(&[target], class.into()),
        Command::Affinity {
            target,
            cpus,
            pcores,
            ecores,
            lp_ecores,
            all_cores,
        } => {
            use wtm_core::cpu::CoreSelection as C;
            let selection = if pcores {
                Some(C::Performance)
            } else if ecores {
                Some(C::Efficient)
            } else if lp_ecores {
                Some(C::LowPower)
            } else if all_cores {
                Some(C::All)
            } else {
                None
            };
            if let Some(selection) = selection {
                actions::core_affinity(&[target], selection)
            } else {
                actions::affinity(&[target], cpus.as_deref())
            }
        }
        Command::Eco { target, mode } => actions::eco(&[target], mode.into()),
        Command::Limit {
            targets,
            cpu,
            memory,
            detach,
        } => actions::limit(&targets, cpu, memory, detach),
    }
}

fn list(
    sort: SortKey,
    limit: Option<usize>,
    sample_ms: u64,
    cmdline: bool,
    json: bool,
) -> anyhow::Result<()> {
    let mut sampler = Sampler::new();
    sampler.sample()?;
    std::thread::sleep(Duration::from_millis(sample_ms.max(100)));
    let mut snap = sampler.sample()?;
    sort_processes(&mut snap.processes, sort);
    snap.processes.truncate(limit.unwrap_or(usize::MAX));

    let mut out = io::stdout().lock();
    if json {
        // Read with the caller's own rights; null when not readable.
        let command_line = |p: &wtm_core::ProcessInfo| {
            cmdline
                .then(|| control::command_line(&p.into()).ok())
                .flatten()
        };
        serde_json::to_writer_pretty(&mut out, &list_json(&snap, cmdline, command_line)?)?;
        writeln!(out)?;
        return Ok(());
    }
    let net = snap.net_traced;
    writeln!(out, "{}", table::summary(&snap.system))?;
    writeln!(
        out,
        "{}{}",
        table::io_summary(&snap.system),
        table::network_note(net)
    )?;
    if cmdline {
        writeln!(
            out,
            "{}",
            table::then(table::header(net), 4, "COMMAND LINE")
        )?;
    } else {
        writeln!(out, "{}", table::header(net))?;
    }
    for p in &snap.processes {
        let row = table::row(p, usize::MAX, net);
        if cmdline {
            // Read with the caller's own rights; empty when not readable.
            let cmd = control::command_line(&p.into()).unwrap_or_default();
            writeln!(out, "{}", table::then(row, p.name.chars().count(), &cmd))?;
        } else {
            writeln!(out, "{row}")?;
        }
    }
    Ok(())
}

/// `wtm list --json`: the snapshot, with network rates `null` when they
/// were not measured (not 0) and, with `cmdline`, each command line.
fn list_json(
    snap: &wtm_core::Snapshot,
    cmdline: bool,
    command_line: impl Fn(&wtm_core::ProcessInfo) -> Option<String>,
) -> serde_json::Result<serde_json::Value> {
    let mut value = serde_json::to_value(snap)?;
    if let Some(rows) = value
        .get_mut("processes")
        .and_then(serde_json::Value::as_array_mut)
    {
        for (row, p) in rows.iter_mut().zip(&snap.processes) {
            if !snap.net_traced {
                row["net_rx_rate"] = serde_json::Value::Null;
                row["net_tx_rate"] = serde_json::Value::Null;
            }
            if cmdline {
                row["command_line"] = command_line(p).into();
            }
        }
    }
    Ok(value)
}

/// Prints the service's history report; `client` is the connection attempt.
fn history(
    client: io::Result<wtm_ipc::Client>,
    hours: u32,
    limit: usize,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    use wtm_ipc::{Request, Response};
    let mut client = client.map_err(|e| {
        anyhow::anyhow!("RustWTM service not reachable ({e}); history is recorded by the service")
    })?;
    let (stats, findings) = match client.call(&Request::History { hours })? {
        Response::History { stats, findings } => (stats, findings),
        Response::Error(e) => anyhow::bail!(e),
        other => anyhow::bail!("unexpected response: {other:?}"),
    };
    writeln!(
        out,
        "{:<28} {:>8} {:>8} {:>8} {:>8} {:>10} {:>10} {:>10}",
        "APP", "SEEN", "AVG CPU", "P95 CPU", "AVG GPU", "AVG MEM", "PEAK MEM", "GROWTH/H"
    )?;
    for s in stats.iter().take(limit) {
        writeln!(
            out,
            "{:<28} {:>7}m {:>7.2}% {:>7.2}% {:>7.1}% {:>10} {:>10} {:>+9.0}M",
            s.app,
            s.minutes,
            s.cpu_avg,
            s.cpu_p95,
            s.gpu_avg,
            table::bytes(s.mem_avg as f64),
            table::bytes(s.mem_peak as f64),
            s.mem_growth_per_hour / (1024.0 * 1024.0)
        )?;
    }
    if findings.is_empty() {
        writeln!(out, "\nNo long-term findings.")?;
    } else {
        writeln!(out, "\nFindings:")?;
        for f in findings {
            writeln!(out, "  {:<26} {:?}: {}", f.app, f.kind, f.detail)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use wtm_ipc::{Client, PROTOCOL_VERSION, Request, Response, read_frame, write_frame};
    use wtm_policy::{AppStats, Finding, FindingKind};

    #[test]
    fn a_process_started_after_the_port_lookup_is_no_owner() {
        let procs = Sampler::new().sample().unwrap().processes;
        let me = procs
            .iter()
            .find(|p| p.pid == std::process::id())
            .unwrap()
            .clone();
        let started = me.start_time().unwrap();
        let before = owners_then(procs.clone(), started - Duration::from_secs(1));
        assert!(
            before.iter().all(|p| p.pid != me.pid),
            "started later: left out"
        );
        assert!(
            before.iter().any(|p| p.pid == 4),
            "System, started at boot, stays"
        );
        let after = owners_then(procs, started + Duration::from_secs(1));
        assert!(after.iter().any(|p| p.pid == me.pid));
    }

    #[test]
    fn value_enums_map_to_core_types() {
        let arg = |a: &str| SortArg::from_str(a, false).unwrap();
        assert_eq!(SortKey::from(SortArg::default()), SortKey::Cpu);
        assert_eq!(SortKey::from(arg("cpu")), SortKey::Cpu);
        assert_eq!(SortKey::from(arg("mem")), SortKey::Memory);
        assert_eq!(SortKey::from(arg("disk")), SortKey::Disk);
        assert_eq!(SortKey::from(arg("pid")), SortKey::Pid);
        assert_eq!(SortKey::from(arg("name")), SortKey::Name);

        let class = |a: &str| PriorityClass::from(PriorityArg::from_str(a, false).unwrap());
        assert_eq!(class("idle"), PriorityClass::Idle);
        assert_eq!(class("below-normal"), PriorityClass::BelowNormal);
        assert_eq!(class("normal"), PriorityClass::Normal);
        assert_eq!(class("above-normal"), PriorityClass::AboveNormal);
        assert_eq!(class("high"), PriorityClass::High);
        assert_eq!(class("realtime"), PriorityClass::Realtime);

        let eco = |a: &str| EcoQos::from(EcoArg::from_str(a, false).unwrap());
        assert_eq!(eco("on"), EcoQos::On);
        assert_eq!(eco("off"), EcoQos::Off);
        assert_eq!(eco("auto"), EcoQos::SystemManaged);
    }

    #[test]
    fn parses_commands() {
        for flag in ["--pcores", "--ecores", "--lp-ecores", "--all-cores"] {
            assert!(Cli::try_parse_from(["wtm", "affinity", "1234", flag]).is_ok());
            assert!(Cli::try_parse_from(["wtm", "affinity", "1234", "0-3", flag]).is_err());
        }
        assert!(Cli::try_parse_from(["wtm", "affinity", "1234", "--pcores", "--ecores"]).is_err());
        assert!(Cli::try_parse_from(["wtm", "cores", "--json"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "wtm", "power", "limit", "--class", "0", "--mhz", "1800", "--source", "both",
                "--yes"
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "wtm", "power", "limit", "--class", "2", "--mhz", "1800", "--yes"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "wtm", "power", "limit", "--class", "0", "--mhz", "0", "--yes"
            ])
            .is_err()
        );
        let cli = Cli::try_parse_from(["wtm", "eco", "notepad"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Eco {
                mode: EcoArg::On,
                ..
            }
        ));
        let cli = Cli::try_parse_from(["wtm", "history"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::History {
                hours: 24,
                limit: 20
            }
        ));
        assert!(Cli::try_parse_from(["wtm", "kill"]).is_err());
        let cli = Cli::try_parse_from(["wtm", "dump", "app", "-d", "C:/dumps"]).unwrap();
        assert!(
            matches!(cli.command, Command::Dump { targets, dir: Some(d) } if targets == ["app"] && d.ends_with("dumps"))
        );
        assert!(Cli::try_parse_from(["wtm", "dump"]).is_err());
        let cli = Cli::try_parse_from(["wtm", "panic", "--yes"]).unwrap();
        assert!(matches!(cli.command, Command::Panic { yes: true }));
        let cli = Cli::try_parse_from(["wtm", "free-port", ":3000", "--yes"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::FreePort {
                port: 3000,
                yes: true,
                tree: false
            }
        ));
        assert!(Cli::try_parse_from(["wtm", "free-port", "0"]).is_err());
        let cli = Cli::try_parse_from(["wtm", "ports", "-uk"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Ports {
                udp: true,
                known: true,
                json: false
            }
        ));
        let cli = Cli::try_parse_from(["wtm", "ports", "--json"]).unwrap();
        assert!(matches!(cli.command, Command::Ports { json: true, .. }));
        let cli = Cli::try_parse_from(["wtm", "list", "--json", "-n", "3"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::List {
                json: true,
                limit: Some(3),
                ..
            }
        ));
        let cli = Cli::try_parse_from(["wtm", "info", "notepad", "--json"]).unwrap();
        assert!(matches!(cli.command, Command::Info { json: true, .. }));
        let cli = Cli::try_parse_from([
            "wtm",
            "kill-list",
            "add",
            "node.exe",
            "--cmdline",
            "vitest",
            "--cpu",
            "50%",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::KillList {
                action: kill::KillAction::Add {
                    cpu: Some(50.0),
                    secs: 60,
                    ..
                }
            }
        ));
    }

    #[test]
    fn list_json_marks_unmeasured_network_as_null() {
        let mut snap = Sampler::new().sample().unwrap();
        snap.processes.truncate(2);
        let value = list_json(&snap, true, |p| Some(format!("cmd {}", p.pid))).unwrap();
        let rows = value["processes"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(value["net_traced"], false);
        assert!(rows[0]["net_rx_rate"].is_null() && rows[0]["net_tx_rate"].is_null());
        assert_eq!(
            rows[1]["command_line"],
            format!("cmd {}", snap.processes[1].pid)
        );
        assert!(rows[0]["create_time"].is_i64());

        snap.net_traced = true;
        snap.processes[0].net_rx_rate = 5.0;
        let value = list_json(&snap, false, |_| None).unwrap();
        assert_eq!(value["processes"][0]["net_rx_rate"], 5.0);
        assert!(value["processes"][0].get("command_line").is_none());
    }

    fn stats(app: &str) -> AppStats {
        AppStats {
            app: app.into(),
            minutes: 90,
            cpu_avg: 12.5,
            cpu_p95: 40.0,
            cpu_peak: 80.0,
            gpu_avg: 3.0,
            gpu_p95: 9.0,
            mem_avg: 100 << 20,
            mem_peak: 200 << 20,
            mem_last: 150 << 20,
            mem_growth_per_hour: 64.0 * 1024.0 * 1024.0,
            mem_growth_fit: 0.9,
            cpu_core_seconds: 600.0,
            ..AppStats::default()
        }
    }

    /// Runs `f` with a client of a one-shot fake service that answers one
    /// request with `reply`; returns that request and `f`'s result.
    pub(crate) fn with_reply<R>(
        reply: Response,
        f: impl FnOnce(io::Result<Client>) -> R,
    ) -> (Option<Request>, R) {
        let (mut requests, result) = with_replies(vec![reply], f);
        (requests.pop(), result)
    }

    /// Like `with_reply`, answering consecutive requests with `replies`.
    pub(crate) fn with_replies<R>(
        replies: Vec<Response>,
        f: impl FnOnce(io::Result<Client>) -> R,
    ) -> (Vec<Request>, R) {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            r"\\.\pipe\wtm-cli-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let mut listener = wtm_sys::pipe::PipeListener::bind(
            &name,
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)(A;;GA;;;OW)",
        )
        .unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            let Ok(mut conn) = listener.accept() else {
                return requests;
            };
            let Ok(hello) = read_frame::<Request>(&mut conn) else {
                return requests;
            };
            assert!(matches!(hello, Request::Hello { .. }));
            let hello = Response::Hello {
                version: PROTOCOL_VERSION,
                admin: false,
            };
            if write_frame(&mut conn, &hello).is_err() {
                return requests;
            }
            for reply in replies {
                let Ok(request) = read_frame::<Request>(&mut conn) else {
                    break;
                };
                requests.push(request);
                if write_frame(&mut conn, &reply).is_err() {
                    break;
                }
            }
            requests
        });
        let result = f(Client::connect_to(&name));
        (server.join().unwrap(), result)
    }

    /// Runs `history` against a fake service answering with `reply`.
    fn history_with(reply: Response, limit: usize, out: &mut impl Write) -> anyhow::Result<()> {
        let (request, result) = with_reply(reply, |c| history(c, 6, limit, out));
        assert_eq!(request, Some(Request::History { hours: 6 }), "{result:?}");
        result
    }

    fn history_text(reply: Response, limit: usize) -> anyhow::Result<String> {
        let mut out = Vec::new();
        history_with(reply, limit, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    fn sample_history() -> Response {
        Response::History {
            stats: vec![stats("busy.exe"), stats("idle.exe"), stats("hidden.exe")],
            findings: vec![Finding {
                app: "busy.exe".into(),
                kind: FindingKind::ChronicCpu,
                detail: "averages 12.5% CPU".into(),
            }],
        }
    }

    fn empty_history() -> Response {
        Response::History {
            stats: Vec::new(),
            findings: Vec::new(),
        }
    }

    /// Runs `test` unless the RustWTM service is installed, which makes the
    /// client (rightly) refuse fake servers.
    pub(crate) fn with_fake_service(test: impl FnOnce()) {
        if wtm_sys::services::service_pid(wtm_ipc::SERVICE_NAME).is_none() {
            test();
        }
    }

    #[test]
    fn history_prints_stats_and_findings() {
        with_fake_service(|| {
            let out = history_text(sample_history(), 2).unwrap();
            let lines: Vec<&str> = out.lines().collect();
            assert!(
                lines[0].starts_with("APP") && lines[0].ends_with("GROWTH/H"),
                "{out}"
            );
            assert!(lines[1].starts_with("busy.exe"), "{out}");
            assert!(lines[1].contains("90m"), "{out}");
            assert!(
                lines[1].contains("12.50%") && lines[1].contains("40.00%"),
                "{out}"
            );
            assert!(
                lines[1].contains("3.0%") && lines[1].ends_with("+64M"),
                "{out}"
            );
            assert!(lines[2].starts_with("idle.exe"), "{out}");
            assert!(!out.contains("hidden.exe"), "limit not applied: {out}");
            assert!(out.contains("\nFindings:\n"), "{out}");
            assert!(
                out.contains("  busy.exe                   ChronicCpu: averages 12.5% CPU"),
                "{out}"
            );
        });
    }

    #[test]
    fn history_without_data_or_with_errors() {
        with_fake_service(|| {
            let out = history_text(empty_history(), 20).unwrap();
            assert_eq!(out.lines().count(), 3, "{out}");
            assert!(out.ends_with("\nNo long-term findings.\n"), "{out}");

            let e = history_text(Response::Error("database locked".into()), 20).unwrap_err();
            assert_eq!(e.to_string(), "database locked");

            let e = history_text(Response::Events(Vec::new()), 20).unwrap_err();
            assert!(e.to_string().starts_with("unexpected response"), "{e}");
        });
    }

    /// Accepts `budget` writes, then fails every write and flush.
    pub(crate) struct FailingWriter {
        pub budget: usize,
    }

    impl Write for FailingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.budget = self
                .budget
                .checked_sub(1)
                .ok_or(io::Error::other("closed"))?;
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            match self.budget {
                0 => Err(io::Error::other("closed")),
                _ => Ok(()),
            }
        }
    }

    #[test]
    fn history_reports_write_errors() {
        with_fake_service(|| {
            for reply in [sample_history, empty_history] {
                // Fail at every possible point of the report until one fits.
                let mut budget = 0;
                while let Err(e) = history_with(reply(), 20, &mut FailingWriter { budget }) {
                    assert_eq!(e.to_string(), "closed");
                    budget += 1;
                    assert!(budget < 1000, "history never succeeded");
                }
                assert!(budget > 3, "{budget}");
            }
        });
    }

    #[test]
    fn history_reports_an_unreachable_service() {
        let refused = io::Error::new(io::ErrorKind::NotFound, "no pipe");
        let e = history(Err(refused), 1, 1, &mut Vec::new()).unwrap_err();
        assert!(
            e.to_string().contains("service not reachable (no pipe)"),
            "{e}"
        );
    }
}

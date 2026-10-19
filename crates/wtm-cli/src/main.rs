#![forbid(unsafe_code)]

mod actions;
mod parse;
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
    },
    /// Terminate processes
    Kill {
        #[arg(required = true)]
        targets: Vec<String>,
        /// Also terminate all descendants
        #[arg(long, short)]
        tree: bool,
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
        cpus: Option<String>,
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

fn main() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();
    // Lets an elevated prompt control services and other users' processes.
    control::enable_debug_privilege();

    match cli.command {
        Command::List {
            sort,
            limit,
            sample_ms,
        } => list(sort.into(), limit, sample_ms).map(|()| ExitCode::SUCCESS),
        Command::Top { sort, interval_ms } => {
            top::run(sort.into(), Duration::from_millis(interval_ms.max(100)))
                .map(|()| ExitCode::SUCCESS)
        }
        Command::Info { targets } => actions::info(&targets),
        Command::History { hours, limit } => history(hours, limit).map(|()| ExitCode::SUCCESS),
        Command::Kill { targets, tree } => actions::kill(&targets, tree),
        Command::Suspend { targets } => actions::suspend(&targets),
        Command::Resume { targets } => actions::resume(&targets),
        Command::Priority { target, class } => actions::priority(&[target], class.into()),
        Command::Affinity { target, cpus } => actions::affinity(&[target], cpus.as_deref()),
        Command::Eco { target, mode } => actions::eco(&[target], mode.into()),
        Command::Limit {
            targets,
            cpu,
            memory,
            detach,
        } => actions::limit(&targets, cpu, memory, detach),
    }
}

fn list(sort: SortKey, limit: Option<usize>, sample_ms: u64) -> anyhow::Result<()> {
    let mut sampler = Sampler::new();
    sampler.sample()?;
    std::thread::sleep(Duration::from_millis(sample_ms.max(100)));
    let mut snap = sampler.sample()?;
    sort_processes(&mut snap.processes, sort);

    let mut out = io::stdout().lock();
    writeln!(out, "{}", table::summary(&snap.system))?;
    writeln!(out, "{}", table::header())?;
    for p in snap.processes.iter().take(limit.unwrap_or(usize::MAX)) {
        writeln!(out, "{}", table::row(p, usize::MAX))?;
    }
    Ok(())
}

fn history(hours: u32, limit: usize) -> anyhow::Result<()> {
    use wtm_ipc::{Client, Request, Response};
    let mut client = Client::connect().map_err(|e| {
        anyhow::anyhow!("RustWTM service not reachable ({e}); history is recorded by the service")
    })?;
    let (stats, findings) = match client.call(&Request::History { hours })? {
        Response::History { stats, findings } => (stats, findings),
        Response::Error(e) => anyhow::bail!(e),
        other => anyhow::bail!("unexpected response: {other:?}"),
    };
    println!(
        "{:<28} {:>8} {:>8} {:>8} {:>10} {:>10} {:>10}",
        "APP", "SEEN", "AVG CPU", "P95 CPU", "AVG MEM", "PEAK MEM", "GROWTH/H"
    );
    for s in stats.iter().take(limit) {
        println!(
            "{:<28} {:>7}m {:>7.2}% {:>7.2}% {:>10} {:>10} {:>+9.0}M",
            s.app,
            s.minutes,
            s.cpu_avg,
            s.cpu_p95,
            table::bytes(s.mem_avg as f64),
            table::bytes(s.mem_peak as f64),
            s.mem_growth_per_hour / (1024.0 * 1024.0)
        );
    }
    if findings.is_empty() {
        println!("\nNo long-term findings.");
    } else {
        println!("\nFindings:");
        for f in findings {
            println!("  {:<26} {:?}: {}", f.app, f.kind, f.detail);
        }
    }
    Ok(())
}

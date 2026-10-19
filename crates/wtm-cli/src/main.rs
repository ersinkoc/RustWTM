#![forbid(unsafe_code)]

mod actions;
mod config;
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
        Command::History { hours, limit } => history(
            wtm_ipc::Client::connect(),
            hours,
            limit,
            &mut io::stdout().lock(),
        )
        .map(|()| ExitCode::SUCCESS),
        Command::Config { action } => {
            config::run(action, wtm_ipc::Client::connect, &mut io::stdout().lock())
        }
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
        }
    }

    /// Runs `f` with a client of a one-shot fake service that answers one
    /// request with `reply`; returns that request and `f`'s result.
    pub(crate) fn with_reply<R>(
        reply: Response,
        f: impl FnOnce(io::Result<Client>) -> R,
    ) -> (Option<Request>, R) {
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
            let mut conn = listener.accept().ok()?;
            let hello: Request = read_frame(&mut conn).ok()?;
            assert!(matches!(hello, Request::Hello { .. }));
            let hello = Response::Hello {
                version: PROTOCOL_VERSION,
                admin: false,
            };
            write_frame(&mut conn, &hello).ok()?;
            let request: Request = read_frame(&mut conn).ok()?;
            write_frame(&mut conn, &reply).ok()?;
            Some(request)
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

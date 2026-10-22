#![forbid(unsafe_code)]

/// Unwraps one `wtm_ipc::Response` variant, failing the test on any other reply.
#[cfg(test)]
macro_rules! expect_reply {
    ($response:expr, $variant:path) => {
        match $response {
            $variant(v) => v,
            other => panic!("unexpected reply: {other:?}"),
        }
    };
}

mod app_limits;
mod blocker;
mod engine;
mod events;
mod executor;
mod hardware;
mod history;
mod ipc_server;
mod logging;
mod rescue;
mod service;
mod telegram;

#[cfg(test)]
mod test_util {
    use std::time::{Duration, Instant};

    /// Polls `f` until it yields a value, failing the test after 30 s.
    pub fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};

use clap::{Parser, Subcommand};

use crate::ipc_server::EngineMsg;

#[derive(Parser)]
#[command(name = "wtm-service", version, about = "RustWTM governor service")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Register as an auto-start Windows service running as LocalSystem (elevated)
    Install,
    /// Stop and remove the service, then clean up as `cleanup` does (elevated)
    Uninstall,
    /// Remove RustWTM's blocked-program entries and firewall rules (elevated;
    /// run by the MSI on uninstall)
    Cleanup,
    /// Start the installed service
    Start,
    /// Stop the installed service; all limits are rolled back
    Stop,
    /// Run the engine in the foreground; Ctrl+C stops and rolls back
    Console {
        /// Directory holding config.toml and logs (default: %ProgramData%\RustWTM)
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Entry point used by the Service Control Manager
    #[command(hide = true)]
    Service,
    /// Make DIR a folder only SYSTEM and Administrators can change (used by
    /// install.ps1; elevated)
    #[command(hide = true)]
    PrepareInstallDir { dir: PathBuf },
}

fn main() -> anyhow::Result<()> {
    run(Cli::parse().command, |stop| {
        ctrlc::set_handler(move || {
            let _ = stop.send(EngineMsg::Stop);
        })?;
        Ok(())
    })
}

/// `on_interrupt` arranges for [`EngineMsg::Stop`] to be sent to the console
/// engine when the user interrupts it.
fn run(
    command: Command,
    on_interrupt: impl FnOnce(Sender<EngineMsg>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    match command {
        Command::Install => service::install(),
        Command::Uninstall => service::uninstall(),
        Command::Cleanup => service::cleanup(),
        Command::Start => service::start(),
        Command::Stop => service::stop(),
        Command::Service => service::dispatch(),
        Command::PrepareInstallDir { dir } => service::prepare_install_dir(&dir),
        Command::Console { data_dir } => {
            let paths = engine::Paths::under(&data_dir.unwrap_or_else(engine::default_data_dir));
            let (tx, rx) = mpsc::channel();
            on_interrupt(tx.clone())?;
            let _guard = logging::init(&paths.logs, true);
            engine::run(&paths, tx, &rx)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_subcommand() {
        let parse = |args: &[&str]| Cli::try_parse_from(["wtm-service"].iter().chain(args));
        for (args, expected) in [
            (&["install"][..], "Install"),
            (&["uninstall"], "Uninstall"),
            (&["cleanup"], "Cleanup"),
            (&["start"], "Start"),
            (&["stop"], "Stop"),
            (&["service"], "Service"),
            (&["console"], "Console"),
            (
                &["prepare-install-dir", r"C:\Program Files\RustWTM"],
                "PrepareInstallDir",
            ),
        ] {
            let name = match parse(args).unwrap().command {
                Command::Install => "Install",
                Command::Uninstall => "Uninstall",
                Command::Cleanup => "Cleanup",
                Command::Start => "Start",
                Command::Stop => "Stop",
                Command::Service => "Service",
                Command::PrepareInstallDir { dir } => {
                    assert_eq!(dir, PathBuf::from(r"C:\Program Files\RustWTM"));
                    "PrepareInstallDir"
                }
                Command::Console { data_dir } => {
                    assert!(data_dir.is_none());
                    "Console"
                }
            };
            assert_eq!(name, expected);
        }
        let console = parse(&["console", "--data-dir", r"C:\x"]).unwrap();
        assert!(
            matches!(console.command, Command::Console { data_dir: Some(d) } if d == std::path::Path::new(r"C:\x"))
        );
        assert!(parse(&["bogus"]).is_err());
        assert!(parse(&[]).is_err());
    }

    /// Install/uninstall/start/stop change the machine; they are exercised
    /// by the gated SCM tests.
    #[test]
    fn console_runs_until_interrupted_and_service_needs_the_scm() {
        let dir = std::env::temp_dir().join(format!("wtm-console-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let console = || Command::Console {
            data_dir: Some(dir.clone()),
        };

        // Failing to install the interrupt handler aborts before the engine starts.
        let err = run(console(), |_| anyhow::bail!("no handler")).unwrap_err();
        assert_eq!(err.to_string(), "no handler");
        assert!(!dir.join("config.toml").exists());

        // An immediate interrupt: the engine starts, writes its config and stops.
        run(console(), |stop| {
            stop.send(EngineMsg::Stop)?;
            Ok(())
        })
        .unwrap();
        assert!(dir.join("config.toml").exists());
        assert!(dir.join("logs").is_dir());

        assert!(run(Command::Service, |_| Ok(())).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

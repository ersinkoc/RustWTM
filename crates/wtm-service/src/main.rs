#![forbid(unsafe_code)]

mod engine;
mod events;
mod executor;
mod history;
mod ipc_server;
mod logging;
mod service;

use std::path::PathBuf;
use std::sync::mpsc;

use clap::{Parser, Subcommand};

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
    /// Stop and remove the service (elevated)
    Uninstall,
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
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Install => service::install(),
        Command::Uninstall => service::uninstall(),
        Command::Start => service::start(),
        Command::Stop => service::stop(),
        Command::Service => service::dispatch(),
        Command::Console { data_dir } => {
            let paths = engine::Paths::under(&data_dir.unwrap_or_else(engine::default_data_dir));
            let _guard = logging::init(&paths.logs, true);
            let (tx, rx) = mpsc::channel();
            let stop_tx = tx.clone();
            ctrlc::set_handler(move || {
                let _ = stop_tx.send(ipc_server::EngineMsg::Stop);
            })?;
            engine::run(&paths, tx, &rx)
        }
    }
}

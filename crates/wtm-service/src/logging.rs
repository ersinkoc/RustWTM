use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

/// Logs to a daily rolling file and, in console mode, to stdout.
/// Keep the returned guard alive to flush the file on exit.
pub fn init(logs_dir: &Path, console: bool) -> WorkerGuard {
    let _ = std::fs::create_dir_all(logs_dir);
    let (writer, guard) = tracing_appender::non_blocking(tracing_appender::rolling::daily(
        logs_dir,
        "wtm-service.log",
    ));

    tracing_subscriber::registry()
        .with(LevelFilter::INFO)
        .with(fmt::layer().with_writer(writer).with_ansi(false))
        .with(console.then(|| fmt::layer().with_writer(std::io::stdout)))
        .init();
    guard
}

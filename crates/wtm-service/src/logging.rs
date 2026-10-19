use std::path::Path;

use tracing::Subscriber;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

/// Logs to a daily rolling file and, in console mode, to stdout.
/// Keep the returned guard alive to flush the file on exit.
pub fn init(logs_dir: &Path, console: bool) -> WorkerGuard {
    let (subscriber, guard) = subscriber(logs_dir, console);
    subscriber.init();
    guard
}

fn subscriber(
    logs_dir: &Path,
    console: bool,
) -> (impl Subscriber + Send + Sync + 'static, WorkerGuard) {
    let _ = std::fs::create_dir_all(logs_dir);
    let (writer, guard) = tracing_appender::non_blocking(tracing_appender::rolling::daily(
        logs_dir,
        "wtm-service.log",
    ));

    let subscriber = tracing_subscriber::registry()
        .with(LevelFilter::INFO)
        .with(fmt::layer().with_writer(writer).with_ansi(false))
        .with(console.then(|| fmt::layer().with_writer(std::io::stdout)));
    (subscriber, guard)
}

#[cfg(test)]
mod tests {
    #[test]
    fn writes_to_a_daily_file() {
        let dir = std::env::temp_dir().join(format!("wtm-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Scoped: the global subscriber belongs to the console-mode test.
        let (subscriber, guard) = super::subscriber(&dir, false);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("hello from the logging test");
            tracing::debug!("below the level filter");
        });
        drop(guard); // flushes the non-blocking writer
        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(files.len(), 1);
        let text = std::fs::read_to_string(files[0].path()).unwrap();
        assert!(text.contains("hello from the logging test"));
        assert!(!text.contains("below the level filter"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! "Run new task" and restarting RustWTM with administrator rights.

use std::path::Path;

/// Whether this process has administrator rights; `false` if unknown.
pub fn is_elevated() -> bool {
    wtm_sys::shell::is_elevated().unwrap_or(false)
}

/// Splits a Run-dialog line into the program and its arguments. A quoted
/// first word may contain spaces; so may an unquoted line that names an
/// existing file as a whole.
pub fn split_command(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if let Some(rest) = line.strip_prefix('"') {
        let (file, args) = rest.split_once('"').unwrap_or((rest, ""));
        return Some((file.to_owned(), args.trim().to_owned()));
    }
    if Path::new(line).is_file() {
        return Some((line.to_owned(), String::new()));
    }
    let (file, args) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    Some((file.to_owned(), args.trim().to_owned()))
}

/// Starts `line` like the Run dialog, optionally elevated (UAC prompt).
pub fn run(line: &str, elevated: bool) -> Result<(), String> {
    let (file, args) = split_command(line).ok_or("nothing to run")?;
    wtm_sys::shell::launch(&file, &args, elevated).map_err(|e| describe(&file, e.is_cancelled(), e))
}

/// Starts this program again with administrator rights. The caller exits
/// once it returns `Ok`; the new copy is told to wait for that (see
/// [`REPLACE_ARG`]) instead of finding this one and closing again.
pub fn restart_elevated() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy();
    let args = format!("{REPLACE_ARG} {}", std::process::id());
    wtm_sys::shell::launch(&exe, &args, true).map_err(|e| describe(&exe, e.is_cancelled(), e))
}

/// `--replace <pid>`: this copy takes over from process `pid`, which is
/// about to exit.
pub const REPLACE_ARG: &str = "--replace";

/// Waits until process `pid` has exited, at most `timeout`. Returns whether
/// it is gone; a process that cannot be opened counts as gone.
pub fn wait_for_exit(pid: u32, timeout: std::time::Duration) -> bool {
    use wtm_sys::control::{Access, ProcessHandle};
    match ProcessHandle::open(pid, Access::SYNCHRONIZE) {
        Ok(process) => process.wait_exit(timeout).unwrap_or(true),
        Err(_) => true,
    }
}

fn describe(file: &str, cancelled: bool, e: impl std::fmt::Display) -> String {
    if cancelled {
        "cancelled at the administrator prompt".to_owned()
    } else {
        format!("cannot start {file}: {e}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(line: &str) -> Option<(&str, &str)> {
        // Leaks are fine in a test and keep the assertions short.
        split_command(line).map(|(f, a)| (&*f.leak(), &*a.leak()))
    }

    #[test]
    fn splits_program_and_arguments() {
        assert_eq!(split("  "), None);
        assert_eq!(split("notepad"), Some(("notepad", "")));
        assert_eq!(split("notepad  a b.txt "), Some(("notepad", "a b.txt")));
        assert_eq!(
            split(r#""C:\Program Files\App\app.exe" --flag"#),
            Some((r"C:\Program Files\App\app.exe", "--flag"))
        );
        assert_eq!(split(r#""unterminated"#), Some(("unterminated", "")));
        let exe = std::env::current_exe().unwrap();
        let exe = exe.to_str().unwrap();
        assert_eq!(split(exe), Some((exe, "")));
    }

    #[test]
    fn run_reports_problems() {
        assert_eq!(run(" ", false).unwrap_err(), "nothing to run");
        let err = run(r"C:\definitely\missing-program.exe", false).unwrap_err();
        assert!(
            err.starts_with(r"cannot start C:\definitely\missing-program.exe: "),
            "{err}"
        );
        assert_eq!(
            describe("x", true, "The operation was canceled by the user."),
            "cancelled at the administrator prompt"
        );
        // Either answer is fine; it must not panic.
        let _ = is_elevated();
    }

    #[test]
    fn waits_for_a_replaced_process_to_exit() {
        use std::time::{Duration, Instant};
        // A PID that cannot be opened counts as gone at once.
        assert!(wait_for_exit(0xFFFF_FFF0, Duration::from_secs(10)));
        // Ourselves: still running, so the wait ends after the timeout.
        let started = Instant::now();
        assert!(!wait_for_exit(
            std::process::id(),
            Duration::from_millis(50)
        ));
        assert!(started.elapsed() >= Duration::from_millis(40));
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "exit"])
            .spawn()
            .unwrap();
        assert!(wait_for_exit(child.id(), Duration::from_secs(10)));
        let _ = child.wait();
    }
}

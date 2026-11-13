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
/// Without "as administrator", a RustWTM that runs elevated still starts it
/// with the user's standard rights, through the desktop's Explorer; only
/// when no Explorer shows the desktop does it start with RustWTM's rights.
pub fn run(line: &str, elevated: bool) -> Result<(), String> {
    run_with(line, elevated, is_elevated(), |file, args| {
        match wtm_sys::shell::launch_as_desktop_user(file, args) {
            Ok(()) => Desktop::Started,
            Err(e) if wtm_sys::shell::is_timeout(&e) => Desktop::NoAnswer,
            Err(_) => Desktop::Unavailable,
        }
    })
}

/// What the desktop's Explorer did with a launch request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Desktop {
    Started,
    /// It did not answer in time and may still start the program.
    NoAnswer,
    /// No desktop shell could be asked.
    Unavailable,
}

/// [`run`] with the desktop launch passed in, so tests need no desktop.
fn run_with(
    line: &str,
    elevated: bool,
    running_elevated: bool,
    desktop: impl FnOnce(&str, &str) -> Desktop,
) -> Result<(), String> {
    let (file, args) = split_command(line).ok_or("nothing to run")?;
    if through_desktop(elevated, running_elevated) {
        match desktop(&file, &args) {
            Desktop::Started => return Ok(()),
            // Explorer may still start it: starting it here too could run it twice.
            Desktop::NoAnswer => return Err("Windows Explorer did not answer; try again".into()),
            // Start it with this program's rights instead.
            Desktop::Unavailable => {}
        }
    }
    wtm_sys::shell::launch(&file, &args, elevated).map_err(|e| describe(&file, e.is_cancelled(), e))
}

/// A standard-rights request from an elevated RustWTM goes through the
/// desktop's Explorer; everything else is started directly.
fn through_desktop(as_administrator: bool, running_elevated: bool) -> bool {
    !as_administrator && running_elevated
}

/// Starts this program again with administrator rights. The caller exits
/// once it returns `Ok`; the new copy is told to wait for that (see
/// [`REPLACE_ARG`]) instead of finding this one and closing again.
pub fn restart_elevated() -> Result<(), String> {
    restart_elevated_with(false)
}

pub fn restart_elevated_with(background: bool) -> Result<(), String> {
    if crate::gui_startup::launch_task(background).is_ok() {
        return Ok(());
    }
    restart_with_uac(background)
}

/// Direct fallback when a scheduled launch did not receive administrator rights.
/// Do not re-enter the same scheduled task and create a relaunch loop.
pub fn restart_with_uac(background: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy();
    let args = format!(
        "{REPLACE_ARG} {}{}",
        std::process::id(),
        if background { " --background" } else { "" }
    );
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

    #[test]
    fn only_standard_requests_from_an_elevated_window_go_through_the_desktop() {
        assert!(through_desktop(false, true));
        assert!(
            !through_desktop(true, true),
            "administrator requests keep UAC"
        );
        assert!(!through_desktop(false, false), "already standard rights");
        assert!(!through_desktop(true, false));
    }

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
        const MISSING: &str = r"C:\definitely\missing-program.exe";
        let unused = |_: &str, _: &str| -> Desktop { panic!("asked the desktop") };
        assert_eq!(
            run_with(" ", false, true, unused).unwrap_err(),
            "nothing to run"
        );
        let cannot_start = |err: String| {
            assert!(
                err.starts_with(&format!("cannot start {MISSING}: ")),
                "{err}"
            );
        };
        // Already running with standard rights: started here.
        cannot_start(run_with(MISSING, false, false, unused).unwrap_err());
        // From an elevated window the desktop is asked first.
        let mut asked = None;
        let started = run_with(&format!("\"{MISSING}\" -x"), false, true, |file, args| {
            asked = Some((file.to_owned(), args.to_owned()));
            Desktop::Started
        });
        assert_eq!(started, Ok(()));
        assert_eq!(asked, Some((MISSING.to_owned(), "-x".to_owned())));
        // No answer is not retried here: Explorer might still start it.
        assert_eq!(
            run_with(MISSING, false, true, |_, _| Desktop::NoAnswer).unwrap_err(),
            "Windows Explorer did not answer; try again"
        );
        // Without a desktop shell it falls back to starting it here.
        cannot_start(run_with(MISSING, false, true, |_, _| Desktop::Unavailable).unwrap_err());
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

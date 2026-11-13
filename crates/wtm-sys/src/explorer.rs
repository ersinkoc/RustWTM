//! Restarting the Explorer that shows the desktop and taskbar.
//!
//! The replacement is created first, suspended, as a child of the old
//! Explorer, so it inherits that Explorer's account and rights (a standard
//! user's Explorer stays standard even when RustWTM runs as administrator).
//! The old one is asked to close and is ended only if it does not; the new
//! one then runs, unless Windows has already brought a desktop back itself.

use std::path::PathBuf;
use std::time::Duration;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetShellWindow, GetWindowThreadProcessId, PostMessageW, WM_USER,
};
use windows::core::w;

use crate::Result;
use crate::spawn::Spawned;

/// The process showing the desktop, if one does.
pub fn desktop_pid() -> Option<u32> {
    // SAFETY: plain FFI calls; a null window yields no PID.
    unsafe {
        let window = GetShellWindow();
        if window.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, Some(&mut pid));
        (pid != 0).then_some(pid)
    }
}

/// How a restart ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restarted {
    /// The old Explorer closed when asked, and the new one runs.
    Closed,
    /// The old Explorer did not close in time and was ended.
    Ended,
    /// Windows restored a desktop by itself; the prepared copy was dropped.
    RestoredByWindows,
}

/// The steps of a restart, so the order can be tested without touching the
/// real desktop.
trait Desktop {
    fn desktop_pid(&self) -> Option<u32>;
    /// Prepares the replacement, suspended, under `parent`; its PID.
    fn prepare(&mut self, parent: u32) -> Result<u32>;
    /// Asks the taskbar's Explorer to close the way "Exit Explorer" does.
    fn ask_to_close(&self);
    fn wait_exit(&self, pid: u32, timeout: Duration) -> bool;
    fn end(&self, pid: u32) -> Result<()>;
    fn run_prepared(&mut self) -> Result<()>;
    fn drop_prepared(&mut self);
    fn pause(&self, time: Duration);
}

const ASK_TIMEOUT: Duration = Duration::from_secs(6);
const END_TIMEOUT: Duration = Duration::from_secs(4);
/// How long to watch for a desktop Windows brings back on its own.
const RESTORE_WATCH: Duration = Duration::from_millis(1500);
const STEP: Duration = Duration::from_millis(100);

fn restart_with(desktop: &mut impl Desktop) -> std::result::Result<Restarted, String> {
    let old = desktop
        .desktop_pid()
        .ok_or("No Explorer is showing the desktop; start explorer.exe instead.")?;
    let new = desktop.prepare(old).map_err(|e| e.to_string())?;
    desktop.ask_to_close();
    let mut outcome = Restarted::Closed;
    if !desktop.wait_exit(old, ASK_TIMEOUT) {
        if let Err(e) = desktop.end(old) {
            desktop.drop_prepared();
            return Err(e.to_string());
        }
        if !desktop.wait_exit(old, END_TIMEOUT) {
            desktop.drop_prepared();
            return Err("Explorer did not end.".into());
        }
        outcome = Restarted::Ended;
    }
    let mut watched = Duration::ZERO;
    while watched < RESTORE_WATCH {
        if desktop
            .desktop_pid()
            .is_some_and(|pid| pid != old && pid != new)
        {
            desktop.drop_prepared();
            return Ok(Restarted::RestoredByWindows);
        }
        desktop.pause(STEP);
        watched += STEP;
    }
    desktop.run_prepared().map_err(|e| e.to_string())?;
    Ok(outcome)
}

struct Real {
    explorer: PathBuf,
    prepared: Option<Spawned>,
}

impl Desktop for Real {
    fn desktop_pid(&self) -> Option<u32> {
        desktop_pid()
    }

    fn prepare(&mut self, parent: u32) -> Result<u32> {
        let spawned = crate::spawn::with_parent(parent, &self.explorer, "", true)?;
        let pid = spawned.pid;
        self.prepared = Some(spawned);
        Ok(pid)
    }

    fn ask_to_close(&self) {
        // SAFETY: plain FFI calls; a missing taskbar window is ignored. The
        // message is the one the taskbar's own "Exit Explorer" command sends.
        unsafe {
            if let Ok(tray) = FindWindowW(w!("Shell_TrayWnd"), None) {
                let _ = PostMessageW(Some(tray), WM_USER + 436, WPARAM(0), LPARAM(0));
            }
        }
    }

    fn wait_exit(&self, pid: u32, timeout: Duration) -> bool {
        // A process that can no longer be opened has ended.
        crate::spawn::wait_for_exit(pid, timeout).unwrap_or(true)
    }

    fn end(&self, pid: u32) -> Result<()> {
        crate::spawn::terminate(pid)
    }

    fn run_prepared(&mut self) -> Result<()> {
        match self.prepared.take() {
            Some(spawned) => spawned.resume(),
            None => Ok(()),
        }
    }

    fn drop_prepared(&mut self) {
        if let Some(spawned) = self.prepared.take() {
            let _ = spawned.discard();
        }
    }

    fn pause(&self, time: Duration) {
        std::thread::sleep(time);
    }
}

impl Drop for Real {
    fn drop(&mut self) {
        // An error path must not leave a suspended Explorer behind.
        self.drop_prepared();
    }
}

/// Restarts the desktop's Explorer. Blocks for up to about ten seconds.
pub fn restart() -> std::result::Result<Restarted, String> {
    let windows = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let mut real = Real {
        explorer: windows.join("explorer.exe"),
        prepared: None,
    };
    restart_with(&mut real)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// A desktop whose Explorer behaves as the test says; it records the
    /// steps taken.
    #[derive(Default)]
    struct Fake {
        desktop: Cell<Option<u32>>,
        closes_when_asked: bool,
        ends_when_ended: bool,
        windows_restores: Option<u32>,
        steps: RefCell<Vec<String>>,
    }

    impl Fake {
        fn log(&self, step: impl Into<String>) {
            self.steps.borrow_mut().push(step.into());
        }
    }

    impl Desktop for Fake {
        fn desktop_pid(&self) -> Option<u32> {
            self.desktop.get()
        }
        fn prepare(&mut self, parent: u32) -> Result<u32> {
            self.log(format!("prepare under {parent}"));
            Ok(900)
        }
        fn ask_to_close(&self) {
            self.log("ask");
        }
        fn wait_exit(&self, pid: u32, _: Duration) -> bool {
            let gone = match self.steps.borrow().last().map(String::as_str) {
                Some("ask") => self.closes_when_asked,
                _ => self.ends_when_ended,
            };
            if gone {
                self.desktop.set(self.windows_restores);
            }
            self.log(format!("wait {pid} {gone}"));
            gone
        }
        fn end(&self, pid: u32) -> Result<()> {
            self.log(format!("end {pid}"));
            Ok(())
        }
        fn run_prepared(&mut self) -> Result<()> {
            self.log("run new");
            Ok(())
        }
        fn drop_prepared(&mut self) {
            self.log("drop new");
        }
        fn pause(&self, _: Duration) {}
    }

    fn fake(closes: bool, ends: bool, restores: Option<u32>) -> Fake {
        Fake {
            desktop: Cell::new(Some(100)),
            closes_when_asked: closes,
            ends_when_ended: ends,
            windows_restores: restores,
            ..Fake::default()
        }
    }

    #[test]
    fn a_polite_close_runs_the_prepared_copy() {
        let mut d = fake(true, true, None);
        assert_eq!(restart_with(&mut d), Ok(Restarted::Closed));
        assert_eq!(
            *d.steps.borrow(),
            ["prepare under 100", "ask", "wait 100 true", "run new"]
        );
    }

    #[test]
    fn a_stuck_explorer_is_ended_before_the_copy_runs() {
        let mut d = fake(false, true, None);
        assert_eq!(restart_with(&mut d), Ok(Restarted::Ended));
        assert_eq!(
            *d.steps.borrow(),
            [
                "prepare under 100",
                "ask",
                "wait 100 false",
                "end 100",
                "wait 100 true",
                "run new"
            ]
        );
    }

    #[test]
    fn a_desktop_windows_restored_wins_over_the_copy() {
        let mut d = fake(false, true, Some(555));
        assert_eq!(restart_with(&mut d), Ok(Restarted::RestoredByWindows));
        assert_eq!(d.steps.borrow().last().unwrap(), "drop new");
        assert!(!d.steps.borrow().iter().any(|s| s == "run new"));
    }

    #[test]
    fn failures_never_leave_the_copy_waiting() {
        let mut d = fake(false, false, None);
        assert!(restart_with(&mut d).is_err());
        assert_eq!(d.steps.borrow().last().unwrap(), "drop new");
        let mut none = Fake::default();
        assert!(
            restart_with(&mut none).is_err(),
            "no desktop: nothing prepared"
        );
        assert!(none.steps.borrow().is_empty());
    }

    #[test]
    fn the_desktop_process_can_be_looked_up() {
        // Either answer is fine (CI may have no desktop); the query must not fail.
        let _ = desktop_pid();
    }
}

//! Starting programs with a chosen parent, or past an Image File Execution
//! Options debugger.
//!
//! A process started "under" another one inherits that parent's token: a
//! program started under the desktop's Explorer runs with the user's
//! standard rights even when RustWTM itself runs as administrator.

use std::path::Path;
use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Diagnostics::Debug::{
    DebugActiveProcessStop, DebugSetProcessKillOnExit,
};
use windows::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, DEBUG_ONLY_THIS_PROCESS, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, OpenProcess, PROC_THREAD_ATTRIBUTE_PARENT_PROCESS,
    PROCESS_CREATE_PROCESS, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, ResumeThread,
    STARTUPINFOEXW, STARTUPINFOW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

use crate::Result;
use crate::handle::OwnedHandle;

/// A process this module started, with its first thread.
#[derive(Debug)]
pub struct Spawned {
    pub pid: u32,
    process: OwnedHandle,
    thread: OwnedHandle,
}

impl Spawned {
    fn from(info: PROCESS_INFORMATION) -> Self {
        Self {
            pid: info.dwProcessId,
            process: OwnedHandle::new(info.hProcess),
            thread: OwnedHandle::new(info.hThread),
        }
    }

    /// Lets a process started suspended run.
    pub fn resume(&self) -> Result<()> {
        // SAFETY: we own the thread handle, opened with full access.
        if unsafe { ResumeThread(self.thread.raw()) } == u32::MAX {
            return Err(windows::core::Error::from_thread().into());
        }
        Ok(())
    }

    /// Ends a process started suspended that is no longer wanted.
    pub fn discard(&self) -> Result<()> {
        // SAFETY: we own the process handle, opened with full access.
        unsafe { TerminateProcess(self.process.raw(), 1)? };
        Ok(())
    }

    /// Waits up to `timeout` for the process to end; its exit code, if it did.
    pub fn wait(&self, timeout: Duration) -> Option<u32> {
        wait_exit(self.process.raw(), timeout)
    }
}

fn wait_exit(process: HANDLE, timeout: Duration) -> Option<u32> {
    let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
    // SAFETY: a live process handle with SYNCHRONIZE access.
    if unsafe { WaitForSingleObject(process, ms) } != WAIT_OBJECT_0 {
        return None;
    }
    let mut code = 0u32;
    // SAFETY: as above, with query access; `code` is writable.
    unsafe { GetExitCodeProcess(process, &mut code) }.ok()?;
    Some(code)
}

/// The command line Windows passes to a program: its quoted path, then
/// `arguments` as given.
fn command_line(application: &Path, arguments: &str) -> Vec<u16> {
    let mut line = format!("\"{}\"", application.display());
    if !arguments.is_empty() {
        line.push(' ');
        line.push_str(arguments);
    }
    line.encode_utf16().chain([0]).collect()
}

/// A process attribute list with one entry: the parent process.
struct ParentAttribute {
    buffer: Vec<u64>,
    /// Kept alive for the list, which points at it.
    _parent: Box<HANDLE>,
}

impl ParentAttribute {
    fn new(parent: HANDLE) -> Result<Self> {
        let mut size = 0usize;
        // SAFETY: a null list only reports the size one attribute needs.
        let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &mut size) };
        let mut buffer = vec![0u64; size.div_ceil(8).max(1)];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(buffer.as_mut_ptr().cast());
        // SAFETY: `buffer` is writable for `size` bytes.
        unsafe { InitializeProcThreadAttributeList(Some(list), 1, None, &mut size)? };
        let parent = Box::new(parent);
        let attribute = Self {
            buffer,
            _parent: parent,
        };
        // SAFETY: the list was initialised above; the value is a HANDLE that
        // lives in the box as long as the list.
        unsafe {
            UpdateProcThreadAttribute(
                attribute.list(),
                0,
                PROC_THREAD_ATTRIBUTE_PARENT_PROCESS as usize,
                Some(std::ptr::from_ref::<HANDLE>(&attribute._parent).cast()),
                size_of::<HANDLE>(),
                None,
                None,
            )?
        };
        Ok(attribute)
    }

    fn list(&self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        LPPROC_THREAD_ATTRIBUTE_LIST(self.buffer.as_ptr().cast_mut().cast())
    }
}

impl Drop for ParentAttribute {
    fn drop(&mut self) {
        // SAFETY: the list was initialised in `new` and is deleted once.
        unsafe { DeleteProcThreadAttributeList(self.list()) };
    }
}

/// Starts `application` with `arguments` as a child of process `parent`,
/// inheriting that process's account, rights and session. `suspended` leaves
/// it waiting for [`Spawned::resume`].
pub fn with_parent(
    parent: u32,
    application: &Path,
    arguments: &str,
    suspended: bool,
) -> Result<Spawned> {
    // SAFETY: plain FFI call; the handle is owned below.
    let parent = OwnedHandle::new(unsafe { OpenProcess(PROCESS_CREATE_PROCESS, false, parent)? });
    let attribute = ParentAttribute::new(parent.raw())?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attribute.list();
    let mut flags = EXTENDED_STARTUPINFO_PRESENT;
    if suspended {
        flags |= CREATE_SUSPENDED;
    }
    create(application, arguments, flags, &startup.StartupInfo)
}

/// Starts `application` so that an Image File Execution Options "Debugger"
/// value for it is not followed: it starts as a debuggee of this process,
/// which lets go of it at once. Used to open the real Windows Task Manager
/// once while RustWTM replaces it.
pub fn bypassing_debugger(application: &Path, arguments: &str) -> Result<Spawned> {
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let spawned = create(application, arguments, DEBUG_ONLY_THIS_PROCESS, &startup)?;
    // SAFETY: plain FFI calls about the process this thread just started.
    let detached = unsafe {
        DebugSetProcessKillOnExit(false).and_then(|()| DebugActiveProcessStop(spawned.pid))
    };
    if let Err(e) = detached {
        // Never leave a frozen debuggee behind.
        let _ = spawned.discard();
        return Err(e.into());
    }
    Ok(spawned)
}

fn create(
    application: &Path,
    arguments: &str,
    flags: PROCESS_CREATION_FLAGS,
    startup: *const STARTUPINFOW,
) -> Result<Spawned> {
    let app: Vec<u16> = application.as_os_str().encode_wide_nul();
    let mut line = command_line(application, arguments);
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: every string is NUL-terminated and outlives the call; the
    // command line buffer is writable as CreateProcessW requires.
    unsafe {
        CreateProcessW(
            PCWSTR(app.as_ptr()),
            Some(PWSTR(line.as_mut_ptr())),
            None,
            None,
            false,
            flags,
            None,
            PCWSTR::null(),
            startup,
            &mut info,
        )?
    };
    Ok(Spawned::from(info))
}

trait EncodeWideNul {
    fn encode_wide_nul(&self) -> Vec<u16>;
}

impl EncodeWideNul for std::ffi::OsStr {
    fn encode_wide_nul(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain([0]).collect()
    }
}

/// Waits up to `timeout` for process `pid` to end. `Some(true)` when it
/// ended, `Some(false)` when it still runs, `None` when it cannot be watched.
pub fn wait_for_exit(pid: u32, timeout: Duration) -> Option<bool> {
    // SAFETY: plain FFI call; the handle is owned below.
    let process = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
    }
    .ok()?;
    let process = OwnedHandle::new(process);
    Some(wait_exit(process.raw(), timeout).is_some())
}

/// Ends process `pid` (an Explorer that did not close when asked).
pub fn terminate(pid: u32) -> Result<()> {
    // SAFETY: plain FFI calls; the handle is owned below.
    let process = OwnedHandle::new(unsafe { OpenProcess(PROCESS_TERMINATE, false, pid)? });
    // SAFETY: a live handle with terminate access.
    unsafe { TerminateProcess(process.raw(), 1)? };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd() -> std::path::PathBuf {
        crate::shell::system_directory().unwrap().join("cmd.exe")
    }

    #[test]
    fn command_lines_quote_the_program_path() {
        let line = command_line(Path::new(r"C:\Program Files\x.exe"), "/a b");
        assert_eq!(
            String::from_utf16_lossy(&line),
            "\"C:\\Program Files\\x.exe\" /a b\0"
        );
        let bare = command_line(Path::new(r"C:\x.exe"), "");
        assert_eq!(String::from_utf16_lossy(&bare), "\"C:\\x.exe\"\0");
    }

    #[test]
    fn a_child_of_a_chosen_parent_runs_and_reports_its_exit_code() {
        // Parent: this test process. One short-lived console command.
        let child = with_parent(std::process::id(), &cmd(), "/d /c exit 7", true).unwrap();
        assert_eq!(
            child.wait(Duration::from_millis(100)),
            None,
            "suspended until resumed"
        );
        child.resume().unwrap();
        assert_eq!(child.wait(Duration::from_secs(10)), Some(7));
    }

    #[test]
    fn a_suspended_child_can_be_discarded() {
        let child = with_parent(std::process::id(), &cmd(), "/d /c exit 0", true).unwrap();
        child.discard().unwrap();
        assert_eq!(child.wait(Duration::from_secs(10)), Some(1));
    }

    #[test]
    fn a_program_started_past_the_debugger_runs_on_its_own() {
        let child = bypassing_debugger(&cmd(), "/d /c exit 3").unwrap();
        assert_eq!(
            child.wait(Duration::from_secs(10)),
            Some(3),
            "not left frozen as a debuggee"
        );
    }

    #[test]
    fn waiting_for_a_running_process_times_out() {
        assert_eq!(
            wait_for_exit(std::process::id(), Duration::ZERO),
            Some(false)
        );
    }
}

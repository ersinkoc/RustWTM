//! Read-only Restart Manager queries, plus the working directories of
//! processes for folders. Never shutdown, restart or close handles.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows::Win32::System::RestartManager::*;
use windows::core::{PCWSTR, PWSTR};
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Locker {
    pub pid: u32,
    pub create_time: i64,
    pub name: String,
    pub service: String,
    /// How the process holds the path.
    #[serde(default)]
    pub reason: Hold,
}

/// Why a process keeps a file or folder busy.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Hold {
    /// The process works in the folder or one inside it (a console sitting
    /// there, for example): the folder cannot be removed or renamed.
    WorkingDirectory,
    /// Windows reports a file in use (Restart Manager); also any reason a
    /// later version adds.
    #[default]
    #[serde(other)]
    OpenFile,
}

/// Whether `candidate` is `folder` or lies inside it: compared without case
/// and by whole names, so `C:\Data` does not contain `C:\Database`.
pub fn inside(folder: &str, candidate: &str) -> bool {
    const SEP: char = std::path::MAIN_SEPARATOR;
    let trim = |p: &str| {
        p.replace('/', std::path::MAIN_SEPARATOR_STR)
            .trim_end_matches(SEP)
            .to_lowercase()
    };
    let (folder, candidate) = (trim(folder), trim(candidate));
    !folder.is_empty()
        && (candidate == folder
            || candidate
                .strip_prefix(&folder)
                .is_some_and(|rest| rest.starts_with(SEP)))
}

/// Processes whose working directory is `folder` or inside it, within
/// `budget`. Processes this program may not read are skipped and counted.
fn working_in(folder: &Path, budget: Duration) -> (Vec<Locker>, usize) {
    let started = Instant::now();
    let folder = folder.to_string_lossy();
    let mut found = Vec::new();
    let mut unreadable = 0;
    let processes = crate::process::ProcessSnapshotter::new()
        .snapshot()
        .unwrap_or_default();
    for p in processes
        .into_iter()
        .filter(|p| p.pid > 4 && p.pid != std::process::id())
    {
        if started.elapsed() > budget {
            unreadable += 1;
            continue;
        }
        let directory =
            crate::control::ProcessHandle::open(p.pid, crate::control::Access::READ_PARAMETERS)
                .and_then(|h| h.current_directory());
        match directory {
            Ok(dir) if inside(&folder, &dir) => found.push(Locker {
                pid: p.pid,
                create_time: p.create_time,
                name: p.name,
                service: String::new(),
                reason: Hold::WorkingDirectory,
            }),
            Ok(_) => {}
            Err(_) => unreadable += 1,
        }
    }
    (found, unreadable)
}
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Report {
    pub path: String,
    pub files_checked: usize,
    pub partial: bool,
    pub warnings: Vec<String>,
    pub lockers: Vec<Locker>,
}
struct Session(u32);
impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: this guard exclusively owns the successfully opened session.
        let _ = unsafe { RmEndSession(self.0) };
    }
}
fn lockers(paths: &[PathBuf]) -> Result<Vec<Locker>, String> {
    let mut session = 0;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    // SAFETY: handle/key point to initialized writable storage of the documented size.
    let status = unsafe { RmStartSession(&mut session, None, PWSTR(key.as_mut_ptr())) };
    if status != ERROR_SUCCESS {
        return Err(format!("Restart Manager: {}", status.0));
    }
    let session = Session(session);
    let wide: Vec<Vec<u16>> = paths
        .iter()
        .map(|p| {
            p.as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain(Some(0))
                .collect()
        })
        .collect();
    let names: Vec<PCWSTR> = wide.iter().map(|p| PCWSTR(p.as_ptr())).collect();
    // SAFETY: the NUL-terminated names remain alive throughout registration.
    let status = unsafe { RmRegisterResources(session.0, Some(&names), None, None) };
    if status != ERROR_SUCCESS {
        return Err(format!("Register files: {}", status.0));
    }
    // Fixed capacity, never allocate from a Windows-reported process count.
    let mut rows = vec![RM_PROCESS_INFO::default(); 512];
    let mut needed = 0;
    let mut count = rows.len() as u32;
    let mut reasons = 0;
    // SAFETY: rows contains exactly `count` initialized records; all output pointers are valid.
    let status = unsafe {
        RmGetList(
            session.0,
            &mut needed,
            &mut count,
            Some(rows.as_mut_ptr()),
            &mut reasons,
        )
    };
    if status == ERROR_MORE_DATA {
        return Err("Too many file owners; result is incomplete".into());
    }
    if status != ERROR_SUCCESS {
        return Err(format!("Read file owners: {}", status.0));
    }
    if count as usize > rows.len() {
        return Err("Invalid file owner response".into());
    }
    let text = |s: &[u16]| {
        String::from_utf16_lossy(&s[..s.iter().position(|c| *c == 0).unwrap_or(s.len())])
    };
    Ok(rows[..count as usize]
        .iter()
        .map(|r| Locker {
            pid: r.Process.dwProcessId,
            create_time: ((r.Process.ProcessStartTime.dwHighDateTime as u64) << 32
                | r.Process.ProcessStartTime.dwLowDateTime as u64) as i64,
            name: text(&r.strAppName),
            service: text(&r.strServiceShortName),
            reason: Hold::OpenFile,
        })
        .collect())
}

pub fn scan(path: &Path) -> Result<Report, String> {
    use std::os::windows::fs::MetadataExt;
    if path.as_os_str().len() > 32760 || path.to_string_lossy().contains('\0') {
        return Err("Invalid file path".into());
    }
    let input = path.to_string_lossy();
    if input.starts_with(r"\\") || !path.is_absolute() {
        return Err("Choose an absolute local file or folder".into());
    }
    let root = input
        .get(..3)
        .filter(|r| r.ends_with(":\\"))
        .ok_or("Choose a local drive path")?;
    let root: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
    // SAFETY: a valid NUL-terminated drive root; the API retains no pointer.
    if unsafe { windows::Win32::Storage::FileSystem::GetDriveTypeW(PCWSTR(root.as_ptr())) } == 4 {
        return Err("Network drives are not scanned".into());
    }
    let started = Instant::now();
    let mut report = Report {
        path: input.into_owned(),
        ..Default::default()
    };
    let mut files = Vec::new();
    let mut pending = vec![path.to_owned()];
    let mut entries = 0;
    while let Some(p) = pending.pop() {
        if started.elapsed() > Duration::from_secs(5) || files.len() >= 128 || entries >= 4096 {
            report.partial = true;
            break;
        }
        let meta = match p.symlink_metadata() {
            Ok(m) => m,
            Err(e) => {
                report.partial = true;
                if report.warnings.len() < 8 {
                    report.warnings.push(e.to_string())
                }
                continue;
            }
        };
        if meta.file_attributes() & 0x400 != 0 {
            report.partial = true;
            continue;
        } // Never follow reparse points.
        if meta.is_file() {
            files.push(p)
        } else if meta.is_dir() {
            let dir = match p.read_dir() {
                Ok(d) => d,
                Err(e) => {
                    report.partial = true;
                    if report.warnings.len() < 8 {
                        report.warnings.push(e.to_string())
                    }
                    continue;
                }
            };
            for entry in dir.take(4097 - entries) {
                entries += 1;
                if entries > 4096 || pending.len() >= 256 {
                    report.partial = true;
                    break;
                }
                if let Ok(entry) = entry {
                    pending.push(entry.path())
                } else {
                    report.partial = true
                }
            }
        }
    }
    report.files_checked = files.len();
    if !files.is_empty() {
        report.lockers = lockers(&files)?;
    }
    if path.is_dir() {
        let (working, unreadable) = working_in(path, Duration::from_secs(3));
        if unreadable > 0 {
            report.warnings.push(format!(
                "{unreadable} processes could not be checked for working in this folder (32-bit or no access)"
            ));
        }
        for locker in working {
            if !report
                .lockers
                .iter()
                .any(|l| l.pid == locker.pid && l.create_time == locker.create_time)
            {
                report.lockers.push(locker);
            }
        }
    }
    if report.partial {
        report
            .warnings
            .push("Folder query was limited; not every item was checked".into());
    }
    Ok(report)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folders_contain_whole_names_only() {
        assert!(inside(r"C:\Data", r"C:\Data"));
        assert!(inside(r"C:\Data\", r"c:\data\logs"));
        assert!(inside(r"C:\Data", r"C:/Data/logs/"));
        assert!(!inside(r"C:\Data", r"C:\Database"));
        assert!(!inside(r"C:\Data\logs", r"C:\Data"));
        assert!(!inside("", r"C:\Data"));
    }

    #[test]
    fn a_child_working_in_a_folder_is_reported_as_its_owner() {
        // One short-lived program waits inside a fresh temporary folder; it
        // is the child itself, so ending the child frees the folder.
        let dir = std::env::temp_dir().join(format!("wtm-cwd-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        let mut child =
            std::process::Command::new(crate::shell::system_directory().unwrap().join("PING.EXE"))
                .args(["-n", "30", "127.0.0.1"])
                .current_dir(dir.join("inner"))
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let report = scan(&dir);
        let _ = child.kill();
        let _ = child.wait();
        std::fs::remove_dir_all(&dir).expect("the ended child no longer holds the folder");
        let report = report.unwrap();
        assert!(
            report
                .lockers
                .iter()
                .any(|l| l.pid == child.id() && l.reason == Hold::WorkingDirectory),
            "{report:?}"
        );
    }

    #[test]
    fn invalid_and_remote_paths_are_rejected_before_native_queries() {
        for path in ["relative.txt", r"\\server\share\file", "C:\\bad\0path"] {
            assert!(scan(Path::new(path)).is_err());
        }
    }
}

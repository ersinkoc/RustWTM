//! Read-only Restart Manager queries. Never shutdown, restart or close handles.
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
    fn invalid_and_remote_paths_are_rejected_before_native_queries() {
        for path in ["relative.txt", r"\\server\share\file", "C:\\bad\0path"] {
            assert!(scan(Path::new(path)).is_err());
        }
    }
}

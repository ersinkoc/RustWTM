//! Process control with safety checks.
//!
//! Every mutating action re-opens the process, verifies its creation time
//! still matches (guarding against PID reuse) and refuses protected or
//! critical processes.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use serde::{Deserialize, Serialize};

use wtm_sys::control::{Access, ProcessHandle};
pub use wtm_sys::control::{EcoQos, PriorityClass};
use wtm_sys::job::Job;

use crate::ProcessInfo;
use crate::protection::protection_reason;
use crate::tree::descendants;

/// Identifies a specific process instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessRef {
    pub pid: u32,
    /// FILETIME from the snapshot; 0 skips the identity check.
    pub create_time: i64,
    pub name: String,
}

impl From<&ProcessInfo> for ProcessRef {
    fn from(p: &ProcessInfo) -> Self {
        Self {
            pid: p.pid,
            create_time: p.create_time,
            name: p.name.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("process {pid}: {reason}")]
    Affinity { pid: u32, reason: String },
    #[error("process {pid} is protected ({reason})")]
    Protected { pid: u32, reason: &'static str },
    #[error("process {pid} no longer exists")]
    NotFound { pid: u32 },
    #[error("process {pid} exited and its PID was reused")]
    Stale { pid: u32 },
    #[error("access denied to process {pid} (try an elevated prompt)")]
    AccessDenied { pid: u32 },
    #[error("process {pid}: {source}")]
    Sys { pid: u32, source: wtm_sys::Error },
}

impl ControlError {
    /// For failures of OpenProcess, which reports a missing PID as an
    /// invalid parameter.
    fn from_open(pid: u32, source: wtm_sys::Error) -> Self {
        if source.is_invalid_parameter() {
            Self::NotFound { pid }
        } else {
            Self::from_sys(pid, source)
        }
    }

    fn from_sys(pid: u32, source: wtm_sys::Error) -> Self {
        if source.is_access_denied() {
            Self::AccessDenied { pid }
        } else {
            Self::Sys { pid, source }
        }
    }
}

pub type ControlResult<T> = Result<T, ControlError>;

/// Enables `SeDebugPrivilege`; returns `false` when not elevated.
pub fn enable_debug_privilege() -> bool {
    wtm_sys::privilege::enable_debug_privilege().unwrap_or(false)
}

/// Sets the priority of the calling process (e.g. to keep the governor
/// responsive under load). Bypasses the self-protection rule by design.
pub fn set_own_priority(priority: PriorityClass) -> ControlResult<()> {
    let pid = std::process::id();
    let h = ProcessHandle::open(pid, Access::SET_INFORMATION)
        .map_err(|e| ControlError::from_open(pid, e))?;
    sys(pid, h.set_priority(priority))
}

fn open_verified(target: &ProcessRef, access: Access) -> ControlResult<ProcessHandle> {
    let pid = target.pid;
    let handle = ProcessHandle::open(pid, access | Access::QUERY)
        .map_err(|e| ControlError::from_open(pid, e))?;
    if target.create_time != 0 {
        let created = handle
            .create_time()
            .map_err(|e| ControlError::from_sys(pid, e))?;
        if created != target.create_time {
            return Err(ControlError::Stale { pid });
        }
    }
    Ok(handle)
}

/// Opens a process for a mutating action, enforcing the protection rules.
fn open_controlled(target: &ProcessRef, access: Access) -> ControlResult<ProcessHandle> {
    let pid = target.pid;
    if let Some(reason) = protection_reason(pid, &target.name) {
        return Err(ControlError::Protected { pid, reason });
    }
    let handle = open_verified(target, access)?;
    if handle.is_critical().unwrap_or(true) {
        return Err(ControlError::Protected {
            pid,
            reason: "critical system process",
        });
    }
    Ok(handle)
}

fn sys<T>(pid: u32, r: wtm_sys::Result<T>) -> ControlResult<T> {
    r.map_err(|e| ControlError::from_sys(pid, e))
}

pub fn kill(target: &ProcessRef) -> ControlResult<()> {
    let h = open_controlled(target, Access::TERMINATE)?;
    sys(target.pid, h.terminate(1))
}

/// Moves the process's working set to the standby list (frees RAM now;
/// pages come back on demand).
pub fn trim_memory(target: &ProcessRef) -> ControlResult<()> {
    let h = open_controlled(target, Access::SET_QUOTA)?;
    sys(target.pid, h.trim_working_set())
}

/// Kills `root` first (so it cannot respawn children), then its descendants.
pub fn kill_tree(root: &ProcessRef, procs: &[ProcessInfo]) -> Vec<(ProcessRef, ControlResult<()>)> {
    let mut targets = vec![root.clone()];
    targets.extend(
        descendants(root.pid, root.create_time, procs)
            .into_iter()
            .map(ProcessRef::from),
    );
    targets
        .into_iter()
        .map(|t| {
            let r = kill(&t);
            (t, r)
        })
        .collect()
}

pub fn suspend(target: &ProcessRef) -> ControlResult<()> {
    let h = open_controlled(target, Access::SUSPEND_RESUME)?;
    sys(target.pid, h.suspend())
}

/// Resuming is always allowed, so a mistakenly frozen process can be recovered.
pub fn resume(target: &ProcessRef) -> ControlResult<()> {
    let h = open_verified(target, Access::SUSPEND_RESUME)?;
    sys(target.pid, h.resume())
}

pub fn set_priority(target: &ProcessRef, priority: PriorityClass) -> ControlResult<()> {
    let h = open_controlled(target, Access::SET_INFORMATION)?;
    sys(target.pid, h.set_priority(priority))
}

pub fn set_affinity(target: &ProcessRef, mask: u64) -> ControlResult<()> {
    let h = open_controlled(target, Access::SET_INFORMATION)?;
    sys(target.pid, h.set_affinity(mask))
}

pub fn set_core_affinity(
    target: &ProcessRef,
    selection: crate::cpu::CoreSelection,
) -> ControlResult<()> {
    let h = open_controlled(target, Access::SET_INFORMATION)?;
    let mask = h
        .core_affinity(selection)
        .map_err(|reason| ControlError::Affinity {
            pid: target.pid,
            reason,
        })?;
    sys(target.pid, h.set_affinity(mask))
}

pub fn set_eco_qos(target: &ProcessRef, mode: EcoQos) -> ControlResult<()> {
    let h = open_controlled(target, Access::SET_INFORMATION)?;
    sys(target.pid, h.set_eco_qos(mode))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessDetails {
    pub path: Option<String>,
    pub command_line: Option<String>,
    /// Account the process runs as, `DOMAIN\user`.
    pub user: Option<String>,
    pub priority: Option<PriorityClass>,
    /// `(process_mask, system_mask)`.
    pub affinity: Option<(u64, u64)>,
    pub eco_qos: Option<EcoQos>,
    pub critical: Option<bool>,
    pub protected: Option<String>,
}

/// `dir\name.DMP`, or `name (2).DMP` … when that exists already.
fn dump_path(dir: &std::path::Path, image_name: &str) -> std::path::PathBuf {
    let stem = std::path::Path::new(image_name)
        .file_stem()
        .map_or_else(|| "process".into(), |s| s.to_string_lossy().into_owned());
    (1..)
        .map(|n| {
            dir.join(if n == 1 {
                format!("{stem}.DMP")
            } else {
                format!("{stem} ({n}).DMP")
            })
        })
        .find(|p| !p.exists())
        .unwrap_or_else(|| dir.join(format!("{stem}.DMP")))
}

/// Writes a full memory dump of `target` into `dir` (Task Manager's
/// "Create memory dump file"); returns its path.
pub fn create_dump(
    target: &ProcessRef,
    dir: &std::path::Path,
) -> ControlResult<std::path::PathBuf> {
    let h = open_verified(target, Access::DUMP)?;
    let path = dump_path(dir, &target.name);
    sys(target.pid, h.write_dump(&path))?;
    Ok(path)
}

/// Full image path only; cheaper than [`inspect`].
pub fn image_path(target: &ProcessRef) -> ControlResult<String> {
    let h = open_verified(target, Access::QUERY)?;
    sys(target.pid, h.image_path())
}

/// The command line a process was started with.
pub fn command_line(target: &ProcessRef) -> ControlResult<String> {
    let h = open_verified(target, Access::QUERY)?;
    sys(target.pid, h.command_line())
}

/// Read-only details; individual fields are `None` when access is denied.
pub fn inspect(target: &ProcessRef) -> ControlResult<ProcessDetails> {
    let h = open_verified(target, Access::QUERY)?;
    Ok(ProcessDetails {
        path: h.image_path().ok(),
        command_line: h.command_line().ok(),
        user: h.user().ok(),
        priority: h.priority().ok(),
        affinity: h.affinity().ok(),
        eco_qos: h.eco_qos().ok(),
        critical: h.is_critical().ok(),
        protected: protection_reason(target.pid, &target.name).map(Into::into),
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// Hard cap as a share of total machine CPU, 0 < x <= 100.
    pub cpu_percent: Option<f32>,
    /// Committed memory limit per process.
    pub memory_bytes: Option<u64>,
}

/// Applies job-object limits and keeps the job handles so limits can be
/// changed or lifted later. Dropping the limiter leaves limits in place.
#[derive(Debug, Default)]
pub struct Limiter {
    /// Job handle and the limits currently set on it.
    jobs: HashMap<(u32, i64), (ProcessRef, Job, Limits)>,
}

impl Limiter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn limited(&self) -> impl Iterator<Item = &ProcessRef> {
        self.jobs.values().map(|(r, _, _)| r)
    }

    /// Sets exactly `limits` on the target, replacing earlier ones.
    pub fn apply(&mut self, target: &ProcessRef, limits: Limits) -> ControlResult<()> {
        let pid = target.pid;
        let (_, job, current) = match self.jobs.entry((pid, target.create_time)) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => {
                let h = open_controlled(target, Access::SET_QUOTA | Access::TERMINATE)?;
                let job = sys(pid, Job::create())?;
                sys(pid, job.assign(&h))?;
                e.insert((target.clone(), job, Limits::default()))
            }
        };
        // Only touch what changed: disabling a CPU cap that was never enabled
        // is rejected by Windows.
        if limits.cpu_percent != current.cpu_percent {
            sys(pid, job.set_cpu_cap(limits.cpu_percent))?;
            current.cpu_percent = limits.cpu_percent;
        }
        if limits.memory_bytes != current.memory_bytes {
            sys(pid, job.set_process_memory_limit(limits.memory_bytes))?;
            current.memory_bytes = limits.memory_bytes;
        }
        Ok(())
    }

    pub fn lift(&mut self, target: &ProcessRef) -> ControlResult<()> {
        let Some((_, job, current)) = self.jobs.remove(&(target.pid, target.create_time)) else {
            return Ok(());
        };
        if current.cpu_percent.is_some() {
            sys(target.pid, job.set_cpu_cap(None))?;
        }
        if current.memory_bytes.is_some() {
            sys(target.pid, job.set_process_memory_limit(None))?;
        }
        Ok(())
    }

    /// Drops the job handle without lifting limits (e.g. the process exited).
    pub fn forget(&mut self, target: &ProcessRef) {
        self.jobs.remove(&(target.pid, target.create_time));
    }

    pub fn lift_all(&mut self) -> Vec<(ProcessRef, ControlResult<()>)> {
        let targets: Vec<ProcessRef> = self.limited().cloned().collect();
        targets
            .into_iter()
            .map(|t| {
                let r = self.lift(&t);
                (t, r)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS_ACCESS_DENIED: u32 = 0xC000_0022;
    const STATUS_INVALID_PARAMETER: u32 = 0xC000_000D;
    const STATUS_UNSUCCESSFUL: u32 = 0xC000_0001;

    fn nt(status: u32) -> wtm_sys::Error {
        wtm_sys::Error::Nt { call: "x", status }
    }

    #[test]
    fn classifies_open_and_call_failures() {
        assert!(matches!(
            ControlError::from_open(7, nt(STATUS_INVALID_PARAMETER)),
            ControlError::NotFound { pid: 7 }
        ));
        assert!(matches!(
            ControlError::from_open(7, nt(STATUS_ACCESS_DENIED)),
            ControlError::AccessDenied { pid: 7 }
        ));
        // Outside OpenProcess an invalid parameter is a plain failure.
        let e = ControlError::from_sys(7, nt(STATUS_INVALID_PARAMETER));
        assert!(matches!(e, ControlError::Sys { pid: 7, .. }));
        let e = ControlError::from_open(7, nt(STATUS_UNSUCCESSFUL));
        assert_eq!(
            e.to_string(),
            "process 7: x failed with NTSTATUS 0xc0000001"
        );
        assert_eq!(
            ControlError::AccessDenied { pid: 7 }.to_string(),
            "access denied to process 7 (try an elevated prompt)"
        );
    }

    #[test]
    fn dumps_get_free_names() {
        let dir = std::env::temp_dir().join(format!("wtm-dump-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(dump_path(&dir, "app.exe"), dir.join("app.DMP"));
        std::fs::write(dir.join("app.DMP"), b"").unwrap();
        assert_eq!(dump_path(&dir, "app.exe"), dir.join("app (2).DMP"));
        let _ = std::fs::remove_dir_all(&dir);

        let gone = ProcessRef {
            pid: 0xFFFF_FFF0,
            create_time: 1,
            name: "gone.exe".into(),
        };
        assert!(create_dump(&gone, &std::env::temp_dir()).is_err());
    }

    #[test]
    fn reads_the_command_line() {
        let me = ProcessRef {
            pid: std::process::id(),
            create_time: 0,
            name: "me.exe".into(),
        };
        let exe = std::env::current_exe().unwrap();
        let stem = exe.file_stem().unwrap().to_string_lossy().to_lowercase();
        assert!(command_line(&me).unwrap().to_lowercase().contains(&stem));
        let gone = ProcessRef {
            pid: 0xFFFF_FFF0,
            ..me
        };
        assert!(matches!(
            command_line(&gone),
            Err(ControlError::NotFound { .. })
        ));
    }
}

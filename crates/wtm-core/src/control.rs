//! Process control with safety checks.
//!
//! Every mutating action re-opens the process, verifies its creation time
//! still matches (guarding against PID reuse) and refuses protected or
//! critical processes.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use wtm_sys::control::{Access, ProcessHandle};
pub use wtm_sys::control::{EcoQos, PriorityClass, enter_background_mode, raise_current_thread};
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
    #[error("process {pid} is not limited by RustWTM in this session")]
    NotLimited { pid: u32 },
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

/// Keeps the calling process responsive on an overloaded machine: raises its
/// priority class, opts it out of EcoQoS, restores normal memory priority and
/// keeps `working_set_bytes` of it resident. Every step is attempted; the
/// returned messages describe the ones Windows refused.
pub fn harden_self(priority: PriorityClass, working_set_bytes: usize) -> Vec<String> {
    let pid = std::process::id();
    let access = Access::SET_INFORMATION | Access::SET_QUOTA | Access::QUERY;
    let h = match ProcessHandle::open(pid, access) {
        Ok(h) => h,
        Err(e) => return vec![format!("cannot open own process: {e}")],
    };
    let mut failures = Vec::new();
    let mut step = |what: &str, r: wtm_sys::Result<()>| {
        if let Err(e) = r {
            failures.push(format!("{what}: {e}"));
        }
    };
    step("priority", h.set_priority(priority));
    step("EcoQoS opt-out", h.set_eco_qos(EcoQos::Off));
    step("memory priority", h.set_normal_memory_priority());
    if working_set_bytes > 0 {
        step("working set floor", h.lock_working_set(working_set_bytes));
    }
    failures
}

/// Confines `target` to the CPU Sets `ids`. Returns `false`, changing
/// nothing, when the process already chose CPU Sets of its own.
pub fn confine_cpu_sets(target: &ProcessRef, ids: &[u32]) -> ControlResult<bool> {
    let h = open_controlled(target, Access::SET_LIMITED_INFORMATION)?;
    if !sys(target.pid, h.default_cpu_sets())?.is_empty() {
        return Ok(false);
    }
    sys(target.pid, h.set_default_cpu_sets(ids))?;
    Ok(true)
}

/// Undoes [`confine_cpu_sets`], unless the process has since chosen other
/// CPU Sets itself.
pub fn release_cpu_sets(target: &ProcessRef, ids: &[u32]) -> ControlResult<()> {
    let h = open_verified(target, Access::SET_LIMITED_INFORMATION)?;
    let mut current = sys(target.pid, h.default_cpu_sets())?;
    current.sort_unstable();
    let mut ours = ids.to_vec();
    ours.sort_unstable();
    if current == ours {
        sys(target.pid, h.set_default_cpu_sets(&[]))?;
    }
    Ok(())
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

/// The file name of the image `h` runs. A `ProcessRef`'s name is only what
/// the caller claims; protection rules judge the process actually opened.
fn image_name(h: &ProcessHandle) -> Option<String> {
    let path = h.image_path().ok()?;
    Some(file_name(&path).to_owned())
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Opens a process for a mutating action, enforcing the protection rules
/// on both the name given and the image actually opened.
fn open_controlled(target: &ProcessRef, access: Access) -> ControlResult<ProcessHandle> {
    let pid = target.pid;
    if let Some(reason) = protection_reason(pid, &target.name) {
        return Err(ControlError::Protected { pid, reason });
    }
    let handle = open_verified(target, access)?;
    if let Some(reason) = image_name(&handle).and_then(|name| protection_reason(pid, &name)) {
        return Err(ControlError::Protected { pid, reason });
    }
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
    // When the PID now belongs to a newer process, that process's subtree is
    // not the root's, although it is younger than the root: leave it alone.
    let foreign: HashSet<(u32, i64)> = procs
        .iter()
        .find(|p| root.create_time != 0 && p.pid == root.pid && p.create_time > root.create_time)
        .map(|owner| descendants(owner.pid, owner.create_time, procs))
        .unwrap_or_default()
        .into_iter()
        .map(|p| (p.pid, p.create_time))
        .collect();
    let mut targets = vec![root.clone()];
    targets.extend(
        descendants(root.pid, root.create_time, procs)
            .into_iter()
            .filter(|p| !foreign.contains(&(p.pid, p.create_time)))
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

/// Resumes that undo stacked suspensions before [`resume`] gives up.
const MAX_RESUMES: usize = 64;

/// Resuming is always allowed, so a mistakenly frozen process can be recovered.
///
/// Suspensions stack: a process suspended twice needs two resumes. This
/// keeps resuming (a bounded number of times) until some thread of the
/// process is no longer suspended.
pub fn resume(target: &ProcessRef) -> ControlResult<()> {
    let pid = target.pid;
    let h = open_verified(target, Access::SUSPEND_RESUME)?;
    sys(pid, h.resume())?;
    let mut snapshotter = wtm_sys::process::ProcessSnapshotter::new();
    for _ in 1..MAX_RESUMES {
        let threads = sys(pid, snapshotter.threads(pid))?;
        if !all_suspended(&threads) {
            break;
        }
        sys(pid, h.resume())?;
    }
    Ok(())
}

/// A process is frozen while every one of its threads is suspended; one
/// without threads (exited) is not.
fn all_suspended(threads: &[wtm_sys::process::ThreadInfo]) -> bool {
    !threads.is_empty() && threads.iter().all(wtm_sys::process::ThreadInfo::suspended)
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
    let secrets = ControlError::Protected {
        pid: target.pid,
        reason: "holds this PC's sign-in secrets",
    };
    if holds_secrets(&target.name) {
        return Err(secrets);
    }
    let h = open_verified(target, Access::DUMP_CLONE)
        .or_else(|_| open_verified(target, Access::DUMP))?;
    // The name given is only a claim; the image opened decides.
    if image_name(&h).is_some_and(|name| holds_secrets(&name)) {
        return Err(secrets);
    }
    let path = dump_path(dir, &target.name);
    sys(target.pid, h.write_dump(&path))?;
    Ok(path)
}

/// Full image path only; cheaper than [`inspect`].
/// Their memory holds the sign-in secrets of this PC; dumping them is what
/// credential theft does, and antivirus software rightly stops it.
fn holds_secrets(name: &str) -> bool {
    ["lsass.exe", "lsaiso.exe"]
        .iter()
        .any(|n| n.eq_ignore_ascii_case(name))
}

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

    /// Whether this limiter holds limits on `target`.
    pub fn is_limited(&self, target: &ProcessRef) -> bool {
        self.jobs.contains_key(&(target.pid, target.create_time))
    }

    /// Lifts the limits [`apply`](Self::apply) set. A process this limiter
    /// never limited is an error: its job (if any) is not ours to change,
    /// and reporting success would leave the user believing it is free.
    ///
    /// Both limits are lifted even when the first fails; whatever could not
    /// be lifted stays recorded with its job, so it can be lifted later.
    pub fn lift(&mut self, target: &ProcessRef) -> ControlResult<()> {
        let key = (target.pid, target.create_time);
        let Some((who, job, mut current)) = self.jobs.remove(&key) else {
            return Err(ControlError::NotLimited { pid: target.pid });
        };
        let mut failed = None;
        if current.cpu_percent.is_some() {
            match job.set_cpu_cap(None) {
                Ok(()) => current.cpu_percent = None,
                Err(e) => failed = Some(e),
            }
        }
        if current.memory_bytes.is_some() {
            match job.set_process_memory_limit(None) {
                Ok(()) => current.memory_bytes = None,
                Err(e) => failed = failed.or(Some(e)),
            }
        }
        match failed {
            None => Ok(()),
            Some(e) => {
                self.jobs.insert(key, (who, job, current));
                Err(ControlError::from_sys(target.pid, e))
            }
        }
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

    #[test]
    fn secrets_and_image_names_are_judged_by_file_name() {
        assert_eq!(file_name(r"C:\Windows\System32\lsass.exe"), "lsass.exe");
        assert_eq!(file_name("lsass.exe"), "lsass.exe");
        assert_eq!(
            file_name(r"\Device\HarddiskVolume3/x/LsaIso.exe"),
            "LsaIso.exe"
        );
        assert!(holds_secrets("LSASS.EXE") && holds_secrets("lsaiso.exe"));
        assert!(!holds_secrets("notepad.exe") && !holds_secrets("lsass.exe.bak"));
    }

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
    fn a_failed_lift_still_lifts_the_rest_and_keeps_the_job() {
        let t = ProcessRef {
            pid: u32::MAX - 3,
            create_time: 1,
            name: "gone.exe".into(),
        };
        let job = Job::create().unwrap();
        job.set_process_memory_limit(Some(1 << 30)).unwrap();
        // Recorded with a CPU cap the job never had: Windows refuses to lift
        // it, which stands in for any failure on the first limit.
        let limits = Limits {
            cpu_percent: Some(50.0),
            memory_bytes: Some(1 << 30),
        };
        let mut limiter = Limiter::new();
        limiter
            .jobs
            .insert((t.pid, t.create_time), (t.clone(), job, limits));
        assert!(limiter.lift(&t).is_err());
        let (_, _, left) = &limiter.jobs[&(t.pid, t.create_time)];
        assert_eq!(
            left.memory_bytes, None,
            "the memory limit was lifted anyway"
        );
        assert_eq!(left.cpu_percent, Some(50.0), "kept, to be lifted later");
        assert!(limiter.is_limited(&t));
    }

    #[test]
    fn lifting_what_was_never_limited_is_an_error() {
        let mut limiter = Limiter::new();
        let t = ProcessRef {
            pid: std::process::id(),
            create_time: 1,
            name: "me.exe".into(),
        };
        assert!(!limiter.is_limited(&t));
        let e = limiter.lift(&t).unwrap_err();
        assert!(matches!(e, ControlError::NotLimited { .. }));
        assert!(e.to_string().contains("not limited by RustWTM"), "{e}");
        assert!(limiter.lift_all().is_empty());
    }

    #[test]
    fn frozen_means_every_thread_suspended() {
        use wtm_sys::process::ThreadInfo;
        let thread = |state, wait_reason| ThreadInfo {
            tid: 1,
            state,
            wait_reason,
            context_switches: 0,
        };
        assert!(!all_suspended(&[]));
        assert!(all_suspended(&[thread(5, 5), thread(5, 12)]));
        // One thread waiting on something else, or running: not frozen.
        assert!(!all_suspended(&[thread(5, 5), thread(5, 6)]));
        assert!(!all_suspended(&[thread(5, 5), thread(2, 0)]));
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
    fn sign_in_secrets_are_never_dumped() {
        for name in ["lsass.exe", "LsaIso.exe"] {
            let t = ProcessRef {
                pid: std::process::id(),
                create_time: 0,
                name: name.into(),
            };
            assert!(matches!(
                create_dump(&t, &std::env::temp_dir()),
                Err(ControlError::Protected { .. })
            ));
        }
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

    #[test]
    fn confines_and_releases_cpu_sets_without_clobbering_foreign_ones() {
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let handle = ProcessHandle::open(child.id(), Access::QUERY).unwrap();
        let target = ProcessRef {
            pid: child.id(),
            create_time: handle.create_time().unwrap(),
            name: "PING.EXE".into(),
        };
        let ids: Vec<u32> = crate::cpu::topology()
            .unwrap()
            .cpus
            .iter()
            .map(|c| c.id)
            .collect();
        let ours = &ids[..1];
        assert!(confine_cpu_sets(&target, ours).unwrap());
        assert_eq!(handle.default_cpu_sets().unwrap(), ours);
        // Already confined (by us or the app itself): left alone.
        assert!(!confine_cpu_sets(&target, &ids).unwrap());
        release_cpu_sets(&target, ours).unwrap();
        assert!(handle.default_cpu_sets().unwrap().is_empty());

        // A restriction we did not make survives our release.
        let h = ProcessHandle::open(target.pid, Access::SET_LIMITED_INFORMATION).unwrap();
        h.set_default_cpu_sets(&ids).unwrap();
        release_cpu_sets(&target, ours).unwrap();
        assert_eq!(handle.default_cpu_sets().unwrap().len(), ids.len());

        let stale = ProcessRef {
            create_time: target.create_time + 1,
            ..target.clone()
        };
        assert!(matches!(
            confine_cpu_sets(&stale, ours),
            Err(ControlError::Stale { .. })
        ));
        let me = ProcessRef {
            pid: std::process::id(),
            create_time: 0,
            name: "me.exe".into(),
        };
        assert!(matches!(
            confine_cpu_sets(&me, ours),
            Err(ControlError::Protected { .. })
        ));
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn hardening_self_succeeds_for_an_ordinary_process() {
        assert_eq!(
            harden_self(PriorityClass::Normal, 16 << 20),
            Vec::<String>::new()
        );
    }
}

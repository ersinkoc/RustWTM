//! Explicit, persistent system power limits with a write-ahead rollback journal.
//! No automatic startup application and no claim of a per-process MHz cap.
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
pub use wtm_sys::power::PowerSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PowerSupply {
    Ac,
    Dc,
    Both,
}
impl PowerSupply {
    fn sources(self) -> &'static [PowerSource] {
        match self {
            Self::Ac => &[PowerSource::Ac],
            Self::Dc => &[PowerSource::Dc],
            Self::Both => &[PowerSource::Ac, PowerSource::Dc],
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Ac => "Plugged in",
            Self::Dc => "Battery",
            Self::Both => "Plugged in and battery",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrequencyRequest {
    pub plan: String,
    pub class: u8,
    pub source: PowerSupply,
    pub mhz: u32,
}
impl FrequencyRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.plan.is_empty() {
            return Err("Choose a current power plan".into());
        }
        if self.class > 1 {
            return Err("Windows frequency policy class must be 0 or 1".into());
        }
        if !(100..=64000).contains(&self.mhz) {
            return Err("Frequency must be between 100 and 64000 MHz; use Restore for the saved original value".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrequencyValue {
    pub class: u8,
    pub source: PowerSource,
    pub mhz: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrequencyReport {
    pub plan: String,
    pub values: Vec<FrequencyValue>,
    pub backup_plan: Option<String>,
    pub backup_error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Change {
    class: u8,
    source: PowerSource,
    before: u32,
    after: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Journal {
    schema: u32,
    plan: String,
    changes: Vec<Change>,
}

trait PowerApi {
    fn admin(&self) -> bool;
    fn active(&self) -> Result<String, String>;
    fn read(&self, plan: &str, class: u8, source: PowerSource) -> Result<u32, String>;
    fn write(&mut self, plan: &str, class: u8, source: PowerSource, mhz: u32)
    -> Result<(), String>;
    fn refresh(&mut self, plan: &str) -> Result<(), String>;
}
struct WindowsPower;
impl PowerApi for WindowsPower {
    fn admin(&self) -> bool {
        crate::launch::is_elevated()
    }
    fn active(&self) -> Result<String, String> {
        wtm_sys::power::active_scheme().map_err(|e| e.to_string())
    }
    fn read(&self, p: &str, c: u8, s: PowerSource) -> Result<u32, String> {
        wtm_sys::power::read_frequency(p, c, s).map_err(|e| e.to_string())
    }
    fn write(&mut self, p: &str, c: u8, s: PowerSource, m: u32) -> Result<(), String> {
        wtm_sys::power::write_frequency(p, c, s, m).map_err(|e| e.to_string())
    }
    fn refresh(&mut self, p: &str) -> Result<(), String> {
        wtm_sys::power::refresh_scheme(p).map_err(|e| e.to_string())
    }
}
fn directory() -> Result<PathBuf, String> {
    Ok(
        PathBuf::from(std::env::var_os("ProgramData").ok_or("ProgramData is unavailable")?)
            .join("RustWTM/PowerLimits"),
    )
}
fn load(path: &Path) -> Result<Option<Journal>, String> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("Power backup is too large".into());
    }
    let record: Journal =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid power backup: {e}"))?;
    if record.schema != 1
        || record.plan.is_empty()
        || record.changes.is_empty()
        || record.changes.len() > 4
        || record.changes.iter().enumerate().any(|(i, c)| {
            c.class > 1
                || !(100..=64000).contains(&c.after)
                || record.changes[..i]
                    .iter()
                    .any(|p| p.class == c.class && p.source == c.source)
        })
    {
        return Err("Unsupported power backup; no settings were changed".into());
    }
    Ok(Some(record))
}

struct Manager<B> {
    backend: B,
    dir: PathBuf,
    secure: bool,
}
impl<B: PowerApi> Manager<B> {
    fn lock(&self) -> Result<File, String> {
        if !self.backend.admin() {
            return Err("Administrator rights required to change Windows power limits".into());
        }
        fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        if self.secure {
            wtm_sys::security::set_dacl(
                &self.dir,
                "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;GRGX;;;BU)",
            )
            .map_err(|e| e.to_string())?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.dir.join("operation.lock"))
            .map_err(|e| e.to_string())?;
        lock.try_lock()
            .map_err(|e| format!("Another power-limit operation is running: {e}"))?;
        Ok(lock)
    }
    fn refresh_if_active(&mut self, plan: &str) -> Result<(), String> {
        if self.backend.active()? == plan {
            self.backend.refresh(plan)?;
        }
        Ok(())
    }
    fn apply(&mut self, request: &FrequencyRequest) -> Result<(), String> {
        request.validate()?;
        let _lock = self.lock()?;
        let path = self.dir.join("original.json");
        if load(&path)?.is_some() {
            return Err(
                "A saved power limit already exists. Restore it before applying another limit"
                    .into(),
            );
        }
        let plan = self.backend.active()?;
        if plan != request.plan {
            return Err(
                "The active power plan changed. Refresh before applying a frequency limit".into(),
            );
        }
        let changes = request
            .source
            .sources()
            .iter()
            .map(|&source| {
                Ok(Change {
                    class: request.class,
                    source,
                    before: self.backend.read(&plan, request.class, source)?,
                    after: request.mhz,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let journal = Journal {
            schema: 1,
            plan: plan.clone(),
            changes,
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let saved = serde_json::to_writer_pretty(&mut file, &journal)
            .map_err(|e| e.to_string())
            .and_then(|()| {
                file.flush()
                    .and_then(|()| file.sync_all())
                    .map_err(|e| e.to_string())
            });
        drop(file);
        if let Err(e) = saved {
            let _ = fs::remove_file(&path);
            return Err(e);
        }
        let result = (|| {
            if self.backend.active()? != plan {
                return Err("Active power plan changed before applying; retry".into());
            }
            for c in &journal.changes {
                if self.backend.read(&plan, c.class, c.source)? != c.before {
                    return Err("Power settings changed before writing; refresh and retry".into());
                }
                self.backend.write(&plan, c.class, c.source, c.after)?;
            }
            self.refresh_if_active(&plan)
        })();
        if let Err(error) = result {
            match self.rollback(&journal) {
                Ok(()) => {
                    fs::remove_file(&path).map_err(|e| e.to_string())?;
                    return Err(format!("{error}; original values restored"));
                }
                Err(restore) => {
                    return Err(format!(
                        "{error}; restore pending: {restore}. The saved original values were kept"
                    ));
                }
            }
        }
        Ok(())
    }
    fn rollback(&mut self, j: &Journal) -> Result<(), String> {
        // Preflight every value before changing any. Never silently overwrite
        // settings another application changed after this request.
        for c in &j.changes {
            let current = self.backend.read(&j.plan, c.class, c.source)?;
            if current != c.before && current != c.after {
                return Err(
                    "Power settings changed externally; restore refused to overwrite them".into(),
                );
            }
        }
        for c in &j.changes {
            let current = self.backend.read(&j.plan, c.class, c.source)?;
            if current != c.before && current != c.after {
                return Err("Power settings changed externally during restore".into());
            }
            if current != c.before {
                self.backend.write(&j.plan, c.class, c.source, c.before)?;
            }
        }
        self.refresh_if_active(&j.plan)
    }
    fn restore(&mut self) -> Result<(), String> {
        let _lock = self.lock()?;
        let path = self.dir.join("original.json");
        let journal = load(&path)?.ok_or("No saved power limit to restore")?;
        self.rollback(&journal)?;
        fs::remove_file(path).map_err(|e| e.to_string())
    }
}

pub fn frequency_report() -> Result<FrequencyReport, String> {
    let backend = WindowsPower;
    let plan = backend.active()?;
    let mut values = Vec::new();
    for class in [0, 1] {
        for source in [PowerSource::Ac, PowerSource::Dc] {
            values.push(FrequencyValue {
                class,
                source,
                mhz: backend.read(&plan, class, source).ok(),
            });
        }
    }
    let backup = directory().and_then(|dir| load(&dir.join("original.json")));
    let (backup_plan, backup_error) = match backup {
        Ok(j) => (j.map(|j| j.plan), None),
        Err(e) => (None, Some(e)),
    };
    Ok(FrequencyReport {
        plan,
        values,
        backup_plan,
        backup_error,
    })
}
pub fn set_frequency(request: &FrequencyRequest) -> Result<(), String> {
    Manager {
        backend: WindowsPower,
        dir: directory()?,
        secure: true,
    }
    .apply(request)
}
pub fn restore_frequency() -> Result<(), String> {
    Manager {
        backend: WindowsPower,
        dir: directory()?,
        secure: true,
    }
    .restore()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    #[derive(Default)]
    struct Fake {
        values: HashMap<(String, u8, u8), u32>,
        active: String,
        admin: bool,
        writes: usize,
        fail_at: Option<usize>,
        fail_restore: bool,
        journal: Option<PathBuf>,
        refreshes: usize,
    }
    impl PowerApi for Fake {
        fn admin(&self) -> bool {
            self.admin
        }
        fn active(&self) -> Result<String, String> {
            Ok(self.active.clone())
        }
        fn read(&self, p: &str, c: u8, s: PowerSource) -> Result<u32, String> {
            self.values
                .get(&(p.into(), c, s as u8))
                .copied()
                .ok_or("Unsupported setting".into())
        }
        fn write(&mut self, p: &str, c: u8, s: PowerSource, m: u32) -> Result<(), String> {
            if let Some(path) = &self.journal
                && load(path)?.is_none()
            {
                return Err("Original settings were not saved before the device write".into());
            }
            self.writes += 1;
            if self.fail_at == Some(self.writes) || (m == 0 && self.fail_restore) {
                return Err("Injected write failure".into());
            }
            self.values.insert((p.into(), c, s as u8), m);
            Ok(())
        }
        fn refresh(&mut self, _: &str) -> Result<(), String> {
            self.refreshes += 1;
            Ok(())
        }
    }
    fn manager(name: &str) -> Manager<Fake> {
        let dir = std::env::temp_dir().join(format!("wtm-power-{}-{name}", std::process::id()));
        let mut backend = Fake {
            journal: Some(dir.join("original.json")),
            admin: true,
            active: "original-plan".into(),
            ..Default::default()
        };
        for s in [0, 1] {
            backend.values.insert(("original-plan".into(), 0, s), 0);
        }
        Manager {
            backend,
            dir,
            secure: false,
        }
    }
    fn request() -> FrequencyRequest {
        FrequencyRequest {
            plan: "original-plan".into(),
            class: 0,
            source: PowerSupply::Both,
            mhz: 1800,
        }
    }
    fn cleanup(m: &Manager<Fake>) {
        for name in ["operation.lock", "original.json"] {
            let _ = fs::remove_file(m.dir.join(name));
        }
        let _ = fs::remove_dir(&m.dir);
    }
    #[test]
    fn writes_backup_before_mutation_and_restores_original_inactive_plan() {
        let mut m = manager("restore");
        m.apply(&request()).unwrap();
        assert!(load(&m.dir.join("original.json")).unwrap().is_some());
        assert!(m.apply(&request()).unwrap_err().contains("already exists"));
        m.backend.active = "another-plan".into();
        let refreshes = m.backend.refreshes;
        m.restore().unwrap();
        assert_eq!(m.backend.refreshes, refreshes);
        assert_eq!(
            m.backend.read("original-plan", 0, PowerSource::Ac).unwrap(),
            0
        );
        assert!(!m.dir.join("original.json").exists());
        cleanup(&m);
    }
    #[test]
    fn partial_failure_rolls_back_and_external_edits_are_preserved() {
        let mut m = manager("failure");
        m.backend.fail_at = Some(2);
        assert!(
            m.apply(&request())
                .unwrap_err()
                .contains("original values restored")
        );
        assert_eq!(
            m.backend.read("original-plan", 0, PowerSource::Ac).unwrap(),
            0
        );
        assert!(!m.dir.join("original.json").exists());
        m.backend.fail_at = None;
        m.apply(&request()).unwrap();
        m.backend
            .values
            .insert(("original-plan".into(), 0, 1), 2200);
        assert!(m.restore().unwrap_err().contains("externally"));
        assert_eq!(
            m.backend.read("original-plan", 0, PowerSource::Ac).unwrap(),
            1800
        );
        assert!(m.dir.join("original.json").exists());
        cleanup(&m);
    }
    #[test]
    fn invalid_unsupported_unprivileged_and_concurrent_requests_cannot_write() {
        let mut m = manager("guards");
        assert!(
            m.apply(&FrequencyRequest {
                plan: "changed-plan".into(),
                ..request()
            })
            .unwrap_err()
            .contains("plan changed")
        );
        for mhz in [0, 99, 64001] {
            assert!(m.apply(&FrequencyRequest { mhz, ..request() }).is_err());
        }
        assert!(
            m.apply(&FrequencyRequest {
                class: 1,
                ..request()
            })
            .is_err()
        );
        assert_eq!(m.backend.writes, 0);
        m.backend.admin = false;
        assert!(m.apply(&request()).unwrap_err().contains("Administrator"));
        m.backend.admin = true;
        let lock = m.lock().unwrap();
        assert!(m.apply(&request()).is_err());
        drop(lock);
        assert_eq!(m.backend.writes, 0);
        cleanup(&m);
    }

    #[test]
    fn failed_rollback_keeps_the_journal_for_a_later_restore() {
        let mut m = manager("rollback-retry");
        m.backend.fail_at = Some(2);
        m.backend.fail_restore = true;
        assert!(m.apply(&request()).unwrap_err().contains("restore pending"));
        assert!(load(&m.dir.join("original.json")).unwrap().is_some());
        m.backend.fail_at = None;
        m.backend.fail_restore = false;
        m.restore().unwrap();
        assert_eq!(
            m.backend.read("original-plan", 0, PowerSource::Ac).unwrap(),
            0
        );
        assert!(!m.dir.join("original.json").exists());
        cleanup(&m);
    }
}

//! Startup programs (Run keys and Startup folders), enabled/disabled the
//! same way Windows Task Manager does: via `Explorer\StartupApproved`.

use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RegType};
use winreg::{RegKey, RegValue};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN32: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartupScope {
    CurrentUser,
    /// Changing these requires elevation.
    AllUsers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartupSource {
    Run,
    /// 32-bit Run key (all users only).
    Run32,
    Folder,
}

impl StartupSource {
    fn approved_subkey(self) -> &'static str {
        match self {
            Self::Run => "Run",
            Self::Run32 => "Run32",
            Self::Folder => "StartupFolder",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupEntry {
    pub name: String,
    /// Command line (Run keys) or file path (Startup folder).
    pub command: String,
    pub scope: StartupScope,
    pub source: StartupSource,
    pub enabled: bool,
}

/// StartupApproved values: an even first byte (or no value) means enabled.
fn is_enabled(bytes: Option<&[u8]>) -> bool {
    bytes.and_then(<[u8]>::first).is_none_or(|b| b & 1 == 0)
}

/// 12 bytes: state flag, 3 reserved bytes, FILETIME of when it was disabled.
fn approved_bytes(enabled: bool) -> Vec<u8> {
    let mut v = vec![0u8; 12];
    if enabled {
        v[0] = 0x02;
    } else {
        v[0] = 0x03;
        let filetime = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64 / 100 + 116_444_736_000_000_000);
        v[4..].copy_from_slice(&filetime.to_le_bytes());
    }
    v
}

fn root(scope: StartupScope) -> RegKey {
    RegKey::predef(match scope {
        StartupScope::CurrentUser => HKEY_CURRENT_USER,
        StartupScope::AllUsers => HKEY_LOCAL_MACHINE,
    })
}

fn approved(root: &RegKey, approved_base: &str, source: StartupSource, name: &str) -> bool {
    let bytes = root
        .open_subkey_with_flags(
            format!(r"{approved_base}\{}", source.approved_subkey()),
            KEY_READ,
        )
        .and_then(|k| k.get_raw_value(name))
        .ok();
    is_enabled(bytes.as_ref().map(|v| &v.bytes[..]))
}

fn registry_entries(
    root: &RegKey,
    run_path: &str,
    approved_base: &str,
    scope: StartupScope,
    source: StartupSource,
) -> Vec<StartupEntry> {
    let Ok(run) = root.open_subkey_with_flags(run_path, KEY_READ) else {
        return Vec::new();
    };
    run.enum_values()
        .filter_map(Result::ok)
        .filter(|(_, v)| matches!(v.vtype, RegType::REG_SZ | RegType::REG_EXPAND_SZ))
        .map(|(name, _)| StartupEntry {
            command: run.get_value::<String, _>(&name).unwrap_or_default(),
            enabled: approved(root, approved_base, source, &name),
            name,
            scope,
            source,
        })
        .collect()
}

fn startup_folder(scope: StartupScope) -> Option<PathBuf> {
    let (var, tail) = match scope {
        StartupScope::CurrentUser => ("APPDATA", r"Microsoft\Windows\Start Menu\Programs\Startup"),
        StartupScope::AllUsers => (
            "ProgramData",
            r"Microsoft\Windows\Start Menu\Programs\StartUp",
        ),
    };
    std::env::var_os(var).map(|base| PathBuf::from(base).join(tail))
}

fn folder_entries(scope: StartupScope) -> Vec<StartupEntry> {
    folder_entries_in(startup_folder(scope), scope)
}

/// Files in `dir`; `None` (folder location unknown) or an unreadable
/// folder yields nothing.
fn folder_entries_in(dir: Option<PathBuf>, scope: StartupScope) -> Vec<StartupEntry> {
    let Some(dir) = dir else {
        return Vec::new();
    };
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let root = root(scope);
    read.filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            (!name.eq_ignore_ascii_case("desktop.ini")).then(|| StartupEntry {
                enabled: approved(&root, APPROVED, StartupSource::Folder, &name),
                command: e.path().display().to_string(),
                name,
                scope,
                source: StartupSource::Folder,
            })
        })
        .collect()
}

pub fn list_startup() -> Vec<StartupEntry> {
    let mut out = Vec::new();
    for scope in [StartupScope::CurrentUser, StartupScope::AllUsers] {
        let root = root(scope);
        out.extend(registry_entries(
            &root,
            RUN,
            APPROVED,
            scope,
            StartupSource::Run,
        ));
        if scope == StartupScope::AllUsers {
            out.extend(registry_entries(
                &root,
                RUN32,
                APPROVED,
                scope,
                StartupSource::Run32,
            ));
        }
        out.extend(folder_entries(scope));
    }
    out.sort_by_key(|e| e.name.to_lowercase());
    out
}

fn set_approved(
    root: &RegKey,
    approved_base: &str,
    entry: &StartupEntry,
    enabled: bool,
) -> io::Result<()> {
    let (key, _) = root.create_subkey(format!(
        r"{approved_base}\{}",
        entry.source.approved_subkey()
    ))?;
    key.set_raw_value(
        &entry.name,
        &RegValue {
            bytes: approved_bytes(enabled).into(),
            vtype: RegType::REG_BINARY,
        },
    )
}

/// Enables or disables a startup entry without deleting it.
pub fn set_startup_enabled(entry: &StartupEntry, enabled: bool) -> io::Result<()> {
    set_approved(&root(entry.scope), APPROVED, entry, enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_flag_round_trip() {
        assert!(is_enabled(None));
        assert!(is_enabled(Some(&approved_bytes(true))));
        assert!(!is_enabled(Some(&approved_bytes(false))));
        assert!(is_enabled(Some(&[0x06, 0, 0, 0])));
        assert!(!is_enabled(Some(&[0x07, 0, 0, 0])));
    }

    #[test]
    fn toggles_entry_in_scratch_key() {
        let base = format!(r"Software\RustWTM-test-{}", std::process::id());
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let run_path = format!(r"{base}\Run");
        let approved_base = format!(r"{base}\Approved");
        let (run, _) = hkcu.create_subkey(&run_path).unwrap();
        run.set_value("demo", &"notepad.exe").unwrap();

        let list = || {
            registry_entries(
                &hkcu,
                &run_path,
                &approved_base,
                StartupScope::CurrentUser,
                StartupSource::Run,
            )
        };
        let entries = list();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command, "notepad.exe");
        assert!(entries[0].enabled);

        set_approved(&hkcu, &approved_base, &entries[0], false).unwrap();
        assert!(!list()[0].enabled);
        set_approved(&hkcu, &approved_base, &entries[0], true).unwrap();
        assert!(list()[0].enabled);

        hkcu.delete_subkey_all(&base).unwrap();
    }

    #[test]
    fn approved_subkeys_per_source() {
        assert_eq!(StartupSource::Run.approved_subkey(), "Run");
        assert_eq!(StartupSource::Run32.approved_subkey(), "Run32");
        assert_eq!(StartupSource::Folder.approved_subkey(), "StartupFolder");
    }

    #[test]
    fn missing_run_key_lists_nothing() {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let missing = format!(r"Software\RustWTM-test-missing-{}", std::process::id());
        let entries = registry_entries(
            &hkcu,
            &missing,
            &missing,
            StartupScope::CurrentUser,
            StartupSource::Run,
        );
        assert!(entries.is_empty());
    }

    #[test]
    fn folder_entries_skip_desktop_ini_and_subdirectories() {
        assert!(folder_entries_in(None, StartupScope::CurrentUser).is_empty());

        let dir = std::env::temp_dir().join(format!("wtm-startup-test-{}", std::process::id()));
        // Not created yet: unreadable folders yield nothing.
        assert!(folder_entries_in(Some(dir.clone()), StartupScope::CurrentUser).is_empty());

        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("desktop.ini"), b"").unwrap();
        std::fs::write(dir.join("RustWTM-test-app.lnk"), b"").unwrap();
        let entries = folder_entries_in(Some(dir.clone()), StartupScope::CurrentUser);
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(entries.len(), 1, "{entries:?}");
        let e = &entries[0];
        assert_eq!(e.name, "RustWTM-test-app.lnk");
        assert_eq!(e.source, StartupSource::Folder);
        // No StartupApproved value exists for this name, so it is enabled.
        assert!(e.enabled);
        assert!(e.command.ends_with("RustWTM-test-app.lnk"));
    }

    #[test]
    fn set_approved_fails_for_invalid_key_name() {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let entry = StartupEntry {
            name: "demo".into(),
            command: String::new(),
            scope: StartupScope::CurrentUser,
            source: StartupSource::Run,
            enabled: true,
        };
        // Key name components are limited to 255 characters, so nothing is
        // created.
        let base = format!(r"Software\RustWTM-test-{}", "x".repeat(300));
        assert!(set_approved(&hkcu, &base, &entry, false).is_err());
    }

    #[test]
    fn set_startup_enabled_writes_real_approved_key() {
        // Uses a unique value name in the real HKCU key and removes it again.
        let entry = StartupEntry {
            name: format!("RustWTM-test-{}", std::process::id()),
            command: String::new(),
            scope: StartupScope::CurrentUser,
            source: StartupSource::Run,
            enabled: true,
        };
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let subkey = format!(r"{APPROVED}\Run");
        let existed = hkcu.open_subkey(&subkey).is_ok();

        let result = set_startup_enabled(&entry, false);
        let enabled = approved(&hkcu, APPROVED, StartupSource::Run, &entry.name);
        if let Ok(key) = hkcu.open_subkey_with_flags(&subkey, winreg::enums::KEY_SET_VALUE) {
            let _ = key.delete_value(&entry.name);
        }
        // Remove the key too if this test created it.
        let _ = (!existed).then(|| hkcu.delete_subkey(&subkey));

        result.unwrap();
        assert!(!enabled);
    }

    #[test]
    fn lists_real_startup_without_errors() {
        // Content is machine-specific; this only checks it does not fail.
        let _ = list_startup();
    }
}

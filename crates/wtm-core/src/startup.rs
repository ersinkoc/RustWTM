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
    let Some(dir) = startup_folder(scope) else {
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
    fn lists_real_startup_without_errors() {
        // Content is machine-specific; this only checks it does not fail.
        let _ = list_startup();
    }
}

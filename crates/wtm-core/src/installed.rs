//! Read-only desktop application registration and interactive uninstall handoff.
//! Never use Win32_Product: enumeration must not repair installed packages.
use std::{
    io,
    time::{Duration, Instant},
};
use winreg::{RegKey, enums::*};

const ROOT: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
const MAX_ENTRIES: usize = 4096;
const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    User,
    Machine,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppId {
    pub scope: Scope,
    pub view32: bool,
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledApp {
    pub id: AppId,
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub estimated_bytes: Option<u64>,
    pub location: String,
    pub uninstall: String,
    pub msi: bool,
    pub no_remove: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub apps: Vec<InstalledApp>,
    /// An inaccessible source or a bound was reached; successful rows remain usable.
    pub partial: bool,
}

fn text(key: &RegKey, name: &str) -> String {
    match wtm_sys::registry::value(key.raw_handle() as usize, name) {
        Some(wtm_sys::registry::Value::Text(s)) => s.trim().to_owned(),
        _ => String::new(),
    }
}

fn number(key: &RegKey, name: &str) -> Option<u32> {
    match wtm_sys::registry::value(key.raw_handle() as usize, name) {
        Some(wtm_sys::registry::Value::Dword(n)) => Some(n),
        _ => None,
    }
}

fn read(key: &RegKey, id: AppId) -> Option<InstalledApp> {
    if number(key, "SystemComponent").unwrap_or(0) != 0 {
        return None;
    }
    let name = text(key, "DisplayName");
    if name.is_empty() {
        return None;
    }
    Some(InstalledApp {
        id,
        name,
        publisher: text(key, "Publisher"),
        version: text(key, "DisplayVersion"),
        location: text(key, "InstallLocation"),
        uninstall: text(key, "UninstallString"),
        estimated_bytes: number(key, "EstimatedSize")
            .filter(|n| *n > 0)
            .map(|n| n as u64 * 1024),
        msi: number(key, "WindowsInstaller").unwrap_or(0) == 1,
        no_remove: number(key, "NoRemove").unwrap_or(0) != 0,
    })
}

fn source(id: &AppId) -> io::Result<RegKey> {
    RegKey::predef(match id.scope {
        Scope::User => HKEY_CURRENT_USER,
        Scope::Machine => HKEY_LOCAL_MACHINE,
    })
    .open_subkey_with_flags(
        ROOT,
        KEY_READ
            | if id.view32 {
                KEY_WOW64_32KEY
            } else {
                KEY_WOW64_64KEY
            },
    )
}

/// One local scan; no registry writes, package repair, process launch or network.
pub fn list() -> Report {
    let start = Instant::now();
    let mut report = Report::default();
    let mut bytes = 0;
    let mut scanned = 0;
    for scope in [Scope::User, Scope::Machine] {
        for view32 in [false, true] {
            let template = AppId {
                scope,
                view32,
                key: String::new(),
            };
            let root = match source(&template) {
                Ok(root) => root,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => {
                    report.partial = true;
                    continue;
                }
            };
            for key in root.enum_keys() {
                scanned += 1;
                if scanned > MAX_ENTRIES
                    || bytes > MAX_BYTES
                    || start.elapsed() > Duration::from_secs(5)
                {
                    report.partial = true;
                    return report;
                }
                let Ok(name) = key else {
                    report.partial = true;
                    continue;
                };
                let Ok(key) = root.open_subkey_with_flags(&name, KEY_READ) else {
                    report.partial = true;
                    continue;
                };
                let Some(app) = read(
                    &key,
                    AppId {
                        key: name,
                        ..template.clone()
                    },
                ) else {
                    continue;
                };
                // HKCU registry views may alias the same registration. Never merge
                // different product keys simply because their display names match.
                if app.id.scope == Scope::User
                    && report.apps.iter().any(|a| {
                        a.id.scope == app.id.scope
                            && a.id.key == app.id.key
                            && a.name == app.name
                            && a.version == app.version
                            && a.publisher == app.publisher
                            && a.location == app.location
                            && a.uninstall == app.uninstall
                            && a.msi == app.msi
                            && a.no_remove == app.no_remove
                            && a.estimated_bytes == app.estimated_bytes
                    })
                {
                    continue;
                }
                bytes += app.id.key.len()
                    + app.name.len()
                    + app.publisher.len()
                    + app.version.len()
                    + app.location.len()
                    + app.uninstall.len();
                if bytes > MAX_BYTES {
                    report.partial = true;
                    return report;
                }
                report.apps.push(app);
            }
        }
    }
    report.apps.sort_by_cached_key(|a| a.name.to_lowercase());
    report
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchPlan {
    pub executable: String,
    pub arguments: String,
}

fn product_code(key: &str) -> bool {
    key.len() == 38
        && key.starts_with('{')
        && key.ends_with('}')
        && key[1..37].bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

/// Conservative handoff: no command shell, PATH lookup, silent mode or forced UAC.
pub fn plan(app: &InstalledApp) -> Result<LaunchPlan, &'static str> {
    if app.no_remove {
        return Err("Removal is disabled by this application.");
    }
    if app.msi && product_code(&app.id.key) {
        let exe = wtm_sys::shell::system_directory()
            .map_err(|_| "Windows system directory is unavailable.")?
            .join("msiexec.exe");
        return Ok(LaunchPlan {
            executable: exe.to_string_lossy().into_owned(),
            arguments: format!("/x {} /norestart", app.id.key),
        });
    }
    let line = app.uninstall.trim();
    if line.is_empty() || line.chars().any(|c| matches!(c, '\0' | '\r' | '\n')) {
        return Err("No supported interactive uninstaller is registered.");
    }
    let (file, args) = if let Some(rest) = line.strip_prefix('"') {
        rest.split_once('"')
            .ok_or("The registered uninstaller path is ambiguous.")?
    } else {
        let (file, args) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        (file, args)
    };
    let path = std::path::Path::new(file);
    if !path.is_absolute()
        || !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        return Err("The registered uninstaller needs Windows Apps settings.");
    }
    if path.file_name().is_some_and(|n| {
        [
            "cmd.exe",
            "powershell.exe",
            "pwsh.exe",
            "wscript.exe",
            "cscript.exe",
            "mshta.exe",
            "rundll32.exe",
            "regsvr32.exe",
            "msiexec.exe",
        ]
        .iter()
        .any(|v| n.eq_ignore_ascii_case(v))
    }) || args.split_whitespace().any(|arg| {
        [
            "/s",
            "/silent",
            "/verysilent",
            "/quiet",
            "/qn",
            "/q",
            "/passive",
            "--silent",
            "--quiet",
        ]
        .iter()
        .any(|flag| arg.eq_ignore_ascii_case(flag))
    }) {
        return Err("The registered uninstaller needs Windows Apps settings.");
    }
    Ok(LaunchPlan {
        executable: file.into(),
        arguments: args.trim().into(),
    })
}

fn current(app: &InstalledApp) -> Result<InstalledApp, String> {
    if app.id.key.contains(['\\', '/']) {
        return Err("Application registration changed. Refresh the list.".into());
    }
    let root = source(&app.id).map_err(|e| e.to_string())?;
    let key = root
        .open_subkey_with_flags(&app.id.key, KEY_READ)
        .map_err(|e| e.to_string())?;
    read(&key, app.id.clone())
        .ok_or_else(|| "Application registration changed. Refresh the list.".into())
}

fn uninstall_with(
    app: &InstalledApp,
    reread: impl FnOnce(&InstalledApp) -> Result<InstalledApp, String>,
    launch: impl FnOnce(&LaunchPlan) -> Result<(), String>,
) -> Result<(), String> {
    let fresh = reread(app)?;
    if fresh != *app {
        return Err("Application registration changed. Refresh the list.".into());
    }
    launch(&plan(&fresh).map_err(str::to_owned)?)
}

/// Starts the registered interactive uninstaller; success means started, not removed.
pub fn uninstall(app: &InstalledApp) -> Result<(), String> {
    uninstall_with(app, current, |p| {
        wtm_sys::shell::launch(&p.executable, &p.arguments, false).map_err(|e| e.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> InstalledApp {
        InstalledApp {
            id: AppId {
                scope: Scope::User,
                view32: false,
                key: "example".into(),
            },
            name: "Example".into(),
            publisher: "Maker".into(),
            version: "1".into(),
            estimated_bytes: None,
            location: String::new(),
            uninstall: r#""C:\Program Files\Example\uninstall.exe" /interactive"#.into(),
            msi: false,
            no_remove: false,
        }
    }
    #[test]
    fn interactive_plans_reject_ambiguous_or_missing_programs() {
        let mut a = app();
        assert_eq!(plan(&a).unwrap().arguments, "/interactive");
        for command in [
            "",
            "uninstall.exe /s",
            r"C:\Program Files\Example\uninstall.exe /s",
            "ms-settings:appsfeatures",
            "\"C:\\unterminated.exe",
            "C:\\test.exe\n/f",
        ] {
            a.uninstall = command.into();
            assert!(plan(&a).is_err(), "{command}");
        }
        a = app();
        a.no_remove = true;
        assert!(plan(&a).is_err());
        a = app();
        a.msi = true;
        a.id.key = "{12345678-1234-1234-1234-1234567890AB}".into();
        let p = plan(&a).unwrap();
        assert!(p.executable.ends_with("msiexec.exe"));
        assert_eq!(
            p.arguments,
            "/x {12345678-1234-1234-1234-1234567890AB} /norestart"
        );
        assert!(!product_code("{../../not-a-product-code}"));
    }
    #[test]
    fn changed_registration_never_launches_and_failure_is_reported() {
        let a = app();
        let calls = std::cell::Cell::new(0);
        let launch = |_: &LaunchPlan| {
            calls.set(calls.get() + 1);
            Ok(())
        };
        uninstall_with(&a, |a| Ok(a.clone()), launch).unwrap();
        assert_eq!(calls.get(), 1);
        assert!(
            uninstall_with(
                &a,
                |a| {
                    let mut b = a.clone();
                    b.uninstall = "changed".into();
                    Ok(b)
                },
                launch
            )
            .is_err()
        );
        assert_eq!(calls.get(), 1);
        assert!(uninstall_with(&a, |_| Err("missing".into()), launch).is_err());
        assert_eq!(calls.get(), 1);
        assert_eq!(
            uninstall_with(&a, |a| Ok(a.clone()), |_| Err("launch refused".into())).unwrap_err(),
            "launch refused"
        );
    }
    #[test]
    fn unsupported_shell_and_quiet_handoffs_use_settings_instead() {
        let mut a = app();
        for command in [
            r#""C:\Windows\System32\cmd.exe" /c uninstall.cmd"#,
            r#""C:\Program Files\Example\uninstall.exe" /quiet"#,
            r#""C:\Program Files\Example\uninstall.exe" /S"#,
            r#""C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe" -File remove.ps1"#,
        ] {
            a.uninstall = command.into();
            assert!(plan(&a).is_err());
        }
    }
    #[test]
    #[ignore = "read-only local installed application inventory; run explicitly"]
    fn local_inventory_smoke() {
        let r = list();
        assert!(!r.apps.is_empty());
        println!(
            "Desktop applications: {}; partial: {}",
            r.apps.len(),
            r.partial
        );
        assert!(r.apps.len() <= MAX_ENTRIES);
    }
}

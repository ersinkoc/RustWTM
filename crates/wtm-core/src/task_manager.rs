//! Opt-in, owned Task Manager replacement. Tests use only scratch HKCU keys.
use std::io;
use std::path::{Path, PathBuf};
use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, KEY_WRITE, REG_DWORD, REG_SZ};

const KEY: &str =
    r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe";
const OWNER: &str = "RustWTMTaskManager";
const MARKER: &str = "RustWTM task manager v1";
pub const LAUNCH_ARG: &str = "--task-manager";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub enum State {
    #[default]
    Windows,
    RustWTM,
    Other,
}

fn gui_path() -> Result<PathBuf, String> {
    std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .map(|p| p.join("rustwtm.exe"))
        .ok_or_else(|| "Missing program directory".into())
}

fn command(exe: &Path) -> Result<String, String> {
    let path = exe
        .to_str()
        .ok_or("The installation path is not valid Unicode")?;
    if !exe.is_absolute() || path.contains(['"', '\r', '\n', '\0']) {
        return Err("Invalid Task Manager replacement path".into());
    }
    Ok(format!("\"{path}\" {LAUNCH_ARG}"))
}

fn open(root: &RegKey, path: &str, write: bool) -> io::Result<Option<RegKey>> {
    match root.open_subkey_with_flags(
        path,
        KEY_READ | KEY_WOW64_64KEY | if write { KEY_WRITE } else { 0 },
    ) {
        Ok(key) => Ok(Some(key)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn registration_state(key: Option<&RegKey>, expected: &str) -> io::Result<State> {
    let Some(key) = key else {
        return Ok(State::Windows);
    };
    let raw = match key.get_raw_value("Debugger") {
        Ok(value) => value,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(State::Windows),
        Err(e) => return Err(e),
    };
    if raw.vtype != REG_SZ {
        return Ok(State::Other);
    }
    let debugger: String = key.get_value("Debugger")?;
    if debugger.is_empty() {
        return Ok(State::Windows);
    }
    let owner: Option<String> = match key.get_value(OWNER) {
        Ok(value) => Some(value),
        Err(e) if e.kind() == io::ErrorKind::NotFound || e.kind() == io::ErrorKind::InvalidData => {
            None
        }
        Err(e) => return Err(e),
    };
    Ok(
        if owner.as_deref() == Some(MARKER) && debugger.eq_ignore_ascii_case(expected) {
            State::RustWTM
        } else {
            State::Other
        },
    )
}

fn state(key: Option<&RegKey>, expected: &str) -> io::Result<State> {
    if let Some(key) = key {
        match key.get_raw_value("UseFilter") {
            Ok(value) => {
                if value.vtype != REG_DWORD || key.get_value::<u32, _>("UseFilter")? != 0 {
                    return Ok(State::Other);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    registration_state(key, expected)
}

fn remove_owned(key: &RegKey) -> Result<(), String> {
    key.delete_value("Debugger").map_err(|e| e.to_string())?;
    key.delete_value(OWNER).map_err(|e| e.to_string())?;
    Ok(())
}

/// Read-only. Unknown or unreadable registration is never reported as disabled.
pub fn status() -> Result<State, String> {
    let expected = command(&gui_path()?)?;
    let key = open(&RegKey::predef(HKEY_LOCAL_MACHINE), KEY, false).map_err(|e| e.to_string())?;
    state(key.as_ref(), &expected).map_err(|e| e.to_string())
}

fn configure_in(root: &RegKey, path: &str, exe: &Path, enabled: bool) -> Result<State, String> {
    let expected = command(exe)?;
    let key = open(root, path, true).map_err(|e| e.to_string())?;
    let current = state(key.as_ref(), &expected).map_err(|e| e.to_string())?;
    if current == State::Other {
        if !enabled
            && registration_state(key.as_ref(), &expected).map_err(|e| e.to_string())?
                == State::RustWTM
        {
            let key = key.as_ref().expect("owned value requires a key");
            remove_owned(key)?;
            return state(Some(key), &expected).map_err(|e| e.to_string());
        }
        return Err("Another application controls the Task Manager replacement".into());
    }
    if enabled && current == State::Windows {
        let key = match key {
            Some(key) => key,
            None => {
                root.create_subkey_with_flags(path, KEY_READ | KEY_WRITE | KEY_WOW64_64KEY)
                    .map_err(|e| e.to_string())?
                    .0
            }
        };
        // Write ownership first: a failed debugger write can leave only a
        // harmless marker, never an unowned redirect to a future deleted file.
        key.set_value(OWNER, &MARKER).map_err(|e| e.to_string())?;
        key.set_value("Debugger", &expected)
            .map_err(|e| e.to_string())?;
    } else if !enabled && current == State::RustWTM {
        let key = key.expect("owned value requires a key");
        remove_owned(&key)?;
    }
    Ok(if enabled {
        State::RustWTM
    } else {
        State::Windows
    })
}

/// Requires an elevated caller and an administrator-controlled installation.
/// It does not replace an existing debugger owned by another program.
pub fn configure(enabled: bool) -> Result<State, String> {
    if !crate::launch::is_elevated() {
        return Err(
            "Restart RustWTM as administrator to change the Task Manager replacement".into(),
        );
    }
    let exe = gui_path()?;
    if enabled {
        protected_installation(&exe)?;
    }
    configure_in(&RegKey::predef(HKEY_LOCAL_MACHINE), KEY, &exe, enabled)
}

fn protected_installation(exe: &Path) -> Result<(), String> {
    if !exe.is_file() || !wtm_sys::security::only_admins_control(exe).map_err(|e| e.to_string())? {
        return Err(
            "Task Manager replacement requires an administrator-protected RustWTM installation"
                .into(),
        );
    }
    Ok(())
}

/// Full uninstall only: preserve other debuggers and unrelated IFEO values.
pub fn cleanup(exe: &Path) -> Result<(), String> {
    cleanup_in(&RegKey::predef(HKEY_LOCAL_MACHINE), KEY, exe)
}

fn cleanup_in(root: &RegKey, path: &str, exe: &Path) -> Result<(), String> {
    let expected = command(exe)?;
    let key = open(root, path, false).map_err(|e| e.to_string())?;
    if registration_state(key.as_ref(), &expected).map_err(|e| e.to_string())? == State::RustWTM {
        // Do not demand write access to a foreign registration. The mutating
        // path rechecks ownership after reopening the key for writing.
        configure_in(root, path, exe, false)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use winreg::enums::HKEY_CURRENT_USER;

    struct Scratch {
        root: RegKey,
        path: String,
    }
    impl Scratch {
        fn new(name: &str) -> Self {
            Self {
                root: RegKey::predef(HKEY_CURRENT_USER),
                path: format!(
                    r"Software\RustWTM-test-task-manager-{name}-{}",
                    std::process::id()
                ),
            }
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = self.root.delete_subkey_all(&self.path);
        }
    }

    #[test]
    fn registration_is_owned_reversible_and_preserves_other_values() {
        let scratch = Scratch::new("owned");
        let exe = Path::new(r"C:\Program Files\RustWTM\rustwtm.exe");
        assert_eq!(
            configure_in(&scratch.root, &scratch.path, exe, false).unwrap(),
            State::Windows
        );
        assert!(open(&scratch.root, &scratch.path, false).unwrap().is_none());
        for _ in 0..2 {
            assert_eq!(
                configure_in(&scratch.root, &scratch.path, exe, true).unwrap(),
                State::RustWTM
            );
        }
        let key = open(&scratch.root, &scratch.path, true).unwrap().unwrap();
        assert_eq!(
            key.get_value::<String, _>("Debugger").unwrap(),
            r#""C:\Program Files\RustWTM\rustwtm.exe" --task-manager"#
        );
        key.set_value("GlobalFlag", &0u32).unwrap();
        for _ in 0..2 {
            assert_eq!(
                configure_in(&scratch.root, &scratch.path, exe, false).unwrap(),
                State::Windows
            );
        }
        assert_eq!(key.get_value::<u32, _>("GlobalFlag").unwrap(), 0);
        assert!(key.get_raw_value("Debugger").is_err());
    }

    #[test]
    fn foreign_and_replaced_debuggers_are_never_modified() {
        let scratch = Scratch::new("foreign");
        let exe = Path::new(r"C:\Program Files\RustWTM\rustwtm.exe");
        let key = scratch.root.create_subkey(&scratch.path).unwrap().0;
        for debugger in [
            r#""C:\Other\manager.exe""#,
            r#""C:\Other\rustwtm.exe" --task-manager"#,
        ] {
            key.set_value("Debugger", &debugger).unwrap();
            for enabled in [true, false] {
                assert!(configure_in(&scratch.root, &scratch.path, exe, enabled).is_err());
            }
            assert_eq!(key.get_value::<String, _>("Debugger").unwrap(), debugger);
        }
        key.set_value("Debugger", &23u32).unwrap();
        assert!(configure_in(&scratch.root, &scratch.path, exe, true).is_err());
        assert_eq!(key.get_value::<u32, _>("Debugger").unwrap(), 23);
        key.delete_value("Debugger").unwrap();
        configure_in(&scratch.root, &scratch.path, exe, true).unwrap();
        key.set_value("Debugger", &r#""C:\Other\manager.exe""#)
            .unwrap();
        cleanup_in(&scratch.root, &scratch.path, exe).unwrap();
        assert_eq!(
            key.get_value::<String, _>("Debugger").unwrap(),
            r#""C:\Other\manager.exe""#
        );
    }

    #[test]
    fn installation_command_is_literal_and_rejects_invalid_paths() {
        assert_eq!(
            command(Path::new(r"C:\Program Files\it's ’ app\rustwtm.exe")).unwrap(),
            "\"C:\\Program Files\\it's ’ app\\rustwtm.exe\" --task-manager"
        );
        for path in [
            "relative.exe",
            "C:\\bad\"path\\rustwtm.exe",
            "C:\\bad\npath\\rustwtm.exe",
        ] {
            assert!(command(Path::new(path)).is_err());
        }
        // The writable development build must never be registered globally.
        let current = std::env::current_exe().unwrap();
        assert!(!wtm_sys::security::only_admins_control(&current).unwrap());
        assert!(protected_installation(&current).is_err());
        assert!(
            protected_installation(
                &wtm_sys::shell::system_directory()
                    .unwrap()
                    .join("WindowsPowerShell/v1.0/powershell.exe")
            )
            .is_ok()
        );
    }

    #[test]
    fn image_filters_are_not_overwritten_or_reported_as_active_replacement() {
        let scratch = Scratch::new("filters");
        let exe = Path::new(r"C:\Program Files\RustWTM\rustwtm.exe");
        let key = scratch.root.create_subkey(&scratch.path).unwrap().0;
        key.set_value("UseFilter", &1u32).unwrap();
        assert!(configure_in(&scratch.root, &scratch.path, exe, true).is_err());
        assert!(key.get_raw_value("Debugger").is_err());
        key.set_value("UseFilter", &0u32).unwrap();
        configure_in(&scratch.root, &scratch.path, exe, true).unwrap();
        key.set_value("UseFilter", &1u32).unwrap();
        assert_eq!(
            state(Some(&key), &command(exe).unwrap()).unwrap(),
            State::Other
        );
        cleanup_in(&scratch.root, &scratch.path, exe).unwrap();
        assert!(key.get_raw_value("Debugger").is_err());
        assert_eq!(key.get_value::<u32, _>("UseFilter").unwrap(), 1);
    }
}

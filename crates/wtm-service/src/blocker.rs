//! Prevents programs from starting via Image File Execution Options: Windows
//! launches the configured "debugger" instead of the blocked image.
//!
//! Only keys marked with [`MARKER`] are ever changed or removed, and a
//! `Debugger` value set by another program is left alone.

use std::collections::BTreeSet;
use std::io;

use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS};

pub const IFEO: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options";
/// DWORD value identifying entries RustWTM created.
const MARKER: &str = "RustWTMBlock";
const DEBUGGER: &str = "Debugger";
/// How RustWTM's debugger command ends (see the service's `block_debugger`).
const BLOCKED_ARG: &str = "--blocked";

pub struct Blocker {
    root: RegKey,
    /// Command run instead of a blocked program; Windows appends the
    /// original command line.
    debugger: String,
}

/// What [`Blocker::sync`] changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Names whose IFEO entry already has someone else's debugger.
    pub conflicts: Vec<String>,
}

impl Blocker {
    /// The real IFEO key; changes need administrator rights.
    pub fn system(debugger: String) -> io::Result<Self> {
        Self::at(RegKey::predef(HKEY_LOCAL_MACHINE), IFEO, debugger)
    }

    /// Entries are managed below `parent\path`; tests use a scratch key.
    pub fn at(parent: RegKey, path: &str, debugger: String) -> io::Result<Self> {
        let (root, _) = parent.create_subkey(path)?;
        Ok(Self { root, debugger })
    }

    /// Image names currently blocked by RustWTM (lower case).
    pub fn blocked(&self) -> BTreeSet<String> {
        self.root
            .enum_keys()
            .filter_map(Result::ok)
            .filter(|name| {
                self.root
                    .open_subkey(name)
                    .and_then(|k| k.get_value::<u32, _>(MARKER))
                    .is_ok()
            })
            .map(|name| name.to_lowercase())
            .collect()
    }

    /// Makes the set of RustWTM entries equal to `names`.
    pub fn sync(&self, names: &[String]) -> io::Result<SyncReport> {
        let wanted: BTreeSet<String> = names.iter().map(|n| n.to_lowercase()).collect();
        let current = self.blocked();
        let mut report = SyncReport::default();
        for name in current.difference(&wanted) {
            self.clear(name)?;
            report.removed.push(name.clone());
        }
        for name in wanted.difference(&current) {
            if self.add(name)? {
                report.added.push(name.clone());
            } else {
                report.conflicts.push(name.clone());
            }
        }
        Ok(report)
    }

    /// Returns false when another program owns the entry's debugger.
    fn add(&self, name: &str) -> io::Result<bool> {
        let (key, _) = self.root.create_subkey(name)?;
        if key.get_value::<String, _>(DEBUGGER).is_ok() {
            return Ok(false);
        }
        key.set_value(DEBUGGER, &self.debugger)?;
        key.set_value(MARKER, &1u32)?;
        Ok(true)
    }

    /// Removes every entry RustWTM marked (on uninstall), carrying on past
    /// failures. A debugger that is no longer RustWTM's (another program
    /// replaced it) is left in place; only the marker goes. Returns the
    /// names cleared and the failures.
    pub fn remove_all(&self) -> (Vec<String>, Vec<String>) {
        let (mut removed, mut failed) = (Vec::new(), Vec::new());
        for name in self.blocked() {
            match self.clear(&name) {
                Ok(()) => removed.push(name),
                Err(e) => failed.push(format!("{name}: {e}")),
            }
        }
        (removed, failed)
    }

    /// Takes RustWTM's entry for `name` back out: its debugger only while it
    /// is still RustWTM's, then the marker, then the key once it is empty.
    fn clear(&self, name: &str) -> io::Result<()> {
        let key = self.root.open_subkey_with_flags(name, KEY_ALL_ACCESS)?;
        let ours = key
            .get_value::<String, _>(DEBUGGER)
            .is_ok_and(|d| d.trim_end().ends_with(BLOCKED_ARG));
        if ours {
            key.delete_value(DEBUGGER)?;
        }
        key.delete_value(MARKER)?;
        if key.enum_values().next().is_none() && key.enum_keys().next().is_none() {
            drop(key);
            self.root.delete_subkey(name)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winreg::enums::HKEY_CURRENT_USER;

    /// A blocker on a scratch key under HKCU, deleted on drop.
    struct Scratch {
        path: String,
        blocker: Blocker,
    }

    impl Scratch {
        fn new(test: &str) -> Self {
            let path = format!(r"Software\RustWTM-test-ifeo-{test}-{}", std::process::id());
            let blocker = Blocker::at(
                RegKey::predef(HKEY_CURRENT_USER),
                &path,
                r#""C:\RustWTM\rustwtm.exe" --blocked"#.into(),
            )
            .unwrap();
            Self { path, blocker }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.path);
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn sync_adds_and_removes_only_its_own_entries() {
        let s = Scratch::new("sync");
        let b = &s.blocker;
        // Another program's entries: one with a debugger, one with settings.
        let (foreign, _) = b.root.create_subkey("other.exe").unwrap();
        foreign.set_value(DEBUGGER, &"vsjitdebugger.exe").unwrap();
        let (tuned, _) = b.root.create_subkey("tuned.exe").unwrap();
        tuned.set_value("GlobalFlag", &2u32).unwrap();

        let r = b
            .sync(&names(&["XMRig.exe", "other.exe", "tuned.exe"]))
            .unwrap();
        assert_eq!(r.added, ["tuned.exe", "xmrig.exe"]);
        assert_eq!(r.conflicts, ["other.exe"]);
        assert!(r.removed.is_empty());
        assert_eq!(
            b.blocked().into_iter().collect::<Vec<_>>(),
            ["tuned.exe", "xmrig.exe"]
        );
        let debugger: String = b
            .root
            .open_subkey("xmrig.exe")
            .unwrap()
            .get_value(DEBUGGER)
            .unwrap();
        assert!(debugger.ends_with("--blocked"));

        // Idempotent.
        assert_eq!(
            b.sync(&names(&["xmrig.exe", "tuned.exe"])).unwrap(),
            SyncReport::default()
        );

        // Removing restores the previous state exactly.
        let r = b.sync(&[]).unwrap();
        assert_eq!(r.removed, ["tuned.exe", "xmrig.exe"]);
        assert!(b.blocked().is_empty());
        assert!(b.root.open_subkey("xmrig.exe").is_err(), "own key deleted");
        let tuned = b.root.open_subkey("tuned.exe").unwrap();
        assert_eq!(tuned.get_value::<u32, _>("GlobalFlag").unwrap(), 2);
        assert!(tuned.get_value::<String, _>(DEBUGGER).is_err());
        let foreign: String = b
            .root
            .open_subkey("other.exe")
            .unwrap()
            .get_value(DEBUGGER)
            .unwrap();
        assert_eq!(foreign, "vsjitdebugger.exe");
    }

    #[test]
    fn unblocking_keeps_a_debugger_another_program_put_there() {
        let s = Scratch::new("sync-taken");
        let b = &s.blocker;
        b.sync(&names(&["a.exe", "b.exe"])).unwrap();
        let (taken, _) = b.root.create_subkey("b.exe").unwrap();
        taken.set_value(DEBUGGER, &"vsjitdebugger.exe").unwrap();
        // Marked, but its debugger already vanished.
        let (broken, _) = b.root.create_subkey("broken.exe").unwrap();
        broken.set_value(MARKER, &1u32).unwrap();

        let r = b.sync(&[]).unwrap();
        assert_eq!(r.removed, ["a.exe", "b.exe", "broken.exe"]);
        assert!(b.blocked().is_empty());
        assert!(b.root.open_subkey("a.exe").is_err());
        assert!(b.root.open_subkey("broken.exe").is_err());
        let foreign: String = b
            .root
            .open_subkey("b.exe")
            .unwrap()
            .get_value(DEBUGGER)
            .unwrap();
        assert_eq!(foreign, "vsjitdebugger.exe");
    }

    #[test]
    fn remove_all_clears_only_marked_entries() {
        let s = Scratch::new("remove-all");
        let b = &s.blocker;
        b.sync(&names(&["a.exe", "b.exe"])).unwrap();
        // Another program took over b.exe's debugger meanwhile.
        let (taken, _) = b.root.create_subkey("b.exe").unwrap();
        taken.set_value(DEBUGGER, &"vsjitdebugger.exe").unwrap();
        // Marked, but its debugger already vanished.
        let (broken, _) = b.root.create_subkey("broken.exe").unwrap();
        broken.set_value(MARKER, &1u32).unwrap();
        let (foreign, _) = b.root.create_subkey("other.exe").unwrap();
        foreign.set_value(DEBUGGER, &"x.exe").unwrap();

        let (removed, failed) = b.remove_all();
        assert_eq!(removed, ["a.exe", "b.exe", "broken.exe"]);
        assert!(failed.is_empty(), "{failed:?}");
        assert!(b.blocked().is_empty());
        assert!(b.root.open_subkey("a.exe").is_err());
        assert!(b.root.open_subkey("broken.exe").is_err());
        let kept: String = b
            .root
            .open_subkey("b.exe")
            .unwrap()
            .get_value(DEBUGGER)
            .unwrap();
        assert_eq!(kept, "vsjitdebugger.exe");
        let other: String = b
            .root
            .open_subkey("other.exe")
            .unwrap()
            .get_value(DEBUGGER)
            .unwrap();
        assert_eq!(other, "x.exe");
        assert_eq!(b.remove_all(), (Vec::new(), Vec::new()));
    }

    #[test]
    fn errors_are_reported() {
        let s = Scratch::new("errors");
        // Registry key names are limited to 255 characters.
        let too_long = format!("{}.exe", "x".repeat(300));
        assert!(s.blocker.sync(&[too_long]).is_err());
    }

    /// Writes the real IFEO key, so it only runs where RUSTWTM_SCM_TESTS is
    /// set (an elevated, disposable CI machine).
    #[test]
    fn windows_runs_the_debugger_instead_of_a_blocked_program() {
        if std::env::var_os("RUSTWTM_SCM_TESTS").is_none() {
            return;
        }
        let name = format!("wtm-ifeo-test-{}.exe", std::process::id());
        let exe = std::env::temp_dir().join(&name);
        std::fs::copy(r"C:\Windows\System32\cmd.exe", &exe).unwrap();
        // Windows appends the original command line: xit 7 "<exe>" /c exit 0.
        let blocker = Blocker::system(r"C:\Windows\System32\cmd.exe /c exit 7".into()).unwrap();
        struct Unblock<'a>(&'a Blocker);
        impl Drop for Unblock<'_> {
            fn drop(&mut self) {
                let _ = self.0.sync(&[]);
            }
        }
        let _unblock = Unblock(&blocker);
        let run = || {
            std::process::Command::new(&exe)
                .args(["/c", "exit 0"])
                .status()
                .unwrap()
                .code()
        };
        assert_eq!(run(), Some(0));
        let report = blocker.sync(std::slice::from_ref(&name)).unwrap();
        assert_eq!(report.added, [name.to_lowercase()]);
        assert_eq!(run(), Some(7), "the blocked program did not start");
        blocker.sync(&[]).unwrap();
        assert_eq!(run(), Some(0));
        let _ = std::fs::remove_file(&exe);
    }
}

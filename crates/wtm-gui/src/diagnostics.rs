//! "Copy diagnostic information": the facts a problem report needs, and
//! nothing personal. Process names, command lines, account and computer
//! names, file paths, serial numbers and network addresses are never part
//! of it.

use wtm_core::format_bytes;

/// What the report says. Built only when the user asks for it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    pub version: String,
    /// Product name, display version and build, e.g. "Windows 11 Pro 24H2 (26100.2605)".
    pub windows: String,
    pub elevated: bool,
    /// How this window gets its data (service, read-only service, local).
    pub engine: String,
    /// Installed under Program Files, or a portable copy.
    pub installed: bool,
    pub language: String,
    pub logical_cpus: u32,
    pub memory: u64,
    pub scale: f32,
    pub window: (f32, f32),
}

impl Facts {
    /// The report as plain text, in English: it is read by maintainers.
    pub fn report(&self) -> String {
        let lines = [
            ("RustWTM", self.version.clone()),
            ("Windows", self.windows.clone()),
            (
                "Install",
                if self.installed {
                    "installed"
                } else {
                    "portable"
                }
                .into(),
            ),
            (
                "Rights",
                if self.elevated {
                    "administrator"
                } else {
                    "standard user"
                }
                .into(),
            ),
            ("Engine", self.engine.clone()),
            ("Language", self.language.clone()),
            ("Logical processors", self.logical_cpus.to_string()),
            ("Memory", format_bytes(self.memory as f64)),
            ("Interface scale", format!("{:.0}%", self.scale * 100.0)),
            (
                "Window",
                format!("{:.0} x {:.0}", self.window.0, self.window.1),
            ),
        ];
        let mut text = String::from("RustWTM diagnostic information\n");
        for (label, value) in lines {
            let value = if value.trim().is_empty() {
                "unknown".into()
            } else {
                value
            };
            text.push_str(&format!("{label}: {value}\n"));
        }
        text
    }
}

/// The Windows edition, version and build from the registry, without the
/// registered owner or organization.
pub fn windows_version() -> String {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let Ok(key) = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
    else {
        return String::new();
    };
    let text = |name: &str| key.get_value::<String, _>(name).unwrap_or_default();
    let build = text("CurrentBuild");
    // The registry still says "Windows 10" on Windows 11; the build tells.
    let mut product = text("ProductName");
    if build.parse::<u32>().is_ok_and(|b| b >= 22_000) {
        product = product.replacen("Windows 10", "Windows 11", 1);
    }
    let revision = key.get_value::<u32, _>("UBR").ok();
    let build = match revision {
        Some(ubr) => format!("{build}.{ubr}"),
        None => build,
    };
    format!("{product} {} ({build})", text("DisplayVersion"))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether this copy runs from Program Files.
pub fn installed() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    ["ProgramFiles", "ProgramW6432"]
        .iter()
        .filter_map(std::env::var_os)
        .any(|dir| exe.starts_with(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_lists_settings_and_leaves_unknowns_marked() {
        let facts = Facts {
            version: "0.30.1".into(),
            windows: "Windows 11 Pro 24H2 (26100.2605)".into(),
            elevated: true,
            engine: "service".into(),
            installed: true,
            language: "tr".into(),
            logical_cpus: 16,
            memory: 32 << 30,
            scale: 1.25,
            window: (1280.0, 800.0),
        };
        let text = facts.report();
        for line in [
            "RustWTM: 0.30.1",
            "Windows: Windows 11 Pro 24H2 (26100.2605)",
            "Install: installed",
            "Rights: administrator",
            "Interface scale: 125%",
            "Window: 1280 x 800",
        ] {
            assert!(text.contains(line), "{line}\n{text}");
        }
        let empty = Facts::default().report();
        assert!(empty.contains("Windows: unknown") && empty.contains("Rights: standard user"));
    }

    #[test]
    fn nothing_personal_reaches_the_report() {
        let facts = Facts {
            windows: windows_version(),
            installed: installed(),
            ..Facts::default()
        };
        let text = facts.report();
        for private in [std::env::var("USERNAME"), std::env::var("COMPUTERNAME")]
            .into_iter()
            .flatten()
            .filter(|v| v.len() > 2)
        {
            assert!(!text.contains(&private), "{text}");
        }
        assert!(!text.contains(":\\"), "no paths: {text}");
        assert!(facts.windows.contains("Windows"), "{}", facts.windows);
    }
}

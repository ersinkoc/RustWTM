//! Processes RustWTM must never kill, suspend or throttle.

/// Core OS processes. Matched case-insensitively against the image name.
/// Name matching errs on the side of safety: an impostor `csrss.exe` is also
/// left alone until path verification lands with the governor.
const PROTECTED_NAMES: &[&str] = &[
    "System Idle Process",
    "System",
    "Secure System",
    "Registry",
    "Memory Compression",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "LsaIso.exe",
    "dwm.exe",
    "fontdrvhost.exe",
];

/// Why a policy acting on its own (kill list, rules, governor, game mode,
/// insights) must never end or suspend a process called `name`, which a
/// person may still do by hand: a service host takes Windows services with
/// it (DcomLaunch's forces a restart), and RustWTM's own programs are the
/// way back.
pub fn automatic_end_blocked(name: &str) -> Option<&'static str> {
    const SERVICE_HOSTS: [&str; 1] = ["svchost.exe"];
    const RUSTWTM: [&str; 3] = ["rustwtm.exe", "wtm.exe", "wtm-service.exe"];
    if SERVICE_HOSTS.iter().any(|n| n.eq_ignore_ascii_case(name)) {
        Some("it hosts Windows services")
    } else if RUSTWTM.iter().any(|n| n.eq_ignore_ascii_case(name)) {
        Some("it is part of RustWTM")
    } else {
        None
    }
}

/// Returns why `pid`/`name` is protected, or `None` if it may be controlled.
pub fn protection_reason(pid: u32, name: &str) -> Option<&'static str> {
    if pid == 0 || pid == 4 {
        return Some("kernel process");
    }
    if pid == std::process::id() {
        return Some("RustWTM itself");
    }
    PROTECTED_NAMES
        .iter()
        .any(|p| p.eq_ignore_ascii_case(name))
        .then_some("core Windows process")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policies_never_end_service_hosts_or_rustwtm() {
        assert!(automatic_end_blocked("SVCHOST.EXE").is_some());
        assert!(automatic_end_blocked("wtm-service.exe").is_some());
        assert!(automatic_end_blocked("chrome.exe").is_none());
    }

    #[test]
    fn protects_core_processes() {
        assert!(protection_reason(0, "").is_some());
        assert!(protection_reason(4, "System").is_some());
        assert!(protection_reason(999_999, "CSRSS.EXE").is_some());
        assert!(protection_reason(std::process::id(), "x").is_some());
        assert!(protection_reason(999_999, "notepad.exe").is_none());
    }
}

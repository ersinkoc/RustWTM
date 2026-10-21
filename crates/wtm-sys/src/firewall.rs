//! Cutting a program off the network with Windows Firewall: two rules, in
//! and out, that block its executable. They are named and grouped as
//! RustWTM's, so they are easy to find in Windows' own firewall settings too.
//! Removing them gives the program its network back. Needs administrator
//! rights.

use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, VARIANT_TRUE};
use windows::Win32::NetworkManagement::WindowsFirewall::{
    INetFwPolicy2, INetFwRule, INetFwRules, NET_FW_ACTION_BLOCK, NET_FW_PROFILE2_ALL,
    NET_FW_RULE_DIR_IN, NET_FW_RULE_DIR_OUT, NET_FW_RULE_DIRECTION, NetFwPolicy2, NetFwRule,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    IDispatch,
};
use windows::Win32::System::Ole::IEnumVARIANT;
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BSTR, Interface};

use crate::Result;

/// The rule group, as Windows' firewall settings show it.
pub const GROUP: &str = "RustWTM";

/// Blocks the program at `path` in both directions.
pub fn block(path: &str) -> Result<()> {
    let _com = ComScope::enter();
    let rules = firewall_rules()?;
    let existing = group_rules(&rules)?;
    for direction in [NET_FW_RULE_DIR_IN, NET_FW_RULE_DIR_OUT] {
        let already = existing
            .iter()
            .any(|r| same_program(&r.program, path) && r.direction == direction);
        if !already {
            rules.add_block(path, direction)?;
        }
    }
    Ok(())
}

/// Removes the rules [`block`] made.
pub fn unblock(path: &str) -> Result<()> {
    let _com = ComScope::enter();
    let rules = firewall_rules()?;
    let mut names: Vec<String> = group_rules(&rules)?
        .into_iter()
        .filter(|r| same_program(&r.program, path))
        .map(|r| r.name)
        .collect();
    names.sort();
    names.dedup();
    for name in names {
        // SAFETY: plain COM call on a live interface. Removing by name drops
        // every rule carrying it.
        unsafe { rules.0.Remove(&BSTR::from(name.as_str()))? };
    }
    Ok(())
}

/// Programs RustWTM has blocked: the paths of the rules in its group.
pub fn blocked() -> Result<Vec<String>> {
    let _com = ComScope::enter();
    let rules = firewall_rules()?;
    let mut programs: Vec<String> = Vec::new();
    for rule in group_rules(&rules)? {
        if !rule.program.is_empty() && !programs.iter().any(|p| same_program(p, &rule.program)) {
            programs.push(rule.program);
        }
    }
    programs.sort_by_key(|p| p.to_lowercase());
    Ok(programs)
}

/// Keeps COM initialised on this thread for the scope of one call. Declared
/// first in each function, so every interface is released before it drops.
struct ComScope {
    owned: bool,
}

impl ComScope {
    fn enter() -> Self {
        // SAFETY: balanced by `Drop` when it succeeds. A thread already in a
        // single-threaded apartment reports RPC_E_CHANGED_MODE and can still
        // make the calls below, so that is not treated as an error.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self {
            owned: hr.is_ok() && hr != RPC_E_CHANGED_MODE,
        }
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: pairs with the successful CoInitializeEx in `enter`.
            unsafe { CoUninitialize() };
        }
    }
}

/// The firewall's rule collection.
struct Rules(INetFwRules);

/// The parts of a RustWTM rule this module cares about.
struct GroupRule {
    name: String,
    program: String,
    direction: NET_FW_RULE_DIRECTION,
}

fn firewall_rules() -> Result<Rules> {
    // SAFETY: COM is initialised by the caller's `ComScope`.
    let policy: INetFwPolicy2 =
        unsafe { CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)? };
    // SAFETY: plain COM call on a live interface.
    Ok(Rules(unsafe { policy.Rules()? }))
}

impl Rules {
    fn add_block(&self, path: &str, direction: NET_FW_RULE_DIRECTION) -> Result<()> {
        // SAFETY: COM is initialised; every setter takes owned values that
        // live across the call.
        unsafe {
            let rule: INetFwRule = CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER)?;
            rule.SetName(&BSTR::from(rule_name(path, direction).as_str()))?;
            rule.SetDescription(&BSTR::from(
                "Network access blocked from RustWTM. Unblock it there, or delete this rule.",
            ))?;
            rule.SetApplicationName(&BSTR::from(path))?;
            rule.SetDirection(direction)?;
            rule.SetAction(NET_FW_ACTION_BLOCK)?;
            rule.SetProfiles(NET_FW_PROFILE2_ALL.0)?;
            rule.SetGrouping(&BSTR::from(GROUP))?;
            rule.SetEnabled(VARIANT_TRUE)?;
            self.0.Add(&rule)?;
        }
        Ok(())
    }
}

/// Every rule in [`GROUP`]. Rules that fail to read are skipped.
fn group_rules(rules: &Rules) -> Result<Vec<GroupRule>> {
    // SAFETY: plain COM calls on live interfaces.
    let items: IEnumVARIANT = unsafe { rules.0._NewEnum()? }.cast()?;
    let mut found = Vec::new();
    loop {
        let mut slot = [VARIANT::default()];
        let mut fetched = 0u32;
        // SAFETY: one writable slot and a writable count.
        let hr = unsafe { items.Next(&mut slot, &mut fetched) };
        if hr.is_err() || fetched == 0 {
            break;
        }
        let Ok(dispatch) = IDispatch::try_from(&slot[0]) else {
            continue;
        };
        let Ok(rule) = dispatch.cast::<INetFwRule>() else {
            continue;
        };
        if let Some(entry) = read_group_rule(&rule) {
            found.push(entry);
        }
    }
    Ok(found)
}

fn read_group_rule(rule: &INetFwRule) -> Option<GroupRule> {
    // SAFETY: plain COM getters on a live interface; the BSTRs are owned.
    unsafe {
        if !rule
            .Grouping()
            .ok()?
            .to_string()
            .eq_ignore_ascii_case(GROUP)
        {
            return None;
        }
        Some(GroupRule {
            name: rule.Name().ok()?.to_string(),
            program: rule.ApplicationName().ok()?.to_string(),
            direction: rule.Direction().ok()?,
        })
    }
}

/// Windows paths are case-insensitive, and the firewall may hand a path back
/// with different casing than it was given.
fn same_program(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// A rule name that says whose it is, which way it blocks and for what, so
/// it reads well in Windows' firewall console.
fn rule_name(path: &str, direction: NET_FW_RULE_DIRECTION) -> String {
    let way = if direction == NET_FW_RULE_DIR_IN {
        "inbound"
    } else {
        "outbound"
    };
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    format!("{GROUP}: block {way} for {file} ({path})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_names_identify_owner_direction_and_program() {
        let path = r"C:\Tools\app.exe";
        let inbound = rule_name(path, NET_FW_RULE_DIR_IN);
        let outbound = rule_name(path, NET_FW_RULE_DIR_OUT);
        assert!(inbound.starts_with(GROUP));
        assert!(inbound.contains("inbound") && inbound.contains("app.exe"));
        assert!(inbound.contains(path));
        assert!(outbound.contains("outbound"));
        assert_ne!(inbound, outbound);
        assert_ne!(inbound, rule_name(r"C:\Other\app.exe", NET_FW_RULE_DIR_IN));
    }

    #[test]
    fn program_paths_compare_without_case() {
        assert!(same_program(r"C:\A\B.exe", r"c:\a\b.EXE"));
        assert!(!same_program(r"C:\A\B.exe", r"C:\A\C.exe"));
    }

    #[test]
    fn listing_works_or_is_denied() {
        // Reading rules usually works unelevated; either way it must not panic.
        let _ = blocked();
    }

    #[test]
    fn block_round_trip_when_elevated() {
        if !crate::shell::is_elevated().unwrap_or(false) {
            eprintln!("skipped: needs administrator rights");
            return;
        }
        let path = format!(r"C:\RustWTM-test-{}\does-not-exist.exe", std::process::id());
        block(&path).unwrap();
        block(&path).unwrap(); // idempotent
        let listed = blocked().unwrap();
        assert_eq!(listed.iter().filter(|p| same_program(p, &path)).count(), 1);
        unblock(&path).unwrap();
        assert!(!blocked().unwrap().iter().any(|p| same_program(p, &path)));
    }
}

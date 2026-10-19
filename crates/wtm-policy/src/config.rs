use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::action::{Action, PriorityLevel};
use crate::insights::InsightsConfig;
use crate::limits::AppLimit;
use crate::rules::Rule;

/// Written to disk on first start; parses to `Config::default()`.
pub const DEFAULT_CONFIG_TOML: &str = include_str!("default_config.toml");

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Log decisions without touching any process.
    pub dry_run: bool,
    pub sample_interval_ms: u64,
    pub governor: GovernorConfig,
    /// Days of per-minute usage history kept on disk.
    pub history_days: u32,
    pub insights: InsightsConfig,
    /// Image names (`xmrig.exe`) that are prevented from starting at all.
    pub block: Vec<String>,
    #[serde(rename = "rule")]
    pub rules: Vec<Rule>,
    /// Per-application CPU / memory budgets.
    #[serde(rename = "limit")]
    pub limits: Vec<AppLimit>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dry_run: true,
            sample_interval_ms: 1000,
            governor: GovernorConfig::default(),
            history_days: 30,
            insights: InsightsConfig::default(),
            block: Vec::new(),
            rules: Vec::new(),
            limits: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GovernorConfig {
    pub enabled: bool,
    /// System CPU at or above this level counts as overloaded.
    pub high_cpu_percent: f32,
    /// How long the overload must last before the governor acts.
    pub sustain_secs: u64,
    /// System CPU at or below this level counts as recovered.
    pub recover_cpu_percent: f32,
    /// How long recovery must last before limits are rolled back.
    pub recover_secs: u64,
    /// Ignore processes using less than this share of the machine.
    pub offender_min_cpu_percent: f32,
    /// Act on at most this many top consumers per tick.
    pub max_offenders: usize,
    /// Minimum time between two escalation steps for the same process.
    pub step_cooldown_secs: u64,
    /// Escalation steps, applied one at a time while overload persists.
    pub ladder: Vec<Action>,
    /// Image-name wildcards the governor never touches.
    pub exempt: Vec<String>,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            high_cpu_percent: 90.0,
            sustain_secs: 5,
            recover_cpu_percent: 70.0,
            recover_secs: 15,
            offender_min_cpu_percent: 2.0,
            max_offenders: 3,
            step_cooldown_secs: 5,
            ladder: vec![
                Action::Priority(PriorityLevel::BelowNormal),
                Action::EcoQos,
                Action::CpuCapRelative(0.5),
                Action::CpuCapRelative(0.25),
            ],
            exempt: vec!["explorer.exe".into(), "Taskmgr.exe".into()],
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid config: {0}")]
    Invalid(String),
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(text)?;
        config.validate().map_err(ConfigError::Invalid)?;
        Ok(config)
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_owned(),
            source,
        })?;
        Self::from_toml(&text)
    }

    fn validate(&self) -> Result<(), String> {
        if self.sample_interval_ms < 100 {
            return Err("sample_interval_ms must be at least 100".into());
        }
        let g = &self.governor;
        for (name, v) in [
            ("high_cpu_percent", g.high_cpu_percent),
            ("recover_cpu_percent", g.recover_cpu_percent),
            ("offender_min_cpu_percent", g.offender_min_cpu_percent),
        ] {
            if !(0.0..=100.0).contains(&v) {
                return Err(format!("governor.{name} must be within 0..=100"));
            }
        }
        if g.recover_cpu_percent >= g.high_cpu_percent {
            return Err("governor.recover_cpu_percent must be below high_cpu_percent".into());
        }
        for a in &g.ladder {
            a.validate().map_err(|e| format!("governor.ladder: {e}"))?;
        }
        self.insights
            .validate()
            .map_err(|e| format!("insights: {e}"))?;
        if self.history_days == 0 {
            return Err("history_days must be positive".into());
        }
        for name in &self.block {
            validate_block(name).map_err(|e| format!("block '{name}': {e}"))?;
        }
        for l in &self.limits {
            l.validate()
                .map_err(|e| format!("limit '{}': {e}", l.name))?;
        }
        for r in &self.rules {
            r.validate()
                .map_err(|e| format!("rule '{}': {e}", r.name))?;
        }
        Ok(())
    }
}

/// Windows matches blocked programs by file name only, without wildcards.
fn validate_block(name: &str) -> Result<(), String> {
    if name.contains(['\\', '/', '*', '?', '"']) || name.trim() != name {
        return Err("must be a plain file name like pp.exe (no path or wildcards)".into());
    }
    if !name.to_ascii_lowercase().ends_with(".exe") || name.len() <= 4 {
        return Err("must end with .exe".into());
    }
    const NEVER: [&str; 4] = ["explorer.exe", "rustwtm.exe", "wtm.exe", "wtm-service.exe"];
    if NEVER.iter().any(|n| n.eq_ignore_ascii_case(name))
        || wtm_core::protection_reason(u32::MAX, name).is_some()
    {
        return Err("this program cannot be blocked".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_names_must_be_plain_unprotected_executables() {
        let config = |name: &str| Config {
            block: vec![name.into()],
            ..Config::default()
        };
        assert!(config("xmrig.exe").validate().is_ok());
        assert!(config("Game.EXE").validate().is_ok());
        for bad in [
            "C:\\x.exe",
            "x*.exe",
            "notes.txt",
            ".exe",
            " x.exe",
            "csrss.exe",
            "explorer.exe",
            "WTM.exe",
        ] {
            let e = config(bad).validate().unwrap_err();
            assert!(e.starts_with(&format!("block '{bad}'")), "{e}");
        }
    }

    #[test]
    fn default_file_matches_default_struct() {
        assert_eq!(
            Config::from_toml(DEFAULT_CONFIG_TOML).unwrap(),
            Config::default()
        );
    }

    #[test]
    fn commented_example_rules_are_valid() {
        let start = DEFAULT_CONFIG_TOML.find("# [[rule]]").unwrap();
        let examples: String = DEFAULT_CONFIG_TOML[start..]
            .lines()
            .map(|l| l.strip_prefix("# ").unwrap_or(l.trim_start_matches('#')))
            .collect::<Vec<_>>()
            .join("\n");
        let c = Config::from_toml(&examples).unwrap();
        assert_eq!(c.rules.len(), 3);
        assert_eq!(c.rules[2].when.as_ref().unwrap().gpu_above, Some(80.0));
    }

    #[test]
    fn parses_rules_and_actions() {
        let c = Config::from_toml(
            r#"
            dry_run = false
            [governor]
            ladder = ["eco_qos", { cpu_cap = 10 }, "suspend"]

            [[rule]]
            name = "cap renderers"
            match = { name = "chrome.exe" }
            when = { cpu_above = 40, for_secs = 10 }
            actions = [{ priority = "idle" }, { cpu_cap_relative = 0.5 }]

            [[rule]]
            name = "no miners"
            match = { path = "*\\xmrig*.exe" }
            actions = ["kill"]
            "#,
        )
        .unwrap();
        assert!(!c.dry_run);
        assert_eq!(
            c.governor.ladder,
            [Action::EcoQos, Action::CpuCap(10.0), Action::Suspend]
        );
        assert_eq!(c.rules.len(), 2);
        assert_eq!(
            c.rules[0].actions,
            [
                Action::Priority(PriorityLevel::Idle),
                Action::CpuCapRelative(0.5)
            ]
        );
        assert!(c.rules[1].when.is_none());
    }

    #[test]
    fn rejects_invalid_values() {
        for bad in [
            "[governor]\nhigh_cpu_percent = 50\nrecover_cpu_percent = 60",
            "[governor]\nladder = [{ cpu_cap = 0 }]",
            "[governor]\nladder = [{ cpu_cap_relative = 2 }]",
            "unknown_key = 1",
            "[[rule]]\nname = \"x\"\nmatch = {}\nactions = [\"kill\"]",
            "[[rule]]\nname = \"x\"\nmatch = { name = \"a\" }\nactions = []",
            "sample_interval_ms = 99",
            "[governor]\nhigh_cpu_percent = 101",
            "history_days = 0",
            "[insights]\nwindow_hours = 0",
        ] {
            assert!(Config::from_toml(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn load_reads_file_and_reports_missing_path() {
        let path = std::env::temp_dir().join(format!("wtm-policy-cfg-{}.toml", std::process::id()));
        std::fs::write(&path, "dry_run = false\n").unwrap();
        let loaded = Config::load(&path);
        std::fs::remove_file(&path).unwrap();
        assert!(!loaded.unwrap().dry_run);

        // The file is gone now.
        let err = Config::load(&path).unwrap_err();
        assert!(matches!(err, ConfigError::Io { .. }));
        assert!(err.to_string().starts_with("cannot read "), "{err}");
    }
}

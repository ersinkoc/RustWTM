//! One-click protection levels for the CPU governor.

use crate::action::{Action, PriorityLevel};
use crate::config::GovernorConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// The governor does nothing.
    Off,
    /// Acts only on long, heavy overloads, and only softly.
    Gentle,
    /// The default: keeps the machine responsive without hard caps first.
    Balanced,
    /// Acts early and caps hard.
    Strict,
}

impl Preset {
    pub const ALL: [Preset; 4] = [
        Preset::Off,
        Preset::Gentle,
        Preset::Balanced,
        Preset::Strict,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Off => "Off",
            Preset::Gentle => "Gentle",
            Preset::Balanced => "Balanced",
            Preset::Strict => "Strict",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Preset::Off => "No automatic CPU limiting.",
            Preset::Gentle => {
                "Above 95% CPU for 10 s: lower the priority and use efficiency mode for the top 2 apps."
            }
            Preset::Balanced => {
                "Above 90% CPU for 5 s: lower priority, efficiency mode, then cap the top 3 apps to half, then a quarter."
            }
            Preset::Strict => {
                "Above 80% CPU for 3 s: idle priority, efficiency mode, then caps down to a tenth for the top 5 apps."
            }
        }
    }

    /// The governor settings of this level; other fields keep `base`.
    pub fn apply(self, base: &GovernorConfig) -> GovernorConfig {
        let rel = Action::CpuCapRelative;
        let (high, sustain, recover, offenders, ladder) = match self {
            Preset::Off => {
                return GovernorConfig {
                    enabled: false,
                    ..base.clone()
                };
            }
            Preset::Gentle => (
                95.0,
                10,
                80.0,
                2,
                vec![Action::Priority(PriorityLevel::BelowNormal), Action::EcoQos],
            ),
            Preset::Balanced => {
                let d = GovernorConfig::default();
                (
                    d.high_cpu_percent,
                    d.sustain_secs,
                    d.recover_cpu_percent,
                    d.max_offenders,
                    d.ladder,
                )
            }
            Preset::Strict => (
                80.0,
                3,
                60.0,
                5,
                vec![
                    Action::Priority(PriorityLevel::Idle),
                    Action::EcoQos,
                    rel(0.5),
                    rel(0.25),
                    rel(0.1),
                ],
            ),
        };
        GovernorConfig {
            enabled: true,
            high_cpu_percent: high,
            sustain_secs: sustain,
            recover_cpu_percent: recover,
            max_offenders: offenders,
            ladder,
            ..base.clone()
        }
    }

    /// The level `cfg` matches, if any (`None` for hand-tuned settings).
    pub fn of(cfg: &GovernorConfig) -> Option<Preset> {
        if !cfg.enabled {
            return Some(Preset::Off);
        }
        Preset::ALL
            .into_iter()
            .skip(1)
            .find(|p| p.apply(cfg) == *cfg)
    }
}

fn ladder_item(a: &Action) -> toml_edit::Value {
    let mut t = toml_edit::InlineTable::new();
    match *a {
        Action::EcoQos => return "eco_qos".into(),
        Action::Suspend => return "suspend".into(),
        Action::Kill => return "kill".into(),
        Action::TrimMemory => return "trim_memory".into(),
        Action::Priority(p) => {
            let name = match p {
                PriorityLevel::Idle => "idle",
                PriorityLevel::BelowNormal => "below_normal",
                PriorityLevel::Normal => "normal",
                PriorityLevel::AboveNormal => "above_normal",
            };
            t.insert("priority", name.into());
        }
        Action::CpuCap(c) => {
            t.insert("cpu_cap", f64::from(c).into());
        }
        Action::CpuCapRelative(r) => {
            t.insert("cpu_cap_relative", f64::from(r).into());
        }
        Action::MemoryCapMb(m) => {
            t.insert("memory_cap_mb", (m as i64).into());
        }
    }
    t.into()
}

/// Writes the level's governor settings into a configuration text, keeping
/// comments and every other setting.
pub fn set_preset(text: &str, preset: Preset) -> Result<String, String> {
    let config = crate::Config::from_toml(text).map_err(|e| e.to_string())?;
    let g = preset.apply(&config.governor);
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    let table = doc
        .entry("governor")
        .or_insert_with(toml_edit::table)
        .as_table_mut()
        .ok_or("`governor` must be a table")?;
    table["enabled"] = toml_edit::value(g.enabled);
    if preset != Preset::Off {
        table["high_cpu_percent"] = toml_edit::value(f64::from(g.high_cpu_percent));
        table["sustain_secs"] = toml_edit::value(g.sustain_secs as i64);
        table["recover_cpu_percent"] = toml_edit::value(f64::from(g.recover_cpu_percent));
        table["max_offenders"] = toml_edit::value(g.max_offenders as i64);
        let ladder: toml_edit::Array = g.ladder.iter().map(ladder_item).collect();
        table["ladder"] = toml_edit::value(ladder);
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, DEFAULT_CONFIG_TOML};

    #[test]
    fn presets_round_trip_through_the_config_text() {
        // The default configuration is Balanced.
        let default = Config::from_toml(DEFAULT_CONFIG_TOML).unwrap();
        assert_eq!(Preset::of(&default.governor), Some(Preset::Balanced));
        for p in Preset::ALL {
            let text = set_preset(DEFAULT_CONFIG_TOML, p).unwrap();
            let c = Config::from_toml(&text).unwrap();
            assert_eq!(Preset::of(&c.governor), Some(p), "{p:?}");
            assert!(text.contains("# RustWTM configuration"), "comments kept");
            assert!(!p.label().is_empty() && !p.description().is_empty());
        }
        // Off keeps the tuned values, so switching back restores the level.
        let off = set_preset(
            &set_preset(DEFAULT_CONFIG_TOML, Preset::Strict).unwrap(),
            Preset::Off,
        )
        .unwrap();
        let c = Config::from_toml(&off).unwrap();
        assert!(!c.governor.enabled);
        assert_eq!(c.governor.high_cpu_percent, 80.0);

        // Hand-tuned settings match no level.
        let custom = GovernorConfig {
            high_cpu_percent: 85.0,
            ..GovernorConfig::default()
        };
        assert_eq!(Preset::of(&custom), None);
        // A config without a [governor] table gets one.
        let c =
            Config::from_toml(&set_preset("dry_run = true\n", Preset::Gentle).unwrap()).unwrap();
        assert_eq!(c.governor.max_offenders, 2);
    }

    #[test]
    fn every_ladder_action_is_written_back() {
        for a in [
            Action::Priority(PriorityLevel::Normal),
            Action::Priority(PriorityLevel::AboveNormal),
            Action::CpuCap(5.0),
            Action::MemoryCapMb(512),
            Action::Suspend,
            Action::Kill,
            Action::TrimMemory,
        ] {
            let text = format!("[governor]\nladder = [{}]\n", ladder_item(&a));
            let c = Config::from_toml(&text).unwrap();
            assert_eq!(c.governor.ladder, [a]);
        }
        assert!(set_preset("x = [", Preset::Off).is_err());
        assert!(set_preset("governor = 1", Preset::Off).is_err());
    }
}

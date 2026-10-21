//! Resources kept for RustWTM itself, so it stays usable on a machine that is
//! pinned at 100% CPU or out of RAM: a working-set floor for the service, and
//! a rescue core that every other program is moved off while overloaded.

use std::time::{Duration, Instant};

use serde::Deserialize;
use wtm_core::protection_reason;

use crate::glob_match;

/// RustWTM's own programs, never moved off the rescue core.
const OWN_IMAGES: [&str; 3] = ["rustwtm.exe", "wtm.exe", "wtm-service.exe"];

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReserveConfig {
    /// Keep one core free for RustWTM and Windows while overloaded.
    pub rescue_core: bool,
    /// System CPU at or above this level...
    pub trigger_cpu_percent: f32,
    /// ...for this long engages the rescue core.
    pub sustain_secs: u64,
    /// System CPU at or below this level...
    pub release_cpu_percent: f32,
    /// ...for this long hands the core back.
    pub release_secs: u64,
    /// Memory (MB) of the service that Windows may never page out; 0 = none.
    pub working_set_mb: u64,
    /// Image-name wildcards that keep every core.
    pub exempt: Vec<String>,
}

impl Default for ReserveConfig {
    fn default() -> Self {
        Self {
            rescue_core: true,
            trigger_cpu_percent: 95.0,
            sustain_secs: 3,
            release_cpu_percent: 75.0,
            release_secs: 10,
            working_set_mb: 48,
            exempt: Vec::new(),
        }
    }
}

impl ReserveConfig {
    pub(crate) fn validate(&self) -> Result<(), String> {
        for (name, v) in [
            ("trigger_cpu_percent", self.trigger_cpu_percent),
            ("release_cpu_percent", self.release_cpu_percent),
        ] {
            if !(1.0..=100.0).contains(&v) {
                return Err(format!("{name} must be within 1..=100"));
            }
        }
        if self.release_cpu_percent >= self.trigger_cpu_percent {
            return Err("release_cpu_percent must be below trigger_cpu_percent".into());
        }
        if self.working_set_mb > 1024 {
            return Err("working_set_mb must be at most 1024".into());
        }
        Ok(())
    }

    /// Whether a process is moved off the rescue core while it is engaged.
    pub fn confines(&self, pid: u32, name: &str) -> bool {
        protection_reason(pid, name).is_none()
            && !OWN_IMAGES.iter().any(|n| n.eq_ignore_ascii_case(name))
            && !self.exempt.iter().any(|g| glob_match(g, name))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RescueStep {
    Engage,
    Release,
}

/// Decides when the rescue core is engaged, with hysteresis so it does not
/// flap around the thresholds.
#[derive(Debug, Default)]
pub struct RescueCore {
    cfg: ReserveConfig,
    engaged: bool,
    since: Option<Instant>,
}

impl RescueCore {
    pub fn new(cfg: ReserveConfig) -> Self {
        Self {
            cfg,
            ..Self::default()
        }
    }

    pub fn config(&self) -> &ReserveConfig {
        &self.cfg
    }

    /// Takes effect on the next tick; turning the rescue core off releases it.
    pub fn set_config(&mut self, cfg: ReserveConfig) {
        self.cfg = cfg;
    }

    pub fn engaged(&self) -> bool {
        self.engaged
    }

    pub fn tick(&mut self, now: Instant, cpu_percent: f32) -> Option<RescueStep> {
        if !self.cfg.rescue_core {
            self.since = None;
            return std::mem::take(&mut self.engaged).then_some(RescueStep::Release);
        }
        let (crossing, hold) = if self.engaged {
            (
                cpu_percent <= self.cfg.release_cpu_percent,
                self.cfg.release_secs,
            )
        } else {
            (
                cpu_percent >= self.cfg.trigger_cpu_percent,
                self.cfg.sustain_secs,
            )
        };
        if !crossing {
            self.since = None;
            return None;
        }
        let since = *self.since.get_or_insert(now);
        if now.saturating_duration_since(since) < Duration::from_secs(hold) {
            return None;
        }
        self.since = None;
        self.engaged = !self.engaged;
        Some(if self.engaged {
            RescueStep::Engage
        } else {
            RescueStep::Release
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(start: Instant, s: u64) -> Instant {
        start + Duration::from_secs(s)
    }

    #[test]
    fn engages_after_sustained_overload_and_releases_after_recovery() {
        let t = Instant::now();
        let mut r = RescueCore::new(ReserveConfig::default());
        assert_eq!(r.tick(t, 99.0), None);
        assert_eq!(r.tick(secs(t, 2), 99.0), None);
        // A dip resets the clock.
        assert_eq!(r.tick(secs(t, 3), 90.0), None);
        assert_eq!(r.tick(secs(t, 4), 97.0), None);
        assert_eq!(r.tick(secs(t, 7), 97.0), Some(RescueStep::Engage));
        assert!(r.engaged());
        // Between the thresholds nothing changes.
        assert_eq!(r.tick(secs(t, 30), 85.0), None);
        assert_eq!(r.tick(secs(t, 31), 50.0), None);
        assert_eq!(r.tick(secs(t, 40), 50.0), None);
        assert_eq!(r.tick(secs(t, 41), 50.0), Some(RescueStep::Release));
        assert!(!r.engaged());
    }

    #[test]
    fn turning_it_off_releases_at_once() {
        let t = Instant::now();
        let mut r = RescueCore::new(ReserveConfig {
            sustain_secs: 0,
            ..ReserveConfig::default()
        });
        assert_eq!(r.tick(t, 100.0), Some(RescueStep::Engage));
        r.set_config(ReserveConfig {
            rescue_core: false,
            ..ReserveConfig::default()
        });
        assert_eq!(r.tick(t, 100.0), Some(RescueStep::Release));
        assert_eq!(r.tick(t, 100.0), None);
    }

    #[test]
    fn confines_ordinary_programs_only() {
        let cfg = ReserveConfig {
            exempt: vec!["obs*.exe".into()],
            ..ReserveConfig::default()
        };
        assert!(cfg.confines(9000, "chrome.exe"));
        assert!(!cfg.confines(9000, "obs64.exe"));
        assert!(!cfg.confines(9000, "RustWTM.exe"));
        assert!(!cfg.confines(9000, "csrss.exe"));
        assert!(!cfg.confines(4, "System"));
    }

    #[test]
    fn validation_rejects_inverted_thresholds() {
        let bad = ReserveConfig {
            release_cpu_percent: 96.0,
            ..ReserveConfig::default()
        };
        assert!(bad.validate().is_err());
        let huge = ReserveConfig {
            working_set_mb: 4096,
            ..ReserveConfig::default()
        };
        assert!(huge.validate().is_err());
        assert!(ReserveConfig::default().validate().is_ok());
    }
}

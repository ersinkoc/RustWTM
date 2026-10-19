use std::fmt;

use serde::{Deserialize, Serialize};
use wtm_core::control::{PriorityClass, ProcessRef};

/// Ordered from most to least restrictive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityLevel {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
}

impl From<PriorityLevel> for PriorityClass {
    fn from(p: PriorityLevel) -> Self {
        match p {
            PriorityLevel::Idle => PriorityClass::Idle,
            PriorityLevel::BelowNormal => PriorityClass::BelowNormal,
            PriorityLevel::Normal => PriorityClass::Normal,
            PriorityLevel::AboveNormal => PriorityClass::AboveNormal,
        }
    }
}

/// One enforcement step. In TOML: `"eco_qos"`, `{ cpu_cap = 25 }`,
/// `{ priority = "idle" }`, ...
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Priority(PriorityLevel),
    EcoQos,
    /// Hard cap as a share of total machine CPU (0 < x <= 100).
    CpuCap(f32),
    /// Hard cap as a fraction (0 < x <= 1) of the CPU usage observed when
    /// the process was first acted on. Resolved to `CpuCap` before execution.
    CpuCapRelative(f32),
    MemoryCapMb(u64),
    Suspend,
    Kill,
}

impl Action {
    /// Turns relative caps into absolute ones using `baseline_cpu`.
    pub(crate) fn resolve(self, baseline_cpu: f32) -> Self {
        match self {
            Action::CpuCapRelative(f) => Action::CpuCap((baseline_cpu * f).clamp(0.01, 100.0)),
            other => other,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        match *self {
            Action::CpuCap(p) if !(p > 0.0 && p <= 100.0) => {
                Err(format!("cpu_cap must be in (0, 100], got {p}"))
            }
            Action::CpuCapRelative(f) if !(f > 0.0 && f <= 1.0) => {
                Err(format!("cpu_cap_relative must be in (0, 1], got {f}"))
            }
            Action::MemoryCapMb(0) => Err("memory_cap_mb must be positive".into()),
            _ => Ok(()),
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Priority(p) => write!(f, "priority {p:?}"),
            Action::EcoQos => f.write_str("efficiency mode"),
            Action::CpuCap(p) => write!(f, "CPU cap {p:.2}%"),
            Action::CpuCapRelative(r) => write!(f, "CPU cap {:.0}% of baseline", r * 100.0),
            Action::MemoryCapMb(mb) => write!(f, "memory cap {mb} MiB"),
            Action::Suspend => f.write_str("suspend"),
            Action::Kill => f.write_str("kill"),
        }
    }
}

/// Who asked for an action. Each source's actions are tracked separately so
/// one source rolling back never undoes another source's limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    Governor,
    Rule,
    /// Long-term history analysis.
    Insight,
    Manual,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Apply an (already resolved) action to a process.
    Apply {
        source: Source,
        target: ProcessRef,
        action: Action,
        reason: String,
    },
    /// Undo everything this source applied to a process.
    Restore {
        source: Source,
        target: ProcessRef,
        reason: String,
    },
}

impl Decision {
    pub fn target(&self) -> &ProcessRef {
        match self {
            Decision::Apply { target, .. } | Decision::Restore { target, .. } => target,
        }
    }
}

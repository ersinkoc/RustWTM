//! Configuration, rule engine and CPU governor for RustWTM.
//!
//! Everything here is pure decision logic: it consumes snapshots and emits
//! [`Decision`]s. Applying them (or only logging them in dry-run mode) is the
//! caller's job, which keeps this crate fully unit-testable.

#![forbid(unsafe_code)]

mod action;
mod config;
mod cooling;
mod glob;
mod governor;
mod insights;
mod kill;
mod limits;
mod memory_guard;
mod presets;
mod rules;
mod swarm;
mod telegram;

pub use action::{Action, Decision, PriorityLevel, Source};
pub use config::{Config, ConfigError, DEFAULT_CONFIG_TOML, GovernorConfig, set_config_value};
pub use cooling::{CoolingMode, CoolingProfile, CurvePoint, smooth_duty};
pub use glob::glob_match;
pub use governor::Governor;
pub use insights::{
    AppStats, Finding, FindingKind, InsightPolicy, InsightsConfig, MinuteRow, analyze, summarize,
};
pub use kill::{
    KillEntry, KillWatch, PanicTarget, panic_targets, remove_kill_entry, set_kill_entry,
    text_matches,
};
pub use limits::{
    AppLimit, GroupId, GroupUsage, LimitPlanner, LimitScope, remove_app_limit, set_app_limit,
};
pub use memory_guard::{MemoryGuard, MemoryGuardConfig};
pub use presets::{Preset, set_preset};
pub use rules::{Condition, Detail, Matcher, Rule, RuleEngine};
pub use swarm::{Swarm, SwarmConfig, SwarmWatch, find_swarms, live_parent};
pub use telegram::{TELEGRAM_SDDL, TelegramConfig, parse_chats, token_looks_valid};

/// Identifies a process instance across PID reuse.
pub(crate) type Key = (u32, i64);

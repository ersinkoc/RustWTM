//! Configuration, rule engine and CPU governor for RustWTM.
//!
//! Everything here is pure decision logic: it consumes snapshots and emits
//! [`Decision`]s. Applying them (or only logging them in dry-run mode) is the
//! caller's job, which keeps this crate fully unit-testable.

#![forbid(unsafe_code)]

mod action;
mod config;
mod glob;
mod governor;
mod insights;
mod limits;
mod rules;

pub use action::{Action, Decision, PriorityLevel, Source};
pub use config::{Config, ConfigError, DEFAULT_CONFIG_TOML, GovernorConfig};
pub use glob::glob_match;
pub use governor::Governor;
pub use insights::{
    AppStats, Finding, FindingKind, InsightPolicy, InsightsConfig, MinuteRow, analyze, summarize,
};
pub use limits::{
    AppLimit, GroupId, GroupUsage, LimitPlanner, LimitScope, remove_app_limit, set_app_limit,
};
pub use rules::{Condition, Matcher, Rule, RuleEngine};

/// Identifies a process instance across PID reuse.
pub(crate) type Key = (u32, i64);

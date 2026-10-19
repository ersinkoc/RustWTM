//! Long-term analysis: summarizes per-minute history per application and
//! turns sustained patterns into decisions.
//!
//! Two findings are detected over a sliding window:
//! - **Chronic CPU**: an app whose average CPU over the window stays high.
//! - **Memory growth**: an app whose memory keeps climbing (leak suspect).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use wtm_core::control::ProcessRef;
use wtm_core::{ProcessInfo, protection_reason};

use crate::{Action, Decision, Key, Source, glob_match};

/// One minute of aggregated usage for one application (all its processes).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MinuteRow {
    /// Unix time in minutes.
    pub minute: i64,
    /// Average total CPU (share of the machine) during the minute.
    pub cpu_avg: f32,
    pub cpu_peak: f32,
    /// Average total private working set.
    pub mem_avg: u64,
    pub mem_peak: u64,
    pub processes: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppStats {
    pub app: String,
    /// Minutes in the window the app was running.
    pub minutes: u32,
    pub cpu_avg: f32,
    /// 95th percentile of per-minute CPU.
    pub cpu_p95: f32,
    pub cpu_peak: f32,
    pub mem_avg: u64,
    pub mem_peak: u64,
    /// Memory at the end of the window.
    pub mem_last: u64,
    /// Least-squares slope of memory, in bytes per hour.
    pub mem_growth_per_hour: f64,
    /// Fit quality of the memory trend (R², 0..=1): near 1 means steady growth,
    /// near 0 means noise that a slope alone would misread as a trend.
    pub mem_growth_fit: f64,
    /// CPU-time used in the window, in core-seconds.
    pub cpu_core_seconds: f64,
}

/// Summarizes `rows` (any order). `logical_cpus` converts shares to core-seconds.
pub fn summarize(app: &str, rows: &[MinuteRow], logical_cpus: u32) -> Option<AppStats> {
    if rows.is_empty() {
        return None;
    }
    let mut rows = rows.to_vec();
    rows.sort_by_key(|r| r.minute);
    let n = rows.len() as f64;

    let mut cpu: Vec<f32> = rows.iter().map(|r| r.cpu_avg).collect();
    cpu.sort_by(f32::total_cmp);
    let p95_index = ((cpu.len() as f64 * 0.95).ceil() as usize).clamp(1, cpu.len()) - 1;

    // Linear regression of memory against time (hours).
    let t0 = rows[0].minute;
    let xs: Vec<f64> = rows.iter().map(|r| (r.minute - t0) as f64 / 60.0).collect();
    let ys: Vec<f64> = rows.iter().map(|r| r.mem_avg as f64).collect();
    let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
    let var: f64 = xs.iter().map(|x| (x - mx).powi(2)).sum();
    let cov: f64 = xs.iter().zip(&ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let slope = if var > 0.0 { cov / var } else { 0.0 };
    let var_y: f64 = ys.iter().map(|y| (y - my).powi(2)).sum();
    let fit = if var > 0.0 && var_y > 0.0 {
        (cov * cov / (var * var_y)).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let cpu_sum: f64 = rows.iter().map(|r| f64::from(r.cpu_avg)).sum();
    Some(AppStats {
        app: app.to_owned(),
        minutes: rows.len() as u32,
        cpu_avg: (cpu_sum / n) as f32,
        cpu_p95: cpu[p95_index],
        cpu_peak: rows.iter().map(|r| r.cpu_peak).fold(0.0, f32::max),
        mem_avg: (ys.iter().sum::<f64>() / n) as u64,
        mem_peak: rows.iter().map(|r| r.mem_peak).max().unwrap_or(0),
        mem_last: rows.last().map_or(0, |r| r.mem_avg),
        mem_growth_per_hour: slope,
        mem_growth_fit: fit,
        // Each row is one minute: share% / 100 * cpus * 60 s.
        cpu_core_seconds: cpu_sum / 100.0 * f64::from(logical_cpus) * 60.0,
    })
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InsightsConfig {
    pub enabled: bool,
    /// Window the analysis looks back over.
    pub window_hours: u32,
    /// Re-run the analysis this often.
    pub evaluate_every_minutes: u32,
    /// Ignore apps observed for fewer minutes than this in the window.
    pub min_minutes: u32,
    /// Average CPU (share of the machine) that marks an app as a chronic hog.
    pub chronic_cpu_percent: f32,
    pub chronic_actions: Vec<Action>,
    /// Memory growth that marks a leak suspect...
    pub growth_mb_per_hour: f64,
    /// ...once the app uses at least this much...
    pub growth_min_mb: u64,
    /// ...and the trend is steady (R² of the linear fit, 0..=1).
    pub growth_min_fit: f64,
    /// Empty: only report (event + notification).
    pub growth_actions: Vec<Action>,
    /// Image-name wildcards never acted on.
    pub exempt: Vec<String>,
}

impl Default for InsightsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            window_hours: 24,
            evaluate_every_minutes: 10,
            min_minutes: 60,
            chronic_cpu_percent: 10.0,
            chronic_actions: vec![
                Action::Priority(crate::PriorityLevel::BelowNormal),
                Action::EcoQos,
            ],
            growth_mb_per_hour: 200.0,
            growth_min_mb: 2048,
            growth_min_fit: 0.7,
            growth_actions: Vec::new(),
            exempt: vec!["explorer.exe".into(), "Taskmgr.exe".into()],
        }
    }
}

impl InsightsConfig {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.window_hours == 0 || self.evaluate_every_minutes == 0 {
            return Err("window_hours and evaluate_every_minutes must be positive".into());
        }
        if !(0.0..=100.0).contains(&self.chronic_cpu_percent) {
            return Err("chronic_cpu_percent must be within 0..=100".into());
        }
        for a in self.chronic_actions.iter().chain(&self.growth_actions) {
            if matches!(a, Action::CpuCapRelative(_)) {
                return Err("cpu_cap_relative is not supported for insights; use cpu_cap".into());
            }
            a.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FindingKind {
    ChronicCpu,
    MemoryGrowth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub app: String,
    pub kind: FindingKind,
    pub detail: String,
}

pub fn analyze(stats: &[AppStats], cfg: &InsightsConfig) -> Vec<Finding> {
    let mib = |b: f64| b / (1024.0 * 1024.0);
    let mut out = Vec::new();
    for s in stats.iter().filter(|s| s.minutes >= cfg.min_minutes) {
        if cfg.exempt.iter().any(|e| glob_match(e, &s.app)) {
            continue;
        }
        if s.cpu_avg >= cfg.chronic_cpu_percent {
            out.push(Finding {
                app: s.app.clone(),
                kind: FindingKind::ChronicCpu,
                detail: format!(
                    "average CPU {:.1}% (p95 {:.1}%) over {} min",
                    s.cpu_avg, s.cpu_p95, s.minutes
                ),
            });
        }
        let growth = mib(s.mem_growth_per_hour);
        if growth >= cfg.growth_mb_per_hour
            && s.mem_growth_fit >= cfg.growth_min_fit
            && mib(s.mem_last as f64) >= cfg.growth_min_mb as f64
        {
            out.push(Finding {
                app: s.app.clone(),
                kind: FindingKind::MemoryGrowth,
                detail: format!(
                    "memory growing {growth:.0} MiB/h over {} min (fit {:.2}), now {:.0} MiB",
                    s.minutes,
                    s.mem_growth_fit,
                    mib(s.mem_last as f64)
                ),
            });
        }
    }
    out
}

/// Applies the configured actions to every running instance of a flagged
/// app and rolls them back when the finding goes away.
#[derive(Debug, Default)]
pub struct InsightPolicy {
    /// Currently flagged (app lowercase, kind).
    flagged: HashMap<(String, FindingKind), Finding>,
    /// Instances already acted on.
    applied: HashSet<Key>,
    targets: HashMap<Key, ProcessRef>,
}

impl InsightPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn findings(&self) -> Vec<Finding> {
        let mut v: Vec<Finding> = self.flagged.values().cloned().collect();
        v.sort_by(|a, b| a.app.cmp(&b.app));
        v
    }

    /// Replaces the findings. Returns (newly raised findings, restore decisions
    /// for instances whose app is no longer flagged with an actionable finding).
    pub fn update(
        &mut self,
        findings: Vec<Finding>,
        cfg: &InsightsConfig,
    ) -> (Vec<Finding>, Vec<Decision>) {
        let new: HashMap<(String, FindingKind), Finding> = findings
            .into_iter()
            .map(|f| ((f.app.to_lowercase(), f.kind), f))
            .collect();
        let raised: Vec<Finding> = new
            .iter()
            .filter(|(k, _)| !self.flagged.contains_key(*k))
            .map(|(_, f)| f.clone())
            .collect();
        self.flagged = new;

        let mut restores = Vec::new();
        let still: HashSet<String> = self
            .flagged
            .keys()
            .filter(|(_, kind)| !actions_for(*kind, cfg).is_empty())
            .map(|(app, _)| app.clone())
            .collect();
        self.applied.retain(|key| {
            let target = &self.targets[key];
            let keep = still.contains(&target.name.to_lowercase());
            if !keep {
                restores.push(Decision::Restore {
                    source: Source::Insight,
                    target: target.clone(),
                    reason: "long-term finding cleared".into(),
                });
            }
            keep
        });
        self.targets.retain(|k, _| self.applied.contains(k));
        (raised, restores)
    }

    /// Applies actions to instances of flagged apps not yet handled. Call on
    /// every sample so newly started instances are covered too.
    pub fn tick(&mut self, procs: &[ProcessInfo], cfg: &InsightsConfig) -> Vec<Decision> {
        let live: HashSet<Key> = procs.iter().map(|p| (p.pid, p.create_time)).collect();
        self.applied.retain(|k| live.contains(k));
        self.targets.retain(|k, _| live.contains(k));

        let mut out = Vec::new();
        for p in procs {
            let key = (p.pid, p.create_time);
            if self.applied.contains(&key) || protection_reason(p.pid, &p.name).is_some() {
                continue;
            }
            let app = p.name.to_lowercase();
            let mut actions: Vec<(Action, &Finding)> = Vec::new();
            for kind in [FindingKind::ChronicCpu, FindingKind::MemoryGrowth] {
                if let Some(f) = self.flagged.get(&(app.clone(), kind)) {
                    actions.extend(actions_for(kind, cfg).iter().map(|a| (*a, f)));
                }
            }
            if actions.is_empty() {
                continue;
            }
            let target = ProcessRef::from(p);
            for (action, finding) in actions {
                out.push(Decision::Apply {
                    source: Source::Insight,
                    target: target.clone(),
                    action,
                    reason: format!("{:?}: {}", finding.kind, finding.detail),
                });
            }
            self.applied.insert(key);
            self.targets.insert(key, target);
        }
        out
    }
}

fn actions_for(kind: FindingKind, cfg: &InsightsConfig) -> &[Action] {
    match kind {
        FindingKind::ChronicCpu => &cfg.chronic_actions,
        FindingKind::MemoryGrowth => &cfg.growth_actions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn row(minute: i64, cpu: f32, mem_mb: u64) -> MinuteRow {
        MinuteRow {
            minute,
            cpu_avg: cpu,
            cpu_peak: cpu * 2.0,
            mem_avg: mem_mb << 20,
            mem_peak: (mem_mb + 10) << 20,
            processes: 1,
        }
    }

    fn proc(pid: u32, name: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: 1,
            name: name.into(),
            cpu_percent: 0.0,
            cpu_time: Duration::ZERO,
            working_set: 0,
            private_working_set: 0,
            private_bytes: 0,
            thread_count: 1,
            handle_count: 1,
            session_id: 1,
            base_priority: 8,
            gpu_percent: 0.0,
            gpu_memory: 0,
            io_read_rate: 0.0,
            io_write_rate: 0.0,
            create_time: i64::from(pid),
        }
    }

    #[test]
    fn summarize_computes_averages_percentiles_and_growth() {
        // 120 minutes, memory +5 MiB per minute = 300 MiB/h, CPU 1..=120 %/10.
        let rows: Vec<MinuteRow> = (0..120)
            .rev()
            .map(|m| row(1000 + m, (m + 1) as f32 / 10.0, 1000 + 5 * m as u64))
            .collect();
        let s = summarize("a.exe", &rows, 8).unwrap();
        assert_eq!(s.minutes, 120);
        assert!((s.cpu_avg - 6.05).abs() < 1e-3);
        assert!((s.cpu_p95 - 11.4).abs() < 1e-3);
        assert!((s.cpu_peak - 24.0).abs() < 1e-3);
        assert_eq!(s.mem_last, (1000 + 5 * 119) << 20);
        let growth_mib = s.mem_growth_per_hour / (1024.0 * 1024.0);
        assert!((growth_mib - 300.0).abs() < 0.01, "{growth_mib}");
        assert!(s.mem_growth_fit > 0.999, "perfectly linear growth");

        // Noisy memory with a spurious slope has a poor fit.
        let noisy: Vec<MinuteRow> = (0..60)
            .map(|m| row(m, 1.0, if m % 2 == 0 { 1000 } else { 3000 } + m as u64))
            .collect();
        assert!(summarize("n.exe", &noisy, 8).unwrap().mem_growth_fit < 0.1);
        // 6.05% of 8 cores for 120 minutes.
        assert!((s.cpu_core_seconds - 0.0605 * 8.0 * 7200.0).abs() < 1.0);
        assert!(summarize("x", &[], 8).is_none());
    }

    fn stats(app: &str, minutes: u32, cpu: f32, growth_mib: f64, last_mib: u64) -> AppStats {
        AppStats {
            app: app.into(),
            minutes,
            cpu_avg: cpu,
            cpu_p95: cpu,
            cpu_peak: cpu,
            mem_avg: last_mib << 20,
            mem_peak: last_mib << 20,
            mem_last: last_mib << 20,
            mem_growth_per_hour: growth_mib * 1024.0 * 1024.0,
            mem_growth_fit: 0.9,
            cpu_core_seconds: 0.0,
        }
    }

    #[test]
    fn analyze_flags_chronic_cpu_and_growth() {
        let cfg = InsightsConfig::default();
        let found = analyze(
            &[
                stats("hog.exe", 300, 25.0, 0.0, 100),
                stats("leaky.exe", 300, 1.0, 500.0, 4096),
                stats("brief.exe", 10, 90.0, 900.0, 9000), // too few minutes
                stats("small-leak.exe", 300, 1.0, 500.0, 300), // below growth_min_mb
                stats("Taskmgr.exe", 300, 50.0, 0.0, 100), // exempt
            ],
            &cfg,
        );
        let kinds: Vec<(&str, FindingKind)> =
            found.iter().map(|f| (f.app.as_str(), f.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("hog.exe", FindingKind::ChronicCpu),
                ("leaky.exe", FindingKind::MemoryGrowth)
            ]
        );
    }

    #[test]
    fn policy_applies_to_all_instances_and_restores_when_cleared() {
        let cfg = InsightsConfig::default();
        let mut p = InsightPolicy::new();
        let finding = Finding {
            app: "hog.exe".into(),
            kind: FindingKind::ChronicCpu,
            detail: "x".into(),
        };
        let (raised, restores) = p.update(vec![finding.clone()], &cfg);
        assert_eq!(raised.len(), 1);
        assert!(restores.is_empty());

        let procs = [proc(1, "HOG.EXE"), proc(2, "hog.exe"), proc(3, "other.exe")];
        let d = p.tick(&procs, &cfg);
        // Two default chronic actions for each of the two instances.
        assert_eq!(d.len(), 4);
        assert!(p.tick(&procs, &cfg).is_empty(), "applied once per instance");

        // A new instance is covered on the next tick.
        let more = [proc(1, "hog.exe"), proc(2, "hog.exe"), proc(5, "hog.exe")];
        assert_eq!(p.tick(&more, &cfg).len(), 2);

        // Same finding again: not raised twice.
        assert!(p.update(vec![finding], &cfg).0.is_empty());

        // Finding cleared: every live instance is restored.
        let (_, restores) = p.update(vec![], &cfg);
        let mut pids: Vec<u32> = restores.iter().map(|d| d.target().pid).collect();
        pids.sort_unstable();
        assert_eq!(pids, [1, 2, 5]);
    }

    #[test]
    fn report_only_findings_do_not_act() {
        let cfg = InsightsConfig::default();
        let mut p = InsightPolicy::new();
        p.update(
            vec![Finding {
                app: "leaky.exe".into(),
                kind: FindingKind::MemoryGrowth,
                detail: "x".into(),
            }],
            &cfg,
        );
        assert!(p.tick(&[proc(1, "leaky.exe")], &cfg).is_empty());
    }
}

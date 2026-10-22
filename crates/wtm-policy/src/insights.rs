//! Long-term analysis: summarizes per-minute history per application and
//! turns sustained patterns into decisions.
//!
//! Two findings are detected over a sliding window:
//! - **Chronic CPU**: an app whose average CPU over the window stays high.
//! - **Memory growth**: an app whose memory keeps climbing (leak suspect).
//! - **Handle / thread growth**: an app whose handle or thread count keeps
//!   climbing, the other two classic leaks (report only).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use wtm_core::control::ProcessRef;
use wtm_core::{ProcessInfo, protection_reason};

use crate::{Action, Decision, Key, Source, glob_match};

/// One minute of aggregated usage for one application (all its processes).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct MinuteRow {
    /// Unix time in minutes.
    pub minute: i64,
    /// Average total CPU (share of the machine) during the minute.
    pub cpu_avg: f32,
    pub cpu_peak: f32,
    /// Average total GPU (busiest engine, summed over instances, capped at 100).
    #[serde(default)]
    pub gpu_avg: f32,
    /// Average total private working set.
    pub mem_avg: u64,
    pub mem_peak: u64,
    pub processes: u32,
    /// Average total handles of the app's processes.
    #[serde(default)]
    pub handles_avg: u32,
    /// Average total threads of the app's processes.
    #[serde(default)]
    pub threads_avg: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppStats {
    pub app: String,
    /// Minutes in the window the app was running.
    pub minutes: u32,
    pub cpu_avg: f32,
    /// 95th percentile of per-minute CPU.
    pub cpu_p95: f32,
    pub cpu_peak: f32,
    #[serde(default)]
    pub gpu_avg: f32,
    /// 95th percentile of per-minute GPU.
    #[serde(default)]
    pub gpu_p95: f32,
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
    /// Handles at the end of the window, their slope per hour and fit (R²).
    #[serde(default)]
    pub handles_last: u32,
    #[serde(default)]
    pub handle_growth_per_hour: f64,
    #[serde(default)]
    pub handle_growth_fit: f64,
    /// Threads at the end of the window, their slope per hour and fit (R²).
    #[serde(default)]
    pub threads_last: u32,
    #[serde(default)]
    pub thread_growth_per_hour: f64,
    #[serde(default)]
    pub thread_growth_fit: f64,
}

/// Least-squares slope of `ys` against `xs`, and its fit (R², 0..=1).
fn trend(xs: &[f64], ys: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    if n == 0.0 {
        return (0.0, 0.0);
    }
    let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
    let var: f64 = xs.iter().map(|x| (x - mx).powi(2)).sum();
    let cov: f64 = xs.iter().zip(ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let slope = if var > 0.0 { cov / var } else { 0.0 };
    let var_y: f64 = ys.iter().map(|y| (y - my).powi(2)).sum();
    let fit = if var > 0.0 && var_y > 0.0 {
        (cov * cov / (var * var_y)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (slope, fit)
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
    let mut gpu: Vec<f32> = rows.iter().map(|r| r.gpu_avg).collect();
    gpu.sort_by(f32::total_cmp);

    // Linear regression of memory against time (hours).
    let t0 = rows[0].minute;
    let xs: Vec<f64> = rows.iter().map(|r| (r.minute - t0) as f64 / 60.0).collect();
    let ys: Vec<f64> = rows.iter().map(|r| r.mem_avg as f64).collect();
    let (slope, fit) = trend(&xs, &ys);
    let handles: Vec<f64> = rows.iter().map(|r| f64::from(r.handles_avg)).collect();
    let (handle_slope, handle_fit) = trend(&xs, &handles);
    let threads: Vec<f64> = rows.iter().map(|r| f64::from(r.threads_avg)).collect();
    let (thread_slope, thread_fit) = trend(&xs, &threads);
    let last = rows.last().copied().unwrap_or_default();

    let cpu_sum: f64 = rows.iter().map(|r| f64::from(r.cpu_avg)).sum();
    Some(AppStats {
        app: app.to_owned(),
        minutes: rows.len() as u32,
        cpu_avg: (cpu_sum / n) as f32,
        cpu_p95: cpu[p95_index],
        cpu_peak: rows.iter().map(|r| r.cpu_peak).fold(0.0, f32::max),
        gpu_avg: (gpu.iter().map(|g| f64::from(*g)).sum::<f64>() / n) as f32,
        gpu_p95: gpu[p95_index],
        mem_avg: (ys.iter().sum::<f64>() / n) as u64,
        mem_peak: rows.iter().map(|r| r.mem_peak).max().unwrap_or(0),
        mem_last: last.mem_avg,
        mem_growth_per_hour: slope,
        mem_growth_fit: fit,
        // Each row is one minute: share% / 100 * cpus * 60 s.
        cpu_core_seconds: cpu_sum / 100.0 * f64::from(logical_cpus) * 60.0,
        handles_last: last.handles_avg,
        handle_growth_per_hour: handle_slope,
        handle_growth_fit: handle_fit,
        threads_last: last.threads_avg,
        thread_growth_per_hour: thread_slope,
        thread_growth_fit: thread_fit,
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
    /// Average GPU (busiest engine) that marks an app as a chronic GPU user.
    pub chronic_gpu_percent: f32,
    /// Empty: only report. Priority and efficiency mode do not limit GPU work.
    pub chronic_gpu_actions: Vec<Action>,
    /// Handle growth per hour that marks a handle leak suspect, once the
    /// app holds at least `handle_min` handles (steadiness: `growth_min_fit`).
    pub handle_growth_per_hour: f64,
    pub handle_min: u32,
    /// Thread growth per hour that marks a thread leak suspect, once the
    /// app runs at least `thread_min` threads.
    pub thread_growth_per_hour: f64,
    pub thread_min: u32,
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
            chronic_gpu_percent: 30.0,
            chronic_gpu_actions: Vec::new(),
            handle_growth_per_hour: 1000.0,
            handle_min: 10_000,
            thread_growth_per_hour: 50.0,
            thread_min: 500,
            exempt: vec!["explorer.exe".into(), "Taskmgr.exe".into()],
        }
    }
}

impl InsightsConfig {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.window_hours == 0 || self.evaluate_every_minutes == 0 {
            return Err("window_hours and evaluate_every_minutes must be positive".into());
        }
        for (name, v) in [
            ("chronic_cpu_percent", self.chronic_cpu_percent),
            ("chronic_gpu_percent", self.chronic_gpu_percent),
        ] {
            if !(0.0..=100.0).contains(&v) {
                return Err(format!("{name} must be within 0..=100"));
            }
        }
        for a in self
            .chronic_actions
            .iter()
            .chain(&self.growth_actions)
            .chain(&self.chronic_gpu_actions)
        {
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
    ChronicGpu,
    HandleGrowth,
    ThreadGrowth,
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
        if s.gpu_avg >= cfg.chronic_gpu_percent {
            out.push(Finding {
                app: s.app.clone(),
                kind: FindingKind::ChronicGpu,
                detail: format!(
                    "average GPU {:.1}% (p95 {:.1}%) over {} min",
                    s.gpu_avg, s.gpu_p95, s.minutes
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
        if s.handle_growth_per_hour >= cfg.handle_growth_per_hour
            && s.handle_growth_fit >= cfg.growth_min_fit
            && s.handles_last >= cfg.handle_min
        {
            out.push(Finding {
                app: s.app.clone(),
                kind: FindingKind::HandleGrowth,
                detail: format!(
                    "handles growing {:.0}/h over {} min (fit {:.2}), now {}",
                    s.handle_growth_per_hour, s.minutes, s.handle_growth_fit, s.handles_last
                ),
            });
        }
        if s.thread_growth_per_hour >= cfg.thread_growth_per_hour
            && s.thread_growth_fit >= cfg.growth_min_fit
            && s.threads_last >= cfg.thread_min
        {
            out.push(Finding {
                app: s.app.clone(),
                kind: FindingKind::ThreadGrowth,
                detail: format!(
                    "threads growing {:.0}/h over {} min (fit {:.2}), now {}",
                    s.thread_growth_per_hour, s.minutes, s.thread_growth_fit, s.threads_last
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
    /// Instances already acted on, with the findings whose actions they got.
    applied: HashMap<Key, HashSet<FindingKind>>,
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
        let mut still: HashMap<&str, HashSet<FindingKind>> = HashMap::new();
        for (app, kind) in self.flagged.keys() {
            if !actions_for(*kind, cfg).is_empty() {
                still.entry(app.as_str()).or_default().insert(*kind);
            }
        }
        // An instance keeps its actions only while every finding behind them
        // stands; otherwise all are taken back and the next tick applies
        // those of the findings that remain.
        self.applied.retain(|key, kinds| {
            let target = &self.targets[key];
            let keep = still
                .get(target.name.to_lowercase().as_str())
                .is_some_and(|now| kinds.is_subset(now));
            if !keep {
                restores.push(Decision::Restore {
                    source: Source::Insight,
                    target: target.clone(),
                    reason: "long-term finding cleared".into(),
                });
            }
            keep
        });
        self.targets.retain(|k, _| self.applied.contains_key(k));
        (raised, restores)
    }

    /// Applies actions to instances of flagged apps not yet handled. Call on
    /// every sample so newly started instances are covered too.
    pub fn tick(&mut self, procs: &[ProcessInfo], cfg: &InsightsConfig) -> Vec<Decision> {
        if !self.applied.is_empty() || !self.targets.is_empty() {
            let live: HashSet<Key> = procs.iter().map(|p| (p.pid, p.create_time)).collect();
            self.applied.retain(|k, _| live.contains(k));
            self.targets.retain(|k, _| live.contains(k));
        }

        // The few flagged apps with something to apply, with their actions
        // in finding order; usually none, and then there is nothing to scan.
        let mut wanted: Vec<(&str, Vec<(Action, &Finding)>)> = Vec::new();
        let mut kinds_of: HashMap<&str, HashSet<FindingKind>> = HashMap::new();
        for kind in [
            FindingKind::ChronicCpu,
            FindingKind::MemoryGrowth,
            FindingKind::ChronicGpu,
        ] {
            let actions = actions_for(kind, cfg);
            if actions.is_empty() {
                continue;
            }
            for ((app, _), f) in self.flagged.iter().filter(|((_, k), _)| *k == kind) {
                let i = match wanted.iter().position(|(a, _)| a == app) {
                    Some(i) => i,
                    None => {
                        wanted.push((app, Vec::new()));
                        wanted.len() - 1
                    }
                };
                wanted[i].1.extend(actions.iter().map(|a| (*a, f)));
                kinds_of.entry(app.as_str()).or_default().insert(kind);
            }
        }
        let mut out = Vec::new();
        if wanted.is_empty() {
            return out;
        }
        for p in procs {
            let key = (p.pid, p.create_time);
            // Services (session 0) share their names with every instance:
            // an hour of 80 svchost.exe together is no one host's doing.
            if p.session_id == 0
                || protection_reason(p.pid, &p.name).is_some()
                || wtm_core::automatic_end_blocked(&p.name).is_some()
            {
                continue;
            }
            // Flagged apps are keyed lowercase.
            let Some((app, actions)) = wanted
                .iter()
                .find(|(app, _)| crate::glob::eq_lowercase(&p.name, app))
            else {
                continue;
            };
            // Only findings this instance has not been acted on for yet.
            let done = self.applied.get(&key);
            if done.is_some_and(|d| kinds_of[app].is_subset(d)) {
                continue;
            }
            let target = ProcessRef::from(p);
            for &(action, finding) in actions {
                if done.is_some_and(|d| d.contains(&finding.kind)) {
                    continue;
                }
                out.push(Decision::Apply {
                    source: Source::Insight,
                    target: target.clone(),
                    action,
                    reason: format!("{:?}: {}", finding.kind, finding.detail),
                });
            }
            self.applied
                .entry(key)
                .or_default()
                .extend(kinds_of[app].iter().copied());
            self.targets.insert(key, target);
        }
        out
    }
}

fn actions_for(kind: FindingKind, cfg: &InsightsConfig) -> &[Action] {
    match kind {
        FindingKind::ChronicCpu => &cfg.chronic_actions,
        FindingKind::MemoryGrowth => &cfg.growth_actions,
        FindingKind::ChronicGpu => &cfg.chronic_gpu_actions,
        // A leak of handles or threads is not relieved by any limit.
        FindingKind::HandleGrowth | FindingKind::ThreadGrowth => &[],
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
            gpu_avg: 0.0,
            mem_avg: mem_mb << 20,
            mem_peak: (mem_mb + 10) << 20,
            processes: 1,
            handles_avg: 100,
            threads_avg: 10,
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
            net_rx_rate: 0.0,
            net_tx_rate: 0.0,
            suspended: false,
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
            gpu_avg: 0.0,
            gpu_p95: 0.0,
            mem_avg: last_mib << 20,
            mem_peak: last_mib << 20,
            mem_last: last_mib << 20,
            mem_growth_per_hour: growth_mib * 1024.0 * 1024.0,
            mem_growth_fit: 0.9,
            cpu_core_seconds: 0.0,
            handles_last: 0,
            handle_growth_per_hour: 0.0,
            handle_growth_fit: 0.0,
            threads_last: 0,
            thread_growth_per_hour: 0.0,
            thread_growth_fit: 0.0,
        }
    }

    #[test]
    fn handle_and_thread_leaks_are_found_from_steady_growth() {
        // Two hours: handles +2000/h from 12 000, threads +100/h from 600.
        let rows: Vec<MinuteRow> = (0..120)
            .map(|m| MinuteRow {
                minute: m,
                handles_avg: 12_000 + (m as u32) * 2000 / 60,
                threads_avg: 600 + (m as u32) * 100 / 60,
                ..row(m, 1.0, 100)
            })
            .collect();
        let s = summarize("leaky.exe", &rows, 8).unwrap();
        assert!((s.handle_growth_per_hour - 2000.0).abs() < 50.0, "{s:?}");
        assert!(s.handle_growth_fit > 0.99 && s.thread_growth_fit > 0.99);
        assert_eq!((s.handles_last, s.threads_last), (15_966, 798));
        let found = analyze(std::slice::from_ref(&s), &InsightsConfig::default());
        let kinds: Vec<_> = found.iter().map(|f| f.kind).collect();
        assert_eq!(
            kinds,
            [FindingKind::HandleGrowth, FindingKind::ThreadGrowth]
        );
        assert!(
            found[0].detail.starts_with("handles growing 2"),
            "{}",
            found[0].detail
        );

        // Small apps and noise are not leaks.
        let small = AppStats {
            handles_last: 900,
            threads_last: 40,
            ..s.clone()
        };
        assert!(analyze(&[small], &InsightsConfig::default()).is_empty());
        let noisy = AppStats {
            handle_growth_fit: 0.2,
            thread_growth_fit: 0.2,
            ..s
        };
        assert!(analyze(&[noisy], &InsightsConfig::default()).is_empty());
        // Report only: nothing to apply or roll back.
        let cfg = InsightsConfig::default();
        assert!(actions_for(FindingKind::HandleGrowth, &cfg).is_empty());
        assert!(actions_for(FindingKind::ThreadGrowth, &cfg).is_empty());
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
    fn policy_matches_non_ascii_names_by_lowercase() {
        let cfg = InsightsConfig::default();
        let mut p = InsightPolicy::new();
        let finding = Finding {
            app: "ÇAY.exe".into(),
            kind: FindingKind::ChronicCpu,
            detail: "x".into(),
        };
        p.update(vec![finding], &cfg);
        let procs = [proc(1, "çay.EXE"), proc(2, "cay.exe")];
        let pids: Vec<u32> = p
            .tick(&procs, &cfg)
            .iter()
            .map(|d| d.target().pid)
            .collect();
        assert_eq!(pids, [1, 1]);
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

    #[test]
    fn summarize_without_trend_has_zero_slope_and_fit() {
        // A single minute has no time variance.
        let s = summarize("one.exe", &[row(5, 3.0, 100)], 4).unwrap();
        assert_eq!((s.mem_growth_per_hour, s.mem_growth_fit), (0.0, 0.0));
        assert_eq!(s.cpu_p95, 3.0);
        // Flat memory over time: no memory variance, so no fit either.
        let flat: Vec<MinuteRow> = (0..10).map(|m| row(m, 1.0, 100)).collect();
        let s = summarize("flat.exe", &flat, 4).unwrap();
        assert_eq!((s.mem_growth_per_hour, s.mem_growth_fit), (0.0, 0.0));
    }

    #[test]
    fn validate_rejects_bad_settings() {
        let ok = InsightsConfig::default();
        assert!(ok.validate().is_ok());
        let bad = [
            InsightsConfig {
                window_hours: 0,
                ..ok.clone()
            },
            InsightsConfig {
                evaluate_every_minutes: 0,
                ..ok.clone()
            },
            InsightsConfig {
                chronic_cpu_percent: -1.0,
                ..ok.clone()
            },
            InsightsConfig {
                chronic_gpu_percent: 100.5,
                ..ok.clone()
            },
            InsightsConfig {
                growth_actions: vec![Action::CpuCapRelative(0.5)],
                ..ok.clone()
            },
            InsightsConfig {
                chronic_gpu_actions: vec![Action::MemoryCapMb(0)],
                ..ok.clone()
            },
        ];
        let errors: Vec<String> = bad.iter().map(|c| c.validate().unwrap_err()).collect();
        assert_eq!(
            errors,
            [
                "window_hours and evaluate_every_minutes must be positive",
                "window_hours and evaluate_every_minutes must be positive",
                "chronic_cpu_percent must be within 0..=100",
                "chronic_gpu_percent must be within 0..=100",
                "cpu_cap_relative is not supported for insights; use cpu_cap",
                "memory_cap_mb must be positive",
            ]
        );
    }

    #[test]
    fn chronic_gpu_is_flagged_listed_and_acted_on() {
        let mut gpu_hog = stats("render.exe", 300, 1.0, 0.0, 100);
        gpu_hog.gpu_avg = 45.0;
        gpu_hog.gpu_p95 = 80.0;
        let cfg = InsightsConfig {
            chronic_gpu_actions: vec![Action::Suspend],
            ..InsightsConfig::default()
        };
        let found = analyze(&[gpu_hog, stats("calm.exe", 300, 1.0, 0.0, 100)], &cfg);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, FindingKind::ChronicGpu);
        assert_eq!(
            found[0].detail,
            "average GPU 45.0% (p95 80.0%) over 300 min"
        );

        let mut p = InsightPolicy::new();
        assert!(p.findings().is_empty());
        p.update(found.clone(), &cfg);
        assert_eq!(p.findings(), found);
        let d = p.tick(&[proc(7, "render.exe")], &cfg);
        assert!(matches!(
            d.as_slice(),
            [Decision::Apply { action: Action::Suspend, reason, .. }]
                if reason.starts_with("ChronicGpu: ")
        ));
    }

    #[test]
    fn findings_are_sorted_by_app() {
        let mut p = InsightPolicy::new();
        let f = |app: &str| Finding {
            app: app.into(),
            kind: FindingKind::ChronicCpu,
            detail: String::new(),
        };
        p.update(
            vec![f("b.exe"), f("a.exe"), f("c.exe")],
            &InsightsConfig::default(),
        );
        let apps: Vec<String> = p.findings().into_iter().map(|f| f.app).collect();
        assert_eq!(apps, ["a.exe", "b.exe", "c.exe"]);
    }

    #[test]
    fn a_later_finding_reaches_running_instances_and_each_clears_alone() {
        let cfg = InsightsConfig {
            chronic_actions: vec![Action::EcoQos],
            growth_actions: vec![Action::MemoryCapMb(4096)],
            ..InsightsConfig::default()
        };
        let found = |kind| Finding {
            app: "app.exe".into(),
            kind,
            detail: "x".into(),
        };
        let applied = |d: Vec<Decision>| -> Vec<Action> {
            d.into_iter()
                .filter_map(|d| match d {
                    Decision::Apply { action, .. } => Some(action),
                    Decision::Restore { .. } => None,
                })
                .collect()
        };
        let procs = [proc(1, "app.exe")];
        let mut p = InsightPolicy::new();
        p.update(vec![found(FindingKind::ChronicCpu)], &cfg);
        assert_eq!(applied(p.tick(&procs, &cfg)), [Action::EcoQos]);
        p.update(
            vec![
                found(FindingKind::ChronicCpu),
                found(FindingKind::MemoryGrowth),
            ],
            &cfg,
        );
        assert_eq!(applied(p.tick(&procs, &cfg)), [Action::MemoryCapMb(4096)]);
        // Chronic CPU clears: all taken back, the growth action comes again.
        let (_, restores) = p.update(vec![found(FindingKind::MemoryGrowth)], &cfg);
        assert_eq!(restores.len(), 1);
        assert_eq!(applied(p.tick(&procs, &cfg)), [Action::MemoryCapMb(4096)]);
        assert!(p.tick(&procs, &cfg).is_empty());
    }
}

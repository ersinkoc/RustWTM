//! Bounded incident snapshots: observed contributors, never inferred causality.
use crate::{Snapshot, control::ProcessRef};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Process {
    pub target: ProcessRef,
    pub cpu: f32,
    pub memory: u64,
    pub gpu: f32,
    pub io: f64,
    pub network: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub at: u64,
    pub cpu: f32,
    pub memory: u64,
    pub memory_total: u64,
    pub gpu: f32,
    pub network: f64,
    pub io: f64,
    pub processes: Vec<Process>,
    /// Per-process network attribution needs ETW; system adapter totals do not.
    #[serde(default)]
    pub process_network_available: bool,
    pub partial: bool,
}
impl Frame {
    pub fn of(s: &Snapshot, at: u64) -> Self {
        let rows: Vec<_> = s
            .processes
            .iter()
            .take(16384)
            .filter(|p| p.pid != 0)
            .collect();
        let mut chosen = std::collections::BTreeSet::new();
        for (metric, count) in [(0, 5), (1, 5), (2, 4), (3, 4), (4, 3)] {
            let mut indices: Vec<_> = (0..rows.len()).collect();
            let value = |index: usize| {
                let p = rows[index];
                match metric {
                    0 => f64::from(p.cpu_percent),
                    1 => p.private_working_set as f64,
                    2 => p.io_read_rate + p.io_write_rate,
                    3 => p.net_rx_rate + p.net_tx_rate,
                    _ => f64::from(p.gpu_percent),
                }
            };
            indices.sort_by(|a, b| {
                value(*b)
                    .total_cmp(&value(*a))
                    .then(rows[*a].pid.cmp(&rows[*b].pid))
            });
            chosen.extend(indices.into_iter().take(count));
        }
        let processes = chosen
            .into_iter()
            .map(|index| {
                let p = rows[index];
                let mut target = ProcessRef::from(p);
                target.name = target.name.chars().take(64).collect();
                Process {
                    target,
                    cpu: p.cpu_percent,
                    memory: p.private_working_set,
                    gpu: p.gpu_percent,
                    io: p.io_read_rate + p.io_write_rate,
                    network: p.net_rx_rate + p.net_tx_rate,
                }
            })
            .collect();
        Self {
            at,
            cpu: s.system.cpu_percent,
            memory: s.system.memory_used(),
            memory_total: s.system.memory_total,
            gpu: s.system.gpu_percent,
            network: s.system.net_rx_rate + s.system.net_tx_rate,
            io: s.system.io_read_rate + s.system.io_write_rate,
            processes,
            process_network_available: s.net_traced,
            partial: s.processes.len() > 16384,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedAction {
    pub at: u64,
    pub pid: Option<u32>,
    pub name: Option<String>,
    pub level: String,
    pub message: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Replay {
    pub frames: Vec<Frame>,
    pub actions: Vec<RecordedAction>,
    pub actions_truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProcessInfo, SystemStats};
    #[test]
    fn contributor_union_is_bounded_and_preserves_distinct_resource_leaders() {
        let rows = (1..=24)
            .map(|pid| ProcessInfo {
                pid,
                parent_pid: 0,
                name: if pid == 1 {
                    "測".repeat(100)
                } else {
                    format!("app-{pid}.exe")
                },
                cpu_percent: if pid <= 5 { 50.0 } else { 0.0 },
                cpu_time: std::time::Duration::ZERO,
                working_set: 0,
                private_working_set: if (6..=10).contains(&pid) { 1000 } else { 0 },
                private_bytes: 0,
                thread_count: 1,
                handle_count: 1,
                session_id: 1,
                base_priority: 8,
                gpu_percent: if (19..=21).contains(&pid) { 50.0 } else { 0.0 },
                gpu_memory: 0,
                io_read_rate: if (11..=14).contains(&pid) {
                    1000.0
                } else {
                    0.0
                },
                io_write_rate: 0.0,
                net_rx_rate: if (15..=18).contains(&pid) {
                    1000.0
                } else {
                    0.0
                },
                net_tx_rate: 0.0,
                create_time: i64::from(pid) * 100,
                suspended: false,
            })
            .collect();
        let snapshot = Snapshot {
            processes: rows,
            system: SystemStats::default(),
            interval: Some(std::time::Duration::from_secs(1)),
            taken_at_ms: 5000,
            net_traced: true,
        };
        let frame = Frame::of(&snapshot, 5000);
        assert_eq!(frame.processes.len(), 21);
        for pid in 1..=21 {
            assert_eq!(
                frame
                    .processes
                    .iter()
                    .find(|p| p.target.pid == pid)
                    .unwrap()
                    .target
                    .create_time,
                i64::from(pid) * 100
            );
        }
        assert_eq!(frame.processes[0].target.name.chars().count(), 64);
        assert!(frame.process_network_available);
        assert!(!frame.partial);
        let bytes = serde_json::to_vec(&frame).unwrap();
        assert!(bytes.len() < 16384);
    }
}

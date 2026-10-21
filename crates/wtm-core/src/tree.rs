use std::collections::{HashSet, VecDeque};

use crate::ProcessInfo;

/// All descendants of the given process, breadth-first.
///
/// A child only counts if it was created after its parent, so a stale parent
/// PID that has since been reused does not adopt unrelated processes.
pub fn descendants(
    root_pid: u32,
    root_create_time: i64,
    procs: &[ProcessInfo],
) -> Vec<&ProcessInfo> {
    let mut out = Vec::new();
    let mut seen = HashSet::from([root_pid]);
    let mut queue = VecDeque::from([(root_pid, root_create_time)]);

    while let Some((pid, created)) = queue.pop_front() {
        for child in procs {
            if child.parent_pid == pid && child.create_time >= created && seen.insert(child.pid) {
                queue.push_back((child.pid, child.create_time));
                out.push(child);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn p(pid: u32, parent_pid: u32, create_time: i64) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid,
            name: String::new(),
            cpu_percent: 0.0,
            cpu_time: Duration::ZERO,
            working_set: 0,
            private_working_set: 0,
            private_bytes: 0,
            thread_count: 0,
            handle_count: 0,
            session_id: 0,
            base_priority: 8,
            gpu_percent: 0.0,
            gpu_memory: 0,
            io_read_rate: 0.0,
            io_write_rate: 0.0,
            net_rx_rate: 0.0,
            net_tx_rate: 0.0,
            create_time,
        }
    }

    #[test]
    fn finds_grandchildren_and_skips_stale_parents() {
        let procs = [
            p(10, 1, 100),
            p(11, 10, 110),
            p(12, 11, 120),
            p(13, 10, 50), // older than its "parent": PID reuse, not a child
            p(14, 99, 130),
        ];
        let pids: Vec<u32> = descendants(10, 100, &procs).iter().map(|p| p.pid).collect();
        assert_eq!(pids, [11, 12]);
    }
}

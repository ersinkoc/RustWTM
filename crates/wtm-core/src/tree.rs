use std::collections::{HashSet, VecDeque};

use crate::ProcessInfo;

/// All descendants of the given process, breadth-first.
///
/// A child only counts if it was created after its parent, so a stale parent
/// PID that has since been reused does not adopt unrelated processes.
pub fn descendants<'a>(
    root_pid: u32,
    root_create_time: i64,
    procs: &'a [ProcessInfo],
) -> Vec<&'a ProcessInfo> {
    let mut out = Vec::new();
    let mut seen = HashSet::from([root_pid]);
    let mut queue = VecDeque::new();
    let mut visit = |child: &'a ProcessInfo, created: i64, queue: &mut VecDeque<_>| {
        if child.create_time >= created && seen.insert(child.pid) {
            queue.push_back((child.pid, child.create_time));
            out.push(child);
        }
    };

    // Most processes have no children: one plain pass answers those.
    for child in procs.iter().filter(|c| c.parent_pid == root_pid) {
        visit(child, root_create_time, &mut queue);
    }
    if queue.is_empty() {
        return Vec::new();
    }
    // Otherwise index children by parent once instead of scanning every
    // process per descendant. Sorted by (parent, position), so each parent's
    // children come in `procs` order, as a plain scan would find them.
    let mut by_parent: Vec<(u32, usize)> = procs
        .iter()
        .enumerate()
        .map(|(i, p)| (p.parent_pid, i))
        .collect();
    by_parent.sort_unstable();
    while let Some((pid, created)) = queue.pop_front() {
        let start = by_parent.partition_point(|&(parent, _)| parent < pid);
        for &(_, i) in by_parent[start..].iter().take_while(|&&(p, _)| p == pid) {
            visit(&procs[i], created, &mut queue);
        }
    }
    drop(visit);
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
            suspended: false,
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

    /// The indexed walk finds exactly what scanning every process per
    /// descendant finds, in the same order.
    #[test]
    fn matches_a_plain_scan() {
        fn plain(root: u32, created: i64, procs: &[ProcessInfo]) -> Vec<u32> {
            let mut out = Vec::new();
            let mut seen = HashSet::from([root]);
            let mut queue = VecDeque::from([(root, created)]);
            while let Some((pid, created)) = queue.pop_front() {
                for c in procs {
                    if c.parent_pid == pid && c.create_time >= created && seen.insert(c.pid) {
                        queue.push_back((c.pid, c.create_time));
                        out.push(c.pid);
                    }
                }
            }
            out
        }
        // Shuffled parents, reused PIDs (older "children"), duplicate PIDs
        // and a cycle (3 <-> 4).
        let mut procs: Vec<ProcessInfo> = (1..300u32)
            .map(|pid| p(pid, (pid * 37) % 53, i64::from(pid % 11)))
            .collect();
        procs.push(p(3, 4, 5));
        procs.push(p(4, 3, 5));
        procs.push(p(20, 3, 0));
        for root in 0..60 {
            for created in [0, 5, 10] {
                let fast: Vec<u32> = descendants(root, created, &procs)
                    .iter()
                    .map(|p| p.pid)
                    .collect();
                assert_eq!(fast, plain(root, created, &procs), "root {root}");
            }
        }
    }

    /// `cargo test --release -p wtm-core descendants_speed -- --ignored --nocapture`
    #[test]
    #[ignore = "timing harness"]
    fn descendants_speed() {
        // 2000 processes; pid 1000 heads a tree of 300 (a browser or IDE).
        let mut procs: Vec<ProcessInfo> = (1..=2000u32)
            .map(|pid| {
                p(
                    pid,
                    if pid > 1 {
                        (pid * 7) % (pid - 1) + 1
                    } else {
                        0
                    },
                    1,
                )
            })
            .collect();
        for (i, pid) in (1001..=1300u32).enumerate() {
            procs[pid as usize - 1] = p(pid, if i < 20 { 1000 } else { 1001 + i as u32 / 20 }, 2);
        }
        let check = descendants(1000, 1, &procs).len();
        assert!(check >= 300, "{check}");
        for (label, root) in [("tree of 300", 1000), ("leaf", 2000)] {
            let runs = 200;
            let started = std::time::Instant::now();
            for _ in 0..runs {
                std::hint::black_box(descendants(root, 1, std::hint::black_box(&procs)));
            }
            println!("{label}: {:?} per call", started.elapsed() / runs);
        }
    }
}

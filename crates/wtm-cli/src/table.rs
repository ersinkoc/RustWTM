use wtm_core::{ProcessInfo, SystemStats};

/// Width of every column before NAME, including separators.
const PREFIX_WIDTH: usize = 67;

pub fn header() -> String {
    format!(
        "{:>7} {:>7} {:>6} {:>6} {:>10} {:>5} {:>7} {:>10}  NAME",
        "PID", "PPID", "CPU%", "GPU%", "MEMORY", "THR", "HANDLES", "DISK/s"
    )
}

/// Formats one process; the name is truncated so the line fits `width` columns.
pub fn row(p: &ProcessInfo, width: usize) -> String {
    let name_width = width.saturating_sub(PREFIX_WIDTH).max(8);
    let name: String = if p.name.chars().count() > name_width {
        p.name.chars().take(name_width - 1).chain(['…']).collect()
    } else {
        p.name.clone()
    };
    format!(
        "{:>7} {:>7} {:>6.1} {:>6.1} {:>10} {:>5} {:>7} {:>10}  {}",
        p.pid,
        p.parent_pid,
        p.cpu_percent,
        p.gpu_percent,
        bytes(p.private_working_set as f64),
        p.thread_count,
        p.handle_count,
        bytes(p.io_rate()),
        name
    )
}

pub fn summary(s: &SystemStats) -> String {
    let mem_pct = if s.memory_total > 0 {
        s.memory_used() as f64 / s.memory_total as f64 * 100.0
    } else {
        0.0
    };
    format!(
        "CPU {:5.1}% ({} logical)   Memory {} / {} ({:.0}%)   Processes {}   Threads {}   Handles {}",
        s.cpu_percent,
        s.logical_cpus,
        bytes(s.memory_used() as f64),
        bytes(s.memory_total as f64),
        mem_pct,
        s.process_count,
        s.thread_count,
        s.handle_count
    )
}

pub fn bytes(n: f64) -> String {
    wtm_core::format_bytes(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(0.0), "0 B");
        assert_eq!(bytes(1536.0), "1.5 KiB");
        assert_eq!(bytes(3.0 * 1024.0 * 1024.0 * 1024.0), "3.0 GiB");
    }

    #[test]
    fn header_prefix_matches_row_prefix() {
        assert_eq!(header().find("NAME"), Some(PREFIX_WIDTH));
    }
}

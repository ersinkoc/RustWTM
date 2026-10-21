use wtm_core::{ProcessInfo, SystemStats};

/// Width of every column before NAME, including separators.
const PREFIX_WIDTH: usize = 67;
/// Width the NET/s column adds.
const NET_WIDTH: usize = 11;

/// Width before NAME; the NET/s column appears only when per-process
/// network traffic was measured (`net`), since otherwise it would be all 0.
fn prefix_width(net: bool) -> usize {
    PREFIX_WIDTH + if net { NET_WIDTH } else { 0 }
}

pub fn header(net: bool) -> String {
    let net = if net {
        format!(" {:>10}", "NET/s")
    } else {
        String::new()
    };
    format!(
        "{:>7} {:>7} {:>6} {:>6} {:>10} {:>5} {:>7} {:>10}{net}  NAME",
        "PID", "PPID", "CPU%", "GPU%", "MEMORY", "THR", "HANDLES", "DISK/s"
    )
}

/// Formats one process; the name is truncated so the line fits `width`
/// columns. `net` adds the network column (see [`header`]).
pub fn row(p: &ProcessInfo, width: usize, net: bool) -> String {
    let name_width = width.saturating_sub(prefix_width(net)).max(8);
    let name: String = if p.name.chars().count() > name_width {
        p.name.chars().take(name_width - 1).chain(['…']).collect()
    } else {
        p.name.clone()
    };
    let net = if net {
        format!(" {:>10}", bytes(p.net_rate()))
    } else {
        String::new()
    };
    format!(
        "{:>7} {:>7} {:>6.1} {:>6.1} {:>10} {:>5} {:>7} {:>10}{net}  {}",
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

/// Appended to a summary line when per-process network traffic was not
/// measured (it needs the service's trace), so the missing NET/s column
/// does not read as "no traffic".
pub fn network_note(net: bool) -> &'static str {
    if net {
        ""
    } else {
        "   per-process network: n/a"
    }
}

/// Width the NAME column is padded to when a command line follows it.
const NAME_WIDTH: usize = 28;

/// `line` (a header or row ending with a name of `name_len` characters)
/// followed by an extra column.
pub fn then(line: String, name_len: usize, extra: &str) -> String {
    let pad = NAME_WIDTH.saturating_sub(name_len).max(1);
    format!("{line}{:pad$}{extra}", "")
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

/// GPU, network and disk on one line.
pub fn io_summary(s: &SystemStats) -> String {
    let busiest_disk = s
        .disks
        .iter()
        .map(|d| d.active_percent)
        .fold(0.0f32, f32::max);
    format!(
        "GPU {:5.1}%   Network down {}/s up {}/s   Disk read {}/s write {}/s (busiest {:.0}% active)",
        s.gpu_percent,
        bytes(s.net_rx_rate),
        bytes(s.net_tx_rate),
        bytes(s.io_read_rate),
        bytes(s.io_write_rate),
        busiest_disk
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
    fn summary_without_memory_total() {
        let s = summary(&SystemStats::default());
        assert!(s.contains("Memory 0 B / 0 B (0%)"), "{s}");
    }

    #[test]
    fn io_summary_shows_rates_and_the_busiest_disk() {
        let s = SystemStats {
            net_rx_rate: 2048.0,
            io_write_rate: 1024.0,
            disks: vec![
                wtm_core::DiskUsage {
                    active_percent: 12.0,
                    ..Default::default()
                },
                wtm_core::DiskUsage {
                    active_percent: 64.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let line = io_summary(&s);
        assert!(line.contains("Network down 2.0 KiB/s up 0 B/s"), "{line}");
        assert!(
            line.contains("write 1.0 KiB/s (busiest 64% active)"),
            "{line}"
        );
    }

    #[test]
    fn extra_column_lines_up() {
        assert_eq!(then("ab".into(), 2, "x").find('x'), Some(28));
        assert_eq!(
            then("a".repeat(40), 40, "x"),
            format!("{} x", "a".repeat(40))
        );
    }

    #[test]
    fn header_prefix_matches_row_prefix() {
        for net in [false, true] {
            assert_eq!(header(net).find("NAME"), Some(prefix_width(net)));
        }
    }

    #[test]
    fn network_column_only_when_measured() {
        let p = ProcessInfo {
            pid: 42,
            parent_pid: 1,
            name: "app.exe".into(),
            cpu_percent: 0.0,
            cpu_time: std::time::Duration::ZERO,
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
            net_rx_rate: 1024.0,
            net_tx_rate: 1024.0,
            suspended: false,
            create_time: 0,
        };
        let traced = row(&p, usize::MAX, true);
        assert!(traced.contains("2.0 KiB"), "{traced}");
        assert_eq!(traced.find("app.exe"), Some(prefix_width(true)));
        let untraced = row(&p, usize::MAX, false);
        assert!(!untraced.contains("KiB"), "{untraced}");
        assert!(!header(false).contains("NET"));
        assert!(network_note(false).contains("n/a"));
        assert!(network_note(true).is_empty());
    }
}

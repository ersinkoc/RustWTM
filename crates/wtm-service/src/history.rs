//! Per-minute, per-application usage history stored in SQLite.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, params};
use wtm_core::Snapshot;
use wtm_policy::{AppStats, MinuteRow, summarize};

#[derive(Debug, Default)]
struct Acc {
    cpu_sum: f64,
    cpu_peak: f32,
    mem_sum: f64,
    mem_peak: u64,
    processes: u32,
    samples: u32,
}

pub struct History {
    conn: Connection,
    minute: Option<i64>,
    apps: HashMap<String, Acc>,
}

const SCHEMA: &str = "
    PRAGMA journal_mode = WAL;
    CREATE TABLE IF NOT EXISTS app_minute (
        app TEXT NOT NULL,
        minute INTEGER NOT NULL,
        cpu_avg REAL NOT NULL,
        cpu_peak REAL NOT NULL,
        mem_avg INTEGER NOT NULL,
        mem_peak INTEGER NOT NULL,
        processes INTEGER NOT NULL,
        PRIMARY KEY (app, minute)
    ) WITHOUT ROWID;
    CREATE INDEX IF NOT EXISTS app_minute_time ON app_minute (minute);
";

impl History {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Self::with(Connection::open(path)?)
    }

    pub(crate) fn with(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn,
            minute: None,
            apps: HashMap::new(),
        })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Adds one sample taken at `unix_secs`. Rows for a minute are written
    /// once the next minute starts.
    pub fn record(&mut self, snap: &Snapshot, unix_secs: i64) -> rusqlite::Result<()> {
        if snap.interval.is_none() {
            return Ok(());
        }
        let minute = unix_secs.div_euclid(60);
        if self.minute.is_some_and(|m| m != minute) {
            self.flush()?;
        }
        self.minute = Some(minute);

        let mut per_app: HashMap<String, (f32, u64, u32)> = HashMap::new();
        for p in snap.processes.iter().filter(|p| p.pid != 0) {
            let e = per_app.entry(p.name.to_lowercase()).or_default();
            e.0 += p.cpu_percent;
            e.1 += p.private_working_set;
            e.2 += 1;
        }
        for (app, (cpu, mem, count)) in per_app {
            let acc = self.apps.entry(app).or_default();
            acc.cpu_sum += f64::from(cpu);
            acc.cpu_peak = acc.cpu_peak.max(cpu);
            acc.mem_sum += mem as f64;
            acc.mem_peak = acc.mem_peak.max(mem);
            acc.processes = acc.processes.max(count);
            acc.samples += 1;
        }
        Ok(())
    }

    /// Writes the current minute's aggregates.
    pub fn flush(&mut self) -> rusqlite::Result<()> {
        let Some(minute) = self.minute else {
            return Ok(());
        };
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO app_minute
                 (app, minute, cpu_avg, cpu_peak, mem_avg, mem_peak, processes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for (app, a) in self.apps.drain() {
                let n = f64::from(a.samples.max(1));
                stmt.execute(params![
                    app,
                    minute,
                    a.cpu_sum / n,
                    f64::from(a.cpu_peak),
                    (a.mem_sum / n) as i64,
                    a.mem_peak as i64,
                    a.processes,
                ])?;
            }
        }
        tx.commit()
    }

    pub fn prune(&self, keep_days: u32, now_secs: i64) -> rusqlite::Result<usize> {
        let cutoff = now_secs.div_euclid(60) - i64::from(keep_days) * 24 * 60;
        self.conn
            .execute("DELETE FROM app_minute WHERE minute < ?1", params![cutoff])
    }
}

/// Per-app summaries for rows at or after `since_minute`.
pub fn stats(
    conn: &Connection,
    since_minute: i64,
    logical_cpus: u32,
) -> rusqlite::Result<Vec<AppStats>> {
    let mut stmt = conn.prepare(
        "SELECT app, minute, cpu_avg, cpu_peak, mem_avg, mem_peak, processes
         FROM app_minute WHERE minute >= ?1",
    )?;
    let mut grouped: HashMap<String, Vec<MinuteRow>> = HashMap::new();
    let rows = stmt.query_map(params![since_minute], |r| {
        Ok((r.get::<_, String>(0)?, row(r, 1)?))
    })?;
    for item in rows {
        let (app, row) = item?;
        grouped.entry(app).or_default().push(row);
    }
    let mut out: Vec<AppStats> = grouped
        .iter()
        .filter_map(|(app, rows)| summarize(app, rows, logical_cpus))
        .collect();
    out.sort_by(|a, b| b.cpu_avg.total_cmp(&a.cpu_avg));
    Ok(out)
}

/// Minute rows of one app since `since_minute`, oldest first.
pub fn series(conn: &Connection, app: &str, since_minute: i64) -> rusqlite::Result<Vec<MinuteRow>> {
    let mut stmt = conn.prepare(
        "SELECT minute, cpu_avg, cpu_peak, mem_avg, mem_peak, processes
         FROM app_minute WHERE app = ?1 AND minute >= ?2 ORDER BY minute",
    )?;
    let rows = stmt.query_map(params![app.to_lowercase(), since_minute], |r| row(r, 0))?;
    rows.collect()
}

fn row(r: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<MinuteRow> {
    Ok(MinuteRow {
        minute: r.get(first)?,
        cpu_avg: r.get::<_, f64>(first + 1)? as f32,
        cpu_peak: r.get::<_, f64>(first + 2)? as f32,
        mem_avg: r.get::<_, i64>(first + 3)? as u64,
        mem_peak: r.get::<_, i64>(first + 4)? as u64,
        processes: r.get(first + 5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wtm_core::{ProcessInfo, SystemStats};

    fn proc(pid: u32, name: &str, cpu: f32, mem_mb: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: 1,
            name: name.into(),
            cpu_percent: cpu,
            cpu_time: Duration::ZERO,
            working_set: 0,
            private_working_set: mem_mb << 20,
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

    fn snap(procs: Vec<ProcessInfo>) -> Snapshot {
        Snapshot {
            interval: Some(Duration::from_secs(1)),
            system: SystemStats::default(),
            processes: procs,
        }
    }

    #[test]
    fn aggregates_instances_per_minute_and_queries() {
        let mut h = History::with(Connection::open_in_memory().unwrap()).unwrap();
        let base = 1_000 * 60;
        // Minute 1000: two samples, app "A.exe" has two instances.
        h.record(
            &snap(vec![proc(1, "A.exe", 2.0, 100), proc(2, "a.exe", 1.0, 50)]),
            base,
        )
        .unwrap();
        h.record(
            &snap(vec![proc(1, "A.exe", 6.0, 200), proc(3, "b.exe", 1.0, 10)]),
            base + 30,
        )
        .unwrap();
        // Minute 1001 starts: minute 1000 is flushed.
        h.record(&snap(vec![proc(1, "a.exe", 4.0, 300)]), base + 60)
            .unwrap();
        h.flush().unwrap();

        let a = series(h.connection(), "A.EXE", 0).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].minute, 1000);
        assert!((a[0].cpu_avg - 4.5).abs() < 1e-4); // (3 + 6) / 2
        assert!((a[0].cpu_peak - 6.0).abs() < 1e-4);
        assert_eq!(a[0].mem_peak, 200 << 20);
        assert_eq!(a[0].processes, 2);

        let s = stats(h.connection(), 0, 4).unwrap();
        assert_eq!(s[0].app, "a.exe"); // sorted by CPU
        assert_eq!(s[0].minutes, 2);
        assert_eq!(s.len(), 2);

        assert_eq!(h.prune(1, base + 60 + 2 * 24 * 3600).unwrap(), 3);
        assert!(stats(h.connection(), 0, 4).unwrap().is_empty());
    }
}

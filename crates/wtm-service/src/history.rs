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
    gpu_sum: f64,
    mem_sum: f64,
    mem_peak: u64,
    processes: u32,
    handles_sum: f64,
    threads_sum: f64,
    samples: u32,
    /// Network bytes received and sent in the minute.
    net_rx: f64,
    net_tx: f64,
    /// Totals of the instances in the sample being added.
    sample: Sample,
}

/// One app's instances in one sample.
#[derive(Debug, Default)]
struct Sample {
    cpu: f32,
    gpu: f32,
    mem: u64,
    count: u32,
    handles: u64,
    threads: u64,
    /// Bytes per second.
    net_rx: f64,
    net_tx: f64,
}

pub struct History {
    incident_slot: Option<i64>,
    incident_event: u64,
    incident_bytes: std::cell::Cell<i64>,
    conn: Connection,
    minute: Option<i64>,
    /// The current minute's aggregates; keys are lower-case app names, so
    /// a sample allocates only for apps new in the minute.
    apps: HashMap<String, Acc>,
    /// Scratch for lower-casing names that are not ASCII lower case.
    lower: String,
    /// Samples taken in the current minute, whichever apps they saw.
    minute_samples: u32,
}

const SCHEMA: &str = "
    PRAGMA journal_mode = WAL;
    CREATE TABLE IF NOT EXISTS incident_frame (
        sequence INTEGER PRIMARY KEY, at INTEGER NOT NULL,
        body TEXT NOT NULL CHECK(length(body) <= 16384)
    );
    CREATE INDEX IF NOT EXISTS incident_frame_time ON incident_frame(at);
    CREATE TABLE IF NOT EXISTS incident_action (
        sequence INTEGER PRIMARY KEY, at INTEGER NOT NULL, body TEXT NOT NULL CHECK(length(body)<=16384)
    );
    CREATE INDEX IF NOT EXISTS incident_action_time ON incident_action(at);
    CREATE TABLE IF NOT EXISTS app_minute (
        app TEXT NOT NULL,
        minute INTEGER NOT NULL,
        cpu_avg REAL NOT NULL,
        cpu_peak REAL NOT NULL,
        mem_avg INTEGER NOT NULL,
        mem_peak INTEGER NOT NULL,
        processes INTEGER NOT NULL,
        gpu_avg REAL NOT NULL DEFAULT 0,
        handles_avg INTEGER NOT NULL DEFAULT 0,
        threads_avg INTEGER NOT NULL DEFAULT 0,
        samples INTEGER NOT NULL DEFAULT 0,
        net_rx_bytes INTEGER NOT NULL DEFAULT 0,
        net_tx_bytes INTEGER NOT NULL DEFAULT 0,
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
        // Older databases lack the later columns: GPU (0.1.x), handles and
        // threads (0.15), samples (0.19), network bytes (0.32). A row without
        // a sample count weighs as one sample when a minute is merged.
        for (column, kind) in [
            ("gpu_avg", "REAL"),
            ("handles_avg", "INTEGER"),
            ("threads_avg", "INTEGER"),
            ("samples", "INTEGER"),
            ("net_rx_bytes", "INTEGER"),
            ("net_tx_bytes", "INTEGER"),
        ] {
            let present: bool = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('app_minute') WHERE name = ?1",
                [column],
                |r| r.get::<_, i64>(0).map(|n| n > 0),
            )?;
            if !present {
                conn.execute_batch(&format!(
                    "ALTER TABLE app_minute ADD COLUMN {column} {kind} NOT NULL DEFAULT 0"
                ))?;
            }
        }
        let incident_bytes = conn.query_row(
            "SELECT coalesce(sum(length(CAST(body AS BLOB))),0) FROM incident_frame",
            [],
            |r| r.get(0),
        )?;
        Ok(Self {
            incident_slot: None,
            incident_event: 0,
            incident_bytes: std::cell::Cell::new(incident_bytes),
            conn,
            minute: None,
            apps: HashMap::new(),
            lower: String::new(),
            minute_samples: 0,
        })
    }

    pub fn last_incident_event(&self) -> u64 {
        self.incident_event
    }
    pub fn record_actions(&mut self, events: &[wtm_ipc::Event]) -> rusqlite::Result<()> {
        for event in events.iter().take(2048) {
            let action = wtm_core::incidents::RecordedAction {
                at: event.unix_ms,
                pid: event.pid,
                name: event.name.as_ref().map(|n| n.chars().take(128).collect()),
                level: format!("{:?}", event.level),
                message: event.message.chars().take(2048).collect(),
            };
            if let Ok(body) = serde_json::to_string(&action)
                && body.len() <= 16384
            {
                self.conn.execute(
                    "INSERT INTO incident_action(at,body) VALUES(?1,?2)",
                    params![action.at as i64, body],
                )?;
            }
            self.incident_event = event.id;
        }
        self.conn.execute("DELETE FROM incident_action WHERE sequence NOT IN (SELECT sequence FROM incident_action ORDER BY sequence DESC LIMIT 16384)",[])?;
        Ok(())
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
        let slot = unix_secs.div_euclid(5);
        if self.incident_slot != Some(slot) {
            let at = if snap.taken_at_ms > 0 {
                snap.taken_at_ms
            } else {
                unix_secs.max(0) as u64 * 1000
            };
            let frame = wtm_core::incidents::Frame::of(snap, at);
            if let Ok(body) = serde_json::to_string(&frame)
                && body.len() <= 16384
            {
                self.conn.execute(
                    "INSERT INTO incident_frame(at,body) VALUES(?1,?2)",
                    params![at as i64, body],
                )?;
                self.incident_bytes
                    .set(self.incident_bytes.get() + body.len() as i64);
                if self.incident_bytes.get() > 256 * 1024 * 1024 {
                    let removed:i64 = self.conn.query_row("SELECT coalesce(sum(length(CAST(body AS BLOB))),0) FROM (SELECT body FROM incident_frame ORDER BY sequence LIMIT 512)",[],|r|r.get(0))?;
                    self.conn.execute("DELETE FROM incident_frame WHERE sequence IN (SELECT sequence FROM incident_frame ORDER BY sequence LIMIT 512)",[])?;
                    self.incident_bytes
                        .set(self.incident_bytes.get().saturating_sub(removed));
                }
            }
            self.incident_slot = Some(slot);
        }
        let minute = unix_secs.div_euclid(60);
        if self.minute.is_some_and(|m| m != minute) {
            self.flush()?;
        }
        self.minute = Some(minute);
        self.minute_samples += 1;

        // The time the sample's rates cover; untraced network is zero.
        let seconds = snap
            .interval
            .map_or(1.0, |d| d.as_secs_f64())
            .clamp(0.0, 3600.0);
        for p in snap.processes.iter().filter(|p| p.pid != 0) {
            let app = lower_name(&p.name, &mut self.lower);
            let add = |e: &mut Sample| {
                e.cpu += p.cpu_percent;
                e.gpu += p.gpu_percent;
                e.mem += p.private_working_set;
                e.count += 1;
                e.handles += u64::from(p.handle_count);
                e.threads += u64::from(p.thread_count);
                e.net_rx += p.net_rx_rate;
                e.net_tx += p.net_tx_rate;
            };
            match self.apps.get_mut(app) {
                Some(acc) => add(&mut acc.sample),
                None => add(&mut self.apps.entry(app.to_owned()).or_default().sample),
            }
        }
        for acc in self.apps.values_mut().filter(|a| a.sample.count > 0) {
            let Sample {
                cpu,
                gpu,
                mem,
                count,
                handles,
                threads,
                net_rx,
                net_tx,
            } = std::mem::take(&mut acc.sample);
            // Rates over the time this sample covers give bytes.
            acc.net_rx += net_rx * seconds;
            acc.net_tx += net_tx * seconds;
            acc.cpu_sum += f64::from(cpu);
            acc.cpu_peak = acc.cpu_peak.max(cpu);
            // Instances can share an engine; the total cannot exceed it.
            acc.gpu_sum += f64::from(gpu.min(100.0));
            acc.mem_sum += mem as f64;
            acc.mem_peak = acc.mem_peak.max(mem);
            acc.processes = acc.processes.max(count);
            acc.handles_sum += handles as f64;
            acc.threads_sum += threads as f64;
            acc.samples += 1;
        }
        Ok(())
    }

    /// Writes the current minute's aggregates. A minute already in the
    /// database (the service restarted within it, or the clock went back)
    /// is merged with them, weighted by sample count, not replaced.
    pub fn flush(&mut self) -> rusqlite::Result<()> {
        let Some(minute) = self.minute else {
            return Ok(());
        };
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO app_minute
                 (app, minute, cpu_avg, cpu_peak, mem_avg, mem_peak, processes, gpu_avg,
                  handles_avg, threads_avg, samples, net_rx_bytes, net_tx_bytes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT (app, minute) DO UPDATE SET
                  cpu_avg = (cpu_avg * MAX(samples, 1) + excluded.cpu_avg * excluded.samples)
                            / (MAX(samples, 1) + excluded.samples),
                  cpu_peak = MAX(cpu_peak, excluded.cpu_peak),
                  mem_avg = CAST((mem_avg * MAX(samples, 1) + excluded.mem_avg * excluded.samples)
                            / (MAX(samples, 1) + excluded.samples) AS INTEGER),
                  mem_peak = MAX(mem_peak, excluded.mem_peak),
                  processes = MAX(processes, excluded.processes),
                  gpu_avg = (gpu_avg * MAX(samples, 1) + excluded.gpu_avg * excluded.samples)
                            / (MAX(samples, 1) + excluded.samples),
                  handles_avg = CAST((handles_avg * MAX(samples, 1)
                                + excluded.handles_avg * excluded.samples)
                            / (MAX(samples, 1) + excluded.samples) AS INTEGER),
                  threads_avg = CAST((threads_avg * MAX(samples, 1)
                                + excluded.threads_avg * excluded.samples)
                            / (MAX(samples, 1) + excluded.samples) AS INTEGER),
                  samples = MAX(samples, 1) + excluded.samples,
                  net_rx_bytes = net_rx_bytes + excluded.net_rx_bytes,
                  net_tx_bytes = net_tx_bytes + excluded.net_tx_bytes",
            )?;
            // CPU and GPU are shares of the machine over the whole minute: an
            // app seen in 2 samples of 60 used a 30th of what it used then.
            // Memory, handles and threads describe the app while it ran.
            let samples = self.minute_samples.max(1);
            let minute_n = f64::from(samples);
            for (app, a) in self.apps.drain() {
                let n = f64::from(a.samples.max(1));
                stmt.execute(params![
                    app,
                    minute,
                    a.cpu_sum / minute_n,
                    f64::from(a.cpu_peak),
                    (a.mem_sum / n) as i64,
                    a.mem_peak as i64,
                    a.processes,
                    a.gpu_sum / minute_n,
                    (a.handles_sum / n) as i64,
                    (a.threads_sum / n) as i64,
                    samples,
                    a.net_rx.clamp(0.0, i64::MAX as f64) as i64,
                    a.net_tx.clamp(0.0, i64::MAX as f64) as i64,
                ])?;
            }
        }
        self.minute_samples = 0;
        tx.commit()
    }

    pub fn prune(&self, keep_days: u32, now_secs: i64) -> rusqlite::Result<usize> {
        self.conn.execute(
            "DELETE FROM incident_action WHERE at < ?1",
            [now_secs.saturating_sub(7 * 86400).saturating_mul(1000)],
        )?;
        self.conn.execute(
            "DELETE FROM incident_frame WHERE at < ?1",
            [now_secs.saturating_sub(7 * 86400).saturating_mul(1000)],
        )?;
        self.incident_bytes.set(self.conn.query_row(
            "SELECT coalesce(sum(length(CAST(body AS BLOB))),0) FROM incident_frame",
            [],
            |r| r.get(0),
        )?);

        let cutoff = now_secs.div_euclid(60) - i64::from(keep_days) * 24 * 60;
        self.conn
            .execute("DELETE FROM app_minute WHERE minute < ?1", params![cutoff])
    }
}

/// `name` in lower case, as `str::to_lowercase` gives it; process names
/// are almost always ASCII and often already lower case, which borrows
/// `name` or reuses `buf` instead of allocating.
fn lower_name<'a>(name: &'a str, buf: &'a mut String) -> &'a str {
    if !name
        .bytes()
        .any(|b| b.is_ascii_uppercase() || !b.is_ascii())
    {
        return name;
    }
    buf.clear();
    if name.is_ascii() {
        buf.extend(name.bytes().map(|b| char::from(b.to_ascii_lowercase())));
    } else {
        *buf = name.to_lowercase();
    }
    buf
}

/// Per-app summaries for rows at or after `since_minute`.
pub fn stats(
    conn: &Connection,
    since_minute: i64,
    logical_cpus: u32,
) -> rusqlite::Result<Vec<AppStats>> {
    // Steps from app to app along the primary key and reads each app's
    // window as one range: three times faster than looking every row of
    // the minute index up in the table, and rows arrive grouped by app.
    let mut stmt = conn.prepare(
        "WITH RECURSIVE apps(a) AS (
             SELECT min(app) FROM app_minute
             UNION ALL
             SELECT (SELECT min(app) FROM app_minute WHERE app > a) FROM apps
             WHERE a IS NOT NULL
         )
         SELECT app, minute, cpu_avg, cpu_peak, mem_avg, mem_peak, processes, gpu_avg,
                handles_avg, threads_avg, net_rx_bytes, net_tx_bytes
         FROM apps JOIN app_minute ON app = a AND minute >= ?1",
    )?;
    let mut out = Vec::new();
    let (mut app, mut minutes) = (String::new(), Vec::<MinuteRow>::new());
    let mut rows = stmt.query(params![since_minute])?;
    while let Some(r) = rows.next()? {
        // Borrowed from SQLite: no allocation per row.
        let name = r.get_ref(0)?.as_str()?;
        if name != app {
            out.extend(summarize(&app, &minutes, logical_cpus));
            minutes.clear();
            name.clone_into(&mut app);
        }
        minutes.push(row(r, 1)?);
    }
    out.extend(summarize(&app, &minutes, logical_cpus));
    out.sort_by(|a, b| b.cpu_avg.total_cmp(&a.cpu_avg));
    Ok(out)
}

/// Minute rows of one app since `since_minute`, oldest first.
pub fn series(conn: &Connection, app: &str, since_minute: i64) -> rusqlite::Result<Vec<MinuteRow>> {
    let mut stmt = conn.prepare(
        "SELECT minute, cpu_avg, cpu_peak, mem_avg, mem_peak, processes, gpu_avg,
                handles_avg, threads_avg, net_rx_bytes, net_tx_bytes
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
        gpu_avg: r.get::<_, f64>(first + 6)? as f32,
        handles_avg: r.get::<_, i64>(first + 7)?.clamp(0, i64::from(u32::MAX)) as u32,
        threads_avg: r.get::<_, i64>(first + 8)?.clamp(0, i64::from(u32::MAX)) as u32,
        net: wtm_policy::NetBytes {
            rx: r.get::<_, i64>(first + 9)?.max(0) as u64,
            tx: r.get::<_, i64>(first + 10)?.max(0) as u64,
        },
    })
}

/// At most 600 representative moments and 512 available action markers.
pub fn incidents(
    conn: &Connection,
    since_minutes: i64,
    hours: u32,
) -> rusqlite::Result<wtm_core::incidents::Replay> {
    let bucket = i64::from(hours.clamp(1, 168)) * 3_600_000 / 600;
    let mut query=conn.prepare("SELECT body FROM incident_frame WHERE sequence IN (SELECT max(sequence) FROM incident_frame WHERE at >= ?1 GROUP BY at / ?2) ORDER BY at LIMIT 600")?;
    let frames = query
        .query_map(
            params![since_minutes.saturating_mul(60000), bucket.max(5000)],
            |r| r.get::<_, String>(0),
        )?
        .map(|body| {
            serde_json::from_str(&body?).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut query = conn.prepare(
        "SELECT body FROM incident_action WHERE at >= ?1 ORDER BY at DESC,sequence DESC LIMIT 513",
    )?;
    let mut actions = query
        .query_map([since_minutes.saturating_mul(60000)], |r| {
            r.get::<_, String>(0)
        })?
        .map(|body| {
            serde_json::from_str(&body?).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })
        })
        .collect::<rusqlite::Result<Vec<wtm_core::incidents::RecordedAction>>>()?;
    let actions_truncated = actions.len() > 512;
    actions.truncate(512);
    actions.reverse();
    Ok(wtm_core::incidents::Replay {
        frames,
        actions,
        actions_truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wtm_core::{ProcessInfo, SystemStats};

    fn proc(pid: u32, name: &str, cpu: f32, mem_mb: u64) -> ProcessInfo {
        ProcessInfo {
            io: Default::default(),
            cycle_percent: 0.0,
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
            net_rx_rate: 0.0,
            net_tx_rate: 0.0,
            suspended: false,
            create_time: i64::from(pid),
            memory: Default::default(),
            gpu_detail: Default::default(),
        }
    }

    fn gpu_proc(pid: u32, gpu: f32) -> ProcessInfo {
        ProcessInfo {
            gpu_percent: gpu,
            ..proc(pid, "game.exe", 1.0, 10)
        }
    }

    fn snap(procs: Vec<ProcessInfo>) -> Snapshot {
        Snapshot {
            interval: Some(Duration::from_secs(1)),
            system: SystemStats::default(),
            processes: procs,
            net_traced: false,
            taken_at_ms: 0,
        }
    }

    #[test]
    fn incident_replay_persists_identity_actions_and_five_second_cadence() {
        let mut h = History::with(Connection::open_in_memory().unwrap()).unwrap();
        for second in 60..71 {
            h.record(&snap(vec![proc(7, "çalışma.exe", 33.0, 64)]), second)
                .unwrap();
        }
        let event = wtm_ipc::Event {
            id: 9,
            unix_ms: 65_000,
            level: wtm_ipc::EventLevel::Warn,
            pid: Some(7),
            name: Some("çalışma.exe".into()),
            message: "Applied efficiency mode".into(),
            notify: false,
        };
        h.record_actions(&[event]).unwrap();
        assert_eq!(h.last_incident_event(), 9);
        let replay = incidents(h.connection(), 1, 1).unwrap();
        let recorded: i64 = h
            .connection()
            .query_row("SELECT count(*) FROM incident_frame", [], |r| r.get(0))
            .unwrap();
        assert_eq!(recorded, 3, "one persisted frame every five seconds");
        // A one-hour replay uses six-second buckets and keeps each bucket's
        // latest frame. 60s and 65s therefore share one representative moment.
        assert_eq!(
            replay.frames.iter().map(|f| f.at).collect::<Vec<_>>(),
            [65_000, 70_000]
        );
        assert_eq!(replay.frames[0].processes[0].target.create_time, 7);
        assert_eq!(replay.frames[0].processes[0].target.name, "çalışma.exe");
        assert_eq!(replay.actions[0].message, "Applied efficiency mode");
        assert!(!replay.actions_truncated);
        assert!(incidents(h.connection(), 2, 1).unwrap().frames.is_empty());
        // Reopening the same connection represents a service restart, without
        // relying on a filesystem or allocating a large database.
        let h = History::with(h.conn).unwrap();
        let bytes: i64 = h
            .connection()
            .query_row(
                "SELECT sum(length(CAST(body AS BLOB))) FROM incident_frame",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(h.incident_bytes.get(), bytes);
        assert_eq!(incidents(h.connection(), 1, 1).unwrap().frames.len(), 2);
        h.prune(7, 8 * 86400).unwrap();
        let replay = incidents(h.connection(), 0, 168).unwrap();
        assert!(replay.frames.is_empty() && replay.actions.is_empty());
    }

    #[test]
    fn incident_recording_reports_storage_failure_without_advancing_its_cursor() {
        let mut h = History::with(Connection::open_in_memory().unwrap()).unwrap();
        h.connection().execute_batch("CREATE TRIGGER refuse_incident BEFORE INSERT ON incident_frame BEGIN SELECT RAISE(ABORT,'injected full disk'); END;").unwrap();
        assert!(
            h.record(&snap(vec![proc(1, "app.exe", 1.0, 1)]), 60)
                .is_err()
        );
        assert_eq!(h.incident_slot, None);
        h.connection()
            .execute_batch("DROP TRIGGER refuse_incident;")
            .unwrap();
        h.record(&snap(vec![proc(1, "app.exe", 1.0, 1)]), 60)
            .unwrap();
        assert_eq!(incidents(h.connection(), 1, 1).unwrap().frames.len(), 1);
    }

    #[test]
    fn an_app_seen_briefly_uses_its_share_of_the_minute() {
        let mut h = History::with(Connection::open_in_memory().unwrap()).unwrap();
        let base = 3_000 * 60;
        // 60 samples; the helper runs in 2 of them at 21 % of the machine.
        for s in 0..60 {
            let mut procs = vec![proc(1, "steady.exe", 3.0, 100)];
            if s < 2 {
                procs.push(proc(2, "helper.exe", 21.0, 50));
            }
            h.record(&snap(procs), base + s).unwrap();
        }
        h.flush().unwrap();
        let row = |app: &str| -> (f64, f64, i64) {
            h.connection()
                .query_row(
                    "SELECT cpu_avg, cpu_peak, mem_avg FROM app_minute WHERE app = ?1",
                    [app],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap()
        };
        assert_eq!(row("helper.exe"), (0.7, 21.0, 50 << 20));
        assert_eq!(row("steady.exe"), (3.0, 3.0, 100 << 20));
    }

    #[test]
    fn a_minute_written_twice_is_merged_by_sample_count() {
        let mut h = History::with(Connection::open_in_memory().unwrap()).unwrap();
        let base = 2_000 * 60;
        // One sample, flushed when the next minute starts.
        h.record(&snap(vec![proc(1, "a.exe", 10.0, 100)]), base)
            .unwrap();
        h.record(&snap(vec![]), base + 60).unwrap();
        // The clock goes back into the written minute: three more samples.
        for s in 0..3 {
            h.record(&snap(vec![proc(1, "a.exe", 2.0, 300)]), base + 10 + s)
                .unwrap();
        }
        h.flush().unwrap();

        let row: (f64, f64, i64, i64, i64) = h
            .connection()
            .query_row(
                "SELECT cpu_avg, cpu_peak, mem_avg, mem_peak, samples FROM app_minute
                 WHERE app = 'a.exe' AND minute = 2000",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        // (10 * 1 + 2 * 3) / 4 and (100 * 1 + 300 * 3) / 4 MiB.
        assert_eq!(row, (4.0, 10.0, 250 << 20, 300 << 20, 4));
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

        assert!(s.iter().all(|a| a.gpu_avg == 0.0));

        assert_eq!(h.prune(1, base + 60 + 2 * 24 * 3600).unwrap(), 3);
        assert!(stats(h.connection(), 0, 4).unwrap().is_empty());
    }

    #[test]
    fn gpu_is_summed_per_app_and_capped() {
        let mut h = History::with(Connection::open_in_memory().unwrap()).unwrap();
        h.record(&snap(vec![gpu_proc(1, 40.0), gpu_proc(2, 20.0)]), 60)
            .unwrap();
        h.record(&snap(vec![gpu_proc(1, 80.0), gpu_proc(2, 70.0)]), 90)
            .unwrap();
        h.flush().unwrap();
        let rows = series(h.connection(), "game.exe", 0).unwrap();
        assert!((rows[0].gpu_avg - 80.0).abs() < 1e-4); // (60 + 100) / 2
    }

    #[test]
    fn handles_and_threads_are_summed_per_app_and_averaged() {
        let mut h = History::with(Connection::open_in_memory().unwrap()).unwrap();
        let p = |pid, handles, threads| ProcessInfo {
            handle_count: handles,
            thread_count: threads,
            ..proc(pid, "svc.exe", 1.0, 10)
        };
        h.record(&snap(vec![p(1, 1000, 10), p(2, 500, 5)]), 60)
            .unwrap();
        h.record(&snap(vec![p(1, 1200, 12), p(2, 500, 5)]), 90)
            .unwrap();
        h.flush().unwrap();
        let rows = series(h.connection(), "svc.exe", 0).unwrap();
        assert_eq!((rows[0].handles_avg, rows[0].threads_avg), (1600, 16));
        let s = stats(h.connection(), 0, 4).unwrap();
        assert_eq!((s[0].handles_last, s[0].threads_last), (1600, 16));
    }

    #[test]
    fn network_bytes_add_up_per_app_and_across_a_restart() {
        let mut h = History::with(Connection::open_in_memory().unwrap()).unwrap();
        let p = |pid, rx, tx| ProcessInfo {
            net_rx_rate: rx,
            net_tx_rate: tx,
            ..proc(pid, "browser.exe", 1.0, 10)
        };
        // Two instances, two one-second samples: 2 × (1000 + 500) received.
        h.record(&snap(vec![p(1, 1000.0, 10.0), p(2, 500.0, 0.0)]), 60)
            .unwrap();
        h.record(&snap(vec![p(1, 1000.0, 10.0), p(2, 500.0, 0.0)]), 61)
            .unwrap();
        h.flush().unwrap();
        let rows = series(h.connection(), "browser.exe", 0).unwrap();
        assert_eq!(rows[0].net, wtm_policy::NetBytes { rx: 3000, tx: 20 });
        // The service restarts within the minute: its bytes are added.
        let mut again = History::with(h.conn).unwrap();
        let mut two_seconds = snap(vec![p(1, 100.0, 0.0)]);
        two_seconds.interval = Some(Duration::from_secs(2));
        again.record(&two_seconds, 62).unwrap();
        again.flush().unwrap();
        let s = stats(again.connection(), 0, 4).unwrap();
        assert_eq!(s[0].net, wtm_policy::NetBytes { rx: 3200, tx: 20 });
    }

    #[test]
    fn migrates_databases_without_a_gpu_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE app_minute (
                app TEXT NOT NULL, minute INTEGER NOT NULL,
                cpu_avg REAL NOT NULL, cpu_peak REAL NOT NULL,
                mem_avg INTEGER NOT NULL, mem_peak INTEGER NOT NULL,
                processes INTEGER NOT NULL, PRIMARY KEY (app, minute)
            ) WITHOUT ROWID;
            INSERT INTO app_minute VALUES ('old.exe', 5, 1.0, 2.0, 3, 4, 1);",
        )
        .unwrap();
        let h = History::with(conn).unwrap();
        let rows = series(h.connection(), "old.exe", 0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].gpu_avg, 0.0);
        assert_eq!((rows[0].handles_avg, rows[0].threads_avg), (0, 0));
        assert_eq!(rows[0].net, wtm_policy::NetBytes::default());
        // Opening again does not try to add the column twice.
        let conn = h.conn;
        History::with(conn).unwrap();
    }

    #[test]
    fn a_read_only_old_database_cannot_be_migrated() {
        let dir = std::env::temp_dir().join(format!("wtm-history-ro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.sqlite");
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                CREATE TABLE app_minute (
                    app TEXT NOT NULL, minute INTEGER NOT NULL,
                    cpu_avg REAL NOT NULL, cpu_peak REAL NOT NULL,
                    mem_avg INTEGER NOT NULL, mem_peak INTEGER NOT NULL,
                    processes INTEGER NOT NULL, PRIMARY KEY (app, minute)
                ) WITHOUT ROWID;
                CREATE INDEX app_minute_time ON app_minute (minute);",
            )
            .unwrap();
        let ro =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert!(History::with(ro).is_err());
        assert!(History::open(&path).is_ok(), "writable: migrated");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_are_lowered_like_to_lowercase() {
        let mut buf = String::new();
        for name in ["app.exe", "App.EXE", "ÄPFEL.exe", "ΟΔΟΣ.exe", ""] {
            assert_eq!(lower_name(name, &mut buf), name.to_lowercase());
        }
    }

    /// Timing: one sample of this PC per call, and a day of minute rows for
    /// 150 apps summarised. `cargo test -p wtm-service --release -- --ignored
    /// history_cost --nocapture`
    #[test]
    #[ignore = "timing"]
    fn history_cost() {
        let live = wtm_core::Sampler::new().sample().unwrap();
        let s = snap(live.processes);
        let mut h = History::with(Connection::open_in_memory().unwrap()).unwrap();
        let n = 2000;
        let t = std::time::Instant::now();
        for _ in 0..n {
            // All in one minute: no flush, only the per-sample work.
            h.record(&s, 60).unwrap();
        }
        let per = t.elapsed() / n;
        println!(
            "record: {per:?} per sample of {} processes",
            s.processes.len()
        );

        let tx = h.conn.transaction().unwrap();
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO app_minute (app, minute, cpu_avg, cpu_peak, mem_avg, mem_peak, processes)
                     VALUES (?1, ?2, 1.0, 2.0, 3, 4, 1)",
                )
                .unwrap();
            for app in 0..150 {
                for minute in 0..3 * 24 * 60 {
                    insert
                        .execute(params![format!("app-{app}.exe"), minute])
                        .unwrap();
                }
            }
        }
        tx.commit().unwrap();
        let t = std::time::Instant::now();
        for _ in 0..10 {
            assert_eq!(stats(h.connection(), 2 * 24 * 60, 8).unwrap().len(), 150);
        }
        println!(
            "stats: {:?} for the last of three days of 150 apps",
            t.elapsed() / 10
        );
    }

    #[test]
    fn query_and_write_errors_are_reported() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(stats(&conn, 0, 1).is_err(), "no table yet");
        assert!(series(&conn, "a.exe", 0).is_err());

        let mut h = History::with(conn).unwrap();
        h.flush().unwrap(); // nothing recorded yet
        // Samples without an interval carry no rates and are skipped.
        let first = Snapshot {
            interval: None,
            ..snap(vec![proc(1, "a.exe", 1.0, 1)])
        };
        h.record(&first, 60).unwrap();
        h.flush().unwrap();
        assert!(series(h.connection(), "a.exe", 0).unwrap().is_empty());

        h.connection()
            .execute_batch(
                "CREATE TRIGGER refuse BEFORE INSERT ON app_minute
                 BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
            )
            .unwrap();
        h.record(&snap(vec![proc(1, "a.exe", 1.0, 1)]), 60).unwrap();
        assert!(
            h.record(&snap(vec![]), 120).is_err(),
            "minute rollover flushes"
        );
        h.record(&snap(vec![proc(1, "a.exe", 1.0, 1)]), 180)
            .unwrap();
        assert!(h.flush().is_err());
    }
}

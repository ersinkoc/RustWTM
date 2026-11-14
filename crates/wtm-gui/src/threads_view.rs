//! A process's threads, as Process Explorer's and System Informer's Threads
//! tab: CPU, where each thread started, priority, state and context
//! switches. Read-only, and read again each second only while shown.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use eframe::egui::{self, RichText, Ui};
use wtm_core::control::ProcessRef;
use wtm_sys::process::{ProcessSnapshotter, ThreadInfo};
use wtm_sys::threads::{ModuleRange, symbolize, wait_reason_name, win32_start_address};

use crate::backend::Data;
use crate::processes::Intent;

/// One thread as the view shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadRow {
    pub tid: u32,
    /// With `tid`, identifies the thread across ID reuse.
    pub create_time: i64,
    /// Share of the machine's processor time since the previous read, as
    /// the process CPU column counts it.
    pub cpu_percent: f32,
    /// Processor time in 100 ns units.
    pub cpu_time: u64,
    pub context_switches: u32,
    /// Context switches since the previous read.
    pub switch_delta: u32,
    pub priority: i32,
    pub base_priority: i32,
    pub state: u32,
    pub wait_reason: u32,
    /// "module+0xoffset" of the thread's start routine, when readable.
    pub start: Option<String>,
}

/// The threads of the process being looked at.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadsPanel {
    pub target: ProcessRef,
    pub rows: Vec<ThreadRow>,
    /// Set when the threads could not be read; the process may be gone.
    pub error: Option<String>,
    /// Whether rows have been read at all yet.
    pub read: bool,
}

impl ThreadsPanel {
    pub fn new(target: ProcessRef) -> Self {
        Self {
            target,
            rows: Vec::new(),
            error: None,
            read: false,
        }
    }
}

/// Rows for `now`, with CPU and context switches since `before` over
/// `elapsed_secs` on `logical` processors. A thread not in `before` (new,
/// or a reused ID) shows no CPU yet.
pub fn rows(
    before: &[ThreadRow],
    now: &[ThreadInfo],
    elapsed_secs: f64,
    logical: u32,
    mut start: impl FnMut(&ThreadInfo) -> Option<String>,
) -> Vec<ThreadRow> {
    let capacity = elapsed_secs * 1e7 * f64::from(logical.max(1));
    now.iter()
        .map(|t| {
            let cpu_time = t.kernel_time + t.user_time;
            let prev = before
                .iter()
                .find(|b| b.tid == t.tid && b.create_time == t.create_time);
            let (cpu_percent, switch_delta) = match prev {
                Some(p) if capacity > 0.0 => (
                    (cpu_time.saturating_sub(p.cpu_time) as f64 / capacity * 100.0)
                        .clamp(0.0, 100.0) as f32,
                    t.context_switches.wrapping_sub(p.context_switches),
                ),
                _ => (0.0, 0),
            };
            ThreadRow {
                tid: t.tid,
                create_time: t.create_time,
                cpu_percent,
                cpu_time,
                context_switches: t.context_switches,
                switch_delta,
                priority: t.priority,
                base_priority: t.base_priority,
                state: t.state,
                wait_reason: t.wait_reason,
                start: start(t),
            }
        })
        .collect()
}

/// The worker's side: reads the threads of the shown process each tick.
#[derive(Default)]
pub struct ThreadReader {
    snapshotter: ProcessSnapshotter,
    /// The process the caches below belong to.
    target: Option<ProcessRef>,
    modules: Vec<ModuleRange>,
    /// Start routine per thread, resolved once when the thread first appears.
    starts: HashMap<(u32, i64), Option<String>>,
    last: Option<Instant>,
}

/// The panel stops being read this long after it was last drawn.
const WANTED_FOR_MS: u64 = 5_000;

impl ThreadReader {
    pub fn refresh(&mut self, data: &Mutex<Data>) {
        let (target, before) = {
            let mut d = data.lock().unwrap_or_else(PoisonError::into_inner);
            let wanted = wtm_core::hardware::unix_ms()
                .saturating_sub(d.threads_wanted_ms.load(Ordering::Relaxed))
                < WANTED_FOR_MS;
            if !wanted {
                d.threads = None;
            }
            match &d.threads {
                Some(panel) => (panel.target.clone(), panel.rows.clone()),
                None => {
                    drop(d);
                    *self = Self {
                        snapshotter: std::mem::take(&mut self.snapshotter),
                        ..Self::default()
                    };
                    return;
                }
            }
        };
        if self.target.as_ref() != Some(&target) {
            self.target = Some(target.clone());
            self.modules = wtm_core::process_modules(target.pid, target.create_time)
                .map(|list| {
                    list.into_iter()
                        .map(|m| ModuleRange {
                            name: m.name,
                            base: m.base,
                            size: m.size,
                        })
                        .collect()
                })
                .unwrap_or_default();
            self.starts.clear();
            self.last = None;
        }
        let now = Instant::now();
        let elapsed = self.last.map_or(0.0, |t| (now - t).as_secs_f64());
        self.last = Some(now);
        // Empty once the process is gone, even if its ID was reused.
        let read = self.snapshotter.threads_of(target.pid, target.create_time);
        let (modules, starts) = (&self.modules, &mut self.starts);
        let result = read.map(|threads| {
            starts.retain(|k, _| threads.iter().any(|t| (t.tid, t.create_time) == *k));
            rows(
                &before,
                &threads,
                elapsed,
                wtm_sys::system::logical_cpu_count(),
                |t| {
                    starts
                        .entry((t.tid, t.create_time))
                        .or_insert_with(|| start_of(t, modules))
                        .clone()
                },
            )
        });
        let mut d = data.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(panel) = d.threads.as_mut().filter(|p| p.target == target) {
            panel.read = true;
            match result {
                Ok(rows) if rows.is_empty() => {
                    panel.rows.clear();
                    panel.error = Some(crate::i18n::tr("The process has ended.").to_owned());
                }
                Ok(rows) => {
                    panel.rows = rows;
                    panel.error = None;
                }
                Err(e) => panel.error = Some(e.to_string()),
            }
        }
    }
}

/// Where a thread started: its start routine as module+offset. `None` when
/// the thread cannot be queried, which includes every thread of the System
/// process (pid 4).
fn start_of(t: &ThreadInfo, modules: &[ModuleRange]) -> Option<String> {
    match win32_start_address(t.tid) {
        Ok(address) if address != 0 => Some(symbolize(address, modules)),
        _ => None,
    }
}

/// Task Manager-style words for a `KTHREAD_STATE`.
fn state_label(state: u32) -> String {
    let word = match state {
        0 => "Initialized",
        1 => "Ready",
        2 => "Running",
        3 => "Standby",
        4 => "Terminated",
        5 => "Waiting",
        6 => "Transition",
        7 => "Deferred ready",
        8 => "Gate wait",
        9 => "Waiting for swap-in",
        other => return other.to_string(),
    };
    crate::i18n::tr(word).to_owned()
}

/// The panel under a process's details, while the Threads button has
/// opened it for this process.
pub fn show(ui: &mut Ui, data: &Data, sel: &ProcessRef, intents: &mut Vec<Intent>) {
    let Some(panel) = data.threads.as_ref().filter(|p| p.target == *sel) else {
        return;
    };
    // Read on while drawn; the worker stops once this goes stale.
    data.threads_wanted_ms
        .store(wtm_core::hardware::unix_ms(), Ordering::Relaxed);
    ui.horizontal(|ui| {
        ui.strong(crate::i18n::format(
            "Threads ({0})",
            &[panel.rows.len().to_string()],
        ));
        if ui.small_button(crate::i18n::tr("Hide threads")).clicked() {
            intents.push(Intent::Threads(None));
        }
    });
    if let Some(e) = &panel.error {
        crate::theme::colored(ui, crate::theme::BAD, e);
        return;
    }
    if !panel.read {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(crate::i18n::tr("Reading threads…"));
        });
        return;
    }
    let mut rows: Vec<&ThreadRow> = panel.rows.iter().collect();
    rows.sort_by(|a, b| {
        b.cpu_percent
            .total_cmp(&a.cpu_percent)
            .then(b.switch_delta.cmp(&a.switch_delta))
            .then(a.tid.cmp(&b.tid))
    });
    let shown = rows.len().min(SHOWN_THREADS);
    if shown < rows.len() {
        ui.weak(crate::i18n::format(
            "Showing the {0} busiest of {1} threads.",
            &[shown.to_string(), rows.len().to_string()],
        ));
    }
    // The inspector scrolls vertically; only wide rows scroll sideways.
    egui::ScrollArea::horizontal()
        .id_salt("threads_list")
        .show(ui, |ui| {
            egui::Grid::new("threads_grid")
                .num_columns(6)
                .striped(true)
                .spacing(egui::vec2(12.0, 2.0))
                .show(ui, |ui| {
                    // TID and CPU read the same in every language.
                    ui.weak("TID");
                    ui.weak("CPU");
                    ui.weak(crate::i18n::tr("Start address"));
                    ui.weak(crate::i18n::tr("State"));
                    ui.weak(crate::i18n::tr("Priority"))
                        .on_hover_text(crate::i18n::tr("Current priority (base priority)"));
                    ui.weak(crate::i18n::tr("Switches"))
                        .on_hover_text(crate::i18n::tr("Context switches since the last read"));
                    ui.end_row();
                    for t in rows.into_iter().take(shown) {
                        ui.label(RichText::new(t.tid.to_string()).monospace());
                        let time = crate::processes::cpu_time_text(
                            std::time::Duration::from_nanos(t.cpu_time.saturating_mul(100)),
                        );
                        let cpu = if t.cpu_percent >= 0.01 {
                            format!("{:.2}%", t.cpu_percent)
                        } else {
                            String::new()
                        };
                        ui.label(cpu).on_hover_text(crate::i18n::format(
                            "Processor time: {0}",
                            std::slice::from_ref(&time),
                        ));
                        match &t.start {
                            Some(s) => ui.label(RichText::new(s).monospace().small()),
                            None => ui.weak("—").on_hover_text(crate::i18n::tr(
                                "Not readable with this window's rights",
                            )),
                        };
                        let state = ui.label(state_label(t.state));
                        if t.state == 5 {
                            state.on_hover_text(crate::i18n::format(
                                "Wait reason: {0}",
                                &[wait_reason_name(t.wait_reason)],
                            ));
                        }
                        ui.label(format!("{} ({})", t.priority, t.base_priority));
                        ui.label(format!("+{}", t.switch_delta)).on_hover_text(
                            crate::i18n::format(
                                "Total: {0}",
                                &[crate::perf_pages::count(f64::from(t.context_switches))],
                            ),
                        );
                        ui.end_row();
                    }
                });
        });
}

/// Rows drawn at most; busy processes have hundreds of threads.
const SHOWN_THREADS: usize = 50;

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(tid: u32, create_time: i64, cpu: u64, switches: u32) -> ThreadInfo {
        ThreadInfo {
            tid,
            create_time,
            user_time: cpu,
            context_switches: switches,
            ..Default::default()
        }
    }

    #[test]
    fn cpu_and_switches_count_since_the_previous_read() {
        let first = rows(&[], &[thread(1, 10, 1_000, 5)], 0.0, 4, |_| None);
        assert_eq!(first[0].cpu_percent, 0.0, "no interval yet");
        // One second on four processors is 4e7 units; 1e7 of them is 25%.
        let second = rows(
            &first,
            &[thread(1, 10, 1_000 + 10_000_000, 9)],
            1.0,
            4,
            |_| None,
        );
        assert!((second[0].cpu_percent - 25.0).abs() < 0.01);
        assert_eq!(second[0].switch_delta, 4);
        // A reused thread ID with a new creation time starts over.
        let reused = rows(&second, &[thread(1, 99, 50_000_000, 1)], 1.0, 4, |_| None);
        assert_eq!((reused[0].cpu_percent, reused[0].switch_delta), (0.0, 0));
    }

    #[test]
    fn start_addresses_are_asked_for_each_thread() {
        let mut asked = Vec::new();
        let out = rows(
            &[],
            &[thread(7, 1, 0, 0), thread(8, 1, 0, 0)],
            0.0,
            1,
            |t| {
                asked.push(t.tid);
                (t.tid == 7).then(|| "app.exe+0x10".to_owned())
            },
        );
        assert_eq!(asked, [7, 8]);
        assert_eq!(out[0].start.as_deref(), Some("app.exe+0x10"));
        assert_eq!(out[1].start, None);
    }

    #[test]
    fn this_process_lists_its_own_thread_with_a_start_routine() {
        let data = Mutex::new(Data::default());
        let me = ProcessRef {
            pid: std::process::id(),
            create_time: 0,
            name: String::new(),
        };
        {
            let mut d = data.lock().unwrap();
            d.threads = Some(ThreadsPanel::new(me.clone()));
            d.threads_wanted_ms
                .store(wtm_core::hardware::unix_ms(), Ordering::Relaxed);
        }
        let mut reader = ThreadReader::default();
        reader.refresh(&data);
        let tid = wtm_sys::threads::current_thread_id();
        {
            let d = data.lock().unwrap();
            let panel = d.threads.as_ref().unwrap();
            assert!(panel.read && panel.error.is_none(), "{:?}", panel.error);
            let row = panel.rows.iter().find(|r| r.tid == tid).unwrap();
            assert!(
                row.start.as_deref().is_some_and(|s| s.contains("+0x")),
                "{row:?}"
            );
        }
        // Once the panel is no longer drawn, reading stops and it is dropped.
        data.lock()
            .unwrap()
            .threads_wanted_ms
            .store(0, Ordering::Relaxed);
        reader.refresh(&data);
        assert!(data.lock().unwrap().threads.is_none());
        assert!(reader.target.is_none());

        // A process ID now used by another process reads as ended.
        let reused = ProcessRef {
            create_time: 1,
            ..me
        };
        {
            let mut d = data.lock().unwrap();
            d.threads = Some(ThreadsPanel::new(reused));
            d.threads_wanted_ms
                .store(wtm_core::hardware::unix_ms(), Ordering::Relaxed);
        }
        reader.refresh(&data);
        let d = data.lock().unwrap();
        let panel = d.threads.as_ref().unwrap();
        assert!(panel.rows.is_empty() && panel.error.is_some());
    }
}

//! The Processes tab: sortable flat or tree table with a context menu.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder};
use wtm_core::control::{EcoQos, Limits, PriorityClass, ProcessRef};
use wtm_core::{ProcessInfo, Snapshot, format_bytes};
use wtm_ipc::{Command, Managed};

pub type ManagedMap<'a> = HashMap<(u32, i64), &'a Managed>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Name,
    Pid,
    Cpu,
    Memory,
    Disk,
    Threads,
    Handles,
    Gpu,
}

const COLUMNS: [(Col, &str); 8] = [
    (Col::Name, "Name"),
    (Col::Pid, "PID"),
    (Col::Cpu, "CPU"),
    (Col::Memory, "Memory"),
    (Col::Disk, "Disk"),
    (Col::Gpu, "GPU"),
    (Col::Threads, "Threads"),
    (Col::Handles, "Handles"),
];

/// What the user asked for this frame; handled by the app.
pub enum Intent {
    Select(ProcessRef),
    Command(ProcessRef, Command),
    /// Destructive: ask before sending.
    Confirm(ProcessRef, Command),
}

pub struct ProcessView {
    pub search: String,
    pub tree: bool,
    pub sort: Col,
    pub descending: bool,
    pub selected: Option<ProcessRef>,
}

impl Default for ProcessView {
    fn default() -> Self {
        Self {
            search: String::new(),
            tree: false,
            sort: Col::Cpu,
            descending: true,
            selected: None,
        }
    }
}

fn compare(a: &ProcessInfo, b: &ProcessInfo, col: Col) -> Ordering {
    match col {
        Col::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        Col::Pid => a.pid.cmp(&b.pid),
        Col::Cpu => a.cpu_percent.total_cmp(&b.cpu_percent),
        Col::Memory => a.private_working_set.cmp(&b.private_working_set),
        Col::Disk => a.io_rate().total_cmp(&b.io_rate()),
        Col::Threads => a.thread_count.cmp(&b.thread_count),
        Col::Handles => a.handle_count.cmp(&b.handle_count),
        Col::Gpu => a.gpu_percent.total_cmp(&b.gpu_percent),
    }
    .then(a.pid.cmp(&b.pid))
}

/// Depth-first order with siblings sorted; orphans become roots.
fn tree_rows(procs: &[ProcessInfo], col: Col, descending: bool) -> Vec<(&ProcessInfo, usize)> {
    let index: HashMap<u32, usize> = procs.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut roots = Vec::new();
    for (i, p) in procs.iter().enumerate() {
        match index.get(&p.parent_pid) {
            Some(&parent) if parent != i && procs[parent].create_time <= p.create_time => {
                children.entry(parent).or_default().push(i);
            }
            _ => roots.push(i),
        }
    }
    let sort = |v: &mut Vec<usize>| {
        v.sort_by(|&a, &b| {
            let o = compare(&procs[a], &procs[b], col);
            if descending { o.reverse() } else { o }
        });
    };
    sort(&mut roots);
    children.values_mut().for_each(sort);

    let mut out = Vec::with_capacity(procs.len());
    let mut seen = HashSet::new();
    let mut stack: Vec<(usize, usize)> = roots.into_iter().rev().map(|i| (i, 0)).collect();
    while let Some((i, depth)) = stack.pop() {
        if !seen.insert(i) {
            continue;
        }
        out.push((&procs[i], depth));
        if let Some(kids) = children.get(&i) {
            stack.extend(kids.iter().rev().map(|&k| (k, depth + 1)));
        }
    }
    out
}

fn right(ui: &mut Ui, text: impl Into<egui::WidgetText>) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(text);
    });
}

impl ProcessView {
    pub fn ui(&mut self, ui: &mut Ui, snap: &Snapshot, managed: &ManagedMap<'_>) -> Vec<Intent> {
        let mut intents = Vec::new();
        let logical_cpus = snap.system.logical_cpus.max(1);

        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("name or PID")
                    .desired_width(220.0),
            );
            if !self.search.is_empty() && ui.small_button("✖").clicked() {
                self.search.clear();
            }
            ui.checkbox(&mut self.tree, "Tree view");
            ui.label(format!("{} processes", snap.processes.len()));
        });
        ui.separator();

        let filter = self.search.trim().to_lowercase();
        let rows: Vec<(&ProcessInfo, usize)> = if self.tree && filter.is_empty() {
            tree_rows(&snap.processes, self.sort, self.descending)
        } else {
            let mut v: Vec<&ProcessInfo> = snap
                .processes
                .iter()
                .filter(|p| {
                    filter.is_empty()
                        || p.name.to_lowercase().contains(&filter)
                        || p.pid.to_string().contains(&filter)
                })
                .collect();
            v.sort_by(|a, b| {
                let o = compare(a, b, self.sort);
                if self.descending { o.reverse() } else { o }
            });
            v.into_iter().map(|p| (p, 0)).collect()
        };

        let row_height = ui.spacing().interact_size.y;
        let one_core = 100.0 / logical_cpus as f32;
        let selected = self.selected.as_ref().map(|r| (r.pid, r.create_time));

        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::initial(240.0).at_least(140.0).clip(true))
            .column(Column::initial(64.0))
            .column(Column::initial(64.0))
            .column(Column::initial(92.0))
            .column(Column::initial(92.0))
            .column(Column::initial(60.0))
            .column(Column::initial(64.0))
            .column(Column::initial(70.0))
            .column(Column::remainder().at_least(120.0).clip(true))
            .header(row_height, |mut header| {
                for (col, label) in COLUMNS {
                    header.col(|ui| {
                        let arrow = match (self.sort == col, self.descending) {
                            (true, true) => " ↓",
                            (true, false) => " ↑",
                            _ => "",
                        };
                        if ui
                            .selectable_label(
                                self.sort == col,
                                RichText::new(format!("{label}{arrow}")).strong(),
                            )
                            .clicked()
                        {
                            if self.sort == col {
                                self.descending = !self.descending;
                            } else {
                                self.sort = col;
                                self.descending = !matches!(col, Col::Name | Col::Pid);
                            }
                        }
                    });
                }
                header.col(|ui| {
                    ui.strong("Limits");
                });
            })
            .body(|body| {
                body.rows(row_height, rows.len(), |mut row| {
                    let (p, depth) = rows[row.index()];
                    let key = (p.pid, p.create_time);
                    row.set_selected(selected == Some(key));

                    row.col(|ui| {
                        ui.add_space(depth as f32 * 14.0);
                        ui.label(&p.name);
                    });
                    row.col(|ui| right(ui, p.pid.to_string()));
                    row.col(|ui| {
                        let text = RichText::new(format!("{:.1}%", p.cpu_percent));
                        right(
                            ui,
                            if p.pid != 0 && p.cpu_percent >= one_core {
                                text.color(Color32::from_rgb(230, 140, 40))
                            } else {
                                text
                            },
                        );
                    });
                    row.col(|ui| right(ui, format_bytes(p.private_working_set as f64)));
                    row.col(|ui| right(ui, format!("{}/s", format_bytes(p.io_rate()))));
                    row.col(|ui| right(ui, format!("{:.1}%", p.gpu_percent)));
                    row.col(|ui| right(ui, p.thread_count.to_string()));
                    row.col(|ui| right(ui, p.handle_count.to_string()));
                    row.col(|ui| {
                        if let Some(m) = managed.get(&key) {
                            let sources: Vec<String> =
                                m.sources.iter().map(|s| format!("{s:?}")).collect();
                            ui.label(
                                RichText::new(format!("{}: {}", sources.join("+"), m.summary))
                                    .color(Color32::from_rgb(230, 140, 40)),
                            );
                        }
                    });

                    let response = row.response();
                    let target = ProcessRef::from(p);
                    if response.clicked() {
                        intents.push(Intent::Select(target.clone()));
                    }
                    response
                        .context_menu(|ui| context_menu(ui, &target, logical_cpus, &mut intents));
                });
            });

        intents
    }
}

pub fn context_menu(ui: &mut Ui, t: &ProcessRef, logical_cpus: u32, intents: &mut Vec<Intent>) {
    let mut cmd = |ui: &mut Ui, label: &str, command: Command, confirm: bool| {
        if ui.button(label).clicked() {
            intents.push(if confirm {
                Intent::Confirm(t.clone(), command)
            } else {
                Intent::Command(t.clone(), command)
            });
            ui.close();
        }
    };

    cmd(ui, "End task", Command::Kill, true);
    cmd(ui, "End process tree", Command::KillTree, true);
    ui.separator();
    cmd(ui, "Suspend", Command::Suspend, false);
    cmd(ui, "Resume", Command::Resume, false);
    ui.separator();
    ui.menu_button("Priority", |ui| {
        for (label, p) in [
            ("Idle", PriorityClass::Idle),
            ("Below normal", PriorityClass::BelowNormal),
            ("Normal", PriorityClass::Normal),
            ("Above normal", PriorityClass::AboveNormal),
            ("High", PriorityClass::High),
        ] {
            cmd(ui, label, Command::Priority(p), false);
        }
    });
    ui.menu_button("Efficiency mode", |ui| {
        cmd(ui, "On", Command::EcoQos(EcoQos::On), false);
        cmd(
            ui,
            "Off (let Windows decide)",
            Command::EcoQos(EcoQos::SystemManaged),
            false,
        );
    });
    ui.menu_button("Limit CPU", |ui| {
        for (label, cores) in [
            ("¼ core", 0.25),
            ("½ core", 0.5),
            ("1 core", 1.0),
            ("2 cores", 2.0),
            ("4 cores", 4.0),
        ] {
            if cores > f64::from(logical_cpus) {
                continue;
            }
            let percent = (cores / f64::from(logical_cpus) * 100.0) as f32;
            let limits = Limits {
                cpu_percent: Some(percent),
                memory_bytes: None,
            };
            cmd(
                ui,
                &format!("{label}  ({percent:.2}% of machine)"),
                Command::Limit(limits),
                false,
            );
        }
    });
    ui.menu_button("Limit memory", |ui| {
        for mb in [256u64, 512, 1024, 2048, 4096, 8192] {
            let limits = Limits {
                cpu_percent: None,
                memory_bytes: Some(mb << 20),
            };
            cmd(
                ui,
                &format_bytes((mb << 20) as f64),
                Command::Limit(limits),
                false,
            );
        }
    });
    cmd(ui, "Remove limits", Command::Unlimit, false);
    ui.separator();
    if ui.button("Properties").clicked() {
        intents.push(Intent::Select(t.clone()));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn p(pid: u32, parent_pid: u32, cpu: f32, create_time: i64) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid,
            name: format!("p{pid}"),
            cpu_percent: cpu,
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
            create_time,
        }
    }

    #[test]
    fn tree_orders_children_under_parents() {
        let procs = [
            p(0, 0, 50.0, 0),
            p(4, 0, 1.0, 1),
            p(10, 4, 1.0, 10),
            p(11, 10, 9.0, 11),
            p(12, 10, 3.0, 12),
            p(13, 99, 7.0, 13), // orphan
            p(14, 12, 0.0, 5),  // parent PID reused: not a real child
        ];
        let order: Vec<(u32, usize)> = tree_rows(&procs, Col::Cpu, true)
            .into_iter()
            .map(|(p, d)| (p.pid, d))
            .collect();
        assert_eq!(
            order,
            [(0, 0), (4, 1), (10, 2), (11, 3), (12, 3), (13, 0), (14, 0)]
        );
    }
}

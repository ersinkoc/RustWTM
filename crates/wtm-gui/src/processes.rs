//! The Processes tab: sortable flat or tree table with a context menu.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder};
use wtm_core::control::{EcoQos, Limits, PriorityClass, ProcessRef};
use wtm_core::{ProcessInfo, Snapshot, format_bytes};
use wtm_ipc::Command;

/// The Limits column text per process instance.
pub type ManagedMap = HashMap<(u32, i64), String>;

/// Short form of an app limit's budgets, e.g. `2 cores, 8 GB`.
pub fn budget_label(limit: &wtm_policy::AppLimit) -> String {
    let mut parts = Vec::new();
    if let Some(c) = limit.cpu_cores {
        parts.push(format!("{c} cores"));
    }
    if let Some(m) = limit.memory_gb {
        parts.push(format!("{m} GB"));
    }
    parts.join(", ")
}

/// Labels for processes the service limits: its own actions and app limits.
pub fn limit_labels(status: Option<&wtm_ipc::Status>) -> ManagedMap {
    let mut out = ManagedMap::new();
    let Some(status) = status else { return out };
    for m in &status.managed {
        let sources: Vec<String> = m.sources.iter().map(|s| format!("{s:?}")).collect();
        out.insert(
            (m.target.pid, m.target.create_time),
            format!("{}: {}", sources.join("+"), m.summary),
        );
    }
    for g in &status.app_limits {
        let label = format!("App limit: {}", budget_label(&g.limit));
        for key in &g.members {
            out.entry(*key)
                .and_modify(|l| {
                    l.push_str("; ");
                    l.push_str(&label);
                })
                .or_insert_with(|| label.clone());
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Col {
    Name,
    Pid,
    Cpu,
    Memory,
    Commit,
    WorkingSet,
    Disk,
    Gpu,
    GpuMemory,
    Threads,
    Handles,
    Session,
    Priority,
    Started,
    Limits,
    CommandLine,
}

/// Column, header, initial width, numeric (sorts largest first), shown by default.
const COLUMNS: [(Col, &str, f32, bool, bool); 16] = [
    (Col::Name, "Name", 240.0, false, true),
    (Col::Pid, "PID", 64.0, false, true),
    (Col::Cpu, "CPU", 64.0, true, true),
    (Col::Memory, "Memory", 92.0, true, true),
    (Col::Commit, "Commit", 92.0, true, false),
    (Col::WorkingSet, "Working set", 92.0, true, false),
    (Col::Disk, "Disk", 92.0, true, true),
    (Col::Gpu, "GPU", 60.0, true, true),
    (Col::GpuMemory, "GPU memory", 92.0, true, false),
    (Col::Threads, "Threads", 64.0, true, true),
    (Col::Handles, "Handles", 70.0, true, true),
    (Col::Session, "Session", 64.0, false, false),
    (Col::Priority, "Base priority", 70.0, true, false),
    (Col::Started, "Running for", 96.0, true, false),
    (Col::Limits, "Limits", 160.0, false, true),
    (Col::CommandLine, "Command line", 400.0, false, true),
];

/// A 16 px program icon (or the same space when there is none). Textures are
/// kept in egui's memory per image path, so each icon is uploaded once.
pub fn app_icon(ui: &mut Ui, icon: Option<(&str, &wtm_core::IconImage)>) {
    let Some((path, image)) = icon else {
        ui.add_space(20.0);
        return;
    };
    let id = egui::Id::unique(("app-icon", path));
    let texture = ui
        .data(|d| d.get_temp::<egui::TextureHandle>(id))
        .unwrap_or_else(|| {
            let t = ui.ctx().load_texture(
                path,
                egui::ColorImage::from_rgba_unmultiplied([image.size as usize; 2], &image.rgba),
                egui::TextureOptions::LINEAR,
            );
            ui.data_mut(|d| d.insert_temp(id, t.clone()));
            t
        });
    ui.add(egui::Image::new(&texture).fit_to_exact_size(egui::vec2(16.0, 16.0)));
}

/// Text shown per process besides its own fields.
pub struct Extra<'a> {
    pub labels: &'a ManagedMap,
    pub command_lines: &'a crate::backend::CommandLines,
    pub image_paths: &'a crate::backend::CommandLines,
    pub icons: &'a HashMap<String, Option<std::sync::Arc<wtm_core::IconImage>>>,
}

impl Extra<'_> {
    fn label(&self, p: &ProcessInfo) -> &str {
        self.labels
            .get(&(p.pid, p.create_time))
            .map_or("", String::as_str)
    }

    /// The image path and its icon, once both are known.
    pub fn icon(&self, p: &ProcessInfo) -> Option<(&str, &wtm_core::IconImage)> {
        let path = self.image_paths.get(&(p.pid, p.create_time))?;
        let icon = self.icons.get(path)?.as_deref()?;
        Some((path, icon))
    }

    fn command_line(&self, p: &ProcessInfo) -> &str {
        self.command_lines
            .get(&(p.pid, p.create_time))
            .map_or("", String::as_str)
    }
}

/// What the user asked for this frame; handled by the app.
pub enum Intent {
    Select(ProcessRef),
    Command(ProcessRef, Command),
    /// Destructive: ask before sending.
    Confirm(ProcessRef, Command),
    /// Show the executable in Explorer.
    OpenLocation(ProcessRef),
    /// Open the Limits tab with a new limit for this image name.
    LimitApp(String),
    /// Select the process and show it in the Processes tab.
    Reveal(ProcessRef),
    /// Open the Kill list tab with a new entry for this image name.
    AddToKillList(String),
    /// Destructive, for several processes (every instance of an app).
    ConfirmMany(Vec<ProcessRef>, Command),
    /// Open the Ports tab.
    ShowPorts,
    /// Write a full memory dump of the process.
    Dump(ProcessRef),
    /// Notify when the process exits.
    WatchExit(ProcessRef),
}

/// How the process list is arranged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ViewMode {
    List,
    Tree,
    /// One row per app (image name), expandable to its processes.
    Apps,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QuickFilter {
    #[default]
    All,
    Cpu,
    Memory,
    Managed,
}

pub struct ProcessView {
    pub search: String,
    pub mode: ViewMode,
    /// Apps expanded in the Apps view (lower-case names).
    pub expanded: HashSet<String>,
    pub sort: Col,
    pub descending: bool,
    pub selected: Option<ProcessRef>,
    /// Columns the user turned on or off (Name is always shown).
    pub visible: HashSet<Col>,
    /// Shade CPU, memory, disk and GPU cells by how much they use.
    pub heat: bool,
    pub filter: QuickFilter,
    /// Only the process table is frozen. Sampling, protection and alerts continue.
    pub paused: Option<std::sync::Arc<Snapshot>>,
}

impl Default for ProcessView {
    fn default() -> Self {
        Self {
            search: String::new(),
            mode: ViewMode::List,
            expanded: HashSet::new(),
            sort: Col::Cpu,
            descending: true,
            selected: None,
            visible: COLUMNS.iter().filter(|c| c.4).map(|c| c.0).collect(),
            heat: true,
            filter: QuickFilter::All,
            paused: None,
        }
    }
}

impl ProcessView {
    /// Whether the column with header `label` is shown.
    #[cfg(test)]
    pub fn view_has(&self, label: &str) -> bool {
        COLUMNS
            .iter()
            .any(|c| c.1 == label && (c.0 == Col::Name || self.visible.contains(&c.0)))
    }
}

fn compare(a: &ProcessInfo, b: &ProcessInfo, col: Col, extra: &Extra<'_>) -> Ordering {
    match col {
        Col::Name => crate::sort::text(&a.name, &b.name),
        Col::Pid => a.pid.cmp(&b.pid),
        Col::Cpu => a.cpu_percent.total_cmp(&b.cpu_percent),
        Col::Memory => a.private_working_set.cmp(&b.private_working_set),
        Col::Commit => a.private_bytes.cmp(&b.private_bytes),
        Col::WorkingSet => a.working_set.cmp(&b.working_set),
        Col::Disk => a.io_rate().total_cmp(&b.io_rate()),
        Col::Gpu => a.gpu_percent.total_cmp(&b.gpu_percent),
        Col::GpuMemory => a.gpu_memory.cmp(&b.gpu_memory),
        Col::Threads => a.thread_count.cmp(&b.thread_count),
        Col::Handles => a.handle_count.cmp(&b.handle_count),
        Col::Session => a.session_id.cmp(&b.session_id),
        Col::Priority => a.base_priority.cmp(&b.base_priority),
        // Longest running first when descending.
        Col::Started => b.create_time.cmp(&a.create_time),
        Col::Limits => crate::sort::text(extra.label(a), extra.label(b)),
        Col::CommandLine => crate::sort::text(extra.command_line(a), extra.command_line(b)),
    }
    .then(a.pid.cmp(&b.pid))
}

/// `3d 04h`, `2h 05m`, `4m 10s`.
fn running_for(p: &ProcessInfo) -> String {
    let Some(secs) = p
        .start_time()
        .and_then(|s| std::time::SystemTime::now().duration_since(s).ok())
        .map(|d| d.as_secs())
    else {
        return String::new();
    };
    match secs {
        s if s >= 86_400 => format!("{}d {:02}h", s / 86_400, s % 86_400 / 3600),
        s if s >= 3600 => format!("{}h {:02}m", s / 3600, s % 3600 / 60),
        s => format!("{}m {:02}s", s / 60, s % 60),
    }
}
/// Depth-first order with siblings sorted; orphans become roots.
fn tree_rows<'a>(
    procs: &'a [ProcessInfo],
    col: Col,
    descending: bool,
    extra: &Extra<'_>,
) -> Vec<(&'a ProcessInfo, usize)> {
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
            let o = compare(&procs[a], &procs[b], col, extra);
            if descending { o.reverse() } else { o }
        });
    };
    sort(&mut roots);
    children.values_mut().for_each(sort);

    let mut out = Vec::with_capacity(procs.len());
    let mut seen = HashSet::new();
    // Processes that are only reachable through a parent cycle (same create
    // time, PIDs reused) are appended as extra roots so none goes missing.
    for start in roots.into_iter().chain(0..procs.len()) {
        let mut stack = vec![(start, 0)];
        while let Some((i, depth)) = stack.pop() {
            if !seen.insert(i) {
                continue;
            }
            out.push((&procs[i], depth));
            if let Some(kids) = children.get(&i) {
                stack.extend(kids.iter().rev().map(|&k| (k, depth + 1)));
            }
        }
    }
    out
}

/// Background of a heat-map cell for a share `load` (0..=1) of the machine;
/// `None` for (almost) idle cells. Small loads stay visible, as in Task
/// Manager: the square root spreads them out.
pub fn heat_color(load: f64) -> Option<Color32> {
    // Also rejects NaN.
    if load.is_nan() || load < 0.002 {
        return None;
    }
    let f = load.min(1.0).sqrt() as f32;
    let green = (200.0 - 150.0 * f) as u8;
    let alpha = (36.0 + 150.0 * f) as u8;
    Some(Color32::from_rgba_unmultiplied(255, green, 40, alpha))
}

/// Disk I/O that counts as a fully busy cell, bytes per second.
const DISK_HEAT_FULL: f64 = 100.0 * 1024.0 * 1024.0;

fn right(ui: &mut Ui, text: impl Into<egui::WidgetText>) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(text);
    });
}

impl ProcessView {
    /// Up / Down move the selection, Delete asks to end the selected process.
    /// Ignored while a text field (the search), a menu or a dialog has the
    /// keyboard.
    fn keyboard(
        &self,
        ui: &Ui,
        rows: &[(&ProcessInfo, usize, Option<usize>)],
        intents: &mut Vec<Intent>,
    ) {
        let ctx = ui.ctx();
        if ctx.text_edit_focused()
            || egui::Popup::is_any_open(ctx)
            || ctx.memory(|m| m.top_modal_layer().is_some())
        {
            return;
        }
        let (up, down, delete) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Delete),
            )
        });
        let procs: Vec<&ProcessInfo> = rows.iter().filter(|r| r.2.is_none()).map(|r| r.0).collect();
        let current = self.selected.as_ref().and_then(|s| {
            procs
                .iter()
                .position(|p| p.pid == s.pid && p.create_time == s.create_time)
        });
        let next = match (current, up, down) {
            (Some(i), true, _) => Some(i.saturating_sub(1)),
            (Some(i), _, true) => Some((i + 1).min(procs.len().saturating_sub(1))),
            (None, true, _) | (None, _, true) => (!procs.is_empty()).then_some(0),
            _ => None,
        };
        if let Some(i) = next
            && current != Some(i)
        {
            intents.push(Intent::Select(ProcessRef::from(procs[i])));
        }
        if delete && let Some(i) = current {
            intents.push(Intent::Confirm(ProcessRef::from(procs[i]), Command::Kill));
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, snap: &Snapshot, extra: &Extra<'_>) -> Vec<Intent> {
        let mut intents = Vec::new();
        let logical_cpus = snap.system.logical_cpus.max(1);

        ui.horizontal_wrapped(|ui| {
            ui.label("Search:");
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .id(egui::Id::unique("process_search"))
                    .hint_text("name, PID, path or command line (Ctrl+F)")
                    .desired_width(240.0),
            );
            if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
                search.request_focus();
            }
            // TextEdit gives up focus on Escape in the same frame.
            if (search.has_focus() || search.lost_focus())
                && ui.input(|i| i.key_pressed(egui::Key::Escape))
            {
                self.search.clear();
            }
            if !self.search.is_empty() && ui.small_button("✖").clicked() {
                self.search.clear();
            }
            for (mode, label) in [
                (ViewMode::List, "List"),
                (ViewMode::Tree, "Tree"),
                (ViewMode::Apps, "Apps"),
            ] {
                ui.selectable_value(&mut self.mode, mode, label);
            }
            ui.menu_button("Columns", |ui| {
                ui.checkbox(&mut self.heat, "Heat map")
                    .on_hover_text("Shade busy CPU, memory, disk and GPU cells");
                ui.separator();
                for (col, label, ..) in COLUMNS.iter().skip(1) {
                    let mut on = self.visible.contains(col);
                    if ui.checkbox(&mut on, *label).changed() {
                        if on {
                            self.visible.insert(*col);
                        } else {
                            self.visible.remove(col);
                        }
                    }
                }
            });
            let toggle_key = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F6))
                && !ui.ctx().text_edit_focused()
                && !egui::Popup::is_any_open(ui.ctx())
                && !ui.memory(|m| m.top_modal_layer().is_some());
            if ui.button(if self.paused.is_some() { "Resume updates" } else { "Pause updates" })
                .on_hover_text("Freeze this process list for inspection (F6). Monitoring, alerts and protection continue.")
                .clicked() || toggle_key {
                self.paused = if self.paused.is_some() { None } else { Some(std::sync::Arc::new(snap.clone())) };
            }
        });
        let paused = self.paused.clone();
        let snap = paused.as_deref().unwrap_or(snap);
        ui.horizontal_wrapped(|ui| {
            for (filter, label, hint) in [
                (QuickFilter::All, "All processes", "Every process"),
                (
                    QuickFilter::Cpu,
                    "High CPU",
                    "At least 5% of the whole machine; excludes the idle process",
                ),
                (
                    QuickFilter::Memory,
                    "High memory",
                    "At least 512 MiB of private working set",
                ),
                (
                    QuickFilter::Managed,
                    "Managed",
                    "Processes with a governor action, manual limit or app budget",
                ),
            ] {
                ui.selectable_value(&mut self.filter, filter, label)
                    .on_hover_text(hint);
            }
            if paused.is_some() {
                ui.label(RichText::new("Paused · monitoring continues").color(crate::theme::WARN));
            }
        });
        ui.separator();

        let filter = self.search.trim().to_lowercase();
        let matches = |p: &ProcessInfo| {
            let resource_matches = match self.filter {
                QuickFilter::All => true,
                QuickFilter::Cpu => p.pid != 0 && p.cpu_percent >= 5.0,
                QuickFilter::Memory => p.private_working_set >= 512 * 1024 * 1024,
                QuickFilter::Managed => !extra.label(p).is_empty(),
            };
            resource_matches
                && (filter.is_empty()
                    || p.name.to_lowercase().contains(&filter)
                    || p.pid.to_string().contains(&filter)
                    || extra.command_line(p).to_lowercase().contains(&filter)
                    || extra
                        .image_paths
                        .get(&(p.pid, p.create_time))
                        .is_some_and(|p| p.to_lowercase().contains(&filter)))
        };
        let order = |a: &ProcessInfo, b: &ProcessInfo| {
            let o = compare(a, b, self.sort, extra);
            if self.descending { o.reverse() } else { o }
        };
        // Aggregated rows of the Apps view; the table borrows them.
        let groups: Vec<AppGroup<'_>> = if self.mode == ViewMode::Apps {
            let mut g = app_groups(snap.processes.iter().filter(|p| matches(p)));
            g.sort_by(|a, b| order(&a.total, &b.total));
            g
        } else {
            Vec::new()
        };
        // (process, depth, Some(group index) for an app row)
        let rows: Vec<(&ProcessInfo, usize, Option<usize>)> = match self.mode {
            ViewMode::Tree if filter.is_empty() && self.filter == QuickFilter::All => {
                tree_rows(&snap.processes, self.sort, self.descending, extra)
                    .into_iter()
                    .map(|(p, d)| (p, d, None))
                    .collect()
            }
            ViewMode::Apps => {
                let mut rows = Vec::new();
                for (i, g) in groups.iter().enumerate() {
                    rows.push((&g.total, 0, Some(i)));
                    if self.expanded.contains(&g.key) {
                        let mut members = g.members.clone();
                        members.sort_by(|a, b| order(a, b));
                        rows.extend(members.into_iter().map(|p| (p, 1, None)));
                    }
                }
                rows
            }
            _ => {
                let mut v: Vec<&ProcessInfo> =
                    snap.processes.iter().filter(|p| matches(p)).collect();
                v.sort_by(|a, b| order(a, b));
                v.into_iter().map(|p| (p, 0, None)).collect()
            }
        };
        let shown = snap.processes.iter().filter(|p| matches(p)).count();
        ui.label(
            RichText::new(format!("{shown} of {} processes", snap.processes.len()))
                .small()
                .weak(),
        );
        if rows.is_empty() {
            ui.add_space(24.0);
            ui.label(RichText::new("No matching processes").size(18.0).strong());
            ui.weak("Try another name, PID or resource filter.");
            if ui.button("Clear filters").clicked() {
                self.search.clear();
                self.filter = QuickFilter::All;
            }
            return intents;
        }
        let row_height = ui.spacing().interact_size.y;
        let memory_total = snap.system.memory_total.max(1) as f64;
        self.keyboard(ui, &rows, &mut intents);
        let selected = self.selected.as_ref().map(|r| (r.pid, r.create_time));

        let columns: Vec<_> = COLUMNS
            .iter()
            .filter(|c| c.0 == Col::Name || self.visible.contains(&c.0))
            .collect();

        // Selectable text would swallow clicks meant for the row.
        ui.style_mut().interaction.selectable_labels = false;
        egui::ScrollArea::horizontal()
            .id_salt("process_columns")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width: f32 = columns.iter().map(|c| c.2).sum();
                ui.set_min_width(width + ui.spacing().item_spacing.x * columns.len() as f32);
                let mut table = TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .sense(Sense::click())
                    .cell_layout(Layout::left_to_right(Align::Center));
                for (i, (col, _, width, ..)) in columns.iter().enumerate() {
                    table = table.column(match col {
                        Col::Name => Column::initial(*width).at_least(140.0).clip(true),
                        _ if i == columns.len() - 1 => {
                            Column::remainder().at_least(*width).clip(true)
                        }
                        _ => Column::initial(*width).clip(true),
                    });
                }
                table
                    .header(row_height, |mut header| {
                        for (col, label, _, numeric, _) in &columns {
                            header.col(|ui| {
                                let arrow = match (self.sort == *col, self.descending) {
                                    (true, true) => " ↓",
                                    (true, false) => " ↑",
                                    _ => "",
                                };
                                if ui
                                    .selectable_label(
                                        self.sort == *col,
                                        RichText::new(format!("{label}{arrow}")).strong(),
                                    )
                                    .clicked()
                                {
                                    if self.sort == *col {
                                        self.descending = !self.descending;
                                    } else {
                                        self.sort = *col;
                                        self.descending = *numeric;
                                    }
                                }
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(row_height, rows.len(), |mut row| {
                            let (p, depth, group) = rows[row.index()];
                            let group = group.map(|i| &groups[i]);
                            let key = (p.pid, p.create_time);
                            row.set_selected(group.is_none() && selected == Some(key));

                            for (col, ..) in &columns {
                                row.col(|ui| {
                                    let load = match col {
                                        Col::Cpu if p.pid != 0 => f64::from(p.cpu_percent) / 100.0,
                                        Col::Memory => p.private_working_set as f64 / memory_total,
                                        Col::Disk => p.io_rate() / DISK_HEAT_FULL,
                                        Col::Gpu => f64::from(p.gpu_percent) / 100.0,
                                        _ => 0.0,
                                    };
                                    if self.heat
                                        && let Some(color) = heat_color(load)
                                    {
                                        ui.painter().rect_filled(
                                            ui.max_rect().expand(2.0),
                                            0.0,
                                            color,
                                        );
                                    }
                                    match col {
                                        Col::Name => {
                                            ui.add_space(depth as f32 * 14.0);
                                            if let Some(g) = group {
                                                let open = self.expanded.contains(&g.key);
                                                ui.label(if open { "⏷" } else { "⏵" });
                                            }
                                            app_icon(ui, extra.icon(p));
                                            match group {
                                                Some(g) => ui.label(
                                                    RichText::new(format!(
                                                        "{} ({})",
                                                        p.name,
                                                        g.members.len()
                                                    ))
                                                    .strong(),
                                                ),
                                                None => ui.label(&p.name),
                                            };
                                        }
                                        Col::Pid => right(ui, p.pid.to_string()),
                                        Col::Cpu => right(ui, format!("{:.1}%", p.cpu_percent)),
                                        Col::Memory => {
                                            right(ui, format_bytes(p.private_working_set as f64))
                                        }
                                        Col::Commit => {
                                            right(ui, format_bytes(p.private_bytes as f64))
                                        }
                                        Col::WorkingSet => {
                                            right(ui, format_bytes(p.working_set as f64))
                                        }
                                        Col::Disk => {
                                            right(ui, format!("{}/s", format_bytes(p.io_rate())))
                                        }
                                        Col::Gpu => right(ui, format!("{:.1}%", p.gpu_percent)),
                                        Col::GpuMemory => {
                                            right(ui, format_bytes(p.gpu_memory as f64))
                                        }
                                        Col::Threads => right(ui, p.thread_count.to_string()),
                                        Col::Handles => right(ui, p.handle_count.to_string()),
                                        Col::Session => right(ui, p.session_id.to_string()),
                                        Col::Priority => right(ui, p.base_priority.to_string()),
                                        Col::Started => right(ui, running_for(p)),
                                        Col::Limits => {
                                            if let Some(label) = extra.labels.get(&key) {
                                                ui.label(
                                                    RichText::new(label)
                                                        .color(Color32::from_rgb(230, 140, 40)),
                                                );
                                            }
                                        }
                                        Col::CommandLine => {
                                            let cmd = extra.command_line(p);
                                            ui.label(RichText::new(cmd).monospace().small())
                                                .on_hover_text(cmd);
                                        }
                                    }
                                });
                            }

                            let response = row.response();
                            let target = ProcessRef::from(p);
                            if let Some(g) = group {
                                if response.clicked() && !self.expanded.remove(&g.key) {
                                    self.expanded.insert(g.key.clone());
                                }
                                response.context_menu(|ui| app_menu(ui, g, &mut intents));
                            } else {
                                if response.clicked() {
                                    intents.push(Intent::Select(target.clone()));
                                }
                                response.context_menu(|ui| {
                                    context_menu(ui, &target, logical_cpus, &mut intents)
                                });
                            }
                        });
                    });
            });
        intents
    }
}

/// All processes sharing an image name, with their totals.
pub struct AppGroup<'a> {
    /// Lower-case image name.
    pub key: String,
    /// Sums of the members; PID and start time are the oldest member's, so
    /// icons and labels resolve.
    pub total: ProcessInfo,
    pub members: Vec<&'a ProcessInfo>,
}

fn app_groups<'a>(procs: impl Iterator<Item = &'a ProcessInfo>) -> Vec<AppGroup<'a>> {
    let mut by_name: HashMap<String, Vec<&ProcessInfo>> = HashMap::new();
    for p in procs {
        by_name.entry(p.name.to_lowercase()).or_default().push(p);
    }
    by_name
        .into_iter()
        .map(|(key, mut members)| {
            members.sort_by_key(|p| (p.create_time, p.pid));
            let mut total = members[0].clone();
            for p in &members[1..] {
                total.cpu_percent += p.cpu_percent;
                total.cpu_time += p.cpu_time;
                total.working_set += p.working_set;
                total.private_working_set += p.private_working_set;
                total.private_bytes += p.private_bytes;
                total.thread_count += p.thread_count;
                total.handle_count += p.handle_count;
                total.gpu_percent += p.gpu_percent;
                total.gpu_memory += p.gpu_memory;
                total.io_read_rate += p.io_read_rate;
                total.io_write_rate += p.io_write_rate;
            }
            AppGroup {
                key,
                total,
                members,
            }
        })
        .collect()
}

/// Actions for every process of an app.
fn app_menu(ui: &mut Ui, g: &AppGroup<'_>, intents: &mut Vec<Intent>) {
    let all: Vec<ProcessRef> = g.members.iter().map(|p| ProcessRef::from(*p)).collect();
    let mut each = |ui: &mut Ui, label: &str, command: Command| {
        if ui.button(label).clicked() {
            intents.extend(all.iter().map(|t| Intent::Command(t.clone(), command)));
            ui.close();
        }
    };
    each(ui, "Efficiency mode for all", Command::EcoQos(EcoQos::On));
    each(
        ui,
        "Normal mode for all",
        Command::EcoQos(EcoQos::SystemManaged),
    );
    each(ui, "Free memory of all", Command::TrimMemory);
    each(ui, "Suspend all", Command::Suspend);
    each(ui, "Resume all", Command::Resume);
    ui.separator();
    if ui.button(format!("Limit all {}…", g.total.name)).clicked() {
        intents.push(Intent::LimitApp(g.total.name.clone()));
        ui.close();
    }
    if ui.button("Add to kill list…").clicked() {
        intents.push(Intent::AddToKillList(g.total.name.clone()));
        ui.close();
    }
    if ui
        .button(
            RichText::new(format!("End all {} processes", all.len()))
                .color(Color32::from_rgb(220, 70, 70)),
        )
        .clicked()
    {
        intents.push(Intent::ConfirmMany(all.clone(), Command::Kill));
        ui.close();
    }
}

/// A web search for an image name, as Task Manager does it.
pub fn search_url(name: &str) -> String {
    let query: String = name
        .bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'-' | b'_' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("https://www.bing.com/search?q={query}")
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
    cmd(ui, "Free memory", Command::TrimMemory, false);
    ui.separator();
    if ui.button("Open file location").clicked() {
        intents.push(Intent::OpenLocation(t.clone()));
        ui.close();
    }
    if ui.button(format!("Limit all {}…", t.name)).clicked() {
        intents.push(Intent::LimitApp(t.name.clone()));
        ui.close();
    }
    if ui.button("Add to kill list…").clicked() {
        intents.push(Intent::AddToKillList(t.name.clone()));
        ui.close();
    }
    ui.separator();
    if ui
        .button("Notify me when it exits")
        .on_hover_text(
            "A desktop notification when it ends, e.g. a long build or download; again to stop",
        )
        .clicked()
    {
        intents.push(Intent::WatchExit(t.clone()));
        ui.close();
    }
    if ui
        .button("Create memory dump file")
        .on_hover_text("A full dump in your Temp folder, for debugging a hang or crash")
        .clicked()
    {
        intents.push(Intent::Dump(t.clone()));
        ui.close();
    }
    if ui.button("Search online").clicked() {
        ui.ctx()
            .open_url(egui::OpenUrl::new_tab(search_url(&t.name)));
        ui.close();
    }
    if ui.button("Properties").clicked() {
        intents.push(Intent::Select(t.clone()));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn searches_are_url_encoded() {
        assert_eq!(
            search_url("chrome.exe"),
            "https://www.bing.com/search?q=chrome.exe"
        );
        assert_eq!(
            search_url("My App & Co.exe"),
            "https://www.bing.com/search?q=My+App+%26+Co.exe"
        );
    }

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

    fn no_extra() -> Extra<'static> {
        Extra {
            labels: Box::leak(Box::default()),
            command_lines: Box::leak(Box::default()),
            image_paths: Box::leak(Box::default()),
            icons: Box::leak(Box::default()),
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
        let order: Vec<(u32, usize)> = tree_rows(&procs, Col::Cpu, true, &no_extra())
            .into_iter()
            .map(|(p, d)| (p.pid, d))
            .collect();
        assert_eq!(
            order,
            [(0, 0), (4, 1), (10, 2), (11, 3), (12, 3), (13, 0), (14, 0)]
        );
    }

    #[test]
    fn tree_keeps_processes_in_parent_cycles() {
        // Each names the other as parent with the same create time.
        let procs = [p(20, 21, 1.0, 7), p(21, 20, 2.0, 7), p(22, 21, 0.0, 8)];
        let order: Vec<(u32, usize)> = tree_rows(&procs, Col::Cpu, true, &no_extra())
            .into_iter()
            .map(|(p, d)| (p.pid, d))
            .collect();
        assert_eq!(order, [(20, 0), (21, 1), (22, 2)]);
    }
}

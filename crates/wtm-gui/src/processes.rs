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
        parts.push(crate::i18n::format("{0} cores", &[format!("{}", c)]));
    }
    if let Some(m) = limit.memory_gb {
        parts.push(crate::i18n::format("{0} GB", &[format!("{}", m)]));
    }
    parts.join(", ")
}

/// Labels for processes the service limits: its own actions and app limits.
pub fn limit_labels(status: Option<&wtm_ipc::Status>) -> ManagedMap {
    let mut out = ManagedMap::new();
    let Some(status) = status else { return out };
    for m in &status.managed {
        let sources: Vec<String> = m.sources.iter().map(crate::i18n::debug).collect();
        out.insert(
            (m.target.pid, m.target.create_time),
            format!("{}: {}", sources.join("+"), m.summary),
        );
    }
    for g in &status.app_limits {
        let label = {
            let __wtm_i18n_0 = &(budget_label(&g.limit));
            crate::i18n::format("App limit: {0}", std::slice::from_ref(__wtm_i18n_0))
        };
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
    Network,
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
const COLUMNS: [(Col, &str, f32, bool, bool); 17] = [
    (Col::Name, "Name", 240.0, false, true),
    (Col::Pid, "PID", 64.0, false, true),
    (Col::Cpu, "CPU", 64.0, true, true),
    (Col::Memory, "Memory", 92.0, true, true),
    (Col::Commit, "Commit", 92.0, true, false),
    (Col::WorkingSet, "Working set", 92.0, true, false),
    (Col::Disk, "Disk", 92.0, true, true),
    (Col::Network, "Network", 92.0, true, true),
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
    pub topology: Option<&'a wtm_core::cpu::Topology>,
    pub labels: &'a ManagedMap,
    pub command_lines: &'a crate::backend::CommandLines,
    pub image_paths: &'a crate::backend::CommandLines,
    pub icons: &'a HashMap<String, Option<std::sync::Arc<wtm_core::IconImage>>>,
    /// Processes whose window does not respond.
    pub hung: &'a crate::backend::HungMap,
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
    /// Find out what the process's threads wait for.
    WaitChain(ProcessRef),
    /// Find out why the process cannot be ended.
    WhyAlive(ProcessRef),
    /// List the modules the process has loaded, with their signatures.
    Modules(ProcessRef),
    /// Block (`true`) or unblock the program's network access with Windows Firewall.
    Network(ProcessRef, bool),
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
    NotResponding,
}

/// Which Windows sessions to list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionFilter {
    #[default]
    Any,
    /// Programs in a signed-in user's session.
    Users,
    /// Services and system processes (session 0).
    Services,
}

/// Disk rate that counts as "disk active".
const DISK_ACTIVE: f64 = 1024.0 * 1024.0;
/// GPU share that counts as "GPU active".
const GPU_ACTIVE: f32 = 5.0;

/// Narrows the list beyond the quick filter; every part that is set must match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MoreFilters {
    /// Only processes started within this many minutes.
    pub started_within: Option<u32>,
    pub disk_active: bool,
    pub gpu_active: bool,
    pub session: SessionFilter,
}

impl MoreFilters {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn matches(&self, p: &ProcessInfo, now: std::time::SystemTime) -> bool {
        let recent = self.started_within.is_none_or(|mins| {
            p.start_time().is_some_and(|start| {
                // A start "after now" is clock skew of a moment ago.
                now.duration_since(start)
                    .map_or(true, |age| age.as_secs() <= u64::from(mins) * 60)
            })
        });
        let session = match self.session {
            SessionFilter::Any => true,
            SessionFilter::Users => p.session_id != 0,
            SessionFilter::Services => p.session_id == 0,
        };
        recent
            && session
            && (!self.disk_active || p.io_rate() >= DISK_ACTIVE)
            && (!self.gpu_active || p.gpu_percent >= GPU_ACTIVE)
    }
}

/// Minutes offered for "started in the last ...".
const RECENT_PRESETS: [u32; 5] = [1, 5, 15, 60, 240];

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
    pub more: MoreFilters,
    /// Minutes last chosen for the "started in the last" filter.
    pub recent_minutes: u32,
    /// Only the process table is frozen. Sampling, protection and alerts continue.
    pub paused: Option<std::sync::Arc<Snapshot>>,
    /// The filtered and sorted rows of an earlier frame. Every frame draws
    /// the table, but the rows only change with the data or the settings.
    cache: Option<RowCache>,
}

/// One table row: a process (index into the snapshot) at a tree depth, or
/// an app (index into the groups).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Process(usize, usize),
    App(usize),
}

/// Everything the rows are built from. `revision` stands for the data the
/// window received (snapshot, labels, command lines, hung windows).
#[derive(Debug, PartialEq)]
struct RowKey {
    revision: u64,
    snapshot: usize,
    processes: usize,
    search: String,
    mode: ViewMode,
    sort: Col,
    descending: bool,
    filter: QuickFilter,
    more: MoreFilters,
    expanded: HashSet<String>,
    /// The "started in the last" filter moves with the clock.
    second: Option<u64>,
}

struct RowCache {
    key: RowKey,
    rows: Vec<Row>,
    groups: Vec<AppGroup>,
    /// Processes that match the filters.
    shown: usize,
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
            more: MoreFilters::default(),
            recent_minutes: 5,
            paused: None,
            cache: None,
        }
    }
}

impl ProcessView {
    /// Turns the "started in the last" filter on or off; on also shows how
    /// long each process has been running.
    fn set_recent(&mut self, on: bool) {
        self.more.started_within = on.then_some(self.recent_minutes);
        if on {
            self.visible.insert(Col::Started);
        }
    }

    fn more_filters_menu(&mut self, ui: &mut Ui) {
        let active = [
            self.more.started_within.is_some(),
            self.more.disk_active,
            self.more.gpu_active,
            self.more.session != SessionFilter::Any,
        ]
        .into_iter()
        .filter(|on| *on)
        .count();
        let title = if active == 0 {
            crate::i18n::tr("More filters").to_string()
        } else {
            crate::i18n::format("More filters ({0})", &[active.to_string()])
        };
        ui.menu_button(title, |ui| {
            ui.strong(crate::i18n::tr("Started in the last"));
            ui.horizontal(|ui| {
                for mins in RECENT_PRESETS {
                    let label = if mins >= 60 {
                        crate::i18n::format("{0} h", &[(mins / 60).to_string()])
                    } else {
                        crate::i18n::format("{0} min", &[mins.to_string()])
                    };
                    let on = self.more.started_within == Some(mins);
                    if ui.selectable_label(on, label).clicked() {
                        self.recent_minutes = mins;
                        self.set_recent(!on);
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(crate::i18n::tr("Custom"));
                let mut mins = self.recent_minutes;
                if ui
                    .add(
                        egui::DragValue::new(&mut mins)
                            .range(1..=10_080)
                            .suffix(" min"),
                    )
                    .changed()
                {
                    self.recent_minutes = mins;
                    self.set_recent(true);
                }
            });
            ui.separator();
            ui.strong(crate::i18n::tr("Activity"));
            ui.checkbox(
                &mut self.more.disk_active,
                crate::i18n::tr("Disk active (1 MiB/s or more)"),
            );
            ui.checkbox(
                &mut self.more.gpu_active,
                crate::i18n::tr("GPU active (5% or more)"),
            );
            ui.separator();
            ui.strong(crate::i18n::tr("Session"));
            for (session, label) in [
                (SessionFilter::Any, crate::i18n::tr("All sessions")),
                (SessionFilter::Users, crate::i18n::tr("User apps")),
                (
                    SessionFilter::Services,
                    crate::i18n::tr("Services and system (session 0)"),
                ),
            ] {
                ui.radio_value(&mut self.more.session, session, label);
            }
            if !self.more.is_empty() {
                ui.separator();
                if ui.button(crate::i18n::tr("Reset these filters")).clicked() {
                    self.more = MoreFilters::default();
                    ui.close();
                }
            }
        });
    }

    /// One removable chip per active activity or session filter.
    fn filter_chips(&mut self, ui: &mut Ui) {
        let chip = |ui: &mut Ui, text: &str| {
            ui.add(
                egui::Button::new(format!("{text}  ✖"))
                    .small()
                    .corner_radius(10),
            )
            .on_hover_text(crate::i18n::tr("Remove this filter"))
            .clicked()
        };
        if self.more.disk_active && chip(ui, crate::i18n::tr("Disk active")) {
            self.more.disk_active = false;
        }
        if self.more.gpu_active && chip(ui, crate::i18n::tr("GPU active")) {
            self.more.gpu_active = false;
        }
        let session = match self.more.session {
            SessionFilter::Any => None,
            SessionFilter::Users => Some(crate::i18n::tr("User apps")),
            SessionFilter::Services => Some(crate::i18n::tr("Services and system")),
        };
        if let Some(label) = session
            && chip(ui, label)
        {
            self.more.session = SessionFilter::Any;
        }
    }

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
        Col::Network => a.net_rate().total_cmp(&b.net_rate()),
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
#[cfg(test)]
fn tree_rows<'a>(
    procs: &'a [ProcessInfo],
    col: Col,
    descending: bool,
    extra: &Extra<'_>,
) -> Vec<(&'a ProcessInfo, usize)> {
    tree_order(procs, col, descending, extra)
        .into_iter()
        .map(|(i, depth)| (&procs[i], depth))
        .collect()
}

/// [`tree_rows`] as (index, depth) pairs.
fn tree_order(
    procs: &[ProcessInfo],
    col: Col,
    descending: bool,
    extra: &Extra<'_>,
) -> Vec<(usize, usize)> {
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
    let mut seen = vec![false; procs.len()];
    let mut stack = Vec::new();
    // Processes that are only reachable through a parent cycle (same create
    // time, PIDs reused) are appended as extra roots so none goes missing.
    for start in roots.into_iter().chain(0..procs.len()) {
        stack.push((start, 0));
        while let Some((i, depth)) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            out.push((i, depth));
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
    fn keyboard(&self, ui: &Ui, rows: &[Row], all: &[ProcessInfo], intents: &mut Vec<Intent>) {
        let (up, down, delete) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Delete),
            )
        });
        let ctx = ui.ctx();
        if !(up || down || delete)
            || ctx.text_edit_focused()
            || egui::Popup::is_any_open(ctx)
            || ctx.memory(|m| m.top_modal_layer().is_some())
        {
            return;
        }
        let procs: Vec<&ProcessInfo> = rows
            .iter()
            .filter_map(|r| match *r {
                Row::Process(i, _) => Some(&all[i]),
                Row::App(_) => None,
            })
            .collect();
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

    /// Shows the table, building its rows anew every frame (tests change the
    /// data in place).
    #[cfg(test)]
    pub fn ui(&mut self, ui: &mut Ui, snap: &Snapshot, extra: &Extra<'_>) -> Vec<Intent> {
        self.show(ui, snap, extra, None)
    }

    /// Shows the table, reusing the rows of earlier frames until `revision`
    /// (a count of the data updates the window received) or a setting changes.
    pub fn ui_cached(
        &mut self,
        ui: &mut Ui,
        snap: &Snapshot,
        extra: &Extra<'_>,
        revision: u64,
    ) -> Vec<Intent> {
        self.show(ui, snap, extra, Some(revision))
    }

    fn show(
        &mut self,
        ui: &mut Ui,
        snap: &Snapshot,
        extra: &Extra<'_>,
        revision: Option<u64>,
    ) -> Vec<Intent> {
        let mut intents = Vec::new();
        let logical_cpus = snap.system.logical_cpus.max(1);

        ui.horizontal_wrapped(|ui| {
            ui.label(crate::i18n::tr("Search:"));
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .id(egui::Id::unique("process_search"))
                    .hint_text(crate::i18n::tr("name, PID, path or command line (Ctrl+F)"))
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
                (ViewMode::List, crate::i18n::tr("List")),
                (ViewMode::Tree, crate::i18n::tr("Tree")),
                (ViewMode::Apps, crate::i18n::tr("Apps")),
            ] {
                ui.selectable_value(&mut self.mode, mode, label);
            }
            ui.menu_button(crate::i18n::tr("Columns"), |ui| {
                ui.checkbox(&mut self.heat, crate::i18n::tr("Heat map"))
                    .on_hover_text(crate::i18n::tr("Shade busy CPU, memory, disk and GPU cells"));
                ui.separator();
                for (col, label, ..) in COLUMNS.iter().skip(1) {
                    let mut on = self.visible.contains(col);
                    if ui.checkbox(&mut on, crate::i18n::tr(label)).changed() {
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
            if ui.button(if self.paused.is_some() { crate::i18n::tr("Resume updates") } else { crate::i18n::tr("Pause updates") })
                .on_hover_text(crate::i18n::tr("Freeze this process list for inspection (F6). Monitoring, alerts and protection continue."))
                .clicked() || toggle_key {
                self.paused = if self.paused.is_some() { None } else { Some(std::sync::Arc::new(snap.clone())) };
            }
        });
        let paused = self.paused.clone();
        let snap = paused.as_deref().unwrap_or(snap);
        ui.horizontal_wrapped(|ui| {
            for (filter, label, hint) in [
                (
                    QuickFilter::All,
                    crate::i18n::tr("All processes"),
                    crate::i18n::tr("Every process"),
                ),
                (
                    QuickFilter::Cpu,
                    crate::i18n::tr("High CPU"),
                    crate::i18n::tr("At least 5% of the whole machine; excludes the idle process"),
                ),
                (
                    QuickFilter::Memory,
                    crate::i18n::tr("High memory"),
                    crate::i18n::tr("At least 512 MiB of private working set"),
                ),
                (
                    QuickFilter::Managed,
                    crate::i18n::tr("Managed"),
                    crate::i18n::tr("Processes with a governor action, manual limit or app budget"),
                ),
                (
                    QuickFilter::NotResponding,
                    crate::i18n::tr("Not responding"),
                    crate::i18n::tr("Programs whose window Windows marks as not responding"),
                ),
            ] {
                ui.selectable_value(&mut self.filter, filter, label)
                    .on_hover_text(hint);
            }
            ui.separator();
            let mut recent = self.more.started_within.is_some();
            if ui
                .toggle_value(
                    &mut recent,
                    crate::i18n::format(
                        "Started in the last {0} min",
                        &[self.recent_minutes.to_string()],
                    ),
                )
                .on_hover_text(crate::i18n::tr(
                    "Only processes that started recently; change the minutes under More filters",
                ))
                .changed()
            {
                self.set_recent(recent);
            }
            self.more_filters_menu(ui);
            self.filter_chips(ui);
            if paused.is_some() {
                ui.label(
                    RichText::new(crate::i18n::tr("Paused · monitoring continues"))
                        .color(crate::theme::WARN),
                );
            }
        });
        ui.separator();

        let cache = self.rows(snap, extra, revision);
        let (rows, groups, shown) = (&cache.rows, &cache.groups, cache.shown);
        ui.label(
            RichText::new({
                let __wtm_i18n_0 = &(snap.processes.len());
                crate::i18n::format(
                    "{0} of {1} processes",
                    &[format!("{}", shown), format!("{}", __wtm_i18n_0)],
                )
            })
            .small()
            .weak(),
        );
        if rows.is_empty() {
            ui.add_space(24.0);
            ui.label(
                RichText::new(crate::i18n::tr("No matching processes"))
                    .size(18.0)
                    .strong(),
            );
            ui.weak(crate::i18n::tr("Try another name, PID or resource filter."));
            if ui.button(crate::i18n::tr("Clear filters")).clicked() {
                self.search.clear();
                self.filter = QuickFilter::All;
                self.more = MoreFilters::default();
            }
            self.keep(cache, revision);
            return intents;
        }
        let row_height = ui.spacing().interact_size.y;
        let memory_total = snap.system.memory_total.max(1) as f64;
        self.keyboard(ui, rows, &snap.processes, &mut intents);
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
                            let label = crate::i18n::tr(label);
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
                            let (p, depth, group) = match rows[row.index()] {
                                Row::Process(i, depth) => (&snap.processes[i], depth, None),
                                Row::App(i) => (&groups[i].total, 0, Some(&groups[i])),
                            };
                            let key = (p.pid, p.create_time);
                            row.set_selected(group.is_none() && selected == Some(key));

                            for (col, ..) in &columns {
                                row.col(|ui| {
                                    let load = match col {
                                        Col::Cpu if p.pid != 0 => f64::from(p.cpu_percent) / 100.0,
                                        Col::Memory => p.private_working_set as f64 / memory_total,
                                        Col::Disk => p.io_rate() / DISK_HEAT_FULL,
                                        Col::Network => p.net_rate() / DISK_HEAT_FULL,
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
                                            if extra.hung.contains_key(&p.pid) {
                                                ui.label(
                                                    RichText::new(crate::i18n::tr(
                                                        "Not responding",
                                                    ))
                                                    .color(crate::theme::BAD)
                                                    .small(),
                                                );
                                            }
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
                                        Col::Network => {
                                            right(ui, format!("{}/s", format_bytes(p.net_rate())))
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
                                response.context_menu(|ui| {
                                    app_menu(ui, g, &snap.processes, &mut intents)
                                });
                            } else {
                                if response.clicked() {
                                    intents.push(Intent::Select(target.clone()));
                                }
                                response.context_menu(|ui| {
                                    context_menu(
                                        ui,
                                        &target,
                                        logical_cpus,
                                        extra.topology,
                                        &mut intents,
                                    )
                                });
                            }
                        });
                    });
            });
        self.keep(cache, revision);
        intents
    }

    /// Holds on to the rows for the next frame when the data has a revision.
    fn keep(&mut self, cache: RowCache, revision: Option<u64>) {
        self.cache = revision.map(|_| cache);
    }

    /// Whether `key` still describes these settings and data.
    fn is_current(
        &self,
        key: &RowKey,
        snap: &Snapshot,
        revision: u64,
        second: Option<u64>,
    ) -> bool {
        key.revision == revision
            && key.snapshot == std::ptr::from_ref(snap) as usize
            && key.processes == snap.processes.len()
            && key.search == self.search
            && key.mode == self.mode
            && key.sort == self.sort
            && key.descending == self.descending
            && key.filter == self.filter
            && key.more == self.more
            && key.second == second
            && (self.mode != ViewMode::Apps || key.expanded == self.expanded)
    }

    /// The rows to show: the cached ones while they are current, otherwise
    /// filtered, grouped and sorted anew. Without a `revision` the data may
    /// have changed in any way, so they are always rebuilt.
    fn rows(&mut self, snap: &Snapshot, extra: &Extra<'_>, revision: Option<u64>) -> RowCache {
        let now = std::time::SystemTime::now();
        let second = self.more.started_within.map(|_| {
            now.duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        });
        if let Some(revision) = revision
            && let Some(cache) = self.cache.take()
            && self.is_current(&cache.key, snap, revision, second)
        {
            return cache;
        }

        let procs = &snap.processes;
        let filter = self.search.trim().to_lowercase();
        let matches = |p: &ProcessInfo| {
            let resource_matches = match self.filter {
                QuickFilter::All => true,
                QuickFilter::Cpu => p.pid != 0 && p.cpu_percent >= 5.0,
                QuickFilter::Memory => p.private_working_set >= 512 * 1024 * 1024,
                QuickFilter::Managed => !extra.label(p).is_empty(),
                QuickFilter::NotResponding => extra.hung.contains_key(&p.pid),
            };
            resource_matches
                && self.more.matches(p, now)
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
        let unfiltered =
            filter.is_empty() && self.filter == QuickFilter::All && self.more.is_empty();
        let matching = || (0..procs.len()).filter(|&i| matches(&procs[i]));
        let mut groups = Vec::new();
        let (rows, shown) = match self.mode {
            ViewMode::Tree if unfiltered => {
                let rows = tree_order(procs, self.sort, self.descending, extra)
                    .into_iter()
                    .map(|(i, depth)| Row::Process(i, depth))
                    .collect();
                (rows, procs.len())
            }
            ViewMode::Apps => {
                let members: Vec<usize> = matching().collect();
                groups = app_groups(procs, &members);
                groups.sort_by(|a, b| order(&a.total, &b.total));
                let mut rows = Vec::with_capacity(groups.len());
                for (i, g) in groups.iter().enumerate() {
                    rows.push(Row::App(i));
                    if self.expanded.contains(&g.key) {
                        let mut members = g.members.clone();
                        members.sort_by(|&a, &b| order(&procs[a], &procs[b]));
                        rows.extend(members.into_iter().map(|i| Row::Process(i, 1)));
                    }
                }
                (rows, members.len())
            }
            _ => {
                let mut v: Vec<usize> = matching().collect();
                v.sort_by(|&a, &b| order(&procs[a], &procs[b]));
                let shown = v.len();
                (v.into_iter().map(|i| Row::Process(i, 0)).collect(), shown)
            }
        };
        RowCache {
            key: RowKey {
                revision: revision.unwrap_or_default(),
                snapshot: std::ptr::from_ref(snap) as usize,
                processes: procs.len(),
                search: self.search.clone(),
                mode: self.mode,
                sort: self.sort,
                descending: self.descending,
                filter: self.filter,
                more: self.more.clone(),
                expanded: if self.mode == ViewMode::Apps {
                    self.expanded.clone()
                } else {
                    HashSet::new()
                },
                second,
            },
            rows,
            groups,
            shown,
        }
    }
}

/// All processes sharing an image name, with their totals.
pub struct AppGroup {
    /// Lower-case image name.
    pub key: String,
    /// Sums of the members; PID and start time are the oldest member's, so
    /// icons and labels resolve.
    pub total: ProcessInfo,
    /// Indices into the snapshot's processes.
    pub members: Vec<usize>,
}

/// Groups the processes at `members` (indices into `procs`) by image name.
fn app_groups(procs: &[ProcessInfo], members: &[usize]) -> Vec<AppGroup> {
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for &i in members {
        by_name
            .entry(procs[i].name.to_lowercase())
            .or_default()
            .push(i);
    }
    by_name
        .into_iter()
        .map(|(key, mut members)| {
            members.sort_by_key(|&i| (procs[i].create_time, procs[i].pid));
            let mut total = procs[members[0]].clone();
            for p in members[1..].iter().map(|&i| &procs[i]) {
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
fn app_menu(ui: &mut Ui, g: &AppGroup, procs: &[ProcessInfo], intents: &mut Vec<Intent>) {
    let all: Vec<ProcessRef> = g
        .members
        .iter()
        .map(|&i| ProcessRef::from(&procs[i]))
        .collect();
    let mut each = |ui: &mut Ui, label: &str, command: Command| {
        if ui.button(label).clicked() {
            intents.extend(all.iter().map(|t| Intent::Command(t.clone(), command)));
            ui.close();
        }
    };
    each(
        ui,
        crate::i18n::tr("Efficiency mode for all"),
        Command::EcoQos(EcoQos::On),
    );
    each(
        ui,
        crate::i18n::tr("Normal mode for all"),
        Command::EcoQos(EcoQos::SystemManaged),
    );
    each(
        ui,
        crate::i18n::tr("Free memory of all"),
        Command::TrimMemory,
    );
    each(ui, crate::i18n::tr("Suspend all"), Command::Suspend);
    each(ui, crate::i18n::tr("Resume all"), Command::Resume);
    ui.separator();
    if ui
        .button({
            let __wtm_i18n_0 = &(g.total.name);
            crate::i18n::format("Limit all {0}…", std::slice::from_ref(__wtm_i18n_0))
        })
        .clicked()
    {
        intents.push(Intent::LimitApp(g.total.name.clone()));
        ui.close();
    }
    if ui.button(crate::i18n::tr("Add to kill list…")).clicked() {
        intents.push(Intent::AddToKillList(g.total.name.clone()));
        ui.close();
    }
    if ui
        .button(
            RichText::new({
                let __wtm_i18n_0 = &(all.len());
                crate::i18n::format("End all {0} processes", &[format!("{}", __wtm_i18n_0)])
            })
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

pub fn context_menu(
    ui: &mut Ui,
    t: &ProcessRef,
    logical_cpus: u32,
    topology: Option<&wtm_core::cpu::Topology>,
    intents: &mut Vec<Intent>,
) {
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

    cmd(ui, crate::i18n::tr("End task"), Command::Kill, true);
    cmd(
        ui,
        crate::i18n::tr("End process tree"),
        Command::KillTree,
        true,
    );
    ui.separator();
    cmd(ui, crate::i18n::tr("Suspend"), Command::Suspend, false);
    cmd(ui, crate::i18n::tr("Resume"), Command::Resume, false);
    ui.separator();
    ui.menu_button(crate::i18n::tr("Priority"), |ui| {
        for (label, p) in [
            (crate::i18n::tr("Idle"), PriorityClass::Idle),
            (crate::i18n::tr("Below normal"), PriorityClass::BelowNormal),
            (crate::i18n::tr("Normal"), PriorityClass::Normal),
            (crate::i18n::tr("Above normal"), PriorityClass::AboveNormal),
            (crate::i18n::tr("High"), PriorityClass::High),
        ] {
            cmd(ui, label, Command::Priority(p), false);
        }
    });
    ui.menu_button(crate::i18n::tr("Efficiency mode"), |ui| {
        cmd(
            ui,
            crate::i18n::tr("On"),
            Command::EcoQos(EcoQos::On),
            false,
        );
        cmd(
            ui,
            crate::i18n::tr("Off (let Windows decide)"),
            Command::EcoQos(EcoQos::SystemManaged),
            false,
        );
    });
    if let Some(topology) = topology.filter(|t| t.hybrid()) {
        ui.menu_button(crate::i18n::tr("CPU core type"), |ui| {
            use wtm_core::cpu::CoreSelection as C;
            cmd(
                ui,
                crate::i18n::tr("P-cores only"),
                Command::AffinityTier(C::Performance),
                false,
            );
            if topology.classes().len() <= 3 {
                cmd(
                    ui,
                    crate::i18n::tr("E-cores only"),
                    Command::AffinityTier(C::Efficient),
                    false,
                );
            }
            if topology.classes().len() == 3 {
                cmd(
                    ui,
                    crate::i18n::tr("LP-E cores only"),
                    Command::AffinityTier(C::LowPower),
                    false,
                );
            }
            cmd(
                ui,
                crate::i18n::tr("All available cores"),
                Command::AffinityTier(C::All),
                false,
            );
            ui.weak(crate::i18n::tr(
                "Windows performance tiers. Cross-group processes are rejected.",
            ));
        });
    }
    ui.menu_button(crate::i18n::tr("Limit CPU"), |ui| {
        for (label, cores) in [
            (crate::i18n::tr("¼ core"), 0.25),
            (crate::i18n::tr("½ core"), 0.5),
            (crate::i18n::tr("1 core"), 1.0),
            (crate::i18n::tr("2 cores"), 2.0),
            (crate::i18n::tr("4 cores"), 4.0),
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
                &{
                    crate::i18n::format(
                        "{0}  ({1}% of machine)",
                        &[label.to_string(), format!("{:.2}", percent)],
                    )
                },
                Command::Limit(limits),
                false,
            );
        }
    });
    ui.menu_button(crate::i18n::tr("Limit memory"), |ui| {
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
    cmd(
        ui,
        crate::i18n::tr("Remove limits"),
        Command::Unlimit,
        false,
    );
    cmd(
        ui,
        crate::i18n::tr("Free memory"),
        Command::TrimMemory,
        false,
    );
    ui.separator();
    if ui.button(crate::i18n::tr("Open file location")).clicked() {
        intents.push(Intent::OpenLocation(t.clone()));
        ui.close();
    }
    if ui
        .button({
            let __wtm_i18n_0 = &(t.name);
            crate::i18n::format("Limit all {0}…", std::slice::from_ref(__wtm_i18n_0))
        })
        .clicked()
    {
        intents.push(Intent::LimitApp(t.name.clone()));
        ui.close();
    }
    if ui.button(crate::i18n::tr("Add to kill list…")).clicked() {
        intents.push(Intent::AddToKillList(t.name.clone()));
        ui.close();
    }
    if ui
        .button(crate::i18n::tr("Block network access"))
        .on_hover_text(crate::i18n::tr(
            "Windows Firewall rules that cut this program off the network; undo in the Network tab",
        ))
        .clicked()
    {
        intents.push(Intent::Network(t.clone(), true));
        ui.close();
    }
    ui.separator();
    if ui
        .button(crate::i18n::tr("Notify me when it exits"))
        .on_hover_text(crate::i18n::tr(
            "A desktop notification when it ends, e.g. a long build or download; again to stop",
        ))
        .clicked()
    {
        intents.push(Intent::WatchExit(t.clone()));
        ui.close();
    }
    if ui
        .button(crate::i18n::tr("Create memory dump file"))
        .on_hover_text(crate::i18n::tr(
            "A full dump in your Temp folder, for debugging a hang or crash",
        ))
        .clicked()
    {
        intents.push(Intent::Dump(t.clone()));
        ui.close();
    }
    if ui.button(crate::i18n::tr("Search online")).clicked() {
        ui.ctx()
            .open_url(egui::OpenUrl::new_tab(search_url(&t.name)));
        ui.close();
    }
    if ui.button(crate::i18n::tr("Properties")).clicked() {
        intents.push(Intent::Select(t.clone()));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// FILETIME of `ago` before `now`.
    fn started(now: std::time::SystemTime, ago: Duration) -> i64 {
        let unix = now - ago;
        let since = unix.duration_since(std::time::UNIX_EPOCH).unwrap();
        116_444_736_000_000_000 + (since.as_nanos() / 100) as i64
    }

    #[test]
    fn more_filters_match_start_time_activity_and_session() {
        let now = std::time::SystemTime::now();
        let mut fresh = p(10, 0, 0.0, started(now, Duration::from_secs(90)));
        let old = p(11, 0, 0.0, started(now, Duration::from_secs(3600)));
        let none = MoreFilters::default();
        assert!(none.is_empty() && none.matches(&old, now));

        let recent = MoreFilters {
            started_within: Some(5),
            ..MoreFilters::default()
        };
        assert!(recent.matches(&fresh, now));
        assert!(!recent.matches(&old, now));
        // The boundary is inclusive; a start in the future is just now.
        let edge = p(12, 0, 0.0, started(now, Duration::from_secs(300)));
        assert!(recent.matches(&edge, now));
        let skewed = p(13, 0, 0.0, started(now, Duration::ZERO) + 10_000_000);
        assert!(recent.matches(&skewed, now));
        // No start time (System Idle Process) never counts as recent.
        assert!(!recent.matches(&p(0, 0, 0.0, 0), now));

        let active = MoreFilters {
            disk_active: true,
            gpu_active: true,
            ..MoreFilters::default()
        };
        assert!(!active.matches(&fresh, now));
        fresh.io_read_rate = 1024.0 * 1024.0;
        fresh.gpu_percent = 5.0;
        assert!(active.matches(&fresh, now));

        let services = MoreFilters {
            session: SessionFilter::Services,
            ..MoreFilters::default()
        };
        let users = MoreFilters {
            session: SessionFilter::Users,
            ..MoreFilters::default()
        };
        fresh.session_id = 0;
        assert!(services.matches(&fresh, now) && !users.matches(&fresh, now));
        fresh.session_id = 1;
        assert!(!services.matches(&fresh, now) && users.matches(&fresh, now));
    }

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
            net_rx_rate: 0.0,
            net_tx_rate: 0.0,
            create_time,
        }
    }

    fn no_extra() -> Extra<'static> {
        Extra {
            topology: None,
            labels: Box::leak(Box::default()),
            command_lines: Box::leak(Box::default()),
            image_paths: Box::leak(Box::default()),
            icons: Box::leak(Box::default()),
            hung: Box::leak(Box::default()),
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

    #[test]
    fn cached_rows_last_until_the_data_or_a_setting_changes() {
        let mut snap = crate::ui_tests::snapshot();
        snap.processes = vec![p(10, 0, 1.0, 1), p(11, 0, 2.0, 2), p(12, 0, 3.0, 3)];
        let extra = no_extra();
        let mut view = ProcessView::default();
        let pids = |view: &mut ProcessView, snap: &Snapshot, revision| {
            let cache = view.rows(snap, &extra, revision);
            let pids: Vec<u32> = cache
                .rows
                .iter()
                .map(|r| match *r {
                    Row::Process(i, _) => snap.processes[i].pid,
                    Row::App(g) => cache.groups[g].total.pid,
                })
                .collect();
            view.keep(cache, revision);
            pids
        };
        assert_eq!(pids(&mut view, &snap, Some(1)), [12, 11, 10]);
        // Same revision: the rows of the earlier frame.
        snap.processes[0].cpu_percent = 9.0;
        assert_eq!(pids(&mut view, &snap, Some(1)), [12, 11, 10]);
        // New data, or no revision at all: built again.
        assert_eq!(pids(&mut view, &snap, Some(2)), [10, 12, 11]);
        snap.processes[1].cpu_percent = 10.0;
        assert_eq!(pids(&mut view, &snap, None), [11, 10, 12]);
        assert!(view.cache.is_none());
        // Every setting that orders or narrows the rows rebuilds them.
        assert_eq!(pids(&mut view, &snap, Some(3)), [11, 10, 12]);
        view.descending = false;
        assert_eq!(pids(&mut view, &snap, Some(3)), [12, 10, 11]);
        view.sort = Col::Pid;
        assert_eq!(pids(&mut view, &snap, Some(3)), [10, 11, 12]);
        view.search = "12".into();
        assert_eq!(pids(&mut view, &snap, Some(3)), [12]);
        view.search.clear();
        view.filter = QuickFilter::Cpu;
        assert_eq!(pids(&mut view, &snap, Some(3)), [10, 11]);
        view.filter = QuickFilter::All;
        view.mode = ViewMode::Apps;
        assert_eq!(pids(&mut view, &snap, Some(3)), [10, 11, 12]);
        view.expanded.insert("p11".into());
        assert_eq!(pids(&mut view, &snap, Some(3)), [10, 11, 11, 12]);
    }
}

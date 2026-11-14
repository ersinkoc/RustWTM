//! The Processes tab: sortable flat or tree table with a context menu.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Ui};
use egui_extras::Column;
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
            format!(
                "{}: {}",
                sources.join("+"),
                crate::i18n::message(&m.summary)
            ),
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

/// The name of a priority class, as the Priority menu shows it.
pub fn priority_label(p: PriorityClass) -> &'static str {
    match p {
        PriorityClass::Idle => crate::i18n::tr("Idle"),
        PriorityClass::BelowNormal => crate::i18n::tr("Below normal"),
        PriorityClass::Normal => crate::i18n::tr("Normal"),
        PriorityClass::AboveNormal => crate::i18n::tr("Above normal"),
        PriorityClass::High => crate::i18n::tr("High"),
        PriorityClass::Realtime => crate::i18n::tr("Realtime"),
    }
}

/// The state of efficiency mode in words.
pub fn eco_label(e: EcoQos) -> &'static str {
    match e {
        EcoQos::On => crate::i18n::tr("On"),
        EcoQos::Off => crate::i18n::tr("Off"),
        EcoQos::SystemManaged => crate::i18n::tr("Windows decides"),
    }
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
    User,
    Architecture,
    Elevated,
    FileVersion,
    ParentPid,
    ImagePath,
    StartTime,
    Integrity,
    /// Total processor time used, as Task Manager's Details column.
    CpuTime,
    DiskRead,
    DiskWrite,
    /// Running, Suspended or Not responding.
    Status,
    PeakWorkingSet,
    PagedPool,
    NonPagedPool,
    PageFaults,
    PageFaultRate,
    GpuEngine,
    SharedGpuMemory,
    GdiObjects,
    UserObjects,
    /// Forward-compatible saved preferences; never a rendered column.
    #[serde(other)]
    Unknown,
}

/// Column, header, initial width, numeric (sorts largest first), shown by default.
pub(crate) const COLUMNS: [(Col, &str, f32, bool, bool); 38] = [
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
    (Col::User, "User name", 140.0, false, false),
    (Col::Architecture, "Architecture", 84.0, false, false),
    (Col::Elevated, "Elevated", 76.0, false, false),
    (Col::FileVersion, "File version", 120.0, false, false),
    (Col::ParentPid, "Parent PID", 76.0, false, false),
    (Col::ImagePath, "Image path", 320.0, false, false),
    (Col::StartTime, "Start time", 150.0, false, false),
    (Col::Integrity, "Integrity", 96.0, false, false),
    (Col::CpuTime, "CPU time", 88.0, true, false),
    (Col::DiskRead, "Disk read", 92.0, true, false),
    (Col::DiskWrite, "Disk write", 92.0, true, false),
    (Col::Status, "Status", 110.0, false, false),
    (Col::PeakWorkingSet, "Peak working set", 104.0, true, false),
    (Col::PagedPool, "Paged pool", 92.0, true, false),
    (Col::NonPagedPool, "Non-paged pool", 92.0, true, false),
    (Col::PageFaults, "Page faults", 96.0, true, false),
    (Col::PageFaultRate, "Page faults/s", 96.0, true, false),
    (Col::GpuEngine, "GPU engine", 130.0, false, false),
    (
        Col::SharedGpuMemory,
        "Shared GPU memory",
        104.0,
        true,
        false,
    ),
    (Col::GdiObjects, "GDI objects", 84.0, true, false),
    (Col::UserObjects, "USER objects", 84.0, true, false),
];

impl Col {
    /// Columns read only while shown: the window asks for them.
    pub fn gui_objects(self) -> bool {
        matches!(self, Self::GdiObjects | Self::UserObjects)
    }
}

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
    pub descriptions: &'a HashMap<String, Option<String>>,
    pub icons: &'a HashMap<String, Option<std::sync::Arc<wtm_core::IconImage>>>,
    /// Processes whose window does not respond.
    pub hung: &'a crate::backend::HungMap,
    pub facts: &'a crate::backend::FactsMap,
    /// File version per image path.
    pub versions: &'a HashMap<String, Option<String>>,
    /// Services each process hosts.
    pub services: &'a crate::backend::HostedServices,
    /// GPU LUIDs in the order the Performance page numbers them ("GPU 0").
    pub gpu_order: &'a [String],
    /// GDI and USER objects, while one of their columns is shown.
    pub gui_objects: &'a crate::backend::GuiObjects,
}

impl Extra<'_> {
    fn image_path(&self, p: &ProcessInfo) -> Option<&str> {
        self.image_paths
            .get(&(p.pid, p.create_time))
            .map(String::as_str)
            .filter(|path| !path.is_empty())
    }

    fn description(&self, p: &ProcessInfo) -> Option<&str> {
        self.descriptions
            .get(self.image_path(p)?)?
            .as_deref()
            .filter(|name| !name.eq_ignore_ascii_case(&p.name))
    }

    /// Task Manager's GPU engine text, such as "GPU 0 - 3D"; empty while the
    /// process uses no GPU engine.
    fn gpu_engine(&self, p: &ProcessInfo) -> String {
        let Some(engine) = &p.gpu_detail.engine else {
            return String::new();
        };
        match self.gpu_order.iter().position(|l| *l == engine.luid) {
            Some(n) => format!("GPU {n} - {}", engine.kind),
            None => engine.kind.clone(),
        }
    }

    /// GDI and USER objects of `p`, when read.
    fn gui(&self, p: &ProcessInfo) -> Option<(u32, u32)> {
        self.gui_objects.get(&(p.pid, p.create_time)).copied()
    }

    /// The services `p` hosts, by display name.
    pub fn hosted(&self, p: &ProcessInfo) -> &[String] {
        self.services.get(&p.pid).map_or(&[], Vec::as_slice)
    }

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

    fn facts(&self, p: &ProcessInfo) -> Option<&wtm_core::control::ProcessFacts> {
        self.facts.get(&(p.pid, p.create_time))
    }

    /// The account, `DOMAIN\user`, when it could be read.
    pub fn user(&self, p: &ProcessInfo) -> Option<&str> {
        self.facts(p)?.user.as_deref()
    }

    fn version(&self, p: &ProcessInfo) -> &str {
        self.image_path(p)
            .and_then(|path| self.versions.get(path)?.as_deref())
            .unwrap_or("")
    }

    fn architecture(&self, p: &ProcessInfo) -> String {
        self.facts(p)
            .and_then(|f| f.architecture)
            .map(wtm_core::control::Architecture::label)
            .unwrap_or_default()
    }

    /// Sorts unknown before no before yes.
    fn elevated(&self, p: &ProcessInfo) -> Option<bool> {
        self.facts(p)?.elevated
    }

    /// Sorts unknown before the least trusted level.
    fn integrity(&self, p: &ProcessInfo) -> Option<wtm_core::control::Integrity> {
        self.facts(p)?.integrity
    }
}

/// The integrity level in words, as Windows names it.
pub fn integrity_label(level: wtm_core::control::Integrity) -> String {
    use wtm_core::control::Integrity;
    match level {
        Integrity::Untrusted => crate::i18n::tr("Untrusted").into(),
        Integrity::Low => crate::i18n::tr("Low").into(),
        Integrity::Medium => crate::i18n::tr("Medium").into(),
        Integrity::MediumPlus => crate::i18n::tr("Medium plus").into(),
        Integrity::High => crate::i18n::tr("High").into(),
        Integrity::System => crate::i18n::tr("System").into(),
        Integrity::Protected => crate::i18n::tr("Protected").into(),
        Integrity::Other(rid) => format!("0x{rid:04x}"),
    }
}

/// A process's services in a few words: up to two names, then how many more.
pub fn services_summary(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => crate::i18n::format("Service: {0}", std::slice::from_ref(one)),
        [a, b] => crate::i18n::format("Services: {0}, {1}", &[a.clone(), b.clone()]),
        [a, b, rest @ ..] => crate::i18n::format(
            "Services: {0}, {1} and {2} more",
            &[a.clone(), b.clone(), rest.len().to_string()],
        ),
    }
}

/// The local date and time a process started, to the second.
fn start_clock(p: &ProcessInfo) -> String {
    p.start_time()
        .map(|t| {
            chrono::DateTime::<chrono::Local>::from(t)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

/// `user` from `DOMAIN\user`, as Task Manager shows it.
pub fn short_user(account: &str) -> &str {
    account.rsplit('\\').next().unwrap_or(account)
}

/// Yes, No, or a dash when the token could not be read.
fn elevated_label(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => crate::i18n::tr("Yes"),
        Some(false) => crate::i18n::tr("No"),
        None => "—",
    }
}

/// What the user asked for this frame; handled by the app.
pub enum Intent {
    /// Open the live graph for an overview resource card.
    ShowResource(crate::overview::Resource),
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
    /// Open the Services tab with the services this process hosts.
    ShowServices(u32),
    ShowContainers,
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
    /// Ask the AI assistant what the process is and what to do about it.
    AskAi(ProcessRef),
    /// Select the process and make sure its details are on screen (Enter).
    Details(ProcessRef),
    /// Ask before restarting the desktop's Explorer.
    RestartExplorer,
    /// Open the Windows settings page for other users' accounts.
    ManageAccounts,
    /// Ask before signing out the user of this session.
    SignOut(u32, String),
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

/// When the list shows one group per user account.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UserGrouping {
    Always,
    Never,
    /// Only while more than one person runs programs on this PC.
    #[default]
    #[serde(other)]
    Auto,
}

/// Which processes "Hide system processes" hides.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SystemFilter {
    /// Image names never hidden, although they belong to Windows.
    pub always_show: Vec<String>,
    /// Image names hidden with the system processes.
    pub also_hide: Vec<String>,
}

impl Default for SystemFilter {
    fn default() -> Self {
        Self {
            always_show: [
                "explorer.exe",
                "cmd.exe",
                "powershell.exe",
                "notepad.exe",
                "mmc.exe",
                "regedit.exe",
                "mstsc.exe",
            ]
            .map(String::from)
            .to_vec(),
            also_hide: Vec::new(),
        }
    }
}

impl SystemFilter {
    /// Whether `p` is part of Windows: a core or kernel process, a service
    /// session process from the Windows folder (or one whose image cannot be
    /// read there), or a helper from System32, SysWOW64 or SystemApps.
    pub fn is_system(&self, p: &ProcessInfo, image_path: Option<&str>) -> bool {
        let listed =
            |names: &[String]| names.iter().any(|n| n.trim().eq_ignore_ascii_case(&p.name));
        if listed(&self.always_show) {
            return false;
        }
        if listed(&self.also_hide)
            || wtm_core::protection_reason(p.pid, &p.name).is_some_and(|r| r != "RustWTM itself")
        {
            return true;
        }
        let path = image_path.unwrap_or("").to_lowercase();
        let root = windows_root();
        let in_windows = !root.is_empty() && path.starts_with(&root);
        if p.session_id == 0 && (path.is_empty() || in_windows) {
            return true;
        }
        in_windows
            && ["system32\\", "syswow64\\", "systemapps\\"]
                .iter()
                .any(|dir| path[root.len()..].starts_with(dir))
    }
}

/// `c:\windows\` in lower case, or empty when unknown.
fn windows_root() -> String {
    static ROOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        std::env::var("SystemRoot")
            .map(|r| format!("{}\\", r.trim_end_matches('\\').to_lowercase()))
            .unwrap_or_default()
    })
    .clone()
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
    pub column_order: Vec<Col>,
    column_editor: Option<crate::process_columns::Editor>,
    column_editor_requested: bool,
    table_layout: Vec<Col>,
    /// Shade CPU, memory, disk and GPU cells by how much they use.
    pub heat: bool,
    pub stripes: bool,
    pub descriptive_names: bool,
    fit_widths: bool,
    reset_widths: bool,
    pub hold_sorting: bool,
    /// Hide processes that are part of Windows.
    pub hide_system: bool,
    pub system: SystemFilter,
    /// Tree nodes start collapsed ("Collapse all") or expanded.
    pub tree_collapsed: bool,
    /// Tree nodes the user opened or closed against `tree_collapsed`.
    tree_toggled: HashSet<(u32, i64)>,
    pub user_grouping: UserGrouping,
    /// User groups closed in the list (accounts in lower case).
    collapsed_users: HashSet<String>,
    /// Open the tree and user group down to this process on the next rows.
    expand_to: Option<(u32, i64)>,
    pub filter: QuickFilter,
    pub more: MoreFilters,
    /// Minutes last chosen for the "started in the last" filter.
    pub recent_minutes: u32,
    /// Only the process table is frozen. Sampling, protection and alerts continue.
    pub paused: Option<std::sync::Arc<Snapshot>>,
    /// The list as it was when Ctrl was pressed; dropped when Ctrl is let go.
    ctrl_hold: Option<std::sync::Arc<Snapshot>>,
    /// Last selection revealed automatically. Sampling must not keep pulling
    /// the table back after the user scrolls away to inspect another row.
    revealed: Option<(u32, i64)>,
    /// The filtered and sorted rows of an earlier frame. Every frame draws
    /// the table, but the rows only change with the data or the settings.
    cache: Option<RowCache>,
    /// Only ordering is held. Every row still reads the current snapshot.
    inspection_held: bool,
    inspection_rect: Option<egui::Rect>,
    last_order: Option<StableOrder>,
}

/// One table row: a process (index into the snapshot) at a tree depth, or
/// an app (index into the groups).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Process(usize, usize),
    App(usize),
    /// A user account (index into the groups).
    User(usize),
}

/// Everything the rows are built from. `revision` stands for the data the
/// window received (snapshot, labels, command lines, hung windows).
#[derive(Debug, Clone, PartialEq)]
struct RowKey {
    inspection_held: bool,
    revision: u64,
    snapshot: usize,
    processes: usize,
    search: String,
    mode: ViewMode,
    sort: Col,
    descending: bool,
    descriptive_names: bool,
    filter: QuickFilter,
    more: MoreFilters,
    expanded: HashSet<String>,
    hide_system: bool,
    system: SystemFilter,
    tree_collapsed: bool,
    tree_toggled: HashSet<(u32, i64)>,
    /// Effective: `Auto` already resolved.
    by_user: bool,
    collapsed_users: HashSet<String>,
    /// The "started in the last" filter moves with the clock.
    second: Option<u64>,
}

struct RowCache {
    key: RowKey,
    rows: Vec<Row>,
    groups: Vec<AppGroup>,
    /// Tree rows (snapshot indices) that have children.
    parents: HashSet<usize>,
    /// Processes that match the filters.
    shown: usize,
}

struct StableOrder {
    key: RowKey,
    processes: HashMap<(u32, i64), usize>,
    apps: HashMap<String, usize>,
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
            column_order: crate::process_columns::default_order(),
            column_editor: None,
            column_editor_requested: false,
            table_layout: Vec::new(),
            heat: true,
            stripes: false,
            descriptive_names: true,
            fit_widths: false,
            reset_widths: false,
            hold_sorting: true,
            hide_system: false,
            system: SystemFilter::default(),
            tree_collapsed: false,
            tree_toggled: HashSet::new(),
            user_grouping: UserGrouping::Auto,
            collapsed_users: HashSet::new(),
            expand_to: None,
            filter: QuickFilter::All,
            more: MoreFilters::default(),
            recent_minutes: 5,
            paused: None,
            ctrl_hold: None,
            revealed: None,
            cache: None,
            inspection_held: false,
            inspection_rect: None,
            last_order: None,
        }
    }
}

impl ProcessView {
    fn columns_menu(&mut self, ui: &mut Ui, grouped: bool) {
        ui.menu_button(crate::i18n::tr("Column profiles"), |ui| {
            for profile in crate::process_columns::Profile::ALL {
                if crate::context_actions::action(ui, profile.label()) {
                    self.apply_profile(profile);
                    egui::Popup::close_all(ui.ctx());
                }
            }
        });
        if grouped {
            for (family, label) in ["Process", "CPU", "Memory", "Disk", "Network", "GPU"]
                .into_iter()
                .enumerate()
            {
                let columns = COLUMNS.iter().filter(|c| column_family(c.0) == family);
                let visible = columns
                    .clone()
                    .filter(|c| self.visible.contains(&c.0))
                    .count();
                let total = columns.clone().count();
                ui.menu_button(
                    format!("{}  {visible}/{total}", crate::i18n::tr(label)),
                    |ui| {
                        for (col, label, ..) in columns {
                            self.column_checkbox(ui, *col, label);
                        }
                    },
                );
            }
        }
        crate::theme::divider(ui);
        if crate::context_actions::action(ui, crate::i18n::tr("Arrange columns…")) {
            self.column_editor = Some(crate::process_columns::Editor::new(
                &self.visible,
                &self.column_order,
            ));
        }
        if crate::context_actions::action(ui, crate::i18n::tr("Fit columns")) {
            self.fit_widths = true;
            ui.ctx().request_repaint();
        }
        if crate::context_actions::action(ui, crate::i18n::tr("Restore default columns")) {
            self.apply_columns(crate::process_columns::Selection::default());
            self.reset_widths = true;
            ui.ctx().request_repaint();
        }
        crate::theme::divider(ui);
        if grouped {
            ui.menu_button(crate::i18n::tr("Appearance"), |ui| {
                self.table_appearance(ui)
            });
        } else {
            self.table_appearance(ui);
        }
        if !grouped {
            crate::theme::divider(ui);
            egui::ScrollArea::vertical()
                .id_salt("column-visibility-menu")
                .max_height((ui.ctx().content_rect().height() - 320.0).max(100.0))
                .show(ui, |ui| {
                    for (column, label, ..) in COLUMNS.iter().skip(1) {
                        self.column_checkbox(ui, *column, label);
                    }
                });
        }
    }

    fn table_appearance(&mut self, ui: &mut Ui) {
        ui.checkbox(&mut self.heat, crate::i18n::tr("Heat map"));
        ui.checkbox(
            &mut self.stripes,
            crate::i18n::tr("Alternating row backgrounds"),
        );
        ui.checkbox(
            &mut self.descriptive_names,
            crate::i18n::tr("Show process descriptions"),
        );
        crate::theme::TableLines::controls(ui);
    }

    fn column_checkbox(&mut self, ui: &mut Ui, column: Col, label: &str) {
        let mut visible = self.visible.contains(&column);
        if ui
            .add_enabled(
                column != Col::Name,
                egui::Checkbox::new(&mut visible, crate::i18n::tr(label)),
            )
            .changed()
        {
            if visible {
                self.visible.insert(column);
            } else {
                self.visible.remove(&column);
            }
        }
    }

    pub fn normalize_columns(&mut self) {
        self.visible.remove(&Col::Unknown);
        self.visible.insert(Col::Name);
        crate::process_columns::normalize_order(&mut self.column_order);
        if self.sort == Col::Unknown || !self.visible.contains(&self.sort) {
            self.sort = Col::Name;
            self.descending = false;
        }
    }

    pub fn apply_profile(&mut self, profile: crate::process_columns::Profile) {
        self.apply_columns(profile.selection());
        self.sort = profile.sort();
        self.descending = self.sort != Col::Name;
    }

    pub fn open_column_editor(&mut self) {
        self.column_editor_requested = true;
    }

    fn apply_columns(&mut self, selection: crate::process_columns::Selection) {
        self.visible = selection.visible;
        self.column_order = selection.order;
        self.normalize_columns();
    }

    fn edit_columns(&mut self, ctx: &egui::Context, was_open: bool) {
        if was_open
            && let Some(selection) =
                crate::process_columns::show_editor(ctx, &mut self.column_editor)
        {
            self.apply_columns(selection);
        }
    }

    /// A request from another page must show the target, even if the last
    /// process search or a frozen snapshot would otherwise hide it.
    pub fn reveal(&mut self, target: ProcessRef) {
        self.search.clear();
        self.filter = QuickFilter::All;
        self.more = MoreFilters::default();
        self.paused = None;
        self.ctrl_hold = None;
        if self.mode == ViewMode::Apps {
            self.expanded.insert(target.name.to_lowercase());
        }
        self.expand_to = Some((target.pid, target.create_time));
        self.selected = Some(target);
        self.revealed = None;
        self.last_order = None;
        self.cache = None;
    }

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
            crate::theme::divider(ui);
            ui.strong(crate::i18n::tr("Activity"));
            ui.checkbox(
                &mut self.more.disk_active,
                crate::i18n::tr("Disk active (1 MiB/s or more)"),
            );
            ui.checkbox(
                &mut self.more.gpu_active,
                crate::i18n::tr("GPU active (5% or more)"),
            );
            crate::theme::divider(ui);
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
                crate::theme::divider(ui);
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
                    .corner_radius(0.0),
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

fn column_family(col: Col) -> usize {
    match col {
        Col::Cpu | Col::CpuTime | Col::Threads | Col::Priority => 1,
        Col::Memory
        | Col::Commit
        | Col::WorkingSet
        | Col::PeakWorkingSet
        | Col::PagedPool
        | Col::NonPagedPool
        | Col::PageFaults
        | Col::PageFaultRate => 2,
        Col::Disk | Col::DiskRead | Col::DiskWrite => 3,
        Col::Network => 4,
        Col::Gpu | Col::GpuMemory | Col::GpuEngine | Col::SharedGpuMemory => 5,
        _ => 0,
    }
}

fn compare(
    a: &ProcessInfo,
    b: &ProcessInfo,
    col: Col,
    extra: &Extra<'_>,
    descriptions: bool,
) -> Ordering {
    match col {
        Col::Name if descriptions => crate::sort::text(
            extra.description(a).unwrap_or(&a.name),
            extra.description(b).unwrap_or(&b.name),
        ),
        Col::Name | Col::Unknown => crate::sort::text(&a.name, &b.name),
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
        Col::User => crate::sort::text(
            extra.user(a).map_or("", short_user),
            extra.user(b).map_or("", short_user),
        ),
        Col::Architecture => crate::sort::text(&extra.architecture(a), &extra.architecture(b)),
        Col::Elevated => extra.elevated(a).cmp(&extra.elevated(b)),
        Col::FileVersion => crate::sort::text(extra.version(a), extra.version(b)),
        Col::ParentPid => a.parent_pid.cmp(&b.parent_pid),
        Col::ImagePath => crate::sort::text(
            extra.image_path(a).unwrap_or(""),
            extra.image_path(b).unwrap_or(""),
        ),
        // Earliest first when ascending.
        Col::StartTime => a.create_time.cmp(&b.create_time),
        Col::Integrity => extra.integrity(a).cmp(&extra.integrity(b)),
        Col::CpuTime => a.cpu_time.cmp(&b.cpu_time),
        Col::DiskRead => a.io_read_rate.total_cmp(&b.io_read_rate),
        Col::DiskWrite => a.io_write_rate.total_cmp(&b.io_write_rate),
        Col::Status => status(a, extra).cmp(&status(b, extra)),
        Col::PeakWorkingSet => a.memory.peak_working_set.cmp(&b.memory.peak_working_set),
        Col::PagedPool => a.memory.paged_pool.cmp(&b.memory.paged_pool),
        Col::NonPagedPool => a.memory.nonpaged_pool.cmp(&b.memory.nonpaged_pool),
        Col::PageFaults => a.memory.page_faults.cmp(&b.memory.page_faults),
        Col::PageFaultRate => a
            .memory
            .page_fault_rate
            .total_cmp(&b.memory.page_fault_rate),
        Col::GpuEngine => crate::sort::text(&extra.gpu_engine(a), &extra.gpu_engine(b)),
        Col::GdiObjects => extra.gui(a).map(|g| g.0).cmp(&extra.gui(b).map(|g| g.0)),
        Col::UserObjects => extra.gui(a).map(|g| g.1).cmp(&extra.gui(b).map(|g| g.1)),
        Col::SharedGpuMemory => a.gpu_detail.shared_memory.cmp(&b.gpu_detail.shared_memory),
    }
    .then(a.pid.cmp(&b.pid))
}

/// A process's state, from running to not responding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Status {
    Running,
    Suspended,
    NotResponding,
}

fn status(p: &ProcessInfo, extra: &Extra<'_>) -> Status {
    if p.suspended {
        Status::Suspended
    } else if extra.hung.contains_key(&p.pid) {
        Status::NotResponding
    } else {
        Status::Running
    }
}

/// Processor time as Task Manager writes it: `1:02:03` (hours unbounded).
fn cpu_time_text(d: std::time::Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
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

/// `tree_rows` (a test helper) as (index, depth) pairs.
#[cfg(test)]
fn tree_order(
    procs: &[ProcessInfo],
    col: Col,
    descending: bool,
    extra: &Extra<'_>,
) -> Vec<(usize, usize)> {
    let all: Vec<usize> = (0..procs.len()).collect();
    tree_order_by(
        procs,
        &all,
        false,
        |a, b| {
            let o = compare(a, b, col, extra, false);
            if descending { o.reverse() } else { o }
        },
        |_| false,
    )
    .0
}

/// Depth-first rows of the processes at `members` (indices into `procs`),
/// without the descendants of `collapsed` ones, and every member that has
/// children. A member whose parent is not a member becomes a root.
fn tree_order_by(
    procs: &[ProcessInfo],
    members: &[usize],
    ordered_fallback: bool,
    order: impl Fn(&ProcessInfo, &ProcessInfo) -> Ordering,
    collapsed: impl Fn(usize) -> bool,
) -> (Vec<(usize, usize)>, HashSet<usize>) {
    let index: HashMap<u32, usize> = members.iter().map(|&i| (procs[i].pid, i)).collect();
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut roots = Vec::new();
    for &i in members {
        let p = &procs[i];
        match index.get(&p.parent_pid) {
            Some(&parent) if parent != i && procs[parent].create_time <= p.create_time => {
                children.entry(parent).or_default().push(i);
            }
            _ => roots.push(i),
        }
    }
    let sort = |v: &mut Vec<usize>| {
        v.sort_by(|&a, &b| order(&procs[a], &procs[b]));
    };
    sort(&mut roots);
    children.values_mut().for_each(sort);

    let mut out = Vec::with_capacity(members.len());
    let mut seen = vec![false; procs.len()];
    let mut stack = Vec::new();
    // Processes that are only reachable through a parent cycle (same create
    // time, PIDs reused) are appended as extra roots so none goes missing.
    let mut fallback = members.to_vec();
    if ordered_fallback {
        sort(&mut fallback);
    }
    for start in roots.into_iter().chain(fallback) {
        stack.push((start, 0, false));
        while let Some((i, depth, hidden)) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            // Hidden under a collapsed ancestor: seen, so the fallback does
            // not show it as a root, but not listed.
            if !hidden {
                out.push((i, depth));
            }
            if let Some(kids) = children.get(&i) {
                let hide = hidden || collapsed(i);
                stack.extend(kids.iter().rev().map(|&k| (k, depth + 1, hide)));
            }
        }
    }
    (out, children.into_keys().collect())
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
    let alpha = (24.0 + 112.0 * f) as u8;
    Some(Color32::from_rgba_unmultiplied(255, green, 40, alpha))
}

/// Disk I/O that counts as a fully busy cell, bytes per second.
const DISK_HEAT_FULL: f64 = 100.0 * 1024.0 * 1024.0;

use crate::theme::right;

/// Below this table width the name column starts narrower.
const NARROW_TABLE: f32 = 1000.0;
const NARROW_NAME: f32 = 180.0;

#[derive(Clone, Copy)]
enum Step {
    By(isize),
    First,
    Last,
}

#[derive(Clone, Copy)]
enum End {
    /// Delete or Alt+E: confirm first.
    Ask,
    /// Shift+Delete: no confirmation.
    Now,
}

/// The list keys pressed this frame.
#[derive(Default)]
struct ListKeys {
    step: Option<Step>,
    end: Option<End>,
    details: bool,
    focus_search: bool,
    /// Letters and digits typed onto the list, which start a search.
    typed: String,
}

/// How far Page Up/Down move.
const PAGE: isize = 10;

impl ListKeys {
    fn read(i: &egui::InputState) -> Self {
        let m = i.modifiers;
        let plain = !m.ctrl && !m.alt && !m.command;
        let pressed = |key| i.key_pressed(key);
        let step = [
            (egui::Key::ArrowUp, Step::By(-1)),
            (egui::Key::ArrowDown, Step::By(1)),
            (egui::Key::PageUp, Step::By(-PAGE)),
            (egui::Key::PageDown, Step::By(PAGE)),
            (egui::Key::Home, Step::First),
            (egui::Key::End, Step::Last),
        ]
        .into_iter()
        .find(|(key, _)| plain && !m.shift && pressed(*key))
        .map(|(_, step)| step);
        let end = if plain && pressed(egui::Key::Delete) {
            Some(if m.shift { End::Now } else { End::Ask })
        } else if m.alt && !m.ctrl && !m.shift && pressed(egui::Key::E) {
            Some(End::Ask)
        } else {
            None
        };
        let mut keys = Self {
            step,
            end,
            details: plain && !m.shift && pressed(egui::Key::Enter),
            ..Self::default()
        };
        if plain {
            for event in &i.events {
                if let egui::Event::Text(text) = event {
                    if text == "/" {
                        keys.focus_search = true;
                    } else if keys.focus_search || text.chars().any(char::is_alphanumeric) {
                        keys.typed.push_str(text);
                    }
                }
            }
        }
        keys
    }
}

/// The process rows of the table, without app and user group headings.
fn process_rows<'a>(rows: &[Row], all: &'a [ProcessInfo]) -> Vec<&'a ProcessInfo> {
    rows.iter()
        .filter_map(|r| match *r {
            Row::Process(i, _) => Some(&all[i]),
            Row::App(_) | Row::User(_) => None,
        })
        .collect()
}

pub(crate) const SEARCH_ID: &str = "process_search";

/// Focuses the process search with the cursor after `text`, so typing goes on
/// where the list left off.
fn focus_search_at_end(ctx: &egui::Context, text: &str) {
    let id = egui::Id::unique(SEARCH_ID);
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    let end = egui::text::CCursor::new(text.chars().count());
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(end)));
    state.store(ctx, id);
    ctx.memory_mut(|m| m.request_focus(id));
}

/// Columns whose values (and so their headers) are aligned right.
fn right_aligned(col: Col) -> bool {
    !matches!(
        col,
        Col::Name
            | Col::Limits
            | Col::CommandLine
            | Col::User
            | Col::Architecture
            | Col::Elevated
            | Col::FileVersion
            | Col::ImagePath
            | Col::StartTime
            | Col::Integrity
            | Col::Status
            | Col::GpuEngine
    )
}

impl ProcessView {
    /// List keys, ignored while a text field (the search), a menu or a
    /// dialog has the keyboard:
    /// - Up/Down, Page Up/Down, Home/End move the selection.
    /// - Enter shows the selected process's details.
    /// - Delete or Alt+E ask to end it; Shift+Delete ends it without asking.
    /// - `/` moves to the search; typing a letter or digit starts a search
    ///   with it.
    fn keyboard(&mut self, ui: &Ui, rows: &[Row], all: &[ProcessInfo], intents: &mut Vec<Intent>) {
        let ctx = ui.ctx();
        if ctx.text_edit_focused()
            || egui::Popup::is_any_open(ctx)
            || ctx.memory(|m| m.top_modal_layer().is_some())
        {
            return;
        }
        let keys = ui.input(ListKeys::read);
        if keys.focus_search || !keys.typed.is_empty() {
            self.search.push_str(&keys.typed);
            focus_search_at_end(ctx, &self.search);
        }
        let Some(step) = keys.step else {
            return self.act_on_selection(rows, all, keys, intents);
        };
        let procs = process_rows(rows, all);
        let current = self.position_in(&procs);
        let last = procs.len().saturating_sub(1);
        let next = match (current, step) {
            (_, Step::First) => 0,
            (_, Step::Last) => last,
            (Some(i), Step::By(delta)) => i.saturating_add_signed(delta).min(last),
            (None, _) => 0,
        };
        if !procs.is_empty() && current != Some(next) {
            intents.push(Intent::Select(ProcessRef::from(procs[next])));
        }
    }

    fn position_in(&self, procs: &[&ProcessInfo]) -> Option<usize> {
        let s = self.selected.as_ref()?;
        procs
            .iter()
            .position(|p| p.pid == s.pid && p.create_time == s.create_time)
    }

    fn act_on_selection(
        &self,
        rows: &[Row],
        all: &[ProcessInfo],
        keys: ListKeys,
        intents: &mut Vec<Intent>,
    ) {
        if keys.end.is_none() && !keys.details {
            return;
        }
        let procs = process_rows(rows, all);
        let Some(target) = self.position_in(&procs).map(|i| ProcessRef::from(procs[i])) else {
            return;
        };
        if keys.details {
            intents.push(Intent::Details(target.clone()));
        }
        match keys.end {
            Some(End::Ask) => intents.push(Intent::Confirm(target, Command::Kill)),
            Some(End::Now) => intents.push(Intent::Command(target, Command::Kill)),
            None => {}
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
        let editor_was_open = self.column_editor.is_some();
        // Navigation can occur on a pointer-release frame. Open on the next
        // process frame so that release cannot dismiss the new modal.
        if std::mem::take(&mut self.column_editor_requested) {
            self.column_editor = Some(crate::process_columns::Editor::new(
                &self.visible,
                &self.column_order,
            ));
            ui.ctx().request_repaint();
        }

        ui.horizontal_wrapped(|ui| {
            crate::theme::search_bar(
                ui,
                egui::Id::unique(SEARCH_ID),
                &mut self.search,
                crate::i18n::tr("name, PID, path or command line"),
                if ui.available_width() < 600.0 { 150.0 } else { 240.0 },
            );
            egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(0.0).inner_margin(0.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (mode, label) in [
                        (ViewMode::List, crate::i18n::tr("List")),
                        (ViewMode::Tree, crate::i18n::tr("Tree")),
                        (ViewMode::Apps, crate::i18n::tr("Apps")),
                    ] {
                        crate::theme::choice(ui, &mut self.mode, mode, label);
                    }
                });
            });
            ui.menu_button(crate::i18n::tr("Columns"), |ui| self.columns_menu(ui, false));
            let toggle_key = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F6))
                && !ui.ctx().text_edit_focused()
                && !egui::Popup::is_any_open(ui.ctx())
                && !ui.memory(|m| m.top_modal_layer().is_some());
            if crate::hold::ctrl_held(ui.ctx()) && self.paused.is_none() {
                self.ctrl_hold
                    .get_or_insert_with(|| std::sync::Arc::new(snap.clone()));
            } else {
                self.ctrl_hold = None;
            }
            if ui.button(if self.paused.is_some() { crate::i18n::tr("Resume updates") } else { crate::i18n::tr("Pause updates") })
                .on_hover_text(crate::i18n::tr("Freeze this process list for inspection (F6), or hold Ctrl for a quick look. Monitoring, alerts and protection continue."))
                .clicked() || toggle_key {
                // Pausing while Ctrl is held keeps the list Ctrl froze.
                self.paused = match self.paused.take() {
                    Some(_) => None,
                    None => Some(
                        self.ctrl_hold
                            .take()
                            .unwrap_or_else(|| std::sync::Arc::new(snap.clone())),
                    ),
                };
            }
        });
        self.normalize_columns();
        let ctrl_held = self.paused.is_none() && self.ctrl_hold.is_some();
        let paused = self.paused.clone().or_else(|| self.ctrl_hold.clone());
        let snap = paused.as_deref().unwrap_or(snap);
        let compact_filters = ui.available_width() < 600.0;
        ui.horizontal_wrapped(|ui| {
            let filters = [
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
            ];
            if compact_filters {
                let label = filters.iter().find(|f| f.0 == self.filter).unwrap().1;
                ui.menu_button(label, |ui| {
                    for (filter, label, hint) in filters {
                        if ui
                            .selectable_value(&mut self.filter, filter, label)
                            .on_hover_text(hint)
                            .clicked()
                        {
                            ui.close();
                        }
                    }
                });
            } else {
                for (filter, label, hint) in filters {
                    crate::theme::choice(ui, &mut self.filter, filter, label).on_hover_text(hint);
                }
            }
            crate::theme::divider(ui);
            let mut recent = self.more.started_within.is_some();
            if !compact_filters && ui
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
            ui.toggle_value(
                &mut self.hide_system,
                crate::i18n::tr("Hide system processes"),
            )
            .on_hover_text(crate::i18n::tr(
                "Hide Windows components and services. Edit the program lists in Settings > Processes.",
            ));
            self.filter_chips(ui);
            if paused.is_some() {
                ui.label(
                    RichText::new(crate::i18n::tr(if ctrl_held {
                        "Paused while Ctrl is held"
                    } else {
                        "Paused · monitoring continues"
                    }))
                        .color(crate::theme::text_color(ui, crate::theme::WARN)),
                );
            }
        });
        crate::theme::divider(ui);

        self.inspection_held = self.hold_sorting
            && ui.input(|i| i.raw.focused)
            && (self.inspection_rect.is_some_and(|rect| {
                ui.input(|i| i.pointer.hover_pos().is_some_and(|pos| rect.contains(pos)))
            }) || egui::Popup::is_any_open(ui.ctx())
                || ui.memory(|m| m.top_modal_layer().is_some()));
        let cache = self.rows(snap, extra, revision);
        let (rows, groups, shown) = (&cache.rows, &cache.groups, cache.shown);
        let parents = &cache.parents;
        let has_user_rows = rows.iter().any(|r| matches!(r, Row::User(_)));
        let mut toggled_node = None;
        ui.horizontal(|ui| {
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
            if self.inspection_held && paused.is_none() {
                ui.add(
                    egui::Label::new(
                        RichText::new(crate::i18n::tr("Order held · values are live"))
                            .small()
                            .weak(),
                    )
                    .truncate(),
                ).on_hover_text(crate::i18n::tr("Automatic sorting resumes after the pointer leaves the table and open menus or dialogs close."));
            }
        });
        if rows.is_empty() {
            self.inspection_rect = None;
            crate::theme::empty_state(
                ui,
                crate::i18n::tr("No matching processes"),
                crate::i18n::tr("Try another name, PID or resource filter."),
            );
            if ui.button(crate::i18n::tr("Clear filters")).clicked() {
                self.search.clear();
                self.filter = QuickFilter::All;
                self.more = MoreFilters::default();
            }
            self.keep(cache, revision);
            self.edit_columns(ui.ctx(), editor_was_open);
            return intents;
        }
        let layout: Vec<_> = self
            .column_order
            .iter()
            .copied()
            .filter(|c| self.visible.contains(c))
            .collect();
        if layout != self.table_layout {
            self.revealed = None;
            self.table_layout.clone_from(&layout);
        }
        let row_height = ui.spacing().interact_size.y;
        let memory_total = snap.system.memory_total.max(1) as f64;
        self.keyboard(ui, rows, &snap.processes, &mut intents);
        let selected = self.selected.as_ref().map(|r| (r.pid, r.create_time));
        let reveal_row = (self.revealed != selected)
            .then(|| selected.and_then(|key| rows.iter().position(|row| {
                matches!(row, Row::Process(i, _) if (snap.processes[*i].pid, snap.processes[*i].create_time) == key)
            })))
            .flatten();
        self.revealed = selected;

        let columns: Vec<_> = layout
            .iter()
            .filter_map(|col| COLUMNS.iter().find(|c| c.0 == *col))
            .collect();
        let fit_widths = std::mem::take(&mut self.fit_widths);
        let reset_widths = std::mem::take(&mut self.reset_widths);

        // Every header fits in its column at first, with room for the sort
        // arrow; the name column gives way on small windows.
        let narrow = ui.available_width() < NARROW_TABLE;
        let widths: Vec<f32> = columns
            .iter()
            .map(|(col, label, width, ..)| {
                let width = if *col == Col::Name && narrow && !self.descriptive_names {
                    NARROW_NAME
                } else {
                    *width
                };
                crate::sort::width(ui, label, width)
            })
            .collect();

        // Selectable text would swallow clicks meant for the row.
        ui.style_mut().interaction.selectable_labels = false;
        // A scroll bar that stays visible: on small windows the columns go
        // on to the right.
        let scroll_style = ui.spacing().scroll;
        ui.spacing_mut().scroll = crate::theme::solid_scroll();
        // Row indices move with live sorting. An open menu belongs to its
        // process identity (including PID reuse), never to that row position.
        let menu_id = ui.scope_id().with("process_context_menu");
        egui::ScrollArea::horizontal()
            .id_salt("process_columns")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().scroll = scroll_style;
                let grid = ui.visuals().widgets.noninteractive.bg_stroke.color.gamma_multiply(0.6);
                ui.visuals_mut().widgets.noninteractive.bg_stroke.color = grid;
                let width: f32 = widths.iter().sum();
                ui.set_min_width(width + ui.spacing().item_spacing.x * columns.len() as f32);
                let mut table = crate::theme::table(ui)
                    .id_salt(("process-table-layout", &layout))
                    .striped(self.stripes)
                    .resizable(true)
                    .sense(Sense::click())
                    .cell_layout(Layout::left_to_right(Align::Center));
                if let Some(row) = reveal_row {
                    table = table.scroll_to_row(row, None);
                }
                for (i, ((col, ..), width)) in columns.iter().zip(&widths).enumerate() {
                    let column = match col {
                        Col::Name => Column::initial(*width).at_least(140.0).clip(true),
                        _ if i == columns.len() - 1 => {
                            Column::remainder().at_least(*width).clip(true)
                        }
                        _ => Column::initial(*width).clip(true),
                    };
                    table = table.column(if fit_widths {
                        let maximum: f32 = match col {
                            Col::Name => 540.0,
                            Col::CommandLine | Col::ImagePath => 640.0,
                            _ => 280.0,
                        };
                        column.at_most(maximum.max(*width)).auto_size_this_frame(true)
                    } else {
                        column
                    });
                }
                if reset_widths {
                    table.reset();
                }
                table
                    .header(row_height * 2.0, |mut header| {
                        for (col, label, _, numeric, _) in &columns {
                            let label = crate::i18n::tr(label);
                            header.col(|ui| crate::theme::table_cell(ui, |ui| {
                                ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                let total = match col {
                                    Col::Cpu => Some(format!("{:.0}%", snap.system.cpu_percent)),
                                    Col::Memory if snap.system.memory_total > 0 => Some(format!("{:.0}%", snap.system.memory_used() as f64 * 100.0 / snap.system.memory_total as f64)),
                                    Col::Disk => Some(format!("{}/s", format_bytes(snap.system.io_read_rate + snap.system.io_write_rate))),
                                    Col::Network => Some(format!("{}/s", format_bytes(snap.system.net_rx_rate + snap.system.net_tx_rate))),
                                    Col::Gpu => Some(format!("{:.0}%", snap.system.gpu_percent)),
                                    _ => None,
                                };
                                if ui.is_sizing_pass() {
                                    if let Some(total) = total {
                                        ui.strong(total);
                                    }
                                    ui.strong(format!("{label} ↓"));
                                    return;
                                }
                                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), row_height), Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_sized([ui.available_width(), row_height], egui::Label::new(RichText::new(total.unwrap_or_default()).strong()).truncate())
                                        .on_hover_text(crate::i18n::tr("System-wide total; filters only change the rows. Memory is physical use; disk is process I/O; network includes all adapters."));
                                });
                                let current = (self.sort == *col).then_some(self.descending);
                                let response = crate::sort::cell_response(ui, label, right_aligned(*col), current);
                                if response.clicked() {
                                    if self.sort == *col {
                                        self.descending = !self.descending;
                                    } else {
                                        self.sort = *col;
                                        self.descending = *numeric;
                                    }
                                }
                                crate::context_actions::menu(&response, (menu_id, "header", col), |ui| {
                                    ui.strong(label);
                                    if crate::context_actions::action(ui, crate::i18n::tr("Sort ascending")) {
                                        self.sort = *col;
                                        self.descending = false;
                                    }
                                    if crate::context_actions::action(ui, crate::i18n::tr("Sort descending")) {
                                        self.sort = *col;
                                        self.descending = true;
                                    }
                                    if *col != Col::Name && crate::context_actions::action(ui, crate::i18n::tr("Hide")) {
                                        self.visible.remove(col);
                                        self.normalize_columns();
                                    }
                                    if self.mode == ViewMode::Tree || has_user_rows {
                                        crate::theme::divider(ui);
                                        if crate::context_actions::action(ui, crate::i18n::tr("Expand all")) {
                                            self.expand_all();
                                        }
                                        if crate::context_actions::action(ui, crate::i18n::tr("Collapse all")) {
                                            self.collapse_all();
                                            self.collapsed_users.extend(groups.iter().map(|g| g.key.clone()));
                                        }
                                    }
                                    crate::theme::divider(ui);
                                    self.columns_menu(ui, true);
                                });
                                });
                            }));
                        }
                    })
                    .body(|mut body| {
                        self.inspection_rect = Some(body.max_rect().intersect(body.ui_mut().clip_rect()));
                        body.rows(row_height, rows.len(), |mut row| {
                            let (p, depth, group, user_row, index) = match rows[row.index()] {
                                Row::Process(i, depth) => (&snap.processes[i], depth, None, false, Some(i)),
                                Row::App(i) => (&groups[i].total, 0, Some(&groups[i]), false, None),
                                Row::User(i) => (&groups[i].total, 0, Some(&groups[i]), true, None),
                            };
                            let key = (p.pid, p.create_time);
                            row.set_selected(group.is_none() && selected == Some(key));

                            for (col, ..) in &columns {
                                row.col(|ui| crate::theme::table_cell(ui, |ui| {
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
                                            ui.max_rect().expand2(egui::vec2(6.0, 0.0)).shrink(0.5), 0.0,
                                            color,
                                        );
                                    }
                                    match col {
                                        Col::Name if user_row => {
                                            let g = group.expect("user rows have a group");
                                            let open = !self.collapsed_users.contains(&g.key);
                                            ui.label(if open { "⏷" } else { "⏵" });
                                            let account = g.members.first().and_then(|&i| extra.user(&snap.processes[i]));
                                            let name = account.map_or_else(
                                                || crate::i18n::tr("Unknown account").to_owned(),
                                                |a| short_user(a).to_owned(),
                                            );
                                            let response = ui.add(
                                                egui::Label::new(
                                                    RichText::new(format!("{name} ({})", g.members.len())).strong(),
                                                )
                                                .truncate(),
                                            );
                                            if let Some(account) = account {
                                                response.on_hover_text(account);
                                            }
                                        }
                                        Col::Name => {
                                            ui.add_space(depth as f32 * 14.0);
                                            if let Some(g) = group {
                                                let open = self.expanded.contains(&g.key);
                                                ui.label(if open { "⏷" } else { "⏵" });
                                            } else if self.mode == ViewMode::Tree
                                                && let Some(i) = index
                                                && parents.contains(&i)
                                            {
                                                let open = self.tree_collapsed
                                                    == self.tree_toggled.contains(&key);
                                                let arrow = ui
                                                    .add(egui::Button::new(if open { "⏷" } else { "⏵" }).frame(false).small())
                                                    .on_hover_text(if open {
                                                        crate::i18n::tr("Collapse")
                                                    } else {
                                                        crate::i18n::tr("Expand")
                                                    });
                                                if arrow.clicked() {
                                                    toggled_node = Some(key);
                                                }
                                            } else if self.mode == ViewMode::Tree {
                                                ui.add_space(14.0);
                                            }
                                            app_icon(ui, extra.icon(p));
                                            let description = self.descriptive_names.then(|| extra.description(p)).flatten();
                                            let name = description.unwrap_or(&p.name);
                                            let title = if let Some(g) = group {
                                                RichText::new(format!("{name} ({})", g.members.len())).strong()
                                            } else {
                                                RichText::new(name)
                                            };
                                            let mut job = egui::text::LayoutJob::default();
                                            title.append_to(&mut job, ui.style(), egui::FontSelection::Default, Align::Center);
                                            if description.is_some() {
                                                RichText::new(format!("  {}", p.name)).weak().small()
                                                    .append_to(&mut job, ui.style(), egui::FontSelection::Default, Align::Center);
                                            }
                                            let hosted = if group.is_none() { extra.hosted(p) } else { &[] };
                                            if !hosted.is_empty() {
                                                RichText::new(format!("  {}", services_summary(hosted))).weak().small()
                                                    .append_to(&mut job, ui.style(), egui::FontSelection::Default, Align::Center);
                                            }
                                            let label = egui::Label::new(job);
                                            let label = if ui.is_sizing_pass() { label.extend() } else { label.truncate() };
                                            ui.add(label);
                                            if p.suspended {
                                                ui.label(
                                                    RichText::new(crate::i18n::tr("Suspended"))
                                                        .color(crate::theme::text_color(
                                                            ui,
                                                            crate::theme::WARN,
                                                        ))
                                                        .small(),
                                                );
                                            } else if extra.hung.contains_key(&p.pid) {
                                                ui.label(
                                                    RichText::new(crate::i18n::tr(
                                                        "Not responding",
                                                    ))
                                                    .color(crate::theme::text_color(
                                                        ui,
                                                        crate::theme::BAD,
                                                    ))
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
                                        Col::DiskRead => {
                                            right(ui, format!("{}/s", format_bytes(p.io_read_rate)))
                                        }
                                        Col::DiskWrite => {
                                            right(ui, format!("{}/s", format_bytes(p.io_write_rate)))
                                        }
                                        Col::CpuTime => right(ui, cpu_time_text(p.cpu_time)),
                                        Col::PeakWorkingSet => {
                                            right(ui, format_bytes(p.memory.peak_working_set as f64))
                                        }
                                        Col::PagedPool => {
                                            right(ui, format_bytes(p.memory.paged_pool as f64))
                                        }
                                        Col::NonPagedPool => {
                                            right(ui, format_bytes(p.memory.nonpaged_pool as f64))
                                        }
                                        Col::PageFaults => right(
                                            ui,
                                            crate::perf_pages::count(p.memory.page_faults as f64),
                                        ),
                                        Col::GpuEngine => {
                                            ui.label(extra.gpu_engine(p));
                                        }
                                        Col::GdiObjects | Col::UserObjects => {
                                            // A group shows the sum of what was read.
                                            let read: Vec<(u32, u32)> = match group {
                                                Some(g) => g
                                                    .members
                                                    .iter()
                                                    .filter_map(|&i| extra.gui(&snap.processes[i]))
                                                    .collect(),
                                                None => extra.gui(p).into_iter().collect(),
                                            };
                                            if read.is_empty() {
                                                ui.with_layout(
                                                    Layout::right_to_left(Align::Center),
                                                    |ui| {
                                                        ui.weak("—").on_hover_text(crate::i18n::tr(
                                                            "Not readable with this window's rights",
                                                        ));
                                                    },
                                                );
                                            } else {
                                                let sum = read.iter().fold(0u64, |a, g| {
                                                    a + u64::from(if *col == Col::GdiObjects { g.0 } else { g.1 })
                                                });
                                                right(ui, sum.to_string());
                                            }
                                        }
                                        Col::SharedGpuMemory => right(
                                            ui,
                                            format_bytes(p.gpu_detail.shared_memory as f64),
                                        ),
                                        Col::PageFaultRate => right(
                                            ui,
                                            crate::perf_pages::count(p.memory.page_fault_rate),
                                        ),
                                        // Groups have no single state.
                                        Col::Status if group.is_some() => {}
                                        Col::Status => {
                                            let (text, color) = match status(p, extra) {
                                                Status::Running => (crate::i18n::tr("Running"), None),
                                                Status::Suspended => {
                                                    (crate::i18n::tr("Suspended"), Some(crate::theme::WARN))
                                                }
                                                Status::NotResponding => {
                                                    (crate::i18n::tr("Not responding"), Some(crate::theme::BAD))
                                                }
                                            };
                                            match color {
                                                Some(c) => ui.label(
                                                    RichText::new(text).color(crate::theme::text_color(ui, c)),
                                                ),
                                                None => ui.label(text),
                                            };
                                        }
                                        Col::Network if !snap.net_traced => {
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    ui.weak("—").on_hover_text(crate::i18n::tr(
                                                        "Per-process network needs the RustWTM service or administrator rights",
                                                    ));
                                                },
                                            );
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
                                                let color = crate::theme::text_color(
                                                    ui,
                                                    crate::theme::WARN,
                                                );
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(label).color(color),
                                                    )
                                                    .truncate(),
                                                )
                                                .on_hover_text(label);
                                            }
                                        }
                                        Col::Unknown => {},
                                        Col::CommandLine => {
                                            let cmd = extra.command_line(p);
                                            ui.label(RichText::new(cmd).monospace().small())
                                                .on_hover_text(cmd);
                                        }
                                        Col::User => match extra.user(p) {
                                            Some(account) => {
                                                ui.label(short_user(account)).on_hover_text(account);
                                            }
                                            None => {
                                                ui.weak("—").on_hover_text(crate::i18n::tr(
                                                    "Not readable with this window's rights",
                                                ));
                                            }
                                        },
                                        Col::Architecture => {
                                            let arch = extra.architecture(p);
                                            if arch.is_empty() {
                                                ui.weak("—");
                                            } else {
                                                ui.label(arch);
                                            }
                                        }
                                        Col::Elevated => {
                                            ui.label(elevated_label(extra.elevated(p)));
                                        }
                                        Col::FileVersion => {
                                            ui.label(extra.version(p));
                                        }
                                        Col::ParentPid => right(ui, p.parent_pid.to_string()),
                                        Col::ImagePath => match extra.image_path(p) {
                                            Some(path) => {
                                                ui.label(RichText::new(path).small())
                                                    .on_hover_text(path);
                                            }
                                            None => {
                                                ui.weak("—");
                                            }
                                        },
                                        Col::StartTime => {
                                            ui.label(start_clock(p));
                                        }
                                        Col::Integrity => match extra.integrity(p) {
                                            Some(level) => {
                                                ui.label(integrity_label(level));
                                            }
                                            None => {
                                                ui.weak("—").on_hover_text(crate::i18n::tr(
                                                    "Not readable with this window's rights",
                                                ));
                                            }
                                        },
                                    }
                                }));
                            }

                            let response = row.response();
                            let target = ProcessRef::from(p);
                            if user_row {
                                let g = group.expect("user rows have a group");
                                if response.clicked() && !self.collapsed_users.remove(&g.key) {
                                    self.collapsed_users.insert(g.key.clone());
                                }
                                crate::context_actions::menu(&response, (menu_id, "user", &g.key), |ui| {
                                    user_menu(ui, g, &snap.processes, extra, &mut intents);
                                });
                            } else if let Some(g) = group {
                                if response.clicked() && !self.expanded.remove(&g.key) {
                                    self.expanded.insert(g.key.clone());
                                }
                                crate::context_actions::menu(&response, (menu_id, "app", &g.key), |ui| {
                                        crate::context_actions::copy(ui, crate::i18n::tr("Name"), || p.name.clone());
                                        crate::theme::divider(ui);
                                        app_menu(ui, g, &snap.processes, &mut intents);
                                    });
                            } else {
                                if response.clicked() {
                                    intents.push(Intent::Select(target.clone()));
                                }
                                crate::context_actions::menu(&response, (menu_id, "process", key), |ui| {
                                        if crate::context_actions::action(ui, crate::i18n::tr("Details")) {
                                            intents.push(Intent::Select(target.clone()));
                                        }
                                        crate::context_actions::copy(ui, crate::i18n::tr("Name"), || p.name.clone());
                                        crate::context_actions::copy(ui, "PID", || p.pid.to_string());
                                        if let Some(path) = extra.image_path(p) {
                                            crate::context_actions::copy(ui, crate::i18n::tr("Path"), || path.to_owned());
                                        }
                                        crate::theme::divider(ui);
                                        context_menu(
                                            ui,
                                            &target,
                                            logical_cpus,
                                            extra.topology,
                                            if group.is_none() { extra.hosted(p) } else { &[] },
                                            &mut intents,
                                        )
                                    });
                            }
                        });
                    });
            });
        if let Some(key) = toggled_node {
            self.toggle_node(key);
            ui.ctx().request_repaint();
        }
        self.keep(cache, revision);
        self.edit_columns(ui.ctx(), editor_was_open);
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
        key.inspection_held == self.inspection_held
            && key.revision == revision
            && key.snapshot == std::ptr::from_ref(snap) as usize
            && key.processes == snap.processes.len()
            && key.search == self.search
            && key.mode == self.mode
            && key.sort == self.sort
            && key.descending == self.descending
            && key.descriptive_names == self.descriptive_names
            && key.filter == self.filter
            && key.more == self.more
            && key.second == second
            && (self.mode != ViewMode::Apps || key.expanded == self.expanded)
            && key.hide_system == self.hide_system
            && key.system == self.system
            && (self.mode != ViewMode::Tree
                || key.tree_collapsed == self.tree_collapsed
                    && key.tree_toggled == self.tree_toggled)
            && key.collapsed_users == self.collapsed_users
            && self.expand_to.is_none()
            && self.by_user_setting_unchanged(key)
    }

    /// Grouping by user depends on the data in `Auto`; the revision covers
    /// that, so only the setting itself is compared here.
    fn by_user_setting_unchanged(&self, key: &RowKey) -> bool {
        match self.user_grouping {
            UserGrouping::Auto => true,
            UserGrouping::Always => key.by_user || self.mode != ViewMode::List,
            UserGrouping::Never => !key.by_user,
        }
    }

    fn same_order_settings(&self, key: &RowKey) -> bool {
        key.search == self.search
            && key.hide_system == self.hide_system
            && key.system == self.system
            && self.by_user_setting_unchanged(key)
            && key.mode == self.mode
            && key.sort == self.sort
            && key.descending == self.descending
            && key.descriptive_names == self.descriptive_names
            && key.filter == self.filter
            && key.more == self.more
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
        if let Some(target) = self.expand_to.take() {
            self.open_path_to(procs, extra, target);
        }
        let filter = self.search.trim().to_lowercase();
        let selected = self.selected.as_ref().map(|s| (s.pid, s.create_time));
        // The selected process stays listed, so a reveal always shows it.
        let shown_system = |p: &ProcessInfo| {
            !self.hide_system
                || selected == Some((p.pid, p.create_time))
                || !self.system.is_system(p, extra.image_path(p))
        };
        let matches = |p: &ProcessInfo| {
            let resource_matches = match self.filter {
                QuickFilter::All => true,
                QuickFilter::Cpu => p.pid != 0 && p.cpu_percent >= 5.0,
                QuickFilter::Memory => p.private_working_set >= 512 * 1024 * 1024,
                QuickFilter::Managed => !extra.label(p).is_empty(),
                QuickFilter::NotResponding => extra.hung.contains_key(&p.pid),
            };
            resource_matches
                && shown_system(p)
                && self.more.matches(p, now)
                && (filter.is_empty()
                    || p.name.to_lowercase().contains(&filter)
                    || extra
                        .description(p)
                        .is_some_and(|name| name.to_lowercase().contains(&filter))
                    || p.pid.to_string().contains(&filter)
                    || extra.command_line(p).to_lowercase().contains(&filter)
                    || extra
                        .hosted(p)
                        .iter()
                        .any(|s| s.to_lowercase().contains(&filter))
                    || extra
                        .image_paths
                        .get(&(p.pid, p.create_time))
                        .is_some_and(|p| p.to_lowercase().contains(&filter)))
        };
        let stable = self
            .last_order
            .as_ref()
            .filter(|s| self.inspection_held && self.same_order_settings(&s.key));
        let live_order = |a: &ProcessInfo, b: &ProcessInfo| {
            let o = compare(a, b, self.sort, extra, self.descriptive_names);
            if self.descending { o.reverse() } else { o }
        };
        let order = |a: &ProcessInfo, b: &ProcessInfo| {
            let rank = |p: &ProcessInfo| {
                stable
                    .and_then(|s| s.processes.get(&(p.pid, p.create_time)))
                    .copied()
                    .unwrap_or(usize::MAX)
            };
            rank(a).cmp(&rank(b)).then_with(|| live_order(a, b))
        };
        let unfiltered =
            filter.is_empty() && self.filter == QuickFilter::All && self.more.is_empty();
        let matching = || (0..procs.len()).filter(|&i| matches(&procs[i]));
        let mut groups = Vec::new();
        let mut parents = HashSet::new();
        let by_user = self.mode == ViewMode::List
            && match self.user_grouping {
                UserGrouping::Always => true,
                UserGrouping::Never => false,
                UserGrouping::Auto => signed_in_users(procs, extra) > 1,
            };
        let (rows, shown) = match self.mode {
            ViewMode::Tree if unfiltered => {
                let members: Vec<usize> = matching().collect();
                let toggled = |i: usize| {
                    self.tree_toggled
                        .contains(&(procs[i].pid, procs[i].create_time))
                };
                let (order, with_children) =
                    tree_order_by(procs, &members, stable.is_some(), order, |i| {
                        self.tree_collapsed != toggled(i)
                    });
                parents = with_children;
                let rows = order
                    .into_iter()
                    .map(|(i, depth)| Row::Process(i, depth))
                    .collect();
                (rows, members.len())
            }
            ViewMode::List if by_user => {
                let members: Vec<usize> = matching().collect();
                groups = user_groups(procs, &members, extra);
                groups.sort_by(|a, b| {
                    let rank = |g: &AppGroup| {
                        stable
                            .and_then(|s| s.apps.get(&g.key))
                            .copied()
                            .unwrap_or(usize::MAX)
                    };
                    rank(a).cmp(&rank(b)).then_with(|| a.key.cmp(&b.key))
                });
                let mut rows = Vec::with_capacity(members.len() + groups.len());
                for (i, g) in groups.iter().enumerate() {
                    rows.push(Row::User(i));
                    if !self.collapsed_users.contains(&g.key) {
                        let mut members = g.members.clone();
                        members.sort_by(|&a, &b| order(&procs[a], &procs[b]));
                        rows.extend(members.into_iter().map(|i| Row::Process(i, 1)));
                    }
                }
                (rows, members.len())
            }
            ViewMode::Apps => {
                let members: Vec<usize> = matching().collect();
                groups = app_groups(procs, &members);
                groups.sort_by(|a, b| {
                    let rank = |g: &AppGroup| {
                        stable
                            .and_then(|s| s.apps.get(&g.key))
                            .copied()
                            .unwrap_or(usize::MAX)
                    };
                    rank(a)
                        .cmp(&rank(b))
                        .then_with(|| live_order(&a.total, &b.total))
                });
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
        let cache = RowCache {
            key: RowKey {
                inspection_held: self.inspection_held,
                revision: revision.unwrap_or_default(),
                snapshot: std::ptr::from_ref(snap) as usize,
                processes: procs.len(),
                search: self.search.clone(),
                mode: self.mode,
                sort: self.sort,
                descending: self.descending,
                descriptive_names: self.descriptive_names,
                filter: self.filter,
                more: self.more.clone(),
                expanded: if self.mode == ViewMode::Apps {
                    self.expanded.clone()
                } else {
                    HashSet::new()
                },
                hide_system: self.hide_system,
                system: self.system.clone(),
                tree_collapsed: self.tree_collapsed,
                tree_toggled: self.tree_toggled.clone(),
                by_user,
                collapsed_users: self.collapsed_users.clone(),
                second,
            },
            rows,
            groups,
            parents,
            shown,
        };
        let mut processes = HashMap::new();
        let mut apps = HashMap::new();
        for (rank, row) in cache.rows.iter().enumerate() {
            match *row {
                Row::Process(i, _) => {
                    processes.insert((procs[i].pid, procs[i].create_time), rank);
                }
                Row::App(i) | Row::User(i) => {
                    apps.insert(cache.groups[i].key.clone(), rank);
                }
            }
        }
        self.last_order = Some(StableOrder {
            key: cache.key.clone(),
            processes,
            apps,
        });
        cache
    }
}

impl ProcessView {
    /// Opens every collapsed tree ancestor and the user group of `target`.
    fn open_path_to(&mut self, procs: &[ProcessInfo], extra: &Extra<'_>, target: (u32, i64)) {
        let Some(start) = procs.iter().position(|p| (p.pid, p.create_time) == target) else {
            return;
        };
        if let Some(user) = extra.user(&procs[start]) {
            self.collapsed_users.remove(&user.to_lowercase());
        }
        let index: HashMap<u32, usize> =
            procs.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
        let mut current = start;
        // Bounded: a PID cycle cannot loop forever.
        for _ in 0..procs.len() {
            let Some(&parent) = index.get(&procs[current].parent_pid) else {
                break;
            };
            if parent == current || procs[parent].create_time > procs[current].create_time {
                break;
            }
            let key = (procs[parent].pid, procs[parent].create_time);
            // Open: the toggled state must differ from "collapsed" by default.
            if self.tree_collapsed {
                self.tree_toggled.insert(key);
            } else {
                self.tree_toggled.remove(&key);
            }
            current = parent;
        }
    }

    /// Opens or closes one tree node.
    fn toggle_node(&mut self, key: (u32, i64)) {
        if !self.tree_toggled.remove(&key) {
            self.tree_toggled.insert(key);
        }
    }

    pub fn expand_all(&mut self) {
        self.tree_collapsed = false;
        self.tree_toggled.clear();
        self.collapsed_users.clear();
    }

    pub fn collapse_all(&mut self) {
        self.tree_collapsed = true;
        self.tree_toggled.clear();
    }
}

/// Accounts that run programs in a signed-in session (not session 0).
fn signed_in_users(procs: &[ProcessInfo], extra: &Extra<'_>) -> usize {
    procs
        .iter()
        .filter(|p| p.session_id != 0)
        .filter_map(|p| extra.user(p))
        .filter(|u| !is_service_account(u))
        .map(str::to_lowercase)
        .collect::<HashSet<_>>()
        .len()
}

/// Built-in accounts Windows runs its own programs as.
fn is_service_account(account: &str) -> bool {
    let name = short_user(account);
    ["SYSTEM", "LOCAL SERVICE", "NETWORK SERVICE"]
        .iter()
        .any(|n| n.eq_ignore_ascii_case(name))
        || name.starts_with("DWM-")
        || name.starts_with("UMFD-")
}

/// Groups the processes at `members` by account; unknown accounts share one
/// group with an empty key.
fn user_groups(procs: &[ProcessInfo], members: &[usize], extra: &Extra<'_>) -> Vec<AppGroup> {
    let mut by_user: HashMap<String, Vec<usize>> = HashMap::new();
    for &i in members {
        let key = extra
            .user(&procs[i])
            .map(str::to_lowercase)
            .unwrap_or_default();
        by_user.entry(key).or_default().push(i);
    }
    by_user
        .into_iter()
        .map(|(key, members)| {
            let mut group = app_groups(procs, &members)
                .into_iter()
                .reduce(|mut a, b| {
                    let t = &mut a.total;
                    let o = &b.total;
                    t.cpu_percent += o.cpu_percent;
                    t.cpu_time += o.cpu_time;
                    t.working_set += o.working_set;
                    t.private_working_set += o.private_working_set;
                    t.private_bytes += o.private_bytes;
                    t.thread_count += o.thread_count;
                    t.handle_count += o.handle_count;
                    t.gpu_percent += o.gpu_percent;
                    t.gpu_memory += o.gpu_memory;
                    t.io_read_rate += o.io_read_rate;
                    t.io_write_rate += o.io_write_rate;
                    t.net_rx_rate += o.net_rx_rate;
                    t.net_tx_rate += o.net_tx_rate;
                    t.memory.add(&o.memory);
                    t.gpu_detail.shared_memory += o.gpu_detail.shared_memory;
                    a.members.extend(b.members);
                    a
                })
                .expect("every group has a member");
            group.total.gpu_detail.engine = busiest_engine(procs, &group.members);
            group.key = key;
            group
        })
        .collect()
}

/// The GPU engine of the member that uses the GPU most, for a group row.
fn busiest_engine(procs: &[ProcessInfo], members: &[usize]) -> Option<wtm_core::GpuEngineUse> {
    members
        .iter()
        .map(|&i| &procs[i])
        .filter(|p| p.gpu_detail.engine.is_some())
        .max_by(|a, b| a.gpu_percent.total_cmp(&b.gpu_percent))
        .and_then(|p| p.gpu_detail.engine.clone())
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
                total.net_rx_rate += p.net_rx_rate;
                total.net_tx_rate += p.net_tx_rate;
                total.memory.add(&p.memory);
                total.gpu_detail.shared_memory += p.gpu_detail.shared_memory;
            }
            // The total starts as the first member: its engine is not the
            // group's.
            total.gpu_detail.engine = busiest_engine(procs, &members);
            AppGroup {
                key,
                total,
                members,
            }
        })
        .collect()
}

/// Actions for the processes of one user account.
fn user_menu(
    ui: &mut Ui,
    g: &AppGroup,
    procs: &[ProcessInfo],
    extra: &Extra<'_>,
    intents: &mut Vec<Intent>,
) {
    let account = g.members.first().and_then(|&i| extra.user(&procs[i]));
    if let Some(account) = account {
        crate::context_actions::copy(ui, crate::i18n::tr("User name"), || account.to_owned());
        crate::theme::divider(ui);
    }
    if crate::context_actions::action(ui, crate::i18n::tr("Manage user accounts")) {
        intents.push(Intent::ManageAccounts);
    }
    // Only a person's own sign-in session can be signed out.
    let session = g
        .members
        .iter()
        .map(|&i| procs[i].session_id)
        .find(|&s| s != 0);
    if let (Some(session), Some(account)) = (session, account)
        && !is_service_account(account)
        && crate::context_actions::action(ui, crate::i18n::tr("Sign out…"))
    {
        intents.push(Intent::SignOut(session, short_user(account).to_owned()));
    }
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
            if command == Command::Suspend {
                // Freezing every window of an app is confirmed first.
                intents.push(Intent::ConfirmMany(all.clone(), command));
            } else {
                intents.extend(all.iter().map(|t| Intent::Command(t.clone(), command)));
            }
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
    crate::theme::divider(ui);
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
            .color(crate::theme::text_color(ui, crate::theme::BAD)),
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
    hosted: &[String],
    intents: &mut Vec<Intent>,
) {
    if t.name.eq_ignore_ascii_case("explorer.exe")
        && ui
            .button(crate::i18n::tr("Restart Windows Explorer"))
            .on_hover_text(crate::i18n::tr(
                "Closes the desktop and taskbar and starts them again with the same account and rights.",
            ))
            .clicked()
    {
        intents.push(Intent::RestartExplorer);
        ui.close();
    }
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
    crate::theme::divider(ui);
    cmd(ui, crate::i18n::tr("Suspend"), Command::Suspend, true);
    cmd(ui, crate::i18n::tr("Resume"), Command::Resume, false);
    crate::theme::divider(ui);
    ui.menu_button(crate::i18n::tr("Priority"), |ui| {
        for p in [
            PriorityClass::Idle,
            PriorityClass::BelowNormal,
            PriorityClass::Normal,
            PriorityClass::AboveNormal,
            PriorityClass::High,
        ] {
            cmd(ui, priority_label(p), Command::Priority(p), false);
        }
        // As in Task Manager, realtime is offered but asked about first.
        cmd(
            ui,
            priority_label(PriorityClass::Realtime),
            Command::Priority(PriorityClass::Realtime),
            true,
        );
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
    ui.menu_button(crate::i18n::tr("Limits"), |ui| {
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
    });
    cmd(
        ui,
        crate::i18n::tr("Free memory"),
        Command::TrimMemory,
        false,
    );
    crate::theme::divider(ui);
    if ui.button(crate::i18n::tr("Open file location")).clicked() {
        intents.push(Intent::OpenLocation(t.clone()));
        ui.close();
    }
    if !hosted.is_empty()
        && ui
            .button(crate::i18n::tr("Go to services"))
            .on_hover_text(hosted.join(", "))
            .clicked()
    {
        intents.push(Intent::ShowServices(t.pid));
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
    crate::theme::divider(ui);
    ui.menu_button(crate::i18n::tr("Diagnostics"), |ui| {
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
        if ui
            .button(crate::i18n::tr("Ask AI about it"))
            .on_hover_text(crate::i18n::tr(
                "What it is, whether it is safe and whether to end it, from your AI provider",
            ))
            .clicked()
        {
            intents.push(Intent::AskAi(t.clone()));
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
    });
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
            suspended: false,
            create_time,
            memory: Default::default(),
            gpu_detail: Default::default(),
        }
    }

    #[test]
    fn an_app_sums_what_its_processes_use() {
        let mut procs = vec![p(10, 4, 1.0, 10), p(11, 4, 2.0, 11), p(12, 4, 4.0, 12)];
        for (i, q) in procs.iter_mut().enumerate() {
            q.name = if i < 2 { "Chrome.exe" } else { "other.exe" }.into();
            q.net_rx_rate = 100.0 * (i + 1) as f64;
            q.net_tx_rate = 10.0 * (i + 1) as f64;
            q.io_read_rate = 1.0;
        }
        let groups = app_groups(&procs, &[0, 1, 2]);
        let chrome = groups.iter().find(|g| g.key == "chrome.exe").unwrap();
        assert_eq!(chrome.members, [0, 1]);
        assert_eq!(chrome.total.pid, 10, "the oldest member's");
        assert_eq!(chrome.total.cpu_percent, 3.0);
        assert_eq!(chrome.total.io_read_rate, 2.0);
        assert_eq!(chrome.total.net_rx_rate, 300.0);
        assert_eq!(chrome.total.net_tx_rate, 30.0);
    }

    fn no_extra() -> Extra<'static> {
        Extra {
            topology: None,
            labels: Box::leak(Box::default()),
            command_lines: Box::leak(Box::default()),
            image_paths: Box::leak(Box::default()),
            descriptions: Box::leak(Box::default()),
            icons: Box::leak(Box::default()),
            hung: Box::leak(Box::default()),
            facts: Box::leak(Box::default()),
            versions: Box::leak(Box::default()),
            services: Box::leak(Box::default()),
            gui_objects: Box::leak(Box::default()),
            gpu_order: &[],
        }
    }

    fn with_facts(facts: &'static crate::backend::FactsMap) -> Extra<'static> {
        Extra {
            facts,
            ..no_extra()
        }
    }

    fn account(user: &str) -> wtm_core::control::ProcessFacts {
        wtm_core::control::ProcessFacts {
            user: Some(user.into()),
            ..Default::default()
        }
    }

    #[test]
    fn app_rows_show_the_engine_of_their_busiest_member() {
        let engine = |kind: &str| {
            Some(wtm_core::GpuEngineUse {
                luid: "00000000_00000001".into(),
                kind: kind.into(),
            })
        };
        let mut first = p(1, 0, 0.0, 1);
        first.name = "game.exe".into();
        first.gpu_percent = 2.0;
        first.gpu_detail.engine = engine("Copy");
        first.gpu_detail.shared_memory = 100;
        let mut second = p(2, 0, 0.0, 2);
        second.name = "game.exe".into();
        second.gpu_percent = 60.0;
        second.gpu_detail.engine = engine("3D");
        second.gpu_detail.shared_memory = 50;
        let idle = {
            let mut q = p(3, 0, 0.0, 3);
            q.name = "game.exe".into();
            q
        };
        let procs = [first, second, idle];
        let groups = app_groups(&procs, &[0, 1, 2]);
        assert_eq!(groups.len(), 1);
        let total = &groups[0].total;
        assert_eq!(
            total.gpu_detail.engine,
            engine("3D"),
            "not the first member's"
        );
        assert_eq!(total.gpu_detail.shared_memory, 150);
        // No member busy: no engine.
        assert_eq!(app_groups(&procs, &[2])[0].total.gpu_detail.engine, None);
    }

    #[test]
    fn new_identity_columns_sort_by_trust_start_and_parent() {
        use wtm_core::control::{Integrity, ProcessFacts};
        let mut low = p(10, 7, 0.0, 300);
        low.create_time = 300;
        let mut system = p(11, 4, 0.0, 100);
        system.create_time = 100;
        let unknown = p(12, 9, 0.0, 200);
        let facts: &'static crate::backend::FactsMap = Box::leak(Box::new(
            [
                (
                    (10, low.create_time),
                    ProcessFacts {
                        integrity: Some(Integrity::Low),
                        ..Default::default()
                    },
                ),
                (
                    (11, system.create_time),
                    ProcessFacts {
                        integrity: Some(Integrity::System),
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        ));
        let extra = with_facts(facts);
        let order = |col| {
            let mut v = [&low, &system, &unknown];
            v.sort_by(|a, b| compare(a, b, col, &extra, false));
            v.map(|q| q.pid)
        };
        assert_eq!(
            order(Col::Integrity),
            [12, 10, 11],
            "unknown, then least trusted"
        );
        assert_eq!(order(Col::StartTime), [11, 12, 10], "earliest first");
        assert_eq!(
            cpu_time_text(std::time::Duration::from_secs(3723)),
            "1:02:03"
        );
        assert_eq!(
            cpu_time_text(std::time::Duration::from_secs(360_000)),
            "100:00:00"
        );
        assert_eq!(order(Col::ParentPid), [11, 10, 12]);
        assert_eq!(integrity_label(Integrity::MediumPlus), "Medium plus");
        assert_eq!(integrity_label(Integrity::Other(0x2500)), "0x2500");
        assert!(start_clock(&low).is_empty() || start_clock(&low).len() == 19);
    }

    #[test]
    fn a_collapsed_tree_node_hides_its_descendants_without_making_them_roots() {
        let procs = vec![
            p(10, 0, 0.0, 1),
            p(11, 10, 0.0, 2),
            p(12, 11, 0.0, 3),
            p(13, 0, 0.0, 4),
        ];
        let all: Vec<usize> = (0..procs.len()).collect();
        let by_pid = |a: &ProcessInfo, b: &ProcessInfo| a.pid.cmp(&b.pid);
        let (open, parents) = tree_order_by(&procs, &all, false, by_pid, |_| false);
        assert_eq!(open, [(0, 0), (1, 1), (2, 2), (3, 0)]);
        assert_eq!(parents, HashSet::from([0, 1]));
        let (closed, _) = tree_order_by(&procs, &all, false, by_pid, |i| i == 0);
        assert_eq!(closed, [(0, 0), (3, 0)]);
        // Without its parent among the members, a child becomes a root.
        let (subset, _) = tree_order_by(&procs, &[1, 2, 3], false, by_pid, |_| false);
        assert_eq!(subset, [(1, 0), (2, 1), (3, 0)]);
    }

    #[test]
    fn expand_and_collapse_all_reset_individual_nodes() {
        let mut view = ProcessView::default();
        view.toggle_node((1, 1));
        view.collapse_all();
        assert!(view.tree_collapsed && view.tree_toggled.is_empty());
        view.toggle_node((1, 1));
        view.expand_all();
        assert!(!view.tree_collapsed && view.tree_toggled.is_empty());
    }

    #[test]
    fn system_processes_are_windows_components_and_the_lists_override() {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let filter = SystemFilter::default();
        let proc = |name: &str, session: u32| {
            let mut q = p(500, 4, 0.0, 1);
            q.name = name.into();
            q.session_id = session;
            q
        };
        let svchost = proc("svchost.exe", 0);
        assert!(filter.is_system(&svchost, Some(&format!(r"{root}\System32\svchost.exe"))));
        // A service whose image cannot be read is a Windows one too.
        assert!(filter.is_system(&svchost, None));
        let helper = proc("sihost.exe", 1);
        assert!(filter.is_system(&helper, Some(&format!(r"{root}\System32\sihost.exe"))));
        assert!(filter.is_system(&proc("csrss.exe", 1), None));
        let explorer = proc("explorer.exe", 1);
        assert!(!filter.is_system(&explorer, Some(&format!(r"{root}\explorer.exe"))));
        let app = proc("code.exe", 1);
        let path = r"C:\Program Files\Code\code.exe";
        assert!(!filter.is_system(&app, Some(path)));
        // A third-party service in session 0 stays visible.
        assert!(!filter.is_system(&proc("vendor.exe", 0), Some(path)));
        let lists = SystemFilter {
            always_show: vec!["SIHOST.EXE".into()],
            also_hide: vec![" code.exe ".into()],
        };
        assert!(!lists.is_system(&helper, Some(&format!(r"{root}\System32\sihost.exe"))));
        assert!(lists.is_system(&app, Some(path)));
    }

    #[test]
    fn processes_group_by_user_only_when_asked_or_several_people_run_programs() {
        let mut procs = vec![
            p(10, 0, 1.0, 1),
            p(11, 0, 2.0, 2),
            p(12, 0, 3.0, 3),
            p(13, 0, 0.0, 4),
        ];
        for q in &mut procs {
            q.session_id = 1;
        }
        procs[3].session_id = 0;
        let facts: &'static crate::backend::FactsMap = Box::leak(Box::new(
            [
                ((10, 1), account(r"PC\alice")),
                ((11, 2), account(r"PC\alice")),
                ((12, 3), account(r"PC\bob")),
                ((13, 4), account(r"NT AUTHORITY\SYSTEM")),
            ]
            .into_iter()
            .collect(),
        ));
        let extra = with_facts(facts);
        let snap = Snapshot {
            interval: None,
            system: Default::default(),
            processes: procs,
            net_traced: false,
            taken_at_ms: 0,
        };
        let users = |view: &mut ProcessView| {
            let cache = view.rows(&snap, &extra, None);
            cache
                .rows
                .iter()
                .filter(|r| matches!(r, Row::User(_)))
                .count()
        };
        let mut view = ProcessView::default();
        // Alice and Bob both run programs; SYSTEM does not count as a person.
        assert_eq!(users(&mut view), 3);
        view.user_grouping = UserGrouping::Never;
        assert_eq!(users(&mut view), 0);
        view.user_grouping = UserGrouping::Always;
        view.mode = ViewMode::Tree;
        assert_eq!(users(&mut view), 0, "only the list is grouped");
        assert_eq!(short_user(r"PC\alice"), "alice");
        assert!(is_service_account(r"Font Driver Host\UMFD-0"));
    }

    #[test]
    fn hiding_system_processes_keeps_the_selected_one() {
        let mut procs = vec![p(10, 0, 0.0, 1), p(11, 0, 0.0, 2)];
        procs[0].name = "csrss.exe".into();
        procs[1].name = "app.exe".into();
        procs[1].session_id = 1;
        let snap = Snapshot {
            interval: None,
            system: Default::default(),
            processes: procs,
            net_traced: false,
            taken_at_ms: 0,
        };
        let extra = no_extra();
        let mut view = ProcessView {
            hide_system: true,
            ..Default::default()
        };
        assert_eq!(view.rows(&snap, &extra, None).shown, 1);
        view.selected = Some(ProcessRef::from(&snap.processes[0]));
        assert_eq!(view.rows(&snap, &extra, None).shown, 2);
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
                    Row::App(g) | Row::User(g) => cache.groups[g].total.pid,
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

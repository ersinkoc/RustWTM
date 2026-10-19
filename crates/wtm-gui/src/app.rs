use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use egui_plot::{Line, Plot, PlotPoints};
use wtm_core::control::ProcessRef;
use wtm_core::{Snapshot, format_bytes};
use wtm_ipc::{Command, EventLevel};

use crate::backend::{Backend, Data, HISTORY_LEN, Mode, Panel, ServiceOp, UiCommand, describe};
use crate::processes::{Intent, ManagedMap, ProcessView, context_menu, limit_labels};
use crate::tray::Tray;
use crate::views;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Processes,
    Performance,
    Governor,
    Events,
    History,
    Startup,
    Users,
    Services,
    Network,
    Limits,
    Settings,
    About,
}

/// Navigation: tab, icon, label. Separators split monitoring, protection,
/// system and the app itself.
const NAV: [(Tab, &str, &str); 13] = [
    (Tab::Overview, "🏠", "Overview"),
    (Tab::Processes, "📋", "Processes"),
    (Tab::Performance, "📈", "Performance"),
    (Tab::History, "🕘", "App history"),
    (Tab::Governor, "🛡", "Governor"),
    (Tab::Limits, "⏱", "Limits"),
    (Tab::Events, "🔔", "Events"),
    (Tab::Settings, "⚙", "Settings"),
    (Tab::Startup, "🚀", "Startup apps"),
    (Tab::Services, "🔧", "Services"),
    (Tab::Users, "👤", "Users"),
    (Tab::Network, "🌐", "Network"),
    (Tab::About, "ℹ", "About"),
];
const NAV_GROUP_STARTS: [usize; 3] = [4, 8, 12];
/// Below this window width the navigation shows icons only by default.
const NAV_AUTO_COLLAPSE: f32 = 1000.0;

fn nav(ui: &mut Ui, state: &mut UiState, collapsed: bool) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let logo = ui.ctx().load_texture(
            "nav-logo",
            egui::ColorImage::from_rgba_unmultiplied([32, 32], &crate::logo::rgba(32)),
            egui::TextureOptions::LINEAR,
        );
        ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(28.0, 28.0)));
        if !collapsed {
            ui.label(RichText::new("RustWTM").size(18.0).strong());
        }
    });
    ui.add_space(6.0);
    let (arrow, hint) = if collapsed {
        ("»", "Show labels")
    } else {
        ("«", "Icons only")
    };
    if ui.small_button(arrow).on_hover_text(hint).clicked() {
        state.nav_collapsed = Some(!collapsed);
    }
    ui.add_space(6.0);
    for (i, (tab, icon, label)) in NAV.iter().enumerate() {
        if NAV_GROUP_STARTS.contains(&i) {
            ui.separator();
        }
        let selected = state.tab == *tab;
        let (text, width) = if collapsed {
            (RichText::new(*icon).size(18.0), 40.0)
        } else {
            (RichText::new(format!("{icon}  {label}")).size(15.0), 160.0)
        };
        let response =
            ui.add(egui::Button::selectable(selected, text).min_size(egui::vec2(width, 28.0)));
        let response = if collapsed {
            response.on_hover_text(*label)
        } else {
            response
        };
        if response.clicked() {
            state.tab = *tab;
        }
    }
}
pub struct App {
    backend: Backend,
    state: UiState,
    tray: Option<Tray>,
}

struct UiState {
    tab: Tab,
    processes: ProcessView,
    confirm: Option<(ProcessRef, Command)>,
    /// A destructive command for every process of an app.
    confirm_many: Option<(Vec<ProcessRef>, Command)>,
    network_filter: String,
    services_filter: String,
    selected_startup: Option<wtm_core::startup::StartupEntry>,
    history: views::HistoryView,
    history_dirty: bool,
    selected_service: Option<String>,
    config: crate::settings::ConfigView,
    limits: crate::limits_tab::LimitsView,
    /// `None`: collapse automatically on narrow windows.
    nav_collapsed: Option<bool>,
    last_load: HashMap<Panel, Instant>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Effective only when started elevated; harmless otherwise.
        wtm_core::control::enable_debug_privilege();
        crate::backend::register_app_id();
        let backend = Backend::start(cc.egui_ctx.clone());
        let tray = Tray::new(&cc.egui_ctx, backend.notifications.clone());
        Self {
            backend,
            tray,
            state: UiState::default(),
        }
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            tab: Tab::Overview,
            processes: ProcessView::default(),
            confirm: None,
            confirm_many: None,
            network_filter: String::new(),
            services_filter: String::new(),
            selected_startup: None,
            history: views::HistoryView::default(),
            history_dirty: true,
            selected_service: None,
            config: crate::settings::ConfigView::default(),
            limits: crate::limits_tab::LimitsView::default(),
            nav_collapsed: None,
            last_load: HashMap::new(),
        }
    }
}

const WARN: Color32 = Color32::from_rgb(230, 140, 40);
const BAD: Color32 = Color32::from_rgb(220, 70, 70);
const GOOD: Color32 = Color32::from_rgb(80, 180, 100);

fn level_color(level: EventLevel) -> Color32 {
    match level {
        EventLevel::Info => Color32::GRAY,
        EventLevel::Warn => WARN,
        EventLevel::Error => BAD,
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let App {
            backend,
            state,
            tray,
        } = self;
        let data = backend.data();
        if let (Some(tray), Some(s)) = (tray.as_mut(), &data.snapshot) {
            tray.set_tooltip(format!("RustWTM - CPU {:.0}%", s.system.cpu_percent));
        }
        let mut intents = Vec::new();
        let mut service_cmds: Vec<UiCommand> = Vec::new();

        // Narrow windows get the icon-only rail unless the user chose.
        let collapsed = state
            .nav_collapsed
            .unwrap_or(ui.ctx().content_rect().width() < NAV_AUTO_COLLAPSE);
        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(if collapsed { 52.0 } else { 176.0 })
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("nav_scroll")
                    .show(ui, |ui| nav(ui, state, collapsed));
            });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                match &data.message {
                    Some((level, text)) => {
                        ui.label(RichText::new(text).color(level_color(*level)));
                    }
                    None => {
                        ui.weak("Right-click a process for actions.");
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    mode_badge(ui, data.mode);
                    if let Some(s) = &data.snapshot {
                        ui.label(format!(
                            "CPU {:.0}%   Memory {} / {}",
                            s.system.cpu_percent,
                            format_bytes(s.system.memory_used() as f64),
                            format_bytes(s.system.memory_total as f64)
                        ));
                    }
                });
            });
        });
        let managed: ManagedMap = limit_labels(data.status.as_ref());

        if state.tab == Tab::Processes
            && let (Some(sel), Some(snap)) = (state.processes.selected.clone(), &data.snapshot)
        {
            egui::Panel::right("details")
                .resizable(true)
                .default_size(330.0)
                .show(ui, |ui| details_panel(ui, &data, snap, &sel, &mut intents));
        }

        egui::CentralPanel::default().show(ui, |ui| match state.tab {
            Tab::Processes => match &data.snapshot {
                Some(snap) => {
                    let extra = crate::processes::Extra {
                        labels: &managed,
                        command_lines: &data.command_lines,
                        image_paths: &data.image_paths,
                        icons: &data.icons,
                    };
                    intents.extend(state.processes.ui(ui, snap, &extra));
                }
                None => {
                    ui.spinner();
                }
            },
            Tab::Performance => performance(ui, &data),
            Tab::Governor => governor(ui, &data),
            Tab::Events => events(ui, &data),
            Tab::History => {
                if views::app_history(ui, &data, &mut state.history) {
                    state.history_dirty = true;
                }
                if let Some((app, cores)) = state.history.limit_request.take() {
                    state.limits.prefill_cores(&app, cores);
                    state.tab = Tab::Limits;
                }
            }
            Tab::Startup => {
                if let Some(cmd) = views::startup(ui, &data, &mut state.selected_startup) {
                    service_cmds.push(cmd);
                }
            }
            Tab::Users => views::users(ui, &data),
            Tab::Overview => {
                let (cmds, more) = crate::overview::overview(ui, &data);
                service_cmds.extend(cmds);
                intents.extend(more);
            }
            Tab::About => crate::about::about(ui),
            Tab::Limits => {
                service_cmds.extend(crate::limits_tab::limits_tab(ui, &data, &mut state.limits));
            }
            Tab::Settings => {
                service_cmds.extend(crate::settings::settings(ui, &data, &mut state.config));
            }
            Tab::Services => {
                if let Some(op) = service_bar(ui, &data, state.selected_service.as_deref()) {
                    service_cmds.push(op);
                }
                if let Some(s) = views::services(ui, &data, &mut state.services_filter) {
                    state.selected_service = Some(s.name);
                }
            }
            Tab::Network => {
                let only = state.processes.selected.as_ref().map(|r| r.pid);
                views::network(ui, &data, &mut state.network_filter, only);
            }
        });

        if let Some((target, command)) = state.confirm.clone() {
            let modal = egui::Modal::new(egui::Id::unique("confirm")).show(&ctx, |ui| {
                ui.heading(describe(command));
                ui.label(format!(
                    "{} \"{}\" (PID {})? Unsaved data in that program will be lost.",
                    describe(command),
                    target.name,
                    target.pid
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(RichText::new(describe(command)).color(BAD))
                        .clicked()
                    {
                        intents.push(Intent::Command(target.clone(), command));
                        state.confirm = None;
                    }
                    if ui.button("Cancel").clicked() {
                        state.confirm = None;
                    }
                });
            });
            if modal.should_close() {
                state.confirm = None;
            }
        }

        if let Some((targets, command)) = state.confirm_many.clone() {
            let name = targets.first().map_or("", |t| t.name.as_str()).to_owned();
            let modal = egui::Modal::new(egui::Id::unique("confirm_many")).show(&ctx, |ui| {
                ui.heading(format!("{} — {name}", describe(command)));
                ui.label(format!(
                    "{} all {} processes of \"{name}\"? Unsaved data in that program will be lost.",
                    describe(command),
                    targets.len()
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(RichText::new(format!("{} all", describe(command))).color(BAD))
                        .clicked()
                    {
                        intents.extend(targets.iter().map(|t| Intent::Command(t.clone(), command)));
                        state.confirm_many = None;
                    }
                    if ui.button("Cancel").clicked() {
                        state.confirm_many = None;
                    }
                });
            });
            if modal.should_close() {
                state.confirm_many = None;
            }
        }

        if state.tab == Tab::Processes
            && state.confirm.is_none()
            && let Some(sel) = &state.processes.selected
            && ctx.input(|i| i.key_pressed(egui::Key::Delete))
        {
            intents.push(Intent::Confirm(sel.clone(), Command::Kill));
        }

        drop(data);
        if state.tab == Tab::History {
            let stale = state
                .last_load
                .get(&Panel::History)
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(60));
            if stale || state.history_dirty {
                state.history_dirty = false;
                state.last_load.insert(Panel::History, Instant::now());
                let hours = state.history.hours;
                backend.send(UiCommand::LoadHistory { hours });
                if let Some(app) = state.history.selected.clone() {
                    backend.send(UiCommand::LoadSeries { app, hours });
                }
            }
        }
        for cmd in service_cmds {
            backend.send(cmd);
        }
        let due = match state.tab {
            Tab::Users => Some((Panel::Users, Duration::from_secs(5))),
            Tab::Network => Some((Panel::Network, Duration::from_secs(2))),
            Tab::Services => Some((Panel::Services, Duration::from_secs(5))),
            Tab::History => None,
            Tab::Startup => Some((Panel::Startup, Duration::from_secs(10))),
            Tab::Settings | Tab::Limits | Tab::Overview => {
                Some((Panel::Config, Duration::from_secs(30)))
            }
            _ => None,
        };
        if let Some((panel, every)) = due
            && state
                .last_load
                .get(&panel)
                .is_none_or(|t| t.elapsed() >= every)
        {
            state.last_load.insert(panel, Instant::now());
            backend.send(UiCommand::Load(panel));
        }
        for intent in intents {
            match intent {
                Intent::Select(t) => {
                    backend.send(UiCommand::Inspect(t.clone()));
                    state.processes.selected = Some(t);
                }
                Intent::Command(target, command) => {
                    backend.send(UiCommand::Control { target, command });
                }
                Intent::Confirm(target, command) => state.confirm = Some((target, command)),
                Intent::ConfirmMany(targets, command) => {
                    state.confirm_many = Some((targets, command));
                }
                Intent::OpenLocation(target) => backend.send(UiCommand::OpenLocation(target)),
                Intent::Reveal(target) => {
                    backend.send(UiCommand::Inspect(target.clone()));
                    state.processes.selected = Some(target);
                    state.tab = Tab::Processes;
                }
                Intent::LimitApp(name) => {
                    state.limits.prefill(&name);
                    state.tab = Tab::Limits;
                }
            }
        }
    }
}

fn mode_badge(ui: &mut Ui, mode: Mode) {
    let (text, color, hover) = match mode {
        Mode::Starting => ("○ Starting", Color32::GRAY, "Collecting data…"),
        Mode::Service {
            verified: false, ..
        } => (
            "● Console instance",
            WARN,
            "Connected to a RustWTM engine that is not the installed service (e.g. `wtm-service console`).",
        ),
        Mode::Service { admin: true, .. } => (
            "● Service",
            GOOD,
            "Connected to the RustWTM service with full control.",
        ),
        Mode::Service { admin: false, .. } => (
            "● Service (read-only)",
            WARN,
            "Connected to the service. Run RustWTM as administrator to send commands through it; until then actions use your own rights.",
        ),
        Mode::Local => (
            "○ Local",
            WARN,
            "The RustWTM service is not running. Monitoring and actions use this window's own rights.",
        ),
    };
    ui.label(RichText::new(text).color(color))
        .on_hover_text(hover);
}

fn uptime(start: Option<SystemTime>) -> String {
    let Some(secs) = start
        .and_then(|s| SystemTime::now().duration_since(s).ok())
        .map(|d| d.as_secs())
    else {
        return "n/a".into();
    };
    let (d, h, m, s) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    if d > 0 {
        format!("{d}d {h:02}h {m:02}m")
    } else {
        format!("{h}h {m:02}m {s:02}s")
    }
}

fn details_panel(
    ui: &mut Ui,
    data: &Data,
    snap: &Snapshot,
    sel: &ProcessRef,
    intents: &mut Vec<Intent>,
) {
    ui.heading(&sel.name);
    let Some(p) = snap
        .processes
        .iter()
        .find(|p| p.pid == sel.pid && p.create_time == sel.create_time)
    else {
        ui.colored_label(WARN, "This process has exited.");
        return;
    };

    egui::Grid::new("details_grid")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            let mut row = |k: &str, v: String| {
                ui.label(k);
                ui.label(v);
                ui.end_row();
            };
            row("PID", p.pid.to_string());
            row("Parent PID", p.parent_pid.to_string());
            row("Session", p.session_id.to_string());
            row("Uptime", uptime(p.start_time()));
            row("CPU", format!("{:.2}%", p.cpu_percent));
            row("CPU time", format!("{:.1} s", p.cpu_time.as_secs_f64()));
            row(
                "Private working set",
                format_bytes(p.private_working_set as f64),
            );
            row("Working set", format_bytes(p.working_set as f64));
            row("Commit", format_bytes(p.private_bytes as f64));
            row("Disk read", format!("{}/s", format_bytes(p.io_read_rate)));
            row("Disk write", format!("{}/s", format_bytes(p.io_write_rate)));
            row("Threads", p.thread_count.to_string());
            row("Handles", p.handle_count.to_string());

            match &data.details {
                Some((r, Ok(d))) if r == sel => {
                    let na = || "n/a".to_owned();
                    row("Priority", d.priority.map_or_else(na, |p| format!("{p:?}")));
                    row(
                        "Efficiency",
                        d.eco_qos.map_or_else(na, |e| format!("{e:?}")),
                    );
                    row(
                        "Affinity",
                        d.affinity.map_or_else(na, |(m, s)| {
                            format!("{} of {} CPUs", m.count_ones(), s.count_ones())
                        }),
                    );
                    row(
                        "Critical",
                        d.critical
                            .map_or_else(na, |c| if c { "yes" } else { "no" }.into()),
                    );
                    row(
                        "Protected",
                        d.protected.clone().unwrap_or_else(|| "no".into()),
                    );
                }
                Some((r, Err(e))) if r == sel => row("Details", e.clone()),
                _ => row("Details", "loading…".into()),
            }
        });

    if let Some((r, Ok(d))) = &data.details
        && r == sel
    {
        if let Some(user) = &d.user {
            ui.add_space(6.0);
            ui.label(format!("User: {user}"));
        }
        if let Some(path) = &d.path {
            ui.add_space(6.0);
            ui.label(RichText::new(path).small().weak());
        }
        if let Some(cmd) = d.command_line.as_deref().filter(|c| !c.is_empty()) {
            ui.add_space(4.0);
            ui.label(RichText::new(cmd).small().monospace());
            if ui.small_button("Copy command line").clicked() {
                ui.ctx().copy_text(cmd.to_owned());
            }
        }
    }

    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        if ui.button(RichText::new("End task").color(BAD)).clicked() {
            intents.push(Intent::Confirm(sel.clone(), Command::Kill));
        }
        ui.menu_button("More actions", |ui| {
            context_menu(ui, sel, snap.system.logical_cpus, intents)
        });
        if ui.button("Open file location").clicked() {
            intents.push(Intent::OpenLocation(sel.clone()));
        }
        if ui.button("Refresh").clicked() {
            intents.push(Intent::Select(sel.clone()));
        }
    });
}

fn history_plot(ui: &mut Ui, id: &str, label: &str, values: &std::collections::VecDeque<f32>) {
    let len = values.len() as f64;
    let points: PlotPoints = values
        .iter()
        .enumerate()
        .map(|(i, v)| [i as f64 - len, f64::from(*v)])
        .collect();
    Plot::new(id)
        .height(190.0)
        .include_y(0.0)
        .include_y(100.0)
        .include_x(-(HISTORY_LEN as f64))
        .include_x(0.0)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .x_axis_label("seconds")
        .y_axis_label("%")
        .show(ui, |plot| plot.line(Line::new(label, points)));
}

/// Two byte rates (for example download and upload) on an auto-scaled axis.
fn rate_plot(ui: &mut Ui, id: &str, series: [(&str, &std::collections::VecDeque<f32>); 2]) {
    let line = |label: &str, values: &std::collections::VecDeque<f32>| {
        let len = values.len() as f64;
        let points: PlotPoints = values
            .iter()
            .enumerate()
            .map(|(i, v)| [i as f64 - len, f64::from(*v)])
            .collect();
        Line::new(label.to_owned(), points)
    };
    Plot::new(id)
        .height(140.0)
        .include_y(0.0)
        .include_y(1024.0)
        .include_x(-(HISTORY_LEN as f64))
        .include_x(0.0)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .x_axis_label("seconds")
        .y_axis_formatter(|mark, _| format_bytes(mark.value.max(0.0)) + "/s")
        .label_formatter(rate_label)
        .legend(egui_plot::Legend::default())
        .show(ui, |plot| {
            for (label, values) in series {
                plot.line(line(label, values));
            }
        });
}

pub(crate) fn rate_label(hover: &egui_plot::HoverPosition<'_>) -> Option<String> {
    match hover {
        egui_plot::HoverPosition::NearDataPoint {
            plot_name,
            position,
            ..
        } => Some(format!(
            "{plot_name}: {}/s",
            format_bytes(position.y.max(0.0))
        )),
        egui_plot::HoverPosition::Elsewhere { .. } => None,
    }
}

/// One small box per logical processor, filled to its load.
fn core_grid(ui: &mut Ui, cores: &[f32]) {
    const CELL: egui::Vec2 = egui::vec2(64.0, 34.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        for (i, &load) in cores.iter().enumerate() {
            let (rect, response) = ui.allocate_exact_size(CELL, egui::Sense::hover());
            let painter = ui.painter_at(rect);
            let visuals = ui.visuals();
            painter.rect_filled(rect, 3.0, visuals.extreme_bg_color);
            let color = if load >= 90.0 {
                BAD
            } else if load >= 60.0 {
                WARN
            } else {
                GOOD
            };
            let mut fill = rect;
            fill.set_top(rect.bottom() - rect.height() * load.clamp(0.0, 100.0) / 100.0);
            painter.rect_filled(fill, 3.0, color.gamma_multiply(0.7));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("{load:.0}%"),
                egui::FontId::proportional(12.0),
                visuals.strong_text_color(),
            );
            let text = format!("CPU {i}: {load:.1}%");
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text.clone()));
            response.on_hover_text(text);
        }
    });
}

pub(crate) fn performance(ui: &mut Ui, data: &Data) {
    let Some(snap) = &data.snapshot else {
        ui.spinner();
        return;
    };
    let s = &snap.system;
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.heading(format!("CPU  {:.1}%", s.cpu_percent));
        history_plot(ui, "cpu_plot", "CPU", &data.history.cpu);
        if !s.core_percent.is_empty() {
            ui.add_space(4.0);
            ui.label(format!("Per logical processor ({})", s.core_percent.len()));
            core_grid(ui, &s.core_percent);
        }
        ui.add_space(10.0);
        let mem_pct = if s.memory_total > 0 {
            s.memory_used() as f64 / s.memory_total as f64 * 100.0
        } else {
            0.0
        };
        ui.heading(format!(
            "Memory  {} / {} ({mem_pct:.0}%)",
            format_bytes(s.memory_used() as f64),
            format_bytes(s.memory_total as f64)
        ));
        history_plot(ui, "mem_plot", "Memory", &data.history.memory);
        ui.add_space(10.0);
        ui.heading(format!("GPU  {:.1}%  (busiest engine)", s.gpu_percent));
        history_plot(ui, "gpu_plot", "GPU", &data.history.gpu);
        ui.add_space(10.0);
        ui.heading(format!(
            "Network  ↓ {}/s  ↑ {}/s",
            format_bytes(s.net_rx_rate),
            format_bytes(s.net_tx_rate)
        ));
        rate_plot(
            ui,
            "net_plot",
            [
                ("Receive", &data.history.net_rx),
                ("Send", &data.history.net_tx),
            ],
        );
        ui.add_space(10.0);
        ui.heading(format!(
            "Disk & I/O  read {}/s  write {}/s",
            format_bytes(s.io_read_rate),
            format_bytes(s.io_write_rate)
        ));
        rate_plot(
            ui,
            "io_plot",
            [
                ("Read", &data.history.io_read),
                ("Write", &data.history.io_write),
            ],
        );
        ui.add_space(10.0);
        egui::Grid::new("perf_grid")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                let mut row = |k: &str, v: String| {
                    ui.label(k);
                    ui.label(v);
                    ui.end_row();
                };
                row("Logical processors", s.logical_cpus.to_string());
                row("Processes", s.process_count.to_string());
                row("Threads", s.thread_count.to_string());
                row("Handles", s.handle_count.to_string());
                row(
                    "Committed",
                    format!(
                        "{} / {}",
                        format_bytes(s.commit_limit.saturating_sub(s.commit_available) as f64),
                        format_bytes(s.commit_limit as f64)
                    ),
                );
            });
    });
}

fn governor(ui: &mut Ui, data: &Data) {
    let Some(status) = &data.status else {
        ui.heading("Service not running");
        ui.label("The governor runs inside the RustWTM service. Install and start it from an elevated prompt:");
        ui.code("wtm-service install\nwtm-service start");
        return;
    };
    egui::Grid::new("gov_grid")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            let mut row = |k: &str, v: RichText| {
                ui.label(k);
                ui.label(v);
                ui.end_row();
            };
            row(
                "Mode",
                if status.dry_run {
                    RichText::new("Dry-run (decisions are only logged)").color(WARN)
                } else {
                    RichText::new("Enforcing").color(GOOD)
                },
            );
            row(
                "Governor",
                RichText::new(if status.governor_enabled {
                    "enabled"
                } else {
                    "disabled"
                }),
            );
            row(
                "Thresholds",
                RichText::new(format!(
                    "act at ≥ {:.0}% CPU, roll back at ≤ {:.0}%",
                    status.high_cpu_percent, status.recover_cpu_percent
                )),
            );
            row("Rules", RichText::new(status.rules.to_string()));
            row("Config", RichText::new(&status.config_path).monospace());
        });

    ui.add_space(12.0);
    ui.heading(format!("Managed processes ({})", status.managed.len()));
    if status.managed.is_empty() {
        ui.weak("No process is currently limited.");
        return;
    }
    use crate::sort::{Sort, header, text};
    let sources = |m: &wtm_ipc::Managed| {
        let s: Vec<String> = m.sources.iter().map(|s| format!("{s:?}")).collect();
        s.join(", ")
    };
    let mut rows: Vec<_> = status.managed.iter().collect();
    let mut sort = Sort::load(ui, "managed", Sort::new(1, false));
    sort.apply(&mut rows, |a, b, col| match col {
        0 => a.target.pid.cmp(&b.target.pid),
        1 => text(&a.target.name, &b.target.name),
        2 => text(&sources(a), &sources(b)),
        _ => text(&a.summary, &b.summary),
    });
    egui::Grid::new("managed_grid")
        .num_columns(4)
        .striped(true)
        .show(ui, |ui| {
            for (col, label) in ["PID", "Name", "Source", "Effective limits"]
                .into_iter()
                .enumerate()
            {
                header(ui, label, col, col == 0, &mut sort);
            }
            ui.end_row();
            for m in rows {
                ui.label(m.target.pid.to_string());
                ui.label(&m.target.name);
                ui.label(sources(m));
                ui.label(&m.summary);
                ui.end_row();
            }
        });
    sort.store(ui, "managed");
}

fn events(ui: &mut Ui, data: &Data) {
    if data.events.is_empty() {
        ui.weak("No events yet.");
        return;
    }
    let row_height = ui.spacing().interact_size.y;
    let items: Vec<_> = data.events.iter().rev().collect();
    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .column(Column::initial(80.0))
        .column(Column::initial(56.0))
        .column(Column::initial(64.0))
        .column(Column::initial(160.0).clip(true))
        .column(Column::remainder().clip(true))
        .header(row_height, |mut h| {
            for label in ["Time", "Level", "PID", "Process", "Message"] {
                h.col(|ui| {
                    ui.strong(label);
                });
            }
        })
        .body(|body| {
            body.rows(row_height, items.len(), |mut row| {
                let e = items[row.index()];
                row.col(|ui| {
                    let time = chrono::DateTime::from_timestamp_millis(e.unix_ms as i64)
                        .map(|t| {
                            t.with_timezone(&chrono::Local)
                                .format("%H:%M:%S")
                                .to_string()
                        })
                        .unwrap_or_default();
                    ui.label(time);
                });
                row.col(|ui| {
                    ui.label(RichText::new(format!("{:?}", e.level)).color(level_color(e.level)));
                });
                row.col(|ui| {
                    ui.label(e.pid.map(|p| p.to_string()).unwrap_or_default());
                });
                row.col(|ui| {
                    ui.label(e.name.clone().unwrap_or_default());
                });
                row.col(|ui| {
                    ui.label(&e.message);
                });
            });
        });
}

/// Actions for the selected service; requires elevation to take effect.
fn service_bar(ui: &mut Ui, data: &Data, selected: Option<&str>) -> Option<UiCommand> {
    let service = selected.and_then(|name| {
        data.services
            .as_ref()?
            .as_ref()
            .ok()?
            .iter()
            .find(|s| s.name == name)
    });
    let mut out = None;
    ui.horizontal(|ui| {
        let Some(s) = service else {
            ui.weak("Select a service to start, stop or change its startup type.");
            return;
        };
        ui.strong(&s.display_name);
        let running = s.state == wtm_core::ServiceState::Running;
        let stopped = s.state == wtm_core::ServiceState::Stopped;
        let mut send = |op| {
            out = Some(UiCommand::Service {
                name: s.name.clone(),
                op,
            });
        };
        if ui
            .add_enabled(stopped, egui::Button::new("Start"))
            .clicked()
        {
            send(ServiceOp::Start);
        }
        if ui.add_enabled(running, egui::Button::new("Stop")).clicked() {
            send(ServiceOp::Stop);
        }
        ui.menu_button("Startup type", |ui| {
            for t in [
                wtm_core::StartType::Automatic,
                wtm_core::StartType::Manual,
                wtm_core::StartType::Disabled,
            ] {
                if ui.button(format!("{t:?}")).clicked() {
                    send(ServiceOp::StartType(t));
                    ui.close();
                }
            }
        });
    });
    ui.separator();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use crate::backend::Backend;
    use crate::ui_tests::data;

    #[test]
    fn full_app_navigates_every_tab() {
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1300.0, 850.0])
            .build_eframe(|cc| App::new(cc));
        let sampled = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(50));
            h.state().backend.data().snapshot.is_some()
        });
        assert!(sampled, "no snapshot from backend");
        h.step();

        for tab in [
            "Performance",
            "Governor",
            "App history",
            "Startup apps",
            "Users",
            "Services",
            "Network",
            "Events",
            "Processes",
        ] {
            nav_to(&mut h, tab);
            h.run_steps(2);
            // Let the backend answer the tab's load request.
            std::thread::sleep(Duration::from_millis(200));
            h.run_steps(2);
        }

        // Select our own process, open its details, ask to end it and cancel.
        let me = std::process::id();
        h.state_mut().state.processes.search = me.to_string();
        h.run_steps(2);
        let name = std::env::current_exe()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        h.get_by_label(&name).click();
        h.run_steps(2);
        std::thread::sleep(Duration::from_millis(300));
        h.run_steps(2);
        assert_eq!(
            h.state().state.processes.selected.as_ref().map(|r| r.pid),
            Some(me)
        );
        h.key_press(egui::Key::Delete);
        h.run_steps(2);
        assert!(h.state().state.confirm.is_some());
        h.get_by_label("Cancel").click();
        h.run_steps(2);
        assert!(h.state().state.confirm.is_none());
    }

    #[test]
    fn panels_render_with_synthetic_data() {
        let mut d = data();
        for level in [EventLevel::Info, EventLevel::Error] {
            let mut e = d.events[0].clone();
            e.level = level;
            d.events.push_back(e);
        }
        let sel = d.details.as_ref().unwrap().0.clone();
        let snap = d.snapshot.clone().unwrap();
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 1600.0])
            .build_ui(|ui| {
                let mut intents = Vec::new();
                ui.columns(2, |cols| {
                    details_panel(&mut cols[0], &d, &snap, &sel, &mut intents);
                    governor(&mut cols[0], &d);
                    let _ = service_bar(&mut cols[0], &d, Some("Spooler"));
                    let _ = service_bar(&mut cols[0], &d, None);
                    for mode in [
                        Mode::Starting,
                        Mode::Local,
                        Mode::Service {
                            admin: true,
                            verified: true,
                        },
                        Mode::Service {
                            admin: false,
                            verified: true,
                        },
                        Mode::Service {
                            admin: true,
                            verified: false,
                        },
                    ] {
                        mode_badge(&mut cols[0], mode);
                    }
                    performance(&mut cols[1], &d);
                    events(&mut cols[1], &d);
                });
            });
        h.run_steps(3);
        assert!(
            h.query_by_label("Dry-run (decisions are only logged)")
                .is_some()
        );
        assert!(h.query_by_label(r"C:\Apps\alpha.exe").is_some());
        drop(h);

        // Empty states.
        d.status = None;
        d.events.clear();
        d.snapshot = None;
        let mut h = Harness::builder().allow_missing_glyphs().build_ui(|ui| {
            governor(ui, &d);
            events(ui, &d);
            performance(ui, &d);
        });
        h.run_steps(2);
        assert!(h.query_by_label("Service not running").is_some());
        assert!(h.query_by_label("No events yet.").is_some());
        drop(h);

        // Enforcing governor without managed processes; no memory figures.
        let mut d = data();
        let status = d.status.as_mut().unwrap();
        status.dry_run = false;
        status.governor_enabled = false;
        status.managed.clear();
        let mut snap = crate::ui_tests::snapshot();
        snap.system.memory_total = 0;
        d.snapshot = Some(std::sync::Arc::new(snap));
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([900.0, 1200.0])
            .build_ui(|ui| {
                governor(ui, &d);
                performance(ui, &d);
            });
        h.run_steps(2);
        assert!(h.query_by_label("Enforcing").is_some());
        assert!(h.query_by_label("disabled").is_some());
        assert!(
            h.query_by_label("No process is currently limited.")
                .is_some()
        );
        assert!(h.query_by_label_contains("(0%)").is_some());
    }

    #[test]
    fn uptime_formats_days_and_hours() {
        assert_eq!(uptime(None), "n/a");
        let start = SystemTime::now() - Duration::from_secs(2 * 86_400 + 3 * 3600 + 4 * 60);
        assert!(uptime(Some(start)).starts_with("2d 03h 04m"));
        let start = SystemTime::now() - Duration::from_secs(3661);
        assert!(uptime(Some(start)).starts_with("1h 01m 0"));
    }

    /// Clicks the navigation entry ending with `label` (entries carry an icon).
    fn nav_to(h: &mut Harness<'_, App>, label: &str) {
        use egui_kittest::kittest::NodeT;
        h.get_all_by_role(egui::accesskit::Role::Button)
            .find(|n| {
                n.accesskit_node()
                    .label()
                    .is_some_and(|l| l.ends_with(label))
            })
            .unwrap_or_else(|| panic!("no navigation entry {label}"))
            .click();
    }

    fn describe_command(c: UiCommand) -> String {
        match c {
            UiCommand::Control { target, command } => format!("control {} {command:?}", target.pid),
            UiCommand::Inspect(t) => format!("inspect {}", t.pid),
            UiCommand::Load(panel) => format!("load {panel:?}"),
            UiCommand::LoadHistory { hours } => format!("history {hours}"),
            UiCommand::SetStartup { entry, enabled } => {
                format!("startup {} {enabled}", entry.name)
            }
            UiCommand::LoadSeries { app, hours } => format!("series {app} {hours}"),
            UiCommand::Service { name, op } => format!("service {name} {op:?}"),
            UiCommand::SaveConfig(toml) => format!("save config {}", toml.len()),
            UiCommand::OpenLocation(t) => format!("open {}", t.pid),
        }
    }

    #[test]
    fn app_sends_commands_for_clicks_on_synthetic_data() {
        let (backend, rx) = Backend::offline(data());
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 900.0])
            .build_eframe(|_| App {
                backend,
                state: UiState {
                    tab: Tab::Processes,
                    ..Default::default()
                },
                tray: None,
            });
        let mut sent = Vec::new();
        let step = |h: &mut Harness<'_, App>, sent: &mut Vec<String>| {
            h.run_steps(2);
            sent.extend(rx.try_iter().map(describe_command));
            sent.clone()
        };
        step(&mut h, &mut sent);
        // The status bar shows the backend's message.
        assert!(h.query_by_label("ready").is_some());

        // Select a process, then use the buttons of its details panel.
        h.get_by_label("alpha.exe").click();
        step(&mut h, &mut sent);
        h.get_by_label("Refresh").click();
        step(&mut h, &mut sent);
        h.get_by_label("More actions").click();
        step(&mut h, &mut sent);
        h.get_by_label("Suspend").click();
        let s = step(&mut h, &mut sent);
        assert_eq!(s, ["inspect 100", "inspect 100", "control 100 Suspend"]);

        // Account, command line and file location.
        assert!(h.query_by_label(r"User: PC\user").is_some());
        h.get_by_label("Copy command line").click();
        step(&mut h, &mut sent);
        h.get_all_by_label("Open file location")
            .next()
            .unwrap()
            .click();
        let s = step(&mut h, &mut sent);
        assert_eq!(s.last().map(String::as_str), Some("open 100"));

        // End task asks first; the dialog's own button sends the command.
        h.get_by_label("End task").click();
        step(&mut h, &mut sent);
        assert!(h.state().state.confirm.is_some());
        // The dialog appears on the frame after the request.
        h.run();
        h.get_all_by_role_and_label(egui::accesskit::Role::Button, "End task")
            .last()
            .unwrap()
            .click();
        let s = step(&mut h, &mut sent);
        assert_eq!(s.last().map(String::as_str), Some("control 100 Kill"));
        assert!(h.state().state.confirm.is_none());
        // Escape dismisses the dialog.
        let alpha = h.state().state.processes.selected.clone().unwrap();
        // Ending every process of an app asks once for all of them.
        h.state_mut().state.confirm_many =
            Some((vec![alpha.clone(), alpha.clone()], Command::Kill));
        h.run();
        h.run();
        h.get_by_label("End task all").click();
        let s = step(&mut h, &mut sent);
        assert_eq!(s.iter().filter(|c| *c == "control 100 Kill").count(), 3);
        h.state_mut().state.confirm_many = Some((vec![alpha.clone()], Command::Kill));
        h.run();
        h.run();
        h.get_by_label("Cancel").click();
        h.run();
        assert!(h.state().state.confirm_many.is_none());
        h.state_mut().state.confirm = Some((alpha.clone(), Command::KillTree));
        step(&mut h, &mut sent);
        h.key_press(egui::Key::Escape);
        step(&mut h, &mut sent);
        assert!(h.state().state.confirm.is_none());

        // Failed details and a process that has gone away.
        h.state().backend.data().details = Some((alpha, Err("access denied".into())));
        step(&mut h, &mut sent);
        assert!(h.query_by_label("access denied").is_some());
        h.state_mut().state.processes.selected = Some(ProcessRef {
            pid: 999,
            create_time: 1,
            name: "gone.exe".into(),
        });
        step(&mut h, &mut sent);
        assert!(h.query_by_label("This process has exited.").is_some());
        // Details of another process are not shown for this one.
        h.state_mut().state.processes.selected = Some(ProcessRef::from(&crate::ui_tests::proc(
            200, 4, "beta.exe", 0.5,
        )));
        step(&mut h, &mut sent);
        assert!(h.query_by_label("loading…").is_some());

        // App history: choosing an app reloads the summary and its series.
        sent.clear();
        nav_to(&mut h, "App history");
        step(&mut h, &mut sent);
        h.get_by_label("alpha.exe").click();
        let s = step(&mut h, &mut sent);
        assert_eq!(s, ["history 24", "history 24", "series alpha.exe 24"]);

        // Startup apps: toggle the selected entry.
        sent.clear();
        nav_to(&mut h, "Startup apps");
        step(&mut h, &mut sent);
        h.get_by_label("Updater").click();
        step(&mut h, &mut sent);
        h.get_by_label("Disable").click();
        let s = step(&mut h, &mut sent);
        assert_eq!(s, ["load Startup", "startup Updater false"]);

        // Services: stop a running one, change its startup type, start a
        // stopped one.
        sent.clear();
        nav_to(&mut h, "Services");
        step(&mut h, &mut sent);
        h.get_by_label("Print Spooler").click();
        step(&mut h, &mut sent);
        h.get_by_label("Stop").click();
        step(&mut h, &mut sent);
        h.get_by_label("Startup type").click();
        step(&mut h, &mut sent);
        h.get_by_label("Disabled").click();
        step(&mut h, &mut sent);
        h.get_all_by_label("Fax").next().unwrap().click();
        step(&mut h, &mut sent);
        h.get_by_label("Start").click();
        let s = step(&mut h, &mut sent);
        assert_eq!(
            s,
            [
                "load Services",
                "service Spooler Stop",
                "service Spooler StartType(Disabled)",
                "service Fax Start",
            ]
        );
    }

    #[test]
    fn settings_tab_validates_toggles_and_saves() {
        let mut d = data();
        d.mode = Mode::Service {
            admin: true,
            verified: true,
        };
        d.config = Some(Ok((
            r"C:\ProgramData\RustWTM\config.toml".into(),
            wtm_policy::DEFAULT_CONFIG_TOML.into(),
        )));
        let (backend, rx) = Backend::offline(d);
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 900.0])
            .build_eframe(|_| App {
                backend,
                state: UiState::default(),
                tray: None,
            });
        h.run_steps(2);
        nav_to(&mut h, "Settings");
        h.run_steps(2);
        assert!(h.query_by_label_contains("Valid configuration").is_some());
        assert!(h.query_by_label_contains("unsaved").is_none());

        h.get_by_label("Switch to enforcing").click();
        h.run_steps(2);
        assert!(h.query_by_label_contains("unsaved").is_some());
        assert!(h.query_by_label("Switch to dry-run").is_some());
        h.get_by_label("Save").click();
        h.run_steps(2);
        let sent: Vec<UiCommand> = rx.try_iter().collect();
        assert!(
            sent.iter()
                .any(|c| matches!(c, UiCommand::Load(Panel::Config)))
        );
        let saved = sent.iter().find_map(|c| match c {
            UiCommand::SaveConfig(toml) => Some(toml.clone()),
            _ => None,
        });
        let saved = saved.expect("Save sends the edited text");
        assert!(wtm_policy::Config::from_toml(&saved).is_ok_and(|c| !c.dry_run));

        // Discarding asks the service for its text again.
        h.get_by_label("Discard changes").click();
        h.run_steps(2);
        assert!(
            rx.try_iter()
                .any(|c| matches!(c, UiCommand::Load(Panel::Config)))
        );
    }

    #[test]
    fn settings_tab_shows_errors_and_needs_admin_to_save() {
        let mut d = data();
        d.config = Some(Err("service not running".into()));
        let (backend, rx) = Backend::offline(d);
        let state = UiState {
            tab: Tab::Settings,
            ..Default::default()
        };
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 900.0])
            .build_eframe(|_| App {
                backend,
                state,
                tray: None,
            });
        h.run_steps(2);
        assert!(h.query_by_label("service not running").is_some());
        h.get_by_label("Retry").click();
        h.run_steps(2);
        assert!(
            rx.try_iter()
                .any(|c| matches!(c, UiCommand::Load(Panel::Config)))
        );
    }

    #[test]
    fn limits_tab_lists_edits_and_removes_app_limits() {
        let mut d = data();
        d.mode = Mode::Service {
            admin: true,
            verified: true,
        };
        let text =
            "dry_run = true\n[[limit]]\nname = \"beta.exe\"\ncpu_cores = 2.0\nmemory_gb = 8.0\n";
        d.config = Some(Ok(("config.toml".into(), text.into())));
        let (backend, rx) = Backend::offline(d);
        let state = UiState {
            tab: Tab::Limits,
            ..Default::default()
        };
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 900.0])
            .build_eframe(|_| App {
                backend,
                state,
                tray: None,
            });
        h.run_steps(2);
        assert!(h.query_by_label_contains("2 cores, 8 GB").is_some());
        assert!(h.query_by_label("1 (1 outside)").is_some());
        assert!(h.query_by_label_contains("Dry-run mode").is_some());
        let saved = |rx: &std::sync::mpsc::Receiver<UiCommand>| {
            rx.try_iter().find_map(|c| match c {
                UiCommand::SaveConfig(t) => Some(wtm_policy::Config::from_toml(&t).unwrap()),
                _ => None,
            })
        };

        h.get_by_label("Edit").click();
        h.run_steps(2);
        assert!(h.query_by_label("Change limit").is_some());
        h.get_by_label("Each instance separately").click();
        h.run_steps(1);
        h.get_by_label("Save limit").click();
        h.run_steps(2);
        let c = saved(&rx).expect("saved");
        assert_eq!(c.limits.len(), 1);
        assert_eq!(c.limits[0].scope, wtm_policy::LimitScope::Each);

        h.get_by_label("Remove").click();
        h.run_steps(2);
        assert!(saved(&rx).expect("saved").limits.is_empty());

        // A new limit from the form.
        h.get_all_by_role(egui::accesskit::Role::TextInput)
            .next()
            .unwrap()
            .click();
        h.run();
        h.get_all_by_role(egui::accesskit::Role::TextInput)
            .next()
            .unwrap()
            .type_text("node.exe");
        h.run_steps(2);
        h.get_by_label("Save limit").click();
        h.run_steps(2);
        let c = saved(&rx).expect("saved");
        assert_eq!(c.limits.len(), 2);
        assert_eq!(c.limits[1].name, "node.exe");

        // Picking a running app fills in its name; the existing entry is replaced.
        h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "or a running app")
            .click();
        h.run_steps(2);
        h.get_by_label_contains("beta.exe  (").click();
        h.run_steps(2);
        h.get_by_label("Save limit").click();
        h.run_steps(2);
        let c = saved(&rx).expect("saved");
        assert_eq!(c.limits.len(), 1);
        assert_eq!(c.limits[0].name, "beta.exe");
        assert_eq!(c.limits[0].memory_gb, None);
    }

    #[test]
    fn limits_tab_handles_missing_and_invalid_configuration() {
        for config in [
            None,
            Some(Err("service not running".to_owned())),
            Some(Ok(("c".to_owned(), "dry_run = 3".to_owned()))),
            Some(Ok(("c".to_owned(), String::new()))),
        ] {
            let mut d = data();
            d.config = config;
            let (backend, rx) = Backend::offline(d);
            let state = UiState {
                tab: Tab::Limits,
                ..Default::default()
            };
            let mut h = Harness::builder()
                .allow_missing_glyphs()
                .with_size([1400.0, 900.0])
                .build_eframe(|_| App {
                    backend,
                    state,
                    tray: None,
                });
            h.run_steps(2);
            if let Some(retry) = h.query_by_label("Retry") {
                retry.click();
                h.run_steps(2);
            }
            assert!(
                rx.try_iter()
                    .any(|c| matches!(c, UiCommand::Load(Panel::Config)))
            );
        }
    }

    fn overview_app(d: Data) -> (Harness<'static, App>, std::sync::mpsc::Receiver<UiCommand>) {
        let (backend, rx) = Backend::offline(d);
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 1000.0])
            .build_eframe(|_| App {
                backend,
                state: UiState::default(),
                tray: None,
            });
        h.run_steps(2);
        (h, rx)
    }

    fn saved_config(rx: &std::sync::mpsc::Receiver<UiCommand>) -> Option<wtm_policy::Config> {
        rx.try_iter().find_map(|c| match c {
            UiCommand::SaveConfig(t) => Some(wtm_policy::Config::from_toml(&t).unwrap()),
            _ => None,
        })
    }

    #[test]
    fn overview_turns_protection_on_and_picks_a_level() {
        let mut d = data();
        d.mode = Mode::Service {
            admin: true,
            verified: true,
        };
        d.config = Some(Ok((
            "config.toml".into(),
            "dry_run = true\nblock = [\"x.exe\"]\n[[limit]]\nname = \"beta.exe\"\ncpu_cores = 1.5\n".into(),
        )));
        let (mut h, rx) = overview_app(d);
        assert_eq!(h.state().state.tab, Tab::Overview);
        assert!(h.query_by_label_contains("Monitoring only").is_some());
        assert!(
            h.query_by_label_contains("alpha.exe is in use and never throttled")
                .is_some()
        );
        assert!(h.query_by_label_contains("1 programs blocked").is_some());
        assert!(
            h.query_by_label_contains("1 app limits covering 1 processes")
                .is_some()
        );
        assert!(
            h.query_by_label_contains("Governor: priority BelowNormal")
                .is_some()
        );

        h.get_by_label("Turn protection on").click();
        h.run_steps(2);
        assert!(!saved_config(&rx).expect("saved").dry_run);

        h.get_by_label("Strict").click();
        h.run_steps(2);
        let c = saved_config(&rx).expect("saved");
        assert_eq!(
            wtm_policy::Preset::of(&c.governor),
            Some(wtm_policy::Preset::Strict)
        );
        // The current level does nothing when clicked again.
        h.get_by_label("Balanced").click();
        h.run_steps(2);
        assert!(saved_config(&rx).is_none());

        // Quick actions on the busiest app.
        h.get_all_by_label("Efficiency mode")
            .next()
            .unwrap()
            .click();
        h.run_steps(2);
        assert!(rx.try_iter().any(|c| matches!(
            c,
            UiCommand::Control {
                command: Command::EcoQos(wtm_core::control::EcoQos::On),
                ..
            }
        )));
        h.get_all_by_label("Free memory").next().unwrap().click();
        h.run_steps(2);
        assert!(rx.try_iter().any(|c| matches!(
            c,
            UiCommand::Control {
                command: Command::TrimMemory,
                ..
            }
        )));
        assert!(
            h.query_by_label_contains("memory guard above 90% RAM")
                .is_some()
        );
        h.get_all_by_label("Details").next().unwrap().click();
        h.run_steps(2);
        assert_eq!(h.state().state.tab, Tab::Processes);
        assert!(h.state().state.processes.selected.is_some());
    }

    #[test]
    fn overview_explains_missing_service_and_bad_configs() {
        // Local mode: no service.
        let (h, _rx) = overview_app(data());
        assert!(
            h.query_by_label_contains("service is not running")
                .is_some()
        );
        drop(h);

        let mut d = data();
        d.mode = Mode::Service {
            admin: false,
            verified: true,
        };
        d.config = Some(Ok((
            "c".into(),
            "dry_run = false\n[governor]\nhigh_cpu_percent = 85.0\n".into(),
        )));
        let (mut h, _rx) = overview_app(d);
        assert!(h.query_by_label_contains("Protection is on").is_some());
        assert!(h.query_by_label_contains("Custom").is_some());
        assert!(h.query_by_label_contains("as administrator").is_some());
        h.get_all_by_label("Limit…").next().unwrap().click();
        h.run_steps(2);
        assert_eq!(h.state().state.tab, Tab::Limits);
        drop(h);

        let mut d = data();
        d.mode = Mode::Service {
            admin: true,
            verified: true,
        };
        d.config = Some(Ok(("c".into(), "dry_run = 3".into())));
        let (h, _rx) = overview_app(d);
        assert!(
            h.query_by_label_contains("configuration is invalid")
                .is_some()
        );
        drop(h);

        // No snapshot and no events yet.
        let mut d = data();
        d.snapshot = None;
        d.events.clear();
        let (h, _rx) = overview_app(d);
        assert!(h.query_by_label_contains("Nothing yet").is_some());
    }

    #[test]
    fn about_tab_is_reachable() {
        let (mut h, _rx) = overview_app(data());
        nav_to(&mut h, "About");
        h.run_steps(2);
        assert!(h.query_by_label("Crafted by Ersin KOÇ").is_some());
    }

    #[test]
    fn navigation_collapses_to_icons_by_choice_or_on_narrow_windows() {
        let (mut h, _rx) = overview_app(data());
        assert!(
            h.query_by_label("RustWTM").is_some(),
            "labels shown on wide windows"
        );
        h.get_by_label("«").click();
        h.run_steps(2);
        assert_eq!(h.state().state.nav_collapsed, Some(true));
        assert!(h.query_by_label("⚙").is_some(), "icon-only entries");
        h.get_by_label("⚙").click();
        h.run_steps(2);
        assert_eq!(h.state().state.tab, Tab::Settings);
        h.get_by_label("»").click();
        h.run_steps(2);
        assert_eq!(h.state().state.nav_collapsed, Some(false));
        drop(h);

        let (backend, _rx) = Backend::offline(data());
        let h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([900.0, 700.0])
            .build_eframe(|_| App {
                backend,
                state: UiState::default(),
                tray: None,
            });
        let mut h = h;
        h.run_steps(2);
        assert!(
            h.query_by_label("»").is_some(),
            "narrow windows start collapsed"
        );
    }
}

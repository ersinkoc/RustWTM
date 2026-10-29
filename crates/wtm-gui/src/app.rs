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
use crate::theme::{BAD, GOOD, WARN};
use crate::tray::Tray;
use crate::views;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Tab {
    Overview,
    Processes,
    Performance,
    Disks,
    Sensors,
    Governor,
    Events,
    History,
    Startup,
    Users,
    Services,
    Network,
    Ports,
    Limits,
    KillList,
    Settings,
    About,
}

/// Navigation follows the task: observe, protect, inspect the system, configure.
const NAV: [(Tab, crate::navigation::Icon, &str); 17] = {
    use crate::navigation::Icon as I;
    [
        (Tab::Overview, I::Overview, "Overview"),
        (Tab::Processes, I::Processes, "Processes"),
        (Tab::Performance, I::Performance, "Performance"),
        (Tab::Disks, I::Disk, "Disks"),
        (Tab::Sensors, I::Sensors, "Sensors & cooling"),
        (Tab::History, I::History, "App history"),
        (Tab::Governor, I::Shield, "Governor"),
        (Tab::Limits, I::Limits, "Limits"),
        (Tab::KillList, I::Stop, "Kill list"),
        (Tab::Events, I::Events, "Events"),
        (Tab::Startup, I::Startup, "Startup apps"),
        (Tab::Services, I::Services, "Services"),
        (Tab::Users, I::Users, "Users"),
        (Tab::Network, I::Network, "Network"),
        (Tab::Ports, I::Ports, "Ports"),
        (Tab::Settings, I::Settings, "Settings"),
        (Tab::About, I::About, "About"),
    ]
};
const NAV_AUTO_COLLAPSE: f32 = 1000.0;

fn nav_width(ctx: &egui::Context) -> f32 {
    ctx.fonts_mut(|fonts| {
        NAV.iter()
            .map(|(_, _, label)| {
                fonts
                    .layout_no_wrap(
                        crate::i18n::tr(label).to_owned(),
                        egui::FontId::proportional(14.0),
                        Color32::WHITE,
                    )
                    .size()
                    .x
                    // The row's label offset and gap, and the panel's margins.
                    + crate::navigation::LABEL_LEFT
                    + crate::navigation::LABEL_RIGHT_GAP
                    + 20.0
            })
            .fold(200.0, f32::max)
            .min(280.0)
    })
}

impl Tab {
    fn label(self) -> &'static str {
        crate::i18n::tr(NAV.iter().find(|n| n.0 == self).map_or("RustWTM", |n| n.2))
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Overview => crate::i18n::tr("Your machine at a glance"),
            Self::Processes => crate::i18n::tr("Find what is busy. Keep what matters running."),
            Self::Performance => crate::i18n::tr("Live resource usage and recent trends"),
            Self::Disks => crate::i18n::tr("Free space, disk activity and what takes up the space"),
            Self::Sensors => crate::i18n::tr("Temperatures, fan speeds and thermal control"),
            Self::History => crate::i18n::tr("Understand resource usage over time"),
            Self::Governor => crate::i18n::tr("How automatic protection keeps your PC responsive"),
            Self::Limits => crate::i18n::tr("Give each app a predictable resource budget"),
            Self::KillList => crate::i18n::tr("Choose which background tasks may be ended"),
            Self::Events => crate::i18n::tr("A record of decisions and actions"),
            Self::Startup => crate::i18n::tr("Choose what starts when you sign in"),
            Self::Services => crate::i18n::tr("Inspect and manage Windows services"),
            Self::Users => crate::i18n::tr("Sessions and the people using this machine"),
            Self::Network => crate::i18n::tr("Connections and the processes behind them"),
            Self::Ports => crate::i18n::tr("Find listeners, developer tools and available ports"),
            Self::Settings => crate::i18n::tr("Make RustWTM work the way you do"),
            Self::About => crate::i18n::tr("Built for a responsive Windows PC"),
        }
    }
}

fn nav(ui: &mut Ui, state: &mut UiState, collapsed: bool) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let id = egui::Id::unique("nav-logo-texture");
        let logo = ui
            .data(|d| d.get_temp::<egui::TextureHandle>(id))
            .unwrap_or_else(|| {
                let texture = ui.ctx().load_texture(
                    "nav-logo",
                    egui::ColorImage::from_rgba_unmultiplied([32, 32], &crate::logo::rgba(32)),
                    egui::TextureOptions::LINEAR,
                );
                ui.data_mut(|d| d.insert_temp(id, texture.clone()));
                texture
            });
        ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(28.0, 28.0)));
        if !collapsed {
            ui.vertical(|ui| {
                ui.label(RichText::new("RustWTM").size(18.0).strong());
                ui.label(
                    RichText::new(crate::i18n::tr("SYSTEM MONITOR"))
                        .size(9.0)
                        .weak(),
                );
            });
        }
    });
    ui.add_space(8.0);
    let (arrow, hint) = if collapsed {
        ("»", crate::i18n::tr("Show labels"))
    } else {
        ("«", crate::i18n::tr("Icons only"))
    };
    if ui.small_button(arrow).on_hover_text(hint).clicked() {
        state.nav_collapsed = Some(!collapsed);
    }
    for (i, (tab, icon, label)) in NAV.iter().enumerate() {
        let group = match i {
            0 => Some(crate::i18n::tr("MONITOR")),
            6 => Some(crate::i18n::tr("PROTECT")),
            10 => Some(crate::i18n::tr("SYSTEM")),
            15 => Some("RUSTWTM"),
            _ => None,
        };
        if let Some(group) = group {
            ui.add_space(10.0);
            if collapsed {
                ui.separator();
            } else {
                ui.label(RichText::new(group).size(10.0).weak());
            }
        }
        if crate::navigation::row(
            ui,
            *icon,
            crate::i18n::tr(label),
            state.tab == *tab,
            collapsed,
        )
        .clicked()
        {
            state.tab = *tab;
        }
    }
}
pub struct App {
    backend: Backend,
    state: UiState,
    tray: Option<Tray>,
    frames: crate::frames::FrameStats,
}

struct UiState {
    language: crate::i18n::Language,
    language_error: Option<String>,
    tab: Tab,
    performance_focus: Option<crate::overview::Resource>,
    processes: ProcessView,
    sensors: crate::sensors::SensorsView,
    confirm: Option<(ProcessRef, Command)>,
    /// A session to disconnect or sign out, once confirmed: (id, user, op).
    confirm_session: Option<(u32, String, crate::backend::SessionOp)>,
    /// A destructive command for every process of an app.
    confirm_many: Option<(Vec<ProcessRef>, Command)>,
    /// A service or firewall change that is easy to make by mistake.
    confirm_change: Option<Change>,
    network_filter: String,
    /// The selected process the Network tab was asked to show everything
    /// for; a new selection narrows it again.
    network_all_for: Option<ProcessRef>,
    services_filter: String,
    /// The Events tab shows only this level; `None` shows all.
    event_level: Option<EventLevel>,
    selected_startup: Option<wtm_core::startup::StartupEntry>,
    history: views::HistoryView,
    history_dirty: bool,
    selected_service: Option<String>,
    config: crate::settings::ConfigView,
    telegram: crate::telegram_settings::TelegramView,
    limits: crate::limits_tab::LimitsView,
    kill: crate::kill_tab::KillView,
    ports: crate::ports_tab::PortsView,
    disks: crate::disks_tab::DisksView,
    /// The panic confirmation is open.
    panic: bool,
    /// `None`: collapse automatically on narrow windows.
    nav_collapsed: Option<bool>,
    /// The Run new task dialog: command line and "as administrator".
    run_task: Option<(String, bool)>,
    /// When to pop up on CPU overload.
    alerts: crate::alert::AlertSettings,
    /// The close button hides the window; the tray menu exits.
    tray_on_close: bool,
    /// Start in the tray at sign-in; read from the registry when needed.
    autostart: Option<bool>,
    /// Frames left in which the window is hidden (started in the tray).
    hide_frames: u8,
    check_updates: bool,
    skip_version: Option<String>,
    /// "Later" on the update bar: hidden until the next start.
    update_later: bool,
    /// "Update now" was clicked (its failure is shown).
    update_started: bool,
    last_update_check: Option<Instant>,
    last_load: HashMap<Panel, Instant>,
    /// The Limits column of the process table, for this data revision and
    /// language.
    limit_labels: Option<((u64, crate::i18n::Language), ManagedMap)>,
}

impl App {
    /// `background`: start hidden in the tray.
    pub fn new(cc: &eframe::CreationContext<'_>, background: bool) -> Self {
        // Effective only when started elevated; harmless otherwise.
        wtm_core::control::enable_debug_privilege();
        crate::backend::register_app_id();
        crate::theme::install_fonts(&cc.egui_ctx);
        crate::theme::apply(&cc.egui_ctx);
        let backend = Backend::start(cc.egui_ctx.clone());
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = cc.window_handle()
            && let RawWindowHandle::Win32(w) = handle.as_raw()
        {
            backend
                .window
                .store(w.hwnd.get(), std::sync::atomic::Ordering::Relaxed);
        }
        let tray = Tray::new(
            &cc.egui_ctx,
            backend.notifications.clone(),
            backend.window.clone(),
        );
        let first_run = cc.storage.is_none_or(|s| s.get_string(PREFS_KEY).is_none());
        if first_run && !cfg!(test) {
            // Ready to pop up on overload from the next sign-in on; the user
            // can turn it off in Settings.
            let _ = crate::backend::set_autostart(true);
        }
        let mut state = UiState::restored(cc.storage);
        state.language = crate::i18n::preference();
        // eframe shows the window after its first frame; hide it again.
        state.hide_frames = if background { 3 } else { 0 };
        Self {
            backend,
            tray,
            state,
            frames: crate::frames::FrameStats::from_env(),
        }
    }
}

const PREFS_KEY: &str = "prefs";

/// What the window remembers between runs (eframe keeps the window size and
/// the table sorts itself).
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Prefs {
    language: crate::i18n::Language,
    tab: Tab,
    nav_collapsed: Option<bool>,
    process_mode: crate::processes::ViewMode,
    process_sort: crate::processes::Col,
    process_descending: bool,
    process_columns: std::collections::HashSet<crate::processes::Col>,
    process_heat: bool,
    cooling_profiles: HashMap<String, wtm_policy::CoolingProfile>,
    alerts: crate::alert::AlertSettings,
    tray_on_close: bool,
    check_updates: bool,
    /// A release the user chose to skip.
    skip_version: Option<String>,
    /// Ports always listed in the Ports tab.
    watched_ports: Vec<u16>,
}

impl Default for Prefs {
    fn default() -> Self {
        UiState::default().prefs()
    }
}

impl UiState {
    fn prefs(&self) -> Prefs {
        let p = &self.processes;
        Prefs {
            language: self.language,
            tab: self.tab,
            nav_collapsed: self.nav_collapsed,
            process_mode: p.mode,
            process_sort: p.sort,
            process_descending: p.descending,
            process_columns: p.visible.clone(),
            process_heat: p.heat,
            cooling_profiles: self.sensors.profiles.clone(),
            alerts: self.alerts,
            tray_on_close: self.tray_on_close,
            check_updates: self.check_updates,
            skip_version: self.skip_version.clone(),
            watched_ports: self.ports.watched.clone(),
        }
    }

    /// The default state with the preferences saved by the previous run;
    /// unreadable ones (from an older version) are ignored.
    fn restored(storage: Option<&dyn eframe::Storage>) -> Self {
        let mut state = Self::default();
        if let Some(prefs) = storage.and_then(|s| eframe::get_value::<Prefs>(s, PREFS_KEY)) {
            state.apply(prefs);
        }
        state
    }

    fn apply(&mut self, prefs: Prefs) {
        self.language = prefs.language;
        self.tab = prefs.tab;
        self.nav_collapsed = prefs.nav_collapsed;
        let p = &mut self.processes;
        p.mode = prefs.process_mode;
        p.sort = prefs.process_sort;
        p.descending = prefs.process_descending;
        p.visible = prefs.process_columns;
        p.heat = prefs.process_heat;
        self.sensors.profiles = prefs.cooling_profiles;
        self.alerts = prefs.alerts;
        self.tray_on_close = prefs.tray_on_close;
        self.check_updates = prefs.check_updates;
        self.skip_version = prefs.skip_version;
        self.ports.watched = prefs.watched_ports;
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            language: crate::i18n::Language::System,
            language_error: None,
            tab: Tab::Overview,
            performance_focus: None,
            processes: ProcessView::default(),
            sensors: crate::sensors::SensorsView::default(),
            confirm: None,
            confirm_session: None,
            confirm_many: None,
            confirm_change: None,
            network_filter: String::new(),
            network_all_for: None,
            services_filter: String::new(),
            event_level: None,
            selected_startup: None,
            history: views::HistoryView::default(),
            history_dirty: true,
            selected_service: None,
            config: crate::settings::ConfigView::default(),
            telegram: crate::telegram_settings::TelegramView::default(),
            limits: crate::limits_tab::LimitsView::default(),
            kill: crate::kill_tab::KillView::default(),
            ports: crate::ports_tab::PortsView::default(),
            disks: crate::disks_tab::DisksView::default(),
            panic: false,
            nav_collapsed: None,
            run_task: None,
            alerts: crate::alert::AlertSettings::default(),
            tray_on_close: true,
            autostart: None,
            hide_frames: 0,
            check_updates: true,
            skip_version: None,
            update_later: false,
            update_started: false,
            last_update_check: None,
            last_load: HashMap::new(),
            limit_labels: None,
        }
    }
}

/// The colour of an event's text: secondary for information.
fn level_color(ui: &Ui, level: EventLevel) -> Color32 {
    match level {
        EventLevel::Info => ui.visuals().weak_text_color(),
        EventLevel::Warn => crate::theme::text_color(ui, WARN),
        EventLevel::Error => crate::theme::text_color(ui, BAD),
    }
}

/// Windows shorter than this leave out the page subtitle.
const COMPACT_HEIGHT: f32 = 640.0;

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, PREFS_KEY, &self.state.prefs());
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let _language = crate::i18n::scope(self.state.language);
        let ctx = ui.ctx().clone();
        let App {
            backend,
            state,
            tray,
            frames,
        } = self;
        let frame_start = Instant::now();
        let mut data = backend.data();
        let data_ready = Instant::now();
        if let Some(page) = frames.bench_page() {
            state.tab = NAV[page].0;
            ctx.request_repaint();
        }
        // Freshness must age even if the worker is blocked or stops repainting.
        ctx.request_repaint_after(Duration::from_secs(1));
        if let Some(tray) = tray.as_mut() {
            tray.localize();
        }
        let service_mode = matches!(data.mode, Mode::Service { .. });
        data.alert_watch.settings = state.alerts;
        if state.hide_frames > 0 && data.alert_watch.active.is_none() {
            state.hide_frames -= 1;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            let window = backend.window.load(std::sync::atomic::Ordering::Relaxed);
            if window != 0 {
                wtm_core::window::hide(window);
            }
        }
        // The close button keeps RustWTM watching from the tray.
        if ctx.input(|i| i.viewport().close_requested())
            && state.tray_on_close
            && tray.is_some()
            && !crate::tray::exit_requested(&ctx)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        if let (Some(tray), Some(s)) = (tray.as_mut(), &data.snapshot) {
            let s = &s.system;
            let memory = s.memory_used() as f64 / s.memory_total.max(1) as f64 * 100.0;
            tray.set_tooltip({
                let __wtm_i18n_0 = &(s.cpu_percent);
                crate::i18n::format(
                    "RustWTM - CPU {0}%, memory {1}%",
                    &[format!("{:.0}", __wtm_i18n_0), format!("{:.0}", memory)],
                )
            });
            tray.set_load(s.cpu_percent);
        }
        let mut intents = Vec::new();
        let mut service_cmds: Vec<UiCommand> = Vec::new();

        if state.check_updates
            && state
                .last_update_check
                .is_none_or(|t| t.elapsed() >= UPDATE_CHECK_EVERY)
        {
            state.last_update_check = Some(Instant::now());
            service_cmds.push(UiCommand::CheckUpdate);
        }
        // Before the pages: a text field that Esc just left keeps it.
        shortcuts(&ctx, state);
        update_bar(ui, &data, state, &mut service_cmds);

        // Narrow windows get the icon-only rail unless the user chose.
        let collapsed = state
            .nav_collapsed
            .unwrap_or(ui.ctx().content_rect().width() < NAV_AUTO_COLLAPSE);
        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(if collapsed { 56.0 } else { nav_width(&ctx) })
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("nav_scroll")
                    .show(ui, |ui| nav(ui, state, collapsed));
            });
        egui::Panel::bottom("status").show(ui, |ui| {
            // Actions keep their space even when an operation reports a long
            // error. Wrap the toolbar, then truncate the separate message row.
            ui.horizontal_wrapped(|ui| {
                mode_badge(
                    ui,
                    data.mode,
                    crate::backend::service_problem(data.service_error.as_deref()),
                    stale_age(&data, Instant::now()),
                );
                if let Some(game) = data.status.as_ref().and_then(|s| s.game.as_ref()) {
                    ui.label(
                        RichText::new(crate::i18n::tr("▶ Game mode"))
                            .color(crate::theme::text_color(ui, crate::theme::GPU))
                            .strong(),
                    )
                    .on_hover_text(crate::overview::game_text(game));
                }
                if let Some(rescue) = data.status.as_ref().and_then(|s| s.rescue_core.as_ref()) {
                    ui.label(
                        RichText::new(crate::i18n::tr("⛨ Rescue core"))
                            .color(crate::theme::text_color(ui, crate::theme::CPU))
                            .strong(),
                    )
                    .on_hover_text(crate::overview::rescue_text(rescue));
                }
                if !data.kill_list.is_empty()
                    && ui
                        .small_button(
                            RichText::new(crate::i18n::tr("🚨 Panic"))
                                .strong()
                                .color(crate::theme::text_color(ui, BAD)),
                        )
                        .on_hover_text(crate::i18n::tr(
                            "End everything on the kill list (Ctrl+Shift+K)",
                        ))
                        .clicked()
                {
                    state.panic = true;
                }
                if ui
                    .small_button(crate::i18n::tr("Run new task"))
                    .on_hover_text(crate::i18n::tr(
                        "Start a program, folder, document or website",
                    ))
                    .clicked()
                {
                    state.run_task = Some((String::new(), false));
                }
                if !data.elevated
                    && ui
                        .small_button(crate::i18n::tr("Restart as administrator"))
                        .on_hover_text(crate::i18n::tr(
                            "Control every process, including those of other users and services",
                        ))
                        .clicked()
                {
                    service_cmds.push(UiCommand::RestartElevated);
                }
                if let Some(s) = &data.snapshot {
                    let summary = {
                        let __wtm_i18n_0 = &(s.system.cpu_percent);
                        let __wtm_i18n_1 = &(format_bytes(s.system.memory_used() as f64));
                        let __wtm_i18n_2 = &(format_bytes(s.system.memory_total as f64));
                        crate::i18n::format(
                            "CPU {0}%   Memory {1} / {2}",
                            &[
                                format!("{:.0}", __wtm_i18n_0),
                                __wtm_i18n_1.to_string(),
                                __wtm_i18n_2.to_string(),
                            ],
                        )
                    };
                    ui.add(egui::Label::new(&summary).truncate())
                        .on_hover_text(summary);
                }
            });
            let (color, text) = match &data.message {
                Some((level, text)) => (level_color(ui, *level), crate::i18n::message(text)),
                None => (
                    ui.visuals().weak_text_color(),
                    crate::i18n::tr("Right-click a process for actions.").into(),
                ),
            };
            ui.add(egui::Label::new(RichText::new(text.as_ref()).color(color)).truncate())
                .on_hover_text(text.as_ref());
        });
        // Only the process table shows them; rebuilt once per data update.
        let labels_key = (data.revision, state.language);
        let (managed, labels_valid): (ManagedMap, bool) = match state.limit_labels.take() {
            Some((key, labels)) if key == labels_key => (labels, true),
            _ if state.tab == Tab::Processes => (limit_labels(data.status.as_ref()), true),
            _ => (ManagedMap::new(), false),
        };

        if state.tab == Tab::Processes
            && let (Some(sel), Some(snap)) = (state.processes.selected.clone(), &data.snapshot)
        {
            egui::Panel::right("details")
                .resizable(true)
                .default_size(330.0)
                .show(ui, |ui| {
                    if ui
                        .small_button(crate::i18n::tr("Close details"))
                        .on_hover_text("Esc")
                        .clicked()
                    {
                        state.processes.selected = None;
                    }
                    let mut area = egui::ScrollArea::vertical().id_salt("process_details");
                    if let Some(y) = crate::theme::take_scroll(ui, "process_details") {
                        area = area.vertical_scroll_offset(y);
                    }
                    let out = area.show(ui, |ui| {
                        details_panel(ui, &data, snap, &sel, &mut intents);
                    });
                    let fill = ui.visuals().panel_fill;
                    crate::theme::more_below(ui, "process_details", &out, fill);
                });
        }

        egui::CentralPanel::default().show(ui, |ui| {
            let now = Instant::now();
            page_header(ui, state.tab, &data, now);
            ui.add_space(12.0);
            // Values that stopped updating must not pass for live ones.
            if stale_age(&data, now).is_some() {
                ui.multiply_opacity(STALE_OPACITY);
            }
            if state.tab == Tab::Settings {
                crate::settings::page_bar(ui, &mut state.config.page);
            }
            let scroll_page = match state.tab {
                Tab::Settings => state.config.page.scrolls(),
                _ => matches!(
                    state.tab,
                    Tab::Overview
                        | Tab::Disks
                        | Tab::Sensors
                        | Tab::Governor
                        | Tab::About
                        | Tab::Limits
                        | Tab::KillList
                ),
            };
            let page_id = state.tab.label();
            let mut content = |ui: &mut Ui| match state.tab {
                Tab::Processes => match &data.snapshot {
                    Some(snap) => {
                        let extra = crate::processes::Extra {
                            topology: data.cpu_topology.as_ref(),
                            labels: &managed,
                            command_lines: &data.command_lines,
                            image_paths: &data.image_paths,
                            icons: &data.icons,
                            hung: &data.hung,
                        };
                        intents.extend(state.processes.ui_cached(ui, snap, &extra, data.revision));
                    }
                    None => crate::theme::loading(ui, crate::i18n::tr("Loading processes…")),
                },
                Tab::Performance => performance_focused(ui, &data, state.performance_focus.take()),
                Tab::Disks => crate::disks_tab::disks_tab(ui, &data, &mut state.disks),
                Tab::Sensors => {
                    service_cmds.extend(crate::sensors::sensors(ui, &data, &mut state.sensors))
                }
                Tab::Governor => governor(ui, &data),
                Tab::Events => events(ui, &data, &mut state.event_level),
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
                Tab::Users => {
                    if let Some(request) = views::users(ui, &data) {
                        state.confirm_session = Some(request);
                    }
                }
                Tab::Overview => {
                    let (cmds, more) = crate::overview::overview(ui, &data);
                    service_cmds.extend(cmds);
                    intents.extend(more);
                }
                Tab::About => {
                    if crate::about::about(ui, &data.update, frames.summary()) {
                        state.last_update_check = Some(Instant::now());
                        service_cmds.push(UiCommand::CheckUpdate);
                    }
                }
                Tab::Limits => {
                    service_cmds.extend(crate::limits_tab::limits_tab(
                        ui,
                        &data,
                        &mut state.limits,
                    ));
                }
                Tab::KillList => {
                    service_cmds.extend(crate::kill_tab::kill_tab(
                        ui,
                        &data,
                        &mut state.kill,
                        &mut state.panic,
                    ));
                }
                Tab::Settings => match state.config.page {
                    crate::settings::Page::Window => crate::theme::panel(ui, |ui| {
                        window_settings(ui, state);
                    }),
                    crate::settings::Page::Telegram => {
                        service_cmds.extend(crate::telegram_settings::telegram(
                            ui,
                            &data,
                            &mut state.telegram,
                        ));
                    }
                    crate::settings::Page::Service | crate::settings::Page::File => {
                        service_cmds.extend(crate::settings::settings(
                            ui,
                            &data,
                            &mut state.config,
                        ));
                    }
                },
                Tab::Services => {
                    match service_bar(ui, &data, state.selected_service.as_deref()) {
                        Some(UiCommand::Service { name, op }) if Change::needs_confirming(op) => {
                            let display = data
                                .services
                                .as_ref()
                                .and_then(|r| r.as_ref().ok())
                                .and_then(|list| list.iter().find(|s| s.name == name))
                                .map_or_else(|| name.clone(), |s| s.display_name.clone());
                            state.confirm_change = Some(Change::Service { name, display, op });
                        }
                        Some(cmd) => service_cmds.push(cmd),
                        None => {}
                    }
                    if let Some(s) = views::services(ui, &data, &mut state.services_filter) {
                        state.selected_service = Some(s.name);
                    }
                }
                Tab::Network => {
                    let only = state
                        .processes
                        .selected
                        .clone()
                        .filter(|r| state.network_all_for.as_ref() != Some(r));
                    match views::network(ui, &data, &mut state.network_filter, only.as_ref()) {
                        Some(views::NetworkAction::Close(local, remote)) => {
                            service_cmds.push(UiCommand::CloseConnection { local, remote });
                        }
                        Some(views::NetworkAction::Unblock(path)) => {
                            service_cmds.push(UiCommand::Firewall { path, block: false });
                        }
                        Some(views::NetworkAction::ShowAll) => {
                            state.network_all_for = state.processes.selected.clone();
                        }
                        None => {}
                    }
                }
                Tab::Ports => {
                    crate::ports_tab::ports_tab(ui, &data, &mut state.ports, &mut intents);
                }
            };
            if scroll_page {
                let mut area = egui::ScrollArea::vertical()
                    .id_salt(("page", page_id))
                    .auto_shrink([false, false]);
                if let Some(y) = crate::theme::take_scroll(ui, page_id) {
                    area = area.vertical_scroll_offset(y);
                }
                let out = area.show(ui, &mut content);
                crate::theme::more_below(ui, page_id, &out, ui.visuals().panel_fill);
            } else {
                content(ui);
            }
        });
        if labels_valid {
            state.limit_labels = Some((labels_key, managed));
        }

        if let Some((target, command)) = state.confirm.clone() {
            let modal = egui::Modal::new(egui::Id::unique("confirm")).show(&ctx, |ui| {
                ui.heading(describe(command));
                ui.label({
                    let __wtm_i18n_0 = &(describe(command));
                    let __wtm_i18n_1 = &(target.name);
                    let __wtm_i18n_2 = &(target.pid);
                    crate::i18n::format(
                        "{0} \"{1}\" (PID {2})? Unsaved data in that program will be lost.",
                        &[
                            __wtm_i18n_0.to_string(),
                            __wtm_i18n_1.to_string(),
                            format!("{}", __wtm_i18n_2),
                        ],
                    )
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(
                            RichText::new(describe(command))
                                .color(crate::theme::text_color(ui, BAD)),
                        )
                        .clicked()
                    {
                        intents.push(Intent::Command(target.clone(), command));
                        state.confirm = None;
                    }
                    if ui.button(crate::i18n::tr("Cancel")).clicked() {
                        state.confirm = None;
                    }
                });
            });
            if modal.should_close() {
                state.confirm = None;
            }
        }

        if let Some((id, user, op)) = state.confirm_session.clone() {
            use crate::backend::SessionOp;
            let (title, warning) = match op {
                SessionOp::Disconnect => (
                    crate::i18n::tr("Disconnect"),
                    crate::i18n::tr(
                        "Their programs keep running; they can sign in again to continue.",
                    ),
                ),
                SessionOp::SignOff => (
                    crate::i18n::tr("Sign off"),
                    crate::i18n::tr("Their programs close and unsaved work is lost."),
                ),
            };
            let modal = egui::Modal::new(egui::Id::unique("confirm_session")).show(&ctx, |ui| {
                ui.heading(title);
                ui.label({
                    crate::i18n::format(
                        "{0} {1} (session {2})? {3}",
                        &[
                            title.to_string(),
                            user.to_string(),
                            format!("{}", id),
                            warning.to_string(),
                        ],
                    )
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(RichText::new(title).color(crate::theme::text_color(ui, BAD)))
                        .clicked()
                    {
                        service_cmds.push(UiCommand::Session { id, op });
                        state.confirm_session = None;
                    }
                    if ui.button(crate::i18n::tr("Cancel")).clicked() {
                        state.confirm_session = None;
                    }
                });
            });
            if modal.should_close() {
                state.confirm_session = None;
            }
        }

        if ctx.input(|i| i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::K)) {
            state.panic = true;
        }
        crate::alert::dialog(
            &ctx,
            &data,
            &mut state.alerts,
            &mut state.panic,
            &mut service_cmds,
            &mut intents,
        );
        crate::kill_tab::panic_dialog(&ctx, &data, &mut state.panic, &mut intents);

        if let Some((line, elevated)) = &mut state.run_task {
            let mut run = false;
            let mut close = false;
            let modal = egui::Modal::new(egui::Id::unique("run_task")).show(&ctx, |ui| {
                ui.heading(crate::i18n::tr("Run new task"));
                ui.label(crate::i18n::tr(
                    "Type the name of a program, folder, document or website.",
                ));
                ui.add_space(4.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(line)
                        .hint_text(r"notepad, cmd, C:\Tools\app.exe --flag, https://…")
                        .desired_width(380.0),
                );
                // Ready to type as soon as the dialog opens.
                if ui.memory(|m| m.focused().is_none()) && !edit.lost_focus() {
                    edit.request_focus();
                }
                run |= edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.checkbox(
                    elevated,
                    crate::i18n::tr("Create this task with administrator rights"),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let ready = !line.trim().is_empty();
                    run |= ui
                        .add_enabled(ready, egui::Button::new(crate::i18n::tr("Run")))
                        .clicked();
                    close |= ui.button(crate::i18n::tr("Cancel")).clicked();
                });
            });
            if run && !line.trim().is_empty() {
                service_cmds.push(UiCommand::Run {
                    line: line.trim().to_owned(),
                    elevated: *elevated,
                });
                close = true;
            }
            if close || modal.should_close() {
                state.run_task = None;
            }
        }

        if let Some(change) = state.confirm_change.clone() {
            let (title, question) = change.describe();
            let modal = egui::Modal::new(egui::Id::unique("confirm_change")).show(&ctx, |ui| {
                ui.heading(&title);
                ui.label(question);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(RichText::new(&title).color(crate::theme::text_color(ui, BAD)))
                        .clicked()
                    {
                        service_cmds.push(change.command());
                        state.confirm_change = None;
                    }
                    if ui.button(crate::i18n::tr("Cancel")).clicked() {
                        state.confirm_change = None;
                    }
                });
            });
            if modal.should_close() {
                state.confirm_change = None;
            }
        }

        if let Some((targets, command)) = state.confirm_many.clone() {
            let name = targets.first().map_or("", |t| t.name.as_str()).to_owned();
            // One app's instances, or different programs (e.g. on one port).
            let same = targets.iter().all(|t| t.name == name);
            let modal = egui::Modal::new(egui::Id::unique("confirm_many")).show(&ctx, |ui| {
                if same {
                    ui.heading(format!("{} — {name}", describe(command)));
                    ui.label({ let __wtm_i18n_0 = &(describe(command)); let __wtm_i18n_1 = &(targets.len()); crate::i18n::format("{0} all {1} processes of \"{2}\"? Unsaved data in that program will be lost.", &[__wtm_i18n_0.to_string(), format!("{}", __wtm_i18n_1), name.to_string()]) });
                } else {
                    let list: Vec<String> =
                        targets.iter().map(|t| format!("{} ({})", t.name, t.pid)).collect();
                    ui.heading(describe(command));
                    ui.label({ let __wtm_i18n_0 = &(describe(command)); let __wtm_i18n_1 = &(targets.len()); let __wtm_i18n_2 = &(list.join(", ")); crate::i18n::format("{0} these {1} processes: {2}? Unsaved data in them will be lost.", &[__wtm_i18n_0.to_string(), format!("{}", __wtm_i18n_1), __wtm_i18n_2.to_string()]) });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(RichText::new({ let __wtm_i18n_0 = &(describe(command)); crate::i18n::format("{0} all", std::slice::from_ref(__wtm_i18n_0)) }).color(crate::theme::text_color(ui, BAD)))
                        .clicked()
                    {
                        intents.extend(targets.iter().map(|t| Intent::Command(t.clone(), command)));
                        state.confirm_many = None;
                    }
                    if ui.button(crate::i18n::tr("Cancel")).clicked() {
                        state.confirm_many = None;
                    }
                });
            });
            if modal.should_close() {
                state.confirm_many = None;
            }
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
            Tab::Sensors => Some((Panel::Sensors, Duration::from_secs(2))),
            Tab::Users => Some((Panel::Users, Duration::from_secs(5))),
            Tab::Network | Tab::Ports => Some((Panel::Network, Duration::from_secs(2))),
            Tab::Disks => Some((Panel::DiskActivity, Duration::from_secs(2))),
            Tab::Services => Some((Panel::Services, Duration::from_secs(5))),
            Tab::History => None,
            Tab::Startup => Some((Panel::Startup, Duration::from_secs(10))),
            Tab::Settings | Tab::Limits | Tab::Overview | Tab::KillList => {
                Some((Panel::Config, Duration::from_secs(30)))
            }
            // The panic button needs the kill list on every tab.
            _ if service_mode => Some((Panel::Config, Duration::from_secs(30))),
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
        // The details panel lists the selected process's ports.
        if state.tab == Tab::Processes
            && state.processes.selected.is_some()
            && state
                .last_load
                .get(&Panel::Network)
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(5))
        {
            state.last_load.insert(Panel::Network, Instant::now());
            backend.send(UiCommand::Load(Panel::Network));
        }
        for intent in intents {
            match intent {
                Intent::ShowResource(resource) => {
                    state.tab = Tab::Performance;
                    state.performance_focus = Some(resource);
                }
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
                    state.processes.reveal(target);
                    state.tab = Tab::Processes;
                }
                Intent::AddToKillList(name) => {
                    state.kill.prefill(&name, "");
                    state.tab = Tab::KillList;
                }
                Intent::LimitApp(name) => {
                    state.limits.prefill(&name);
                    state.tab = Tab::Limits;
                }
                Intent::ShowPorts => state.tab = Tab::Ports,
                Intent::Dump(target) => backend.send(UiCommand::CreateDump(target)),
                Intent::WatchExit(target) => backend.send(UiCommand::WatchExit(target)),
                Intent::WaitChain(target) => backend.send(UiCommand::WaitChain(target)),
                Intent::WhyAlive(target) => backend.send(UiCommand::WhyAlive(target)),
                Intent::Modules(target) => backend.send(UiCommand::Modules(target)),
                Intent::Network(target, block) => {
                    let path = backend
                        .data()
                        .image_paths
                        .get(&(target.pid, target.create_time))
                        .cloned()
                        .unwrap_or_default();
                    if path.is_empty() {
                        backend.data().message = Some((
                            EventLevel::Error,
                            crate::i18n::format(
                                "The program file of {0} is unknown, so it cannot be blocked",
                                std::slice::from_ref(&target.name),
                            ),
                        ));
                    } else if block {
                        state.confirm_change = Some(Change::Block {
                            path,
                            name: target.name,
                        });
                    } else {
                        backend.send(UiCommand::Firewall { path, block });
                    }
                }
            }
        }
        let pages: Vec<&str> = NAV.iter().map(|n| n.2).collect();
        if frames.record(frame_start, data_ready, &pages) {
            // Report mode is a benchmark run; nothing to save.
            std::process::exit(0);
        }
    }
}

/// How often RustWTM asks GitHub for a newer release.
const UPDATE_CHECK_EVERY: Duration = Duration::from_secs(12 * 3600);

/// A bar above everything when a newer release is out (or being installed).
fn update_bar(ui: &mut Ui, data: &Data, state: &mut UiState, commands: &mut Vec<UiCommand>) {
    use crate::backend::UpdateState;
    let release = match &data.update {
        UpdateState::Available(r)
            if !state.update_later && state.skip_version.as_deref() != Some(&r.tag) =>
        {
            r
        }
        UpdateState::Installing(r) => r,
        // Only the failure of an install the user started is worth a bar.
        UpdateState::Failed(e) if state.update_started => {
            egui::Panel::top("update_bar").show(ui, |ui| {
                ui.horizontal(|ui| {
                    crate::theme::colored(ui, BAD, {
                        crate::i18n::format("The update failed: {0}", std::slice::from_ref(e))
                    });
                    ui.hyperlink_to(
                        crate::i18n::tr("Release page"),
                        wtm_core::update::RELEASES_PAGE,
                    );
                    if ui.button(crate::i18n::tr("Dismiss")).clicked() {
                        state.update_started = false;
                    }
                });
            });
            return;
        }
        _ => return,
    };
    egui::Panel::top("update_bar").show(ui, |ui| {
        ui.horizontal(|ui| {
            if matches!(data.update, UpdateState::Installing(_)) {
                ui.spinner();
                ui.label({ let __wtm_i18n_0 = &(release.version_text()); crate::i18n::format("Downloading and verifying RustWTM {0}; the installer starts next and RustWTM restarts by itself.", std::slice::from_ref(__wtm_i18n_0)) });
                return;
            }
            ui.label(
                RichText::new({ let __wtm_i18n_0 = &(release.version_text()); let __wtm_i18n_1 = &(env!("CARGO_PKG_VERSION")); crate::i18n::format("⬆ RustWTM {0} is available (you have {1}).", &[__wtm_i18n_0.to_string(), __wtm_i18n_1.to_string()]) })
                .strong()
                .color(crate::theme::text_color(ui, GOOD)),
            );
            if data.installable && release.msi.is_some() {
                if ui
                    .button(RichText::new(crate::i18n::tr("Update now")).strong())
                    .on_hover_text(
                        crate::i18n::tr("Downloads the installer, checks it against its published SHA-256, \
                         installs it (Windows asks for administrator rights) and restarts RustWTM"),
                    )
                    .clicked()
                {
                    commands.push(UiCommand::InstallUpdate(release.clone()));
                    state.update_started = true;
                }
            } else {
                ui.hyperlink_to(crate::i18n::tr("Download"), &release.page)
                    .on_hover_text(crate::i18n::tr("This copy was not installed with the installer"));
            }
            ui.hyperlink_to(crate::i18n::tr("What's new"), &release.page);
            if ui.button(crate::i18n::tr("Later")).clicked() {
                state.update_later = true;
            }
            if ui.button(crate::i18n::tr("Skip this version")).clicked() {
                state.skip_version = Some(release.tag.clone());
            }
        });
    });
}

fn language_settings(
    ui: &mut Ui,
    state: &mut UiState,
    save: impl Fn(crate::i18n::Language) -> std::io::Result<()>,
) {
    ui.horizontal_wrapped(|ui| {
        let label = ui.label(crate::i18n::tr("Language"));
        let previous = state.language;
        egui::ComboBox::from_id_salt("language")
            .selected_text(state.language.name())
            .show_ui(ui, |ui| {
                for language in crate::i18n::Language::ALL {
                    ui.selectable_value(&mut state.language, language, language.name());
                }
            })
            .response
            .labelled_by(label.id);
        if previous != state.language {
            match save(state.language) {
                Ok(()) => {
                    state.language_error = None;
                    ui.ctx().request_repaint();
                }
                Err(e) => {
                    state.language = previous;
                    state.language_error = Some(e.to_string());
                }
            }
        }
    });
    if let Some(error) = &state.language_error {
        crate::theme::colored(
            ui,
            BAD,
            crate::i18n::format("Cannot save language: {0}", std::slice::from_ref(error)),
        );
    }
}

/// Preferences of this window (the service's settings follow below them).
fn window_settings(ui: &mut Ui, state: &mut UiState) {
    language_settings(ui, state, crate::i18n::save);
    ui.horizontal(|ui| {
        ui.label(crate::i18n::tr("Theme"));
        let mut preference = ui.ctx().options(|o| o.theme_preference);
        for (value, label) in [
            (egui::ThemePreference::System, crate::i18n::tr("System")),
            (egui::ThemePreference::Light, crate::i18n::tr("Light")),
            (egui::ThemePreference::Dark, crate::i18n::tr("Dark")),
        ] {
            if ui.selectable_value(&mut preference, value, label).changed() {
                ui.ctx().set_theme(preference);
            }
        }
    });
    let a = &mut state.alerts;
    ui.horizontal(|ui| {
        ui.checkbox(
            &mut a.enabled,
            crate::i18n::tr("Pop up when the CPU stays above"),
        );
        ui.add_enabled(
            a.enabled,
            egui::DragValue::new(&mut a.threshold)
                .range(50.0..=100.0)
                .suffix("%"),
        );
        ui.label(crate::i18n::tr("for"));
        ui.add_enabled(
            a.enabled,
            egui::DragValue::new(&mut a.seconds)
                .range(1..=120)
                .suffix(" s"),
        );
    })
    .response
    .on_hover_text(crate::i18n::tr(
        "Shows the busiest apps with one-click fixes before the PC becomes unusable",
    ));
    ui.checkbox(
        &mut state.tray_on_close,
        crate::i18n::tr("Keep running in the tray when the window is closed"),
    );
    ui.checkbox(
        &mut state.check_updates,
        crate::i18n::tr("Check for updates automatically"),
    )
    .on_hover_text(crate::i18n::tr(
        "Asks GitHub for a newer release at start and twice a day",
    ));
    let mut on = *state
        .autostart
        .get_or_insert_with(crate::backend::autostart_enabled);
    if ui
        .checkbox(
            &mut on,
            crate::i18n::tr("Start RustWTM in the tray when I sign in"),
        )
        .changed()
        && crate::backend::set_autostart(on).is_ok()
    {
        state.autostart = Some(on);
    }
    ui.add_space(6.0);
}

/// How much page content fades while its data is out of date.
const STALE_OPACITY: f32 = 0.55;

/// `Some` while the shown data is out of date, with its age when known.
fn stale_age(data: &Data, now: Instant) -> Option<Option<Duration>> {
    match data.sample_freshness(now) {
        crate::backend::SampleFreshness::Stale(age) => Some(age),
        _ => None,
    }
}

/// The page title and subtitle with the age of the data on the right, and
/// a warning below them while the data is out of date. Short windows leave
/// out the subtitle and show the age next to the title.
fn page_header(ui: &mut Ui, tab: Tab, data: &Data, now: Instant) {
    let compact = ui.ctx().content_rect().height() < COMPACT_HEIGHT;
    ui.horizontal(|ui| {
        let title = ui.heading(RichText::new(tab.label()).size(25.0));
        if compact {
            title.on_hover_text(tab.subtitle());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                data_age(ui, data, now);
            });
        }
    });
    if !compact {
        ui.horizontal(|ui| {
            ui.label(RichText::new(tab.subtitle()).weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                data_age(ui, data, now);
            });
        });
    }
    stale_banner(ui, data, now);
}

/// How old the shown data is: secondary text, a warning once out of date.
fn data_age(ui: &mut Ui, data: &Data, now: Instant) {
    use crate::backend::SampleFreshness;
    let (age, stale) = match data.sample_freshness(now) {
        SampleFreshness::Waiting => {
            ui.weak(crate::i18n::tr("Waiting for monitoring data…"));
            return;
        }
        SampleFreshness::Current(age) => (age, false),
        SampleFreshness::Stale(Some(age)) => (age, true),
        SampleFreshness::Stale(None) => return,
    };
    let text = crate::i18n::format("Data measured {0}s ago", &[age.as_secs().to_string()]);
    if stale {
        crate::theme::colored(ui, WARN, text);
    } else {
        ui.weak(text);
    }
}

/// A full-width warning while the data is out of date, with the reason.
fn stale_banner(ui: &mut Ui, data: &Data, now: Instant) {
    if stale_age(data, now).is_none() {
        return;
    }
    ui.add_space(6.0);
    crate::theme::strip(ui, WARN, |ui| {
        ui.horizontal_wrapped(|ui| {
            let color = crate::theme::text_color(ui, WARN);
            ui.label(RichText::new("⚠").size(16.0).color(color));
            ui.label(
                RichText::new(crate::i18n::tr("Monitoring data is out of date"))
                    .size(16.0)
                    .color(color),
            )
            .on_hover_text(crate::i18n::tr("Displayed values remain from the last received sample. They do not indicate current activity."));
        });
        if let Some(error) = &data.sample_error {
            ui.label(crate::i18n::message(error).as_ref());
        }
    });
}

/// Keys that work on every page: Ctrl+Tab and Ctrl+Shift+Tab go through
/// the pages, Esc closes the process details.
fn shortcuts(ctx: &egui::Context, state: &mut UiState) {
    let blocked = egui::Popup::is_any_open(ctx) || ctx.memory(|m| m.top_modal_layer().is_some());
    if blocked {
        return;
    }
    let step = ctx.input_mut(|i| {
        if i.consume_key(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::Tab,
        ) {
            Some(NAV.len() - 1)
        } else if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Tab) {
            Some(1)
        } else {
            None
        }
    });
    if let Some(step) = step
        && let Some(i) = NAV.iter().position(|n| n.0 == state.tab)
    {
        state.tab = NAV[(i + step) % NAV.len()].0;
    }
    if state.tab == Tab::Processes
        && state.processes.selected.is_some()
        && !ctx.text_edit_focused()
        && ctx.input(|i| i.key_pressed(egui::Key::Escape))
    {
        state.processes.selected = None;
    }
}

/// The connection in the status bar; a warning while no data arrives.
/// `problem`: why there is no service connection, for `Mode::Local`.
fn mode_badge(
    ui: &mut Ui,
    mode: Mode,
    problem: crate::backend::ServiceProblem,
    stale: Option<Option<Duration>>,
) {
    use crate::backend::ServiceProblem;
    let (text, color, hover) = match mode {
        Mode::Starting => {
            ui.weak(crate::i18n::tr("○ Starting"))
                .on_hover_text(crate::i18n::tr("Collecting data…"));
            return;
        }
        Mode::Service {
            verified: false, ..
        } => (
            crate::i18n::tr("● Console instance"),
            WARN,
            crate::i18n::tr(
                "Connected to a RustWTM engine that is not the installed service (e.g. `wtm-service console`).",
            ),
        ),
        Mode::Service { admin: true, .. } => (
            crate::i18n::tr("● Service"),
            GOOD,
            crate::i18n::tr("Connected to the RustWTM service with full control."),
        ),
        Mode::Service { admin: false, .. } => (
            crate::i18n::tr("● Service (read-only)"),
            WARN,
            crate::i18n::tr(
                "Connected to the service. Run RustWTM as administrator to send commands through it; until then actions use your own rights.",
            ),
        ),
        Mode::Local => (
            crate::i18n::tr("○ Local"),
            WARN,
            match problem {
                ServiceProblem::NotRunning => crate::i18n::tr(
                    "The RustWTM service is not running. Monitoring and actions use this window's own rights.",
                ),
                ServiceProblem::Older => crate::i18n::tr(
                    "The RustWTM service is an older version this window cannot talk to; update it. Monitoring and actions use this window's own rights.",
                ),
                ServiceProblem::Newer => crate::i18n::tr(
                    "The RustWTM service is newer than this window; restart RustWTM to use it. Until then monitoring and actions use this window's own rights.",
                ),
                ServiceProblem::Impostor => crate::i18n::tr(
                    "Another program answers in place of the RustWTM service, so it is not used. Monitoring and actions use this window's own rights.",
                ),
            },
        ),
    };
    let (text, color) = match stale {
        None => (text.to_owned(), crate::theme::text_color(ui, color)),
        Some(age) => (
            match age {
                Some(age) => crate::i18n::format(
                    "{0} · no data for {1} s",
                    &[text.to_owned(), age.as_secs().to_string()],
                ),
                None => crate::i18n::format("{0} · no data", &[text.to_owned()]),
            },
            crate::theme::text_color(ui, WARN),
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

/// Why the program does not respond: its hung window and, on request, what
/// its threads wait for.
fn diagnostics(ui: &mut Ui, data: &Data, sel: &ProcessRef, intents: &mut Vec<Intent>) {
    ui.add_space(8.0);
    ui.strong(crate::i18n::tr("Diagnostics"));
    if let Some(title) = data.hung.get(&sel.pid) {
        crate::theme::colored(
            ui,
            BAD,
            crate::i18n::format("Not responding: \"{0}\"", std::slice::from_ref(title)),
        );
    }
    let report = data.wait_report.as_ref().filter(|(r, _)| r == sel);
    let running = report.is_some_and(|(_, r)| r.is_none());
    let ending = data.end_report.as_ref().filter(|(r, _)| r == sel);
    let checking = ending.is_some_and(|(_, r)| r.is_none());
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                !running,
                egui::Button::new(crate::i18n::tr("Analyze wait chain")),
            )
            .on_hover_text(crate::i18n::tr(
                "What the program's blocked threads are waiting for, across processes",
            ))
            .clicked()
        {
            intents.push(Intent::WaitChain(sel.clone()));
        }
        if ui
            .add_enabled(
                !checking,
                egui::Button::new(crate::i18n::tr("Why won't it end?")),
            )
            .on_hover_text(crate::i18n::tr(
                "Protection, drivers that block ending it, and who still holds it open",
            ))
            .clicked()
        {
            intents.push(Intent::WhyAlive(sel.clone()));
        }
        let modules = data.modules.as_ref().filter(|(r, _)| r == sel);
        if ui
            .add_enabled(
                !modules.is_some_and(|(_, m)| m.is_none()),
                egui::Button::new(crate::i18n::tr("Modules")),
            )
            .on_hover_text(crate::i18n::tr(
                "The DLLs it has loaded and who signed each one",
            ))
            .clicked()
        {
            intents.push(Intent::Modules(sel.clone()));
        }
    });
    match data
        .modules
        .as_ref()
        .filter(|(r, _)| r == sel)
        .map(|(_, m)| m)
    {
        Some(None) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(crate::i18n::tr("Reading modules and checking signatures…"));
            });
        }
        Some(Some(Err(e))) => {
            crate::theme::colored(ui, BAD, e);
        }
        Some(Some(Ok(list))) => modules_list(ui, list),
        None => {}
    }
    match ending.map(|(_, r)| r) {
        Some(None) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(crate::i18n::tr("Checking…"));
            });
        }
        Some(Some(Err(e))) => {
            crate::theme::colored(ui, BAD, e);
        }
        Some(Some(Ok(r))) => end_report(ui, r, data.elevated),
        None => {}
    }
    match report.map(|(_, r)| r) {
        Some(None) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(crate::i18n::tr("Analyzing…"));
            });
        }
        Some(Some(Err(e))) => {
            crate::theme::colored(ui, BAD, e);
        }
        Some(Some(Ok(r))) => wait_report(ui, r),
        None => {}
    }
}

/// Unsigned and invalid modules first: they are the ones worth a look.
fn modules_list(ui: &mut Ui, list: &[(wtm_core::Module, wtm_core::Signature)]) {
    use wtm_core::Signature;
    let rank = |s: &Signature| match s {
        Signature::Invalid(_) => 0,
        Signature::Unsigned => 1,
        Signature::Signed { .. } => 2,
    };
    let mut sorted: Vec<_> = list.iter().collect();
    sorted.sort_by_key(|(m, s)| (rank(s), m.name.to_lowercase()));
    let unsigned = list.iter().filter(|(_, s)| rank(s) == 1).count();
    let invalid = list.iter().filter(|(_, s)| rank(s) == 0).count();
    ui.label(crate::i18n::format(
        "{0} modules: {1} unsigned, {2} with an invalid signature",
        &[
            list.len().to_string(),
            unsigned.to_string(),
            invalid.to_string(),
        ],
    ));
    egui::ScrollArea::vertical()
        .id_salt("modules_list")
        .max_height(260.0)
        .show(ui, |ui| {
            for (m, s) in sorted {
                let (color, state) = match s {
                    Signature::Invalid(e) => (
                        crate::theme::text_color(ui, BAD),
                        crate::i18n::format("invalid signature: {0}", std::slice::from_ref(e)),
                    ),
                    Signature::Unsigned => (
                        crate::theme::text_color(ui, WARN),
                        crate::i18n::tr("unsigned").to_owned(),
                    ),
                    Signature::Signed { signer, .. } => (
                        ui.visuals().weak_text_color(),
                        signer
                            .clone()
                            .unwrap_or_else(|| crate::i18n::tr("signed").to_owned()),
                    ),
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&m.name).small().strong())
                        .on_hover_text(&m.path);
                    ui.label(RichText::new(state).small().color(color));
                });
            }
        });
}

/// The reasons a process stays, most decisive first.
pub(crate) fn end_reasons(r: &wtm_core::EndReport, elevated: bool) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    if r.deleting || r.exited {
        let holders: Vec<String> = r
            .holders
            .iter()
            .take(5)
            .map(|h| {
                crate::i18n::format(
                    "{0} (PID {1}, {2} handles)",
                    &[h.name.clone(), h.pid.to_string(), h.handles.to_string()],
                )
            })
            .collect();
        out.push((
            true,
            if holders.is_empty() {
                crate::i18n::format(
                    "Windows is already ending it ({0} threads left) and cannot remove it yet.",
                    &[r.threads.to_string()],
                )
            } else {
                crate::i18n::format(
                    "Windows is already ending it, but these keep it open: {0}. Closing them lets it go.",
                    &[holders.join(", ")],
                )
            },
        ));
    }
    if r.critical {
        out.push((
            true,
            crate::i18n::tr("It is a critical system process: ending it would stop Windows.")
                .into(),
        ));
    }
    if let Some(p) = &r.protection {
        out.push((
            true,
            crate::i18n::format(
                "Windows protects it ({0}); no program may end it.",
                std::slice::from_ref(p),
            ),
        ));
    }
    if r.can_end == Some(false) {
        let guards: Vec<String> = r
            .guards
            .iter()
            .map(|(_, product)| product.clone())
            .collect();
        out.push((
            true,
            if guards.is_empty() {
                crate::i18n::tr(
                    "The right to end it is withheld, usually by a security or anti-cheat driver.",
                )
                .into()
            } else {
                crate::i18n::format(
                    "The right to end it is withheld; loaded drivers known to do this: {0}.",
                    &[guards.join(", ")],
                )
            },
        ));
    }
    if let Some(e) = &r.open_error {
        out.push((
            true,
            if elevated {
                crate::i18n::format("It cannot be opened to end it: {0}", std::slice::from_ref(e))
            } else {
                crate::i18n::format(
                    "It cannot be opened to end it: {0}. Restart RustWTM as administrator and try again.",
                    std::slice::from_ref(e),
                )
            },
        ));
    }
    if out.is_empty() {
        out.push((
            false,
            crate::i18n::tr("Nothing stops it: it can be ended normally.").into(),
        ));
    }
    if !r.holders_known && !elevated {
        out.push((
            false,
            crate::i18n::tr("Run as administrator to also see which programs hold it open.").into(),
        ));
    }
    out
}

fn end_report(ui: &mut Ui, r: &wtm_core::EndReport, elevated: bool) {
    for (problem, text) in end_reasons(r, elevated) {
        if problem {
            crate::theme::colored(ui, BAD, text);
        } else {
            ui.label(text);
        }
    }
}

fn wait_report(ui: &mut Ui, r: &wtm_core::WaitReport) {
    if r.deadlocked() {
        crate::theme::colored(
            ui,
            BAD,
            crate::i18n::tr("Deadlock: threads wait for each other and will never continue."),
        );
    }
    ui.weak(crate::i18n::format(
        "{0} threads, {1} blocked, {2} with a wait chain",
        &[
            r.threads.to_string(),
            r.analyzed.to_string(),
            r.chains.len().to_string(),
        ],
    ));
    if r.skipped > 0 {
        ui.weak(crate::i18n::format(
            "{0} threads could not be analyzed in time",
            &[r.skipped.to_string()],
        ));
    }
    for chain in r.chains.iter().take(12) {
        let text: Vec<String> = chain.links.iter().map(link_text).collect();
        let mut line = RichText::new(text.join("  →  ")).small().monospace();
        if chain.cycle {
            line = line.color(crate::theme::text_color(ui, BAD));
        }
        ui.label(line);
    }
}

fn link_text(link: &wtm_core::Link) -> String {
    use wtm_core::{Link, LinkStatus, WaitKind};
    match link {
        Link::Thread { pid, tid, status } => {
            let state = match status {
                LinkStatus::Blocked => crate::i18n::tr("blocked"),
                LinkStatus::Running => crate::i18n::tr("running"),
                LinkStatus::NoAccess => crate::i18n::tr("no access"),
                _ => "",
            };
            crate::i18n::format(
                "thread {0} (PID {1}) {2}",
                &[tid.to_string(), pid.to_string(), state.to_owned()],
            )
            .trim_end()
            .to_owned()
        }
        Link::Object { kind, name, .. } => {
            let kind = match kind {
                WaitKind::CriticalSection => crate::i18n::tr("critical section"),
                WaitKind::SendMessage => crate::i18n::tr("window message"),
                WaitKind::Mutex => crate::i18n::tr("mutex"),
                WaitKind::Alpc => "ALPC",
                WaitKind::Com => "COM",
                WaitKind::Thread => crate::i18n::tr("thread exit"),
                WaitKind::Process => crate::i18n::tr("process exit"),
                WaitKind::ComActivation => crate::i18n::tr("COM activation"),
                WaitKind::Socket => crate::i18n::tr("network socket"),
                WaitKind::Smb => crate::i18n::tr("network share (SMB)"),
                WaitKind::Unknown => crate::i18n::tr("unknown"),
            };
            if name.is_empty() {
                kind.to_owned()
            } else {
                format!("{kind} \"{name}\"")
            }
        }
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
        crate::theme::colored(ui, WARN, crate::i18n::tr("This process has exited."));
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
            row(crate::i18n::tr("Parent PID"), p.parent_pid.to_string());
            row(crate::i18n::tr("Session"), p.session_id.to_string());
            row(crate::i18n::tr("Uptime"), uptime(p.start_time()));
            row("CPU", format!("{:.2}%", p.cpu_percent));
            row(
                crate::i18n::tr("CPU time"),
                format!("{:.1} s", p.cpu_time.as_secs_f64()),
            );
            row(
                crate::i18n::tr("Private working set"),
                format_bytes(p.private_working_set as f64),
            );
            row(
                crate::i18n::tr("Working set"),
                format_bytes(p.working_set as f64),
            );
            row(
                crate::i18n::tr("Commit"),
                format_bytes(p.private_bytes as f64),
            );
            row(
                crate::i18n::tr("Disk read"),
                format!("{}/s", format_bytes(p.io_read_rate)),
            );
            row(
                crate::i18n::tr("Disk write"),
                format!("{}/s", format_bytes(p.io_write_rate)),
            );
            row(
                crate::i18n::tr("Network receive"),
                format!("{}/s", format_bytes(p.net_rx_rate)),
            );
            row(
                crate::i18n::tr("Network send"),
                format!("{}/s", format_bytes(p.net_tx_rate)),
            );
            row(crate::i18n::tr("Threads"), p.thread_count.to_string());
            row(crate::i18n::tr("Handles"), p.handle_count.to_string());

            match &data.details {
                Some((r, Ok(d))) if r == sel => {
                    let na = || "n/a".to_owned();
                    row(
                        crate::i18n::tr("Priority"),
                        d.priority
                            .map_or_else(na, |p| crate::processes::priority_label(p).into()),
                    );
                    row(
                        crate::i18n::tr("Efficiency"),
                        d.eco_qos
                            .map_or_else(na, |e| crate::processes::eco_label(e).into()),
                    );
                    row(
                        crate::i18n::tr("Affinity"),
                        d.affinity.map_or_else(na, |(m, s)| {
                            let __wtm_i18n_0 = &(m.count_ones());
                            let __wtm_i18n_1 = &(s.count_ones());
                            crate::i18n::format(
                                "{0} of {1} CPUs",
                                &[format!("{}", __wtm_i18n_0), format!("{}", __wtm_i18n_1)],
                            )
                        }),
                    );
                    row(
                        crate::i18n::tr("Critical"),
                        d.critical.map_or_else(na, |c| {
                            crate::i18n::tr(if c { "yes" } else { "no" }).into()
                        }),
                    );
                    row(
                        crate::i18n::tr("Protected"),
                        d.protected
                            .clone()
                            .unwrap_or_else(|| crate::i18n::tr("no").into()),
                    );
                }
                Some((r, Err(e))) if r == sel => row(crate::i18n::tr("Details"), e.clone()),
                _ => row(
                    crate::i18n::tr("Details"),
                    crate::i18n::tr("loading…").into(),
                ),
            }
        });

    if let Some((r, Ok(d))) = &data.details
        && r == sel
    {
        if let Some(user) = &d.user {
            ui.add_space(6.0);
            ui.label(crate::i18n::format("User: {0}", std::slice::from_ref(user)));
        }
        if let Some(path) = &d.path {
            ui.add_space(6.0);
            ui.label(RichText::new(path).small().weak());
        }
        if let Some(cmd) = d.command_line.as_deref().filter(|c| !c.is_empty()) {
            ui.add_space(4.0);
            ui.label(RichText::new(cmd).small().monospace());
            if ui
                .small_button(crate::i18n::tr("Copy command line"))
                .clicked()
            {
                ui.ctx().copy_text(cmd.to_owned());
            }
        }
    }

    diagnostics(ui, data, sel, intents);

    if let Some(Ok(conns)) = &data.connections {
        let uses: Vec<_> = wtm_core::ports::port_uses(conns)
            .into_iter()
            .filter(|u| u.pid == p.pid)
            .collect();
        if !uses.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(crate::i18n::tr("Listening on")).strong());
            for u in &uses {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} {}",
                            format!("{:?}", u.protocol).to_uppercase(),
                            u.port
                        ))
                        .monospace(),
                    );
                    if let Some(k) = u.known() {
                        ui.weak(k.service);
                    }
                    ui.weak(crate::i18n::tr(u.exposure().label()));
                });
            }
            if ui.small_button(crate::i18n::tr("Show in Ports")).clicked() {
                intents.push(Intent::ShowPorts);
            }
        }
    }

    if let Some(h) = data.process_history.get(&(p.pid, p.create_time))
        && h.cpu.len() > 1
    {
        ui.add_space(8.0);
        ui.label(RichText::new(crate::i18n::tr("Last two minutes")).strong());
        process_plot(ui, "proc_cpu", "CPU", &h.times, &h.cpu, false);
        process_plot(
            ui,
            "proc_mem",
            crate::i18n::tr("Memory"),
            &h.times,
            &h.memory,
            true,
        );
    }

    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        if ui
            .button(
                RichText::new(crate::i18n::tr("End task")).color(crate::theme::text_color(ui, BAD)),
            )
            .clicked()
        {
            intents.push(Intent::Confirm(sel.clone(), Command::Kill));
        }
        ui.menu_button(crate::i18n::tr("More actions"), |ui| {
            context_menu(
                ui,
                sel,
                snap.system.logical_cpus,
                data.cpu_topology.as_ref(),
                intents,
            )
        });
        if ui.button(crate::i18n::tr("Open file location")).clicked() {
            intents.push(Intent::OpenLocation(sel.clone()));
        }
        if ui.button(crate::i18n::tr("Refresh")).clicked() {
            intents.push(Intent::Select(sel.clone()));
        }
    });
}

/// A small graph in the details panel: CPU in percent or memory in bytes.
fn process_plot(
    ui: &mut Ui,
    id: &str,
    label: &str,
    times: &std::collections::VecDeque<u64>,
    values: &std::collections::VecDeque<f32>,
    bytes: bool,
) {
    let points = series_points(times, values);
    let unit = move |v: f64| {
        if bytes {
            format_bytes(v.max(0.0))
        } else {
            format!("{v:.0}%")
        }
    };
    let latest = values.back().map_or(0.0, |v| f64::from(*v));
    Plot::new(id)
        .height(80.0)
        .include_y(0.0)
        .include_y(if bytes { 1024.0 * 1024.0 } else { 5.0 })
        .include_x(-(crate::backend::PROCESS_HISTORY_LEN as f64))
        .include_x(0.0)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .show_x(false)
        .show_axes([false, true])
        .y_axis_formatter(move |mark, _| unit(mark.value))
        .legend(egui_plot::Legend::default())
        .show(ui, |plot| {
            plot.line(Line::new(format!("{label} {}", unit(latest)), points));
        });
}

/// Height of the large graphs on the Performance tab.
const PLOT_HEIGHT: f32 = 120.0;

/// `values` at the times they were measured (Unix milliseconds), in seconds
/// before the newest, so a slower sampling interval or a delayed sample
/// keeps its real place on the time axis. The newest entries of both pair up.
fn series_points(
    times: &std::collections::VecDeque<u64>,
    values: &std::collections::VecDeque<f32>,
) -> PlotPoints<'static> {
    let n = times.len().min(values.len());
    let newest = times.back().copied().unwrap_or_default();
    times
        .iter()
        .skip(times.len() - n)
        .zip(values.iter().skip(values.len() - n))
        .map(|(t, v)| {
            let before = newest.saturating_sub(*t) as f64 / 1000.0;
            [-before, f64::from(*v)]
        })
        .collect()
}

/// The common look of the Performance graphs: no interaction, light grid,
/// time axis without labels (the last five minutes).
fn perf_plot(id: &str) -> Plot<'static> {
    Plot::new(id)
        .height(PLOT_HEIGHT)
        .include_x(-(HISTORY_LEN as f64))
        .include_x(0.0)
        .include_y(0.0)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .show_x(false)
        .x_axis_formatter(|mark, _| format!("{:.0}s", mark.value))
        // One width for every graph, so that graphs above each other line up.
        .y_axis_min_width(64.0)
}

/// A 0..=100% history as a filled area.
fn history_plot(
    ui: &mut Ui,
    id: &str,
    label: &str,
    times: &std::collections::VecDeque<u64>,
    values: &std::collections::VecDeque<f32>,
    color: Color32,
) {
    perf_plot(id)
        .include_y(100.0)
        .y_axis_formatter(|mark, _| format!("{:.0}%", mark.value))
        .label_formatter(|hover| match hover {
            egui_plot::HoverPosition::NearDataPoint {
                plot_name,
                position,
                ..
            } => Some(format!("{plot_name}: {:.0}%", position.y)),
            egui_plot::HoverPosition::Elsewhere { .. } => None,
        })
        .show(ui, |plot| {
            plot.line(
                Line::new(label.to_owned(), series_points(times, values))
                    .color(color)
                    .width(2.0)
                    .fill(0.0)
                    .fill_alpha(0.25),
            );
        });
}

/// Two byte rates (for example download and upload) on an auto-scaled axis.
/// The card title carries the legend (see [`rate_legend`]).
fn rate_plot(
    ui: &mut Ui,
    id: &str,
    times: &std::collections::VecDeque<u64>,
    series: [(&str, &std::collections::VecDeque<f32>, Color32); 2],
) {
    perf_plot(id)
        .include_y(1024.0)
        .y_axis_formatter(|mark, _| format_bytes(mark.value.max(0.0)) + "/s")
        .label_formatter(rate_label)
        .show(ui, |plot| {
            for (label, values, color) in series {
                plot.line(
                    Line::new(label.to_owned(), series_points(times, values))
                        .color(color)
                        .width(2.0)
                        .fill(0.0)
                        .fill_alpha(0.15),
                );
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

/// Rows of name, a slim bar filled to `load` (0..=100) and its caption.
fn load_bars(ui: &mut Ui, id: &str, rows: impl Iterator<Item = (String, f32, String)>) {
    egui::Grid::new(id)
        .num_columns(3)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            for (name, load, caption) in rows {
                ui.label(name);
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(160.0, 8.0), egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 4.0, crate::theme::track_color(ui));
                let frac = load.clamp(0.0, 100.0) / 100.0;
                if frac > 0.0 {
                    let mut fill = rect;
                    fill.set_width((rect.width() * frac).max(rect.height()));
                    painter.rect_filled(fill, 4.0, crate::theme::load_color(load));
                }
                ui.weak(caption);
                ui.end_row();
            }
        });
}

/// One small history graph per logical processor, as Task Manager shows them.
fn core_grid(
    ui: &mut Ui,
    cores: &[f32],
    history: &[std::collections::VecDeque<f32>],
    topology: Option<&wtm_core::cpu::Topology>,
) {
    const CELL: egui::Vec2 = egui::vec2(124.0, 46.0);
    let topology =
        topology.filter(|t| t.hybrid() && t.groups().len() == 1 && t.cpus.len() == cores.len());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        for (i, &load) in cores.iter().enumerate() {
            let cpu = topology.and_then(|t| t.cpus.iter().find(|c| usize::from(c.index) == i));
            let label = cpu.zip(topology).map_or_else(
                || crate::i18n::format("CPU {0}", &[format!("{}", i)]),
                |(c, t)| {
                    let __wtm_i18n_0 = &(t.tier(c.efficiency_class));
                    crate::i18n::format(
                        "CPU {0} · {1}",
                        &[format!("{}", i), __wtm_i18n_0.to_string()],
                    )
                },
            );
            let (rect, response) = ui.allocate_exact_size(CELL, egui::Sense::hover());
            let painter = ui.painter_at(rect);
            let visuals = ui.visuals();
            let color = crate::theme::text_color(ui, crate::theme::load_color(load));
            painter.rect_filled(rect, 6.0, visuals.extreme_bg_color);
            painter.rect_stroke(
                rect,
                6.0,
                egui::Stroke::new(1.0, crate::theme::CPU.gamma_multiply(0.35)),
                egui::StrokeKind::Inside,
            );
            let graph = rect
                .shrink2(egui::vec2(4.0, 4.0))
                .with_min_y(rect.top() + 18.0);
            if let Some(h) = history.get(i) {
                crate::theme::sparkline(
                    &painter,
                    graph,
                    h,
                    100.0,
                    crate::backend::CORE_HISTORY_LEN,
                    crate::theme::CPU,
                );
            }
            painter.text(
                rect.left_top() + egui::vec2(6.0, 3.0),
                egui::Align2::LEFT_TOP,
                &label,
                egui::FontId::proportional(11.0),
                visuals.weak_text_color(),
            );
            painter.text(
                rect.right_top() + egui::vec2(-6.0, 3.0),
                egui::Align2::RIGHT_TOP,
                format!("{load:.0}%"),
                egui::FontId::proportional(12.0),
                color,
            );
            let mut text = format!("{label}: {load:.1}%");
            if let Some(c) = cpu {
                text.push_str(&{
                    let __wtm_i18n_0 = &(c.group);
                    let __wtm_i18n_1 = &(c.core);
                    let __wtm_i18n_2 = &(c.efficiency_class);
                    crate::i18n::format(
                        " · Group {0}, physical core {1}, Windows class {2}",
                        &[
                            format!("{}", __wtm_i18n_0),
                            format!("{}", __wtm_i18n_1),
                            format!("{}", __wtm_i18n_2),
                        ],
                    )
                });
            }
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text.clone()));
            response.on_hover_text(text);
        }
    });
}

#[cfg(test)]
pub(crate) fn performance(ui: &mut Ui, data: &Data) {
    performance_focused(ui, data, None);
}

fn performance_focused(ui: &mut Ui, data: &Data, focus: Option<crate::overview::Resource>) {
    let Some(snap) = &data.snapshot else {
        crate::theme::loading(ui, crate::i18n::tr("Waiting for monitoring data…"));
        return;
    };
    let s = &snap.system;
    type Section = fn(&mut Ui, &Data, &wtm_core::SystemStats);
    use crate::overview::Resource;
    let sections: [(Option<Resource>, Section); 6] = [
        (Some(Resource::Cpu), perf_cpu),
        (Some(Resource::Memory), perf_memory),
        (Some(Resource::Gpu), perf_gpu),
        (Some(Resource::Network), perf_network),
        (Some(Resource::Disk), perf_disk),
        (None, perf_summary),
    ];
    let show_section = |ui: &mut Ui, (resource, section): (Option<Resource>, Section)| {
        let response = ui.scope(|ui| section(ui, data, s)).response;
        if focus.is_some() && focus == resource {
            ui.scroll_to_rect(response.rect, Some(egui::Align::TOP));
        }
        ui.add_space(10.0);
    };
    let mut area = egui::ScrollArea::vertical().id_salt("performance");
    if let Some(y) = crate::theme::take_scroll(ui, "performance") {
        area = area.vertical_scroll_offset(y);
    }
    let out = area.show(ui, |ui| {
        ui.add_space(4.0);
        // Two columns of similar height when there is room: the tall CPU
        // card with Disk and the summary on the left, the others on the
        // right.
        if ui.available_width() >= PERF_TWO_COLUMNS {
            let [cpu, memory, gpu, network, disk, summary] = sections;
            ui.columns(2, |cols| {
                for (col, list) in cols
                    .iter_mut()
                    .zip([&[cpu, disk, summary][..], &[memory, gpu, network][..]])
                {
                    for &section in list {
                        show_section(col, section);
                    }
                }
            });
        } else {
            for section in sections {
                show_section(ui, section);
            }
        }
    });
    crate::theme::more_below(ui, "performance", &out, ui.visuals().panel_fill);
}

/// GPU engines listed by load; the rest are summarised in one line.
const GPU_ENGINES_SHOWN: usize = 4;

/// Width from which the Performance tab uses two columns.
const PERF_TWO_COLUMNS: f32 = 900.0;

/// Each series of a rate graph in its colour with its current value, for
/// the card title.
fn rate_legend(series: [(&str, f64, Color32); 2]) -> [(String, Color32); 2] {
    series.map(|(name, rate, color)| (format!("● {name} {}/s", format_bytes(rate)), color))
}

fn perf_cpu(ui: &mut Ui, data: &Data, s: &wtm_core::SystemStats) {
    crate::theme::card_with(
        ui,
        "CPU",
        &[(
            &format!("{:.0}%", s.cpu_percent),
            crate::theme::load_color(s.cpu_percent),
        )],
        |ui| {
            history_plot(
                ui,
                "cpu_plot",
                "CPU",
                &data.history.times,
                &data.history.cpu,
                crate::theme::CPU,
            );
            if !s.core_percent.is_empty() {
                ui.add_space(6.0);
                ui.weak({
                    let __wtm_i18n_0 = &(s.core_percent.len());
                    crate::i18n::format(
                        "Per logical processor ({0})",
                        &[format!("{}", __wtm_i18n_0)],
                    )
                });
                if let Some(t) = data.cpu_topology.as_ref().filter(|t| t.hybrid()) {
                    let summary = t
                        .classes()
                        .iter()
                        .rev()
                        .map(|class| {
                            let __wtm_i18n_0 = &(t.tier(*class));
                            let __wtm_i18n_1 = &(t
                                .cpus
                                .iter()
                                .filter(|c| c.efficiency_class == *class)
                                .count());
                            crate::i18n::format(
                                "{0}: {1} threads",
                                &[__wtm_i18n_0.to_string(), format!("{}", __wtm_i18n_1)],
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" · ");
                    ui.label(summary).on_hover_text(crate::i18n::tr(
                        "Relative Windows CPU performance tiers; not vendor model identifiers.",
                    ));
                    if t.groups().len() > 1 {
                        ui.weak(crate::i18n::tr("Per-core tier labels are unavailable across multiple processor groups."));
                    }
                }
                core_grid(
                    ui,
                    &s.core_percent,
                    &data.history.cores,
                    data.cpu_topology.as_ref(),
                );
            }
        },
    );
}

fn perf_memory(ui: &mut Ui, data: &Data, s: &wtm_core::SystemStats) {
    let mem_pct = if s.memory_total > 0 {
        s.memory_used() as f64 / s.memory_total as f64 * 100.0
    } else {
        0.0
    };
    crate::theme::card_with(
        ui,
        crate::i18n::tr("Memory"),
        &[(
            &format!(
                "{} / {} ({mem_pct:.0}%)",
                format_bytes(s.memory_used() as f64),
                format_bytes(s.memory_total as f64)
            ),
            crate::theme::load_color(mem_pct as f32),
        )],
        |ui| {
            history_plot(
                ui,
                "mem_plot",
                crate::i18n::tr("Memory"),
                &data.history.times,
                &data.history.memory,
                crate::theme::MEMORY,
            );
            ui.add_space(4.0);
            let committed = s.commit_limit.saturating_sub(s.commit_available);
            ui.horizontal_wrapped(|ui| {
                ui.weak({
                    let __wtm_i18n_0 = &(format_bytes(s.memory_used() as f64));
                    crate::i18n::format("In use {0}", std::slice::from_ref(__wtm_i18n_0))
                });
                ui.weak("·");
                ui.weak({
                    let __wtm_i18n_0 = &(format_bytes(s.memory_available as f64));
                    crate::i18n::format("Available {0}", std::slice::from_ref(__wtm_i18n_0))
                });
                ui.weak("·");
                ui.weak({
                    let __wtm_i18n_0 = &(format_bytes(committed as f64));
                    let __wtm_i18n_1 = &(format_bytes(s.commit_limit as f64));
                    crate::i18n::format(
                        "Committed {0} of {1}",
                        &[__wtm_i18n_0.to_string(), __wtm_i18n_1.to_string()],
                    )
                });
            });
        },
    );
}

fn perf_gpu(ui: &mut Ui, data: &Data, s: &wtm_core::SystemStats) {
    crate::theme::card_with(
        ui,
        "GPU",
        &[(
            &crate::i18n::format("{0}%  busiest engine", &[format!("{:.0}", s.gpu_percent)]),
            crate::theme::GPU,
        )],
        |ui| {
            history_plot(
                ui,
                "gpu_plot",
                "GPU",
                &data.history.times,
                &data.history.gpu,
                crate::theme::GPU,
            );
            if !s.gpu_engines.is_empty() {
                // The busiest engines; most of the others idle at 0%.
                let mut engines: Vec<&(String, f32)> = s.gpu_engines.iter().collect();
                engines.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                ui.add_space(6.0);
                load_bars(
                    ui,
                    "gpu_engines",
                    engines
                        .iter()
                        .take(GPU_ENGINES_SHOWN)
                        .map(|(name, load)| (name.clone(), *load, format!("{load:.0}%"))),
                );
                if engines.len() > GPU_ENGINES_SHOWN {
                    let rest: Vec<String> = engines[GPU_ENGINES_SHOWN..]
                        .iter()
                        .map(|(name, load)| format!("{name} {load:.0}%"))
                        .collect();
                    ui.weak({
                        let __wtm_i18n_0 = &(engines.len() - GPU_ENGINES_SHOWN);
                        crate::i18n::format("+ {0} more engines", &[format!("{}", __wtm_i18n_0)])
                    })
                    .on_hover_text(rest.join("\n"));
                }
            }
        },
    );
}

fn perf_network(ui: &mut Ui, data: &Data, s: &wtm_core::SystemStats) {
    let legend = rate_legend([
        (
            crate::i18n::tr("Receive"),
            s.net_rx_rate,
            crate::theme::NET_RX,
        ),
        (crate::i18n::tr("Send"), s.net_tx_rate, crate::theme::NET_TX),
    ]);
    crate::theme::card_with(
        ui,
        crate::i18n::tr("Network"),
        &legend.each_ref().map(|(t, c)| (t.as_str(), *c)),
        |ui| {
            rate_plot(
                ui,
                "net_plot",
                &data.history.times,
                [
                    (
                        crate::i18n::tr("Receive"),
                        &data.history.net_rx,
                        crate::theme::NET_RX,
                    ),
                    (
                        crate::i18n::tr("Send"),
                        &data.history.net_tx,
                        crate::theme::NET_TX,
                    ),
                ],
            );
        },
    );
}

fn perf_disk(ui: &mut Ui, data: &Data, s: &wtm_core::SystemStats) {
    let legend = rate_legend([
        (
            crate::i18n::tr("Read"),
            s.io_read_rate,
            crate::theme::DISK_READ,
        ),
        (
            crate::i18n::tr("Write"),
            s.io_write_rate,
            crate::theme::DISK_WRITE,
        ),
    ]);
    crate::theme::card_with(
        ui,
        crate::i18n::tr("Disk & I/O"),
        &legend.each_ref().map(|(t, c)| (t.as_str(), *c)),
        |ui| {
            rate_plot(
                ui,
                "io_plot",
                &data.history.times,
                [
                    (
                        crate::i18n::tr("Read"),
                        &data.history.io_read,
                        crate::theme::DISK_READ,
                    ),
                    (
                        crate::i18n::tr("Write"),
                        &data.history.io_write,
                        crate::theme::DISK_WRITE,
                    ),
                ],
            );
            if !s.disks.is_empty() {
                ui.add_space(6.0);
                load_bars(
                    ui,
                    "disks",
                    s.disks.iter().map(|d| {
                        (
                            {
                                let __wtm_i18n_0 = &(d.name);
                                crate::i18n::format("Disk {0}", std::slice::from_ref(__wtm_i18n_0))
                            },
                            d.active_percent,
                            {
                                let __wtm_i18n_0 = &(d.active_percent);
                                let __wtm_i18n_1 = &(format_bytes(d.read_rate));
                                let __wtm_i18n_2 = &(format_bytes(d.write_rate));
                                crate::i18n::format(
                                    "{0}% active   read {1}/s   write {2}/s",
                                    &[
                                        format!("{:.0}", __wtm_i18n_0),
                                        __wtm_i18n_1.to_string(),
                                        __wtm_i18n_2.to_string(),
                                    ],
                                )
                            },
                        )
                    }),
                );
            }
        },
    );
}

fn perf_summary(ui: &mut Ui, _data: &Data, s: &wtm_core::SystemStats) {
    crate::theme::card(ui, crate::i18n::tr("System"), |ui| {
        // A short list of facts: no stripes that would end mid-card.
        egui::Grid::new("perf_grid")
            .num_columns(2)
            .spacing([24.0, 6.0])
            .show(ui, |ui| {
                let mut row = |k: &str, v: String| {
                    ui.weak(k);
                    ui.strong(v);
                    ui.end_row();
                };
                row(
                    crate::i18n::tr("Logical processors"),
                    s.logical_cpus.to_string(),
                );
                row(crate::i18n::tr("Processes"), s.process_count.to_string());
                row(crate::i18n::tr("Threads"), s.thread_count.to_string());
                row(crate::i18n::tr("Handles"), s.handle_count.to_string());
                row(
                    crate::i18n::tr("Committed"),
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
        crate::theme::panel(ui, |ui| {
            crate::theme::empty_state(
                ui,
                crate::i18n::tr("Service not running"),
                crate::i18n::tr(
                    "The governor runs inside the RustWTM service. Install and start it from an elevated prompt:",
                ),
            );
            ui.add_space(4.0);
            ui.code("wtm-service install\nwtm-service start");
        });
        return;
    };
    crate::theme::card(ui, crate::i18n::tr("Status"), |ui| {
        governor_status(ui, status)
    });
    ui.add_space(10.0);
    crate::theme::card(
        ui,
        &crate::i18n::format(
            "Managed processes ({0})",
            &[status.managed.len().to_string()],
        ),
        |ui| managed_table(ui, status),
    );
}

/// The governor's mode and settings, as a list of facts.
fn governor_status(ui: &mut Ui, status: &wtm_ipc::Status) {
    egui::Grid::new("gov_grid")
        .num_columns(2)
        .spacing([24.0, 6.0])
        .show(ui, |ui| {
            let mode = if status.dry_run {
                RichText::new(crate::i18n::tr("Dry-run (decisions are only logged)"))
                    .color(crate::theme::text_color(ui, WARN))
            } else {
                RichText::new(crate::i18n::tr("Enforcing"))
                    .color(crate::theme::text_color(ui, GOOD))
            };
            let mut row = |k: &str, v: RichText| {
                ui.weak(k);
                ui.label(v);
                ui.end_row();
            };
            row(crate::i18n::tr("Mode"), mode);
            row(
                crate::i18n::tr("Governor"),
                RichText::new(if status.governor_enabled {
                    crate::i18n::tr("enabled")
                } else {
                    crate::i18n::tr("disabled")
                }),
            );
            row(
                crate::i18n::tr("Thresholds"),
                RichText::new({
                    let __wtm_i18n_0 = &(status.high_cpu_percent);
                    let __wtm_i18n_1 = &(status.recover_cpu_percent);
                    crate::i18n::format(
                        "act at ≥ {0}% CPU, roll back at ≤ {1}%",
                        &[
                            format!("{:.0}", __wtm_i18n_0),
                            format!("{:.0}", __wtm_i18n_1),
                        ],
                    )
                }),
            );
            row(
                crate::i18n::tr("Rules"),
                RichText::new(status.rules.to_string()),
            );
            row(
                crate::i18n::tr("Config"),
                RichText::new(&status.config_path).monospace(),
            );
        });
}

/// What the service limits right now, sortable.
fn managed_table(ui: &mut Ui, status: &wtm_ipc::Status) {
    if status.managed.is_empty() {
        ui.weak(crate::i18n::tr("No process is currently limited."));
        return;
    }
    use crate::sort::{Sort, header, text};
    let sources = |m: &wtm_ipc::Managed| {
        let s: Vec<String> = m.sources.iter().map(crate::i18n::debug).collect();
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
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            for (col, label) in [
                "PID",
                crate::i18n::tr("Name"),
                crate::i18n::tr("Source"),
                crate::i18n::tr("Effective limits"),
            ]
            .into_iter()
            .enumerate()
            {
                header(ui, label, col, col == 0, &mut sort);
            }
            ui.end_row();
            for m in rows {
                crate::theme::right(ui, m.target.pid.to_string());
                ui.label(&m.target.name);
                ui.label(sources(m));
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::message(&m.summary).as_ref());
                    crate::theme::fill_row(ui);
                });
                ui.end_row();
            }
        });
    sort.store(ui, "managed");
}

/// Order of event levels when sorting, the most severe last.
fn level_rank(level: EventLevel) -> u8 {
    match level {
        EventLevel::Info => 0,
        EventLevel::Warn => 1,
        EventLevel::Error => 2,
    }
}

fn events(ui: &mut Ui, data: &Data, level: &mut Option<EventLevel>) {
    use crate::sort::{Sort, header, text};
    if data.events.is_empty() {
        crate::theme::panel(ui, |ui| {
            crate::theme::empty_state(
                ui,
                crate::i18n::tr("No events yet."),
                crate::i18n::tr(
                    "Decisions of the service and actions taken from this window are listed here.",
                ),
            );
        });
        return;
    }
    let mut items: Vec<_> = data
        .events
        .iter()
        .rev()
        .filter(|e| level.is_none_or(|l| e.level == l))
        .collect();
    ui.horizontal_wrapped(|ui| {
        for (value, label) in [
            (None, crate::i18n::tr("All events")),
            (Some(EventLevel::Info), crate::i18n::tr("Information")),
            (Some(EventLevel::Warn), crate::i18n::tr("Warnings")),
            (Some(EventLevel::Error), crate::i18n::tr("Errors")),
        ] {
            ui.selectable_value(level, value, label);
        }
        ui.separator();
        ui.weak(crate::i18n::format(
            "{0} of {1} events",
            &[items.len().to_string(), data.events.len().to_string()],
        ));
    });
    ui.add_space(4.0);
    // Newest first, the order they arrive in.
    let mut sort = Sort::load(ui, "events", Sort::new(0, true));
    sort.apply(&mut items, |a, b, col| match col {
        0 => a.unix_ms.cmp(&b.unix_ms),
        1 => level_rank(a.level).cmp(&level_rank(b.level)),
        2 => a.pid.cmp(&b.pid),
        3 => text(
            a.name.as_deref().unwrap_or(""),
            b.name.as_deref().unwrap_or(""),
        ),
        _ => text(&a.message, &b.message),
    });
    let row_height = ui.spacing().interact_size.y;
    // Every header fits at first; narrow windows scroll sideways.
    let widths: Vec<f32> = [
        ("Time", 80.0),
        ("Level", 90.0),
        ("PID", 64.0),
        ("Process", 160.0),
    ]
    .iter()
    .map(|(label, width)| crate::sort::width(ui, label, *width))
    .collect();
    let min_width = widths.iter().sum::<f32>() + 200.0;
    crate::theme::table_panel(ui, "events", min_width, |ui| {
        if items.is_empty() {
            crate::theme::empty_state(
                ui,
                crate::i18n::tr("No events of this kind"),
                crate::i18n::tr("Choose another level to see more."),
            );
            return;
        }
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(widths[0]))
            .column(Column::initial(widths[1]))
            .column(Column::initial(widths[2]))
            .column(Column::initial(widths[3]).clip(true))
            .column(Column::remainder().clip(true))
            .header(row_height, |mut h| {
                for (col, (label, numeric)) in [
                    (crate::i18n::tr("Time"), false),
                    (crate::i18n::tr("Level"), false),
                    ("PID", true),
                    (crate::i18n::tr("Process"), false),
                    (crate::i18n::tr("Message"), false),
                ]
                .into_iter()
                .enumerate()
                {
                    h.col(|ui| header(ui, label, col, numeric, &mut sort));
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
                        let color = level_color(ui, e.level);
                        ui.label(RichText::new(crate::i18n::debug(e.level)).color(color));
                    });
                    row.col(|ui| {
                        crate::theme::right(ui, e.pid.map(|p| p.to_string()).unwrap_or_default());
                    });
                    row.col(|ui| {
                        ui.label(e.name.clone().unwrap_or_default());
                    });
                    row.col(|ui| {
                        ui.label(crate::i18n::message(&e.message).as_ref());
                    });
                });
            });
    });
    sort.store(ui, "events");
}

/// A change asked about before it is made: stopping or disabling a service
/// can break what depends on it, and a blocked program stays offline.
#[derive(Debug, Clone, PartialEq)]
enum Change {
    Service {
        name: String,
        display: String,
        op: ServiceOp,
    },
    Block {
        path: String,
        name: String,
    },
}

impl Change {
    fn needs_confirming(op: ServiceOp) -> bool {
        matches!(
            op,
            ServiceOp::Stop
                | ServiceOp::Restart
                | ServiceOp::StartType(wtm_core::StartType::Disabled)
        )
    }

    /// The confirming button's label and the question.
    fn describe(&self) -> (String, String) {
        match self {
            Change::Service { display, op, .. } => {
                let (title, question) = match op {
                    ServiceOp::Restart => (
                        "Restart",
                        "Restart the service \"{0}\"? It stops for a moment; programs that rely on it may lose their connection to it.",
                    ),
                    ServiceOp::StartType(_) => (
                        "Disable",
                        "Disable the service \"{0}\"? It will no longer start, not even when Windows or a program needs it.",
                    ),
                    _ => (
                        "Stop",
                        "Stop the service \"{0}\"? Programs that rely on it may stop working until it runs again.",
                    ),
                };
                (
                    crate::i18n::tr(title).to_owned(),
                    crate::i18n::format(question, std::slice::from_ref(display)),
                )
            }
            Change::Block { name, .. } => (
                crate::i18n::tr("Block network access").to_owned(),
                crate::i18n::format(
                    "Block network access for {0}? It cannot connect until you unblock it in the Network tab.",
                    std::slice::from_ref(name),
                ),
            ),
        }
    }

    fn command(&self) -> UiCommand {
        match self.clone() {
            Change::Service { name, op, .. } => UiCommand::Service { name, op },
            Change::Block { path, .. } => UiCommand::Firewall { path, block: true },
        }
    }
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
            ui.weak(crate::i18n::tr(
                "Select a service to start, stop or change its startup type.",
            ));
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
            .add_enabled(stopped, egui::Button::new(crate::i18n::tr("Start")))
            .clicked()
        {
            send(ServiceOp::Start);
        }
        if ui
            .add_enabled(running, egui::Button::new(crate::i18n::tr("Stop")))
            .clicked()
        {
            send(ServiceOp::Stop);
        }
        if ui
            .add_enabled(running, egui::Button::new(crate::i18n::tr("Restart")))
            .clicked()
        {
            send(ServiceOp::Restart);
        }
        ui.menu_button(crate::i18n::tr("Startup type"), |ui| {
            for t in [
                wtm_core::StartType::Automatic,
                wtm_core::StartType::Manual,
                wtm_core::StartType::Disabled,
            ] {
                if ui.button(crate::i18n::debug(t)).clicked() {
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
    fn overview_details_reveal_a_process_despite_old_filters_and_a_paused_list() {
        let (mut h, _rx) = overview_app(data());
        {
            let view = &mut h.state_mut().state.processes;
            view.search = "does-not-match".into();
            view.filter = crate::processes::QuickFilter::Memory;
            view.more.disk_active = true;
            view.mode = crate::processes::ViewMode::Apps;
            view.paused = Some(std::sync::Arc::new(Snapshot {
                processes: Vec::new(),
                ..crate::ui_tests::snapshot()
            }));
        }
        h.get_all_by_label("Details").next().unwrap().click();
        h.run_steps(20);
        let view = &h.state().state.processes;
        assert_eq!(view.selected.as_ref().unwrap().pid, 100);
        println!(
            "EXPECTED: requested process visible despite old filters; ACTUAL search: {:?}",
            view.search
        );
        assert!(view.search.is_empty());
        assert_eq!(view.filter, crate::processes::QuickFilter::All);
        assert!(view.more.is_empty());
        assert!(view.paused.is_none());
        assert!(view.expanded.contains("alpha.exe"));
        assert!(h.query_by_label("alpha.exe (1)").is_some());
    }

    #[test]
    fn overview_resource_cards_open_and_scroll_to_their_graphs() {
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            for (name, graph_label, keyboard) in [
                ("CPU", "CPU42%", false),
                ("Memory", "Memory8.0 GiB", true),
                ("GPU", "GPU12%", false),
                ("Disk", "Disk & I/O●", false),
                ("Network", "Network●", false),
            ] {
                let (backend, _rx) = Backend::offline(data());
                let mut h = Harness::builder()
                    .allow_missing_glyphs()
                    .with_size([820.0, 600.0])
                    .with_theme(theme)
                    .build_eframe(|cc| {
                        crate::theme::apply(&cc.egui_ctx);
                        App {
                            backend,
                            state: UiState {
                                check_updates: false,
                                ..Default::default()
                            },
                            tray: None,
                            frames: Default::default(),
                        }
                    });
                h.run_steps(3);
                let card_label = format!("{name}:");
                let card = h.get_by_label_contains(&card_label);
                if keyboard {
                    card.focus();
                    h.run_steps(2);
                    h.key_press(egui::Key::Enter);
                } else {
                    card.click();
                }
                h.run_steps(20);
                assert_eq!(h.state().state.tab, Tab::Performance, "{name}");
                let graph = h.get_by_label_contains(graph_label);
                let rect = graph.rect();
                assert!(
                    rect.min.y >= 40.0 && rect.max.y < 200.0,
                    "{name} graph should be near the top: {rect:?}"
                );
            }
        }
    }

    #[test]
    fn status_actions_stay_inside_a_compact_window_with_a_long_message() {
        for language in [
            crate::i18n::Language::English,
            crate::i18n::Language::German,
            crate::i18n::Language::Turkish,
            crate::i18n::Language::French,
            crate::i18n::Language::Spanish,
        ] {
            let _language = crate::i18n::scope(language);
            let mut d = data();
            d.elevated = false;
            let long = "A process could not be controlled. ".repeat(12);
            d.message = Some((EventLevel::Warn, long.clone()));
            let (backend, _rx) = Backend::offline(d);
            let mut h = Harness::builder()
                .allow_missing_glyphs()
                .with_size([820.0, 500.0])
                .build_eframe(|cc| {
                    crate::theme::apply(&cc.egui_ctx);
                    App {
                        backend,
                        state: UiState {
                            language,
                            check_updates: false,
                            ..Default::default()
                        },
                        tray: None,
                        frames: Default::default(),
                    }
                });
            h.run_steps(3);
            for label in ["Run new task", "Restart as administrator"] {
                let node = h.get_by_role_and_label(egui::Role::Button, crate::i18n::tr(label));
                let rect = node.rect();
                println!("EXPECTED: {label} inside 820x500; ACTUAL: {rect:?}");
                assert!(
                    rect.min.x >= 0.0 && rect.max.x <= 820.0 && rect.max.y <= 500.0,
                    "{label}: {rect:?}"
                );
            }
            let summary = crate::i18n::format(
                "CPU {0}%   Memory {1} / {2}",
                &["42".into(), "8.0 GiB".into(), "16.0 GiB".into()],
            );
            let rect = h.get_by_label(&summary).rect();
            assert!(
                rect.height() <= 30.0,
                "resource summary stays on one line: {rect:?}"
            );
            h.get_by_role_and_label(egui::Role::Button, crate::i18n::tr("Run new task"))
                .click();
            h.run_steps(2);
            assert!(h.state().state.run_task.is_some());
        }
    }

    #[test]
    fn graphs_place_points_at_their_measurement_time() {
        use std::collections::VecDeque;
        // Every 2 s, then a sample that arrived 5 s late.
        let times = VecDeque::from([10_000, 12_000, 14_000, 21_000]);
        let values = VecDeque::from([9.0, 1.0, 2.0, 3.0, 4.0]);
        let xy: Vec<[f64; 2]> = series_points(&times, &values)
            .points()
            .iter()
            .map(|p| [p.x, p.y])
            .collect();
        // The extra oldest value has no time and is left out.
        assert_eq!(xy, [[-11.0, 1.0], [-9.0, 2.0], [-7.0, 3.0], [0.0, 4.0]]);
        assert!(series_points(&VecDeque::new(), &values).points().is_empty());
    }

    #[test]
    fn full_app_navigates_every_tab() {
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1300.0, 850.0])
            .build_eframe(|cc| {
                let mut app = App::new(cc, false);
                app.state.language = crate::i18n::Language::English;
                app
            });
        let me = std::process::id();
        let sampled = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(50));
            // An installed service can first return a cached sample from
            // before this test started. Wait for the process we will select.
            h.state()
                .backend
                .data()
                .snapshot
                .as_ref()
                .is_some_and(|s| s.processes.iter().any(|p| p.pid == me))
        });
        assert!(sampled, "no snapshot containing this test process");
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
        let history = d
            .process_history
            .entry((sel.pid, sel.create_time))
            .or_default();
        history.cpu.extend([1.0, 30.0, 12.0]);
        history.memory.extend([1e6, 2e6, 3e6]);
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
                        mode_badge(
                            &mut cols[0],
                            mode,
                            crate::backend::ServiceProblem::NotRunning,
                            None,
                        );
                    }
                    for problem in [
                        crate::backend::ServiceProblem::Older,
                        crate::backend::ServiceProblem::Newer,
                        crate::backend::ServiceProblem::Impostor,
                    ] {
                        mode_badge(&mut cols[0], Mode::Local, problem, None);
                    }
                    mode_badge(
                        &mut cols[0],
                        Mode::Local,
                        crate::backend::ServiceProblem::NotRunning,
                        Some(None),
                    );
                    performance(&mut cols[1], &d);
                    events(&mut cols[1], &d, &mut None);
                });
            });
        h.run_steps(3);
        assert!(
            h.query_by_label("Dry-run (decisions are only logged)")
                .is_some()
        );
        assert!(h.query_by_label("Last two minutes").is_some());
        assert!(h.query_by_label(r"C:\Apps\alpha.exe").is_some());
        // alpha.exe listens on 127.0.0.1:8080.
        assert!(h.query_by_label("Listening on").is_some());
        assert!(h.query_by_label("TCP 8080").is_some());
        drop(h);

        // Empty states.
        d.status = None;
        d.events.clear();
        d.snapshot = None;
        let mut h = Harness::builder().allow_missing_glyphs().build_ui(|ui| {
            governor(ui, &d);
            events(ui, &d, &mut None);
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

    #[derive(Default)]
    struct MemStorage(HashMap<String, String>);

    impl eframe::Storage for MemStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn preferences_survive_a_restart() {
        use crate::processes::{Col, ViewMode};
        use eframe::Storage as _;
        let mut storage = MemStorage::default();
        assert_eq!(UiState::restored(None).tab, Tab::Overview);
        assert_eq!(UiState::restored(Some(&storage)).tab, Tab::Overview);

        let mut state = UiState {
            tab: Tab::Performance,
            language: crate::i18n::Language::French,
            nav_collapsed: Some(true),
            ..UiState::default()
        };
        state.processes.mode = ViewMode::Apps;
        state.processes.sort = Col::Memory;
        state.processes.descending = false;
        state.processes.visible.insert(Col::CommandLine);
        state.processes.heat = false;
        eframe::set_value(&mut storage, PREFS_KEY, &state.prefs());

        let restored = UiState::restored(Some(&storage));
        assert_eq!(restored.prefs(), state.prefs());
        assert!(
            restored.processes.search.is_empty(),
            "only preferences return"
        );

        // Settings from another version fall back to the defaults.
        storage.set_string(PREFS_KEY, "(tab: Gone)".into());
        assert_eq!(UiState::restored(Some(&storage)).prefs(), Prefs::default());
        // Missing fields keep their defaults.
        storage.set_string(PREFS_KEY, "(tab: About)".into());
        let partial = UiState::restored(Some(&storage)).prefs();
        assert_eq!(partial.tab, Tab::About);
        assert_eq!(partial.process_sort, Prefs::default().process_sort);
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
            UiCommand::WaitChain(t) => format!("wait chain {}", t.pid),
            UiCommand::WhyAlive(t) => format!("why alive {}", t.pid),
            UiCommand::Modules(t) => format!("modules {}", t.pid),
            UiCommand::CloseConnection { local, remote } => format!("close {local} {remote}"),
            UiCommand::Firewall { path, block } => format!("firewall {path} {block}"),
            UiCommand::Load(panel) => format!("load {panel:?}"),
            UiCommand::LoadHistory { hours } => format!("history {hours}"),
            UiCommand::SetStartup { entry, enabled } => {
                format!("startup {} {enabled}", entry.name)
            }
            UiCommand::LoadSeries { app, hours } => format!("series {app} {hours}"),
            UiCommand::Service { name, op } => format!("service {name} {op:?}"),
            UiCommand::SaveConfig(toml) => format!("save config {}", toml.len()),
            UiCommand::SaveTelegram(c) => format!("save telegram {}", c.enabled),
            UiCommand::TestTelegram => "test telegram".into(),
            UiCommand::SetCooling { channel, .. } => format!("cooling {channel}"),
            UiCommand::SetFrequency(request) => {
                format!("frequency {} {}", request.class, request.mhz)
            }
            UiCommand::RestoreFrequency => "restore frequency".into(),
            UiCommand::CreateDump(t) => format!("dump {}", t.pid),
            UiCommand::WatchExit(t) => format!("watch exit {}", t.pid),
            UiCommand::Session { id, op } => format!("session {id} {op:?}"),
            UiCommand::OpenLocation(t) => format!("open {}", t.pid),
            UiCommand::Run { line, elevated } => format!("run {line} {elevated}"),
            UiCommand::RestartElevated => "restart elevated".into(),
            UiCommand::CloseAlert(quiet) => format!("close alert {}", quiet.as_secs()),
            UiCommand::CheckUpdate => "check update".into(),
            UiCommand::InstallUpdate(r) => format!("install update {}", r.tag),
        }
    }

    const KILL_CONFIG: &str = "dry_run = false\n\
        [[kill]]\nname = \"alpha.exe\"\ncmdline = \"vitest\"\ncpu_above = 50.0\nfor_secs = 60\n\
        [[kill]]\nname = \"beta.exe\"\ncmdline = \"never\"\n";

    fn kill_app(tab: Tab) -> (Harness<'static, App>, std::sync::mpsc::Receiver<UiCommand>) {
        let mut d = data();
        d.mode = Mode::Service {
            admin: true,
            verified: true,
        };
        d.config = Some(Ok(("config.toml".into(), KILL_CONFIG.into())));
        d.kill_list = wtm_policy::Config::from_toml(KILL_CONFIG).unwrap().kill;
        let snap = d.snapshot.clone().unwrap();
        for p in &snap.processes {
            let line = match p.pid {
                100 => "node node_modules/vitest/vitest.mjs run",
                _ => "something else",
            };
            d.command_lines.insert((p.pid, p.create_time), line.into());
        }
        let (backend, rx) = Backend::offline(d);
        let h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 900.0])
            .build_eframe(move |_| App {
                backend,
                state: UiState {
                    tab,
                    ..Default::default()
                },
                tray: None,
                frames: Default::default(),
            });
        (h, rx)
    }

    #[test]
    fn users_are_signed_off_and_services_restarted_after_asking() {
        let (mut h, rx) = kill_app(Tab::Users);
        h.state().backend.data().sessions = Some(Ok(vec![wtm_core::Session {
            id: 3,
            station: "RDP-Tcp#1".into(),
            user: "guest".into(),
            domain: "DESK".into(),
            state: wtm_core::SessionState::Active,
        }]));
        let sent = |h: &mut Harness<'_, App>| {
            h.run_steps(2);
            rx.try_iter().map(describe_command).collect::<Vec<_>>()
        };
        sent(&mut h);
        h.get_by_label("Sign off").click();
        h.run_steps(2);
        assert!(
            sent(&mut h).iter().all(|c| !c.starts_with("session")),
            "asks first"
        );
        assert!(h.query_by_label_contains("unsaved work is lost").is_some());
        h.get_all_by_label("Sign off").last().unwrap().click();
        assert!(sent(&mut h).contains(&"session 3 SignOff".to_owned()));

        h.get_by_label("Disconnect").click();
        h.run_steps(2);
        h.get_by_label("Cancel").click();
        assert!(sent(&mut h).iter().all(|c| !c.starts_with("session")));
        assert!(h.state().state.confirm_session.is_none());
    }

    #[test]
    fn panic_button_ends_the_kill_list_after_one_confirmation() {
        let (mut h, rx) = kill_app(Tab::Processes);
        let sent = |h: &mut Harness<'_, App>| {
            h.run_steps(2);
            rx.try_iter().map(describe_command).collect::<Vec<_>>()
        };
        sent(&mut h);
        h.get_by_label("🚨 Panic").click();
        h.run_steps(2);
        // alpha.exe runs vitest and alpha-child.exe was started by it.
        assert!(h.query_by_label("  └ alpha-child.exe").is_some());
        h.get_by_label("End 2 processes now").click();
        let s = sent(&mut h);
        assert_eq!(s, ["control 100 Kill", "control 101 Kill"]);
        assert!(!h.state().state.panic);

        // The shortcut opens it too; Cancel ends nothing.
        h.key_press_modifiers(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::K,
        );
        h.run_steps(2);
        assert!(h.state().state.panic);
        h.get_by_label("Cancel").click();
        assert!(sent(&mut h).iter().all(|c| !c.contains("Kill")));
        assert!(!h.state().state.panic);

        // Nothing running: it says so.
        h.state().backend.data().command_lines.clear();
        h.state_mut().state.panic = true;
        h.run_steps(2);
        assert!(
            h.query_by_label("Nothing on the kill list is running right now.")
                .is_some()
        );
        h.get_by_label("Close").click();
        h.run_steps(2);
        assert!(!h.state().state.panic);
    }

    #[test]
    fn kill_list_tab_lists_adds_edits_and_removes_entries() {
        let (mut h, rx) = kill_app(Tab::KillList);
        let saved = |h: &mut Harness<'_, App>| {
            h.run_steps(2);
            rx.try_iter().find_map(|c| match c {
                UiCommand::SaveConfig(t) => Some(wtm_policy::Config::from_toml(&t).unwrap()),
                _ => None,
            })
        };
        h.run_steps(2);
        assert!(h.query_by_label("alpha.exe running \"vitest\"").is_some());
        assert!(
            h.query_by_label("automatically after 60 s at 50% CPU or more")
                .is_some()
        );
        assert!(h.query_by_label("only with the panic button").is_some());
        h.get_by_label("🚨 Panic: end 2 processes now").click();
        h.run_steps(2);
        assert!(h.state().state.panic);
        h.get_by_label("Cancel").click();
        h.run_steps(2);

        // A new entry; the preview shows what it matches.
        let inputs =
            |h: &Harness<'_, App>| h.get_all_by_role(egui::accesskit::Role::TextInput).count();
        assert_eq!(inputs(&h), 2);
        h.get_all_by_role(egui::accesskit::Role::TextInput)
            .next()
            .unwrap()
            .click();
        h.run_steps(1);
        h.get_all_by_role(egui::accesskit::Role::TextInput)
            .next()
            .unwrap()
            .type_text("beta.exe");
        h.run_steps(2);
        assert!(
            h.query_by_label("Matches 1 running now: beta.exe (200)")
                .is_some()
        );
        h.get_by_label("only while the PC is above").click();
        h.get_by_label("Also end the processes it started").click();
        h.run_steps(1);
        h.get_by_label("Add entry").click();
        let c = saved(&mut h).expect("saved");
        assert_eq!(c.kill.len(), 3);
        let added = &c.kill[2];
        assert_eq!(added.name, "beta.exe");
        assert_eq!(added.cpu_above, Some(50.0));
        assert_eq!(added.system_cpu_above, Some(85.0));
        assert!(!added.tree && added.cmdline.is_none());

        // Edit the first entry in place.
        h.get_all_by_label("Edit").next().unwrap().click();
        h.run_steps(2);
        assert!(h.query_by_label("Change entry").is_some());
        h.get_by_label("End it automatically at").click();
        h.run_steps(1);
        h.get_by_label("Save entry").click();
        let c = saved(&mut h).expect("saved");
        assert_eq!(c.kill.len(), 2);
        assert_eq!(c.kill[0].name, "alpha.exe");
        assert_eq!(c.kill[0].cpu_above, None, "now panic button only");

        // Cancel an edit, then remove the second entry.
        h.get_all_by_label("Edit").nth(1).unwrap().click();
        h.run_steps(2);
        h.get_by_label("Cancel").click();
        h.run_steps(2);
        assert!(h.query_by_label("Add to the kill list").is_some());
        h.get_all_by_label("Remove").nth(1).unwrap().click();
        let c = saved(&mut h).expect("saved");
        assert_eq!(c.kill.len(), 1);
        assert_eq!(c.kill[0].name, "alpha.exe");

        // Invalid input is explained, not saved.
        h.get_all_by_role(egui::accesskit::Role::TextInput)
            .nth(1)
            .unwrap()
            .click();
        h.run_steps(1);
        h.get_all_by_role(egui::accesskit::Role::TextInput)
            .nth(1)
            .unwrap()
            .type_text("vitest");
        h.run_steps(1);
        h.get_by_label("Add entry").click();
        h.run_steps(2);
        assert!(h.query_by_label("name must not be empty").is_some());
    }

    #[test]
    fn add_to_kill_list_from_a_process_and_states_without_a_config() {
        let (mut h, _rx) = kill_app(Tab::Processes);
        h.run_steps(2);
        h.get_by_label("beta.exe").click_secondary();
        h.run_steps(2);
        h.get_by_label("Add to kill list…").click();
        h.run_steps(3);
        assert_eq!(h.state().state.tab, Tab::KillList);
        assert!(
            h.query_by_label("Matches 1 running now: beta.exe (200)")
                .is_some()
        );

        // Read-only, loading and broken configuration.
        h.state().backend.data().mode = Mode::Service {
            admin: false,
            verified: true,
        };
        h.run_steps(2);
        assert!(
            h.query_by_label("Run RustWTM as administrator to change the kill list.")
                .is_some()
        );
        h.state().backend.data().config = Some(Err("no service".into()));
        h.run_steps(2);
        assert!(h.query_by_label("no service").is_some());
        h.get_by_label("Retry").click();
        h.run_steps(2);
        h.state().backend.data().config = None;
        h.state().backend.data().kill_list.clear();
        h.run_steps(2);
        assert!(
            h.query_by_label("🚨 Panic").is_none(),
            "no kill list, no button"
        );
    }

    #[test]
    fn without_the_service_calming_spares_the_window_in_front() {
        let mut d = data();
        d.mode = Mode::Local;
        // No service status to name the app in front; the window's PID does.
        d.status = None;
        d.foreground_pid = Some(100);
        d.alert_watch.active = Some(crate::alert::Alert {
            cpu: 97.0,
            started: Instant::now(),
        });
        let (backend, rx) = Backend::offline(d);
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
                frames: Default::default(),
            });
        h.run_steps(2);
        rx.try_iter().for_each(drop);
        h.get_by_label("⚡ Calm them all down").click();
        h.run_steps(2);
        let sent: Vec<_> = rx.try_iter().map(describe_command).collect();
        assert_eq!(
            sent,
            [
                "control 101 EcoQos(On)",
                "control 101 Priority(BelowNormal)"
            ]
        );
    }

    #[test]
    fn cpu_alert_offers_quick_fixes_and_closes() {
        let mut d = data();
        d.mode = Mode::Service {
            admin: true,
            verified: true,
        };
        d.config = Some(Ok(("config.toml".into(), "dry_run = true\n".into())));
        d.alert_watch.active = Some(crate::alert::Alert {
            cpu: 97.0,
            started: Instant::now(),
        });
        let (backend, rx) = Backend::offline(d);
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
                frames: Default::default(),
            });
        let sent = |h: &mut Harness<'_, App>| {
            h.run_steps(2);
            rx.try_iter().map(describe_command).collect::<Vec<_>>()
        };
        sent(&mut h);
        assert!(
            h.query_by_label("⚠ Your PC is overloaded: CPU 97%")
                .is_some()
        );
        // alpha.exe is in the foreground (see `data`), so only its child is
        // calmed; System is protected and beta.exe is idle.
        h.get_by_label("⚡ Calm them all down").click();
        assert_eq!(
            sent(&mut h),
            [
                "control 101 EcoQos(On)",
                "control 101 Priority(BelowNormal)"
            ]
        );
        h.get_all_by_label("Efficiency mode")
            .next()
            .unwrap()
            .click();
        assert_eq!(
            sent(&mut h),
            [
                "control 100 EcoQos(On)",
                "control 100 Priority(BelowNormal)"
            ]
        );
        h.get_all_by_label("Limit to 1 core")
            .next()
            .unwrap()
            .click();
        let s = sent(&mut h);
        assert_eq!(s.len(), 1);
        assert!(
            s[0].starts_with("control 100 Limit(Limits { cpu_percent: Some(12.5)"),
            "{s:?}"
        );
        h.get_all_by_label("Pause").next().unwrap().click();
        assert_eq!(sent(&mut h), ["control 100 Suspend"]);

        // Automatic protection can be switched on right here.
        h.get_by_label("Turn it on").click();
        let s = sent(&mut h);
        assert!(s.iter().any(|c| c.starts_with("save config")), "{s:?}");

        h.get_by_label("Snooze for 1 hour").click();
        assert_eq!(sent(&mut h), ["close alert 3600"]);
        h.get_by_label("Turn off these alerts").click();
        assert_eq!(sent(&mut h), ["close alert 300"]);
        assert!(!h.state().state.alerts.enabled);

        // End asks first.
        h.get_all_by_label("End").next().unwrap().click();
        h.run_steps(2);
        assert!(h.state().state.confirm_many.is_some());

        // Once closed by the backend, the dialog is gone.
        h.state().backend.data().alert_watch.active = None;
        h.state_mut().state.confirm_many = None;
        h.run_steps(2);
        assert!(h.query_by_label("Snooze for 1 hour").is_none());
    }

    #[test]
    fn update_bar_offers_the_new_version() {
        use crate::backend::UpdateState;
        let release = |tag: &str, msi: bool| wtm_core::update::Release {
            version: (99, 1, 0),
            tag: tag.into(),
            page: "https://example.invalid/r".into(),
            msi: msi.then(|| ("https://x/a.msi".into(), "https://x/a.sha256".into())),
        };
        let mut d = data();
        d.update = UpdateState::Available(release("v99.1.0", true));
        d.installable = true;
        let (backend, rx) = Backend::offline(d);
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 900.0])
            .build_eframe(|_| App {
                backend,
                state: UiState::default(),
                tray: None,
                frames: Default::default(),
            });
        let sent = |h: &mut Harness<'_, App>| {
            h.run_steps(2);
            rx.try_iter().map(describe_command).collect::<Vec<_>>()
        };
        let s = sent(&mut h);
        assert!(
            s.contains(&"check update".to_owned()),
            "checked at start: {s:?}"
        );
        assert!(
            h.query_by_label_contains("RustWTM 99.1.0 is available (you have")
                .is_some()
        );
        h.get_by_label("Update now").click();
        assert_eq!(sent(&mut h), ["install update v99.1.0"]);
        assert!(h.state().state.update_started);

        // Installing shows progress, a failure says why.
        h.state().backend.data().update = UpdateState::Installing(release("v99.1.0", true));
        h.run_steps(2);
        assert!(
            h.query_by_label_contains("Downloading and verifying RustWTM 99.1.0")
                .is_some()
        );
        h.state().backend.data().update = UpdateState::Failed("checksum mismatch".into());
        h.run_steps(2);
        assert!(
            h.query_by_label("The update failed: checksum mismatch")
                .is_some()
        );
        h.get_by_label("Dismiss").click();
        h.run_steps(2);
        assert!(h.query_by_label_contains("The update failed").is_none());

        // Not installed with the installer: a download link instead.
        h.state().backend.data().installable = false;
        h.state().backend.data().update = UpdateState::Available(release("v99.1.0", true));
        h.run_steps(2);
        assert!(h.query_by_label("Update now").is_none());
        assert!(h.query_by_label("Download").is_some());
        // Skip remembers the version; Later hides it for this run.
        h.get_by_label("Skip this version").click();
        h.run_steps(2);
        assert_eq!(h.state().state.skip_version.as_deref(), Some("v99.1.0"));
        assert!(h.query_by_label_contains("is available").is_none());
        h.state().backend.data().update = UpdateState::Available(release("v99.2.0", false));
        h.run_steps(2);
        assert!(h.query_by_label_contains("is available").is_some());
        h.get_by_label("Later").click();
        h.run_steps(2);
        assert!(h.query_by_label_contains("is available").is_none());

        // About checks on request.
        h.state_mut().state.tab = Tab::About;
        h.run_steps(2);
        sent(&mut h);
        h.get_by_label("Check for updates").click();
        assert_eq!(sent(&mut h), ["check update"]);
        h.state().backend.data().update = UpdateState::Checking;
        h.run_steps(2);
        assert!(h.query_by_label("Checking for updates…").is_some());
    }

    #[test]
    fn closing_the_window_hides_it_to_the_tray_setting() {
        let (backend, _rx) = Backend::offline(data());
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 900.0])
            .build_eframe(|_| App {
                backend,
                state: UiState {
                    tab: Tab::Settings,
                    ..Default::default()
                },
                tray: None,
                frames: Default::default(),
            });
        h.run_steps(2);
        assert!(h.query_by_label("This window").is_some());
        h.get_by_label("Pop up when the CPU stays above").click();
        h.get_by_label("Keep running in the tray when the window is closed")
            .click();
        h.run_steps(2);
        let st = &h.state().state;
        assert!(!st.alerts.enabled && !st.tray_on_close);
        assert!(
            !h.state().backend.data().alert_watch.settings.enabled,
            "the backend follows the setting"
        );
        assert!(st.autostart.is_some(), "read once from the registry");
    }

    #[test]
    fn run_new_task_and_restart_as_administrator() {
        let (backend, rx) = Backend::offline(data());
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 900.0])
            .build_eframe(|_| App {
                backend,
                state: UiState::default(),
                tray: None,
                frames: Default::default(),
            });
        h.run_steps(2);
        let sent = |h: &mut Harness<'_, App>| {
            h.run_steps(3);
            rx.try_iter().map(describe_command).collect::<Vec<_>>()
        };
        // The Overview loads the configuration first.
        sent(&mut h);

        h.get_by_label("Restart as administrator").click();
        assert_eq!(sent(&mut h), ["restart elevated"]);

        // Cancel closes the dialog without running anything.
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Run new task")
            .click();
        h.run_steps(2);
        assert!(h.get_all_by_label("Run new task").count() >= 2);
        h.get_by_label("Cancel").click();
        assert!(sent(&mut h).is_empty());
        assert!(h.state().state.run_task.is_none());

        // Typing and Enter runs it; the checkbox asks for elevation.
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Run new task")
            .click();
        h.run_steps(2);
        h.get_by_label("Create this task with administrator rights")
            .click();
        h.run_steps(2);
        h.get_by_role(egui::accesskit::Role::TextInput).click();
        h.run_steps(2);
        h.get_by_role(egui::accesskit::Role::TextInput)
            .type_text("  notepad a.txt ");
        h.run_steps(2);
        h.key_press(egui::Key::Enter);
        assert_eq!(sent(&mut h), ["run notepad a.txt true"]);
        assert!(h.state().state.run_task.is_none());

        // The Run button works too; elevated GUIs hide the restart button.
        h.state().backend.data().elevated = true;
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Run new task")
            .click();
        h.run_steps(2);
        h.get_by_role(egui::accesskit::Role::TextInput)
            .type_text("cmd");
        h.run_steps(2);
        h.get_by_label("Run").click();
        assert_eq!(sent(&mut h), ["run cmd false"]);
        assert!(h.query_by_label("Restart as administrator").is_none());
    }

    #[test]
    fn established_ipv4_connections_can_be_closed_from_the_network_tab() {
        let mut d = data();
        let established = wtm_core::Connection {
            protocol: wtm_core::Protocol::Tcp,
            local: "10.0.0.5:51000".parse().unwrap(),
            remote: Some("93.184.216.34:443".parse().unwrap()),
            state: Some(wtm_core::TcpState::Established),
            pid: 100,
        };
        assert!(views::closable(&established).is_some());
        let listening = wtm_core::Connection {
            remote: None,
            state: Some(wtm_core::TcpState::Listen),
            ..established.clone()
        };
        assert!(views::closable(&listening).is_none());
        let v6 = wtm_core::Connection {
            local: "[::1]:51000".parse().unwrap(),
            remote: Some("[::1]:443".parse().unwrap()),
            ..established.clone()
        };
        assert!(views::closable(&v6).is_none());

        d.connections = Some(Ok(vec![established, listening]));
        let (backend, rx) = Backend::offline(d);
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 900.0])
            .build_eframe(|_| App {
                backend,
                state: UiState {
                    tab: Tab::Network,
                    check_updates: false,
                    ..Default::default()
                },
                tray: None,
                frames: Default::default(),
            });
        h.run_steps(2);
        assert_eq!(
            h.get_all_by_label("Close").count(),
            1,
            "only the open IPv4 one"
        );
        h.get_by_label("Close").click();
        h.run_steps(2);
        let sent: Vec<String> = rx.try_iter().map(describe_command).collect();
        assert!(
            sent.contains(&"close 10.0.0.5:51000 93.184.216.34:443".to_owned()),
            "{sent:?}"
        );
    }

    #[test]
    fn modules_list_puts_unsigned_and_invalid_first() {
        use wtm_core::{Module, Signature};
        let m = |name: &str| Module {
            name: name.into(),
            path: format!("C:/x/{name}"),
            size: 4096,
        };
        let mut d = data();
        let alpha = ProcessRef::from(&crate::ui_tests::proc(100, 0, "alpha.exe", 20.0));
        d.modules = Some((
            alpha.clone(),
            Some(Ok(vec![
                (
                    m("kernel32.dll"),
                    Signature::Signed {
                        signer: Some("Microsoft Windows".into()),
                        catalog: true,
                    },
                ),
                (m("hook.dll"), Signature::Unsigned),
                (m("evil.dll"), Signature::Invalid("bad digest".into())),
            ])),
        ));
        let (backend, rx) = Backend::offline(d);
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 900.0])
            .build_eframe(|_| App {
                backend,
                state: UiState {
                    tab: Tab::Processes,
                    check_updates: false,
                    ..Default::default()
                },
                tray: None,
                frames: Default::default(),
            });
        h.state_mut().state.processes.selected = Some(alpha);
        h.run_steps(2);
        assert!(
            h.query_by_label("3 modules: 1 unsigned, 1 with an invalid signature")
                .is_some()
        );
        assert!(h.query_by_label("invalid signature: bad digest").is_some());
        assert!(h.query_by_label("Microsoft Windows").is_some());
        let names: Vec<_> = ["evil.dll", "hook.dll", "kernel32.dll"]
            .iter()
            .map(|n| h.get_by_label(n).rect().min.y)
            .collect();
        assert!(
            names.windows(2).all(|w| w[0] < w[1]),
            "invalid, unsigned, signed"
        );
        h.get_by_label("Modules").click();
        h.run_steps(2);
        assert!(
            rx.try_iter()
                .map(describe_command)
                .any(|c| c == "modules 100")
        );
    }

    #[test]
    fn game_mode_shows_on_the_overview_and_in_the_status_bar() {
        let mut d = data();
        d.status.as_mut().unwrap().game = Some(wtm_ipc::GameStatus {
            name: "game.exe".into(),
            calmed: 3,
        });
        let (backend, _rx) = Backend::offline(d);
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 900.0])
            .build_eframe(|_| App {
                backend,
                state: UiState {
                    check_updates: false,
                    ..Default::default()
                },
                tray: None,
                frames: Default::default(),
            });
        h.run_steps(2);
        assert!(
            h.query_by_label_contains(
                "Game mode: game.exe is in front; 3 background apps calmed until it ends"
            )
            .is_some()
        );
        assert!(h.query_by_label("▶ Game mode").is_some());
    }

    #[test]
    fn programs_are_blocked_from_and_given_back_the_network() {
        let mut d = data();
        let alpha = crate::ui_tests::proc(100, 0, "alpha.exe", 20.0);
        d.image_paths
            .insert((alpha.pid, alpha.create_time), "C:/Apps/alpha.exe".into());
        d.firewall = Some(Ok(vec!["C:/Apps/beta.exe".into()]));
        let (backend, rx) = Backend::offline(d);
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 900.0])
            .build_eframe(|_| App {
                backend,
                state: UiState {
                    tab: Tab::Network,
                    check_updates: false,
                    ..Default::default()
                },
                tray: None,
                frames: Default::default(),
            });
        h.run_steps(2);
        assert!(h.query_by_label("beta.exe").is_some());
        h.get_by_label("Unblock").click();
        h.run_steps(2);
        let sent: Vec<String> = rx.try_iter().map(describe_command).collect();
        assert!(
            sent.contains(&"firewall C:/Apps/beta.exe false".to_owned()),
            "{sent:?}"
        );

        // From the process menu: its known program file is blocked.
        h.state_mut().state.tab = Tab::Processes;
        h.run_steps(2);
        h.get_by_label("alpha.exe").click_secondary();
        h.run_steps(2);
        h.get_by_label("Block network access").click();
        h.run_steps(3);
        // Asked first: the program stays offline until unblocked.
        assert!(rx.try_iter().next().is_none());
        assert!(
            h.query_by_label_contains("Block network access for alpha.exe?")
                .is_some()
        );
        h.get_all_by_label("Block network access")
            .last()
            .unwrap()
            .click();
        h.run_steps(2);
        let sent: Vec<String> = rx.try_iter().map(describe_command).collect();
        assert!(
            sent.contains(&"firewall C:/Apps/alpha.exe true".to_owned()),
            "{sent:?}"
        );
    }

    #[test]
    fn end_reasons_explain_why_a_process_stays() {
        let base = wtm_core::EndReport {
            deleting: false,
            exited: false,
            threads: 3,
            protection: None,
            critical: false,
            can_end: Some(true),
            open_error: None,
            holders: vec![],
            holders_known: true,
            guards: vec![],
        };
        let texts = |r: &wtm_core::EndReport, elevated| -> Vec<String> {
            end_reasons(r, elevated)
                .into_iter()
                .map(|(_, t)| t)
                .collect()
        };
        assert_eq!(
            texts(&base, true),
            ["Nothing stops it: it can be ended normally."]
        );
        assert!(!end_reasons(&base, true)[0].0, "not a problem");

        let zombie = wtm_core::EndReport {
            deleting: true,
            threads: 0,
            holders: vec![wtm_core::Holder {
                pid: 42,
                name: "backup.exe".into(),
                handles: 2,
            }],
            ..base.clone()
        };
        assert_eq!(
            texts(&zombie, true),
            [
                "Windows is already ending it, but these keep it open: backup.exe (PID 42, 2 handles). Closing them lets it go."
            ]
        );

        let guarded = wtm_core::EndReport {
            can_end: Some(false),
            protection: Some("light protection, Antimalware".into()),
            guards: vec![("vgk.sys".into(), "Riot Vanguard anti-cheat".into())],
            holders_known: false,
            ..base.clone()
        };
        let t = texts(&guarded, false);
        assert_eq!(
            t[0],
            "Windows protects it (light protection, Antimalware); no program may end it."
        );
        assert_eq!(
            t[1],
            "The right to end it is withheld; loaded drivers known to do this: Riot Vanguard anti-cheat."
        );
        assert_eq!(
            t[2],
            "Run as administrator to also see which programs hold it open."
        );

        let denied = wtm_core::EndReport {
            can_end: None,
            open_error: Some("Access is denied.".into()),
            critical: true,
            ..base
        };
        let t = texts(&denied, false);
        assert!(t[0].contains("critical system process"));
        assert!(t[1].ends_with("Restart RustWTM as administrator and try again."));
    }

    #[test]
    fn hung_programs_are_flagged_and_their_wait_chain_shown() {
        use wtm_core::{Chain, Link, LinkStatus, WaitKind, WaitReport};
        let mut d = data();
        d.hung.insert(100, "Alpha - Document".into());
        let alpha = ProcessRef::from(&crate::ui_tests::proc(100, 0, "alpha.exe", 20.0));
        let (backend, rx) = Backend::offline(d);
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 900.0])
            .build_eframe(|_| App {
                backend,
                state: UiState {
                    check_updates: false,
                    ..Default::default()
                },
                tray: None,
                frames: Default::default(),
            });
        h.run_steps(2);
        // The Overview warns and offers to find out why.
        assert!(
            h.query_by_label_contains("alpha.exe is not responding")
                .is_some()
        );
        h.get_by_label("Why?").click();
        h.run_steps(2);
        assert_eq!(h.state().state.tab, Tab::Processes);
        let sent: Vec<String> = rx.try_iter().map(describe_command).collect();
        assert!(sent.contains(&"wait chain 100".to_owned()), "{sent:?}");

        // The details panel shows the result: a deadlock between two threads.
        h.state().backend.data().wait_report = Some((
            alpha.clone(),
            Some(Ok(WaitReport {
                chains: vec![Chain {
                    links: vec![
                        Link::Thread {
                            pid: 100,
                            tid: 7,
                            status: LinkStatus::Blocked,
                        },
                        Link::Object {
                            kind: WaitKind::Mutex,
                            name: "Lock".into(),
                            status: LinkStatus::Owned,
                        },
                        Link::Thread {
                            pid: 100,
                            tid: 8,
                            status: LinkStatus::Blocked,
                        },
                    ],
                    cycle: true,
                }],
                threads: 12,
                analyzed: 2,
                skipped: 1,
            })),
        ));
        h.run_steps(2);
        assert!(
            h.query_by_label("Not responding: \"Alpha - Document\"")
                .is_some()
        );
        assert!(h.query_by_label_contains("Deadlock").is_some());
        assert!(
            h.query_by_label("12 threads, 2 blocked, 1 with a wait chain")
                .is_some()
        );
        assert!(
            h.query_by_label("1 threads could not be analyzed in time")
                .is_some()
        );
        assert!(
            h.query_by_label(
                "thread 7 (PID 100) blocked  →  mutex \"Lock\"  →  thread 8 (PID 100) blocked"
            )
            .is_some()
        );
        h.get_by_label("Analyze wait chain").click();
        h.run_steps(2);
        assert!(
            rx.try_iter()
                .map(describe_command)
                .any(|c| c == "wait chain 100")
        );

        // While an analysis runs the button waits.
        h.state().backend.data().wait_report = Some((alpha, None));
        h.run_steps(2);
        assert!(h.query_by_label("Analyzing…").is_some());
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
                    // Its command list is compared exactly.
                    check_updates: false,
                    ..Default::default()
                },
                tray: None,
                frames: Default::default(),
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
        // A selection also loads the ports for the details panel.
        assert_eq!(
            s,
            [
                "inspect 100",
                "load Network",
                "inspect 100",
                "control 100 Suspend"
            ]
        );

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
        // Stopping, restarting and disabling are asked first.
        assert!(
            h.query_by_label_contains("Stop the service \"Print Spooler\"?")
                .is_some()
        );
        h.get_all_by_label("Stop").last().unwrap().click();
        step(&mut h, &mut sent);
        h.get_by_label("Restart").click();
        step(&mut h, &mut sent);
        h.get_by_label("Cancel").click();
        step(&mut h, &mut sent);
        h.get_by_label("Startup type").click();
        step(&mut h, &mut sent);
        h.get_by_label("Disabled").click();
        step(&mut h, &mut sent);
        h.get_all_by_label("Disable").last().unwrap().click();
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
                frames: Default::default(),
            });
        h.run_steps(2);
        nav_to(&mut h, "Settings");
        h.run_steps(2);
        assert!(
            h.query_by_label("Pop up when the CPU stays above")
                .is_some()
        );
        h.get_by_label("Service settings").click();
        h.run_steps(2);
        assert!(h.query_by_label_contains("Valid configuration").is_some());
        assert!(h.query_by_label_contains("unsaved").is_none());

        h.get_by_label("Enforcing").click();
        h.run_steps(2);
        assert!(h.query_by_label_contains("unsaved").is_some());
        h.get_by_label("Slow down the busiest apps when the CPU is overloaded")
            .click();
        h.run_steps(2);
        h.get_by_label("Keep one core free for RustWTM when the CPU is overloaded")
            .click();
        h.run_steps(2);
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
        let parsed = wtm_policy::Config::from_toml(&saved).unwrap();
        assert!(!parsed.dry_run && !parsed.governor.enabled);
        assert!(!parsed.reserve.rescue_core);
        assert!(
            saved.contains("# Log what would be done"),
            "comments are kept"
        );

        // Discarding asks the service for its text again.
        h.get_by_label("Discard changes").click();
        h.run_steps(2);
        assert!(
            rx.try_iter()
                .any(|c| matches!(c, UiCommand::Load(Panel::Config)))
        );
    }

    #[test]
    fn settings_config_file_page_fills_the_window() {
        let mut d = data();
        d.config = Some(Ok((
            "config.toml".into(),
            wtm_policy::DEFAULT_CONFIG_TOML.into(),
        )));
        let (backend, _rx) = Backend::offline(d);
        let state = UiState {
            tab: Tab::Settings,
            ..Default::default()
        };
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1000.0, 640.0])
            .build_eframe(|_| App {
                backend,
                state,
                tray: None,
                frames: Default::default(),
            });
        h.run_steps(2);
        h.get_by_label("config.toml").click();
        h.run_steps(2);
        let editor = h.get_by_role(egui::accesskit::Role::MultilineTextInput);
        let rect = editor.rect();
        assert!(rect.min.y < 300.0, "editor starts near the top: {rect:?}");
        assert!(rect.height() > 300.0, "and fills the page: {rect:?}");
        assert!(
            h.query_by_label("More below").is_some(),
            "the long default config continues below"
        );
    }

    #[test]
    fn settings_tab_shows_errors_and_needs_admin_to_save() {
        let mut d = data();
        d.config = Some(Err("service not running".into()));
        let (backend, rx) = Backend::offline(d);
        let mut state = UiState {
            tab: Tab::Settings,
            ..Default::default()
        };
        state.config.page = crate::settings::Page::Service;
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 900.0])
            .build_eframe(|_| App {
                backend,
                state,
                tray: None,
                frames: Default::default(),
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
                frames: Default::default(),
            });
        h.run_steps(2);
        assert!(h.query_by_label_contains("2 cores, 8 GB").is_some());
        assert!(h.query_by_label("1 (1 outside)").is_some());
        // 3.5% of 8 CPUs against a 2-core budget; 1 GiB of 8 GB.
        assert!(h.query_by_label_contains("0.28 cores (14%)").is_some());
        assert!(h.query_by_label_contains("1.0 GiB (12%)").is_some());
        assert!(h.query_by_label_contains("Dry-run mode").is_some());
        // What was saved, loaded back as the service does after saving.
        let saved = |h: &mut Harness<'_, App>, rx: &std::sync::mpsc::Receiver<UiCommand>| {
            let text = rx.try_iter().find_map(|c| match c {
                UiCommand::SaveConfig(t) => Some(t),
                _ => None,
            })?;
            h.state().backend.data().config = Some(Ok(("config.toml".into(), text.clone())));
            h.run_steps(2);
            Some(wtm_policy::Config::from_toml(&text).unwrap())
        };

        h.get_by_label("Edit").click();
        h.run_steps(2);
        assert!(h.query_by_label("Change limit").is_some());
        h.get_by_label("Each instance separately").click();
        h.run_steps(1);
        h.get_by_label("Save limit").click();
        h.run_steps(2);
        let c = saved(&mut h, &rx).expect("saved");
        assert_eq!(c.limits.len(), 1);
        assert_eq!(c.limits[0].scope, wtm_policy::LimitScope::Each);
        // Loaded with the change: the form is done.
        assert!(h.query_by_label("Change limit").is_none());

        h.get_by_label("Remove").click();
        h.run_steps(2);
        assert!(saved(&mut h, &rx).expect("saved").limits.is_empty());

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
        let c = saved(&mut h, &rx).expect("saved");
        assert_eq!(c.limits.len(), 1);
        assert_eq!(c.limits[0].name, "node.exe");

        // Back to the first configuration.
        h.state().backend.data().config = Some(Ok(("config.toml".into(), text.into())));
        h.run_steps(2);

        // Picking a running app fills in its name; the existing entry is replaced.
        h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "or a running app")
            .click();
        h.run_steps(2);
        h.get_by_label_contains("beta.exe  (").click();
        h.run_steps(2);
        h.get_by_label("Save limit").click();
        h.run_steps(2);
        let c = saved(&mut h, &rx).expect("saved");
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
                    frames: Default::default(),
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
                frames: Default::default(),
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

        // Quick actions on the busiest app, behind its "⋯" menu.
        h.get_all_by_label("⋯").next().unwrap().click();
        h.run_steps(2);
        h.get_by_label("Efficiency mode").click();
        h.run_steps(2);
        assert!(rx.try_iter().any(|c| matches!(
            c,
            UiCommand::Control {
                command: Command::EcoQos(wtm_core::control::EcoQos::On),
                ..
            }
        )));
        h.get_all_by_label("⋯").next().unwrap().click();
        h.run_steps(2);
        h.get_by_label("Free memory").click();
        h.run_steps(2);
        assert!(rx.try_iter().any(|c| matches!(
            c,
            UiCommand::Control {
                command: Command::TrimMemory,
                ..
            }
        )));
        assert!(
            h.query_by_label_contains("1 process limited right now")
                .is_some()
        );
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
        // The Overview button only, not the one in the status bar.
        d.elevated = true;
        let (mut h, rx) = overview_app(d);
        assert!(h.query_by_label_contains("Protection is on").is_some());
        assert!(h.query_by_label_contains("Custom").is_some());
        assert!(
            h.query_by_label("Changing it needs administrator rights.")
                .is_some()
        );
        rx.try_iter().for_each(drop);
        h.get_by_label("Restart as administrator").click();
        h.run_steps(2);
        assert!(
            rx.try_iter()
                .any(|c| matches!(c, UiCommand::RestartElevated))
        );
        h.get_all_by_label("⋯").next().unwrap().click();
        h.run_steps(2);
        h.get_by_label("Limit…").click();
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
    fn delete_in_search_does_not_offer_to_end_the_selected_process() {
        let (mut h, _rx) = kill_app(Tab::Processes);
        h.state_mut().state.processes.selected = Some(ProcessRef::from(&crate::ui_tests::proc(
            100,
            4,
            "alpha.exe",
            20.0,
        )));
        h.run_steps(2);
        // Control: Delete from the table still asks before ending anything.
        h.key_press(egui::Key::Delete);
        h.run_steps(2);
        assert!(h.state().state.confirm.is_some());
        h.get_by_label("Cancel").click();
        h.run_steps(2);

        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
        h.run_steps(2);
        h.key_press(egui::Key::Delete);
        h.run_steps(2);
        assert!(
            h.state().state.confirm.is_none(),
            "Delete belongs to the search field while editing"
        );
    }

    #[test]
    fn ctrl_tab_goes_through_the_pages_and_escape_closes_the_details() {
        let (mut h, _rx) = kill_app(Tab::Processes);
        h.run();
        h.get_by_label("alpha.exe").click();
        h.run_steps(2);
        assert!(h.state().state.processes.selected.is_some());
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        assert!(h.state().state.processes.selected.is_none());
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Tab);
        h.run_steps(2);
        assert_eq!(h.state().state.tab, Tab::Performance);
        h.key_press_modifiers(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::Tab,
        );
        h.run_steps(2);
        assert_eq!(h.state().state.tab, Tab::Processes);
        // From the first page back to the last.
        h.state_mut().state.tab = Tab::Overview;
        h.key_press_modifiers(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::Tab,
        );
        h.run_steps(2);
        assert_eq!(h.state().state.tab, Tab::About);
    }

    #[test]
    fn events_filter_by_level_and_stale_data_is_flagged_in_the_status_bar() {
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1000.0, 600.0])
            .build_ui_state(
                |ui, (d, level): &mut (Data, Option<EventLevel>)| {
                    events(ui, d, level);
                    mode_badge(
                        ui,
                        Mode::Service {
                            admin: true,
                            verified: true,
                        },
                        crate::backend::ServiceProblem::NotRunning,
                        Some(Some(Duration::from_secs(42))),
                    );
                },
                (data(), None),
            );
        h.run();
        let total = h.state().0.events.len().to_string();
        assert!(
            h.query_by_label(&format!("{total} of {total} events"))
                .is_some()
        );
        assert!(h.query_by_label("● Service · no data for 42 s").is_some());
        h.get_by_label("Errors").click();
        h.run();
        assert_eq!(h.state().1, Some(EventLevel::Error));
        assert!(h.query_by_label("No events of this kind").is_some());
        h.get_by_label("All events").click();
        h.run();
        assert!(h.query_by_label("No events of this kind").is_none());
    }

    #[test]
    fn process_details_can_be_closed_and_themes_can_be_changed() {
        let (mut h, _rx) = kill_app(Tab::Processes);
        h.run();
        h.get_by_label("alpha.exe").click();
        h.run_steps(2);
        assert!(h.state().state.processes.selected.is_some());
        h.get_by_label("Close details").click();
        h.run_steps(2);
        assert!(h.state().state.processes.selected.is_none());
        nav_to(&mut h, "Settings");
        h.run_steps(2);
        h.get_by_label("Light").click();
        h.run_steps(2);
        assert_eq!(h.ctx.theme(), egui::Theme::Light);
        h.get_by_label("Dark").click();
        h.run_steps(2);
        assert_eq!(h.ctx.theme(), egui::Theme::Dark);
        h.get_by_label("System").click();
        h.run_steps(2);
        assert_eq!(
            h.ctx.options(|o| o.theme_preference),
            egui::ThemePreference::System
        );
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
        // Painted icons retain their full accessible names when collapsed.
        nav_to(&mut h, "Settings");
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
                frames: Default::default(),
            });
        let mut h = h;
        h.run_steps(2);
        assert!(
            h.query_by_label("»").is_some(),
            "narrow windows start collapsed"
        );
    }

    #[test]
    fn language_picker_changes_immediately_and_keeps_the_old_choice_on_save_failure() {
        use crate::i18n::{Language, scope};
        let mut h = Harness::builder().allow_missing_glyphs().build_ui_state(
            |ui, state: &mut UiState| {
                let _language = scope(state.language);
                language_settings(ui, state, |language| {
                    if language == Language::French {
                        Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "denied",
                        ))
                    } else {
                        Ok(())
                    }
                });
            },
            UiState {
                language: Language::English,
                ..UiState::default()
            },
        );
        h.get_by_role(egui::accesskit::Role::ComboBox).click();
        h.run();
        h.get_by_label("Türkçe").click();
        h.run();
        assert_eq!(h.state().language, Language::Turkish);
        assert!(h.query_by_label("Dil").is_some());
        h.get_by_role(egui::accesskit::Role::ComboBox).click();
        h.run();
        h.get_by_label("Français").click();
        h.run();
        assert_eq!(h.state().language, Language::Turkish);
        assert!(h.query_by_label("Dil kaydedilemedi: denied").is_some());
    }

    #[test]
    fn every_page_renders_in_all_five_languages() {
        use crate::i18n::{Language, scope};
        for language in [
            Language::English,
            Language::Turkish,
            Language::German,
            Language::French,
            Language::Spanish,
        ] {
            let _language = scope(language);
            let mut d = data();
            d.hardware = crate::sensors::fixture();
            let (backend, _rx) = Backend::offline(d);
            let mut h = Harness::builder()
                .allow_missing_glyphs()
                .with_size([1320.0, 900.0])
                .build_eframe(|cc| {
                    crate::theme::install_fonts(&cc.egui_ctx);
                    App {
                        backend,
                        state: UiState {
                            language,
                            autostart: Some(false),
                            check_updates: false,
                            ..UiState::default()
                        },
                        tray: None,
                        frames: Default::default(),
                    }
                });
            for (tab, _, _) in NAV {
                h.state_mut().state.tab = tab;
                h.run_steps(2);
                assert!(
                    h.query_by_label(tab.subtitle()).is_some(),
                    "{language:?} {tab:?}"
                );
                let received = crate::i18n::tr("Data measured {0}s ago");
                assert!(
                    h.query_by_label_contains(received.split("{0}").next().unwrap())
                        .is_some()
                );
            }
            h.state_mut().backend.data().sample_error =
                Some("The service has not supplied a sample.".into());
            h.run_steps(2);
            assert!(
                h.query_by_label(crate::i18n::tr("Monitoring data is out of date"))
                    .is_some()
            );
            assert!((200.0..=280.0).contains(&nav_width(&h.ctx)));
        }
    }

    /// Reproducible product screenshots, without connecting to or controlling
    /// the host. Run with `--features visual-tests render_release_review`.
    #[cfg(feature = "visual-tests")]
    #[test]
    fn render_release_review() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/visual-review");
        // `WTM_REVIEW_LANGS=en,de` renders only those languages.
        let only = std::env::var("WTM_REVIEW_LANGS").ok();
        for (code, language) in [
            ("en", crate::i18n::Language::English),
            ("tr", crate::i18n::Language::Turkish),
            ("de", crate::i18n::Language::German),
            ("fr", crate::i18n::Language::French),
            ("es", crate::i18n::Language::Spanish),
        ] {
            if only
                .as_ref()
                .is_some_and(|o| !o.split(',').any(|c| c == code))
            {
                continue;
            }
            let _language = crate::i18n::scope(language);
            let out = root.join("locales").join(code);
            std::fs::create_dir_all(&out).unwrap();
            for (theme_name, theme) in [("dark", egui::Theme::Dark), ("light", egui::Theme::Light)]
            {
                for (size_name, size) in [("wide", [1440.0, 1000.0]), ("compact", [820.0, 500.0])] {
                    for tab in [
                        Tab::Overview,
                        Tab::Processes,
                        Tab::Performance,
                        Tab::Disks,
                        Tab::Sensors,
                        Tab::Settings,
                        Tab::Governor,
                        Tab::Limits,
                        Tab::Events,
                        Tab::Services,
                        Tab::Startup,
                        Tab::Users,
                        Tab::Network,
                        Tab::Ports,
                        Tab::History,
                        Tab::KillList,
                    ] {
                        if std::env::var("WTM_REVIEW_TABS").ok().is_some_and(|only| {
                            !only.split(',').any(|name| name == format!("{tab:?}"))
                        }) {
                            continue;
                        }
                        let mut d = data();
                        d.hardware = crate::sensors::fixture();
                        d.cpu_topology = Some(crate::ui_tests::hybrid_topology());
                        let sensors = crate::sensors::SensorsView::review_fixture(&d.hardware);
                        d.mode = Mode::Service {
                            admin: true,
                            verified: true,
                        };
                        d.elevated = true;
                        d.config = Some(Ok((
                            "config.toml".into(),
                            wtm_policy::DEFAULT_CONFIG_TOML.into(),
                        )));
                        d.telegram = Some(Ok(wtm_policy::TelegramConfig::default()));
                        let mut snap = d.snapshot.as_deref().unwrap().clone();
                        snap.system.core_percent =
                            vec![95.0, 70.0, 10.0, 0.0, 45.0, 8.0, 14.0, 4.0];
                        d.history.cores = snap
                            .system
                            .core_percent
                            .iter()
                            .map(|&load| {
                                (0..crate::backend::CORE_HISTORY_LEN)
                                    .map(|i| {
                                        (load + (i as f32 * 0.2).sin() * 5.0).clamp(0.0, 100.0)
                                    })
                                    .collect()
                            })
                            .collect();
                        d.snapshot = Some(std::sync::Arc::new(snap));
                        // Give every graph a full, deterministic history for review.
                        for i in 0..HISTORY_LEN {
                            d.history
                                .cpu
                                .push_back(30.0 + (i as f32 * 0.09).sin() * 18.0);
                            d.history
                                .memory
                                .push_back(50.0 + (i as f32 * 0.02).sin() * 3.0);
                            d.history
                                .gpu
                                .push_back(12.0 + (i as f32 * 0.12).sin() * 8.0);
                            d.history
                                .io_read
                                .push_back(1000.0 + (i as f32 * 0.07).sin() * 800.0);
                            d.history.io_write.push_back(400.0);
                            d.history
                                .net_rx
                                .push_back(2048.0 + (i as f32 * 0.1).sin() * 1800.0);
                            d.history.net_tx.push_back(512.0);
                        }
                        let (backend, _rx) = Backend::offline(d);
                        let mut h = Harness::builder()
                            .allow_missing_glyphs()
                            .with_size(size)
                            .with_theme(theme)
                            .build_eframe(|cc| {
                                crate::theme::install_fonts(&cc.egui_ctx);
                                crate::theme::apply(&cc.egui_ctx);
                                cc.egui_ctx.set_theme(theme);
                                App {
                                    backend,
                                    state: UiState {
                                        tab,
                                        language,
                                        sensors,
                                        autostart: Some(false),
                                        check_updates: false,
                                        disks: crate::disks_tab::review_fixture(),
                                        ..UiState::default()
                                    },
                                    tray: None,
                                    frames: Default::default(),
                                }
                            });
                        h.run_steps(3);
                        assert_eq!(
                            h.ctx.global_style().visuals.dark_mode,
                            theme == egui::Theme::Dark
                        );
                        // Short windows leave the subtitle out.
                        assert_eq!(
                            h.query_by_label(tab.subtitle()).is_some(),
                            size[1] >= COMPACT_HEIGHT
                        );
                        let path = out.join(format!("{theme_name}-{size_name}-{tab:?}.png"));
                        h.render().expect("GPU render").save(&path).unwrap();
                        println!("Rendered {}", path.display());
                        if tab == Tab::Processes {
                            let target = {
                                let mut data = h.state().backend.data();
                                let mut snap = data.snapshot.as_deref().unwrap().clone();
                                snap.processes = (0..80)
                                    .map(|i| {
                                        crate::ui_tests::proc(
                                            1000 + i,
                                            0,
                                            &format!("app-{i:03}.exe"),
                                            1.0,
                                        )
                                    })
                                    .collect();
                                snap.system.process_count = snap.processes.len();
                                let target = ProcessRef::from(&snap.processes[60]);
                                data.snapshot = Some(std::sync::Arc::new(snap));
                                data.revision = data.revision.wrapping_add(1);
                                data.details = None;
                                target
                            };
                            let state = &mut h.state_mut().state.processes;
                            state.sort = crate::processes::Col::Pid;
                            state.descending = false;
                            state.selected = Some(target);
                            h.run_steps(20);
                            assert!(
                                h.query_all_by_label("app-060.exe").any(|node| {
                                    let rect = node.rect();
                                    rect.min.x < size[0] * 0.5
                                        && rect.min.y >= 0.0
                                        && rect.max.y < size[1]
                                }),
                                "selected row is visible in the table, beside the details panel"
                            );
                            let path = out
                                .join(format!("{theme_name}-{size_name}-ProcessesSelection.png"));
                            h.render().expect("GPU render").save(&path).unwrap();
                            println!("Rendered {}", path.display());
                        }
                        if tab == Tab::Users {
                            {
                                let mut data = h.state().backend.data();
                                let template =
                                    data.sessions.as_ref().unwrap().as_ref().unwrap()[1].clone();
                                data.sessions = Some(Ok((0..24)
                                    .map(|i| {
                                        let mut session = template.clone();
                                        session.id = 100 + i;
                                        session.user = format!("abcdefghijklmnop{i:04}");
                                        session.domain = "ABCDEFGHIJKLMNO".into();
                                        session.station = format!("rdp-tcp#{i}");
                                        session
                                    })
                                    .collect()));
                            }
                            h.run_steps(3);
                            h.event(egui::Event::PointerMoved(egui::pos2(400.0, 200.0)));
                            h.event(egui::Event::MouseWheel {
                                unit: egui::MouseWheelUnit::Point,
                                delta: egui::vec2(-3000.0, 0.0),
                                modifiers: egui::Modifiers::NONE,
                                phase: egui::TouchPhase::Move,
                                source: egui::MouseWheelSource::Unknown,
                            });
                            h.run_steps(20);
                            h.event(egui::Event::MouseWheel {
                                unit: egui::MouseWheelUnit::Point,
                                delta: egui::vec2(0.0, -3000.0),
                                modifiers: egui::Modifiers::NONE,
                                phase: egui::TouchPhase::Move,
                                source: egui::MouseWheelSource::Unknown,
                            });
                            h.run_steps(20);
                            let path =
                                out.join(format!("{theme_name}-{size_name}-UsersSessions.png"));
                            h.render().expect("GPU render").save(&path).unwrap();
                            println!("Rendered {}", path.display());
                        }
                        if tab == Tab::Overview {
                            {
                                let mut data = h.state().backend.data();
                                data.elevated = false;
                                data.mode = Mode::Local;
                                data.message = Some((
                                    EventLevel::Warn,
                                    "The requested process could not be controlled. ".repeat(12),
                                ));
                            }
                            h.run_steps(3);
                            let path =
                                out.join(format!("{theme_name}-{size_name}-StatusMessage.png"));
                            h.render().expect("GPU render").save(&path).unwrap();
                            println!("Rendered {}", path.display());
                        }
                        if tab == Tab::Settings {
                            h.state_mut().state.config.page = crate::settings::Page::File;
                            h.run_steps(2);
                            let path =
                                out.join(format!("{theme_name}-{size_name}-SettingsFile.png"));
                            h.render().expect("GPU render").save(&path).unwrap();
                        }
                        if tab == Tab::Performance {
                            {
                                let mut data = h.state().backend.data();
                                data.sample_received_at =
                                    Some(Instant::now() - Duration::from_secs(42));
                                data.sample_error =
                                    Some("The service has not supplied a sample.".into());
                            }
                            h.run_steps(2);
                            assert!(
                                h.query_by_label(crate::i18n::tr("Monitoring data is out of date"))
                                    .is_some()
                            );
                            let path =
                                out.join(format!("{theme_name}-{size_name}-MonitoringStale.png"));
                            h.render().expect("GPU render").save(&path).unwrap();
                        }
                    }
                }
            }
            // Keep native GPU review contexts in one test; parallel review tests
            // exited with a native access violation on the development machine.
            crate::power_controls::tests::render_power_review(&out);
        }
    }

    #[test]
    fn monitoring_health_shows_waiting_age_failure_and_recovery_in_all_languages() {
        use crate::i18n::{Language, scope};
        for language in [
            Language::English,
            Language::Turkish,
            Language::German,
            Language::French,
            Language::Spanish,
        ] {
            let now = Instant::now();
            let mut h = Harness::builder()
                .allow_missing_glyphs()
                .with_size([760.0, 150.0])
                .build_ui_state(
                    move |ui, state: &mut (Data, Instant)| {
                        let _language = scope(language);
                        data_age(ui, &state.0, state.1);
                        stale_banner(ui, &state.0, state.1);
                    },
                    (Data::default(), now),
                );
            let _language = scope(language);
            assert!(
                h.query_by_label(crate::i18n::tr("Waiting for monitoring data…"))
                    .is_some()
            );
            h.state_mut().0 = data();
            h.state_mut().0.sample_received_at = Some(now);
            h.state_mut().1 = now + Duration::from_secs(2);
            h.run();
            assert!(
                h.query_by_label(&crate::i18n::format(
                    "Data measured {0}s ago",
                    &["2".into()]
                ))
                .is_some()
            );
            assert!(
                h.query_by_label(crate::i18n::tr("Monitoring data is out of date"))
                    .is_none()
            );
            h.state_mut().1 = now + Duration::from_secs(6);
            h.run();
            assert!(
                h.query_by_label(crate::i18n::tr("Monitoring data is out of date"))
                    .is_some()
            );
            h.state_mut().0.sample_received_at = Some(now + Duration::from_secs(6));
            h.state_mut().0.sample_error = Some("native diagnostic".into());
            h.run();
            assert!(h.query_by_label("native diagnostic").is_some());
            assert!(
                h.query_by_label(crate::i18n::tr("Monitoring data is out of date"))
                    .is_some()
            );
            h.state_mut().0.sample_error = None;
            h.run();
            assert!(
                h.query_by_label(crate::i18n::tr("Monitoring data is out of date"))
                    .is_none()
            );
        }
    }

    #[test]
    fn performance_labels_relative_cpu_tiers_and_omits_ambiguous_group_labels() {
        let mut d = data();
        d.cpu_topology = Some(crate::ui_tests::hybrid_topology());
        let mut snap = d.snapshot.as_deref().unwrap().clone();
        snap.system.core_percent = vec![10.0; 8];
        d.snapshot = Some(std::sync::Arc::new(snap));
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1200.0, 1600.0])
            .build_ui_state(|ui, d: &mut Data| performance(ui, d), d);
        h.run();
        assert!(h.query_by_label_contains("CPU 0 · P:").is_some());
        assert!(h.query_by_label_contains("CPU 7 · LP-E:").is_some());
        h.state_mut().cpu_topology.as_mut().unwrap().cpus[7].group = 1;
        h.run();
        assert!(h.query_by_label_contains("CPU 0 · P:").is_none());
        for c in &mut h.state_mut().cpu_topology.as_mut().unwrap().cpus {
            c.group = 0;
            c.efficiency_class = 0;
        }
        h.run();
        assert!(h.query_by_label_contains("CPU 0 ·").is_none());
        assert!(h.query_by_label_contains("CPU 0:").is_some());
    }
}

/// Frame cost with a busy machine's worth of data, per page. Run with
/// `cargo test -p wtm-gui --release frame_cost -- --ignored --nocapture`.
#[cfg(test)]
mod bench {
    use super::*;
    use egui_kittest::Harness;

    use crate::backend::Backend;
    use crate::ui_tests::{data, proc};
    use std::sync::Arc;

    /// The fixed process-list sizes measured (see docs/0.13.0-review.md).
    const SIZES: [u32; 2] = [1_000, 10_000];
    const FRAMES: u32 = 100;

    fn busy_data(processes: u32) -> Data {
        let mut d = data();
        let mut snap = d.snapshot.as_deref().unwrap().clone();
        snap.processes = (0..processes)
            .map(|i| {
                let pid = 1000 + i * 4;
                let parent = if i < 50 { 4 } else { 1000 + (i % 50) * 4 };
                let mut p = proc(
                    pid,
                    parent,
                    &format!("app{}.exe", i % 300),
                    (i % 97) as f32 / 10.0,
                );
                p.io_read_rate = f64::from(i % 13) * 4096.0;
                p.session_id = i % 3;
                p
            })
            .collect();
        for p in &snap.processes {
            let key = (p.pid, p.create_time);
            let path = format!(r"C:\Program Files\Vendor {}\{}", p.pid % 40, p.name);
            d.command_lines
                .insert(key, format!("\"{path}\" --instance {} --flag", p.pid));
            d.image_paths.insert(key, path);
        }
        d.connections = Some(Ok((0..600u16)
            .map(|i| wtm_core::Connection {
                protocol: if i % 3 == 0 {
                    wtm_core::Protocol::Udp
                } else {
                    wtm_core::Protocol::Tcp
                },
                local: std::net::SocketAddr::from(([127, 0, 0, 1], 10_000 + i)),
                remote: (i % 2 == 0)
                    .then(|| std::net::SocketAddr::from(([10, 0, 0, (i % 250) as u8], 443))),
                state: (i % 3 != 0).then_some(if i % 2 == 0 {
                    wtm_core::TcpState::Established
                } else {
                    wtm_core::TcpState::Listen
                }),
                pid: 1000 + u32::from(i) * 4,
            })
            .collect()));
        for i in 0..200 {
            d.events.push_back(wtm_ipc::Event {
                id: i + 2,
                unix_ms: 1_700_000_000_000 + i * 1000,
                level: EventLevel::Info,
                pid: Some(1000),
                name: Some("app0.exe".into()),
                message: format!("Custom note number {i} from a rule"),
                notify: i % 2 == 0,
            });
        }
        d.snapshot = Some(Arc::new(snap));
        d
    }

    /// Median frame time, which other load on the machine skews less than
    /// the mean. With `fresh`, every frame sees a new data revision,
    /// as when a sample arrives: nothing cached from the previous frame holds.
    fn per_frame(
        processes: u32,
        fresh: bool,
        language: crate::i18n::Language,
        setup: impl FnOnce(&mut UiState),
    ) -> Duration {
        let (backend, _rx) = Backend::offline(busy_data(processes));
        let mut state = UiState {
            language,
            ..Default::default()
        };
        setup(&mut state);
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1400.0, 900.0])
            .build_eframe(move |_| App {
                backend,
                state,
                tray: None,
                frames: Default::default(),
            });
        for _ in 0..10 {
            h.step();
        }
        let mut frames: Vec<Duration> = (0..FRAMES)
            .map(|_| {
                if fresh {
                    let mut data = h.state_mut().backend.data();
                    data.revision = data.revision.wrapping_add(1);
                }
                let start = Instant::now();
                h.step();
                start.elapsed()
            })
            .collect();
        frames.sort_unstable();
        frames[frames.len() / 2]
    }

    #[test]
    #[ignore = "benchmark"]
    fn frame_cost() {
        use crate::i18n::Language::{English, Turkish};
        use crate::processes::ViewMode;
        type Case = (&'static str, crate::i18n::Language, fn(&mut UiState));
        let cases: [Case; 10] = [
            ("Overview", English, |s| s.tab = Tab::Overview),
            ("Processes list", English, |s| s.tab = Tab::Processes),
            ("Processes list, by name", English, |s| {
                s.tab = Tab::Processes;
                s.processes.sort = crate::processes::Col::Name;
            }),
            ("Processes search", English, |s| {
                s.tab = Tab::Processes;
                s.processes.search = "vendor 1".into();
            }),
            ("Processes tree", English, |s| {
                s.tab = Tab::Processes;
                s.processes.mode = ViewMode::Tree;
            }),
            ("Processes apps", English, |s| {
                s.tab = Tab::Processes;
                s.processes.mode = ViewMode::Apps;
            }),
            ("Network", English, |s| s.tab = Tab::Network),
            ("Ports", English, |s| s.tab = Tab::Ports),
            ("Disks", English, |s| s.tab = Tab::Disks),
            ("Events (Turkish)", Turkish, |s| s.tab = Tab::Events),
        ];
        for processes in SIZES {
            println!("{processes} processes:");
            for (name, language, setup) in cases {
                let still = per_frame(processes, false, language, setup);
                let fresh = per_frame(processes, true, language, setup);
                println!(
                    "  {name:<28} {:>9.1} us/frame, {:>9.1} with new data",
                    still.as_secs_f64() * 1e6,
                    fresh.as_secs_f64() * 1e6
                );
            }
        }
    }
}

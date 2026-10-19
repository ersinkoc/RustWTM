use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use egui_plot::{Line, Plot, PlotPoints};
use wtm_core::control::ProcessRef;
use wtm_core::{Snapshot, format_bytes};
use wtm_ipc::{Command, EventLevel};

use crate::backend::{Backend, Data, HISTORY_LEN, Mode, Panel, ServiceOp, UiCommand, describe};
use crate::processes::{Intent, ManagedMap, ProcessView, context_menu};
use crate::tray::Tray;
use crate::views;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Processes,
    Performance,
    Governor,
    Events,
    History,
    Startup,
    Users,
    Services,
    Network,
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
    network_filter: String,
    services_filter: String,
    selected_startup: Option<wtm_core::startup::StartupEntry>,
    history: views::HistoryView,
    history_dirty: bool,
    selected_service: Option<String>,
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
            state: UiState {
                tab: Tab::Processes,
                processes: ProcessView::default(),
                confirm: None,
                network_filter: String::new(),
                services_filter: String::new(),
                selected_startup: None,
                history: views::HistoryView::default(),
                history_dirty: true,
                selected_service: None,
                last_load: HashMap::new(),
            },
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

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (tab, label) in [
                    (Tab::Processes, "Processes"),
                    (Tab::Performance, "Performance"),
                    (Tab::Governor, "Governor"),
                    (Tab::History, "App history"),
                    (Tab::Startup, "Startup apps"),
                    (Tab::Users, "Users"),
                    (Tab::Services, "Services"),
                    (Tab::Network, "Network"),
                    (Tab::Events, "Events"),
                ] {
                    ui.selectable_value(&mut state.tab, tab, RichText::new(label).size(15.0));
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

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| match &data.message {
                Some((level, text)) => {
                    ui.label(RichText::new(text).color(level_color(*level)));
                }
                None => {
                    ui.weak("Right-click a process for actions.");
                }
            });
        });

        let managed: ManagedMap<'_> = data
            .status
            .iter()
            .flat_map(|s| &s.managed)
            .map(|m| ((m.target.pid, m.target.create_time), m))
            .collect();

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
                Some(snap) => intents.extend(state.processes.ui(ui, snap, &managed)),
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
            }
            Tab::Startup => {
                if let Some(cmd) = views::startup(ui, &data, &mut state.selected_startup) {
                    service_cmds.push(cmd);
                }
            }
            Tab::Users => views::users(ui, &data),
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
        && let Some(path) = &d.path
    {
        ui.add_space(6.0);
        ui.label(RichText::new(path).small().weak());
    }

    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        if ui.button(RichText::new("End task").color(BAD)).clicked() {
            intents.push(Intent::Confirm(sel.clone(), Command::Kill));
        }
        ui.menu_button("More actions", |ui| {
            context_menu(ui, sel, snap.system.logical_cpus, intents)
        });
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

fn performance(ui: &mut Ui, data: &Data) {
    let Some(snap) = &data.snapshot else {
        ui.spinner();
        return;
    };
    let s = &snap.system;
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.heading(format!("CPU  {:.1}%", s.cpu_percent));
        history_plot(ui, "cpu_plot", "CPU", &data.history.cpu);
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
    egui::Grid::new("managed_grid")
        .num_columns(4)
        .striped(true)
        .show(ui, |ui| {
            ui.strong("PID");
            ui.strong("Name");
            ui.strong("Source");
            ui.strong("Effective limits");
            ui.end_row();
            for m in &status.managed {
                ui.label(m.target.pid.to_string());
                ui.label(&m.target.name);
                let sources: Vec<String> = m.sources.iter().map(|s| format!("{s:?}")).collect();
                ui.label(sources.join(", "));
                ui.label(&m.summary);
                ui.end_row();
            }
        });
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

    use crate::ui_tests::data;

    #[test]
    fn full_app_navigates_every_tab() {
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1300.0, 850.0])
            .build_eframe(|cc| App::new(cc));
        for _ in 0..100 {
            h.step();
            if h.state().backend.data().snapshot.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            h.state().backend.data().snapshot.is_some(),
            "no snapshot from backend"
        );

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
            h.get_by_label(tab).click();
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
    }

    #[test]
    fn uptime_formats_days_and_hours() {
        assert_eq!(uptime(None), "n/a");
        let start = SystemTime::now() - Duration::from_secs(2 * 86_400 + 3 * 3600 + 4 * 60);
        assert!(uptime(Some(start)).starts_with("2d 03h 04m"));
        let start = SystemTime::now() - Duration::from_secs(3661);
        assert!(uptime(Some(start)).starts_with("1h 01m 0"));
    }
}

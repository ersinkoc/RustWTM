//! Container monitoring: bounded lists, per-container graphs and read-only logs.
use crate::theme;
use eframe::egui::{self, RichText, Ui};
use egui_extras::Column;
use std::time::Instant;
use wtm_core::docker::{Container, State};

#[derive(Default)]
pub struct View {
    pub selected: Option<String>,
    search: String,
    filter: u8,
    project: String,
}
fn value(n: Option<f64>) -> String {
    n.map(wtm_core::format_bytes).unwrap_or_else(|| "—".into())
}
fn rate(n: Option<f64>) -> String {
    n.map(|n| format!("{}/s", wtm_core::format_bytes(n)))
        .unwrap_or_else(|| "—".into())
}
fn fresh(c: &Container, state: &State, now: Instant) -> bool {
    state.error.is_none()
        && c.metrics_error.is_none()
        && c.state == "running"
        && c.metrics.fresh(now)
}
fn state_label(state: &str) -> &str {
    match state {
        "running" => crate::i18n::tr("Running"),
        "exited" => crate::i18n::tr("Stopped"),
        "paused" => crate::i18n::tr("Paused"),
        "restarting" => crate::i18n::tr("Restarting"),
        "created" => crate::i18n::tr("Created"),
        "dead" => crate::i18n::tr("Dead"),
        "healthy" => crate::i18n::tr("Healthy"),
        "unhealthy" => crate::i18n::tr("Unhealthy"),
        "starting" => crate::i18n::tr("Starting"),
        _ => state,
    }
}

pub fn overview(ui: &mut Ui, state: &State) -> bool {
    let Some(report) = &state.report else {
        return false;
    };
    let now = Instant::now();
    let live =
        state.error.is_none() && now.saturating_duration_since(report.sampled).as_secs() <= 6;
    let running = report
        .containers
        .iter()
        .filter(|c| c.state == "running")
        .count();
    let mut open = false;
    theme::card(ui, "Docker", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong(crate::i18n::format(
                "{0} running · {1} containers",
                &[running.to_string(), report.containers.len().to_string()],
            ));
            ui.weak(&report.endpoint.context);
            if !live {
                theme::colored(
                    ui,
                    theme::WARN,
                    crate::i18n::tr("Docker data is out of date"),
                );
            }
            if ui.button(crate::i18n::tr("View containers")).clicked() {
                open = true;
            }
        });
        if live {
            let mut busy: Vec<_> = report
                .containers
                .iter()
                .filter(|c| fresh(c, state, now) && c.metrics.cpu.is_some())
                .collect();
            busy.sort_by(|a, b| {
                b.metrics
                    .cpu
                    .unwrap_or(0.0)
                    .total_cmp(&a.metrics.cpu.unwrap_or(0.0))
            });
            for c in busy.into_iter().take(3) {
                ui.label(format!(
                    "{}  ·  {:.1}% CPU  ·  {}",
                    c.name,
                    c.metrics.cpu.unwrap_or(0.0),
                    value(c.metrics.memory.map(|n| n as f64))
                ));
            }
        }
    });
    open
}

pub fn show(ui: &mut Ui, state: &State, view: &mut View) {
    let now = Instant::now();
    let Some(report) = &state.report else {
        theme::card(ui, "Docker", |ui| {
            if let Some(error) = &state.error {
                theme::empty_state(
                    ui,
                    crate::i18n::tr("Docker is unavailable"),
                    crate::i18n::tr(
                        "Start Docker Desktop and select a local Windows named-pipe context.",
                    ),
                );
                ui.add(egui::Label::new(error).wrap());
            } else {
                theme::loading(ui, crate::i18n::tr("Connecting to Docker…"));
            }
        });
        return;
    };
    if let Some(error) = &state.error {
        theme::panel(ui, |ui| {
            theme::colored(
                ui,
                theme::WARN,
                crate::i18n::tr("Docker data is out of date"),
            );
            ui.add(egui::Label::new(error).wrap());
        });
    }
    if view
        .selected
        .as_ref()
        .is_some_and(|id| !report.containers.iter().any(|c| &c.id == id))
    {
        view.selected = None;
    }
    theme::panel(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Docker");
            ui.weak(&report.endpoint.context);
            ui.weak(crate::i18n::format(
                "Updated {0}s ago",
                &[now
                    .saturating_duration_since(report.sampled)
                    .as_secs()
                    .to_string()],
            ));
            for (filter, label) in [
                (0, crate::i18n::tr("All containers")),
                (1, crate::i18n::tr("Running")),
                (2, crate::i18n::tr("Stopped")),
            ] {
                crate::theme::choice(ui, &mut view.filter, filter, label);
            }
        });
        ui.horizontal_wrapped(|ui| {
            let label = ui.label(crate::i18n::tr("Search containers"));
            ui.add(egui::TextEdit::singleline(&mut view.search).desired_width(220.0))
                .labelled_by(label.id);
            if !view.search.is_empty() && ui.small_button(crate::i18n::tr("Clear search")).clicked()
            {
                view.search.clear();
            }
            egui::ComboBox::from_label(crate::i18n::tr("Project"))
                .selected_text(if view.project.is_empty() {
                    crate::i18n::tr("All projects")
                } else {
                    &view.project
                })
                .show_ui(ui, |ui| {
                    crate::theme::choice(
                        ui,
                        &mut view.project,
                        String::new(),
                        crate::i18n::tr("All projects"),
                    );
                    let projects: std::collections::BTreeSet<_> = report
                        .containers
                        .iter()
                        .filter(|c| !c.project.is_empty())
                        .map(|c| c.project.as_str())
                        .collect();
                    for project in projects {
                        crate::theme::choice(ui, &mut view.project, project.into(), project);
                    }
                });
        });
    });
    let query = view.search.trim().to_lowercase();
    let mut rows: Vec<_> = report
        .containers
        .iter()
        .filter(|c| {
            (view.filter == 0 || (view.filter == 1) == (c.state == "running"))
                && (view.project.is_empty() || view.project == c.project)
                && (query.is_empty()
                    || [&c.name, &c.image, &c.id, &c.project]
                        .into_iter()
                        .any(|s| s.to_lowercase().contains(&query)))
        })
        .collect();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.weak(crate::i18n::format(
            "{0} of {1} containers",
            &[rows.len().to_string(), report.containers.len().to_string()],
        ));
        ui.weak(crate::i18n::tr(
            "CPU uses Docker conventions: 100% per core on Linux.",
        ));
    });
    if report.truncated {
        theme::colored(
            ui,
            theme::WARN,
            crate::i18n::tr("Showing the first 512 containers."),
        );
    }
    let mut sort = crate::sort::Sort::load(ui, "containers", crate::sort::Sort::new(0, false));
    sort.apply(&mut rows, |a, b, col| match col {
        0 => crate::sort::text(&a.name, &b.name),
        1 => crate::sort::text(&a.project, &b.project),
        2 => crate::sort::text(&a.state, &b.state),
        3 => a
            .metrics
            .cpu
            .unwrap_or(-1.0)
            .total_cmp(&b.metrics.cpu.unwrap_or(-1.0)),
        4 => a.metrics.memory.cmp(&b.metrics.memory),
        _ => crate::sort::text(&a.image, &b.image),
    });
    let table_height = if view.selected.is_some() {
        let share = if ui.ctx().content_rect().height() < 640.0 {
            0.40
        } else {
            0.44
        };
        (ui.available_height() * share).max(90.0)
    } else {
        (ui.available_height() - 20.0).max(90.0)
    };
    let row_height = ui.spacing().interact_size.y;
    ui.style_mut().interaction.selectable_labels = false;
    theme::table_panel(ui, "docker_table", 740.0, |ui| {
        crate::theme::table(ui)
            .sense(egui::Sense::click())
            .striped(true)
            .resizable(true)
            .min_scrolled_height(60.0)
            .max_scroll_height(table_height)
            .column(Column::initial(180.0).clip(true))
            .column(Column::initial(120.0).clip(true))
            .column(Column::initial(95.0).clip(true))
            .column(Column::initial(85.0))
            .column(Column::initial(120.0))
            .column(Column::remainder().clip(true))
            .header(row_height, |mut header| {
                for (index, label) in [
                    crate::i18n::tr("Container"),
                    crate::i18n::tr("Project"),
                    crate::i18n::tr("State"),
                    "CPU",
                    crate::i18n::tr("Memory"),
                    crate::i18n::tr("Image"),
                ]
                .into_iter()
                .enumerate()
                {
                    header.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            crate::sort::header(ui, label, index, matches!(index, 3 | 4), &mut sort)
                        })
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, rows.len(), |mut row| {
                    let c = rows[row.index()];
                    row.set_selected(view.selected.as_deref() == Some(&c.id));
                    let current = fresh(c, state, now);
                    let mut name_response = None;
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            let response = theme::table_name(
                                ui,
                                &c.name,
                                view.selected.as_deref() == Some(&c.id),
                            );
                            if response.clicked() {
                                view.selected = Some(c.id.clone());
                            }
                            name_response = Some(response);
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.add(egui::Label::new(&c.project).truncate())
                                .on_hover_text(&c.project);
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            theme::colored(
                                ui,
                                if c.state == "running" {
                                    theme::GOOD
                                } else {
                                    theme::WARN
                                },
                                state_label(&c.state),
                            )
                            .on_hover_text(&c.status);
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.label(if current {
                                c.metrics
                                    .cpu
                                    .map(|n| format!("{n:.1}%"))
                                    .unwrap_or("—".into())
                            } else {
                                "—".into()
                            });
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.label(if current {
                                value(c.metrics.memory.map(|n| n as f64))
                            } else {
                                "—".into()
                            });
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.add(egui::Label::new(&c.image).truncate())
                                .on_hover_text(&c.image);
                        })
                    });
                    let row_response = row.response();
                    if row_response.clicked() {
                        view.selected = Some(c.id.clone());
                    }
                    let response = name_response.map_or_else(
                        || row_response.clone(),
                        |name| row_response.clone().union(name),
                    );
                    crate::context_actions::menu(&response, ("container", &c.id), |ui| {
                        use crate::context_actions::{action, copy};
                        if action(ui, crate::i18n::tr("Details")) {
                            view.selected = Some(c.id.clone());
                        }
                        copy(ui, crate::i18n::tr("Name"), || c.name.clone());
                        copy(ui, "ID", || c.id.clone());
                        copy(ui, crate::i18n::tr("Image"), || c.image.clone());
                    });
                });
            });
    });
    sort.store(ui, "containers");
    if rows.is_empty() {
        ui.weak(crate::i18n::tr("No matching containers"));
    }
    if let Some(c) = view
        .selected
        .as_ref()
        .and_then(|id| report.containers.iter().find(|c| &c.id == id))
    {
        egui::ScrollArea::vertical()
            .id_salt("container_details")
            .show(ui, |ui| {
                theme::card(ui, &c.name, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.monospace(c.id.get(..12).unwrap_or(&c.id));
                        ui.label(state_label(&c.state));
                        if ui.small_button(crate::i18n::tr("Close details")).clicked() {
                            view.selected = None;
                        }
                    });
                    ui.add(egui::Label::new(&c.image).wrap());
                    ui.add(egui::Label::new(&c.status).wrap());
                    if let Some(created) = chrono::DateTime::from_timestamp(c.created, 0) {
                        ui.label(format!(
                            "{}: {}",
                            crate::i18n::tr("Created"),
                            created.to_rfc3339()
                        ));
                    }
                    let current = fresh(c, state, now);
                    let m = &c.metrics;
                    if !current && c.state == "running" {
                        theme::colored(
                            ui,
                            theme::WARN,
                            crate::i18n::tr("Waiting for fresh container statistics"),
                        );
                    }
                    if let Some(error) = &c.metrics_error {
                        ui.add(egui::Label::new(error).wrap());
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "{}: {} / {}",
                            crate::i18n::tr("Memory"),
                            if current {
                                value(m.memory.map(|n| n as f64))
                            } else {
                                "—".into()
                            },
                            value(m.limit.map(|n| n as f64))
                        ));
                        ui.label(format!(
                            "{}: ↓ {} · ↑ {}",
                            crate::i18n::tr("Network"),
                            if current {
                                rate(m.rx_rate)
                            } else {
                                "—".into()
                            },
                            if current {
                                rate(m.tx_rate)
                            } else {
                                "—".into()
                            }
                        ));
                        ui.label(format!(
                            "{}: ↓ {} · ↑ {}",
                            crate::i18n::tr("Disk I/O"),
                            if current {
                                rate(m.read_rate)
                            } else {
                                "—".into()
                            },
                            if current {
                                rate(m.write_rate)
                            } else {
                                "—".into()
                            }
                        ));
                    });
                    if current {
                        ui.columns(2, |columns| {
                            {
                                let ui = &mut columns[0];
                                let values = &m.cpu_history;
                                ui.label("CPU");
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), 45.0),
                                    egui::Sense::hover(),
                                );
                                theme::sparkline(
                                    ui.painter(),
                                    rect,
                                    values,
                                    values.iter().copied().fold(100.0, f32::max),
                                    wtm_core::docker::HISTORY,
                                    theme::CPU,
                                );
                            }
                            let ui = &mut columns[1];
                            ui.label(crate::i18n::tr("Memory"));
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 45.0),
                                egui::Sense::hover(),
                            );
                            theme::sparkline(
                                ui.painter(),
                                rect,
                                &m.memory_history,
                                m.memory_history.iter().copied().fold(1.0, f32::max),
                                wtm_core::docker::HISTORY,
                                theme::MEMORY,
                            );
                        });
                    }
                    if !c.ports.is_empty() {
                        ui.label(crate::i18n::tr("Ports"));
                        for port in &c.ports {
                            ui.monospace(port);
                        }
                    }
                    match report
                        .detail
                        .as_ref()
                        .filter(|_| report.detail_id.as_deref() == Some(c.id.as_str()))
                    {
                        Some(Ok(detail)) if detail.id == c.id => {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(format!(
                                    "{}: {}",
                                    crate::i18n::tr("Health"),
                                    if detail.health.is_empty() {
                                        crate::i18n::tr("Not configured")
                                    } else {
                                        state_label(&detail.health)
                                    }
                                ));
                                ui.label(crate::i18n::format(
                                    "{0} restarts",
                                    &[detail.restarts.to_string()],
                                ));
                            });
                            ui.label(format!(
                                "{}: {}",
                                crate::i18n::tr("Started"),
                                detail.started
                            ));
                            ui.label(crate::i18n::tr("Recent logs (last 200 lines)"));
                            match &detail.logs {
                                Ok(logs) => {
                                    if logs.is_empty() {
                                        ui.weak(crate::i18n::tr("No logs available"));
                                    } else {
                                        if ui.small_button(crate::i18n::tr("Copy logs")).clicked() {
                                            ui.ctx().copy_text(logs.clone());
                                        }
                                        egui::ScrollArea::both()
                                            .id_salt("docker_logs")
                                            .max_height(180.0)
                                            .show(ui, |ui| {
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(logs).monospace(),
                                                    )
                                                    .selectable(true),
                                                );
                                            });
                                    }
                                }
                                Err(error) => {
                                    ui.add(egui::Label::new(error).wrap());
                                }
                            }
                        }
                        Some(Err(error)) => {
                            ui.add(egui::Label::new(error).wrap());
                        }
                        _ => {
                            ui.weak(crate::i18n::tr("Loading container details…"));
                        }
                    }
                });
            });
    }
}

#[cfg(test)]
pub fn fixture() -> State {
    use wtm_core::docker::*;
    let now = Instant::now();
    let containers = (0..5)
        .map(|i| Container {
            id: format!("{i:064x}"),
            name: format!("{}-{i}", if i < 3 { "web" } else { "database" }),
            image: if i < 3 { "nginx:alpine" } else { "postgres:17" }.into(),
            project: if i < 3 { "store" } else { "analytics" }.into(),
            state: if i == 4 { "exited" } else { "running" }.into(),
            status: "Up 2 hours (healthy)".into(),
            ports: vec![format!("127.0.0.1:{} → 80/tcp", 8080 + i)],
            created: 1700000000,
            metrics: Metrics {
                cpu: Some(12.5 + i as f32 * 20.0),
                memory: Some((64 + i * 32) * 1024 * 1024),
                limit: Some(1024 * 1024 * 1024),
                rx_rate: Some(2048.0),
                tx_rate: Some(512.0),
                read_rate: Some(4096.0),
                write_rate: Some(1024.0),
                measured: Some(now),
                cpu_history: (0..60)
                    .map(|n| 20.0 + (n as f32 * 0.2).sin() * 10.0)
                    .collect(),
                memory_history: (0..60)
                    .map(|n| (64.0 + n as f32 * 0.1) * 1024.0 * 1024.0)
                    .collect(),
            },
            metrics_error: None,
        })
        .collect();
    State {error:None,report:Some(Report {endpoint:Endpoint {context:"desktop-linux".into(),pipe:r"\\.\pipe\dockerDesktopLinuxEngine".into()},containers,detail_id:Some(format!("{:064x}",0)),detail:Some(Ok(Detail {id:format!("{:064x}",0),health:"healthy".into(),started:"2026-11-03T09:00:00Z".into(),restarts:2,logs:Ok("2026-11-03T09:00:01Z Listening on port 80\n2026-11-03T09:00:02Z GET /health 200\n".into())})),sampled:now,truncated:false})}
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    #[test]
    fn clicking_a_container_metric_selects_its_row() {
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1080.0, 700.0])
            .with_context_setup(theme::apply)
            .build_ui_state(
                |ui, (state, view): &mut (State, View)| {
                    show(ui, state, view);
                },
                (fixture(), View::default()),
            );
        h.run_steps(2);
        h.get_by_label("12.5%").click();
        h.run_steps(2);
        assert_eq!(
            h.state().1.selected.as_deref(),
            Some(format!("{:064x}", 0).as_str())
        );
    }

    #[test]
    fn docker_filters_details_keyboard_and_copy_work_in_both_themes_and_sizes() {
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            for size in [[820.0, 500.0], [1440.0, 1000.0]] {
                let mut h = Harness::builder()
                    .allow_missing_glyphs()
                    .with_size(size)
                    .build_ui_state(
                        |ui, (state, view): &mut (State, View)| {
                            theme::apply(ui.ctx());
                            ui.ctx().set_theme(theme);
                            show(ui, state, view);
                        },
                        (fixture(), View::default()),
                    );
                h.run();
                assert!(h.query_by_label("5 of 5 containers").is_some());
                h.get_by_role(egui::accesskit::Role::TextInput).focus();
                h.run();
                h.get_by_role(egui::accesskit::Role::TextInput)
                    .type_text("WEB");
                h.run();
                assert!(h.query_by_label("3 of 5 containers").is_some());
                h.get_by_label("Clear search").click();
                h.run();
                h.get_all_by_label("Stopped").next().unwrap().click();
                h.run();
                assert!(h.query_by_label("1 of 5 containers").is_some());
                h.get_by_label("All containers").click();
                h.run();
                h.get_by_role(egui::accesskit::Role::ComboBox).click();
                h.run();
                h.get_by(|node| {
                    node.role() != egui::accesskit::Role::Label
                        && node.label().as_deref() == Some("store")
                })
                .click();
                h.run();
                assert!(h.query_by_label("3 of 5 containers").is_some());
                h.get_by_label("web-0").focus();
                h.run();
                h.key_press(egui::Key::Enter);
                h.run();
                assert_eq!(
                    h.state().1.selected.as_deref(),
                    Some(format!("{:064x}", 0).as_str())
                );
                {
                    h.event(egui::Event::PointerMoved(egui::pos2(
                        size[0] * 0.6,
                        size[1] - 50.0,
                    )));
                    h.event(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -1200.0),
                        modifiers: egui::Modifiers::NONE,
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Unknown,
                    });
                    h.run_steps(20);
                }
                {
                    let copy = h.get_by_label("Copy logs");
                    eprintln!("copy {theme:?} {size:?}: {:?}", copy.rect());
                    copy.click();
                    h.run_steps(1);
                    assert!(h.output().platform_output.commands.iter().any(|c|matches!(c,egui::OutputCommand::CopyText(t) if t.contains("GET /health"))));
                }
                {
                    h.event(egui::Event::PointerMoved(egui::pos2(
                        size[0] * 0.6,
                        size[1] - 50.0,
                    )));
                    h.event(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, 1200.0),
                        modifiers: egui::Modifiers::NONE,
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Unknown,
                    });
                    h.run_steps(20);
                }
                h.get_by_label("Close details").click();
                h.run();
                assert!(h.state().1.selected.is_none());
            }
        }
    }
    #[test]
    fn docker_offline_stale_stats_and_eviction_are_explicit() {
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([820.0, 500.0])
            .build_ui_state(
                |ui, (s, v): &mut (State, View)| show(ui, s, v),
                (
                    State {
                        report: None,
                        error: Some("injected offline".into()),
                    },
                    View::default(),
                ),
            );
        h.run();
        assert!(h.query_by_label("Docker is unavailable").is_some());
        h.state_mut().0 = fixture();
        h.state_mut().0.error = Some("injected timeout".into());
        h.state_mut().1.selected = Some(format!("{:064x}", 0));
        h.run();
        assert!(h.query_by_label("Docker data is out of date").is_some());
        assert!(
            h.query_by_label("Waiting for fresh container statistics")
                .is_some()
        );
        h.state_mut().0.report.as_mut().unwrap().containers.clear();
        h.run();
        assert!(h.state().1.selected.is_none());
    }
    #[test]
    fn docker_details_errors_belong_only_to_the_requested_container() {
        let mut state = fixture();
        state.report.as_mut().unwrap().detail = Some(Err("error for original container".into()));
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1100.0, 900.0])
            .build_ui_state(
                |ui, (s, v): &mut (State, View)| show(ui, s, v),
                (
                    state,
                    View {
                        selected: Some(format!("{:064x}", 1)),
                        ..Default::default()
                    },
                ),
            );
        h.run();
        assert!(h.query_by_label("Loading container details…").is_some());
        assert!(h.query_by_label("error for original container").is_none());
    }

    #[test]
    fn docker_fixtures_render_in_five_languages_with_translated_controls() {
        for lang in [
            crate::i18n::Language::English,
            crate::i18n::Language::Turkish,
            crate::i18n::Language::German,
            crate::i18n::Language::French,
            crate::i18n::Language::Spanish,
        ] {
            let _language = crate::i18n::scope(lang);
            let mut h = Harness::builder()
                .allow_missing_glyphs()
                .with_size([820.0, 500.0])
                .build_ui_state(
                    |ui, (s, v): &mut (State, View)| show(ui, s, v),
                    (fixture(), View::default()),
                );
            h.run();
            assert!(
                h.query_by_label(crate::i18n::tr("Search containers"))
                    .is_some()
            );
        }
    }
}

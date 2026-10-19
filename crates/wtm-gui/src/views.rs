//! Read-only Users and Network tabs.

use std::collections::HashMap;

use eframe::egui::{self, Align, Color32, Layout, Ui};
use egui_extras::{Column, TableBuilder};
use wtm_core::format_bytes;

use crate::backend::Data;

fn right(ui: &mut Ui, text: impl Into<egui::WidgetText>) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(text);
    });
}

pub fn users(ui: &mut Ui, data: &Data) {
    let sessions = match &data.sessions {
        None => {
            ui.spinner();
            return;
        }
        Some(Err(e)) => {
            ui.colored_label(Color32::RED, e);
            return;
        }
        Some(Ok(s)) => s,
    };

    // Per-session totals from the latest process snapshot.
    let mut totals: HashMap<u32, (usize, f32, u64)> = HashMap::new();
    if let Some(snap) = &data.snapshot {
        for p in &snap.processes {
            let t = totals.entry(p.session_id).or_default();
            t.0 += 1;
            t.1 += p.cpu_percent;
            t.2 += p.private_working_set;
        }
    }

    egui::Grid::new("users_grid")
        .num_columns(6)
        .striped(true)
        .show(ui, |ui| {
            for h in [
                "User",
                "Session",
                "Station",
                "State",
                "Processes",
                "CPU / Memory",
            ] {
                ui.strong(h);
            }
            ui.end_row();
            for s in sessions.iter().filter(|s| !s.user.is_empty()) {
                let (count, cpu, mem) = totals.get(&s.id).copied().unwrap_or_default();
                ui.label(if s.domain.is_empty() {
                    s.user.clone()
                } else {
                    format!(r"{}\{}", s.domain, s.user)
                });
                ui.label(s.id.to_string());
                ui.label(&s.station);
                ui.label(format!("{:?}", s.state));
                ui.label(count.to_string());
                ui.label(format!("{cpu:.1}%  /  {}", format_bytes(mem as f64)));
                ui.end_row();
            }
        });
}

pub fn network(ui: &mut Ui, data: &Data, filter: &mut String, only_pid: Option<u32>) {
    ui.horizontal(|ui| {
        ui.label("Search:");
        ui.add(
            egui::TextEdit::singleline(filter)
                .hint_text("process, address or port")
                .desired_width(240.0),
        );
        if let Some(pid) = only_pid {
            ui.label(format!("(selected process {pid} only)"));
        }
    });
    ui.separator();
    let conns = match &data.connections {
        None => {
            ui.spinner();
            return;
        }
        Some(Err(e)) => {
            ui.colored_label(Color32::RED, e);
            return;
        }
        Some(Ok(c)) => c,
    };

    let names: HashMap<u32, &str> = data
        .snapshot
        .iter()
        .flat_map(|s| &s.processes)
        .map(|p| (p.pid, p.name.as_str()))
        .collect();
    let f = filter.trim().to_lowercase();
    let rows: Vec<_> = conns
        .iter()
        .filter(|c| only_pid.is_none_or(|pid| c.pid == pid))
        .map(|c| {
            let name = names.get(&c.pid).copied().unwrap_or("?");
            let remote = c.remote.map(|r| r.to_string()).unwrap_or_default();
            (c, name, remote)
        })
        .filter(|(c, name, remote)| {
            f.is_empty()
                || name.to_lowercase().contains(&f)
                || c.local.to_string().contains(&f)
                || remote.contains(&f)
        })
        .collect();

    let row_height = ui.spacing().interact_size.y;
    ui.style_mut().interaction.selectable_labels = false;
    TableBuilder::new(ui)
        .id_salt("network_table")
        .striped(true)
        .resizable(true)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::initial(200.0).clip(true))
        .column(Column::initial(64.0))
        .column(Column::initial(50.0))
        .column(Column::initial(200.0).clip(true))
        .column(Column::initial(200.0).clip(true))
        .column(Column::remainder())
        .header(row_height, |mut h| {
            for label in ["Process", "PID", "Proto", "Local", "Remote", "State"] {
                h.col(|ui| {
                    ui.strong(label);
                });
            }
        })
        .body(|body| {
            body.rows(row_height, rows.len(), |mut row| {
                let (c, name, remote) = &rows[row.index()];
                row.col(|ui| {
                    ui.label(*name);
                });
                row.col(|ui| right(ui, c.pid.to_string()));
                row.col(|ui| {
                    ui.label(format!("{:?}", c.protocol));
                });
                row.col(|ui| {
                    ui.label(c.local.to_string());
                });
                row.col(|ui| {
                    ui.label(remote.as_str());
                });
                row.col(|ui| {
                    ui.label(c.state.map(|s| format!("{s:?}")).unwrap_or_default());
                });
            });
        });
}

fn state_color(state: wtm_core::ServiceState) -> Color32 {
    match state {
        wtm_core::ServiceState::Running => Color32::from_rgb(80, 180, 100),
        wtm_core::ServiceState::Stopped => Color32::GRAY,
        _ => Color32::from_rgb(230, 140, 40),
    }
}

/// Returns the service the user right-clicked or clicked, if any.
pub fn services(ui: &mut Ui, data: &Data, filter: &mut String) -> Option<wtm_core::ServiceEntry> {
    ui.horizontal(|ui| {
        ui.label("Search:");
        ui.add(
            egui::TextEdit::singleline(filter)
                .hint_text("name or description")
                .desired_width(240.0),
        );
    });
    ui.separator();
    let list = match &data.services {
        None => {
            ui.spinner();
            return None;
        }
        Some(Err(e)) => {
            ui.colored_label(Color32::RED, e);
            return None;
        }
        Some(Ok(l)) => l,
    };
    let f = filter.trim().to_lowercase();
    let rows: Vec<_> = list
        .iter()
        .filter(|s| {
            f.is_empty()
                || s.name.to_lowercase().contains(&f)
                || s.display_name.to_lowercase().contains(&f)
        })
        .collect();

    let mut clicked = None;
    let row_height = ui.spacing().interact_size.y;
    ui.style_mut().interaction.selectable_labels = false;
    TableBuilder::new(ui)
        .id_salt("services_table")
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::initial(180.0).clip(true))
        .column(Column::initial(300.0).clip(true))
        .column(Column::initial(64.0))
        .column(Column::initial(100.0))
        .column(Column::initial(90.0))
        .column(Column::remainder().clip(true))
        .header(row_height, |mut h| {
            for label in ["Name", "Description", "PID", "Status", "Startup", "Account"] {
                h.col(|ui| {
                    ui.strong(label);
                });
            }
        })
        .body(|body| {
            body.rows(row_height, rows.len(), |mut row| {
                let s = rows[row.index()];
                row.col(|ui| {
                    ui.label(&s.name);
                });
                row.col(|ui| {
                    ui.label(&s.display_name);
                });
                row.col(|ui| {
                    right(
                        ui,
                        if s.pid == 0 {
                            String::new()
                        } else {
                            s.pid.to_string()
                        },
                    )
                });
                row.col(|ui| {
                    ui.colored_label(state_color(s.state), format!("{:?}", s.state));
                });
                row.col(|ui| {
                    ui.label(s.start_type.map(|t| format!("{t:?}")).unwrap_or_default());
                });
                row.col(|ui| {
                    ui.label(s.account.as_deref().unwrap_or(""));
                });
                if row.response().secondary_clicked() || row.response().clicked() {
                    clicked = Some(s.clone());
                }
            });
        });
    clicked
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistorySort {
    CpuAvg,
    CpuTime,
    MemPeak,
    Growth,
}

pub struct HistoryView {
    pub hours: u32,
    pub sort: HistorySort,
    pub selected: Option<String>,
}

impl Default for HistoryView {
    fn default() -> Self {
        Self {
            hours: 24,
            sort: HistorySort::CpuAvg,
            selected: None,
        }
    }
}

fn mib(bytes: f64) -> f64 {
    bytes / (1024.0 * 1024.0)
}

fn duration_label(minutes: u32) -> String {
    match minutes {
        m if m >= 60 => format!("{}h {:02}m", m / 60, m % 60),
        m => format!("{m}m"),
    }
}

/// Returns true when the window or selection changed and data must be reloaded.
pub fn app_history(ui: &mut Ui, data: &Data, view: &mut HistoryView) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Window:");
        for (label, hours) in [
            ("1 h", 1),
            ("6 h", 6),
            ("24 h", 24),
            ("7 days", 168),
            ("30 days", 720),
        ] {
            if ui.selectable_value(&mut view.hours, hours, label).changed() {
                changed = true;
            }
        }
        ui.separator();
        ui.label("Sort by:");
        for (label, sort) in [
            ("Avg CPU", HistorySort::CpuAvg),
            ("CPU time", HistorySort::CpuTime),
            ("Peak memory", HistorySort::MemPeak),
            ("Memory growth", HistorySort::Growth),
        ] {
            ui.selectable_value(&mut view.sort, sort, label);
        }
    });
    ui.separator();

    let (stats, findings) = match &data.app_history {
        None => {
            ui.spinner();
            return changed;
        }
        Some(Err(e)) => {
            ui.label(e);
            return changed;
        }
        Some(Ok(v)) => v,
    };

    if findings.is_empty() {
        ui.weak("No long-term findings in the analysis window.");
    } else {
        ui.strong(format!("Findings ({})", findings.len()));
        for f in findings {
            ui.colored_label(
                Color32::from_rgb(230, 140, 40),
                format!("{}  —  {:?}: {}", f.app, f.kind, f.detail),
            );
        }
    }
    ui.add_space(6.0);

    let flagged: HashMap<String, String> = findings
        .iter()
        .map(|f| (f.app.to_lowercase(), format!("{:?}", f.kind)))
        .collect();
    let mut rows: Vec<_> = stats.iter().collect();
    match view.sort {
        HistorySort::CpuAvg => rows.sort_by(|a, b| b.cpu_avg.total_cmp(&a.cpu_avg)),
        HistorySort::CpuTime => {
            rows.sort_by(|a, b| b.cpu_core_seconds.total_cmp(&a.cpu_core_seconds))
        }
        HistorySort::MemPeak => rows.sort_by_key(|s| std::cmp::Reverse(s.mem_peak)),
        HistorySort::Growth => {
            rows.sort_by(|a, b| b.mem_growth_per_hour.total_cmp(&a.mem_growth_per_hour))
        }
    }

    let chart_height = if view.selected.is_some() { 230.0 } else { 0.0 };
    let table_height = (ui.available_height() - chart_height).max(120.0);
    let row_height = ui.spacing().interact_size.y;
    let selected = view.selected.clone();
    egui::ScrollArea::vertical()
        .id_salt("history_table")
        .max_height(table_height)
        .show(ui, |ui| {
            ui.style_mut().interaction.selectable_labels = false;
            TableBuilder::new(ui)
                .id_salt("history_table_rows")
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::initial(200.0).clip(true))
                .columns(Column::initial(80.0), 9)
                .column(Column::remainder())
                .header(row_height, |mut h| {
                    for label in [
                        "App", "Seen", "Avg CPU", "p95 CPU", "Peak CPU", "Avg GPU", "CPU time",
                        "Avg mem", "Peak mem", "Growth/h", "Finding",
                    ] {
                        h.col(|ui| {
                            ui.strong(label);
                        });
                    }
                })
                .body(|body| {
                    body.rows(row_height, rows.len(), |mut row| {
                        let s = rows[row.index()];
                        row.set_selected(selected.as_deref() == Some(s.app.as_str()));
                        row.col(|ui| {
                            ui.label(&s.app);
                        });
                        row.col(|ui| right(ui, duration_label(s.minutes)));
                        row.col(|ui| right(ui, format!("{:.2}%", s.cpu_avg)));
                        row.col(|ui| right(ui, format!("{:.2}%", s.cpu_p95)));
                        row.col(|ui| right(ui, format!("{:.1}%", s.cpu_peak)));
                        row.col(|ui| right(ui, format!("{:.1}%", s.gpu_avg)));
                        row.col(|ui| {
                            right(ui, format!("{:.2} core-h", s.cpu_core_seconds / 3600.0))
                        });
                        row.col(|ui| right(ui, format_bytes(s.mem_avg as f64)));
                        row.col(|ui| right(ui, format_bytes(s.mem_peak as f64)));
                        row.col(|ui| {
                            right(
                                ui,
                                format!(
                                    "{:+.0} MiB (R² {:.2})",
                                    mib(s.mem_growth_per_hour),
                                    s.mem_growth_fit
                                ),
                            )
                        });
                        row.col(|ui| {
                            if let Some(kind) = flagged.get(&s.app.to_lowercase()) {
                                ui.colored_label(Color32::from_rgb(230, 140, 40), kind);
                            }
                        });
                        if row.response().clicked() {
                            view.selected = Some(s.app.clone());
                            changed = true;
                        }
                    });
                });
        });

    if let Some(app) = &view.selected {
        ui.separator();
        match &data.series {
            Some((name, rows)) if name == app && !rows.is_empty() => series_charts(ui, app, rows),
            _ => {
                ui.spinner();
            }
        }
    }
    changed
}

fn series_charts(ui: &mut Ui, app: &str, rows: &[wtm_policy::MinuteRow]) {
    use egui_plot::{Line, Plot, PlotPoints};
    let last = rows.last().map_or(0, |r| r.minute);
    let hours_ago = |m: i64| (m - last) as f64 / 60.0;
    let cpu: PlotPoints = rows
        .iter()
        .map(|r| [hours_ago(r.minute), f64::from(r.cpu_avg)])
        .collect();
    let mem: PlotPoints = rows
        .iter()
        .map(|r| [hours_ago(r.minute), mib(r.mem_avg as f64)])
        .collect();
    let has_gpu = rows.iter().any(|r| r.gpu_avg > 0.0);
    let gpu: PlotPoints = rows
        .iter()
        .map(|r| [hours_ago(r.minute), f64::from(r.gpu_avg)])
        .collect();
    ui.strong(format!(
        "{app} — per-minute usage (x: hours before latest sample)"
    ));
    ui.columns(2, |cols| {
        Plot::new("hist_cpu")
            .height(190.0)
            .include_y(0.0)
            .y_axis_label("%")
            .allow_drag(false)
            .allow_scroll(false)
            .show(&mut cols[0], |p| {
                p.line(Line::new("CPU", cpu));
                if has_gpu {
                    p.line(Line::new("GPU", gpu));
                }
            });
        Plot::new("hist_mem")
            .height(190.0)
            .include_y(0.0)
            .y_axis_label("MiB")
            .allow_drag(false)
            .allow_scroll(false)
            .show(&mut cols[1], |p| p.line(Line::new("Memory", mem)));
    });
}

/// Startup programs. Returns a command when the user toggles the selected entry.
pub fn startup(
    ui: &mut Ui,
    data: &Data,
    selected: &mut Option<wtm_core::startup::StartupEntry>,
) -> Option<crate::backend::UiCommand> {
    let Some(entries) = &data.startup else {
        ui.spinner();
        return None;
    };
    let mut command = None;
    ui.horizontal(|ui| {
        // Re-resolve the selection so the button reflects the current state.
        let current = selected.as_ref().and_then(|s| {
            entries
                .iter()
                .find(|e| e.name == s.name && e.scope == s.scope && e.source == s.source)
        });
        match current {
            Some(e) => {
                ui.strong(&e.name);
                let (label, enable) = if e.enabled {
                    ("Disable", false)
                } else {
                    ("Enable", true)
                };
                if ui.button(label).clicked() {
                    command = Some(crate::backend::UiCommand::SetStartup {
                        entry: e.clone(),
                        enabled: enable,
                    });
                }
            }
            None => {
                ui.weak("Select an entry to enable or disable it at sign-in.");
            }
        }
    });
    ui.separator();

    let row_height = ui.spacing().interact_size.y;
    ui.style_mut().interaction.selectable_labels = false;
    TableBuilder::new(ui)
        .id_salt("startup_table")
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::initial(220.0).clip(true))
        .column(Column::initial(80.0))
        .column(Column::initial(100.0))
        .column(Column::initial(80.0))
        .column(Column::remainder().clip(true))
        .header(row_height, |mut h| {
            for label in ["Name", "Status", "Scope", "Source", "Command"] {
                h.col(|ui| {
                    ui.strong(label);
                });
            }
        })
        .body(|body| {
            body.rows(row_height, entries.len(), |mut row| {
                let e = &entries[row.index()];
                row.set_selected(selected.as_ref().is_some_and(|s| {
                    s.name == e.name && s.scope == e.scope && s.source == e.source
                }));
                row.col(|ui| {
                    ui.label(&e.name);
                });
                row.col(|ui| {
                    if e.enabled {
                        ui.colored_label(Color32::from_rgb(80, 180, 100), "Enabled");
                    } else {
                        ui.weak("Disabled");
                    }
                });
                row.col(|ui| {
                    ui.label(format!("{:?}", e.scope));
                });
                row.col(|ui| {
                    ui.label(format!("{:?}", e.source));
                });
                row.col(|ui| {
                    ui.label(&e.command);
                });
                if row.response().clicked() {
                    *selected = Some(e.clone());
                }
            });
        });
    command
}

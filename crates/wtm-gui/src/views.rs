//! User sessions, services, network connections and startup applications.

use std::collections::HashMap;

use eframe::egui::{self, Align, Color32, Layout, Ui};
use egui_extras::Column;
use std::net::SocketAddr;

use wtm_core::format_bytes;

use crate::backend::{Data, ServiceOp, UiCommand};
use crate::context_actions::{action, copy, menu};
use crate::sort::{Sort, header, num, text};
use crate::theme::{BAD, GOOD, WARN, right};

/// Returns a session to disconnect or sign out once confirmed: (id, user, op).
pub fn users(ui: &mut Ui, data: &Data) -> Option<(u32, String, crate::backend::SessionOp)> {
    use crate::backend::SessionOp;
    let sessions = match &data.sessions {
        None => {
            crate::theme::loading(ui, crate::i18n::tr("Loading sessions…"));
            return None;
        }
        Some(Err(e)) => {
            crate::theme::colored(ui, BAD, e);
            return None;
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

    let account = |s: &wtm_core::Session| {
        if s.domain.is_empty() {
            s.user.clone()
        } else {
            format!(r"{}\{}", s.domain, s.user)
        }
    };
    let mut rows: Vec<_> = sessions
        .iter()
        .filter(|s| !s.user.is_empty())
        .map(|s| (s, totals.get(&s.id).copied().unwrap_or_default()))
        .collect();
    let mut sort = Sort::load(ui, "users", Sort::new(0, false));
    sort.apply(&mut rows, |(a, ta), (b, tb), col| match col {
        0 => text(&account(a), &account(b)),
        1 => a.id.cmp(&b.id),
        2 => text(&a.station, &b.station),
        3 => text(&format!("{:?}", a.state), &format!("{:?}", b.state)),
        4 => ta.0.cmp(&tb.0),
        5 => num(f64::from(ta.1), f64::from(tb.1)),
        _ => ta.2.cmp(&tb.2),
    });
    let mut request = None;
    let row_height = ui.spacing().interact_size.y;
    crate::theme::table_panel(ui, "users", 1100.0, |ui| {
        crate::theme::table(ui)
            .id_salt("user_sessions")
            .sense(egui::Sense::click())
            .striped(true)
            .resizable(true)
            .auto_shrink([false, true])
            .min_scrolled_height(0.0)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::initial(200.0).clip(true))
            .column(Column::initial(72.0))
            .column(Column::initial(120.0).clip(true))
            .column(Column::initial(108.0).clip(true))
            .column(Column::initial(104.0))
            .column(Column::initial(90.0))
            .column(Column::initial(120.0))
            .column(Column::remainder().at_least(264.0))
            .header(row_height, |mut row| {
                for (col, (label, numeric)) in [
                    (crate::i18n::tr("User"), false),
                    (crate::i18n::tr("Session"), true),
                    (crate::i18n::tr("Station"), false),
                    (crate::i18n::tr("State"), false),
                    (crate::i18n::tr("Processes"), true),
                    ("CPU", true),
                    (crate::i18n::tr("Memory"), true),
                ]
                .into_iter()
                .enumerate()
                {
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            header(ui, label, col, numeric, &mut sort)
                        })
                    });
                }
                row.col(|ui| {
                    crate::theme::table_cell(ui, |ui| {
                        ui.weak(crate::i18n::tr("Actions"));
                    })
                });
            })
            .body(|body| {
                body.rows(row_height, rows.len(), |mut row| {
                    let (session, (count, cpu, memory)) = rows[row.index()];
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.add(egui::Label::new(account(session)).truncate())
                                .on_hover_text(account(session));
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| right(ui, session.id.to_string()))
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.add(egui::Label::new(&session.station).truncate())
                                .on_hover_text(&session.station);
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.label(crate::i18n::debug(session.state));
                        })
                    });
                    row.col(|ui| crate::theme::table_cell(ui, |ui| right(ui, count.to_string())));
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| right(ui, format!("{cpu:.1}%")))
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| right(ui, format_bytes(memory as f64)))
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.horizontal(|ui| {
                                if session.state != wtm_core::SessionState::Disconnected
                                    && ui
                                        .small_button(crate::i18n::tr("Disconnect"))
                                        .on_hover_text(crate::i18n::tr(
                                            "Programs keep running; the user can sign in again",
                                        ))
                                        .clicked()
                                {
                                    request =
                                        Some((session.id, account(session), SessionOp::Disconnect));
                                }
                                if ui
                                    .small_button(crate::i18n::tr("Sign off"))
                                    .on_hover_text(crate::i18n::tr("Closes the user's programs"))
                                    .clicked()
                                {
                                    request =
                                        Some((session.id, account(session), SessionOp::SignOff));
                                }
                            });
                        })
                    });
                    menu(
                        &row.response(),
                        (
                            "session",
                            session.id,
                            &session.user,
                            &session.domain,
                            &session.station,
                        ),
                        |ui| {
                            copy(ui, crate::i18n::tr("User"), || account(session));
                            copy(ui, crate::i18n::tr("Session"), || session.id.to_string());
                            if session.state != wtm_core::SessionState::Disconnected
                                && action(ui, crate::i18n::tr("Disconnect"))
                            {
                                request =
                                    Some((session.id, account(session), SessionOp::Disconnect));
                            }
                            if action(ui, crate::i18n::tr("Sign off")) {
                                request = Some((session.id, account(session), SessionOp::SignOff));
                            }
                        },
                    );
                })
            });
    });
    sort.store(ui, "users");
    request
}

/// The processes moving the most network data right now.
fn top_talkers(ui: &mut Ui, data: &Data) {
    let Some(snap) = &data.snapshot else {
        return;
    };
    let mut busy: Vec<_> = snap
        .processes
        .iter()
        .filter(|p| p.net_rate() >= 1.0)
        .collect();
    busy.sort_by(|a, b| b.net_rate().total_cmp(&a.net_rate()));
    ui.horizontal_wrapped(|ui| {
        ui.strong(crate::i18n::tr("Most traffic:"));
        if busy.is_empty() {
            ui.weak(crate::i18n::tr(
                "none right now (per-program traffic needs the RustWTM service or administrator rights)",
            ));
        }
        for p in busy.into_iter().take(6) {
            ui.label(format!(
                "{} ↓{}/s ↑{}/s",
                p.name,
                format_bytes(p.net_rx_rate),
                format_bytes(p.net_tx_rate)
            ));
            ui.weak("·");
        }
    });
}

/// An IPv4 TCP connection with a remote end, which Windows can reset.
pub(crate) fn closable(c: &wtm_core::Connection) -> Option<(SocketAddr, SocketAddr)> {
    let remote = c.remote?;
    let open = !matches!(
        c.state,
        Some(
            wtm_core::TcpState::Listen | wtm_core::TcpState::TimeWait | wtm_core::TcpState::Closed
        )
    );
    (c.protocol == wtm_core::Protocol::Tcp && open && c.local.is_ipv4() && remote.is_ipv4())
        .then_some((c.local, remote))
}

/// What the user asked for in the Network tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkAction {
    Close(SocketAddr, SocketAddr),
    Unblock(String),
    Reveal(wtm_core::control::ProcessRef),
    /// Stop narrowing the list to the selected process.
    ShowAll,
}

/// Programs cut off the network, each with a way back.
fn blocked_programs(ui: &mut Ui, data: &Data, action: &mut Option<NetworkAction>) {
    let Some(Ok(list)) = &data.firewall else {
        return;
    };
    if list.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.strong(crate::i18n::tr("Blocked from the network:"));
        for path in list {
            let file = std::path::Path::new(path)
                .file_name()
                .map_or_else(|| path.clone(), |f| f.to_string_lossy().into_owned());
            ui.label(file).on_hover_text(path);
            if ui
                .small_button(crate::i18n::tr("Unblock"))
                .on_hover_text(crate::i18n::tr(
                    "Remove RustWTM's Windows Firewall rules for it",
                ))
                .clicked()
            {
                *action = Some(NetworkAction::Unblock(path.clone()));
            }
        }
    });
}

/// Returns what the user asked for, if anything. `only` narrows the list to
/// one process while it still runs: its PID may have gone to another one.
pub fn network(
    ui: &mut Ui,
    data: &Data,
    filter: &mut String,
    only: Option<&wtm_core::control::ProcessRef>,
) -> Option<NetworkAction> {
    let only_pid = only
        .filter(|r| {
            data.snapshot
                .iter()
                .flat_map(|s| &s.processes)
                .any(|p| p.pid == r.pid && p.create_time == r.create_time)
        })
        .map(|r| r.pid);
    let mut close = None;
    ui.horizontal_wrapped(|ui| {
        crate::theme::search_bar(
            ui,
            egui::Id::unique(("search", "network")),
            filter,
            crate::i18n::tr("process, address or port"),
            240.0,
        );
        if let Some(pid) = only_pid {
            ui.label(crate::i18n::format(
                "(selected process {0} only)",
                &[format!("{}", pid)],
            ));
            if ui.small_button(crate::i18n::tr("Show all")).clicked() {
                close = Some(NetworkAction::ShowAll);
            }
        }
    });
    top_talkers(ui, data);
    blocked_programs(ui, data, &mut close);
    crate::theme::divider(ui);
    let conns = match &data.connections {
        None => {
            crate::theme::loading(ui, crate::i18n::tr("Loading connections…"));
            return None;
        }
        Some(Err(e)) => {
            crate::theme::colored(ui, BAD, e);
            return None;
        }
        Some(Ok(c)) => c,
    };

    let owners: HashMap<u32, &wtm_core::ProcessInfo> = data
        .snapshot
        .iter()
        .flat_map(|s| &s.processes)
        .map(|p| (p.pid, p))
        .collect();
    let f = filter.trim().to_lowercase();
    let mut rows: Vec<_> = conns
        .iter()
        .filter(|c| only_pid.is_none_or(|pid| c.pid == pid))
        .map(|c| {
            let name = owners.get(&c.pid).map_or("?", |p| p.name.as_str());
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
    let mut sort = Sort::load(ui, "network", Sort::new(0, false));
    if sort.col == 2 {
        sort.apply_text(&mut rows, |(c, ..)| format!("{:?}", c.protocol));
    } else if sort.col >= 5 {
        sort.apply_text(&mut rows, |(c, ..)| {
            c.state.map(crate::i18n::debug).unwrap_or_default()
        });
    } else {
        sort.apply(&mut rows, |(a, an, ar), (b, bn, br), col| match col {
            0 => text(an, bn),
            1 => a.pid.cmp(&b.pid),
            3 => a.local.cmp(&b.local),
            _ => text(ar, br),
        });
    }
    if rows.is_empty() && !f.is_empty() {
        crate::theme::empty_state(
            ui,
            crate::i18n::tr("No matching connections"),
            crate::i18n::tr("Try another process name, address or port."),
        );
        if ui.button(crate::i18n::tr("Clear search")).clicked() {
            filter.clear();
        }
        return close;
    }

    let row_height = ui.spacing().interact_size.y;
    ui.style_mut().interaction.selectable_labels = false;
    // Every header fits at first; narrow windows scroll sideways.
    let widths: Vec<f32> = [
        ("Process", 200.0),
        ("PID", 64.0),
        ("Proto", 50.0),
        ("Local", 200.0),
        ("Remote", 200.0),
        ("State", 110.0),
    ]
    .iter()
    .map(|(label, width)| crate::sort::width(ui, label, *width))
    .collect();
    let min_width = widths.iter().sum::<f32>() + 80.0;
    crate::theme::table_panel(ui, "network", min_width, |ui| {
        crate::theme::table(ui)
        .id_salt("network_table")
        .sense(egui::Sense::click())
        .striped(true)
        .resizable(true)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::initial(widths[0]).clip(true))
        .column(Column::initial(widths[1]))
        .column(Column::initial(widths[2]))
        .column(Column::initial(widths[3]).clip(true))
        .column(Column::initial(widths[4]).clip(true))
        .column(Column::initial(widths[5]))
        .column(Column::remainder())
        .header(row_height, |mut h| {
            for (col, (label, numeric)) in [
                (crate::i18n::tr("Process"), false),
                ("PID", true),
                (crate::i18n::tr("Proto"), false),
                (crate::i18n::tr("Local"), false),
                (crate::i18n::tr("Remote"), false),
                (crate::i18n::tr("State"), false),
            ]
            .into_iter()
            .enumerate()
            {
                h.col(|ui| crate::theme::table_cell(ui, |ui| {header(ui, label, col, numeric, &mut sort); }));
            }
        })
        .body(|body| {
            body.rows(row_height, rows.len(), |mut row| {
                let (c, name, remote) = &rows[row.index()];
                row.col(|ui| crate::theme::table_cell(ui, |ui| {
                    ui.label(*name);
                }));
                row.col(|ui| crate::theme::table_cell(ui, |ui| {right(ui, c.pid.to_string()); }));
                row.col(|ui| crate::theme::table_cell(ui, |ui| {
                    ui.label(format!("{:?}", c.protocol));
                }));
                row.col(|ui| crate::theme::table_cell(ui, |ui| {
                    ui.label(c.local.to_string());
                }));
                row.col(|ui| crate::theme::table_cell(ui, |ui| {
                    ui.label(remote.as_str());
                }));
                row.col(|ui| crate::theme::table_cell(ui, |ui| {
                    ui.label(c.state.map(crate::i18n::debug).unwrap_or_default());
                }));
                row.col(|ui| crate::theme::table_cell(ui, |ui| {
                    if let Some(pair) = closable(c)
                        && ui
                            .small_button(crate::i18n::tr("Close"))
                            .on_hover_text(crate::i18n::tr(
                                "Reset this connection; both ends see it closed (needs administrator rights)",
                            ))
                            .clicked()
                    {
                        close = Some(NetworkAction::Close(pair.0, pair.1));
                    }
                }));
                let owner = owners.get(&c.pid).copied();
                menu(&row.response(), ("connection", c.protocol == wtm_core::Protocol::Tcp, c.local, c.remote, c.pid, owner.map(|p| p.create_time)), |ui| {
                    copy(ui, crate::i18n::tr("Local"), || c.local.to_string());
                    if let Some(remote) = c.remote {
                        copy(ui, crate::i18n::tr("Remote"), || remote.to_string());
                    }
                    if let Some(owner) = owner && action(ui, crate::i18n::tr("Show in Processes")) {
                        close = Some(NetworkAction::Reveal(wtm_core::control::ProcessRef::from(owner)));
                    }
                    if let Some((local, remote)) = closable(c) && action(ui, crate::i18n::tr("Close")) {
                        close = Some(NetworkAction::Close(local, remote));
                    }
                });
            });
        });
    });
    sort.store(ui, "network");
    close
}

pub fn service_operations(ui: &mut Ui, service: &wtm_core::ServiceEntry) -> Option<ServiceOp> {
    let mut result = None;
    for (label, enabled, op) in [
        (
            "Start",
            service.state == wtm_core::ServiceState::Stopped,
            ServiceOp::Start,
        ),
        (
            "Stop",
            service.state == wtm_core::ServiceState::Running,
            ServiceOp::Stop,
        ),
        (
            "Restart",
            service.state == wtm_core::ServiceState::Running,
            ServiceOp::Restart,
        ),
    ] {
        if ui
            .add_enabled(enabled, egui::Button::new(crate::i18n::tr(label)))
            .clicked()
        {
            result = Some(op);
        }
    }
    ui.menu_button(crate::i18n::tr("Startup type"), |ui| {
        for kind in [
            wtm_core::StartType::Automatic,
            wtm_core::StartType::Manual,
            wtm_core::StartType::Disabled,
        ] {
            if action(ui, &crate::i18n::debug(kind)) {
                result = Some(ServiceOp::StartType(kind));
            }
        }
    });
    result
}

/// Running services in green, stopped ones in secondary text, the others
/// (starting, stopping, paused) in orange.
fn state_color(ui: &Ui, state: wtm_core::ServiceState) -> Color32 {
    match state {
        wtm_core::ServiceState::Running => crate::theme::text_color(ui, GOOD),
        wtm_core::ServiceState::Stopped => ui.visuals().weak_text_color(),
        _ => crate::theme::text_color(ui, WARN),
    }
}

/// Returns the service the user right-clicked or clicked, if any.
pub fn services(
    ui: &mut Ui,
    data: &Data,
    filter: &mut String,
    command: &mut Option<UiCommand>,
) -> Option<wtm_core::ServiceEntry> {
    let group_id = egui::Id::unique("service-state-filter");
    let mut group = ui.data(|d| d.get_temp::<u8>(group_id)).unwrap_or(0);
    ui.horizontal_wrapped(|ui| {
        crate::theme::search_bar(
            ui,
            egui::Id::unique(("search", "services")),
            filter,
            crate::i18n::tr("name or description"),
            240.0,
        );
        for (value, label) in [(0, "All services"), (1, "Running"), (2, "Stopped")] {
            crate::theme::choice(ui, &mut group, value, crate::i18n::tr(label));
        }
        ui.data_mut(|d| d.insert_temp(group_id, group));
    });
    crate::theme::divider(ui);
    let list = match &data.services {
        None => {
            crate::theme::loading(ui, crate::i18n::tr("Loading services…"));
            return None;
        }
        Some(Err(e)) => {
            crate::theme::colored(ui, BAD, e);
            return None;
        }
        Some(Ok(l)) => l,
    };
    let f = filter.trim().to_lowercase();
    let mut rows: Vec<_> = list
        .iter()
        .filter(|s| {
            f.is_empty()
                || s.name.to_lowercase().contains(&f)
                || s.display_name.to_lowercase().contains(&f)
        })
        .collect();
    rows.retain(|s| match group {
        1 => s.state == wtm_core::ServiceState::Running,
        2 => s.state == wtm_core::ServiceState::Stopped,
        _ => true,
    });
    let mut sort = Sort::load(ui, "services", Sort::new(0, false));
    match sort.col {
        3 => sort.apply_text(&mut rows, |s| format!("{:?}", s.state)),
        4 => sort.apply_text(&mut rows, |s| format!("{:?}", s.start_type)),
        _ => sort.apply(&mut rows, |a, b, col| match col {
            0 => text(&a.name, &b.name),
            1 => text(&a.display_name, &b.display_name),
            2 => a.pid.cmp(&b.pid),
            _ => text(
                a.account.as_deref().unwrap_or(""),
                b.account.as_deref().unwrap_or(""),
            ),
        }),
    }

    ui.weak(crate::i18n::format(
        "{0} of {1} services",
        &[rows.len().to_string(), list.len().to_string()],
    ));
    if rows.is_empty() {
        crate::theme::empty_state(
            ui,
            crate::i18n::tr("No matching services"),
            crate::i18n::tr("Try another name or part of the description."),
        );
        if ui.button(crate::i18n::tr("Clear search")).clicked() {
            filter.clear();
        }
        return None;
    }
    let mut clicked = None;
    let row_height = ui.spacing().interact_size.y;
    ui.style_mut().interaction.selectable_labels = false;
    // Every header fits at first; narrow windows scroll sideways.
    let widths: Vec<f32> = [
        ("Name", 180.0),
        ("Description", 300.0),
        ("PID", 64.0),
        ("Status", 100.0),
        ("Startup", 90.0),
    ]
    .iter()
    .map(|(label, width)| crate::sort::width(ui, label, *width))
    .collect();
    let min_width = widths.iter().sum::<f32>() + 120.0;
    crate::theme::table_panel(ui, "services", min_width, |ui| {
        crate::theme::table(ui)
            .id_salt("services_table")
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::initial(widths[0]).clip(true))
            .column(Column::initial(widths[1]).clip(true))
            .column(Column::initial(widths[2]))
            .column(Column::initial(widths[3]))
            .column(Column::initial(widths[4]))
            .column(Column::remainder().clip(true))
            .header(row_height, |mut h| {
                for (col, (label, numeric)) in [
                    (crate::i18n::tr("Name"), false),
                    (crate::i18n::tr("Description"), false),
                    ("PID", true),
                    (crate::i18n::tr("Status"), false),
                    (crate::i18n::tr("Startup"), false),
                    (crate::i18n::tr("Account"), false),
                ]
                .into_iter()
                .enumerate()
                {
                    h.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            header(ui, label, col, numeric, &mut sort);
                        })
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, rows.len(), |mut row| {
                    let s = rows[row.index()];
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.label(&s.name);
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.label(&s.display_name);
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            right(
                                ui,
                                if s.pid == 0 {
                                    String::new()
                                } else {
                                    s.pid.to_string()
                                },
                            )
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.colored_label(state_color(ui, s.state), crate::i18n::debug(s.state));
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.label(s.start_type.map(crate::i18n::debug).unwrap_or_default());
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.label(s.account.as_deref().unwrap_or(""));
                        })
                    });
                    let response = row.response();
                    if response.clicked() || response.secondary_clicked() {
                        clicked = Some(s.clone());
                    }
                    menu(&response, ("service", &s.name), |ui| {
                        ui.strong(&s.display_name);
                        copy(ui, crate::i18n::tr("Name"), || s.name.clone());
                        if s.pid != 0 {
                            copy(ui, "PID", || s.pid.to_string());
                        }
                        crate::theme::divider(ui);
                        if let Some(op) = service_operations(ui, s) {
                            *command = Some(UiCommand::Service {
                                name: s.name.clone(),
                                op,
                            });
                            ui.close();
                        }
                    });
                });
            });
    });
    sort.store(ui, "services");
    clicked
}

pub struct HistoryView {
    pub hours: u32,
    pub selected: Option<String>,
    /// Set when the user asks to limit an app: (name, suggested cores).
    pub limit_request: Option<(String, f32)>,
}

impl Default for HistoryView {
    fn default() -> Self {
        Self {
            hours: 24,
            selected: None,
            limit_request: None,
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
    ui.horizontal_wrapped(|ui| {
        ui.label(crate::i18n::tr("Window:"));
        for (label, hours) in [
            ("1 h", 1),
            ("6 h", 6),
            ("24 h", 24),
            (crate::i18n::tr("7 days"), 168),
            (crate::i18n::tr("30 days"), 720),
        ] {
            if crate::theme::choice(ui, &mut view.hours, hours, label).changed() {
                changed = true;
            }
        }
        crate::theme::divider(ui);
        ui.weak(crate::i18n::tr("Click a column header to sort."));
    });
    crate::theme::divider(ui);

    let (stats, findings) = match &data.app_history {
        None => {
            crate::theme::loading(ui, crate::i18n::tr("Loading app history…"));
            return changed;
        }
        Some(Err(e)) => {
            ui.label(e);
            return changed;
        }
        Some(Ok(v)) => v,
    };

    if findings.is_empty() {
        ui.weak(crate::i18n::tr(
            "No long-term findings in the analysis window.",
        ));
    } else {
        ui.strong({
            let __wtm_i18n_0 = &(findings.len());
            crate::i18n::format("Findings ({0})", &[format!("{}", __wtm_i18n_0)])
        });
        for f in findings {
            crate::theme::colored(
                ui,
                WARN,
                format!(
                    "{}  —  {}: {}",
                    f.app,
                    finding_label(f.kind),
                    crate::i18n::message(&f.detail)
                ),
            );
        }
    }
    ui.add_space(0.0);

    let flagged: HashMap<String, String> = findings
        .iter()
        .map(|f| (f.app.to_lowercase(), finding_label(f.kind).to_owned()))
        .collect();
    let mut rows: Vec<_> = stats.iter().collect();
    let mut sort = Sort::load(ui, "history", Sort::new(2, true));
    let finding = |s: &wtm_policy::AppStats| {
        flagged
            .get(&s.app.to_lowercase())
            .cloned()
            .unwrap_or_default()
    };
    sort.apply(&mut rows, |a, b, col| match col {
        0 => text(&a.app, &b.app),
        1 => a.minutes.cmp(&b.minutes),
        2 => num(a.cpu_avg.into(), b.cpu_avg.into()),
        3 => num(a.cpu_p95.into(), b.cpu_p95.into()),
        4 => num(a.cpu_peak.into(), b.cpu_peak.into()),
        5 => num(a.gpu_avg.into(), b.gpu_avg.into()),
        6 => num(a.cpu_core_seconds, b.cpu_core_seconds),
        7 => a.mem_avg.cmp(&b.mem_avg),
        8 => a.mem_peak.cmp(&b.mem_peak),
        9 => num(a.mem_growth_per_hour, b.mem_growth_per_hour),
        _ => text(&finding(a), &finding(b)),
    });

    let chart_height = if view.selected.is_some() {
        (ui.available_height() * 0.5).clamp(110.0, 230.0)
    } else {
        0.0
    };
    let table_height = (ui.available_height() - chart_height - 48.0).max(60.0);
    let row_height = ui.spacing().interact_size.y;
    let selected = view.selected.clone();
    // Every numeric header fits at first.
    let widths: Vec<f32> = [
        "Seen", "Avg CPU", "p95 CPU", "Peak CPU", "Avg GPU", "CPU time", "Avg mem", "Peak mem",
        "Growth/h",
    ]
    .iter()
    .map(|label| crate::sort::width(ui, label, 80.0))
    .collect();
    let min_width = 200.0 + widths.iter().sum::<f32>() + 180.0;
    crate::theme::table_panel(ui, "history_columns", min_width, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("history_table")
            .max_height(table_height)
            .show(ui, |ui| {
                ui.style_mut().interaction.selectable_labels = false;
                let mut table = crate::theme::table(ui)
                    .id_salt("history_table_rows")
                    .striped(true)
                    .resizable(true)
                    .min_scrolled_height(0.0)
                    .max_scroll_height(table_height)
                    .sense(egui::Sense::click())
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .column(Column::initial(200.0).clip(true));
                for width in &widths {
                    table = table.column(Column::initial(*width));
                }
                table
                    .column(Column::remainder())
                    .header(row_height, |mut h| {
                        for (col, label) in [
                            crate::i18n::tr("App"),
                            crate::i18n::tr("Seen"),
                            crate::i18n::tr("Avg CPU"),
                            crate::i18n::tr("p95 CPU"),
                            crate::i18n::tr("Peak CPU"),
                            crate::i18n::tr("Avg GPU"),
                            crate::i18n::tr("CPU time"),
                            crate::i18n::tr("Avg mem"),
                            crate::i18n::tr("Peak mem"),
                            crate::i18n::tr("Growth/h"),
                            crate::i18n::tr("Finding"),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            let numeric = !matches!(col, 0 | 10);
                            h.col(|ui| {
                                crate::theme::table_cell(ui, |ui| {
                                    header(ui, label, col, numeric, &mut sort);
                                })
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(row_height, rows.len(), |mut row| {
                            let s = rows[row.index()];
                            row.set_selected(selected.as_deref() == Some(s.app.as_str()));
                            row.col(|ui| {
                                crate::theme::table_cell(ui, |ui| {
                                    ui.label(&s.app);
                                })
                            });
                            row.col(|ui| {
                                crate::theme::table_cell(ui, |ui| {
                                    right(ui, duration_label(s.minutes));
                                })
                            });
                            row.col(|ui| {
                                crate::theme::table_cell(ui, |ui| {
                                    right(ui, format!("{:.2}%", s.cpu_avg));
                                })
                            });
                            row.col(|ui| {
                                crate::theme::table_cell(ui, |ui| {
                                    right(ui, format!("{:.2}%", s.cpu_p95));
                                })
                            });
                            row.col(|ui| {
                                crate::theme::table_cell(ui, |ui| {
                                    right(ui, format!("{:.1}%", s.cpu_peak));
                                })
                            });
                            row.col(|ui| {
                                crate::theme::table_cell(ui, |ui| {
                                    right(ui, format!("{:.1}%", s.gpu_avg));
                                })
                            });
                            row.col(|ui| {
                                crate::theme::table_cell(ui, |ui| {
                                    right(ui, {
                                        let __wtm_i18n_0 = &(s.cpu_core_seconds / 3600.0);
                                        crate::i18n::format(
                                            "{0} core-h",
                                            &[format!("{:.2}", __wtm_i18n_0)],
                                        )
                                    })
                                })
                            });
                            row.col(|ui| {
                                crate::theme::table_cell(ui, |ui| {
                                    right(ui, format_bytes(s.mem_avg as f64));
                                })
                            });
                            row.col(|ui| {
                                crate::theme::table_cell(ui, |ui| {
                                    right(ui, format_bytes(s.mem_peak as f64));
                                })
                            });
                            row.col(|ui| {
                                crate::theme::table_cell(ui, |ui| {
                                    right(ui, {
                                        let __wtm_i18n_0 = &(mib(s.mem_growth_per_hour));
                                        let __wtm_i18n_1 = &(s.mem_growth_fit);
                                        crate::i18n::format(
                                            "{0} MiB (R² {1})",
                                            &[
                                                format!("{:+.0}", __wtm_i18n_0),
                                                format!("{:.2}", __wtm_i18n_1),
                                            ],
                                        )
                                    })
                                })
                            });
                            row.col(|ui| {
                                crate::theme::table_cell(ui, |ui| {
                                    if let Some(kind) = flagged.get(&s.app.to_lowercase()) {
                                        crate::theme::colored(ui, WARN, kind);
                                    }
                                })
                            });
                            let response = row.response();
                            let mut open = response.clicked();
                            menu(&response, ("app-history", &s.app), |ui| {
                                copy(ui, crate::i18n::tr("App"), || s.app.clone());
                                open |= action(ui, crate::i18n::tr("Details"));
                            });
                            if open {
                                view.selected = Some(s.app.clone());
                                changed = true;
                            }
                        });
                    });
            });
    });
    sort.store(ui, "history");

    if let Some(app) = view.selected.clone() {
        crate::theme::divider(ui);
        if let Some(s) = stats.iter().find(|s| s.app == app) {
            let cpus = data.snapshot.as_ref().map_or(1, |s| s.system.logical_cpus);
            let cores = suggested_cores(s.cpu_p95, cpus);
            ui.horizontal(|ui| {
                if ui
                    .button(crate::i18n::format(
                        "Limit {0}…",
                        std::slice::from_ref(&app),
                    ))
                    .on_hover_text({
                        crate::i18n::format(
                            "Create an app limit; suggested from its p95 usage: {0} cores.",
                            &[format!("{}", cores)],
                        )
                    })
                    .clicked()
                {
                    view.limit_request = Some((app.clone(), cores));
                }
            });
        }
        let app = &app;
        match &data.series {
            Some((name, rows)) if name == app && !rows.is_empty() => {
                series_charts(ui, app, rows, (chart_height - 44.0).clamp(66.0, 190.0))
            }
            _ => crate::theme::loading(ui, crate::i18n::tr("Loading the usage of this app…")),
        }
    }
    changed
}

/// What a long-term finding is about.
fn finding_label(kind: wtm_policy::FindingKind) -> &'static str {
    use wtm_policy::FindingKind as K;
    match kind {
        K::ChronicCpu => crate::i18n::tr("Constant CPU use"),
        K::ChronicGpu => crate::i18n::tr("Constant GPU use"),
        K::MemoryGrowth => crate::i18n::tr("Growing memory"),
        K::HandleGrowth => crate::i18n::tr("Growing handle count"),
        K::ThreadGrowth => crate::i18n::tr("Growing thread count"),
    }
}

/// Where a startup entry comes from, in words.
fn source_label(source: wtm_core::startup::StartupSource) -> &'static str {
    use wtm_core::startup::StartupSource as S;
    match source {
        S::Run => crate::i18n::tr("Registry"),
        S::Run32 => crate::i18n::tr("Registry (32-bit)"),
        S::Folder => crate::i18n::tr("Startup folder"),
    }
}

/// A budget that covers normal use (p95, a share of the machine) with some
/// headroom, in half cores, at least half a core.
pub fn suggested_cores(p95_percent: f32, logical_cpus: u32) -> f32 {
    let cores = p95_percent / 100.0 * logical_cpus.max(1) as f32 * 1.25;
    ((cores * 2.0).ceil() / 2.0).clamp(0.5, logical_cpus.max(1) as f32)
}

fn series_charts(ui: &mut Ui, app: &str, rows: &[wtm_policy::MinuteRow], height: f32) {
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
    ui.strong({
        crate::i18n::format(
            "{0} — per-minute usage (x: hours before latest sample)",
            &[app.to_string()],
        )
    });
    ui.columns(2, |cols| {
        Plot::new("hist_cpu")
            .height(height)
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
            .height(height)
            .include_y(0.0)
            .y_axis_label(crate::i18n::tr("MiB"))
            .allow_drag(false)
            .allow_scroll(false)
            .show(&mut cols[1], |p| {
                p.line(Line::new(crate::i18n::tr("Memory"), mem))
            });
    });
}

fn startup_scope_label(scope: wtm_core::startup::StartupScope) -> &'static str {
    crate::i18n::tr(match scope {
        wtm_core::startup::StartupScope::CurrentUser => "Current user",
        wtm_core::startup::StartupScope::AllUsers => "All users",
    })
}

/// Startup programs. Returns a command when the user toggles the selected entry.
pub fn startup(
    ui: &mut Ui,
    data: &Data,
    selected: &mut Option<wtm_core::startup::StartupEntry>,
    filter: &mut String,
) -> Option<crate::backend::UiCommand> {
    let Some(entries) = &data.startup else {
        crate::theme::loading(ui, crate::i18n::tr("Loading startup apps…"));
        return None;
    };
    let mut command = None;
    let group_id = egui::Id::unique("startup-state-filter");
    let mut group = ui.data(|d| d.get_temp::<u8>(group_id)).unwrap_or(0);
    ui.horizontal_wrapped(|ui| {
        crate::theme::search_bar(
            ui,
            egui::Id::unique(("search", "startup")),
            filter,
            &format!(
                "{} · {}",
                crate::i18n::tr("Name"),
                crate::i18n::tr("Command line")
            ),
            240.0,
        );
        for (value, label) in [(0, "All apps"), (1, "Enabled"), (2, "Disabled")] {
            crate::theme::choice(ui, &mut group, value, crate::i18n::tr(label));
        }
    });
    ui.data_mut(|d| d.insert_temp(group_id, group));
    crate::theme::divider(ui);

    let query = filter.trim().to_lowercase();
    let mut rows: Vec<_> = entries
        .iter()
        .filter(|entry| {
            (group == 0 || entry.enabled == (group == 1))
                && (query.is_empty()
                    || entry.name.to_lowercase().contains(&query)
                    || entry.command.to_lowercase().contains(&query)
                    || startup_scope_label(entry.scope)
                        .to_lowercase()
                        .contains(&query)
                    || source_label(entry.source).to_lowercase().contains(&query))
        })
        .collect();
    let mut sort = Sort::load(ui, "startup", Sort::new(0, false));
    sort.apply(&mut rows, |a, b, col| match col {
        0 => text(&a.name, &b.name),
        1 => a.enabled.cmp(&b.enabled),
        2 => text(startup_scope_label(a.scope), startup_scope_label(b.scope)),
        3 => text(source_label(a.source), source_label(b.source)),
        _ => text(&a.command, &b.command),
    });

    if let Some(entry) = selected.as_ref().and_then(|selected| {
        rows.iter().copied().find(|entry| {
            entry.name == selected.name
                && entry.scope == selected.scope
                && entry.source == selected.source
        })
    }) {
        ui.horizontal_wrapped(|ui| {
            // Re-resolve the selection so the button reflects the current state.
            ui.strong(&entry.name);
            let (label, enable) = if entry.enabled {
                (crate::i18n::tr("Disable"), false)
            } else {
                (crate::i18n::tr("Enable"), true)
            };
            if ui.button(label).clicked() {
                command = Some(crate::backend::UiCommand::SetStartup {
                    entry: entry.clone(),
                    enabled: enable,
                });
            }
        });
    }
    if rows.is_empty() {
        if !entries.is_empty() {
            ui.strong(crate::i18n::tr("No matching applications"));
            if ui.button(crate::i18n::tr("Clear filters")).clicked() {
                filter.clear();
                ui.data_mut(|d| d.insert_temp(group_id, 0_u8));
            }
        } else {
            crate::theme::empty_state(
                ui,
                crate::i18n::tr("No startup apps"),
                crate::i18n::tr("Nothing is set to start when you sign in."),
            );
        }
        return command;
    }
    if query.is_empty() && group == 0 {
        ui.weak(crate::i18n::format(
            "{0} startup apps, {1} enabled",
            &[
                rows.len().to_string(),
                rows.iter().filter(|e| e.enabled).count().to_string(),
            ],
        ));
    } else {
        ui.weak(crate::i18n::format(
            "{0} of {1} apps",
            &[rows.len().to_string(), entries.len().to_string()],
        ));
    }
    let row_height = ui.spacing().interact_size.y;
    ui.style_mut().interaction.selectable_labels = false;
    // Every header fits at first; narrow windows scroll sideways.
    let widths: Vec<f32> = [
        ("Name", 220.0),
        ("Status", 140.0),
        ("Scope", 100.0),
        ("Source", 80.0),
    ]
    .iter()
    .map(|(label, width)| crate::sort::width(ui, label, *width))
    .collect();
    let min_width = widths.iter().sum::<f32>() + 200.0;
    crate::theme::table_panel(ui, "startup", min_width, |ui| {
        crate::theme::table(ui)
            .id_salt("startup_table")
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::initial(widths[0]).clip(true))
            .column(Column::initial(widths[1]))
            .column(Column::initial(widths[2]))
            .column(Column::initial(widths[3]))
            .column(Column::remainder().clip(true))
            .header(row_height, |mut h| {
                for (col, label) in [
                    crate::i18n::tr("Name"),
                    crate::i18n::tr("Status"),
                    crate::i18n::tr("Scope"),
                    crate::i18n::tr("Source"),
                    crate::i18n::tr("Command"),
                ]
                .into_iter()
                .enumerate()
                {
                    h.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            header(ui, label, col, false, &mut sort);
                        })
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, rows.len(), |mut row| {
                    let e = rows[row.index()];
                    row.set_selected(selected.as_ref().is_some_and(|s| {
                        s.name == e.name && s.scope == e.scope && s.source == e.source
                    }));
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.label(&e.name);
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            let mut enabled = e.enabled;
                            let label = format!(
                                "{}: {}",
                                e.name,
                                crate::i18n::tr(if enabled { "Enabled" } else { "Disabled" })
                            );
                            ui.horizontal(|ui| {
                                if crate::theme::switch(ui, &mut enabled, &label).changed() {
                                    command = Some(crate::backend::UiCommand::SetStartup {
                                        entry: e.clone(),
                                        enabled,
                                    });
                                }
                                ui.weak(crate::i18n::tr(if e.enabled {
                                    "Enabled"
                                } else {
                                    "Disabled"
                                }));
                            });
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.label(startup_scope_label(e.scope));
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.label(source_label(e.source));
                        })
                    });
                    row.col(|ui| {
                        crate::theme::table_cell(ui, |ui| {
                            ui.label(&e.command);
                        })
                    });
                    let response = row.response();
                    if response.clicked() {
                        *selected = Some(e.clone());
                    }
                    menu(
                        &response,
                        (
                            "startup",
                            &e.name,
                            e.scope as u8,
                            e.source as u8,
                            &e.command,
                        ),
                        |ui| {
                            copy(ui, crate::i18n::tr("Name"), || e.name.clone());
                            if action(ui, crate::i18n::tr("Copy command line")) {
                                ui.ctx().copy_text(e.command.clone());
                            }
                            if action(
                                ui,
                                crate::i18n::tr(if e.enabled { "Disable" } else { "Enable" }),
                            ) {
                                command = Some(UiCommand::SetStartup {
                                    entry: e.clone(),
                                    enabled: !e.enabled,
                                });
                            }
                        },
                    );
                });
            });
    });
    sort.store(ui, "startup");
    command
}

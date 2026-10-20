//! Ports tab: which program listens on which port, the state of well-known
//! developer tools and servers, and ending whatever holds a port.

use std::collections::HashMap;

use eframe::egui::{self, Align, Layout, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use wtm_core::control::ProcessRef;
use wtm_core::ports::{self, Exposure, KNOWN_PORTS, KnownPort, PortUse};
use wtm_core::{ProcessInfo, Protocol, format_bytes};
use wtm_ipc::Command;

use crate::backend::Data;
use crate::processes::Intent;
use crate::sort::{Sort, header, num, text};
use crate::theme::{BAD, GOOD, WARN, card};

/// Categories whose ports usually speak HTTP, so "Open" makes sense.
const BROWSABLE: [&str; 5] = [
    ports::DEV_SERVER,
    ports::WEB,
    ports::MONITORING,
    ports::SEARCH,
    ports::AI,
];

#[derive(Debug, Default)]
pub struct PortsView {
    pub search: String,
    pub show_udp: bool,
    /// List the whole catalog, free ports too.
    pub show_catalog: bool,
    /// The "Free a port" box.
    pub free_input: String,
    /// The "Watch a port" box.
    pub watch_input: String,
    /// Ports the user wants to see always, in use or not.
    pub watched: Vec<u16>,
}

/// A port and the programs that hold it, with what is known about it.
struct PortRow<'a> {
    port: u16,
    known: Option<&'static KnownPort>,
    uses: Vec<(&'a PortUse, Option<&'a ProcessInfo>)>,
}

impl PortRow<'_> {
    fn targets(&self) -> Vec<ProcessRef> {
        let mut out: Vec<ProcessRef> = Vec::new();
        for (u, p) in &self.uses {
            if out.iter().any(|t| t.pid == u.pid) {
                continue;
            }
            out.push(p.map_or_else(
                || ProcessRef {
                    pid: u.pid,
                    create_time: 0,
                    name: format!("PID {}", u.pid),
                },
                ProcessRef::from,
            ));
        }
        out
    }

    fn process_text(&self) -> String {
        let mut names: Vec<String> = Vec::new();
        for (u, p) in &self.uses {
            let name = p.map_or_else(|| "?".to_owned(), |p| p.name.clone());
            let item = format!("{name} ({})", u.pid);
            if !names.contains(&item) {
                names.push(item);
            }
        }
        names.join(", ")
    }

    fn exposure(&self) -> Option<Exposure> {
        self.uses.iter().map(|(u, _)| u.exposure()).max()
    }

    fn connections(&self) -> usize {
        self.uses.iter().map(|(u, _)| u.connections).sum()
    }

    fn browsable(&self) -> bool {
        self.uses.iter().any(|(u, _)| u.protocol == Protocol::Tcp)
            && self.known.is_none_or(|k| BROWSABLE.contains(&k.category))
    }
}

/// Why the programs holding a port cannot be ended from here, if so.
fn blocked(row: &PortRow<'_>) -> Option<String> {
    row.uses.iter().find_map(|(u, p)| {
        let name = p.map_or("", |p| p.name.as_str());
        ports::end_blocked(u.pid, name).map(|r| format!("{name} ({}): {r}", u.pid))
    })
}

/// What else ending the programs behind a port would stop.
fn relay(row: &PortRow<'_>) -> Option<&'static str> {
    row.uses
        .iter()
        .find_map(|(_, p)| p.and_then(|p| ports::relay_hint(&p.name)))
}

fn end_intent(row: &PortRow<'_>, command: Command) -> Intent {
    let mut targets = row.targets();
    if targets.len() == 1 {
        Intent::Confirm(targets.remove(0), command)
    } else {
        Intent::ConfirmMany(targets, command)
    }
}

fn exposure_label(ui: &mut Ui, exposure: Exposure) {
    let (color, hint) = match exposure {
        Exposure::ThisPc => (
            GOOD,
            crate::i18n::tr("Only programs on this PC can connect"),
        ),
        Exposure::Network => (
            WARN,
            crate::i18n::tr("Reachable from one network this PC is on"),
        ),
        Exposure::AllNetworks => (
            WARN,
            crate::i18n::tr(
                "Reachable from every network this PC is on, unless the firewall blocks it",
            ),
        ),
    };
    ui.colored_label(color, exposure.label())
        .on_hover_text(hint);
}

fn open_url(ui: &Ui, port: u16) {
    ui.ctx()
        .open_url(egui::OpenUrl::new_tab(format!("http://localhost:{port}")));
}

/// Groups port uses by port, each with its process.
fn rows<'a>(
    uses: &'a [PortUse],
    procs: &HashMap<u32, &'a ProcessInfo>,
    udp: bool,
) -> Vec<PortRow<'a>> {
    let mut out: Vec<PortRow<'a>> = Vec::new();
    for u in uses.iter().filter(|u| udp || u.protocol == Protocol::Tcp) {
        let entry = (u, procs.get(&u.pid).copied());
        match out.iter_mut().find(|r| r.port == u.port) {
            Some(r) => r.uses.push(entry),
            None => out.push(PortRow {
                port: u.port,
                known: u.known(),
                uses: vec![entry],
            }),
        }
    }
    out
}

pub fn ports_tab(ui: &mut Ui, data: &Data, view: &mut PortsView, intents: &mut Vec<Intent>) {
    let conns = match &data.connections {
        None => {
            ui.spinner();
            return;
        }
        Some(Err(e)) => {
            ui.colored_label(BAD, e);
            return;
        }
        Some(Ok(c)) => c,
    };
    let uses = ports::port_uses(conns);
    let procs: HashMap<u32, &ProcessInfo> = data
        .snapshot
        .iter()
        .flat_map(|s| &s.processes)
        .map(|p| (p.pid, p))
        .collect();
    let all = rows(&uses, &procs, true);
    let tcp = all
        .iter()
        .filter(|r| r.uses.iter().any(|(u, _)| u.protocol == Protocol::Tcp))
        .count();
    let tools = all.iter().filter(|r| r.known.is_some()).count();
    let open = all
        .iter()
        .filter(|r| {
            r.uses
                .iter()
                .any(|(u, _)| u.protocol == Protocol::Tcp && u.exposure() == Exposure::AllNetworks)
        })
        .count();

    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new({
                crate::i18n::format("{0} TCP ports listening", &[format!("{}", tcp)])
            })
            .strong(),
        );
        ui.separator();
        ui.label({
            crate::i18n::format(
                "{0} well-known tools and servers running",
                &[format!("{}", tools)],
            )
        });
        ui.separator();
        ui.label(crate::i18n::format(
            "{0} open to all networks",
            &[format!("{}", open)],
        ))
        .on_hover_text(crate::i18n::tr(
            "Listening on 0.0.0.0 or ::, so other computers may reach them",
        ));
    });
    ui.add_space(4.0);
    free_port_bar(ui, &all, view, intents);
    ui.add_space(6.0);
    known_ports(ui, &all, view, intents);
    ui.add_space(6.0);
    listening_table(ui, &uses, &procs, view, intents);
}

/// "Free a port": ends whatever holds the typed port.
fn free_port_bar(
    ui: &mut Ui,
    all: &[PortRow<'_>],
    view: &mut PortsView,
    intents: &mut Vec<Intent>,
) {
    ui.horizontal(|ui| {
        ui.label(crate::i18n::tr("Free a port:"));
        let edit = ui.add(
            egui::TextEdit::singleline(&mut view.free_input)
                .hint_text("e.g. 3000")
                .desired_width(80.0),
        );
        let port = ports::parse_port(&view.free_input);
        let row = port.and_then(|p| all.iter().find(|r| r.port == p));
        match (port, row) {
            (None, _) if !view.free_input.trim().is_empty() => {
                ui.colored_label(BAD, crate::i18n::tr("Not a port number"));
            }
            (None, _) => {
                ui.weak(crate::i18n::tr(
                    "Ends the program that keeps a port busy (\"address already in use\").",
                ));
            }
            (Some(p), None) => {
                ui.colored_label(GOOD, {
                    crate::i18n::format("Port {0} is free.", &[format!("{}", p)])
                });
            }
            (Some(p), Some(row)) => {
                let blocked = blocked(row);
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let button = ui
                    .add_enabled(
                        blocked.is_none(),
                        egui::Button::new(
                            RichText::new({
                                crate::i18n::format("End what uses port {0}", &[format!("{}", p)])
                            })
                            .color(BAD),
                        ),
                    )
                    .on_disabled_hover_text(blocked.clone().unwrap_or_default());
                if (button.clicked() || enter) && blocked.is_none() {
                    intents.push(end_intent(row, Command::Kill));
                }
                ui.label(row.process_text());
                if let Some(why) = &blocked {
                    ui.weak(why);
                } else if let Some(hint) = relay(row) {
                    ui.colored_label(WARN, hint);
                }
            }
        }
    });
}

fn known_ports(ui: &mut Ui, all: &[PortRow<'_>], view: &mut PortsView, intents: &mut Vec<Intent>) {
    card(
        ui,
        RichText::new(crate::i18n::tr("Developer tools & servers")).strong(),
        |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut view.show_catalog,
                    crate::i18n::tr("Show free ports too"),
                );
                ui.separator();
                ui.label(crate::i18n::tr("Watch a port:"));
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut view.watch_input)
                        .hint_text("e.g. 8090")
                        .desired_width(70.0),
                );
                let port = ports::parse_port(&view.watch_input);
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui
                    .add_enabled(port.is_some(), egui::Button::new(crate::i18n::tr("Watch")))
                    .clicked()
                    || enter)
                    && let Some(p) = port
                {
                    if !view.watched.contains(&p) {
                        view.watched.push(p);
                        view.watched.sort_unstable();
                    }
                    view.watch_input.clear();
                }
            });
            // Watched ports first, then the catalog in its order.
            let mut list: Vec<(u16, Option<&'static KnownPort>, bool)> = view
                .watched
                .iter()
                .map(|&p| (p, ports::known_port(p), true))
                .collect();
            list.extend(
                KNOWN_PORTS
                    .iter()
                    .filter(|k| !view.watched.contains(&k.port))
                    .map(|k| (k.port, Some(k), false)),
            );
            let shown: Vec<_> = list
                .into_iter()
                .map(|(port, known, watched)| {
                    (port, known, watched, all.iter().find(|r| r.port == port))
                })
                .filter(|(_, _, watched, row)| *watched || view.show_catalog || row.is_some())
                .collect();
            if shown.is_empty() {
                ui.weak(crate::i18n::tr(
                    "None of the well-known developer tools or servers is running.",
                ));
                return;
            }
            let mut unwatch = None;
            egui::ScrollArea::vertical()
                .id_salt("known_ports")
                .max_height(240.0)
                .show(ui, |ui| {
                    egui::Grid::new("known_ports_grid")
                        .num_columns(8)
                        .striped(true)
                        .spacing([14.0, 4.0])
                        .show(ui, |ui| {
                            for h in [
                                "",
                                crate::i18n::tr("Port"),
                                crate::i18n::tr("Tool / server"),
                                crate::i18n::tr("Kind"),
                                crate::i18n::tr("Program"),
                                crate::i18n::tr("Reach"),
                                crate::i18n::tr("Clients"),
                                "",
                            ] {
                                ui.strong(h);
                            }
                            ui.end_row();
                            for (port, known, watched, row) in shown {
                                match row {
                                    Some(_) => ui
                                        .colored_label(GOOD, "●")
                                        .on_hover_text(crate::i18n::tr("In use")),
                                    None => ui.weak("○").on_hover_text(crate::i18n::tr("Free")),
                                };
                                ui.label(RichText::new(port.to_string()).monospace().strong());
                                ui.label(
                                    known.map_or(crate::i18n::tr("Watched port"), |k| k.service),
                                );
                                ui.weak(known.map_or("", |k| k.category));
                                match row {
                                    Some(r) => {
                                        let label = ui.label(r.process_text());
                                        if let Some(hint) = relay(r) {
                                            label.on_hover_text(hint);
                                        }
                                        match r.exposure() {
                                            Some(e) => exposure_label(ui, e),
                                            None => {
                                                ui.label("");
                                            }
                                        }
                                        ui.label(r.connections().to_string());
                                    }
                                    None => {
                                        ui.weak("free");
                                        ui.label("");
                                        ui.label("");
                                    }
                                }
                                ui.horizontal(|ui| {
                                    if let Some(r) = row {
                                        if r.browsable()
                                            && ui
                                                .small_button(crate::i18n::tr("Open"))
                                                .on_hover_text(format!("http://localhost:{port}"))
                                                .clicked()
                                        {
                                            open_url(ui, port);
                                        }
                                        let blocked = blocked(r);
                                        if ui
                                            .add_enabled(
                                                blocked.is_none(),
                                                egui::Button::new(
                                                    RichText::new(crate::i18n::tr("End"))
                                                        .color(BAD),
                                                )
                                                .small(),
                                            )
                                            .on_hover_text(relay(r).unwrap_or(crate::i18n::tr(
                                                "End the program to free the port",
                                            )))
                                            .on_disabled_hover_text(blocked.unwrap_or_default())
                                            .clicked()
                                        {
                                            intents.push(end_intent(r, Command::Kill));
                                        }
                                        if let Some((_, Some(p))) = r.uses.first()
                                            && ui
                                                .small_button(crate::i18n::tr("Show"))
                                                .on_hover_text(crate::i18n::tr("Show in Processes"))
                                                .clicked()
                                        {
                                            intents.push(Intent::Reveal(ProcessRef::from(*p)));
                                        }
                                    }
                                    if watched
                                        && ui
                                            .small_button("✖")
                                            .on_hover_text(crate::i18n::tr("Stop watching"))
                                            .clicked()
                                    {
                                        unwatch = Some(port);
                                    }
                                });
                                ui.end_row();
                            }
                        });
                });
            if let Some(p) = unwatch {
                view.watched.retain(|w| *w != p);
            }
        },
    );
}

fn listening_table(
    ui: &mut Ui,
    uses: &[PortUse],
    procs: &HashMap<u32, &ProcessInfo>,
    view: &mut PortsView,
    intents: &mut Vec<Intent>,
) {
    ui.horizontal(|ui| {
        ui.strong(crate::i18n::tr("All listening ports"));
        ui.separator();
        ui.label(crate::i18n::tr("Search:"));
        ui.add(
            egui::TextEdit::singleline(&mut view.search)
                .hint_text(crate::i18n::tr("port, program or tool"))
                .desired_width(200.0),
        );
        ui.checkbox(&mut view.show_udp, crate::i18n::tr("Include UDP"));
    });
    let f = view.search.trim().to_lowercase();
    let mut rows: Vec<(&PortUse, Option<&ProcessInfo>)> = uses
        .iter()
        .filter(|u| view.show_udp || u.protocol == Protocol::Tcp)
        .map(|u| (u, procs.get(&u.pid).copied()))
        .filter(|(u, p)| {
            f.is_empty()
                || u.port.to_string().contains(&f)
                || p.is_some_and(|p| p.name.to_lowercase().contains(&f))
                || u.known()
                    .is_some_and(|k| k.service.to_lowercase().contains(&f))
        })
        .collect();
    let name = |p: &Option<&ProcessInfo>| p.map_or("?", |p| p.name.as_str()).to_owned();
    let mut sort = Sort::load(ui, "ports", Sort::new(0, false));
    sort.apply(&mut rows, |(a, ap), (b, bp), col| match col {
        0 => a.port.cmp(&b.port),
        1 => text(&format!("{:?}", a.protocol), &format!("{:?}", b.protocol)),
        2 => text(
            a.known().map_or("", |k| k.service),
            b.known().map_or("", |k| k.service),
        ),
        3 => text(&name(ap), &name(bp)),
        4 => a.pid.cmp(&b.pid),
        5 => a.exposure().cmp(&b.exposure()),
        6 => a.connections.cmp(&b.connections),
        7 => num(
            f64::from(ap.map_or(0.0, |p| p.cpu_percent)),
            f64::from(bp.map_or(0.0, |p| p.cpu_percent)),
        ),
        _ => ap
            .map_or(0, |p| p.private_working_set)
            .cmp(&bp.map_or(0, |p| p.private_working_set)),
    });

    let row_height = ui.spacing().interact_size.y;
    ui.style_mut().interaction.selectable_labels = false;
    TableBuilder::new(ui)
        .id_salt("ports_table")
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::initial(64.0))
        .column(Column::initial(48.0))
        .column(Column::initial(190.0).clip(true))
        .column(Column::initial(170.0).clip(true))
        .column(Column::initial(64.0))
        .column(Column::initial(110.0))
        .column(Column::initial(60.0))
        .column(Column::initial(60.0))
        .column(Column::initial(80.0))
        .column(Column::remainder().clip(true))
        .header(row_height, |mut h| {
            for (col, (label, numeric)) in [
                (crate::i18n::tr("Port"), true),
                (crate::i18n::tr("Proto"), false),
                (crate::i18n::tr("Known as"), false),
                (crate::i18n::tr("Program"), false),
                ("PID", true),
                (crate::i18n::tr("Reach"), false),
                (crate::i18n::tr("Clients"), true),
                ("CPU", true),
                (crate::i18n::tr("Memory"), true),
                (crate::i18n::tr("Address"), false),
            ]
            .into_iter()
            .enumerate()
            {
                h.col(|ui| header(ui, label, col, numeric, &mut sort));
            }
        })
        .body(|body| {
            body.rows(row_height, rows.len(), |mut row| {
                let (u, p) = rows[row.index()];
                row.col(|ui| {
                    ui.label(RichText::new(u.port.to_string()).monospace());
                });
                row.col(|ui| {
                    ui.label(format!("{:?}", u.protocol).to_uppercase());
                });
                row.col(|ui| {
                    ui.label(u.known().map_or("", |k| k.service));
                });
                row.col(|ui| {
                    let label = ui.label(p.map_or("?", |p| p.name.as_str()));
                    if let Some(hint) = p.and_then(|p| ports::relay_hint(&p.name)) {
                        label.on_hover_text(hint);
                    }
                });
                row.col(|ui| {
                    ui.label(u.pid.to_string());
                });
                row.col(|ui| exposure_label(ui, u.exposure()));
                row.col(|ui| {
                    if u.protocol == Protocol::Tcp {
                        ui.label(u.connections.to_string());
                    }
                });
                row.col(|ui| {
                    ui.label(p.map_or(String::new(), |p| format!("{:.1}%", p.cpu_percent)));
                });
                row.col(|ui| {
                    ui.label(p.map_or(String::new(), |p| {
                        format_bytes(p.private_working_set as f64)
                    }));
                });
                row.col(|ui| {
                    ui.weak(u.address_text());
                });
                let response = row.response();
                if response.clicked()
                    && let Some(p) = p
                {
                    intents.push(Intent::Select(ProcessRef::from(p)));
                }
                response.context_menu(|ui| port_menu(ui, u, p, view, intents));
            });
        });
    sort.store(ui, "ports");
}

fn port_menu(
    ui: &mut Ui,
    u: &PortUse,
    p: Option<&ProcessInfo>,
    view: &mut PortsView,
    intents: &mut Vec<Intent>,
) {
    let port = u.port;
    if let Some(p) = p {
        let target = ProcessRef::from(p);
        let reason = ports::end_blocked(p.pid, &p.name);
        ui.add_enabled_ui(reason.is_none(), |ui| {
            if ui
                .button({
                    let __wtm_i18n_0 = &(p.name);
                    crate::i18n::format(
                        "End {0} (frees port {1})",
                        &[__wtm_i18n_0.to_string(), format!("{}", port)],
                    )
                })
                .clicked()
            {
                intents.push(Intent::Confirm(target.clone(), Command::Kill));
                ui.close();
            }
            if ui.button(crate::i18n::tr("End process tree")).clicked() {
                intents.push(Intent::Confirm(target.clone(), Command::KillTree));
                ui.close();
            }
        });
        if let Some(r) = reason {
            ui.weak(crate::i18n::format(
                "Not here: {0}",
                std::slice::from_ref(&r),
            ));
        } else if let Some(hint) = ports::relay_hint(&p.name) {
            ui.colored_label(WARN, hint);
        }
        if ui.button(crate::i18n::tr("Show in Processes")).clicked() {
            intents.push(Intent::Reveal(target.clone()));
            ui.close();
        }
        if ui.button(crate::i18n::tr("Open file location")).clicked() {
            intents.push(Intent::OpenLocation(target));
            ui.close();
        }
        if ui.button(crate::i18n::tr("Add to kill list…")).clicked() {
            intents.push(Intent::AddToKillList(p.name.clone()));
            ui.close();
        }
        ui.separator();
    }
    if u.protocol == Protocol::Tcp && ui.button(format!("Open http://localhost:{port}")).clicked() {
        open_url(ui, port);
        ui.close();
    }
    if ui
        .button(crate::i18n::format(
            "Copy localhost:{0}",
            &[format!("{}", port)],
        ))
        .clicked()
    {
        ui.ctx().copy_text(format!("localhost:{port}"));
        ui.close();
    }
    if !view.watched.contains(&port) && ui.button(crate::i18n::tr("Watch this port")).clicked() {
        view.watched.push(port);
        view.watched.sort_unstable();
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use wtm_core::{Connection, TcpState};

    struct State {
        data: Data,
        view: PortsView,
        intents: Vec<Intent>,
    }

    fn listen(local: &str, pid: u32) -> Connection {
        Connection {
            protocol: Protocol::Tcp,
            local: local.parse().unwrap(),
            remote: None,
            state: Some(TcpState::Listen),
            pid,
        }
    }

    fn harness(view: PortsView) -> Harness<'static, State> {
        let mut data = crate::ui_tests::data();
        data.connections = Some(Ok(vec![
            listen("0.0.0.0:5173", 100),
            listen("[::]:5173", 100),
            listen("127.0.0.1:5432", 200),
            listen("0.0.0.0:445", 4),
            listen("127.0.0.1:41234", 101),
            listen("127.0.0.1:3000", 100),
            listen("127.0.0.1:3000", 200),
        ]));
        Harness::builder()
            .allow_missing_glyphs()
            .with_size([1300.0, 900.0])
            .build_ui_state(
                |ui, s: &mut State| ports_tab(ui, &s.data, &mut s.view, &mut s.intents),
                State {
                    data,
                    view,
                    intents: Vec::new(),
                },
            )
    }

    #[test]
    fn known_tools_and_listening_ports_are_shown() {
        let mut h = harness(PortsView::default());
        h.run();
        for label in [
            "Vite",
            "PostgreSQL",
            "File sharing (SMB)",
            "All networks",
            "This PC only",
        ] {
            assert!(h.query_all_by_label(label).next().is_some(), "{label}");
        }
        assert!(h.query_by_label_contains("5 TCP ports listening").is_some());
        assert!(h.query_by_label_contains("4 well-known tools").is_some());
        // Free catalog entries only on request.
        assert!(h.query_by_label("Ollama").is_none());
        h.state_mut().view.show_catalog = true;
        h.run();
        assert!(h.query_by_label("Ollama").is_some());
    }

    #[test]
    fn a_busy_port_can_be_freed() {
        let mut h = harness(PortsView {
            free_input: "5173".into(),
            ..PortsView::default()
        });
        h.run();
        h.get_by_label("End what uses port 5173").click();
        h.run();
        assert!(matches!(
            h.state().intents.as_slice(),
            [Intent::Confirm(t, Command::Kill)] if t.pid == 100
        ));

        // Two programs on one port: both are ended together.
        h.state_mut().intents.clear();
        h.state_mut().view.free_input = ":3000".into();
        h.run();
        h.get_by_label("End what uses port 3000").click();
        h.run();
        assert!(matches!(
            h.state().intents.as_slice(),
            [Intent::ConfirmMany(ts, Command::Kill)] if ts.len() == 2
        ));

        // The kernel's ports cannot be ended; free ports say so.
        h.state_mut().intents.clear();
        h.state_mut().view.free_input = "445".into();
        h.run();
        h.get_by_label("End what uses port 445").click();
        h.run();
        assert!(h.state().intents.is_empty());
        h.state_mut().view.free_input = "9999".into();
        h.run();
        assert!(h.query_by_label("Port 9999 is free.").is_some());
        h.state_mut().view.free_input = "x".into();
        h.run();
        assert!(h.query_by_label("Not a port number").is_some());
    }

    #[test]
    fn watched_ports_are_listed_even_when_free() {
        let mut h = harness(PortsView {
            watch_input: "8090".into(),
            ..PortsView::default()
        });
        h.run();
        h.get_by_label("Watch").click();
        h.run();
        assert_eq!(h.state().view.watched, [8090]);
        assert!(h.query_by_label("Watched port").is_some());
        h.get_by_label("✖").click();
        h.run();
        assert!(h.state().view.watched.is_empty());
    }

    #[test]
    fn rows_select_and_offer_port_actions() {
        let mut h = harness(PortsView::default());
        h.run();
        h.get_all_by_label("PostgreSQL").last().unwrap().click();
        h.run();
        assert!(matches!(
            h.state().intents.as_slice(),
            [Intent::Select(t)] if t.pid == 200
        ));
        h.state_mut().intents.clear();
        h.get_all_by_label("PostgreSQL")
            .last()
            .unwrap()
            .click_secondary();
        h.run();
        h.get_by_label("End beta.exe (frees port 5432)").click();
        h.run();
        assert!(matches!(
            h.state().intents.as_slice(),
            [Intent::Confirm(t, Command::Kill)] if t.pid == 200
        ));
    }
}

//! Headless UI tests with egui_kittest: views are rendered from synthetic
//! data and driven through the accessibility tree.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use wtm_core::control::{EcoQos, PriorityClass, ProcessDetails, ProcessRef};
use wtm_core::startup::{StartupEntry, StartupScope, StartupSource};
use wtm_core::{
    Connection, ProcessInfo, Protocol, ServiceEntry, ServiceState, Session, SessionState, Snapshot,
    StartType, SystemStats, TcpState,
};
use wtm_ipc::{Command, Event, EventLevel, Managed, Status};
use wtm_policy::{AppStats, Finding, FindingKind, MinuteRow, Source};

use crate::backend::{Data, Mode, UiCommand};
use crate::processes::{Intent, ManagedMap, ProcessView};
use crate::views;

pub fn proc(pid: u32, parent_pid: u32, name: &str, cpu: f32) -> ProcessInfo {
    ProcessInfo {
        pid,
        parent_pid,
        name: name.into(),
        cpu_percent: cpu,
        cpu_time: Duration::from_secs(5),
        working_set: 20 << 20,
        private_working_set: u64::from(pid) << 20,
        private_bytes: 30 << 20,
        thread_count: 3,
        handle_count: 40,
        session_id: 1,
        base_priority: 8,
        gpu_percent: 1.5,
        gpu_memory: 1 << 20,
        io_read_rate: 1000.0,
        io_write_rate: 0.0,
        create_time: 116_444_736_000_000_000 + i64::from(pid),
    }
}

pub fn snapshot() -> Snapshot {
    Snapshot {
        interval: Some(Duration::from_secs(1)),
        system: SystemStats {
            cpu_percent: 42.0,
            logical_cpus: 8,
            memory_total: 16 << 30,
            memory_available: 8 << 30,
            commit_limit: 20 << 30,
            commit_available: 10 << 30,
            gpu_percent: 12.0,
            process_count: 5,
            thread_count: 15,
            handle_count: 200,
        },
        processes: vec![
            proc(0, 0, "System Idle Process", 50.0),
            proc(4, 0, "System", 1.0),
            proc(100, 4, "alpha.exe", 20.0),
            proc(101, 100, "alpha-child.exe", 2.0),
            proc(200, 4, "beta.exe", 0.5),
        ],
    }
}

fn target(pid: u32, name: &str) -> ProcessRef {
    ProcessRef::from(&proc(pid, 0, name, 0.0))
}

pub fn data() -> Data {
    let alpha = target(100, "alpha.exe");
    let row = |minute, cpu| MinuteRow {
        minute,
        cpu_avg: cpu,
        cpu_peak: cpu * 2.0,
        gpu_avg: cpu / 2.0,
        mem_avg: 100 << 20,
        mem_peak: 120 << 20,
        processes: 1,
    };
    let mut d = Data {
        mode: Mode::Local,
        snapshot: Some(Arc::new(snapshot())),
        status: Some(Status {
            dry_run: true,
            governor_enabled: true,
            high_cpu_percent: 90.0,
            recover_cpu_percent: 70.0,
            rules: 2,
            config_path: r"C:\ProgramData\RustWTM\config.toml".into(),
            managed: vec![Managed {
                target: alpha.clone(),
                sources: vec![Source::Governor],
                summary: "BelowNormal".into(),
            }],
            app_limits: vec![wtm_policy::GroupUsage {
                group: wtm_policy::GroupId {
                    app: "beta.exe".into(),
                    scope: wtm_policy::LimitScope::Group,
                    root: None,
                },
                limit: wtm_policy::AppLimit {
                    name: "beta.exe".into(),
                    cpu_cores: Some(1.5),
                    memory_gb: Some(8.0),
                    scope: wtm_policy::LimitScope::Group,
                },
                processes: 1,
                members: vec![(200, 116_444_736_000_000_200)],
                cpu_percent: 3.5,
                memory_bytes: 1 << 30,
            }],
        }),
        details: Some((
            alpha.clone(),
            Ok(ProcessDetails {
                path: Some(r"C:\Apps\alpha.exe".into()),
                command_line: Some(r#""C:\Apps\alpha.exe" --tray"#.into()),
                user: Some(r"PC\user".into()),
                priority: Some(PriorityClass::Normal),
                affinity: Some((0xff, 0xff)),
                eco_qos: Some(EcoQos::SystemManaged),
                critical: Some(false),
                protected: None,
            }),
        )),
        services: Some(Ok(vec![
            ServiceEntry {
                name: "Spooler".into(),
                display_name: "Print Spooler".into(),
                state: ServiceState::Running,
                pid: 300,
                start_type: Some(StartType::Automatic),
                binary_path: None,
                account: Some("LocalSystem".into()),
            },
            ServiceEntry {
                name: "Fax".into(),
                display_name: "Fax".into(),
                state: ServiceState::Stopped,
                pid: 0,
                start_type: Some(StartType::Manual),
                binary_path: None,
                account: None,
            },
        ])),
        sessions: Some(Ok(vec![
            Session {
                id: 0,
                station: "Services".into(),
                user: String::new(),
                domain: String::new(),
                state: SessionState::Disconnected,
            },
            Session {
                id: 1,
                station: "Console".into(),
                user: "alice".into(),
                domain: "PC".into(),
                state: SessionState::Active,
            },
        ])),
        connections: Some(Ok(vec![
            Connection {
                protocol: Protocol::Tcp,
                local: "127.0.0.1:8080".parse::<SocketAddr>().unwrap(),
                remote: None,
                state: Some(TcpState::Listen),
                pid: 100,
            },
            Connection {
                protocol: Protocol::Udp,
                local: "[::1]:53".parse().unwrap(),
                remote: None,
                state: None,
                pid: 999,
            },
        ])),
        startup: Some(vec![StartupEntry {
            name: "Updater".into(),
            command: r"C:\Apps\updater.exe /background".into(),
            scope: StartupScope::CurrentUser,
            source: StartupSource::Run,
            enabled: true,
        }]),
        app_history: Some(Ok((
            vec![AppStats {
                app: "alpha.exe".into(),
                minutes: 125,
                cpu_avg: 12.5,
                cpu_p95: 20.0,
                cpu_peak: 40.0,
                gpu_avg: 35.0,
                gpu_p95: 50.0,
                mem_avg: 100 << 20,
                mem_peak: 120 << 20,
                mem_last: 110 << 20,
                mem_growth_per_hour: 5.0 * 1024.0 * 1024.0,
                mem_growth_fit: 0.4,
                cpu_core_seconds: 3600.0,
            }],
            vec![Finding {
                app: "alpha.exe".into(),
                kind: FindingKind::ChronicCpu,
                detail: "average CPU 12.5%".into(),
            }],
        ))),
        series: Some(("alpha.exe".into(), vec![row(1, 5.0), row(2, 15.0)])),
        message: Some((EventLevel::Info, "ready".into())),
        ..Data::default()
    };
    d.events.push_back(Event {
        id: 1,
        unix_ms: 1_700_000_000_000,
        level: EventLevel::Warn,
        pid: Some(100),
        name: Some("alpha.exe".into()),
        message: "Governor: priority BelowNormal".into(),
        notify: true,
    });
    for i in 0..10 {
        d.history.cpu.push_back(i as f32 * 5.0);
        d.history.memory.push_back(50.0);
        d.history.gpu.push_back(3.0);
    }
    d
}

struct ProcState {
    view: ProcessView,
    snap: Snapshot,
    intents: Vec<String>,
}

fn describe_intent(i: &Intent) -> String {
    match i {
        Intent::Select(t) => format!("select {}", t.pid),
        Intent::Command(t, c) => format!("command {} {c:?}", t.pid),
        Intent::Confirm(t, c) => format!("confirm {} {c:?}", t.pid),
        Intent::OpenLocation(t) => format!("open {}", t.pid),
        Intent::LimitApp(name) => format!("limit app {name}"),
    }
}

fn process_harness() -> Harness<'static, ProcState> {
    Harness::builder()
        .allow_missing_glyphs()
        .with_size([1300.0, 700.0])
        .build_ui_state(
            |ui, st: &mut ProcState| {
                let managed: ManagedMap = HashMap::new();
                let snap = st.snap.clone();
                let intents = st.view.ui(ui, &snap, &extra(&managed));
                st.intents.extend(intents.iter().map(describe_intent));
            },
            ProcState {
                view: ProcessView::default(),
                snap: snapshot(),
                intents: Vec::new(),
            },
        )
}

#[test]
fn processes_table_sorts_filters_and_shows_tree() {
    let mut h = process_harness();
    h.run();
    assert!(h.query_by_label("alpha.exe").is_some());

    // Sorting by every column, both directions.
    for header in ["Name", "PID", "Memory", "Disk", "GPU", "Threads", "Handles"] {
        h.get_by_label_contains(header).click();
        h.run();
        h.get_by_label_contains(header).click();
        h.run();
    }
    assert!(!h.state().view.descending || h.state().view.sort == crate::processes::Col::Handles);

    // Tree view.
    h.get_by_label("Tree view").click();
    h.run();
    assert!(h.state().view.tree);

    // Search narrows the list.
    h.state_mut().view.search = "beta".into();
    h.run();
    assert!(h.query_by_label("alpha.exe").is_none());
    assert!(h.query_by_label("beta.exe").is_some());
    h.get_by_label("✖").click();
    h.run();
    assert!(h.state().view.search.is_empty());
}

#[test]
fn processes_row_click_and_context_menu_produce_intents() {
    let mut h = process_harness();
    h.run();
    h.get_by_label("beta.exe").click();
    h.run();
    assert!(h.state().intents.contains(&"select 200".to_owned()));

    for (item, expected) in [
        ("End task", "confirm 200 Kill"),
        ("End process tree", "confirm 200 KillTree"),
        ("Suspend", "command 200 Suspend"),
        ("Resume", "command 200 Resume"),
        ("Remove limits", "command 200 Unlimit"),
        ("Open file location", "open 200"),
        ("Limit all beta.exe…", "limit app beta.exe"),
        ("Properties", "select 200"),
    ] {
        h.get_by_label("beta.exe").click_secondary();
        h.run();
        h.get_by_label(item).click();
        h.run();
        assert!(
            h.state().intents.iter().any(|i| i == expected),
            "{item}: {:?}",
            h.state().intents
        );
    }

    // Submenus: hover to open, then pick an entry.
    for (menu, item, expected) in [
        ("Priority", "Idle", "command 200 Priority(Idle)"),
        ("Efficiency mode", "On", "command 200 EcoQos(On)"),
    ] {
        h.get_by_label("beta.exe").click_secondary();
        h.run();
        h.get_by_label(menu).hover();
        h.run();
        h.get_by_label(item).click();
        h.run();
        assert!(
            h.state().intents.iter().any(|i| i == expected),
            "{menu}/{item}"
        );
    }
    h.get_by_label("beta.exe").click_secondary();
    h.run();
    h.get_by_label("Limit CPU").hover();
    h.run();
    h.get_by_label_contains("1 core").click();
    h.run();
    assert!(h.state().intents.iter().any(|i| i.contains("Limit")));
    h.get_by_label("beta.exe").click_secondary();
    h.run();
    h.get_by_label("Limit memory").hover();
    h.run();
    h.get_by_label("512.0 MiB").click();
    h.run();
    assert!(
        h.state()
            .intents
            .iter()
            .filter(|i| i.contains("Limit"))
            .count()
            >= 2
    );
}

#[test]
fn read_only_views_render() {
    let mut d = data();
    let mut filter = String::new();
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size([1200.0, 900.0])
        .build_ui(|ui| {
            views::users(ui, &d);
            views::network(ui, &d, &mut filter, None);
            views::network(ui, &d, &mut "8080".to_owned(), Some(100));
        });
    h.run();
    assert!(h.query_by_label(r"PC\alice").is_some());
    assert!(h.get_all_by_label("127.0.0.1:8080").next().is_some());
    drop(h);

    // Loading and error states.
    d.sessions = None;
    d.connections = Some(Err("boom".into()));
    let mut h = Harness::builder().allow_missing_glyphs().build_ui(|ui| {
        views::users(ui, &d);
        views::network(ui, &d, &mut String::new(), None);
    });
    h.run_steps(2);
    assert!(h.query_by_label("boom").is_some());
}

struct PickState {
    data: Data,
    filter: String,
    service: Option<wtm_core::ServiceEntry>,
    startup: Option<StartupEntry>,
    commands: Vec<String>,
}

#[test]
fn services_and_startup_views_select_and_toggle() {
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size([1200.0, 700.0])
        .build_ui_state(
            |ui, st: &mut PickState| {
                // Two independent areas so neither table hides the other.
                ui.columns(2, |cols| {
                    if let Some(s) = views::services(&mut cols[0], &st.data, &mut st.filter) {
                        st.service = Some(s);
                    }
                    if let Some(cmd) = views::startup(&mut cols[1], &st.data, &mut st.startup) {
                        st.commands.push(match cmd {
                            UiCommand::SetStartup { entry, enabled } => {
                                format!("{} {enabled}", entry.name)
                            }
                            _ => "other".into(),
                        });
                    }
                });
            },
            PickState {
                data: data(),
                filter: String::new(),
                service: None,
                startup: None,
                commands: Vec::new(),
            },
        );
    h.run();
    h.get_by_label("Print Spooler").click();
    h.run();
    assert_eq!(h.state().service.as_ref().unwrap().name, "Spooler");

    h.get_by_label("Updater").click();
    h.run();
    h.get_by_label("Disable").click();
    h.run();
    assert_eq!(h.state().commands, ["Updater false"]);

    h.state_mut().filter = "fax".into();
    h.run();
    assert!(h.query_by_label("Print Spooler").is_none());
}

#[test]
fn app_history_view_switches_window_sort_and_selection() {
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size([1300.0, 900.0])
        .build_ui_state(
            |ui, st: &mut (Data, views::HistoryView, u32)| {
                if views::app_history(ui, &st.0, &mut st.1) {
                    st.2 += 1;
                }
            },
            (data(), views::HistoryView::default(), 0),
        );
    h.run();
    h.get_by_label("7 days").click();
    h.run();
    assert_eq!(h.state().1.hours, 168);
    // Every column header sorts; the second click reverses.
    for header in [
        "App", "Seen", "p95 CPU", "Peak CPU", "Avg GPU", "CPU time", "Avg mem", "Peak mem",
        "Growth/h", "Finding", "Finding", "Avg CPU",
    ] {
        // Headers come after the findings list, which may repeat a word.
        h.get_all_by_label_contains(header).last().unwrap().click();
        h.run();
    }
    assert!(h.query_by_label("Avg CPU ↓").is_some());
    h.get_by_label("alpha.exe").click();
    h.run_steps(3);
    assert_eq!(h.state().1.selected.as_deref(), Some("alpha.exe"));
    assert!(h.state().2 >= 2);

    // Error / loading states.
    h.state_mut().0.app_history = Some(Err("no service".into()));
    h.run();
    assert!(h.query_by_label("no service").is_some());
    h.state_mut().0.app_history = None;
    h.run_steps(2);
}

#[test]
fn backend_describe_covers_every_command() {
    use wtm_core::control::Limits;
    for c in [
        Command::Kill,
        Command::KillTree,
        Command::Suspend,
        Command::Resume,
        Command::Priority(PriorityClass::High),
        Command::Affinity(3),
        Command::EcoQos(EcoQos::On),
        Command::Limit(Limits {
            cpu_percent: Some(5.0),
            memory_bytes: Some(1 << 30),
        }),
        Command::Unlimit,
    ] {
        assert!(!crate::backend::describe(c).is_empty());
    }
}

#[test]
fn processes_show_limits_and_fit_cpu_limits_to_the_machine() {
    let d = data();
    let managed: ManagedMap = crate::processes::limit_labels(d.status.as_ref());
    let mut snap = snapshot();
    snap.system.logical_cpus = 1;
    let mut view = ProcessView::default();
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size([1300.0, 700.0])
        .build_ui(|ui| {
            view.ui(ui, &snap, &extra(&managed));
        });
    h.run();
    assert!(h.query_by_label("Governor: BelowNormal").is_some());
    assert!(h.query_by_label("App limit: 1.5 cores, 8 GB").is_some());
    h.get_by_label("beta.exe").click_secondary();
    h.run();
    h.get_by_label("Limit CPU").hover();
    h.run();
    // Only limits up to the single core are offered.
    assert!(h.query_by_label_contains("1 core").is_some());
    assert!(h.query_by_label_contains("2 cores").is_none());
}

#[test]
fn views_show_alternative_states() {
    let mut d = data();
    d.snapshot = None;
    if let Some(Ok(sessions)) = &mut d.sessions {
        sessions.push(Session {
            id: 2,
            station: "RDP-Tcp#1".into(),
            user: "bob".into(),
            domain: String::new(),
            state: SessionState::Active,
        });
    }
    if let Some(Ok(conns)) = &mut d.connections {
        conns.push(Connection {
            protocol: Protocol::Tcp,
            local: "192.168.1.2:50000".parse().unwrap(),
            remote: Some("10.0.0.5:443".parse().unwrap()),
            state: Some(TcpState::Established),
            pid: 200,
        });
    }
    if let Some(Ok(services)) = &mut d.services {
        services[1].state = ServiceState::StartPending;
    }
    if let Some(entries) = &mut d.startup {
        entries[0].enabled = false;
    }
    if let Some(Ok((stats, findings))) = &mut d.app_history {
        stats[0].minutes = 45;
        findings.clear();
    }
    let mut filter = "10.0.0.5".to_owned();
    let mut services_filter = String::new();
    let mut startup = d.startup.as_ref().map(|e| e[0].clone());
    let mut history = views::HistoryView {
        // The loaded series belongs to another app.
        selected: Some("beta.exe".into()),
        ..views::HistoryView::default()
    };
    let mut command = None;
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size([1600.0, 900.0])
        .build_ui(|ui| {
            ui.columns(3, |cols| {
                views::users(&mut cols[0], &d);
                views::network(&mut cols[0], &d, &mut filter, None);
                let _ = views::services(&mut cols[1], &d, &mut services_filter);
                if let Some(c) = views::startup(&mut cols[1], &d, &mut startup) {
                    command = Some(c);
                }
                views::app_history(&mut cols[2], &d, &mut history);
            });
        });
    h.run_steps(2);
    assert!(h.query_by_label("bob").is_some());
    assert!(h.query_by_label("10.0.0.5:443").is_some());
    assert!(h.query_by_label("127.0.0.1:8080").is_none());
    assert!(h.query_by_label("StartPending").is_some());
    assert!(h.query_by_label("45m").is_some());
    assert!(
        h.query_by_label("No long-term findings in the analysis window.")
            .is_some()
    );
    h.get_by_label("Enable").click();
    // The series spinner keeps repainting.
    h.run_steps(2);
    drop(h);
    assert!(matches!(
        command,
        Some(UiCommand::SetStartup { enabled: true, .. })
    ));

    // Errors from the backend are shown instead of the lists.
    d.sessions = Some(Err("sessions failed".into()));
    d.services = Some(Err("services failed".into()));
    let mut h = Harness::builder().allow_missing_glyphs().build_ui(|ui| {
        views::users(ui, &d);
        let _ = views::services(ui, &d, &mut String::new());
    });
    h.run_steps(2);
    assert!(h.query_by_label("sessions failed").is_some());
    assert!(h.query_by_label("services failed").is_some());
}

#[test]
fn every_table_sorts_by_every_column() {
    use egui_kittest::kittest::NodeT;
    let d = data();
    let mut selected = None;
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size([1600.0, 1400.0])
        .build_ui(|ui| {
            ui.push_id("u", |ui| views::users(ui, &d));
            ui.push_id("n", |ui| views::network(ui, &d, &mut String::new(), None));
            ui.push_id("s", |ui| views::services(ui, &d, &mut String::new()));
            ui.push_id("st", |ui| views::startup(ui, &d, &mut selected));
        });
    h.run();
    for header in [
        "User",
        "Session",
        "Station",
        "State",
        "Processes",
        "CPU",
        "Memory",
        "Process",
        "PID",
        "Proto",
        "Local",
        "Remote",
        "Name",
        "Description",
        "Status",
        "Startup",
        "Account",
        "Scope",
        "Source",
        "Command",
    ] {
        for node in h
            .get_all_by_role(eframe::egui::accesskit::Role::Button)
            .collect::<Vec<_>>()
        {
            if node
                .accesskit_node()
                .label()
                .is_some_and(|l| l.trim_end_matches([' ', '↑', '↓']) == header)
            {
                node.click();
            }
        }
        h.run();
    }
}

#[test]
fn process_columns_can_be_shown_hidden_and_sorted() {
    use egui_kittest::kittest::NodeT;
    let snap = snapshot();
    let managed = crate::processes::limit_labels(data().status.as_ref());
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size([2400.0, 900.0])
        .build_ui_state(
            |ui, view: &mut ProcessView| {
                view.ui(ui, &snap, &extra(&managed));
            },
            ProcessView::default(),
        );
    h.run();
    for label in [
        "Commit",
        "Working set",
        "GPU memory",
        "Session",
        "Base priority",
        "Running for",
    ] {
        h.get_by_label("Columns").click();
        h.run();
        h.get_by_label(label).click();
        h.run();
        assert!(h.state().view_has(label), "{label} not shown");
    }
    // Every visible header sorts.
    for header in [
        "Name",
        "PID",
        "CPU",
        "Memory",
        "Commit",
        "Working set",
        "Disk",
        "GPU",
        "GPU memory",
        "Threads",
        "Handles",
        "Session",
        "Base priority",
        "Running for",
        "Limits",
        "Command line",
    ] {
        let node = h
            .get_all_by_role(eframe::egui::accesskit::Role::Button)
            .find(|n| {
                n.accesskit_node()
                    .label()
                    .is_some_and(|l| l.trim_end_matches([' ', '↑', '↓']) == header)
            });
        node.unwrap_or_else(|| panic!("no header {header}")).click();
        h.run();
    }
    assert_eq!(h.state().sort, crate::processes::Col::CommandLine);
    assert!(h.query_by_label(r#""C:\Apps\beta.exe" --id 200"#).is_some());
    // Searching matches the command line.
    h.state_mut().search = "--id 200".into();
    h.run();
    assert!(h.query_by_label("beta.exe").is_some());
    assert!(h.query_by_label("alpha.exe").is_none());
    h.state_mut().search.clear();
    h.run();
    // Turning a column off again.
    h.get_by_label("Columns").click();
    h.run();
    h.get_by_role_and_label(eframe::egui::accesskit::Role::CheckBox, "Commit")
        .click();
    h.run();
    assert!(!h.state().view_has("Commit"));
}

/// Command lines for the synthetic processes.
fn command_lines() -> &'static crate::backend::CommandLines {
    static LINES: std::sync::OnceLock<crate::backend::CommandLines> = std::sync::OnceLock::new();
    LINES.get_or_init(|| {
        snapshot()
            .processes
            .iter()
            .map(|p| {
                (
                    (p.pid, p.create_time),
                    format!(r#""C:\Apps\{}" --id {}"#, p.name, p.pid),
                )
            })
            .collect()
    })
}

fn extra(managed: &ManagedMap) -> crate::processes::Extra<'_> {
    crate::processes::Extra {
        labels: managed,
        command_lines: command_lines(),
    }
}

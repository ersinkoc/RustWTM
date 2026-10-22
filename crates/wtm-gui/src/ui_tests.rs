//! Headless UI tests with egui_kittest: views are rendered from synthetic
//! data and driven through the accessibility tree.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
        net_rx_rate: 0.0,
        net_tx_rate: 0.0,
        suspended: false,
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
            core_percent: vec![95.0, 70.0, 10.0, 0.0],
            net_rx_rate: 2048.0,
            net_tx_rate: 512.0,
            io_read_rate: 1000.0,
            io_write_rate: 0.0,
            disks: vec![wtm_core::DiskUsage {
                name: "0 C:".into(),
                active_percent: 37.0,
                read_rate: 2048.0,
                write_rate: 0.0,
            }],
            gpu_engines: vec![
                ("3D".into(), 12.0),
                ("Copy".into(), 0.0),
                ("Compute 0".into(), 0.0),
                ("Video Decode".into(), 3.0),
                ("OFA_0".into(), 0.0),
            ],
        },
        processes: vec![
            proc(0, 0, "System Idle Process", 50.0),
            proc(4, 0, "System", 1.0),
            proc(100, 4, "alpha.exe", 20.0),
            proc(101, 100, "alpha-child.exe", 2.0),
            proc(200, 4, "beta.exe", 0.5),
        ],
        net_traced: false,
        taken_at_ms: 0,
    }
}

pub(crate) fn hybrid_topology() -> wtm_core::cpu::Topology {
    wtm_core::cpu::Topology {
        cpus: (0..8)
            .map(|index| wtm_core::cpu::LogicalCpu {
                id: u32::from(index),
                group: 0,
                index,
                core: index,
                efficiency_class: if index < 2 {
                    200
                } else if index < 6 {
                    100
                } else {
                    10
                },
                parked: false,
                reserved_elsewhere: false,
            })
            .collect(),
    }
}

#[test]
fn cpu_type_menu_sends_tier_selection_and_hides_on_uniform_processors() {
    struct State {
        topology: wtm_core::cpu::Topology,
        intents: Vec<Intent>,
    }
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size([600.0, 1200.0])
        .build_ui_state(
            |ui, s: &mut State| {
                crate::processes::context_menu(
                    ui,
                    &ProcessRef {
                        pid: 100,
                        create_time: 1,
                        name: "test.exe".into(),
                    },
                    8,
                    Some(&s.topology),
                    &mut s.intents,
                );
            },
            State {
                topology: hybrid_topology(),
                intents: Vec::new(),
            },
        );
    h.run();
    h.get_by_label("CPU core type").click();
    h.run();
    h.get_by_label("P-cores only").click();
    h.run();
    assert!(h.state().intents.iter().any(|i| matches!(
        i,
        Intent::Command(
            _,
            Command::AffinityTier(wtm_core::cpu::CoreSelection::Performance)
        )
    )));
    for c in &mut h.state_mut().topology.cpus {
        c.efficiency_class = 0;
    }
    h.run();
    assert!(h.query_by_label("CPU core type").is_none());
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
        ..MinuteRow::default()
    };
    let mut d = Data {
        mode: Mode::Local,
        sample_received_at: Some(Instant::now()),
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
                outside: 1,
                cpu_percent: 3.5,
                memory_bytes: 1 << 30,
            }],
            foreground: Some("alpha.exe".into()),
            rescue_core: Some(wtm_ipc::RescueStatus {
                cpus: vec![14, 15],
                moved: 42,
            }),
            game: None,
            config_error: None,
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
                ..AppStats::default()
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
    managed: ManagedMap,
    hung: crate::backend::HungMap,
}

fn describe_intent(i: &Intent) -> String {
    match i {
        Intent::Select(t) => format!("select {}", t.pid),
        Intent::Command(t, c) => format!("command {} {c:?}", t.pid),
        Intent::Confirm(t, c) => format!("confirm {} {c:?}", t.pid),
        Intent::OpenLocation(t) => format!("open {}", t.pid),
        Intent::LimitApp(name) => format!("limit app {name}"),
        Intent::AddToKillList(name) => format!("kill list {name}"),
        Intent::Reveal(t) => format!("reveal {}", t.pid),
        Intent::ConfirmMany(ts, c) => format!("confirm {} processes {c:?}", ts.len()),
        Intent::ShowPorts => "show ports".into(),
        Intent::Dump(t) => format!("dump {}", t.pid),
        Intent::WatchExit(t) => format!("watch exit {}", t.pid),
        Intent::WaitChain(t) => format!("wait chain {}", t.pid),
        Intent::WhyAlive(t) => format!("why alive {}", t.pid),
        Intent::Modules(t) => format!("modules {}", t.pid),
        Intent::Network(t, block) => format!("network {} {block}", t.pid),
    }
}

fn process_harness() -> Harness<'static, ProcState> {
    Harness::builder()
        .allow_missing_glyphs()
        .with_size([1300.0, 700.0])
        .build_ui_state(
            |ui, st: &mut ProcState| {
                let snap = st.snap.clone();
                let intents = st.view.ui(ui, &snap, &extra_with(&st.managed, &st.hung));
                st.intents.extend(intents.iter().map(describe_intent));
            },
            ProcState {
                view: ProcessView::default(),
                snap: snapshot(),
                intents: Vec::new(),
                managed: ManagedMap::new(),
                hung: HashMap::new(),
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
    h.get_by_label("Tree").click();
    h.run();
    assert_eq!(h.state().view.mode, crate::processes::ViewMode::Tree);

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
fn arrow_keys_move_the_selection_and_delete_ends_the_process() {
    use eframe::egui::Key;
    let mut h = process_harness();
    h.run();
    // Sorted by CPU: Idle (0), alpha (100), alpha-child (101), System, beta.
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(h.state().intents, ["select 0"]);

    h.state_mut().view.selected = Some(target(100, "alpha.exe"));
    h.state_mut().intents.clear();
    for key in [Key::ArrowDown, Key::ArrowUp, Key::Delete] {
        h.key_press(key);
        h.run();
    }
    assert_eq!(
        h.state().intents,
        ["select 101", "select 0", "confirm 100 Kill"]
    );

    // At the ends the selection stays put.
    h.state_mut().view.selected = Some(target(200, "beta.exe"));
    h.state_mut().intents.clear();
    h.key_press(Key::ArrowDown);
    h.run();
    assert!(h.state().intents.is_empty());

    // Typing in the search box keeps the keys for the text.
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, Key::F);
    h.run();
    h.key_press(Key::Delete);
    h.key_press(Key::ArrowUp);
    h.run();
    assert!(h.state().intents.is_empty());
}

#[test]
fn heat_map_shades_busy_cells_and_can_be_turned_off() {
    use crate::processes::heat_color;
    assert_eq!(heat_color(0.0), None);
    assert_eq!(heat_color(f64::NAN), None);
    let [_, low_g, _, low_a] = heat_color(0.01).unwrap().to_srgba_unmultiplied();
    let [_, high_g, _, high_a] = heat_color(0.9).unwrap().to_srgba_unmultiplied();
    assert!(
        high_a > low_a && high_g < low_g,
        "busier is stronger and redder"
    );
    assert_eq!(heat_color(5.0), heat_color(1.0));

    let mut h = process_harness();
    h.run();
    h.get_by_label("Columns").click();
    h.run();
    h.get_by_label("Heat map").click();
    h.run();
    assert!(!h.state().view.heat);
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
        ("Free memory", "command 200 TrimMemory"),
        ("Open file location", "open 200"),
        ("Limit all beta.exe…", "limit app beta.exe"),
        ("Add to kill list…", "kill list beta.exe"),
        ("Create memory dump file", "dump 200"),
        ("Notify me when it exits", "watch exit 200"),
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
            let _ = views::network(ui, &d, &mut filter, None);
            let _ = views::network(ui, &d, &mut "8080".to_owned(), Some(100));
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
        let _ = views::network(ui, &d, &mut String::new(), None);
    });
    h.run_steps(2);
    assert!(h.query_by_label("boom").is_some());
}

#[test]
fn performance_shows_cores_network_and_io() {
    let mut d = data();
    for v in [0.0, 4096.0, 2048.0] {
        d.history.net_rx.push_back(v);
        d.history.io_read.push_back(v);
    }
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size([1000.0, 1400.0])
        .build_ui(|ui| crate::app::performance(ui, &d));
    h.run();
    assert!(h.query_by_label("Per logical processor (4)").is_some());
    assert!(h.query_by_label("CPU 0: 95.0%").is_some());
    // The card titles carry the current rates as the graphs' legends.
    let net = format!(
        "Network● Receive {}/s● Send {}/s",
        wtm_core::format_bytes(2048.0),
        wtm_core::format_bytes(512.0)
    );
    assert!(h.query_by_label(&net).is_some(), "{net}");
    assert!(h.query_by_label_contains("Disk & I/O● Read").is_some());
    assert!(h.query_by_label("Disk 0 C:").is_some());
    assert!(h.query_by_label_contains("37% active").is_some());
    // The four busiest engines; the idle rest on one line.
    for engine in ["3D", "Video Decode", "Compute 0", "Copy"] {
        assert!(h.query_by_label(engine).is_some(), "{engine}");
    }
    assert!(h.query_by_label("OFA_0").is_none());
    assert!(h.query_by_label("+ 1 more engines").is_some());
    h.get_by_label("CPU 1: 70.0%").hover();
    h.run();
    assert!(
        h.get_all_by_label("CPU 1: 70.0%").count() >= 2,
        "hover tooltip"
    );
    drop(h);

    // Narrow windows stack the sections.
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size([700.0, 2400.0])
        .build_ui(|ui| crate::app::performance(ui, &d));
    h.run();
    assert!(h.query_by_label("System").is_some());
    assert!(h.query_by_label("Disk 0 C:").is_some());
}

#[test]
fn rate_plot_labels_show_bytes_per_second() {
    use egui_plot::{HoverPosition, PlotPoint};
    let position = PlotPoint::new(-3.0, 2048.0);
    let near = HoverPosition::NearDataPoint {
        plot_name: "Receive",
        position,
        index: 0,
    };
    let text = crate::app::rate_label(&near).unwrap();
    assert_eq!(
        text,
        format!("Receive: {}/s", wtm_core::format_bytes(2048.0))
    );
    assert!(crate::app::rate_label(&HoverPosition::Elsewhere { position }).is_none());
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
    h.get_by_label("Limit alpha.exe…").click();
    h.run();
    let (app, cores) = h.state_mut().1.limit_request.take().expect("requested");
    assert_eq!(app, "alpha.exe");
    assert!(cores >= 0.5);
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
        Command::TrimMemory,
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
                let _ = views::network(&mut cols[0], &d, &mut filter, None);
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
            ui.push_id("n", |ui| {
                let _ = views::network(ui, &d, &mut String::new(), None);
            });
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

type Icons = HashMap<String, Option<Arc<wtm_core::IconImage>>>;

/// Image paths for the synthetic processes and a red icon for beta.exe only.
fn icons() -> &'static (crate::backend::CommandLines, Icons) {
    static ICONS: std::sync::OnceLock<(crate::backend::CommandLines, Icons)> =
        std::sync::OnceLock::new();
    ICONS.get_or_init(|| {
        let paths: crate::backend::CommandLines = snapshot()
            .processes
            .iter()
            .map(|p| ((p.pid, p.create_time), format!(r"C:\Apps\{}", p.name)))
            .collect();
        let red = wtm_core::IconImage {
            size: 16,
            rgba: [220, 40, 40, 255].repeat(256),
        };
        let mut icons = Icons::new();
        icons.insert(r"C:\Apps\beta.exe".into(), Some(Arc::new(red)));
        icons.insert(r"C:\Apps\alpha.exe".into(), None);
        (paths, icons)
    })
}

fn extra(managed: &ManagedMap) -> crate::processes::Extra<'_> {
    static NONE_HUNG: std::sync::LazyLock<crate::backend::HungMap> =
        std::sync::LazyLock::new(HashMap::new);
    extra_with(managed, &NONE_HUNG)
}

fn extra_with<'a>(
    managed: &'a ManagedMap,
    hung: &'a crate::backend::HungMap,
) -> crate::processes::Extra<'a> {
    crate::processes::Extra {
        topology: None,
        labels: managed,
        command_lines: command_lines(),
        image_paths: &icons().0,
        icons: &icons().1,
        hung,
    }
}

#[test]
fn suggested_cores_cover_p95_with_headroom() {
    assert_eq!(views::suggested_cores(0.0, 8), 0.5);
    // 25% of 8 CPUs = 2 cores, +25% headroom = 2.5.
    assert_eq!(views::suggested_cores(25.0, 8), 2.5);
    assert_eq!(views::suggested_cores(100.0, 4), 4.0);
}

#[test]
fn apps_view_groups_processes_and_acts_on_all_of_them() {
    let mut h = process_harness();
    let mut snap = snapshot();
    snap.processes.push(proc(300, 1, "chrome.exe", 10.0));
    snap.processes.push(proc(301, 300, "Chrome.exe", 5.0));
    snap.processes.push(proc(302, 300, "chrome.exe", 2.5));
    h.state_mut().snap = snap;
    h.run();
    h.get_by_label("Apps").click();
    h.run();
    assert_eq!(h.state().view.mode, crate::processes::ViewMode::Apps);
    // One row with the total CPU of the three instances.
    assert!(h.query_by_label("chrome.exe (3)").is_some());
    assert!(h.query_by_label("17.5%").is_some());
    assert!(h.query_by_label("Chrome.exe").is_none(), "collapsed");

    // Clicking expands the app; clicking again collapses it.
    h.get_by_label("chrome.exe (3)").click();
    h.run();
    assert!(h.state().view.expanded.contains("chrome.exe"));
    assert!(h.query_by_label("Chrome.exe").is_some());
    h.get_by_label("chrome.exe (3)").click();
    h.run();
    assert!(h.state().view.expanded.is_empty());

    for (item, expected) in [
        ("Efficiency mode for all", "command 302 EcoQos(On)"),
        ("Free memory of all", "command 301 TrimMemory"),
        ("End all 3 processes", "confirm 3 processes Kill"),
        ("Limit all chrome.exe…", "limit app chrome.exe"),
    ] {
        h.get_by_label("chrome.exe (3)").click_secondary();
        h.run();
        h.get_by_label(item).click();
        h.run();
        assert!(
            h.state().intents.iter().any(|i| i == expected),
            "{item}: {:?}",
            h.state().intents
        );
    }
}

#[test]
fn ctrl_f_focuses_the_search_and_escape_clears_it() {
    let mut h = process_harness();
    h.run();
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::F);
    h.run();
    h.get_all_by_role(eframe::egui::accesskit::Role::TextInput)
        .next()
        .unwrap()
        .type_text("beta");
    h.run();
    assert_eq!(h.state().view.search, "beta");
    h.key_press(eframe::egui::Key::Escape);
    h.run();
    assert!(h.state().view.search.is_empty());
}

#[test]
fn table_searches_clear_and_explain_an_empty_result() {
    #[derive(Default)]
    struct Pages {
        network: String,
        services: String,
        ports: crate::ports_tab::PortsView,
    }
    let d = data();
    let no_match = "zzz-nothing-zzz".to_owned();
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size([1600.0, 900.0])
        .build_ui_state(
            |ui, pages: &mut Pages| {
                ui.columns(3, |cols| {
                    let _ = views::network(&mut cols[0], &d, &mut pages.network, None);
                    let _ = views::services(&mut cols[1], &d, &mut pages.services);
                    crate::ports_tab::ports_tab(
                        &mut cols[2],
                        &d,
                        &mut pages.ports,
                        &mut Vec::new(),
                    );
                });
            },
            Pages {
                network: no_match.clone(),
                services: no_match.clone(),
                ports: crate::ports_tab::PortsView {
                    search: no_match.clone(),
                    ..Default::default()
                },
            },
        );
    h.run_steps(2);
    for title in [
        "No matching connections",
        "No matching services",
        "No matching ports",
    ] {
        assert!(h.query_by_label(title).is_some(), "{title}");
    }
    // Each page offers its own way back, and the ✖ beside each field too.
    assert_eq!(h.query_all_by_label("Clear search").count(), 3);
    assert_eq!(h.query_all_by_label("✖").count(), 3);
    h.query_all_by_label("Clear search").next().unwrap().click();
    h.run_steps(2);
    assert!(h.state().network.is_empty());
    assert_eq!(h.state().services, no_match);

    // Escape in a focused field clears it.
    h.get_all_by_role(eframe::egui::accesskit::Role::TextInput)
        .nth(1)
        .unwrap()
        .focus();
    h.run();
    h.key_press(eframe::egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().services.is_empty());
    drop(h);

    // A narrow window at 200 % scale wraps the toolbars, and a long name
    // stays whole in its (clipped) cell.
    let mut d = d;
    let long = format!("Very long service display name {}", "x".repeat(120));
    if let Some(Ok(services)) = &mut d.services {
        services[0].display_name = long.clone();
    }
    let mut pages = Pages::default();
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size([360.0, 700.0])
        .with_pixels_per_point(2.0)
        .build_ui(|ui| {
            let _ = views::network(ui, &d, &mut pages.network, Some(100));
            let _ = views::services(ui, &d, &mut pages.services);
            crate::ports_tab::ports_tab(ui, &d, &mut pages.ports, &mut Vec::new());
        });
    h.run_steps(2);
    assert!(h.query_by_label("Include UDP").is_some());
    assert!(h.query_by_label(&long).is_some());
}

#[test]
fn paused_process_list_keeps_its_snapshot_and_resumes_live_data() {
    let mut h = process_harness();
    h.run();
    h.get_by_label("Pause updates").click();
    h.run();
    h.state_mut().snap.processes.retain(|p| p.pid != 100);
    h.state_mut()
        .snap
        .processes
        .push(proc(300, 4, "new.exe", 30.0));
    h.run();
    assert!(h.query_by_label("alpha.exe").is_some());
    assert!(h.query_by_label("new.exe").is_none());
    assert!(h.query_by_label("Paused · monitoring continues").is_some());
    // Search and selection still work against the retained process identity.
    h.state_mut().view.search = "alpha.exe".into();
    h.run();
    h.get_by_label("alpha.exe").click();
    h.run();
    assert!(h.state().intents.iter().any(|i| i == "select 100"));
    h.get_by_label("Resume updates").click();
    h.run();
    assert!(h.query_by_label("alpha.exe").is_none());
    assert!(h.query_by_label("No matching processes").is_some());
    h.get_by_label("Clear filters").click();
    h.run();
    assert!(h.query_by_label("new.exe").is_some());
    assert!(h.state().view.paused.is_none());
    h.key_press(eframe::egui::Key::F6);
    h.run();
    assert!(h.state().view.paused.is_some());
}

#[test]
fn not_responding_programs_are_marked_and_filterable() {
    let mut h = process_harness();
    h.run();
    assert!(
        h.query_by_label("Not responding").is_some(),
        "the quick filter"
    );
    h.state_mut().hung.insert(100, "Alpha - Document".into());
    h.run();
    assert_eq!(
        h.get_all_by_label("Not responding").count(),
        2,
        "filter and badge"
    );
    h.get_all_by_label("Not responding").next().unwrap().click();
    h.run();
    assert!(h.query_by_label("alpha.exe").is_some());
    assert!(h.query_by_label("beta.exe").is_none());
}

#[test]
fn recent_and_extra_filters_narrow_the_list_and_clear() {
    let mut h = process_harness();
    h.run();
    h.get_by_label("Started in the last 5 min").click();
    h.run();
    assert!(h.query_by_label("No matching processes").is_some());
    assert!(
        h.state()
            .view
            .visible
            .contains(&crate::processes::Col::Started)
    );
    // A process started a minute ago appears.
    let minute_ago = std::time::SystemTime::now() - Duration::from_secs(60);
    let since = minute_ago.duration_since(std::time::UNIX_EPOCH).unwrap();
    h.state_mut().snap.processes[2].create_time =
        116_444_736_000_000_000 + (since.as_nanos() / 100) as i64;
    h.run();
    assert!(h.query_by_label("alpha.exe").is_some());
    assert!(h.query_by_label("beta.exe").is_none());
    assert!(h.query_by_label("More filters (1)").is_some());

    // Session filter shows as a removable chip.
    h.state_mut().view.more.session = crate::processes::SessionFilter::Services;
    h.run();
    assert!(h.query_by_label("No matching processes").is_some());
    h.get_by_label("Services and system  ✖").click();
    h.run();
    assert!(h.query_by_label("alpha.exe").is_some());

    h.state_mut().view.more.disk_active = true;
    h.state_mut().view.search = "nothing-matches".into();
    h.run();
    h.get_by_label("Clear filters").click();
    h.run();
    assert!(h.state().view.more.is_empty());
    assert!(h.query_by_label("beta.exe").is_some());
}

#[test]
fn quick_filters_combine_with_search_and_exclude_idle_cpu() {
    let mut h = process_harness();
    h.run();
    h.get_by_label("High CPU").click();
    h.run();
    assert!(h.query_by_label("alpha.exe").is_some());
    assert!(h.query_by_label("System Idle Process").is_none());
    assert!(h.query_by_label("beta.exe").is_none());
    h.state_mut().view.search = "beta".into();
    h.run();
    assert!(h.query_by_label("No matching processes").is_some());
    h.get_by_label("Clear filters").click();
    h.run();
    h.get_by_label("High memory").click();
    h.run();
    assert!(h.query_by_label("No matching processes").is_some());
    h.state_mut().snap.processes[2].private_working_set = 512 * 1024 * 1024;
    h.run();
    assert!(
        h.query_by_label("alpha.exe").is_some(),
        "includes the exact boundary"
    );
    h.get_by_label("Managed").click();
    h.run();
    assert!(h.query_by_label("No matching processes").is_some());
    let p = h.state().snap.processes[2].clone();
    h.state_mut()
        .managed
        .insert((p.pid, p.create_time), "App limit: 2 cores".into());
    h.run();
    assert!(h.query_by_label("alpha.exe").is_some());
    assert!(h.query_by_label("beta.exe").is_none());
    h.get_by_label("All processes").click();
    h.state_mut().view.search = r"c:\apps\beta".into();
    h.run();
    assert!(
        h.query_by_label("beta.exe").is_some(),
        "search includes executable paths"
    );
    assert!(h.query_by_label("alpha.exe").is_none());
}

#[test]
fn suspended_processes_and_unmeasured_network_are_marked() {
    let mut h = process_harness();
    h.state_mut().snap.processes[1].suspended = true;
    h.run();
    assert!(h.query_by_label("Suspended").is_some());
    let rows = h.state().snap.processes.len();
    assert_eq!(
        h.query_all_by_label("—").count(),
        rows,
        "network not measured"
    );

    h.state_mut().snap.net_traced = true;
    h.run();
    assert!(h.query_by_label("—").is_none());
}

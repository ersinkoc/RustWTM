//! Overview tab: the machine at a glance, one-click protection, the busiest
//! apps with quick actions, and what the service did recently.

use std::collections::{HashMap, VecDeque};

use eframe::egui::{self, Color32, RichText, Ui};
use wtm_core::control::{EcoQos, ProcessRef};
use wtm_core::{ProcessInfo, format_bytes};
use wtm_ipc::Command;
use wtm_policy::{Config, Preset, set_preset};

use crate::backend::{Data, Mode, UiCommand};
use crate::processes::Intent;

const GOOD: Color32 = Color32::from_rgb(80, 180, 100);
const WARN: Color32 = Color32::from_rgb(230, 140, 40);
const BAD: Color32 = Color32::from_rgb(220, 70, 70);

fn load_color(percent: f32) -> Color32 {
    match percent {
        p if p >= 90.0 => BAD,
        p if p >= 70.0 => WARN,
        _ => GOOD,
    }
}

/// Half the machine is alarming; one full core is worth noticing.
fn app_color(cpu: f32, logical_cpus: u32) -> Color32 {
    if cpu >= 50.0 {
        BAD
    } else if cpu >= 100.0 / logical_cpus.max(1) as f32 {
        WARN
    } else {
        Color32::GRAY
    }
}

/// One resource at a glance: a ring for loads that have a ceiling, or a
/// big value for rates, and the last few minutes as a sparkline.
struct Stat<'a> {
    title: &'static str,
    accent: Color32,
    /// 0..=100 for loads; `None` for rates.
    ring: Option<f32>,
    value: String,
    detail: String,
    series: Vec<(&'a VecDeque<f32>, Color32)>,
    /// Top of the sparkline; `None` scales to the largest value shown.
    max: Option<f32>,
}

const STAT_HEIGHT: f32 = 128.0;
/// Seconds of history in a card's sparkline.
const STAT_SECONDS: usize = 60;

fn stat_card(ui: &mut Ui, s: &Stat<'_>) {
    let dark = ui.visuals().dark_mode;
    egui::Frame::new()
        .fill(if dark {
            Color32::from_rgb(26, 31, 38)
        } else {
            Color32::WHITE
        })
        .stroke(egui::Stroke::new(1.0, s.accent.gamma_multiply(0.45)))
        .corner_radius(10)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(STAT_HEIGHT - 24.0);
            ui.horizontal(|ui| {
                if let Some(p) = s.ring {
                    crate::theme::ring_gauge(ui, 60.0, p, load_color(p), &format!("{p:.0}%"));
                    ui.add_space(6.0);
                }
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(s.title)
                            .size(15.0)
                            .strong()
                            .color(crate::theme::text_color(ui, s.accent)),
                    );
                    if s.ring.is_none() {
                        ui.label(RichText::new(&s.value).size(20.0).strong());
                    }
                    ui.add(egui::Label::new(RichText::new(&s.detail).weak()).truncate());
                });
            });
            ui.add_space(4.0);
            let height = ui.available_height().max(20.0);
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), height),
                egui::Sense::hover(),
            );
            let max = s.max.unwrap_or_else(|| {
                s.series
                    .iter()
                    .flat_map(|(v, _)| v.iter().rev().take(STAT_SECONDS).copied())
                    .fold(1024.0, f32::max)
            });
            for (values, color) in &s.series {
                let tail: VecDeque<f32> = values
                    .iter()
                    .skip(values.len().saturating_sub(STAT_SECONDS))
                    .copied()
                    .collect();
                crate::theme::sparkline(ui.painter(), rect, &tail, max, STAT_SECONDS, *color);
            }
        });
}

/// The cards in as many equal columns as fit, wrapping to further rows.
fn stat_row(ui: &mut Ui, stats: &[Stat<'_>]) {
    let width = ui.available_width();
    let per_row = if width >= 1000.0 {
        stats.len()
    } else if width >= 640.0 {
        3
    } else {
        2
    };
    for chunk in stats.chunks(per_row.max(1)) {
        ui.columns(chunk.len(), |cols| {
            for (col, s) in cols.iter_mut().zip(chunk) {
                stat_card(col, s);
            }
        });
        ui.add_space(4.0);
    }
}

fn rate(bytes_per_sec: f64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

/// The verdict in a tinted strip.
fn banner(ui: &mut Ui, color: Color32, text: &str) {
    let icon = if color == GOOD { "✔" } else { "⚠" };
    egui::Frame::new()
        .fill(color.gamma_multiply(0.14))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.5)))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(format!("{icon}  {text}"))
                    .size(16.0)
                    .color(crate::theme::text_color(ui, color)),
            );
        });
}

/// One app: all processes with the same image name.
pub(crate) struct App<'a> {
    pub name: &'a str,
    pub cpu: f32,
    pub memory: u64,
    pub instances: Vec<ProcessRef>,
}

pub(crate) fn busiest_apps(data: &Data, n: usize) -> Vec<App<'_>> {
    let mut apps: HashMap<String, App<'_>> = HashMap::new();
    for p in data.snapshot.iter().flat_map(|s| &s.processes) {
        if p.pid == 0 || wtm_core::protection_reason(p.pid, &p.name).is_some() {
            continue;
        }
        let a = apps.entry(p.name.to_lowercase()).or_insert_with(|| App {
            name: &p.name,
            cpu: 0.0,
            memory: 0,
            instances: Vec::new(),
        });
        a.cpu += p.cpu_percent;
        a.memory += p.private_working_set;
        a.instances.push(ProcessRef::from(p));
    }
    let mut out: Vec<_> = apps.into_values().collect();
    out.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.memory.cmp(&a.memory)));
    out.truncate(n);
    out
}

/// One sentence on how the machine is doing and who is responsible.
fn verdict(data: &Data) -> Option<(Color32, String)> {
    let s = &data.snapshot.as_ref()?.system;
    let mem = s.memory_used() as f32 / s.memory_total.max(1) as f32 * 100.0;
    let apps = busiest_apps(data, usize::MAX);
    let top_cpu = apps.first();
    let top_mem = apps.iter().max_by_key(|a| a.memory);
    let cpu_blame = || {
        top_cpu.map_or(String::new(), |a| {
            let __wtm_i18n_0 = &(a.name);
            let __wtm_i18n_1 = &(a.cpu);
            crate::i18n::format(
                ": {0} uses {1}% of it",
                &[__wtm_i18n_0.to_string(), format!("{:.0}", __wtm_i18n_1)],
            )
        })
    };
    let mem_blame = || {
        top_mem.map_or(String::new(), |a| {
            let __wtm_i18n_0 = &(a.name);
            let __wtm_i18n_1 = &(format_bytes(a.memory as f64));
            crate::i18n::format(
                ": {0} uses {1} of it",
                &[__wtm_i18n_0.to_string(), __wtm_i18n_1.to_string()],
            )
        })
    };
    Some(if s.cpu_percent >= 90.0 {
        (BAD, {
            let __wtm_i18n_0 = &(cpu_blame());
            crate::i18n::format("CPU is maxed out{0}", std::slice::from_ref(__wtm_i18n_0))
        })
    } else if mem >= 90.0 {
        (BAD, {
            let __wtm_i18n_0 = &(mem_blame());
            crate::i18n::format(
                "Memory is almost full{0}",
                std::slice::from_ref(__wtm_i18n_0),
            )
        })
    } else if s.cpu_percent >= 70.0 {
        (WARN, {
            let __wtm_i18n_0 = &(cpu_blame());
            crate::i18n::format("CPU is busy{0}", std::slice::from_ref(__wtm_i18n_0))
        })
    } else if mem >= 80.0 {
        (WARN, {
            let __wtm_i18n_0 = &(mem_blame());
            crate::i18n::format(
                "Memory is getting full{0}",
                std::slice::from_ref(__wtm_i18n_0),
            )
        })
    } else {
        let swarms = wtm_policy::find_swarms(&data.snapshot.as_ref()?.processes, &data.swarm);
        match swarms.len() {
            0 => (
                GOOD,
                crate::i18n::tr("Everything runs smoothly.").to_owned(),
            ),
            1 => (WARN, {
                let __wtm_i18n_0 = &(swarms[0].members.len());
                let __wtm_i18n_1 = &(swarms[0].name);
                crate::i18n::format(
                    "{0} copies of {1} are running; see below.",
                    &[format!("{}", __wtm_i18n_0), __wtm_i18n_1.to_string()],
                )
            }),
            n => (WARN, {
                crate::i18n::format(
                    "{0} programs run hundreds of copies each; see below.",
                    &[format!("{}", n)],
                )
            }),
        }
    })
}

/// Swarm copies running this long count as stuck when the service does not
/// set its own limit.
const STUCK_MINS: u64 = 10;

/// Hundreds of copies of one program: who started them and one click to end
/// them all, or only those nothing waits for any more.
fn swarms(ui: &mut Ui, data: &Data, procs: &[ProcessInfo], intents: &mut Vec<Intent>) {
    let swarms = wtm_policy::find_swarms(procs, &data.swarm);
    if swarms.is_empty() {
        return;
    }
    let by_pid: HashMap<u32, &ProcessInfo> = procs.iter().map(|p| (p.pid, p)).collect();
    for s in &swarms {
        ui.add_space(6.0);
        crate::theme::card(
            ui,
            RichText::new({
                let __wtm_i18n_0 = &(s.members.len());
                let __wtm_i18n_1 = &(s.name);
                crate::i18n::format(
                    "⚠ {0} copies of {1} are running",
                    &[format!("{}", __wtm_i18n_0), __wtm_i18n_1.to_string()],
                )
            })
            .size(16.0)
            .strong()
            .color(crate::theme::text_color(ui, WARN)),
            |ui| {
                ui.label({ let __wtm_i18n_0 = &(s.parents_text()); let __wtm_i18n_1 = &(format_bytes(s.memory as f64)); let __wtm_i18n_2 = &(s.cpu_percent); crate::i18n::format("Started by {0}. Together {1} of memory and {2}% CPU. Usually a tool that keeps starting helpers and never waits for them.", &[__wtm_i18n_0.to_string(), __wtm_i18n_1.to_string(), format!("{:.1}", __wtm_i18n_2)]) });
                ui.horizontal_wrapped(|ui| {
                    let all: Vec<ProcessRef> =
                        s.members.iter().map(|p| ProcessRef::from(*p)).collect();
                    if ui
                        .button(
                            RichText::new({
                                let __wtm_i18n_0 = &(all.len());
                                crate::i18n::format("End all {0}", &[format!("{}", __wtm_i18n_0)])
                            })
                            .color(crate::theme::text_color(ui, BAD)),
                        )
                        .clicked()
                    {
                        intents.push(Intent::ConfirmMany(all, Command::Kill));
                    }
                    let orphans: Vec<ProcessRef> = s
                        .members
                        .iter()
                        .filter(|p| wtm_policy::live_parent(p, &by_pid).is_none())
                        .map(|p| ProcessRef::from(*p))
                        .collect();
                    if !orphans.is_empty()
                        && ui
                            .button({
                                let __wtm_i18n_0 = &(orphans.len());
                                crate::i18n::format(
                                    "End the {0} whose parent exited",
                                    &[format!("{}", __wtm_i18n_0)],
                                )
                            })
                            .on_hover_text(crate::i18n::tr("Nothing waits for these any more"))
                            .clicked()
                    {
                        intents.push(Intent::ConfirmMany(orphans, Command::Kill));
                    }
                    let mins = match data.swarm.end_after_mins {
                        0 => STUCK_MINS,
                        m => m,
                    };
                    let stuck: Vec<ProcessRef> = s
                        .older_than(
                            std::time::Duration::from_secs(mins * 60),
                            std::time::SystemTime::now(),
                        )
                        .into_iter()
                        .map(ProcessRef::from)
                        .collect();
                    if !stuck.is_empty()
                        && stuck.len() < s.members.len()
                        && ui
                            .button({
                                let __wtm_i18n_0 = &(stuck.len());
                                crate::i18n::format(
                                    "End the {0} running over {1} min",
                                    &[format!("{}", __wtm_i18n_0), format!("{}", mins)],
                                )
                            })
                            .on_hover_text(crate::i18n::tr(
                                "Stuck helpers, with the console hosts they started",
                            ))
                            .clicked()
                    {
                        intents.push(Intent::ConfirmMany(stuck, Command::KillTree));
                    }
                    if ui
                        .button(crate::i18n::tr("Add to kill list…"))
                        .on_hover_text(crate::i18n::tr(
                            "End them automatically or with the panic button",
                        ))
                        .clicked()
                    {
                        intents.push(Intent::AddToKillList(s.name.clone()));
                    }
                    if ui.button(crate::i18n::tr("Show")).clicked() {
                        intents.push(Intent::Reveal(ProcessRef::from(s.members[0])));
                    }
                });
            },
        );
    }
}

/// Returns commands for the backend and intents for the app.
pub fn overview(ui: &mut Ui, data: &Data) -> (Vec<UiCommand>, Vec<Intent>) {
    let mut commands = Vec::new();
    let mut intents = Vec::new();

    if let Some(snap) = &data.snapshot {
        let s = &snap.system;
        let mem = if s.memory_total > 0 {
            s.memory_used() as f32 / s.memory_total as f32 * 100.0
        } else {
            0.0
        };
        let h = &data.history;
        use crate::theme::{CPU, DISK_READ, DISK_WRITE, GPU, MEMORY, NET_RX, NET_TX};
        let stats = [
            Stat {
                title: "CPU",
                accent: CPU,
                ring: Some(s.cpu_percent),
                value: String::new(),
                detail: {
                    let __wtm_i18n_0 = &(s.logical_cpus);
                    crate::i18n::format("{0} logical CPUs", &[format!("{}", __wtm_i18n_0)])
                },
                series: vec![(&h.cpu, CPU)],
                max: Some(100.0),
            },
            Stat {
                title: crate::i18n::tr("Memory"),
                accent: MEMORY,
                ring: Some(mem),
                value: String::new(),
                detail: {
                    let __wtm_i18n_0 = &(format_bytes(s.memory_used() as f64));
                    let __wtm_i18n_1 = &(format_bytes(s.memory_total as f64));
                    crate::i18n::format(
                        "{0} of {1}",
                        &[__wtm_i18n_0.to_string(), __wtm_i18n_1.to_string()],
                    )
                },
                series: vec![(&h.memory, MEMORY)],
                max: Some(100.0),
            },
            Stat {
                title: "GPU",
                accent: GPU,
                ring: Some(s.gpu_percent),
                value: String::new(),
                detail: crate::i18n::tr("busiest engine").into(),
                series: vec![(&h.gpu, GPU)],
                max: Some(100.0),
            },
            Stat {
                title: crate::i18n::tr("Disk"),
                accent: DISK_READ,
                ring: None,
                value: rate(s.io_read_rate + s.io_write_rate),
                detail: {
                    let __wtm_i18n_0 = &(rate(s.io_read_rate));
                    let __wtm_i18n_1 = &(rate(s.io_write_rate));
                    crate::i18n::format(
                        "read {0} · write {1}",
                        &[__wtm_i18n_0.to_string(), __wtm_i18n_1.to_string()],
                    )
                },
                series: vec![(&h.io_read, DISK_READ), (&h.io_write, DISK_WRITE)],
                max: None,
            },
            Stat {
                title: crate::i18n::tr("Network"),
                accent: NET_RX,
                ring: None,
                value: rate(s.net_rx_rate + s.net_tx_rate),
                detail: format!("↓ {} · ↑ {}", rate(s.net_rx_rate), rate(s.net_tx_rate)),
                series: vec![(&h.net_rx, NET_RX), (&h.net_tx, NET_TX)],
                max: None,
            },
        ];
        stat_row(ui, &stats);
        if let Some((color, text)) = verdict(data) {
            ui.add_space(4.0);
            banner(ui, color, &text);
        }
        swarms(ui, data, &snap.processes, &mut intents);
    } else {
        ui.spinner();
    }
    ui.add_space(6.0);
    crate::theme::card(
        ui,
        RichText::new(crate::i18n::tr("Automatic protection"))
            .size(16.0)
            .strong(),
        |ui| {
            protection(ui, data, &mut commands);
        },
    );
    ui.add_space(6.0);

    let busiest = |ui: &mut Ui, intents: &mut Vec<Intent>| {
        crate::theme::card(
            ui,
            RichText::new(crate::i18n::tr("Busiest apps right now"))
                .size(16.0)
                .strong(),
            |ui| {
                busiest_table(ui, data, intents);
            },
        );
    };
    let recent = |ui: &mut Ui| {
        crate::theme::card(
            ui,
            RichText::new(crate::i18n::tr("Recent automatic actions"))
                .size(16.0)
                .strong(),
            |ui| {
                recent_actions(ui, data);
            },
        );
    };
    if ui.available_width() >= 1300.0 {
        ui.columns(2, |cols| {
            busiest(&mut cols[0], &mut intents);
            recent(&mut cols[1]);
        });
    } else {
        busiest(ui, &mut intents);
        ui.add_space(6.0);
        recent(ui);
    }
    (commands, intents)
}

fn busiest_table(ui: &mut Ui, data: &Data, intents: &mut Vec<Intent>) {
    let apps = busiest_apps(data, 8);
    egui::Grid::new("overview_apps")
        .num_columns(5)
        .striped(true)
        .show(ui, |ui| {
            for h in [
                crate::i18n::tr("App"),
                "CPU",
                crate::i18n::tr("Memory"),
                crate::i18n::tr("Processes"),
                "",
            ] {
                ui.strong(h);
            }
            ui.end_row();
            for a in &apps {
                ui.horizontal(|ui| {
                    let first = &a.instances[0];
                    let icon = data
                        .image_paths
                        .get(&(first.pid, first.create_time))
                        .and_then(|path| Some((path.as_str(), data.icons.get(path)?.as_deref()?)));
                    crate::processes::app_icon(ui, icon);
                    ui.label(a.name);
                });
                let cpus = data.snapshot.as_ref().map_or(1, |s| s.system.logical_cpus);
                ui.label(
                    RichText::new(format!("{:.1}%", a.cpu))
                        .color(crate::theme::text_color(ui, app_color(a.cpu, cpus))),
                );
                ui.label(format_bytes(a.memory as f64));
                ui.label(a.instances.len().to_string());
                ui.horizontal(|ui| {
                    if ui
                        .small_button(crate::i18n::tr("Efficiency mode"))
                        .on_hover_text(crate::i18n::tr(
                            "Run every process of this app in efficiency mode (EcoQoS).",
                        ))
                        .clicked()
                    {
                        for t in &a.instances {
                            intents.push(Intent::Command(t.clone(), Command::EcoQos(EcoQos::On)));
                        }
                    }
                    if ui
                        .small_button(crate::i18n::tr("Free memory"))
                        .on_hover_text(crate::i18n::tr(
                            "Move this app's memory to the standby list; it comes back as needed.",
                        ))
                        .clicked()
                    {
                        for t in &a.instances {
                            intents.push(Intent::Command(t.clone(), Command::TrimMemory));
                        }
                    }
                    if ui
                        .small_button(crate::i18n::tr("Limit…"))
                        .on_hover_text(crate::i18n::tr(
                            "Give this app a permanent CPU / memory budget.",
                        ))
                        .clicked()
                    {
                        intents.push(Intent::LimitApp(a.name.to_owned()));
                    }
                    if ui.small_button(crate::i18n::tr("Details")).clicked() {
                        intents.push(Intent::Reveal(a.instances[0].clone()));
                    }
                });
                ui.end_row();
            }
        });
}

fn recent_actions(ui: &mut Ui, data: &Data) {
    let recent: Vec<_> = data
        .events
        .iter()
        .rev()
        .filter(|e| e.notify)
        .take(8)
        .collect();
    if recent.is_empty() {
        ui.weak(crate::i18n::tr(
            "Nothing yet. RustWTM steps in when the machine is overloaded or a rule matches.",
        ));
    }
    for e in recent {
        let when = clock(e.unix_ms);
        ui.label(format!("{when}  {}", crate::i18n::message(&e.message)));
    }
}

/// `HH:MM:SS` in local time.
fn clock(unix_ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(unix_ms as i64)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}
fn protection(ui: &mut Ui, data: &Data, commands: &mut Vec<UiCommand>) {
    if !matches!(data.mode, Mode::Service { .. }) {
        let error = data.service_error.as_deref().unwrap_or_default();
        if error.contains("protocol") {
            ui.colored_label(
                WARN,
                { let __wtm_i18n_0 = &(env!("CARGO_PKG_VERSION")); crate::i18n::format("The installed RustWTM service is a different version ({0}), so nothing is limited automatically. Update it with the installer of version {1}.", &[error.to_string(), __wtm_i18n_0.to_string()]) },
            );
            ui.hyperlink_to(
                crate::i18n::tr("Download the latest release"),
                format!("{}/releases/latest", crate::about::REPO),
            );
        } else {
            ui.colored_label(
                WARN,
                crate::i18n::tr(
                    "The RustWTM service is not running, so nothing is limited automatically.",
                ),
            );
            ui.label(crate::i18n::tr(
                "Install and start it from an elevated prompt:",
            ));
            ui.code("wtm-service install\nwtm-service start");
        }
        return;
    }
    let admin = matches!(data.mode, Mode::Service { admin: true, .. });
    let Some(Ok((_, text))) = &data.config else {
        ui.spinner();
        return;
    };
    let Ok(config) = Config::from_toml(text) else {
        ui.colored_label(
            crate::theme::text_color(ui, BAD),
            crate::i18n::tr("The configuration is invalid; fix it in Settings."),
        );
        return;
    };

    ui.horizontal(|ui| {
        if config.dry_run {
            ui.label(
                RichText::new(crate::i18n::tr("● Monitoring only — nothing is limited"))
                    .size(18.0)
                    .color(crate::theme::text_color(ui, WARN)),
            );
            let turn_on = ui.add_enabled(
                admin,
                egui::Button::new(RichText::new(crate::i18n::tr("Turn protection on")).strong()),
            );
            if turn_on.clicked()
                && let Some(new) = crate::settings::set_dry_run(text, false)
            {
                commands.push(UiCommand::SaveConfig(new));
            }
        } else {
            ui.label(
                RichText::new(crate::i18n::tr("● Protection is on"))
                    .size(18.0)
                    .color(crate::theme::text_color(ui, GOOD)),
            );
            if ui
                .add_enabled(
                    admin,
                    egui::Button::new(crate::i18n::tr("Switch to monitoring only")),
                )
                .clicked()
                && let Some(new) = crate::settings::set_dry_run(text, true)
            {
                commands.push(UiCommand::SaveConfig(new));
            }
        }
        if !admin {
            ui.weak(crate::i18n::tr("Changing it needs administrator rights."));
            if ui
                .small_button(crate::i18n::tr("Restart as administrator"))
                .clicked()
            {
                commands.push(UiCommand::RestartElevated);
            }
        }
    });

    let current = Preset::of(&config.governor);
    ui.horizontal(|ui| {
        ui.label(crate::i18n::tr("CPU protection level:"));
        for p in Preset::ALL {
            let button = egui::Button::selectable(current == Some(p), crate::i18n::tr(p.label()));
            if ui
                .add_enabled(admin, button)
                .on_hover_text(crate::i18n::tr(p.description()))
                .clicked()
                && current != Some(p)
                && let Ok(new) = set_preset(text, p)
            {
                commands.push(UiCommand::SaveConfig(new));
            }
        }
        if current.is_none() {
            ui.weak(crate::i18n::tr("Custom (edited in Settings)"));
        }
    });
    if let Some(p) = current {
        ui.weak(crate::i18n::tr(p.description()));
    }

    if let Some(status) = &data.status {
        let mut facts = vec![crate::i18n::format(
            "{0} processes limited right now",
            &[status.managed.len().to_string()],
        )];
        if !config.limits.is_empty() {
            let processes: usize = status.app_limits.iter().map(|g| g.processes).sum();
            facts.push({
                let __wtm_i18n_0 = &(config.limits.len());
                crate::i18n::format(
                    "{0} app limits covering {1} processes",
                    &[format!("{}", __wtm_i18n_0), format!("{}", processes)],
                )
            });
        }
        if config.governor.protect_foreground
            && let Some(app) = &status.foreground
        {
            facts.push({
                crate::i18n::format(
                    "{0} is in use and never throttled",
                    std::slice::from_ref(app),
                )
            });
        }
        if config.memory_guard.enabled {
            facts.push({
                let __wtm_i18n_0 = &(config.memory_guard.high_percent);
                crate::i18n::format(
                    "memory guard above {0}% RAM",
                    &[format!("{:.0}", __wtm_i18n_0)],
                )
            });
        }
        if !config.block.is_empty() {
            facts.push({
                let __wtm_i18n_0 = &(config.block.len());
                crate::i18n::format("{0} programs blocked", &[format!("{}", __wtm_i18n_0)])
            });
        }
        ui.label(facts.join("  ·  "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_swarm_is_shown_with_ways_to_end_it() {
        use egui_kittest::kittest::Queryable;
        let mut d = crate::ui_tests::data();
        {
            let s = std::sync::Arc::make_mut(d.snapshot.as_mut().unwrap());
            // 160 probes: 100 started by alpha.exe (100), 60 whose parent exited.
            for i in 0..160 {
                let parent = if i < 100 { 100 } else { 9999 };
                let mut p = crate::ui_tests::proc(10_000 + i, parent, "powershell.exe", 0.1);
                p.create_time = i64::MAX - 1;
                s.processes.push(p);
            }
        }
        let mut h = egui_kittest::Harness::builder()
            .allow_missing_glyphs()
            .with_size([1100.0, 1200.0])
            .build_ui_state(
                |ui, (d, out): &mut (Data, Vec<Intent>)| out.extend(overview(ui, d).1),
                (d, Vec::new()),
            );
        h.run();
        assert!(
            h.query_by_label("⚠ 160 copies of powershell.exe are running")
                .is_some()
        );
        assert!(
            h.query_by_label_contains("Started by alpha.exe (100), exited parent (60)")
                .is_some()
        );
        h.get_by_label("End the 60 whose parent exited").click();
        h.run();
        h.get_by_label("End all 160").click();
        h.run();
        let counts: Vec<usize> = h
            .state()
            .1
            .iter()
            .filter_map(|i| match i {
                Intent::ConfirmMany(t, Command::Kill) => Some(t.len()),
                _ => None,
            })
            .collect();
        assert_eq!(counts, [60, 160]);

        // Below the threshold nothing is shown.
        h.state_mut().0.swarm.min_count = 500;
        h.run();
        assert!(
            h.query_by_label_contains("copies of powershell.exe")
                .is_none()
        );
    }

    #[test]
    fn colors_and_times() {
        assert_eq!(load_color(95.0), BAD);
        assert_eq!(load_color(75.0), WARN);
        assert_eq!(load_color(10.0), GOOD);
        assert_eq!(clock(0).len(), 8);
        assert_eq!(app_color(60.0, 8), BAD);
        assert_eq!(app_color(20.0, 8), WARN);
        assert_eq!(app_color(5.0, 8), Color32::GRAY);
    }

    #[test]
    fn the_verdict_names_the_resource_and_the_app_behind_it() {
        let mut d = crate::ui_tests::data();
        let set = |d: &mut Data, cpu: f32, available: u64| {
            let s = std::sync::Arc::make_mut(d.snapshot.as_mut().unwrap());
            s.system.cpu_percent = cpu;
            s.system.memory_available = available;
        };
        let gb = 1u64 << 30;
        // 16 GB in total.
        let cases = [
            (42.0, 8 * gb, GOOD, "Everything runs smoothly."),
            (
                95.0,
                8 * gb,
                BAD,
                "CPU is maxed out: alpha.exe uses 20% of it",
            ),
            (75.0, 8 * gb, WARN, "CPU is busy: alpha.exe uses 20% of it"),
            (10.0, gb, BAD, "Memory is almost full: "),
            (10.0, 3 * gb, WARN, "Memory is getting full: "),
        ];
        for (cpu, available, color, text) in cases {
            set(&mut d, cpu, available);
            let (c, t) = verdict(&d).unwrap();
            assert_eq!(c, color, "{t}");
            assert!(t.starts_with(text), "{t}");
        }
        d.snapshot = None;
        assert!(verdict(&d).is_none());
    }
}

//! The Performance pages of single devices (each disk, network adapter and
//! GPU) and the details Windows Task Manager lists under its graphs: live
//! readings as large values on one side, fixed facts on the other.

use std::collections::VecDeque;

use eframe::egui::{self, RichText, Ui};
use wtm_core::format_bytes;

use crate::backend::Data;
use crate::charts::Unit;
use crate::device_facts::{disk_key, gpu_key, net_key};

/// From this width the readings and the facts stand side by side.
const SIDE_BY_SIDE: f32 = 620.0;
/// Engine graphs shown for a GPU, as Task Manager's four.
const ENGINE_GRAPHS: usize = 4;
/// Height of a GPU engine graph.
const ENGINE_GRAPH_HEIGHT: f32 = 72.0;

type Rows = Vec<(String, String)>;

/// Thousands grouped by thin spaces: `12 345`.
pub fn count(v: f64) -> String {
    let digits = format!("{:.0}", v.max(0.0));
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{202F}');
        }
        out.push(c);
    }
    out
}

/// Up time as Task Manager writes it: days:hours:minutes:seconds.
pub fn uptime(d: std::time::Duration) -> String {
    let s = d.as_secs();
    format!(
        "{}:{:02}:{:02}:{:02}",
        s / 86_400,
        s % 86_400 / 3600,
        s % 3600 / 60,
        s % 60
    )
}

/// A link or bit rate: `866.7 Mbit/s`, `1 Gbit/s`.
pub fn bits(per_second: f64) -> String {
    let v = per_second.max(0.0);
    let (value, unit) = if v >= 1e9 {
        (v / 1e9, "Gbit/s")
    } else if v >= 1e6 {
        (v / 1e6, "Mbit/s")
    } else if v >= 1e3 {
        (v / 1e3, "kbit/s")
    } else {
        (v, "bit/s")
    };
    let text = format!("{value:.1}");
    format!("{} {unit}", text.strip_suffix(".0").unwrap_or(&text))
}

fn yes_no(v: bool) -> String {
    if v {
        crate::i18n::tr("Yes")
    } else {
        crate::i18n::tr("No")
    }
    .to_owned()
}

fn row(name: &str, value: impl Into<String>) -> (String, String) {
    (name.to_owned(), value.into())
}

/// The live readings as large values with a small caption, and the fixed
/// facts as a list beside (or below) them, with a button that copies both.
pub fn details(ui: &mut Ui, id: &str, title: &str, readings: &Rows, facts: &Rows) {
    ui.add_space(4.0);
    if ui.available_width() >= SIDE_BY_SIDE && !facts.is_empty() && !readings.is_empty() {
        ui.columns(2, |c| {
            values(&mut c[0], readings);
            fact_list(&mut c[1], id, facts);
        });
    } else {
        values(ui, readings);
        if !facts.is_empty() {
            ui.add_space(6.0);
            fact_list(ui, id, facts);
        }
    }
    ui.add_space(2.0);
    if ui
        .small_button(crate::i18n::tr("Copy details"))
        .on_hover_text(crate::i18n::tr(
            "Copy these readings and facts as text, as Task Manager's Copy does.",
        ))
        .clicked()
    {
        let mut text = vec![title.to_owned()];
        text.extend(
            readings
                .iter()
                .chain(facts)
                .map(|(name, value)| format!("{name}: {value}")),
        );
        ui.ctx().copy_text(text.join("\n"));
    }
}

/// Narrowest reading: a caption over a large value.
const TILE_WIDTH: f32 = 128.0;
const TILE_GAP: f32 = 12.0;

/// How many readings `tile` wide fit side by side in `width`.
fn tiles_per_row(width: f32, tile: f32) -> usize {
    (((width + TILE_GAP) / (tile + TILE_GAP)).floor() as usize).max(1)
}

fn values(ui: &mut Ui, readings: &Rows) {
    // Wide enough for the longest value and caption, so none is cut short.
    let width = |text: RichText| {
        egui::WidgetText::from(text)
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            )
            .size()
            .x
    };
    let widest = readings
        .iter()
        .map(|(name, value)| {
            width(RichText::new(value).size(17.0).strong()).max(width(RichText::new(name).small()))
        })
        .fold(TILE_WIDTH, f32::max)
        .min(ui.available_width().max(TILE_WIDTH));
    let tile = widest.ceil();
    let per_row = tiles_per_row(ui.available_width(), tile);
    for line in readings.chunks(per_row) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = TILE_GAP;
            for (name, value) in line {
                ui.allocate_ui_with_layout(
                    egui::vec2(tile, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(tile);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.add(egui::Label::new(RichText::new(name).small().weak()).truncate());
                        ui.add(
                            egui::Label::new(RichText::new(value).size(17.0).strong()).truncate(),
                        )
                        .on_hover_text(format!("{name}: {value}"));
                    },
                );
            }
        });
        ui.add_space(6.0);
    }
}

fn fact_list(ui: &mut Ui, id: &str, facts: &Rows) {
    egui::Grid::new(("performance-facts", id))
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            for (name, value) in facts {
                ui.weak(name);
                ui.add(egui::Label::new(value.as_str()).wrap());
                crate::theme::end_row(ui);
            }
        });
}

/// A small graph among others, such as one GPU engine's.
fn plot_at(
    ui: &mut Ui,
    view: &mut crate::charts::State,
    height: f32,
    id: &str,
    times: &VecDeque<u64>,
    series: &[(&str, &VecDeque<f32>, egui::Color32)],
    unit: Unit,
) {
    view.plot_small(ui, id, height, times, series, unit);
    ui.add_space(4.0);
}

fn device_series<'a>(data: &'a Data, key: &str, empty: &'a VecDeque<f32>) -> &'a VecDeque<f32> {
    data.history.devices.get(key).unwrap_or(empty)
}

/// What Task Manager lists under the CPU graph, and the scheduler activity
/// System Informer adds.
pub fn cpu_details(ui: &mut Ui, data: &Data, s: &wtm_core::SystemStats) {
    let f = &data.perf_facts;
    let info = f.cpu.as_ref();
    let ghz = |mhz: f64| format!("{:.2} GHz", mhz / 1000.0);
    let mut readings = vec![
        row(
            crate::i18n::tr("Utilization"),
            format!("{:.0}%", s.cpu_percent),
        ),
        row(
            crate::i18n::tr("Speed"),
            f.activity.current_mhz.map_or_else(|| "—".to_owned(), ghz),
        ),
        row(crate::i18n::tr("Processes"), count(s.process_count as f64)),
        row(crate::i18n::tr("Threads"), count(s.thread_count as f64)),
        row(crate::i18n::tr("Handles"), count(s.handle_count as f64)),
        row(
            crate::i18n::tr("Up time"),
            f.uptime.map_or_else(|| "—".to_owned(), uptime),
        ),
    ];
    let kernel: Vec<f32> = data
        .history
        .core_kernel
        .iter()
        .filter_map(|k| k.back().copied())
        .collect();
    if !kernel.is_empty() {
        let mean = kernel.iter().sum::<f32>() / kernel.len() as f32;
        readings.push(row(crate::i18n::tr("Kernel time"), format!("{mean:.1}%")));
    }
    if let Some(b) = f.breakdown {
        readings.push(row(
            crate::i18n::tr("DPC time"),
            format!("{:.1}%", b.dpc_percent),
        ));
        readings.push(row(
            crate::i18n::tr("Interrupt time"),
            format!("{:.1}%", b.interrupt_percent),
        ));
        if let Some(rate) = b.interrupts_per_sec {
            readings.push(row(crate::i18n::tr("Interrupts/s"), count(rate)));
        }
    }
    if let Some(v) = f.activity.context_switches {
        readings.push(row(crate::i18n::tr("Context switches/s"), count(v)));
    }
    if let Some(v) = f.activity.system_calls {
        readings.push(row(crate::i18n::tr("System calls/s"), count(v)));
    }
    if let Some(v) = f.activity.queue_length {
        readings.push(row(crate::i18n::tr("Processor queue"), count(v)));
    }
    let mut facts = Vec::new();
    if let Some(info) = info {
        if let Some(name) = &info.name {
            facts.push(row(crate::i18n::tr("Model"), name.clone()));
        }
        if let Some(mhz) = info.base_mhz {
            facts.push(row(crate::i18n::tr("Base speed"), ghz(f64::from(mhz))));
        }
        let l = info.layout;
        if l.sockets > 0 {
            facts.push(row(crate::i18n::tr("Sockets"), l.sockets.to_string()));
            facts.push(row(crate::i18n::tr("Cores"), l.cores.to_string()));
        }
        facts.push(row(
            crate::i18n::tr("Logical processors"),
            s.logical_cpus.to_string(),
        ));
        if let Some(v) = info.virtualization {
            facts.push(row(
                crate::i18n::tr("Virtualization"),
                if v {
                    crate::i18n::tr("Enabled")
                } else {
                    crate::i18n::tr("Disabled")
                },
            ));
        }
        for (name, size) in [
            (crate::i18n::tr("L1 cache"), l.l1),
            (crate::i18n::tr("L2 cache"), l.l2),
            (crate::i18n::tr("L3 cache"), l.l3),
        ] {
            if size > 0 {
                facts.push(row(name, format_bytes(size as f64)));
            }
        }
    }
    details(ui, "cpu", "CPU", &readings, &facts);
    core_speeds(ui, &f.core_mhz);
}

/// Each logical processor's current speed, in a compact grid: the fastest
/// are brightest. Read with the same counter as the overall speed.
fn core_speeds(ui: &mut Ui, cores: &[f64]) {
    if cores.is_empty() {
        return;
    }
    ui.add_space(6.0);
    ui.weak(crate::i18n::tr("Speed of each logical processor"));
    let max = cores.iter().copied().fold(1.0, f64::max);
    let cell = 92.0;
    let columns = tiles_per_row(ui.available_width(), cell).max(1);
    egui::Grid::new("core-speeds")
        .spacing(egui::vec2(TILE_GAP, 2.0))
        .show(ui, |ui| {
            for (i, mhz) in cores.iter().enumerate() {
                let strength = (mhz / max).clamp(0.0, 1.0) as f32;
                let color = ui
                    .visuals()
                    .weak_text_color()
                    .lerp_to_gamma(crate::theme::text_color(ui, crate::theme::CPU), strength);
                ui.allocate_ui(egui::vec2(cell, 16.0), |ui| {
                    ui.set_width(cell);
                    ui.label(
                        RichText::new(format!("{i:>2}  {:.2} GHz", mhz / 1000.0))
                            .monospace()
                            .small()
                            .color(color),
                    );
                });
                if (i + 1) % columns == 0 {
                    ui.end_row();
                }
            }
        });
}

/// The battery: its charge over time and what Windows reports now. A
/// RustWTM addition; capacity and wear are on the Devices page.
pub fn battery(ui: &mut Ui, data: &Data, view: &mut crate::charts::State) {
    let Some(b) = data.perf_facts.battery else {
        return;
    };
    let empty = VecDeque::new();
    let title = crate::i18n::tr("Battery");
    let reading = crate::performance_view::battery_reading(&b);
    crate::theme::card_with(
        ui,
        title,
        &[(&reading, ui.visuals().weak_text_color())],
        |ui| {
            view.plot(
                ui,
                "battery_charge",
                &data.history.times,
                &[(
                    crate::i18n::tr("Charge"),
                    device_series(data, crate::device_facts::BATTERY_CHARGE, &empty),
                    crate::theme::GOOD,
                )],
                Unit::Percent,
            );
            let mut readings = vec![row(
                crate::i18n::tr("Charge"),
                b.percent
                    .map_or_else(|| "—".to_owned(), |p| format!("{p}%")),
            )];
            readings.push(row(
                crate::i18n::tr("Power source"),
                match b.on_ac {
                    Some(true) => crate::i18n::tr("Plugged in"),
                    Some(false) => crate::i18n::tr("On battery"),
                    None => "—",
                },
            ));
            readings.push(row(crate::i18n::tr("Charging"), yes_no(b.charging)));
            if let Some(s) = b.seconds_left.filter(|_| b.on_ac != Some(true)) {
                readings.push(row(
                    crate::i18n::tr("Time left"),
                    format!("{}:{:02}", s / 3600, s % 3600 / 60),
                ));
            }
            readings.push(row(crate::i18n::tr("Battery saver"), yes_no(b.saver)));
            details(ui, "battery", title, &readings, &Vec::new());
            ui.weak(crate::i18n::tr(
                "Design capacity, full charge capacity and wear are on the Devices page.",
            ));
        },
    );
}

/// Processes listed under a disk or network page.
const TOP_PROCESSES: usize = 5;

/// The busiest processes now, by I/O or network traffic and both
/// directions. Windows does not tell which disk or adapter a process's I/O
/// or traffic used, so the list covers all of them and says so. The I/O
/// counters also count network and pipe I/O, hence "all devices".
pub fn top_processes(ui: &mut Ui, data: &Data, network: bool) {
    let Some(snap) = &data.snapshot else {
        return;
    };
    ui.add_space(6.0);
    ui.strong(crate::i18n::tr(if network {
        "Most network activity (all adapters)"
    } else {
        "Most I/O (all devices)"
    }));
    if network && !snap.net_traced {
        ui.weak(crate::i18n::tr(
            "Per-process network needs the RustWTM service or administrator rights",
        ));
        return;
    }
    let split = |p: &wtm_core::ProcessInfo| {
        if network {
            (p.net_rx_rate, p.net_tx_rate)
        } else {
            (p.io_read_rate, p.io_write_rate)
        }
    };
    let mut busy: Vec<&wtm_core::ProcessInfo> = snap
        .processes
        .iter()
        .filter(|p| p.pid != 0 && split(p).0 + split(p).1 > 0.0)
        .collect();
    busy.sort_by(|a, b| {
        let total = |p| {
            let (x, y) = split(p);
            x + y
        };
        total(b).total_cmp(&total(a))
    });
    if busy.is_empty() {
        ui.weak(crate::i18n::tr("Nothing measured now."));
        return;
    }
    let (first, second) = if network {
        (crate::i18n::tr("Receive"), crate::i18n::tr("Send"))
    } else {
        (crate::i18n::tr("Read"), crate::i18n::tr("Write"))
    };
    egui::Grid::new(("top-processes", network))
        .num_columns(4)
        .striped(true)
        .show(ui, |ui| {
            ui.weak(crate::i18n::tr("Name"));
            ui.weak("PID");
            ui.weak(first);
            ui.weak(second);
            ui.end_row();
            for p in busy.into_iter().take(TOP_PROCESSES) {
                let (x, y) = split(p);
                ui.label(&p.name);
                ui.label(p.pid.to_string());
                ui.label(crate::performance_view::rate_text(x));
                ui.label(crate::performance_view::rate_text(y));
                ui.end_row();
            }
        });
}

/// The engines shown in the GPU's graphs: the user's choice where that
/// engine exists, otherwise Task Manager's.
pub fn chosen_engines<'a>(engines: &'a [(String, f32)], choice: &[String]) -> Vec<&'a str> {
    let defaults = engine_graphs(engines);
    defaults
        .iter()
        .enumerate()
        .map(|(i, default)| {
            choice
                .get(i)
                .and_then(|c| engines.iter().find(|(n, _)| n == c))
                .map_or(*default, |(n, _)| n.as_str())
        })
        .collect()
}

/// Compressed memory and the modules, which Task Manager lists beside the
/// figures the memory breakdown already shows.
pub fn memory_details(ui: &mut Ui, data: &Data) {
    let mut readings = Vec::new();
    // Windows keeps compressed pages in this process's working set.
    if let Some(store) = data.snapshot.as_ref().and_then(|snap| {
        snap.processes
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case("Memory Compression"))
    }) {
        readings.push(row(
            crate::i18n::tr("Compressed"),
            format_bytes(store.working_set as f64),
        ));
    }
    let mut facts = Vec::new();
    let modules = data.perf_facts.memory.as_deref().unwrap_or_default();
    let installed: Vec<_> = modules.iter().filter(|m| m.size > 0).collect();
    if let Some(speed) = installed
        .iter()
        .filter_map(|m| m.configured_speed.or(m.speed))
        .min()
    {
        facts.push(row(crate::i18n::tr("Speed"), format!("{speed} MT/s")));
    }
    if !modules.is_empty() {
        facts.push(row(
            crate::i18n::tr("Slots used"),
            crate::i18n::format(
                "{0} of {1}",
                &[installed.len().to_string(), modules.len().to_string()],
            ),
        ));
    }
    let first = |f: fn(&wtm_sys::smbios::MemoryModule) -> Option<&'static str>| {
        installed.iter().find_map(|m| f(m))
    };
    if let Some(form) = first(wtm_sys::smbios::MemoryModule::form_factor_name) {
        facts.push(row(crate::i18n::tr("Form factor"), form));
    }
    if let Some(kind) = first(wtm_sys::smbios::MemoryModule::type_name) {
        facts.push(row(crate::i18n::tr("Memory type"), kind));
    }
    for m in &installed {
        let mut text = format_bytes(m.size as f64);
        for part in [&m.manufacturer, &m.part_number] {
            if !part.is_empty() && !part.eq_ignore_ascii_case("unknown") {
                text.push(' ');
                text.push_str(part);
            }
        }
        let slot = match (m.bank.is_empty(), m.locator.is_empty()) {
            (true, true) => crate::i18n::tr("Module").to_owned(),
            (true, false) => m.locator.clone(),
            (false, true) => m.bank.clone(),
            (false, false) => format!("{} · {}", m.bank, m.locator),
        };
        facts.push((slot, text));
    }
    if readings.is_empty() && facts.is_empty() {
        return;
    }
    details(ui, "memory", crate::i18n::tr("Memory"), &readings, &facts);
}

/// One physical disk: its active time and transfer rate, as Task Manager's
/// disk page.
pub fn disk(
    ui: &mut Ui,
    data: &Data,
    s: &wtm_core::SystemStats,
    view: &mut crate::charts::State,
    number: u32,
) {
    let Some(d) = s.disks.iter().find(|d| d.number() == Some(number)) else {
        return;
    };
    let facts_of = data.perf_facts.disk(number);
    data.disk_wanted.store(
        number.saturating_add(1),
        std::sync::atomic::Ordering::Relaxed,
    );
    data.disk_wanted_ms.store(
        wtm_core::hardware::unix_ms(),
        std::sync::atomic::Ordering::Relaxed,
    );
    let health = data.perf_facts.disk_health(number);
    let label = crate::performance_view::disk_label(d);
    let model = facts_of.map(|f| f.model.as_str()).filter(|m| !m.is_empty());
    let empty = VecDeque::new();
    let times = &data.history.times;
    let parts: Vec<(&str, egui::Color32)> = model
        .map(|m| (m, ui.visuals().weak_text_color()))
        .into_iter()
        .collect();
    crate::theme::card_with(ui, &label, &parts, |ui| {
        let active = crate::i18n::tr("Active time");
        view.plot(
            ui,
            &format!("disk_{number}_active"),
            times,
            &[(
                active,
                device_series(data, &disk_key(number, "active"), &empty),
                crate::theme::DISK_READ,
            )],
            Unit::Percent,
        );
        ui.add_space(4.0);
        ui.weak(crate::i18n::tr("Disk transfer rate"));
        view.plot(
            ui,
            &format!("disk_{number}_rate"),
            times,
            &[
                (
                    crate::i18n::tr("Read"),
                    device_series(data, &disk_key(number, "read"), &empty),
                    crate::theme::DISK_READ,
                ),
                (
                    crate::i18n::tr("Write"),
                    device_series(data, &disk_key(number, "write"), &empty),
                    crate::theme::DISK_WRITE,
                ),
            ],
            Unit::Rate,
        );
        let temperature = health.and_then(|h| {
            h.temperature
                .map(|t| t.celsius)
                .or_else(|| h.nvme.as_ref()?.temperature.map(|t| t as f32))
        });
        let mut readings = vec![
            row(
                crate::i18n::tr("Active time"),
                format!("{:.0}%", d.active_percent),
            ),
            row(
                crate::i18n::tr("Average response time"),
                format!("{:.1} ms", d.response_ms),
            ),
            row(
                crate::i18n::tr("Read speed"),
                crate::performance_view::rate_text(d.read_rate),
            ),
            row(
                crate::i18n::tr("Write speed"),
                crate::performance_view::rate_text(d.write_rate),
            ),
            row(
                crate::i18n::tr("Queue length"),
                format!("{:.2}", d.queue_length),
            ),
        ];
        if let Some(t) = temperature {
            readings.push(row(crate::i18n::tr("Temperature"), celsius(t)));
        }
        let f = &data.perf_facts;
        let mut facts = Vec::new();
        if let Some(m) = model {
            facts.push(row(crate::i18n::tr("Model"), m));
        }
        if let Some(kind) = crate::performance_view::disk_kind(facts_of) {
            facts.push(row(crate::i18n::tr("Type"), kind));
        }
        if let Some(size) = facts_of.and_then(|p| p.size) {
            facts.push(row(crate::i18n::tr("Capacity"), format_bytes(size as f64)));
        }
        let letters: Vec<String> = d.letters().map(str::to_ascii_uppercase).collect();
        let volumes: Vec<&wtm_core::Volume> = f
            .volumes
            .iter()
            .filter(|v| {
                letters
                    .iter()
                    .any(|l| v.root.trim_end_matches('\\').eq_ignore_ascii_case(l))
            })
            .collect();
        if !volumes.is_empty() {
            let formatted: u64 = volumes.iter().map(|v| v.total).sum();
            facts.push(row(
                crate::i18n::tr("Formatted"),
                format_bytes(formatted as f64),
            ));
        }
        facts.push(row(
            crate::i18n::tr("System disk"),
            yes_no(
                f.system_drive
                    .as_ref()
                    .is_some_and(|sys| letters.contains(sys)),
            ),
        ));
        facts.push(row(
            crate::i18n::tr("Page file"),
            yes_no(letters.iter().any(|l| f.page_file_drives.contains(l))),
        ));
        if let Some(t) = health.and_then(|h| h.temperature) {
            if let Some(w) = t.warning {
                facts.push(row(crate::i18n::tr("Warning temperature"), celsius(w)));
            }
            if let Some(c) = t.critical {
                facts.push(row(crate::i18n::tr("Critical temperature"), celsius(c)));
            }
        }
        if let Some(n) = health.and_then(|h| h.nvme.as_ref()) {
            facts.extend(nvme_rows(n));
        }
        for v in volumes {
            let name = v.root.trim_end_matches('\\').to_owned();
            let label = if v.label.is_empty() {
                v.file_system.clone()
            } else {
                format!("{} · {}", v.label, v.file_system)
            };
            facts.push((
                name,
                crate::i18n::format(
                    "{0}: {1} free of {2}",
                    &[
                        label,
                        format_bytes(v.free as f64),
                        format_bytes(v.total as f64),
                    ],
                ),
            ));
        }
        details(ui, &format!("disk-{number}"), &label, &readings, &facts);
    });
}

/// A temperature in whole degrees, as Task Manager writes it.
fn celsius(t: f32) -> String {
    format!("{t:.0} °C")
}

/// What an NVMe drive's health log says about its wear, as Windows'
/// Settings lists it.
fn nvme_rows(n: &crate::device_facts::NvmeHealth) -> Rows {
    // Data units are thousands of 512-byte blocks.
    let units = |u: u128| format_bytes(u.saturating_mul(512_000) as f64);
    let mut rows = vec![row(
        crate::i18n::tr("NVMe remaining endurance"),
        format!("{}%", 100u8.saturating_sub(n.used)),
    )];
    if let Some(spare) = n.spare {
        rows.push(row(
            crate::i18n::tr("NVMe available spare"),
            format!("{spare}%"),
        ));
    }
    rows.push(row(
        crate::i18n::tr("NVMe power-on hours"),
        count(n.hours as f64),
    ));
    rows.push(row(crate::i18n::tr("Data read"), units(n.read_units)));
    rows.push(row(crate::i18n::tr("Data written"), units(n.written_units)));
    rows.push(row(
        crate::i18n::tr("Unsafe shutdowns"),
        count(n.unsafe_shutdowns as f64),
    ));
    rows.push(row(
        crate::i18n::tr("Media errors"),
        count(n.media_errors as f64),
    ));
    rows
}

fn connection_type(if_type: u32) -> &'static str {
    match if_type {
        6 => crate::i18n::tr("Ethernet"),
        71 => crate::i18n::tr("Wi-Fi"),
        243 | 244 => crate::i18n::tr("Mobile broadband"),
        _ => crate::i18n::tr("Other"),
    }
}

/// One network adapter: its throughput and identity, as Task Manager's
/// Ethernet and Wi-Fi pages.
pub fn network(
    ui: &mut Ui,
    data: &Data,
    s: &wtm_core::SystemStats,
    view: &mut crate::charts::State,
    luid: u64,
) {
    let Some(a) = s.net_adapters.iter().find(|a| a.luid == luid) else {
        return;
    };
    let empty = VecDeque::new();
    let f = &data.perf_facts;
    if a.if_type == 71 {
        data.wifi_wanted_ms.store(
            wtm_core::hardware::unix_ms(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    let receive = crate::i18n::tr("Receive");
    let send = crate::i18n::tr("Send");
    crate::theme::card_with(
        ui,
        &a.name,
        &[
            (
                &format!(
                    "● {receive} {}",
                    crate::performance_view::rate_text(a.rx_rate)
                ),
                crate::theme::NET_RX,
            ),
            (
                &format!("● {send} {}", crate::performance_view::rate_text(a.tx_rate)),
                crate::theme::NET_TX,
            ),
        ],
        |ui| {
            view.plot(
                ui,
                &format!("net_{luid:016x}"),
                &data.history.times,
                &[
                    (
                        receive,
                        device_series(data, &net_key(luid, "rx"), &empty),
                        crate::theme::NET_RX,
                    ),
                    (
                        send,
                        device_series(data, &net_key(luid, "tx"), &empty),
                        crate::theme::NET_TX,
                    ),
                ],
                Unit::Rate,
            );
            let both = |rate: f64| {
                format!(
                    "{} ({})",
                    crate::performance_view::rate_text(rate),
                    bits(rate * 8.0)
                )
            };
            let mut readings = vec![
                row(crate::i18n::tr("Receive"), both(a.rx_rate)),
                row(crate::i18n::tr("Send"), both(a.tx_rate)),
            ];
            if a.link_speed > 0 {
                let used = a.rx_rate.max(a.tx_rate) * 8.0 / a.link_speed as f64 * 100.0;
                readings.push(row(
                    crate::i18n::tr("Link utilization"),
                    format!("{:.1}%", used.min(100.0)),
                ));
            }
            let mut facts = vec![
                row(crate::i18n::tr("Adapter name"), a.description.clone()),
                row(
                    crate::i18n::tr("Connection type"),
                    connection_type(a.if_type),
                ),
            ];
            if a.link_speed > 0 {
                facts.push(row(
                    crate::i18n::tr("Link speed"),
                    bits(a.link_speed as f64),
                ));
            }
            if let Some(w) = f.wifi(luid) {
                if !w.ssid.is_empty() {
                    facts.push(row(crate::i18n::tr("SSID"), w.ssid.clone()));
                }
                facts.push(row(
                    crate::i18n::tr("Signal strength"),
                    format!("{}%", w.signal),
                ));
                if let Some(standard) = w.standard {
                    facts.push(row(crate::i18n::tr("Wi-Fi standard"), standard));
                }
            }
            if let Some(addr) = f.addresses(luid) {
                if !addr.mac.is_empty() {
                    facts.push(row(crate::i18n::tr("MAC address"), addr.mac.clone()));
                }
                if !addr.dns_suffix.is_empty() {
                    facts.push(row(crate::i18n::tr("DNS suffix"), addr.dns_suffix.clone()));
                }
                let join = |list: Vec<String>| list.join("\n");
                if !addr.ipv4.is_empty() {
                    facts.push(row(
                        crate::i18n::tr("IPv4 address"),
                        join(addr.ipv4.iter().map(ToString::to_string).collect()),
                    ));
                }
                if !addr.ipv6.is_empty() {
                    facts.push(row(
                        crate::i18n::tr("IPv6 address"),
                        join(addr.ipv6.iter().map(ToString::to_string).collect()),
                    ));
                }
            }
            details(ui, &format!("net-{luid:016x}"), &a.name, &readings, &facts);
        },
    );
}

/// An engine type's name without spaces, underscores, case or a trailing
/// index: vendors write "VideoDecode", "Video Decode 1" or "Video_Decode".
fn engine_kind(name: &str) -> String {
    let joined: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    joined
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .to_owned()
}

/// The engine types a GPU's small graphs follow: Task Manager's four when
/// present (3D, Copy, video encode and decode, or a combined video codec),
/// then the busiest others.
pub fn engine_graphs(engines: &[(String, f32)]) -> Vec<&str> {
    let mut chosen: Vec<&str> = Vec::new();
    for kind in ["3d", "copy", "videoencode", "videodecode", "videocodec"] {
        if let Some((name, _)) = engines
            .iter()
            .filter(|(n, _)| !chosen.contains(&n.as_str()))
            .find(|(n, _)| engine_kind(n) == kind)
        {
            chosen.push(name);
        }
    }
    let mut rest: Vec<&(String, f32)> = engines
        .iter()
        .filter(|(n, _)| !chosen.contains(&n.as_str()))
        .collect();
    rest.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    chosen.extend(rest.into_iter().map(|(n, _)| n.as_str()));
    chosen.truncate(ENGINE_GRAPHS);
    chosen
}

/// One GPU: its load, engine graphs and memory, as Task Manager's GPU page.
pub fn gpu(
    ui: &mut Ui,
    data: &Data,
    s: &wtm_core::SystemStats,
    view: &mut crate::charts::State,
    luid: &str,
    search: &mut String,
    choice: &mut Vec<String>,
) {
    let Some(g) = s.gpus.iter().find(|g| g.luid == luid) else {
        return;
    };
    let index = crate::performance_view::gpus(s, &data.perf_facts)
        .into_iter()
        .find(|(_, x)| x.luid == luid)
        .map_or(0, |(i, _)| i);
    let facts_of = data.perf_facts.gpu(luid);
    let title = crate::i18n::format("GPU {0}", &[index.to_string()]);
    let name = facts_of.map(|f| f.name.as_str()).unwrap_or_default();
    let empty = VecDeque::new();
    let times = &data.history.times;
    let parts = [(name, ui.visuals().weak_text_color())];
    crate::theme::card_with(ui, &title, &parts[..usize::from(!name.is_empty())], |ui| {
        view.plot(
            ui,
            &format!("gpu_{luid}_load"),
            times,
            &[(
                crate::i18n::tr("Utilization"),
                device_series(data, &gpu_key(luid, "load"), &empty),
                crate::theme::GPU,
            )],
            Unit::Percent,
        );
        let engines = chosen_engines(&g.engines, choice);
        if !engines.is_empty() {
            ui.add_space(4.0);
            let columns = if ui.available_width() >= 560.0 { 2 } else { 1 };
            let mut picked: Option<(usize, String)> = None;
            for (row_index, pair) in engines.chunks(columns).enumerate() {
                ui.columns(columns, |c| {
                    for (slot, (col, engine)) in c.iter_mut().zip(pair).enumerate() {
                        let slot = row_index * columns + slot;
                        let load = g
                            .engines
                            .iter()
                            .find(|(n, _)| n == engine)
                            .map_or(0.0, |e| e.1);
                        // As in Task Manager, each graph's engine can be chosen.
                        egui::ComboBox::from_id_salt(("gpu-engine", luid, slot))
                            .selected_text(format!("{engine} · {load:.0}%"))
                            .show_ui(col, |ui| {
                                for (name, l) in &g.engines {
                                    if ui
                                        .selectable_label(
                                            name == engine,
                                            format!("{name} · {l:.0}%"),
                                        )
                                        .clicked()
                                    {
                                        picked = Some((slot, name.clone()));
                                    }
                                }
                            })
                            .response
                            .on_hover_text(crate::i18n::tr("Choose the engine this graph shows"));
                        plot_at(
                            col,
                            view,
                            ENGINE_GRAPH_HEIGHT,
                            &format!("gpu_{luid}_{engine}"),
                            times,
                            &[(
                                engine,
                                device_series(
                                    data,
                                    &gpu_key(luid, &format!("engine:{engine}")),
                                    &empty,
                                ),
                                crate::theme::GPU,
                            )],
                            Unit::Percent,
                        );
                    }
                });
            }
            if let Some((slot, name)) = picked {
                if choice.len() < engines.len() {
                    *choice = engines.iter().map(|e| (*e).to_owned()).collect();
                }
                choice[slot] = name;
            }
        }
        let dedicated_total = facts_of.map_or(0, |f| f.dedicated_total);
        let shared_total = facts_of.map_or(0, |f| f.shared_total);
        let memory = [
            (
                "dedicated",
                crate::i18n::tr("Dedicated GPU memory usage"),
                dedicated_total,
                g.dedicated_used,
            ),
            (
                "shared",
                crate::i18n::tr("Shared GPU memory usage"),
                shared_total,
                g.shared_used,
            ),
        ];
        let shown: Vec<_> = memory.iter().filter(|m| m.3.is_some()).collect();
        if !shown.is_empty() {
            ui.add_space(4.0);
            let columns = if ui.available_width() >= 560.0 {
                shown.len()
            } else {
                1
            };
            for pair in shown.chunks(columns) {
                ui.columns(columns, |c| {
                    for (col, (measure, label, total, _)) in c.iter_mut().zip(pair) {
                        col.weak(*label);
                        plot_at(
                            col,
                            view,
                            ENGINE_GRAPH_HEIGHT,
                            &format!("gpu_{luid}_{measure}"),
                            times,
                            &[(
                                label,
                                device_series(data, &gpu_key(luid, measure), &empty),
                                crate::theme::MEMORY,
                            )],
                            Unit::Bytes(*total as f64),
                        );
                    }
                });
            }
        }
        let of = |used: Option<u64>, total: u64| match (used, total) {
            (Some(u), 0) => format_bytes(u as f64),
            (Some(u), t) => format!("{} / {}", format_bytes(u as f64), format_bytes(t as f64)),
            (None, 0) => "—".to_owned(),
            (None, t) => format!("— / {}", format_bytes(t as f64)),
        };
        let mut readings = vec![
            row(crate::i18n::tr("Utilization"), format!("{:.0}%", g.percent)),
            row(
                crate::i18n::tr("Dedicated GPU memory"),
                of(g.dedicated_used, dedicated_total),
            ),
            row(
                crate::i18n::tr("Shared GPU memory"),
                of(g.shared_used, shared_total),
            ),
        ];
        if g.dedicated_used.is_some() || g.shared_used.is_some() {
            readings.push(row(
                crate::i18n::tr("GPU memory"),
                of(
                    Some(g.dedicated_used.unwrap_or(0) + g.shared_used.unwrap_or(0)),
                    dedicated_total + shared_total,
                ),
            ));
        }
        if let Some(p) = data.perf_facts.gpu_perf(luid) {
            if let Some(t) = p.temperature {
                readings.push(row(crate::i18n::tr("Temperature"), celsius(t)));
            }
            if let Some(rpm) = p.fan_rpm {
                readings.push(row(
                    crate::i18n::tr("Fan speed"),
                    format!("{} RPM", count(f64::from(rpm))),
                ));
            }
            if let Some(mhz) = p.memory_mhz {
                readings.push(row(
                    crate::i18n::tr("Memory clock"),
                    format!("{} MHz", count(f64::from(mhz))),
                ));
            }
        }
        let mut facts = Vec::new();
        if let Some(f) = facts_of {
            facts.push(row(crate::i18n::tr("Model"), f.name.clone()));
            if let Some(v) = &f.driver_version {
                facts.push(row(crate::i18n::tr("Driver version"), v.clone()));
            }
            if let Some(d) = &f.driver_date {
                facts.push(row(crate::i18n::tr("Driver date"), d.clone()));
            }
            if let Some(level) = &f.directx {
                facts.push(row(crate::i18n::tr("DirectX feature level"), level.clone()));
            }
        }
        if let Some(p) = data.perf_facts.gpu_perf(luid) {
            if let Some(t) = p.temperature_max {
                facts.push(row(crate::i18n::tr("Critical temperature"), celsius(t)));
            }
            if let Some(mhz) = p.memory_max_mhz {
                facts.push(row(
                    crate::i18n::tr("Max memory clock"),
                    format!("{} MHz", count(f64::from(mhz))),
                ));
            }
        }
        details(ui, &format!("gpu-{luid}"), &title, &readings, &facts);
        ui.add_space(6.0);
        engine_list(ui, luid, &g.engines, search);
    });
}

/// Every engine type of one GPU by load, with a search.
fn engine_list(ui: &mut Ui, luid: &str, engines: &[(String, f32)], search: &mut String) {
    let search_label = ui.label(crate::i18n::tr("Find GPU engine types"));
    ui.add(egui::TextEdit::singleline(search).desired_width(ui.available_width()))
        .labelled_by(search_label.id);
    let query = search.trim().to_lowercase();
    let mut shown: Vec<&(String, f32)> = engines
        .iter()
        .filter(|(name, _)| name.to_lowercase().contains(&query))
        .collect();
    if shown.is_empty() {
        ui.weak(crate::i18n::tr("No GPU engine types match this search."));
        return;
    }
    shown.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    crate::app::load_bars(
        ui,
        &format!("gpu_engines_{luid}"),
        shown
            .into_iter()
            .map(|(name, load)| (name.clone(), *load, format!("{load:.0}%"))),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chosen_engines_fall_back_to_task_managers_choice() {
        let e = |n: &str| (n.to_owned(), 1.0);
        let nvidia = [
            e("3D"),
            e("Copy"),
            e("VideoEncode"),
            e("VideoDecode"),
            e("Compute_0"),
        ];
        assert_eq!(
            chosen_engines(&nvidia, &["Compute_0".into()]),
            ["Compute_0", "Copy", "VideoEncode", "VideoDecode"]
        );
        // A saved engine this GPU lacks (another vendor's name) is skipped.
        let amd = [
            e("3D"),
            e("Video Decode 1"),
            e("Video Encode 0"),
            e("High Priority 3D"),
        ];
        let defaults = engine_graphs(&amd);
        assert_eq!(
            chosen_engines(&amd, &["Compute_0".into(), "Video Encode 0".into()]),
            [defaults[0], "Video Encode 0", defaults[2], defaults[3]]
        );
        assert_eq!(chosen_engines(&amd, &[]), defaults);
    }

    #[test]
    fn nvme_wear_reads_like_windows_settings() {
        let health = crate::device_facts::NvmeHealth {
            critical: 0,
            temperature: Some(46.85),
            spare: Some(100),
            used: 3,
            read_units: 2_000_000,
            written_units: 1_000,
            power_cycles: 12,
            hours: 12_345,
            unsafe_shutdowns: 7,
            media_errors: 0,
        };
        let rows = nvme_rows(&health);
        let value = |name: &str| {
            rows.iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
                .unwrap_or_else(|| panic!("{name}"))
        };
        assert_eq!(value("NVMe remaining endurance"), "97%");
        assert_eq!(value("NVMe available spare"), "100%");
        assert_eq!(value("Unsafe shutdowns"), "7");
        // 2 000 000 units of 512 000 bytes.
        assert_eq!(value("Data read"), format_bytes(1_024_000_000_000.0));
        assert!(value("NVMe power-on hours").starts_with("12"));
        let unknown_spare = crate::device_facts::NvmeHealth {
            spare: None,
            ..health
        };
        assert!(
            !nvme_rows(&unknown_spare)
                .iter()
                .any(|(n, _)| n == "NVMe available spare")
        );
    }

    #[test]
    fn formats_counts_times_and_bit_rates() {
        assert_eq!(count(0.4), "0");
        assert_eq!(count(999.0), "999");
        assert_eq!(count(12_345.0), "12\u{202F}345");
        assert_eq!(count(1_234_567.0), "1\u{202F}234\u{202F}567");
        assert_eq!(
            uptime(std::time::Duration::from_secs(2 * 86_400 + 3723)),
            "2:01:02:03"
        );
        assert_eq!(bits(1e9), "1 Gbit/s");
        assert_eq!(bits(866.7e6), "866.7 Mbit/s");
        assert_eq!(bits(512.0), "512 bit/s");
    }

    #[test]
    fn readings_wrap_by_whole_tiles() {
        assert_eq!(tiles_per_row(0.0, TILE_WIDTH), 1);
        assert_eq!(tiles_per_row(TILE_WIDTH, TILE_WIDTH), 1);
        assert_eq!(tiles_per_row(2.0 * TILE_WIDTH + TILE_GAP, TILE_WIDTH), 2);
        assert_eq!(
            tiles_per_row(2.0 * TILE_WIDTH + TILE_GAP - 1.0, TILE_WIDTH),
            1
        );
        assert_eq!(tiles_per_row(500.0, 200.0), 2);
    }

    #[test]
    fn engine_graphs_follow_task_manager_then_load() {
        let e = |n: &str, l| (n.to_owned(), l);
        let engines = [
            e("Compute_0", 50.0),
            e("VideoDecode", 1.0),
            e("3D", 10.0),
            e("Security", 0.0),
            e("Copy", 0.0),
        ];
        assert_eq!(
            engine_graphs(&engines),
            ["3D", "Copy", "VideoDecode", "Compute_0"]
        );
        // AMD names: spaces, indices and a combined video codec.
        let amd = [
            e("Compute 0", 0.0),
            e("High Priority 3D", 9.0),
            e("3D", 1.0),
            e("Video Codec 0", 2.0),
            e("Video Decode 1", 0.0),
            e("Copy", 0.0),
        ];
        assert_eq!(
            engine_graphs(&amd),
            ["3D", "Copy", "Video Decode 1", "Video Codec 0"]
        );
        assert!(engine_graphs(&[]).is_empty());
        assert_eq!(connection_type(71), "Wi-Fi");
        assert_eq!(connection_type(1), "Other");
    }
}

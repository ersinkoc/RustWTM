//! Resource navigation and presentation-only filters for Performance.
use std::collections::VecDeque;

use eframe::egui::{self, Ui};

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Page {
    Cpu,
    Memory,
    Gpu,
    Network,
    Disk,
    #[default]
    #[serde(other)]
    All,
}

impl From<crate::overview::Resource> for Page {
    fn from(value: crate::overview::Resource) -> Self {
        use crate::overview::Resource;
        match value {
            Resource::Cpu => Self::Cpu,
            Resource::Memory => Self::Memory,
            Resource::Gpu => Self::Gpu,
            Resource::Network => Self::Network,
            Resource::Disk => Self::Disk,
        }
    }
}

#[derive(Default)]
pub struct State {
    pub page: Page,
    /// The device a Disk, Network or GPU page shows ([`disk_id`],
    /// [`net_id`], [`gpu_id`]); `None` for the combined view.
    pub device: Option<String>,
    pub class: Option<u8>,
    pub engine_search: String,
}

/// Content at least this wide shows resources in a rail with sparklines
/// instead of a row of tabs.
pub const RAIL_FROM: f32 = 860.0;
const RAIL_WIDTH: f32 = 208.0;
/// Samples drawn in a rail sparkline.
const RAIL_SAMPLES: usize = 60;

type Series = Vec<(VecDeque<f32>, egui::Color32)>;

/// The newest `RAIL_SAMPLES` values of a history series; gaps read as 0.
fn tail(values: &VecDeque<f32>) -> VecDeque<f32> {
    values
        .iter()
        .skip(values.len().saturating_sub(RAIL_SAMPLES))
        .map(|v| if v.is_finite() { *v } else { 0.0 })
        .collect()
}

fn device_tail(history: &crate::backend::History, key: &str) -> VecDeque<f32> {
    history.devices.get(key).map(tail).unwrap_or_default()
}

fn rate(values: &VecDeque<f32>) -> String {
    rate_text(f64::from(values.back().copied().unwrap_or(0.0)))
}

pub fn rate_text(v: f64) -> String {
    format!("{}/s", wtm_core::format_bytes(v.max(0.0)))
}

fn percent(values: &VecDeque<f32>) -> String {
    values
        .back()
        .map_or_else(|| "—".to_owned(), |v| format!("{v:.0}%"))
}

pub fn disk_id(number: u32) -> String {
    format!("disk:{number}")
}

pub fn net_id(luid: u64) -> String {
    format!("net:{luid:016x}")
}

pub fn gpu_id(luid: &str) -> String {
    format!("gpu:{luid}")
}

/// "Disk 0 (C: D:)", as Task Manager names a disk.
pub fn disk_label(d: &wtm_core::DiskUsage) -> String {
    let number = d.number().map_or_else(|| d.name.clone(), |n| n.to_string());
    let letters: Vec<&str> = d.letters().collect();
    if letters.is_empty() {
        crate::i18n::format("Disk {0}", &[number])
    } else {
        crate::i18n::format("Disk {0} ({1})", &[number, letters.join(" ")])
    }
}

/// "SSD (NVMe)", "HDD", "Removable": the kind of disk, when known.
pub fn disk_kind(disk: Option<&wtm_sys::physical_disk::PhysicalDisk>) -> Option<String> {
    let disk = disk?;
    let kind = if disk.removable {
        crate::i18n::tr("Removable")
    } else {
        match disk.ssd {
            Some(true) => "SSD",
            Some(false) => "HDD",
            None => return disk.bus_name().map(str::to_owned),
        }
    };
    Some(match disk.bus_name() {
        Some(bus @ ("NVMe" | "USB" | "SATA" | "SAS")) => format!("{kind} ({bus})"),
        _ => kind.to_owned(),
    })
}

/// The GPUs to show with their Task Manager number (DXGI order). GPUs that
/// DXGI does not list, such as software renderers, are left out when it
/// lists any.
pub fn gpus<'a>(
    s: &'a wtm_core::SystemStats,
    facts: &crate::device_facts::Facts,
) -> Vec<(usize, &'a wtm_sys::gpu::GpuAdapterUsage)> {
    if facts.gpus.is_empty() {
        return s.gpus.iter().enumerate().collect();
    }
    let mut shown: Vec<_> = s
        .gpus
        .iter()
        .filter_map(|g| Some((facts.gpus.iter().position(|f| f.luid == g.luid)?, g)))
        .collect();
    shown.sort_by_key(|(i, _)| *i);
    shown
}

/// One entry of the resource rail or chip row.
pub struct Entry {
    pub page: Page,
    pub device: Option<String>,
    /// "CPU", "Disk 0 (C:)", "Wi-Fi", "GPU 0".
    pub label: String,
    /// The latest reading, under the label.
    pub reading: String,
    series: Series,
    percent_scale: bool,
}

impl Entry {
    fn base(page: Page, label: &str, reading: String, series: Series, percent_scale: bool) -> Self {
        Self {
            page,
            device: None,
            label: crate::i18n::tr(label).to_owned(),
            reading,
            series,
            percent_scale,
        }
    }
}

/// Every resource and device, in Task Manager's order: CPU, memory, each
/// disk, each network adapter, each GPU. Combined network and GPU entries
/// stand in only when no single device is reported.
pub fn entries(data: &crate::backend::Data) -> Vec<Entry> {
    use crate::theme::{CPU, DISK_READ, DISK_WRITE, GPU, MEMORY, NET_RX, NET_TX};
    let history = &data.history;
    let mut out = vec![
        Entry::base(
            Page::All,
            "All resources",
            format!(
                "CPU {} · {} {}",
                percent(&history.cpu),
                crate::i18n::tr("Memory"),
                percent(&history.memory)
            ),
            Vec::new(),
            true,
        ),
        Entry::base(
            Page::Cpu,
            "CPU",
            percent(&history.cpu),
            vec![(tail(&history.cpu), CPU)],
            true,
        ),
        Entry::base(
            Page::Memory,
            "Memory",
            percent(&history.memory),
            vec![(tail(&history.memory), MEMORY)],
            true,
        ),
    ];
    let stats = data.snapshot.as_ref().map(|s| &s.system);
    let facts = &data.perf_facts;
    let mut disks: Vec<&wtm_core::DiskUsage> = stats
        .map(|s| s.disks.iter().filter(|d| d.number().is_some()).collect())
        .unwrap_or_default();
    disks.sort_by_key(|d| d.number());
    for d in disks {
        let Some(n) = d.number() else { continue };
        let busy = format!("{:.0}%", d.active_percent);
        out.push(Entry {
            page: Page::Disk,
            device: Some(disk_id(n)),
            label: disk_label(d),
            reading: match disk_kind(facts.disk(n)) {
                Some(kind) => format!("{kind} · {busy}"),
                None => busy,
            },
            series: vec![(
                device_tail(history, &crate::device_facts::disk_key(n, "active")),
                DISK_READ,
            )],
            percent_scale: true,
        });
    }
    out.push(Entry::base(
        Page::Disk,
        "Disk & I/O",
        format!(
            "R {}  W {}",
            rate(&history.io_read),
            rate(&history.io_write)
        ),
        vec![
            (tail(&history.io_read), DISK_READ),
            (tail(&history.io_write), DISK_WRITE),
        ],
        false,
    ));
    let adapters = stats.map(|s| &s.net_adapters[..]).unwrap_or_default();
    for a in adapters {
        let key = |m| crate::device_facts::net_key(a.luid, m);
        out.push(Entry {
            page: Page::Network,
            device: Some(net_id(a.luid)),
            label: a.name.clone(),
            reading: format!("↓ {}  ↑ {}", rate_text(a.rx_rate), rate_text(a.tx_rate)),
            series: vec![
                (device_tail(history, &key("rx")), NET_RX),
                (device_tail(history, &key("tx")), NET_TX),
            ],
            percent_scale: false,
        });
    }
    if adapters.is_empty() {
        out.push(Entry::base(
            Page::Network,
            "Network",
            format!("↓ {}  ↑ {}", rate(&history.net_rx), rate(&history.net_tx)),
            vec![
                (tail(&history.net_rx), NET_RX),
                (tail(&history.net_tx), NET_TX),
            ],
            false,
        ));
    }
    let shown = stats.map(|s| gpus(s, facts)).unwrap_or_default();
    for (i, g) in &shown {
        let name = facts.gpu(&g.luid).map(|f| f.name.as_str());
        // "38% (46 °C)", as Task Manager writes a GPU's load and temperature.
        let load = match facts.gpu_perf(&g.luid).and_then(|p| p.temperature) {
            Some(t) => format!("{:.0}% ({t:.0} °C)", g.percent),
            None => format!("{:.0}%", g.percent),
        };
        out.push(Entry {
            page: Page::Gpu,
            device: Some(gpu_id(&g.luid)),
            label: crate::i18n::format("GPU {0}", &[i.to_string()]),
            reading: match name {
                Some(name) => format!("{load} · {name}"),
                None => load,
            },
            series: vec![(
                device_tail(history, &crate::device_facts::gpu_key(&g.luid, "load")),
                GPU,
            )],
            percent_scale: true,
        });
    }
    if shown.is_empty() {
        out.push(Entry::base(
            Page::Gpu,
            "GPU",
            percent(&history.gpu),
            vec![(tail(&history.gpu), GPU)],
            true,
        ));
    }
    out
}

impl State {
    /// Keeps the selection on something listed: a device that went away
    /// falls back to its combined view, or to the first device of its kind
    /// when that view is not listed.
    pub fn settle(&mut self, entries: &[Entry]) {
        let listed = |page: Page, device: &Option<String>| {
            entries
                .iter()
                .any(|e| e.page == page && &e.device == device)
        };
        if matches!(self.page, Page::All | Page::Cpu | Page::Memory) {
            self.device = None;
            return;
        }
        if self.device.is_some() && !listed(self.page, &self.device) {
            self.device = None;
        }
        if self.device.is_none() && !listed(self.page, &None) {
            self.device = entries
                .iter()
                .find(|e| e.page == self.page)
                .and_then(|e| e.device.clone());
        }
    }

    fn select(&mut self, entry: &Entry) {
        self.page = entry.page;
        self.device = entry.device.clone();
    }

    fn selected(&self, entry: &Entry) -> bool {
        self.page == entry.page && self.device == entry.device
    }

    /// The resources as a vertical rail: each with its latest reading and a
    /// sparkline of the last minute. Returns whether the page changed.
    pub fn rail(&mut self, ui: &mut Ui, entries: &[Entry]) -> bool {
        let before = (self.page, self.device.clone());
        ui.allocate_ui_with_layout(
            egui::vec2(RAIL_WIDTH, ui.available_height()),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(RAIL_WIDTH);
                egui::ScrollArea::vertical()
                    .id_salt("performance-rail")
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        for entry in entries {
                            if rail_entry(ui, self.selected(entry), entry).clicked() {
                                self.select(entry);
                            }
                        }
                    });
            },
        );
        (self.page, self.device.clone()) != before
    }

    /// Keep navigation outside the content scroll area, including on compact windows.
    pub fn navigation(&mut self, ui: &mut Ui, entries: &[Entry]) -> bool {
        let before = (self.page, self.device.clone());
        ui.horizontal_wrapped(|ui| {
            for entry in entries {
                let mut current = self.selected(entry);
                let response = crate::theme::choice(ui, &mut current, true, entry.label.as_str());
                if entry.page != Page::All {
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::accesskit::Role::Button,
                            ui.is_enabled(),
                            self.selected(entry),
                            crate::i18n::format(
                                "{0} performance view",
                                std::slice::from_ref(&entry.label),
                            ),
                        )
                    });
                }
                if response.clicked() {
                    self.select(entry);
                }
            }
        });
        ui.add_space(0.0);
        (self.page, self.device.clone()) != before
    }
}

/// One resource in the rail: a sparkline, its name and latest reading. The
/// selected one is raised and marked with the accent, like a chosen chip.
fn rail_entry(ui: &mut Ui, selected: bool, entry: &Entry) -> egui::Response {
    let dark = ui.visuals().dark_mode;
    let height = 48.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    let name = entry.label.as_str();
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::accesskit::Role::Button,
            ui.is_enabled(),
            selected,
            if entry.page == Page::All {
                name.to_owned()
            } else {
                crate::i18n::format("{0} performance view", &[name.into()])
            },
        )
    });
    let painter = ui.painter_at(rect);
    let radius = egui::CornerRadius::same(6);
    if selected {
        painter.rect(
            rect,
            radius,
            crate::theme::chip_fill(dark),
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
            egui::StrokeKind::Inside,
        );
    } else if response.hovered() {
        painter.rect_filled(rect, radius, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    if selected || response.has_focus() {
        let mark = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 2.5, rect.center().y),
            egui::vec2(3.0, 18.0),
        );
        painter.rect_filled(mark, egui::CornerRadius::same(2), crate::theme::ACCENT);
    }
    let series = &entry.series;
    let text_left = if let Some((_, color)) = series.first() {
        let chart = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 10.0, rect.top() + 7.0),
            egui::vec2(60.0, height - 14.0),
        );
        painter.rect_stroke(
            chart,
            egui::CornerRadius::same(3),
            egui::Stroke::new(1.0, color.gamma_multiply(0.55)),
            egui::StrokeKind::Inside,
        );
        let max = if entry.percent_scale {
            100.0
        } else {
            let peak = series
                .iter()
                .flat_map(|(values, _)| values.iter().copied())
                .fold(0.0f32, f32::max);
            crate::theme::rate_ceiling(f64::from(peak)) as f32
        };
        for (values, color) in series {
            crate::theme::sparkline(
                &painter,
                chart.shrink(1.0),
                values,
                max,
                RAIL_SAMPLES,
                *color,
            );
        }
        chart.right() + 10.0
    } else {
        rect.left() + 14.0
    };
    // Long device names end at the entry's edge instead of spilling over.
    let text = painter.with_clip_rect(egui::Rect::from_min_max(
        egui::pos2(text_left, rect.top()),
        egui::pos2(rect.right() - 4.0, rect.bottom()),
    ));
    text.text(
        egui::pos2(text_left, rect.center().y - 1.0),
        egui::Align2::LEFT_BOTTOM,
        name,
        egui::TextStyle::Body.resolve(ui.style()),
        if selected {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().text_color()
        },
    );
    text.text(
        egui::pos2(text_left, rect.center().y + 2.0),
        egui::Align2::LEFT_TOP,
        &entry.reading,
        egui::TextStyle::Small.resolve(ui.style()),
        ui.visuals().weak_text_color(),
    );
    response.on_hover_text(format!("{name}\n{}", entry.reading))
}

/// Flat usage arrays have no processor-group identity. Only use topology when
/// each array index maps uniquely to a CPU in one group.
pub fn matched_topology(
    topology: Option<&wtm_core::cpu::Topology>,
    count: usize,
) -> Option<&wtm_core::cpu::Topology> {
    topology.filter(|t| {
        t.cpus.len() == count
            && t.groups().len() == 1
            && (0..count).all(|i| t.cpus.iter().filter(|c| usize::from(c.index) == i).count() == 1)
    })
}

pub fn cpu_filter(
    ui: &mut Ui,
    class: &mut Option<u8>,
    topology: Option<&wtm_core::cpu::Topology>,
    count: usize,
) {
    let Some(t) = matched_topology(topology, count).filter(|t| t.hybrid()) else {
        *class = None;
        ui.weak(crate::i18n::tr(
            "CPU tier filtering requires matching single-group hybrid topology.",
        ));
        return;
    };
    let classes = t.classes();
    if class.is_some_and(|c| !classes.contains(&c)) {
        *class = None;
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .selectable_label(class.is_none(), crate::i18n::tr("All CPU tiers"))
            .clicked()
        {
            *class = None;
        }
        for c in classes.into_iter().rev() {
            let count = t
                .cpus
                .iter()
                .filter(|cpu| cpu.efficiency_class == c)
                .count();
            let label = crate::i18n::format("{0} tier ({1})", &[t.tier(c), count.to_string()]);
            if ui.selectable_label(*class == Some(c), label).clicked() {
                *class = Some(c);
            }
        }
    });
    ui.weak(crate::i18n::tr(
        "The tier filter affects logical-processor tiles; the main chart remains total CPU usage.",
    ));
}

//! Device workspaces: physical identity, sources and honest gaps in recent history.
use eframe::egui::{self, RichText, Ui};
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};
use wtm_core::devices::{Device, Report};
type Samples = VecDeque<(u64, Option<f64>)>;
fn device_color(kind: &str) -> egui::Color32 {
    match kind {
        "GPU" => crate::theme::GPU,
        "Network" => crate::theme::NET_RX,
        "Memory" => crate::theme::MEMORY,
        "Disk" => crate::theme::DISK_READ,
        _ => crate::theme::CPU,
    }
}
fn reading(m: &wtm_core::devices::Metric) -> String {
    match m.value {
        Some(v) if m.unit == "bytes" => wtm_core::format_bytes(v),
        Some(v) if m.unit == "B/s" => format!("{}/s", wtm_core::format_bytes(v)),
        Some(v) => format!("{v:.0} {}", m.unit),
        None => crate::i18n::tr("Unavailable").into(),
    }
}

fn metric_card(
    ui: &mut Ui,
    name: &str,
    value: &str,
    selected: bool,
    color: egui::Color32,
) -> egui::Response {
    let frame = egui::Frame::new()
        .fill(crate::theme::tile_fill(ui))
        .stroke(egui::Stroke::new(
            1.0,
            if selected {
                color
            } else {
                ui.visuals().widgets.noninteractive.bg_stroke.color
            },
        ))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(58.0);
            ui.label(egui::RichText::new(name).size(12.0).weak());
            ui.label(
                egui::RichText::new(value)
                    .size(23.0)
                    .color(crate::theme::text_color(ui, color)),
            );
        });
    let response = ui.interact(
        frame.response.rect,
        frame.response.id.with(name),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::Button,
            ui.is_enabled(),
            selected,
            format!("{name}: {value}"),
        )
    });
    if response.hovered() || response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            0.0,
            egui::Stroke::new(1.0, color),
            egui::StrokeKind::Inside,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
#[derive(Default)]
pub struct View {
    task: crate::background::Task<Report>,
    report: Option<Result<Report, String>>,
    selected: Option<String>,
    metric: Option<String>,
    disk_details: bool,
    auto: bool,
    last: Option<Instant>,
    previous: BTreeMap<(String, String), (u64, f64)>,
    series: BTreeMap<(String, String), Samples>,
}
impl View {
    fn ingest(&mut self, mut report: Report) {
        self.previous
            .retain(|(d, _), _| report.devices.iter().any(|v| &v.id == d));
        self.series
            .retain(|(d, _), _| report.devices.iter().any(|v| &v.id == d));
        for d in &mut report.devices {
            for m in &mut d.metrics {
                let key = (d.id.clone(), m.id.clone());
                if m.counter {
                    let total = m.value;
                    m.value =
                        wtm_core::devices::rate(self.previous.get(&key).copied(), d.at, total);
                    if let Some(total) = total {
                        self.previous.insert(key.clone(), (d.at, total));
                    } else {
                        self.previous.remove(&key);
                    }
                }
                let values = self.series.entry(key).or_default();
                if values.back().is_none_or(|(at, _)| *at < d.at) {
                    values.push_back((d.at, m.value));
                    if values.len() > 180 {
                        values.pop_front();
                    }
                }
            }
        }
        if !report
            .devices
            .iter()
            .any(|d| Some(&d.id) == self.selected.as_ref())
        {
            self.selected = report.devices.first().map(|d| d.id.clone());
            self.metric = None;
        }
        self.report = Some(Ok(report));
        self.last = Some(Instant::now());
    }
    pub fn show(&mut self, ui: &mut Ui) {
        if let Some(result) = self.task.poll() {
            match result {
                Ok(r) => self.ingest(r),
                Err(e) => {
                    self.report = Some(Err(e));
                    self.last = Some(Instant::now());
                }
            }
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !self.task.busy(),
                    egui::Button::new(crate::i18n::tr("Refresh devices")),
                )
                .clicked()
            {
                self.task.start(ui.ctx(), wtm_core::devices::collect);
            }
            ui.checkbox(
                &mut self.auto,
                crate::i18n::tr("Refresh while this page is open"),
            );
            if self.task.busy() {
                ui.spinner();
            }
            if let Some(at) = self.last {
                ui.weak(crate::i18n::format(
                    "Updated {0}s ago",
                    &[at.elapsed().as_secs().to_string()],
                ));
            }
        });
        ui.weak(crate::i18n::tr("Read-only Windows storage, network and battery telemetry. Missing readings stay unavailable."));
        if (self.report.is_none()
            || self.auto
                && self
                    .last
                    .is_some_and(|t| t.elapsed() >= Duration::from_secs(5)))
            && !self.task.busy()
        {
            self.task.start(ui.ctx(), wtm_core::devices::collect);
        }
        let Some(report) = &self.report else {
            crate::theme::loading(ui, crate::i18n::tr("Loading devices…"));
            return;
        };
        let report = match report {
            Ok(r) => r,
            Err(e) => {
                crate::theme::colored(ui, crate::theme::WARN, e);
                return;
            }
        };
        let devices = report.devices.clone();
        let warnings = report.warnings.clone();
        if devices.is_empty() {
            ui.weak(crate::i18n::tr("No supported devices were reported."));
        }
        if ui.available_width() >= 940.0 {
            let height = (ui.ctx().content_rect().height() - 200.0).max(160.0);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(230.0, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("device-rail")
                            .max_height(height)
                            .show(ui, |ui| self.navigation(ui, &devices));
                    },
                );
                ui.separator();
                ui.vertical(|ui| self.details(ui, &devices));
            });
        } else {
            let label = ui.weak(crate::i18n::tr("Devices and health"));
            let current = devices
                .iter()
                .find(|d| Some(&d.id) == self.selected.as_ref())
                .map(|d| format!("{} · {}", crate::i18n::tr(&d.kind), d.name))
                .unwrap_or_default();
            egui::ComboBox::from_id_salt("selected-hardware-device")
                .width(ui.available_width().min(600.0))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for d in &devices {
                        if ui
                            .selectable_label(
                                Some(&d.id) == self.selected.as_ref(),
                                format!("{} · {}", crate::i18n::tr(&d.kind), d.name),
                            )
                            .clicked()
                        {
                            self.selected = Some(d.id.clone());
                            self.metric = None;
                        }
                    }
                })
                .response
                .labelled_by(label.id);
            self.details(ui, &devices);
        }
        for warning in warnings {
            ui.weak(warning);
        }
    }
    fn navigation(&mut self, ui: &mut Ui, devices: &[Device]) {
        ui.spacing_mut().item_spacing.y = 4.0;
        for d in devices {
            let selected = Some(&d.id) == self.selected.as_ref();
            let color = device_color(&d.kind);
            let label = format!("{} · {}", crate::i18n::tr(&d.kind), d.name);
            let frame = egui::Frame::new()
                .fill(if selected {
                    color.gamma_multiply(0.10)
                } else {
                    crate::theme::card_fill(ui)
                })
                .stroke(egui::Stroke::new(
                    1.0,
                    if selected {
                        color
                    } else {
                        ui.visuals().widgets.noninteractive.bg_stroke.color
                    },
                ))
                .inner_margin(egui::Margin::symmetric(8, 8))
                .show(ui, |ui| {
                    ui.set_width(214.0);
                    ui.horizontal(|ui| {
                        let width = (ui.available_width() - 66.0).max(60.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, 40.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(width);
                                ui.add(egui::Label::new(&label).truncate());
                                if let Some(m) = d.metrics.first() {
                                    ui.add(
                                        egui::Label::new(RichText::new(reading(m)).weak())
                                            .truncate(),
                                    );
                                }
                            },
                        );
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(56.0, 32.0), egui::Sense::hover());
                        if let Some(m) = d.metrics.first()
                            && let Some(samples) = self.series.get(&(d.id.clone(), m.id.clone()))
                        {
                            let samples: Vec<_> = samples.iter().rev().take(30).rev().collect();
                            let ceiling = m
                                .ceiling
                                .unwrap_or_else(|| {
                                    samples.iter().filter_map(|(_, v)| *v).fold(1.0, f64::max)
                                })
                                .max(1.0);
                            let mut points = Vec::new();
                            let mut previous = None;
                            let start = samples.first().map_or(0, |s| s.0);
                            let span = samples
                                .last()
                                .map_or(1, |s| s.0.saturating_sub(start).max(1));
                            for (at, value) in samples {
                                if previous
                                    .is_some_and(|last: u64| at.saturating_sub(last) > 30_000)
                                    || value.is_none()
                                {
                                    if points.len() > 1 {
                                        ui.painter().add(egui::Shape::line(
                                            std::mem::take(&mut points),
                                            egui::Stroke::new(1.3, color),
                                        ));
                                    } else {
                                        points.clear();
                                    }
                                }
                                if let Some(value) = value {
                                    points.push(egui::pos2(
                                        rect.left()
                                            + rect.width()
                                                * (at.saturating_sub(start) as f32 / span as f32),
                                        rect.bottom()
                                            - rect.height()
                                                * (*value / ceiling).clamp(0.0, 1.0) as f32,
                                    ));
                                }
                                previous = Some(*at);
                            }
                            if points.len() > 1 {
                                ui.painter()
                                    .add(egui::Shape::line(points, egui::Stroke::new(1.3, color)));
                            }
                        }
                    });
                });
            let response = ui.interact(
                frame.response.rect,
                frame.response.id.with(&d.id),
                egui::Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, &label)
            });
            if response.hovered() || response.has_focus() {
                ui.painter().rect_stroke(
                    response.rect,
                    0.0,
                    egui::Stroke::new(1.0, color),
                    egui::StrokeKind::Inside,
                );
            }
            let response = response
                .on_hover_text(&d.name)
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() {
                self.selected = Some(d.id.clone());
                self.metric = None;
            }
        }
    }
    fn chart(&self, ui: &mut Ui, d: &Device, m: &wtm_core::devices::Metric, height: f32) {
        let bits = d.kind == "Network" && crate::experience::network_bits(ui.ctx());
        let color = device_color(&d.kind);
        let factor = if bits && m.unit == "B/s" {
            8.0 / 1_000_000.0
        } else {
            1.0
        };
        ui.label(format!(
            "{} · {}",
            crate::i18n::tr(&m.name),
            if bits && m.unit == "B/s" {
                "Mbps"
            } else {
                &m.unit
            }
        ));
        if let Some(series) = self.series.get(&(d.id.clone(), m.id.clone())) {
            let start = series.front().map_or(0, |(t, _)| *t);
            let mut segments = Vec::new();
            let mut points = Vec::new();
            let mut previous = None;
            for (at, value) in series {
                if previous.is_some_and(|before| at.saturating_sub(before) > 30_000)
                    && !points.is_empty()
                {
                    segments.push(std::mem::take(&mut points));
                }
                if let Some(value) = value {
                    points.push([at.saturating_sub(start) as f64 / 1000.0, *value * factor]);
                } else if !points.is_empty() {
                    segments.push(std::mem::take(&mut points));
                }
                previous = Some(*at);
            }
            if !points.is_empty() {
                segments.push(points);
            }
            egui_plot::Plot::new(("device-chart", &d.id, &m.id))
                .height(height)
                .show_background(false)
                .grid_color(ui.visuals().weak_text_color().gamma_multiply(0.20))
                .allow_drag(false)
                .allow_scroll(false)
                .allow_zoom(false)
                .allow_boxed_zoom(false)
                .y_axis_min_width(64.0)
                .y_axis_formatter(|mark, _| match m.unit.as_str() {
                    "bytes" => wtm_core::format_bytes(mark.value.max(0.0)),
                    "B/s" if bits => format!("{:.1} Mbps", mark.value),
                    "B/s" => format!("{}/s", wtm_core::format_bytes(mark.value.max(0.0))),
                    unit => format!("{:.0} {unit}", mark.value),
                })
                .include_y(0.0)
                .include_y(m.ceiling.unwrap_or(0.0) * factor)
                .show(ui, |plot| {
                    for (index, points) in segments.into_iter().enumerate() {
                        plot.line(
                            egui_plot::Line::new(format!("{} {index}", m.name), points)
                                .color(color)
                                .width(1.6)
                                .fill(0.0),
                        );
                    }
                });
        }
    }
    fn details(&mut self, ui: &mut Ui, devices: &[Device]) {
        let Some(d) = devices
            .iter()
            .find(|d| Some(&d.id) == self.selected.as_ref())
        else {
            return;
        };
        let bits = d.kind == "Network" && crate::experience::network_bits(ui.ctx());
        let color = device_color(&d.kind);
        if d.kind == "GPU" {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .selectable_label(self.metric.is_none(), crate::i18n::tr("Overview"))
                    .clicked()
                {
                    self.metric = None;
                }
                if ui
                    .selectable_label(self.metric.is_some(), crate::i18n::tr("Details"))
                    .clicked()
                {
                    self.metric = d.metrics.first().map(|m| m.id.clone());
                }
            });
        }
        if d.kind == "Disk" {
            ui.horizontal_wrapped(|ui| {
                crate::theme::choice(
                    ui,
                    &mut self.disk_details,
                    false,
                    crate::i18n::tr("Overview"),
                );
                crate::theme::choice(ui, &mut self.disk_details, true, crate::i18n::tr("Details"));
            });
            if self.disk_details {
                ui.heading(&d.name);
                if !d.warning.is_empty() {
                    crate::theme::colored(ui, crate::theme::WARN, &d.warning);
                }
                crate::disk_details::show(ui, d);
                return;
            }
        }
        crate::theme::card(ui, &d.name, |ui| {
            ui.weak(crate::i18n::tr("Windows device telemetry"));
            if !d.warning.is_empty() {
                crate::theme::colored(ui, crate::theme::WARN, &d.warning);
            }
            let summary: Vec<_> = d
                .metrics
                .iter()
                .filter(|m| !m.id.starts_with("engine:"))
                .collect();
            let columns = if ui.available_width() >= 620.0 {
                if summary.len() == 4 { 4 } else { 3 }
            } else {
                2
            };
            for chunk in summary.chunks(columns) {
                ui.columns(columns, |cols| {
                    for (col, m) in cols.iter_mut().zip(chunk) {
                        let label = match m.value {
                            Some(v) if m.unit == "B/s" && bits => {
                                format!("{:.2} Mbps", v * 8.0 / 1_000_000.0)
                            }
                            Some(v) if m.unit == "bytes" => wtm_core::format_bytes(v),
                            Some(v) if m.unit == "B/s" => {
                                format!("{}/s", wtm_core::format_bytes(v))
                            }
                            Some(v) => format!("{v:.1} {}", m.unit),
                            None => crate::i18n::tr("Unavailable").into(),
                        };
                        let selected = self.metric.as_ref() == Some(&m.id)
                            || self.metric.is_none()
                                && d.metrics.first().is_some_and(|first| first.id == m.id);
                        if metric_card(col, crate::i18n::tr(&m.name), &label, selected, color)
                            .clicked()
                        {
                            self.metric = Some(m.id.clone());
                        }
                    }
                });
            }
            let engines: Vec<_> = d
                .metrics
                .iter()
                .filter(|m| m.id.starts_with("engine:"))
                .collect();
            if !engines.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.weak(crate::i18n::tr("GPU engines"));
                    for engine in engines {
                        if ui
                            .selectable_label(
                                self.metric.as_ref() == Some(&engine.id),
                                &engine.name,
                            )
                            .clicked()
                        {
                            self.metric = Some(engine.id.clone());
                        }
                    }
                });
            }
            let metric = d
                .metrics
                .iter()
                .find(|m| Some(&m.id) == self.metric.as_ref())
                .or_else(|| d.metrics.first());
            if d.kind == "GPU" && self.metric.is_none() {
                let mut engines: Vec<_> = d
                    .metrics
                    .iter()
                    .filter(|m| m.id.starts_with("engine:"))
                    .collect();
                // Keep engine positions stable as utilization changes.
                engines.sort_by(|a, b| a.id.cmp(&b.id));
                if engines.is_empty() {
                    engines.extend(d.metrics.iter().filter(|m| m.id == "load"));
                }
                engines.truncate(2);
                let columns = if ui.available_width() >= 600.0 { 2 } else { 1 };
                for row in engines.chunks(columns) {
                    ui.columns(columns, |cols| {
                        for (col, m) in cols.iter_mut().zip(row) {
                            self.chart(col, d, m, 180.0);
                        }
                    });
                }
                let memory: Vec<_> = d
                    .metrics
                    .iter()
                    .filter(|m| m.id == "dedicated" || m.id == "shared")
                    .collect();
                for row in memory.chunks(columns) {
                    ui.columns(columns, |cols| {
                        for (col, m) in cols.iter_mut().zip(row) {
                            self.chart(col, d, m, 160.0);
                        }
                    });
                }
            } else if let Some(m) = metric {
                self.chart(ui, d, m, 240.0);
            }
            ui.weak(crate::i18n::tr(
                "Recent samples while this window is open; gaps are not interpolated.",
            ));
            if d.kind == "Memory" {
                let used = d
                    .metrics
                    .iter()
                    .find(|m| m.id == "used")
                    .and_then(|m| m.value);
                let available = d
                    .metrics
                    .iter()
                    .find(|m| m.id == "available")
                    .and_then(|m| m.value);
                if let (Some(used), Some(available)) = (used, available)
                    && used + available > 0.0
                {
                    let ratio = (used / (used + available)).clamp(0.0, 1.0);
                    ui.add(
                        egui::ProgressBar::new(ratio as f32)
                            .corner_radius(0.0)
                            .desired_height(12.0)
                            .fill(crate::theme::text_color(ui, crate::theme::MEMORY))
                            .desired_width(ui.available_width())
                            .text(format!(
                                "{} / {} ({:.0}%)",
                                wtm_core::format_bytes(used),
                                wtm_core::format_bytes(used + available),
                                ratio * 100.0
                            )),
                    );
                }
            }
            crate::theme::divider(ui);
            for f in &d.fields {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(crate::i18n::tr(&f.name)).weak());
                    ui.label(&f.value);
                });
            }
        });
    }
    #[cfg(test)]
    pub fn fixture() -> Self {
        Self::fixture_for("Disk")
    }
    #[cfg(test)]
    pub fn fixture_for(kind: &str) -> Self {
        let metric =
            |id: &str, name: &str, unit: &str, value: f64, ceiling| wtm_core::devices::Metric {
                id: id.into(),
                name: name.into(),
                unit: unit.into(),
                value: Some(value),
                ceiling,
                counter: false,
            };
        let mut report = Report {
            devices: vec![
                Device {
                    id: "disk:fixture".into(),
                    kind: "Disk".into(),
                    name: "Example NVMe 2 TB".into(),
                    at: 0,
                    warning: String::new(),
                    fields: vec![
                        wtm_core::devices::Field {
                            name: "Model".into(),
                            value: "Example SSD controller".into(),
                        },
                        wtm_core::devices::Field {
                            name: "Firmware".into(),
                            value: "EX0201".into(),
                        },
                        wtm_core::devices::Field {
                            name: "Bus".into(),
                            value: "NVMe".into(),
                        },
                        wtm_core::devices::Field {
                            name: "Capacity (bytes)".into(),
                            value: "2000000000000".into(),
                        },
                        wtm_core::devices::Field {
                            name: "Storage health".into(),
                            value: "Healthy".into(),
                        },
                        wtm_core::devices::Field {
                            name: "NVMe power-on hours".into(),
                            value: "1842".into(),
                        },
                        wtm_core::devices::Field {
                            name: "Data read units (512000 bytes)".into(),
                            value: "9400000".into(),
                        },
                        wtm_core::devices::Field {
                            name: "Data written units (512000 bytes)".into(),
                            value: "26000000".into(),
                        },
                        wtm_core::devices::Field {
                            name: "NVMe power cycles".into(),
                            value: "501".into(),
                        },
                        wtm_core::devices::Field {
                            name: "Unsafe shutdowns".into(),
                            value: "7".into(),
                        },
                        wtm_core::devices::Field {
                            name: "Media errors".into(),
                            value: "0".into(),
                        },
                        wtm_core::devices::Field {
                            name: "NVMe critical warning bits".into(),
                            value: "0x00".into(),
                        },
                    ],
                    metrics: vec![
                        metric("active", "Active time", "%", 42.0, Some(100.0)),
                        metric("read", "Read", "B/s", 124_000_000.0, None),
                        metric("temperature", "Temperature", "C", 42.0, None),
                        metric(
                            "nvme-spare",
                            "NVMe available spare",
                            "%",
                            100.0,
                            Some(100.0),
                        ),
                        metric(
                            "nvme-remaining",
                            "NVMe remaining endurance",
                            "%",
                            97.0,
                            Some(100.0),
                        ),
                    ],
                },
                Device {
                    id: "gpu:fixture".into(),
                    kind: "GPU".into(),
                    name: "Example discrete GPU".into(),
                    at: 0,
                    warning: String::new(),
                    fields: vec![wtm_core::devices::Field {
                        name: "Dedicated memory capacity".into(),
                        value: "16 GiB".into(),
                    }],
                    metrics: vec![
                        metric("load", "Busiest engine", "%", 36.0, Some(100.0)),
                        metric(
                            "dedicated",
                            "Dedicated memory",
                            "bytes",
                            6.4 * 1073741824.0,
                            Some(16.0 * 1073741824.0),
                        ),
                        metric("shared", "Shared memory", "bytes", 0.7 * 1073741824.0, None),
                        metric("engine:0:3D", "3D", "%", 36.0, Some(100.0)),
                        metric("engine:1:VideoDecode", "VideoDecode", "%", 2.0, Some(100.0)),
                    ],
                },
                Device {
                    id: "memory:fixture".into(),
                    kind: "Memory".into(),
                    name: "System memory".into(),
                    at: 0,
                    warning: String::new(),
                    fields: vec![wtm_core::devices::Field {
                        name: "Capacity (bytes)".into(),
                        value: "137438953472".into(),
                    }],
                    metrics: vec![
                        metric(
                            "used",
                            "In use",
                            "bytes",
                            80.0 * 1073741824.0,
                            Some(128.0 * 1073741824.0),
                        ),
                        metric(
                            "available",
                            "Available",
                            "bytes",
                            48.0 * 1073741824.0,
                            Some(128.0 * 1073741824.0),
                        ),
                        metric(
                            "commit",
                            "Committed",
                            "bytes",
                            110.0 * 1073741824.0,
                            Some(192.0 * 1073741824.0),
                        ),
                        metric("cache", "Cached", "bytes", 45.0 * 1073741824.0, None),
                    ],
                },
                Device {
                    id: "network:fixture".into(),
                    kind: "Network".into(),
                    name: "Ethernet".into(),
                    at: 0,
                    warning: String::new(),
                    fields: vec![wtm_core::devices::Field {
                        name: "Link speed".into(),
                        value: "2.5 Gbps".into(),
                    }],
                    metrics: vec![
                        metric("receive", "Receive", "B/s", 72_000_000.0, None),
                        metric("send", "Send", "B/s", 4_500_000.0, None),
                    ],
                },
                Device {
                    id: "battery:fixture".into(),
                    kind: "Battery".into(),
                    name: "Example notebook battery".into(),
                    at: 0,
                    warning: String::new(),
                    fields: vec![
                        wtm_core::devices::Field {
                            name: "Design capacity (mWh)".into(),
                            value: "80000".into(),
                        },
                        wtm_core::devices::Field {
                            name: "Cycles".into(),
                            value: "218".into(),
                        },
                    ],
                    metrics: vec![
                        metric("charge", "Charge", "%", 76.0, Some(100.0)),
                        metric("health", "Capacity health", "%", 91.0, Some(100.0)),
                        metric("power", "Charge / discharge", "W", -18.5, None),
                    ],
                },
            ],
            warnings: vec![],
        };
        let mut view = Self::default();
        for tick in 0..60 {
            for d in &mut report.devices {
                d.at = 1_700_000_000_000 + tick * 1000;
                for m in &mut d.metrics {
                    if m.id == "active" || m.id == "load" {
                        m.value = Some(35.0 + 15.0 * (tick as f64 / 8.0).sin());
                    }
                }
            }
            view.ingest(report.clone());
        }
        view.disk_details = kind == "DiskDetails";
        let kind = if view.disk_details { "Disk" } else { kind };
        view.selected = view
            .report
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .and_then(|r| r.devices.iter().find(|d| d.kind == kind))
            .map(|d| d.id.clone());
        view
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    #[test]
    fn memory_composition_requires_both_reported_values() {
        let mut h = egui_kittest::Harness::builder()
            .with_size([1000.0, 1000.0])
            .build_ui_state(|ui, v: &mut View| v.show(ui), View::fixture_for("Memory"));
        h.run();
        let text = "80.0 GiB / 128.0 GiB (62%)";
        assert!(h.query_by_label(text).is_some());
        let report = h.state_mut().report.as_mut().unwrap().as_mut().unwrap();
        let memory = report
            .devices
            .iter_mut()
            .find(|d| d.kind == "Memory")
            .unwrap();
        memory
            .metrics
            .iter_mut()
            .find(|m| m.id == "available")
            .unwrap()
            .value = None;
        h.run();
        assert!(h.query_by_label(text).is_none());
    }
    #[test]
    fn disk_details_and_gpu_overview_switch_in_both_layouts() {
        for width in [780.0, 1200.0] {
            let mut h = egui_kittest::Harness::builder()
                .with_size([width, 1200.0])
                .build_ui_state(|ui, v: &mut View| v.show(ui), View::fixture());
            h.run();
            h.get_by_label("Details").click();
            h.run();
            assert!(h.query_by_label("Health").is_some());
            assert!(h.query_by_label("97%").is_some());
            assert!(h.query_by_label("SMART / NVMe").is_some());
            assert!(h.query_by_label("EX0201").is_some());
            h.get_by_label("Overview").click();
            h.run();
            assert!(h.query_by_label("SMART / NVMe").is_none());
            *h.state_mut() = View::fixture_for("GPU");
            h.run();
            assert!(h.query_by_label("3D · %").is_some());
            assert!(h.query_by_label("Dedicated memory · bytes").is_some());
            assert!(h.query_by_label("Shared memory · bytes").is_some());
            h.get_by_label("Details").click();
            h.run();
            assert!(h.query_by_label("Dedicated memory · bytes").is_none());
            h.get_by_label("Overview").click();
            h.run();
            assert!(h.query_by_label("Dedicated memory · bytes").is_some());
        }
    }
    #[test]
    fn device_selection_and_missing_samples_render_in_both_layouts() {
        for width in [780.0, 1200.0] {
            let mut h = egui_kittest::Harness::builder()
                .with_size([width, 800.0])
                .build_ui_state(|ui, v: &mut View| v.show(ui), View::fixture());
            h.run();
            assert!(h.query_by_label("Example NVMe 2 TB").is_some());
            assert!(h.query_by_label("42.0 C").is_some());
            if width < 940.0 {
                h.get_by_label("Devices and health").click();
                h.run();
                h.get_by_label("GPU · Example discrete GPU").click();
                h.run();
                assert!(h.query_by_label("Example discrete GPU").is_some());
                assert!(h.query_by_label("42.0 C").is_none());
            }
        }
    }
}

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
/// How disks are ordered among themselves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DiskOrder {
    /// By Windows disk number.
    Number,
    /// As arranged with Move up / Move down.
    #[default]
    #[serde(other)]
    Custom,
}

/// Which devices the rail shows and in which order. Remembered by ID.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Layout {
    pub hidden: Vec<String>,
    /// Devices moved by hand, in their chosen order; others follow as reported.
    pub order: Vec<String>,
    pub disks: DiskOrder,
}

fn disk_number(d: &Device) -> Option<u32> {
    d.fields
        .iter()
        .find(|f| f.name == "Disk number")
        .and_then(|f| f.value.parse().ok())
}

impl Layout {
    /// The shown devices in display order.
    pub fn arrange(&self, devices: &[Device]) -> Vec<Device> {
        let rank = |d: &Device| {
            self.order
                .iter()
                .position(|id| id == &d.id)
                .unwrap_or(usize::MAX)
        };
        let mut shown: Vec<(usize, &Device)> = devices
            .iter()
            .enumerate()
            .filter(|(_, d)| !self.hidden.contains(&d.id))
            .collect();
        // Stable: unmoved devices keep the reported order.
        shown.sort_by_key(|(i, d)| (rank(d), *i));
        if self.disks == DiskOrder::Number {
            // Disks keep their slots but are refilled in disk-number order.
            let mut disks: Vec<&Device> = shown
                .iter()
                .map(|(_, d)| *d)
                .filter(|d| d.kind == "Disk")
                .collect();
            disks.sort_by_key(|d| disk_number(d).unwrap_or(u32::MAX));
            let mut disks = disks.into_iter();
            for slot in shown.iter_mut().filter(|(_, d)| d.kind == "Disk") {
                slot.1 = disks.next().expect("same number of disks");
            }
        }
        shown.into_iter().map(|(_, d)| d.clone()).collect()
    }

    /// Moves `id` one place up (`-1`) or down (`1`) among `shown`.
    pub fn shift(&mut self, shown: &[Device], id: &str, step: isize) {
        let mut ids: Vec<String> = shown.iter().map(|d| d.id.clone()).collect();
        let Some(at) = ids.iter().position(|d| d == id) else {
            return;
        };
        let to = at
            .saturating_add_signed(step)
            .min(ids.len().saturating_sub(1));
        ids.swap(at, to);
        // Hidden devices keep their remembered place behind the shown ones.
        let rest: Vec<String> = self
            .order
            .iter()
            .filter(|o| !ids.contains(o))
            .cloned()
            .collect();
        ids.extend(rest);
        self.order = ids;
        if shown.get(at).is_some_and(|d| d.kind == "Disk") {
            self.disks = DiskOrder::Custom;
        }
    }

    pub fn set_hidden(&mut self, id: &str, hidden: bool) {
        self.hidden.retain(|h| h != id);
        if hidden {
            self.hidden.push(id.to_owned());
        }
    }
}

#[derive(Default)]
pub struct View {
    /// Shown devices and their order; saved with the window preferences.
    pub layout: Layout,
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
        let all = report.devices.clone();
        let devices = self.layout.arrange(&all);
        let warnings = report.warnings.clone();
        if !devices
            .iter()
            .any(|d| Some(&d.id) == self.selected.as_ref())
        {
            self.selected = devices.first().map(|d| d.id.clone());
            self.metric = None;
        }
        self.shown_devices_menu(ui, &all);
        if all.is_empty() {
            ui.weak(crate::i18n::tr("No supported devices were reported."));
        } else if devices.is_empty() {
            ui.weak(crate::i18n::tr(
                "Every device is hidden. Choose the ones to show under Shown devices.",
            ));
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
    /// Choose the devices the rail lists, and how disks are ordered.
    fn shown_devices_menu(&mut self, ui: &mut Ui, all: &[Device]) {
        ui.horizontal_wrapped(|ui| {
            let hidden = all
                .iter()
                .filter(|d| self.layout.hidden.contains(&d.id))
                .count();
            let title = if hidden == 0 {
                crate::i18n::tr("Shown devices").to_owned()
            } else {
                crate::i18n::format("Shown devices ({0} hidden)", &[hidden.to_string()])
            };
            ui.menu_button(title, |ui| {
                for d in all {
                    let mut shown = !self.layout.hidden.contains(&d.id);
                    let label = format!("{} · {}", crate::i18n::tr(&d.kind), d.name);
                    if ui.checkbox(&mut shown, label).changed() {
                        self.layout.set_hidden(&d.id, !shown);
                    }
                }
                crate::theme::divider(ui);
                if ui.button(crate::i18n::tr("Show all devices")).clicked() {
                    self.layout.hidden.clear();
                    ui.close();
                }
                if ui
                    .button(crate::i18n::tr("Restore the reported order"))
                    .clicked()
                {
                    self.layout.order.clear();
                    ui.close();
                }
            });
            ui.label(crate::i18n::tr("Disk order"));
            for (value, label) in [
                (DiskOrder::Number, "By disk number"),
                (DiskOrder::Custom, "Custom order"),
            ] {
                crate::theme::choice(ui, &mut self.layout.disks, value, crate::i18n::tr(label));
            }
        });
    }

    fn navigation(&mut self, ui: &mut Ui, devices: &[Device]) {
        ui.spacing_mut().item_spacing.y = 4.0;
        let mut moved: Option<(String, isize)> = None;
        let mut hidden: Option<String> = None;
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
            let mut open = response.clicked();
            crate::context_actions::menu(&response, ("hardware-device", &d.id), |ui| {
                open |= crate::context_actions::action(ui, crate::i18n::tr("Details"));
                crate::theme::divider(ui);
                if crate::context_actions::action(ui, crate::i18n::tr("Move up")) {
                    moved = Some((d.id.clone(), -1));
                }
                if crate::context_actions::action(ui, crate::i18n::tr("Move down")) {
                    moved = Some((d.id.clone(), 1));
                }
                if crate::context_actions::action(ui, crate::i18n::tr("Hide this device")) {
                    hidden = Some(d.id.clone());
                }
                crate::theme::divider(ui);
                crate::context_actions::copy(ui, crate::i18n::tr("Specifications"), || {
                    std::iter::once(d.name.clone())
                        .chain(d.fields.iter().map(|field| {
                            format!("{}: {}", crate::i18n::tr(&field.name), field.value)
                        }))
                        .collect::<Vec<_>>()
                        .join("\n")
                });
            });
            if open {
                self.selected = Some(d.id.clone());
                self.metric = None;
            }
        }
        if let Some((id, step)) = moved {
            self.layout.shift(devices, &id, step);
        }
        if let Some(id) = hidden {
            self.layout.set_hidden(&id, true);
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
        let caption = ui.label(format!(
            "{} · {}",
            crate::i18n::tr(&m.name),
            if bits && m.unit == "B/s" {
                "Mbps"
            } else {
                &m.unit
            }
        ));
        if let Some(value) = m.value {
            let display = if bits && m.unit == "B/s" {
                format!("{:.1} Mbps", value * factor)
            } else {
                reading(m)
            };
            crate::context_actions::copy_text(
                &caption,
                ("device-chart", &d.id, &m.id),
                crate::i18n::tr("Current"),
                &display,
            );
        }
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

    fn device(id: &str, kind: &str, number: Option<u32>) -> Device {
        Device {
            id: id.into(),
            kind: kind.into(),
            name: id.into(),
            at: 0,
            warning: String::new(),
            fields: number
                .map(|n| wtm_core::devices::Field {
                    name: "Disk number".into(),
                    value: n.to_string(),
                })
                .into_iter()
                .collect(),
            metrics: Vec::new(),
        }
    }

    fn ids(devices: &[Device]) -> Vec<&str> {
        devices.iter().map(|d| d.id.as_str()).collect()
    }

    #[test]
    fn device_layout_hides_moves_and_orders_disks_by_number() {
        let all = [
            device("cpu", "CPU", None),
            device("d2", "Disk", Some(2)),
            device("net", "Network", None),
            device("d0", "Disk", Some(0)),
        ];
        let mut layout = Layout::default();
        assert_eq!(ids(&layout.arrange(&all)), ["cpu", "d2", "net", "d0"]);
        layout.disks = DiskOrder::Number;
        // Disks keep their places in the list, refilled by disk number.
        assert_eq!(ids(&layout.arrange(&all)), ["cpu", "d0", "net", "d2"]);
        layout.set_hidden("net", true);
        let shown = layout.arrange(&all);
        assert_eq!(ids(&shown), ["cpu", "d0", "d2"]);
        // Moving a disk by hand switches disks to the custom order.
        layout.shift(&shown, "d2", -1);
        assert_eq!(layout.disks, DiskOrder::Custom);
        assert_eq!(ids(&layout.arrange(&all)), ["cpu", "d2", "d0"]);
        layout.shift(&layout.arrange(&all), "cpu", -1);
        assert_eq!(
            ids(&layout.arrange(&all)),
            ["cpu", "d2", "d0"],
            "already first"
        );
        layout.set_hidden("net", false);
        // A shown-again device follows the moved ones.
        assert_eq!(ids(&layout.arrange(&all)), ["cpu", "d2", "d0", "net"]);
    }

    #[test]
    fn a_hidden_device_can_be_shown_again_from_the_menu() {
        let mut view = View::fixture_for("Disk");
        let id = view.report.as_ref().unwrap().as_ref().unwrap().devices[0]
            .id
            .clone();
        view.layout.set_hidden(&id, true);
        let mut h = egui_kittest::Harness::builder()
            .with_size([1200.0, 900.0])
            .build_ui_state(|ui, v: &mut View| v.show(ui), view);
        h.run();
        h.get_by_label_contains("Shown devices").click();
        h.run();
        h.get_by_label("Show all devices").click();
        h.run();
        assert!(h.state().layout.hidden.is_empty());
    }

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

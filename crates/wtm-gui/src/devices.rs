//! Device workspaces: physical identity, sources and honest gaps in recent history.
use eframe::egui::{self, RichText, Ui};
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};
use wtm_core::devices::{Device, Report};
type Samples = VecDeque<(u64, Option<f64>)>;

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
            let height = ui.available_height().max(120.0);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(230.0, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.navigation(ui, &devices),
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
            let response = ui.add_sized(
                [ui.available_width(), 44.0],
                egui::Button::new(format!("{}  ·  {}", crate::i18n::tr(&d.kind), d.name))
                    .wrap()
                    .selected(selected),
            );
            if response.clicked() {
                self.selected = Some(d.id.clone());
                self.metric = None;
            }
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
        let color = match d.kind.as_str() {
            "GPU" => crate::theme::GPU,
            "Network" => crate::theme::NET_RX,
            "Memory" => crate::theme::MEMORY,
            "Disk" => crate::theme::DISK_READ,
            _ => crate::theme::CPU,
        };
        crate::theme::card(ui, &d.name, |ui| {
            ui.weak(crate::i18n::tr("Windows device telemetry"));
            if !d.warning.is_empty() {
                crate::theme::colored(ui, crate::theme::WARN, &d.warning);
            }
            let columns = if ui.available_width() >= 620.0 { 3 } else { 2 };
            let summary: Vec<_> = d
                .metrics
                .iter()
                .filter(|m| !m.id.starts_with("engine:"))
                .collect();
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
            if let Some(m) = metric {
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
                    for (at, value) in series {
                        if let Some(value) = value {
                            points
                                .push([at.saturating_sub(start) as f64 / 1000.0, *value * factor]);
                        } else if !points.is_empty() {
                            segments.push(std::mem::take(&mut points));
                        }
                    }
                    if !points.is_empty() {
                        segments.push(points);
                    }
                    egui_plot::Plot::new(("device-chart", &d.id, &m.id))
                        .height(240.0)
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
                    ui.weak(crate::i18n::tr(
                        "Recent samples while this window is open; gaps are not interpolated.",
                    ));
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
                            name: "Storage health".into(),
                            value: "Healthy".into(),
                        },
                        wtm_core::devices::Field {
                            name: "Power-on time".into(),
                            value: "1842 h".into(),
                        },
                    ],
                    metrics: vec![
                        metric("active", "Active time", "%", 42.0, Some(100.0)),
                        metric("read", "Read", "B/s", 124_000_000.0, None),
                        metric("temperature", "Temperature", "C", 42.0, None),
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

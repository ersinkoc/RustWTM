//! Sensors and capability-based cooling. Hardware writes are explicit service
//! requests; editing a curve, changing tabs or restarting never applies one.
use crate::backend::{Data, Mode, Panel, UiCommand};
use eframe::egui::{self, RichText, Ui};
use std::collections::{BTreeMap, HashMap, VecDeque};
use wtm_core::hardware::{HardwareReport, SensorKind, unix_ms};
use wtm_policy::{CoolingMode, CoolingProfile, CurvePoint};

#[derive(Default)]
pub struct SensorsView {
    power: crate::power_controls::PowerView,
    pub profiles: HashMap<String, CoolingProfile>,
    search: String,
    filter: Option<SensorKind>,
    observed_at: u64,
    history: HashMap<String, VecDeque<f32>>,
    extremes: HashMap<String, (f32, f32)>,
    confirm: Option<(String, CoolingProfile)>,
}

impl SensorsView {
    #[cfg(feature = "visual-tests")]
    #[cfg(test)]
    pub(crate) fn review_fixture(report: &HardwareReport) -> Self {
        let mut view = Self::default();
        for i in 0..120 {
            let mut sample = report.clone();
            sample.sampled_at_ms = report.sampled_at_ms.saturating_sub((120 - i) * 2000);
            for sensor in &mut sample.sensors {
                sensor.value = sensor
                    .value
                    .map(|v| v * (1.0 + (i as f32 * 0.12).sin() * 0.08));
            }
            view.observe(&sample);
        }
        view.profiles.insert(
            "fan-control".into(),
            CoolingProfile {
                mode: CoolingMode::Curve(CoolingProfile::preset("Balanced")),
                sensor_id: "temp".into(),
                ..Default::default()
            },
        );
        view
    }

    fn observe(&mut self, report: &HardwareReport) {
        if self.observed_at == report.sampled_at_ms {
            return;
        }
        self.observed_at = report.sampled_at_ms;
        for s in &report.sensors {
            if let Some(value) = s.value.filter(|v| v.is_finite()) {
                let values = self.history.entry(s.id.clone()).or_default();
                values.push_back(value);
                if values.len() > 120 {
                    values.pop_front();
                }
                let (min, max) = self.extremes.entry(s.id.clone()).or_insert((value, value));
                *min = min.min(value);
                *max = max.max(value);
            }
        }
        self.history
            .retain(|id, _| report.sensors.iter().any(|s| s.id == *id));
        self.extremes
            .retain(|id, _| report.sensors.iter().any(|s| s.id == *id));
    }
}

fn colour(kind: SensorKind) -> egui::Color32 {
    match kind {
        SensorKind::Temperature => crate::theme::ACCENT,
        SensorKind::Fan | SensorKind::Control => crate::theme::CPU,
        SensorKind::Power | SensorKind::Voltage => crate::theme::GOOD,
        SensorKind::Clock => crate::theme::MEMORY,
        SensorKind::Load => crate::theme::NET_RX,
    }
}

pub fn sensors(ui: &mut Ui, data: &Data, view: &mut SensorsView) -> Vec<UiCommand> {
    let report = &data.hardware;
    view.observe(report);
    let mut commands = Vec::new();
    let fresh = report.fresh(unix_ms());
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(if report.provider.is_empty() {
                "Discovering hardware…"
            } else {
                &report.provider
            })
            .strong(),
        );
        if report.sampled_at_ms != 0 {
            let age = unix_ms().saturating_sub(report.sampled_at_ms) / 1000;
            ui.colored_label(
                crate::theme::text_color(
                    ui,
                    if fresh {
                        crate::theme::GOOD
                    } else {
                        crate::theme::BAD
                    },
                ),
                format!("{} · {age}s ago", if fresh { "Live" } else { "Stale" }),
            );
        }
        if ui.button("Refresh sensors").clicked() {
            commands.push(UiCommand::Load(Panel::Sensors));
        }
        if ui.button("Reset min/max").clicked() {
            view.extremes.clear();
            view.observed_at = 0;
            view.observe(report);
        }
    });
    if !report.notice.is_empty() {
        ui.label(RichText::new(&report.notice).weak());
    }
    if !fresh && !report.sensors.is_empty() {
        ui.colored_label(
            crate::theme::text_color(ui, crate::theme::WARN),
            "Readings are stale. Software fan control cannot be applied.",
        );
    }
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut view.search)
                .hint_text("Find a device or sensor")
                .desired_width(220.0),
        );
        ui.selectable_value(&mut view.filter, None, "All sensors");
        for (kind, label) in [
            (SensorKind::Temperature, "Temperatures"),
            (SensorKind::Fan, "Fan RPM"),
            (SensorKind::Power, "Power"),
            (SensorKind::Clock, "Clocks"),
        ] {
            ui.selectable_value(&mut view.filter, Some(kind), label);
        }
    });
    ui.add_space(8.0);
    let query = view.search.trim().to_lowercase();
    let mut devices: BTreeMap<&str, Vec<_>> = BTreeMap::new();
    for sensor in &report.sensors {
        if view.filter.is_none_or(|kind| kind == sensor.kind)
            && (query.is_empty()
                || sensor.name.to_lowercase().contains(&query)
                || sensor.device.to_lowercase().contains(&query))
        {
            devices.entry(&sensor.device).or_default().push(sensor);
        }
    }
    if report.sensors.is_empty() {
        crate::theme::card(ui, "Hardware discovery", |ui| {
            ui.label("No sensor readings yet");
            ui.weak("The optional hardware provider supplies CPU, GPU, motherboard and storage sensors. NVIDIA driver telemetry is used when it is available.");
            if !data.elevated && ui.button("Restart as administrator").clicked() {
                commands.push(UiCommand::RestartElevated);
            }
        });
    } else if devices.is_empty() {
        ui.label("No matching sensors");
    }
    for (device, mut readings) in devices {
        readings.sort_by(|a, b| {
            format!("{:?}", a.kind)
                .cmp(&format!("{:?}", b.kind))
                .then(a.name.cmp(&b.name))
        });
        crate::theme::card(ui, RichText::new(device).size(17.0).strong(), |ui| {
            let name_width = (ui.available_width() - 440.0).clamp(170.0, 380.0);
            egui::Grid::new(("sensors", device))
                .num_columns(4)
                .spacing([16.0, 8.0])
                .striped(true)
                .show(ui, |ui| {
                    for title in ["Sensor", "Current", "Min / max", "Recent samples"] {
                        ui.strong(title);
                    }
                    ui.end_row();
                    for sensor in readings {
                        ui.allocate_ui_with_layout(
                            egui::vec2(name_width, 22.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.add(egui::Label::new(&sensor.name).truncate())
                                    .on_hover_text(format!("{}\n{}", sensor.name, sensor.id));
                            },
                        );
                        let color = crate::theme::text_color(ui, colour(sensor.kind));
                        ui.label(
                            RichText::new(sensor.value.map_or_else(
                                || "Unavailable".into(),
                                |v| format!("{v:.1} {}", sensor.kind.unit()),
                            ))
                            .color(color),
                        );
                        ui.weak(view.extremes.get(&sensor.id).map_or_else(
                            || "—".into(),
                            |(min, max)| format!("{min:.1} / {max:.1}"),
                        ));
                        let width = if ui.available_width() > 100.0 {
                            120.0
                        } else {
                            60.0
                        };
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(width, 22.0), egui::Sense::hover());
                        if let Some(values) = view.history.get(&sensor.id) {
                            let max = values.iter().copied().fold(1.0, f32::max) * 1.1;
                            crate::theme::sparkline(
                                ui.painter(),
                                rect,
                                values,
                                max,
                                120,
                                colour(sensor.kind),
                            );
                        }
                        ui.end_row();
                    }
                });
        });
        ui.add_space(8.0);
    }
    ui.add_space(8.0);
    ui.heading("Cooling");
    ui.weak("Profiles run in the service. Applying one needs a current temperature, an elevated window and enforcing mode. Startup keeps hardware automatic control.");
    if report.channels.is_empty() {
        ui.label("No controllable fan channels were reported.");
    }
    let admin = matches!(data.mode, Mode::Service { admin: true, .. });
    let enforcing = data.status.as_ref().is_some_and(|s| !s.dry_run);
    for channel in &report.channels {
        crate::theme::card(
            ui,
            RichText::new(format!("{} · {}", channel.device, channel.name)).strong(),
            |ui| {
                if let Some((_, state)) = report.cooling.iter().find(|(id, _)| id == &channel.id) {
                    ui.label(state);
                } else {
                    ui.weak("Hardware automatic / no RustWTM profile applied");
                }
                if !channel.writable {
                    ui.label(&channel.reason);
                }
                let profile = view.profiles.entry(channel.id.clone()).or_default();
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .selectable_label(
                            profile.mode == CoolingMode::Automatic,
                            "Hardware automatic",
                        )
                        .clicked()
                    {
                        profile.mode = CoolingMode::Automatic;
                    }
                    if ui
                        .selectable_label(
                            matches!(profile.mode, CoolingMode::Manual(_)),
                            "Manual duty",
                        )
                        .clicked()
                    {
                        profile.mode = CoolingMode::Manual(50.0);
                    }
                    if ui
                        .selectable_label(
                            matches!(profile.mode, CoolingMode::Curve(_)),
                            "Temperature curve",
                        )
                        .clicked()
                    {
                        profile.mode = CoolingMode::Curve(CoolingProfile::preset("Balanced"));
                    }
                });
                let temps: Vec<_> = report
                    .sensors
                    .iter()
                    .filter(|s| s.kind == SensorKind::Temperature)
                    .collect();
                if profile.mode != CoolingMode::Automatic {
                    if profile.sensor_id.is_empty()
                        && let Some(s) = temps.iter().find(|s| s.device == channel.device)
                    {
                        profile.sensor_id = s.id.clone();
                    }
                    egui::ComboBox::new(("cooling_sensor", &channel.id), "Temperature source")
                        .selected_text(
                            temps
                                .iter()
                                .find(|s| s.id == profile.sensor_id)
                                .map_or("Select temperature", |s| s.name.as_str()),
                        )
                        .show_ui(ui, |ui| {
                            for s in &temps {
                                ui.selectable_value(
                                    &mut profile.sensor_id,
                                    s.id.clone(),
                                    format!("{} · {}", s.device, s.name),
                                );
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.label("Full speed at");
                        ui.add(
                            egui::DragValue::new(&mut profile.critical_celsius)
                                .range(60.0..=95.0)
                                .suffix(" °C"),
                        );
                    });
                }
                match &mut profile.mode {
                    CoolingMode::Manual(value) => {
                        ui.add(
                            egui::Slider::new(value, channel.min_percent..=channel.max_percent)
                                .suffix("%")
                                .text("Requested duty"),
                        );
                    }
                    CoolingMode::Curve(points) => {
                        ui.horizontal_wrapped(|ui| {
                            for name in ["Quiet", "Balanced", "Performance"] {
                                if ui.small_button(name).clicked() {
                                    *points = CoolingProfile::preset(name);
                                }
                            }
                        });
                        ui.weak("Temperature (°C) → Fan duty (%)");
                        let plot = egui_plot::Plot::new(("fan_curve", &channel.id))
                            .height(120.0)
                            .include_y(0.0)
                            .include_y(100.0)
                            .allow_drag(false)
                            .allow_zoom(false)
                            .allow_scroll(false);
                        plot.show(ui, |plot| {
                            plot.line(
                                egui_plot::Line::new(
                                    "Duty % / °C",
                                    egui_plot::PlotPoints::from_iter(
                                        points.iter().map(|p| {
                                            [f64::from(p.temperature), f64::from(p.percent)]
                                        }),
                                    ),
                                )
                                .color(crate::theme::CPU),
                            );
                        });
                        ui.horizontal_wrapped(|ui| {
                            for p in points.iter_mut() {
                                ui.add(
                                    egui::DragValue::new(&mut p.temperature)
                                        .range(0.0..=100.0)
                                        .suffix(" °C"),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut p.percent)
                                        .range(channel.min_percent..=channel.max_percent)
                                        .suffix("%"),
                                );
                            }
                        });
                        ui.horizontal(|ui| {
                            if points.len() < 12 && ui.small_button("Add curve point").clicked() {
                                let t = points.last().map_or(80.0, |p| p.temperature + 1.0);
                                points.push(CurvePoint {
                                    temperature: t.min(100.0),
                                    percent: 100.0,
                                });
                            }
                            if points.len() > 2 && ui.small_button("Remove last point").clicked() {
                                points.pop();
                            }
                        });
                    }
                    CoolingMode::Automatic => {}
                }
                let valid = profile.validate(channel);
                if let Err(e) = &valid {
                    ui.colored_label(crate::theme::text_color(ui, crate::theme::WARN), e);
                }
                let automatic = profile.mode == CoolingMode::Automatic;
                let temperature_ready = profile.duty(report, unix_ms()).is_some();
                let enabled = admin
                    && (automatic
                        || (channel.writable && enforcing && temperature_ready && valid.is_ok()));
                if ui
                    .add_enabled(enabled, egui::Button::new("Apply cooling profile"))
                    .clicked()
                {
                    if automatic {
                        commands.push(UiCommand::SetCooling {
                            channel: channel.id.clone(),
                            profile: profile.clone(),
                        });
                    } else {
                        view.confirm = Some((channel.id.clone(), profile.clone()));
                    }
                }
                if !admin {
                    ui.weak("Connect to the matching service from an elevated RustWTM window to apply controls.");
                } else if !enforcing {
                    ui.weak("The service is monitoring only. Fan writes are disabled.");
                } else if !automatic && fresh && !temperature_ready {
                    ui.weak("The selected temperature is unavailable. Choose a sensor with a current reading.");
                }
            },
        );
        ui.add_space(8.0);
    }
    commands.extend(crate::power_controls::power_controls(
        ui,
        data,
        &mut view.power,
    ));
    if let Some((channel, profile)) = view.confirm.clone() {
        let modal=egui::Modal::new(egui::Id::unique("confirm_cooling")).show(ui.ctx(),|ui| {
            ui.heading("Apply software fan control?");
            let name=report.channels.iter().find(|c|c.id==channel).map_or(channel.as_str(),|c|c.name.as_str());
            ui.label(format!("Channel: {name}. This changes a physical fan's requested speed. Fan duty and measured RPM are different values."));
            ui.label("The selected temperature drives thermal protection. Missing/stale temperature requests hardware automatic mode. The provider attempts defaults when its control lease expires.");
            if ui.button("Apply fan control").clicked() {commands.push(UiCommand::SetCooling{channel:channel.clone(),profile:profile.clone()});view.confirm=None;}
            if ui.button("Cancel").clicked() {view.confirm=None;}
        });
        if modal.should_close() {
            view.confirm = None;
        }
    }
    commands
}

#[cfg(test)]
pub(crate) fn fixture() -> HardwareReport {
    use wtm_core::hardware::{FanChannel, Sensor};
    HardwareReport {
        provider: "Test hardware provider".into(),
        sampled_at_ms: unix_ms(),
        sensors: vec![
            Sensor {
                id: "temp".into(),
                device: "Graphics card".into(),
                name: "GPU core".into(),
                kind: SensorKind::Temperature,
                value: Some(54.0),
            },
            Sensor {
                id: "fan-rpm".into(),
                device: "Graphics card".into(),
                name: "Fan tachometer".into(),
                kind: SensorKind::Fan,
                value: Some(1200.0),
            },
            Sensor {
                id: "power".into(),
                device: "Graphics card".into(),
                name: "Board power".into(),
                kind: SensorKind::Power,
                value: Some(42.5),
            },
            Sensor {
                id: "voltage".into(),
                device: "Graphics card".into(),
                name: "Unavailable rail".into(),
                kind: SensorKind::Voltage,
                value: None,
            },
        ],
        channels: vec![FanChannel {
            id: "fan-control".into(),
            device: "Graphics card".into(),
            name: "GPU Fan 1".into(),
            min_percent: 30.0,
            max_percent: 100.0,
            writable: true,
            reason: String::new(),
        }],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };
    struct State {
        data: Data,
        view: SensorsView,
        sent: Vec<(String, CoolingProfile)>,
    }
    fn harness() -> Harness<'static, State> {
        let mut data = crate::ui_tests::data();
        data.hardware = fixture();
        data.mode = Mode::Service {
            admin: true,
            verified: true,
        };
        data.elevated = true;
        data.status.as_mut().unwrap().dry_run = false;
        Harness::builder()
            .allow_missing_glyphs()
            .with_size([1200.0, 1600.0])
            .build_ui_state(
                |ui, s: &mut State| {
                    for cmd in sensors(ui, &s.data, &mut s.view) {
                        if let UiCommand::SetCooling { channel, profile } = cmd {
                            s.sent.push((channel, profile));
                        }
                    }
                },
                State {
                    data,
                    view: SensorsView::default(),
                    sent: Vec::new(),
                },
            )
    }
    #[test]
    fn sensor_filters_and_confirmed_cooling_profile_flow() {
        let mut h = harness();
        h.run();
        assert!(h.query_by_label("54.0 °C").is_some());
        assert!(h.query_by_label("1200.0 RPM").is_some());
        assert!(h.query_by_label("Unavailable").is_some());
        h.get_by_label("Fan RPM").click();
        h.run();
        assert!(h.query_by_label("GPU core").is_none());
        assert!(h.query_by_label("Fan tachometer").is_some());
        h.get_by_label("All sensors").click();
        h.run();
        h.get_by_label("Manual duty").click();
        h.run();
        h.get_by_label("Apply cooling profile").click();
        h.run();
        assert!(
            h.state().sent.is_empty(),
            "editing/apply asks before a physical write"
        );
        h.get_by_label("Cancel").click();
        h.run();
        assert!(h.state().sent.is_empty());
        h.get_by_label("Temperature curve").click();
        h.run();
        h.get_by_label("Performance").click();
        h.run();
        h.get_by_label("Apply cooling profile").click();
        h.run();
        h.get_by_label("Apply fan control").click();
        h.run();
        assert_eq!(h.state().sent.len(), 1);
        assert_eq!(h.state().sent[0].0, "fan-control");
        assert!(
            matches!(&h.state().sent[0].1.mode,CoolingMode::Curve(points) if points==&CoolingProfile::preset("Performance"))
        );
        h.get_by_label("Hardware automatic").click();
        h.run();
        h.get_by_label("Apply cooling profile").click();
        h.run();
        assert_eq!(h.state().sent[1].1.mode, CoolingMode::Automatic);
    }
    #[test]
    fn stale_data_monitoring_and_read_only_connections_disable_writes() {
        let mut h = harness();
        h.run();
        h.get_by_label("Manual duty").click();
        h.run();
        h.state_mut().data.hardware.sampled_at_ms = unix_ms() - wtm_core::hardware::STALE_MS - 1;
        h.run();
        assert!(
            h.get_by_label("Apply cooling profile")
                .accesskit_node()
                .is_disabled()
        );
        h.state_mut().data.hardware.sampled_at_ms = unix_ms();
        h.state_mut().data.status.as_mut().unwrap().dry_run = true;
        h.run();
        assert!(
            h.get_by_label("Apply cooling profile")
                .accesskit_node()
                .is_disabled()
        );
        h.state_mut().data.status.as_mut().unwrap().dry_run = false;
        h.state_mut().data.hardware.sensors[0].value = None;
        h.run();
        assert!(
            h.get_by_label("Apply cooling profile")
                .accesskit_node()
                .is_disabled()
        );
        h.state_mut().data.hardware = fixture();
        h.state_mut().data.mode = Mode::Local;
        h.run();
        assert!(
            h.get_by_label("Apply cooling profile")
                .accesskit_node()
                .is_disabled()
        );
        h.state_mut().data.hardware = HardwareReport::default();
        h.run();
        assert!(h.query_by_label("No sensor readings yet").is_some());
    }
}

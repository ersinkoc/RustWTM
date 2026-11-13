//! User-selected numeric tray readouts. Rendering and sample selection are pure;
//! native shell ownership stays in `tray` on the application's event thread.
use eframe::egui::{self, Ui};
use wtm_core::hardware::{HardwareReport, SensorKind};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub cpu: bool,
    pub memory: bool,
    pub temperature: bool,
    pub sensor: String,
    pub seconds: u8,
    pub show_logo: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            cpu: false,
            memory: false,
            temperature: false,
            sensor: String::new(),
            seconds: 1,
            show_logo: true,
        }
    }
}
impl Preferences {
    pub fn enabled(&self) -> [bool; 3] {
        [self.cpu, self.memory, self.temperature]
    }
    pub fn interval(&self) -> u64 {
        u64::from(self.seconds.clamp(1, 30))
    }
}
pub const SIZE: usize = 32;
pub const LABELS: [&str; 3] = [
    "CPU usage number",
    "Memory usage number",
    "Temperature number",
];
pub fn refresh_due(
    last: Option<std::time::Instant>,
    changed: bool,
    availability_changed: bool,
    seconds: u64,
    now: std::time::Instant,
) -> bool {
    changed
        || availability_changed
        || last.is_none_or(|at| {
            now.saturating_duration_since(at)
                >= std::time::Duration::from_secs(seconds.clamp(1, 30))
        })
}
const COLORS: [[u8; 3]; 3] = [[90, 180, 245], [183, 145, 245], [242, 187, 76]];

/// No font or native handle allocations. Three fixed-width bitmap digits fit
/// at 100%, including CPU=100. The solid badge works on both taskbar palettes.
pub fn rgba(kind: usize, value: Option<i16>) -> Vec<u8> {
    let mut pixels = vec![0; SIZE * SIZE * 4];
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[18, 25, 33, 255]);
    }
    let text = value
        .filter(|v| (-99..=199).contains(v))
        .map_or_else(|| "--".into(), |v| v.to_string());
    let glyphs: [[u8; 5]; 11] = [
        [7, 5, 5, 5, 7],
        [2, 6, 2, 2, 7],
        [7, 1, 7, 4, 7],
        [7, 1, 7, 1, 7],
        [5, 5, 7, 1, 1],
        [7, 4, 7, 1, 7],
        [7, 4, 7, 5, 7],
        [7, 1, 1, 1, 1],
        [7, 5, 7, 5, 7],
        [7, 5, 7, 1, 7],
        [0, 0, 7, 0, 0],
    ];
    let scale = if text.len() > 2 { 3 } else { 4 };
    let width = text.len() * 3 * scale + (text.len() - 1) * 2;
    let left = (SIZE - width) / 2;
    let top = (28 - 5 * scale) / 2;
    for (index, ch) in text.bytes().enumerate() {
        let glyph = &glyphs[if ch == b'-' {
            10
        } else {
            usize::from(ch - b'0')
        }];
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..3 {
                if bits & (1 << (2 - column)) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let x = left + index * (3 * scale + 2) + column * scale + dx;
                        let y = top + row * scale + dy;
                        let at = (y * SIZE + x) * 4;
                        pixels[at..at + 4].copy_from_slice(&[242, 247, 250, 255]);
                    }
                }
            }
        }
    }
    let color = COLORS[kind.min(2)];
    for y in 28..SIZE {
        for x in 0..SIZE {
            pixels[(y * SIZE + x) * 4..(y * SIZE + x) * 4 + 4]
                .copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
    pixels
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub value: Option<i16>,
    pub tooltip: String,
}
fn temperature<'a>(
    report: &'a HardwareReport,
    prefs: &Preferences,
    now: u64,
) -> Option<&'a wtm_core::hardware::Sensor> {
    if !report.fresh(now) {
        return None;
    }
    report
        .sensors
        .iter()
        .filter(|s| s.kind == SensorKind::Temperature)
        .filter(|s| {
            if prefs.sensor.is_empty() {
                let id = s.id.to_ascii_lowercase();
                let device = s.device.to_ascii_lowercase();
                id.contains("cpu") || device.contains("cpu") || device.contains("processor")
            } else {
                s.id == prefs.sensor
            }
        })
        .filter(|s| {
            s.value
                .is_some_and(|v| v.is_finite() && (-99.0..=199.0).contains(&v))
        })
        .max_by(|a, b| a.value.unwrap().total_cmp(&b.value.unwrap()))
}
pub fn readings(
    data: &crate::backend::Data,
    prefs: &Preferences,
    now: std::time::Instant,
    unix: u64,
) -> [Reading; 3] {
    let fresh = matches!(
        data.sample_freshness(now),
        crate::backend::SampleFreshness::Current(_)
    );
    let system = data.snapshot.as_ref().filter(|_| fresh).map(|s| &s.system);
    let cpu = system.and_then(|s| {
        (s.cpu_percent.is_finite() && (0.0..=100.0).contains(&s.cpu_percent))
            .then_some(s.cpu_percent.round() as i16)
    });
    let memory = system.filter(|s| s.memory_total > 0).map(|s| {
        (s.memory_used() as f64 / s.memory_total as f64 * 100.0)
            .clamp(0.0, 100.0)
            .round() as i16
    });
    let sensor = temperature(&data.hardware, prefs, unix);
    let temp = sensor.and_then(|s| s.value).map(|v| v.round() as i16);
    std::array::from_fn(|kind| {
        let value = [cpu, memory, temp][kind];
        let label = crate::i18n::tr(["CPU", "Memory", "Temperature"][kind]);
        let unit = if kind == 2 { "°C" } else { "%" };
        let text = value.map_or_else(
            || crate::i18n::tr("Unavailable").to_owned(),
            |v| format!("{v}{unit}"),
        );
        let source = if kind == 2 {
            sensor
                .map(|s| format!(" · {} / {}", s.device, s.name))
                .unwrap_or_default()
        } else {
            String::new()
        };
        Reading {
            value,
            tooltip: format!("RustWTM · {label}: {text}{source}")
                .chars()
                .take(120)
                .collect(),
        }
    })
}
pub fn background_interval(ctx: &egui::Context) -> Option<u64> {
    ctx.data(|d| d.get_temp::<u64>(egui::Id::unique("numeric-tray-interval")))
}
pub fn publish_interval(ctx: &egui::Context, prefs: &Preferences) {
    ctx.data_mut(|d| {
        if prefs.enabled().iter().any(|v| *v) {
            d.insert_temp(egui::Id::unique("numeric-tray-interval"), prefs.interval());
        } else {
            d.remove::<u64>(egui::Id::unique("numeric-tray-interval"));
        }
    });
}

fn preview(ui: &mut Ui, kind: usize, value: Option<i16>) {
    let id = egui::Id::unique(("numeric-tray-preview", kind));
    let existing = ui
        .ctx()
        .data(|d| d.get_temp::<(Option<i16>, egui::TextureHandle)>(id));
    let texture = match existing {
        Some((old, texture)) if old == value => texture,
        _ => {
            let texture = ui.ctx().load_texture(
                format!("numeric-tray-{kind}"),
                egui::ColorImage::from_rgba_unmultiplied([SIZE, SIZE], &rgba(kind, value)),
                egui::TextureOptions::NEAREST,
            );
            ui.ctx()
                .data_mut(|d| d.insert_temp(id, (value, texture.clone())));
            texture
        }
    };
    ui.image((texture.id(), egui::vec2(32.0, 32.0)));
}

pub fn settings(ui: &mut Ui, prefs: &mut Preferences, data: &crate::backend::Data) -> bool {
    let readings = readings(
        data,
        prefs,
        std::time::Instant::now(),
        wtm_core::hardware::unix_ms(),
    );
    use crate::theme::{
        icon, option_group, option_labelled, option_named_by_control, option_switch,
    };
    option_group(ui, crate::i18n::tr("Numeric tray icons"));
    ui.weak(crate::i18n::tr(
        "Show resource readings as separate number icons, including while the window is hidden.",
    ));
    for (kind, label) in LABELS.iter().enumerate() {
        let title = crate::i18n::tr(label);
        let enabled = match kind {
            0 => &mut prefs.cpu,
            1 => &mut prefs.memory,
            _ => &mut prefs.temperature,
        };
        option_named_by_control(ui, icon::CHART, title, &readings[kind].tooltip, |ui| {
            preview(ui, kind, readings[kind].value);
            ui.add_space(8.0);
            ui.label(crate::i18n::tr(if *enabled { "On" } else { "Off" }));
            crate::theme::switch(ui, enabled, title);
        });
    }
    option_switch(
        ui,
        icon::MONITOR,
        crate::i18n::tr("Keep the RustWTM logo icon"),
        crate::i18n::tr(
            "The logo stays available if no number icon is active. Windows controls which icons are pinned.",
        ),
        &mut prefs.show_logo,
    );
    option_labelled(
        ui,
        icon::SYNC,
        crate::i18n::tr("Tray numbers refresh"),
        "",
        |ui, id| {
            ui.add(egui::Slider::new(&mut prefs.seconds, 1..=30).suffix(" s"))
                .labelled_by(id);
        },
    );
    let selected = if prefs.sensor.is_empty() {
        crate::i18n::tr("Automatic CPU temperature").into()
    } else {
        data.hardware
            .sensors
            .iter()
            .find(|s| s.id == prefs.sensor)
            .map(|s| format!("{} · {}", s.device, s.name))
            .unwrap_or_else(|| crate::i18n::tr("Saved sensor unavailable").into())
    };
    option_labelled(
        ui,
        icon::SETTINGS,
        crate::i18n::tr("Temperature source"),
        crate::i18n::tr(
            "Temperature requires a supported sensor. Missing or stale readings show dashes, never a guessed value.",
        ),
        |ui, id| {
            egui::ComboBox::from_id_salt("tray-temperature-source")
                .selected_text(selected)
                .width(260.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut prefs.sensor,
                        String::new(),
                        crate::i18n::tr("Automatic CPU temperature"),
                    );
                    for s in data
                        .hardware
                        .sensors
                        .iter()
                        .filter(|s| s.kind == SensorKind::Temperature)
                        .take(128)
                    {
                        ui.selectable_value(
                            &mut prefs.sensor,
                            s.id.clone(),
                            format!("{} · {}", s.device, s.name),
                        );
                    }
                })
                .response
                .labelled_by(id);
        },
    );
    ui.add_space(4.0);
    let sensors = ui.button(crate::i18n::tr("Open sensors")).clicked();
    if let Some(error) = ui
        .ctx()
        .data(|d| d.get_temp::<String>(egui::Id::unique("numeric-tray-error")))
    {
        crate::theme::colored(ui, crate::theme::WARN, error);
    }
    sensors
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    #[test]
    fn refresh_interval_is_respected_and_availability_changes_are_immediate() {
        let now = std::time::Instant::now();
        assert!(refresh_due(None, false, false, 30, now));
        assert!(!refresh_due(
            Some(now),
            false,
            false,
            30,
            now + std::time::Duration::from_secs(1)
        ));
        assert!(refresh_due(
            Some(now),
            false,
            false,
            30,
            now + std::time::Duration::from_secs(30)
        ));
        assert!(refresh_due(Some(now), false, true, 30, now));
        assert!(refresh_due(Some(now), true, false, 30, now));
    }
    #[test]
    fn readouts_use_fresh_samples_and_unknown_memory_is_not_zero_percent() {
        let now = std::time::Instant::now();
        let mut data = crate::backend::Data {
            sample_measured_at: Some(now),
            snapshot: Some(std::sync::Arc::new(wtm_core::Snapshot {
                system: wtm_core::SystemStats {
                    cpu_percent: 99.8,
                    memory_total: 32 << 30,
                    memory_available: 16 << 30,
                    ..Default::default()
                },
                processes: vec![],
                interval: Some(std::time::Duration::from_secs(1)),
                taken_at_ms: 1000,
                net_traced: false,
            })),
            ..Default::default()
        };
        let prefs = Preferences::default();
        let values = readings(&data, &prefs, now, 1000);
        assert_eq!(
            [values[0].value, values[1].value, values[2].value],
            [Some(100), Some(50), None]
        );
        let values = readings(&data, &prefs, now + std::time::Duration::from_secs(6), 1000);
        assert!(values.iter().all(|v| v.value.is_none()));
        std::sync::Arc::make_mut(data.snapshot.as_mut().unwrap())
            .system
            .memory_total = 0;
        assert_eq!(readings(&data, &prefs, now, 1000)[1].value, None);
    }
    #[test]
    fn settings_toggle_each_number_and_select_an_explicit_sensor_without_shell_actions() {
        let data = crate::backend::Data {
            hardware: crate::sensors::fixture(),
            ..Default::default()
        };
        let mut h = egui_kittest::Harness::builder()
            .allow_missing_glyphs()
            .with_size([900.0, 1000.0])
            .build_ui_state(
                move |ui, prefs: &mut Preferences| {
                    settings(ui, prefs, &data);
                },
                Preferences::default(),
            );
        h.run();
        for label in LABELS {
            h.get_by_label(label).click();
            h.run();
        }
        assert_eq!(h.state().enabled(), [true; 3]);
        h.get_by_label("Temperature source").click();
        h.run();
        h.get_by_label("Graphics card · GPU core").click();
        h.run();
        assert_eq!(h.state().sensor, "temp");
        h.get_by_label("Keep the RustWTM logo icon").click();
        h.run();
        assert!(!h.state().show_logo);
    }
    #[test]
    fn numeric_badges_are_bounded_and_distinguish_unavailable_zero_and_one_hundred() {
        for (kind, color) in COLORS.iter().enumerate() {
            for value in [
                None,
                Some(-99),
                Some(0),
                Some(9),
                Some(99),
                Some(100),
                Some(199),
                Some(32767),
            ] {
                let pixels = rgba(kind, value);
                assert_eq!(pixels.len(), 4096);
                assert!(pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
                assert_eq!(&pixels[(31 * 32) * 4..(31 * 32) * 4 + 3], color);
            }
        }
        assert_ne!(rgba(0, None), rgba(0, Some(0)));
        assert_ne!(rgba(0, Some(99)), rgba(0, Some(100)));
        assert_eq!(rgba(0, Some(32767)), rgba(0, None));
    }
    #[test]
    fn automatic_temperature_never_substitutes_a_gpu_or_a_stale_sensor() {
        let mut report = HardwareReport {
            sampled_at_ms: 1000,
            sensors: vec![
                wtm_core::hardware::Sensor {
                    id: "/cpu/temperature".into(),
                    device: "CPU".into(),
                    name: "Package".into(),
                    kind: SensorKind::Temperature,
                    value: Some(64.0),
                },
                wtm_core::hardware::Sensor {
                    id: "/gpu/temperature".into(),
                    device: "GPU".into(),
                    name: "Core".into(),
                    kind: SensorKind::Temperature,
                    value: Some(95.0),
                },
            ],
            ..Default::default()
        };
        let mut prefs = Preferences::default();
        assert_eq!(
            temperature(&report, &prefs, 1000).unwrap().value,
            Some(64.0)
        );
        prefs.sensor = "/gpu/temperature".into();
        assert_eq!(
            temperature(&report, &prefs, 1000).unwrap().value,
            Some(95.0)
        );
        assert!(temperature(&report, &prefs, 20_000).is_none());
        prefs.sensor.clear();
        report.sensors.remove(0);
        assert!(temperature(&report, &prefs, 1000).is_none());
        prefs.sensor = "/gpu/temperature".into();
        report.sensors[0].value = Some(f32::NAN);
        assert!(temperature(&report, &prefs, 1000).is_none());
    }
    #[test]
    fn disabled_preferences_do_not_request_background_updates_and_roundtrip() {
        let ctx = egui::Context::default();
        let mut prefs = Preferences::default();
        publish_interval(&ctx, &prefs);
        assert_eq!(background_interval(&ctx), None);
        prefs.cpu = true;
        prefs.seconds = 0;
        publish_interval(&ctx, &prefs);
        assert_eq!(background_interval(&ctx), Some(1));
        prefs.temperature = true;
        prefs.sensor = "/cpu/package".into();
        prefs.show_logo = false;
        assert_eq!(
            serde_json::from_str::<Preferences>(&serde_json::to_string(&prefs).unwrap()).unwrap(),
            prefs
        );
        prefs.cpu = false;
        prefs.temperature = false;
        publish_interval(&ctx, &prefs);
        assert_eq!(background_interval(&ctx), None);
    }
}

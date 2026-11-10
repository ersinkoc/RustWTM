//! Device health presentation from measured values, with no invented thresholds.
use eframe::egui::{self, RichText, Ui};
use wtm_core::devices::Device;
fn field<'a>(d: &'a Device, name: &str) -> Option<&'a str> {
    d.fields
        .iter()
        .find(|f| f.name == name)
        .map(|f| f.value.as_str())
}
fn metric(d: &Device, id: &str) -> Option<f64> {
    d.metrics
        .iter()
        .find(|m| m.id == id)
        .and_then(|m| m.value)
        .filter(|v| v.is_finite())
}
fn lifetime_bytes(d: &Device, name: &str) -> Option<u128> {
    field(d, name)?.parse::<u128>().ok()?.checked_mul(512_000)
}
fn unavailable() -> String {
    crate::i18n::tr("Unavailable").into()
}
fn value(ui: &mut Ui, title: &str, text: &str) {
    ui.weak(crate::i18n::tr(title));
    ui.label(RichText::new(text).size(22.0));
}
fn property(ui: &mut Ui, title: &str, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(crate::i18n::tr(title)).weak());
        ui.label(crate::i18n::tr(text));
    });
}
pub fn show(ui: &mut Ui, d: &Device) {
    let nvme_remaining = metric(d, "nvme-remaining");
    let remaining = nvme_remaining.or_else(|| metric(d, "wear"));
    let temperature = metric(d, "nvme-temperature").or_else(|| metric(d, "temperature"));
    let spare = metric(d, "nvme-spare");
    let critical = field(d, "NVMe critical warning bits")
        .and_then(|v| u8::from_str_radix(v.trim_start_matches("0x"), 16).ok());
    crate::theme::card(ui, crate::i18n::tr("Health"), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.weak(crate::i18n::tr("Windows device telemetry"));
            if let Some(time) = i64::try_from(d.at)
                .ok()
                .and_then(chrono::DateTime::from_timestamp_millis)
            {
                ui.weak(
                    time.with_timezone(&chrono::Local)
                        .format("%H:%M:%S")
                        .to_string(),
                );
            }
        });
        let status = if critical.is_some_and(|bits| bits != 0) {
            crate::i18n::tr("Warnings")
        } else {
            field(d, "Storage health")
                .map(crate::i18n::tr)
                .unwrap_or(crate::i18n::tr("Unavailable"))
        };
        value(ui, "Storage health", status);
        if let Some(bits) = critical {
            ui.weak(format!("NVMe · 0x{bits:02X}"));
        }
        let columns = if ui.available_width() >= 600.0 { 3 } else { 1 };
        ui.columns(columns, |cols| {
            let col = &mut cols[0];
            value(
                col,
                if nvme_remaining.is_some() {
                    "NVMe remaining endurance"
                } else {
                    "Wear used"
                },
                &remaining.map_or_else(unavailable, |v| format!("{v:.0}%")),
            );
            if let Some(percent) = remaining {
                col.add(
                    egui::ProgressBar::new((percent / 100.0) as f32)
                        .corner_radius(0.0)
                        .desired_height(7.0)
                        .fill(crate::theme::text_color(col, crate::theme::DISK_READ))
                        .desired_width(col.available_width()),
                );
            }
            let col = &mut cols[if columns == 1 { 0 } else { 1 }];
            value(
                col,
                if metric(d, "nvme-temperature").is_some() {
                    "NVMe temperature"
                } else {
                    "Temperatures"
                },
                &temperature.map_or_else(unavailable, |v| format!("{v:.0} °C")),
            );
            let col = &mut cols[if columns == 1 { 0 } else { 2 }];
            value(
                col,
                "NVMe available spare",
                &spare.map_or_else(unavailable, |v| format!("{v:.0}%")),
            );
            if let Some(percent) = spare {
                col.add(
                    egui::ProgressBar::new((percent / 100.0) as f32)
                        .corner_radius(0.0)
                        .desired_height(7.0)
                        .fill(crate::theme::text_color(col, crate::theme::DISK_READ))
                        .desired_width(col.available_width()),
                );
            }
        });
        crate::theme::divider(ui);
        let counters = [
            (
                "Read",
                lifetime_bytes(d, "Data read units (512000 bytes)")
                    .map(|v| wtm_core::format_bytes(v as f64)),
            ),
            (
                "Write",
                lifetime_bytes(d, "Data written units (512000 bytes)")
                    .map(|v| wtm_core::format_bytes(v as f64)),
            ),
            (
                "Power-on time",
                field(d, "NVMe power-on hours")
                    .map(|s| format!("{s} h"))
                    .or_else(|| metric(d, "hours").map(|v| format!("{v:.0} h"))),
            ),
            (
                "NVMe power cycles",
                field(d, "NVMe power cycles").map(str::to_owned),
            ),
            (
                "Unsafe shutdowns",
                field(d, "Unsafe shutdowns").map(str::to_owned),
            ),
            ("Media errors", field(d, "Media errors").map(str::to_owned)),
        ];
        let columns = if ui.available_width() >= 800.0 {
            6
        } else if ui.available_width() >= 500.0 {
            3
        } else {
            2
        };
        for row in counters.chunks(columns) {
            ui.columns(columns, |cols| {
                for (col, (name, text)) in cols.iter_mut().zip(row) {
                    col.weak(crate::i18n::tr(name));
                    col.label(text.clone().unwrap_or_else(unavailable));
                }
            });
        }
    });
    crate::theme::card(ui, crate::i18n::tr("Hardware specifications"), |ui| {
        let fields: Vec<_> = d.fields.iter().filter(|f| !is_health(&f.name)).collect();
        if fields.is_empty() {
            ui.weak(crate::i18n::tr("Unavailable"));
        }
        let columns = if ui.available_width() >= 650.0 { 2 } else { 1 };
        for row in fields.chunks(columns) {
            ui.columns(columns, |cols| {
                for (col, f) in cols.iter_mut().zip(row) {
                    property(col, &f.name, &f.value);
                }
            });
        }
    });
    crate::theme::card(ui, "SMART / NVMe", |ui| {
        let fields: Vec<_> = d.fields.iter().filter(|f| is_health(&f.name)).collect();
        if fields.is_empty() {
            ui.weak(crate::i18n::tr("Unavailable"));
        }
        let columns = if ui.available_width() >= 650.0 { 2 } else { 1 };
        let rows = fields.len().div_ceil(columns).max(1);
        ui.columns(columns, |cols| {
            for (index, col) in cols.iter_mut().enumerate() {
                for f in fields.iter().skip(index * rows).take(rows) {
                    property(
                        col,
                        if f.name == "NVMe critical warning bits" {
                            "Warnings"
                        } else {
                            &f.name
                        },
                        &f.value,
                    );
                    crate::theme::divider(col);
                }
            }
        });
    });
}
fn is_health(name: &str) -> bool {
    name.starts_with("NVMe ")
        || name.starts_with("Data read units")
        || name.starts_with("Data written units")
        || matches!(name, "Storage health" | "Unsafe shutdowns" | "Media errors")
}
#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    #[test]
    fn absent_health_readings_stay_unavailable_and_critical_bits_override_healthy() {
        let mut d = Device {
            id: "d".into(),
            kind: "Disk".into(),
            name: "Unknown controller".into(),
            at: 0,
            fields: vec![],
            metrics: vec![],
            warning: String::new(),
        };
        let mut h = egui_kittest::Harness::builder()
            .with_size([780.0, 1200.0])
            .build_ui_state(|ui, d: &mut Device| show(ui, d), d.clone());
        h.run();
        assert!(h.query_all_by_label("Unavailable").count() >= 6);
        assert!(h.query_by_label("100%").is_none());
        assert!(h.query_by_label("0 °C").is_none());
        d.fields = vec![
            wtm_core::devices::Field {
                name: "Storage health".into(),
                value: "Healthy".into(),
            },
            wtm_core::devices::Field {
                name: "NVMe critical warning bits".into(),
                value: "0x04".into(),
            },
        ];
        *h.state_mut() = d;
        h.run();
        assert!(h.query_all_by_label("Warnings").count() >= 1);
        assert!(h.query_by_label("NVMe · 0x04").is_some());
    }
    #[test]
    fn lifetime_conversion_is_exact_and_does_not_wrap() {
        let mut d = Device {
            id: "d".into(),
            kind: "Disk".into(),
            name: "SSD".into(),
            at: 1000,
            fields: vec![],
            metrics: vec![],
            warning: String::new(),
        };
        assert_eq!(lifetime_bytes(&d, "Data read units (512000 bytes)"), None);
        d.fields.push(wtm_core::devices::Field {
            name: "Data read units (512000 bytes)".into(),
            value: "123".into(),
        });
        assert_eq!(
            lifetime_bytes(&d, "Data read units (512000 bytes)"),
            Some(62_976_000)
        );
        d.fields[0].value = u128::MAX.to_string();
        assert_eq!(lifetime_bytes(&d, "Data read units (512000 bytes)"), None);
    }
}

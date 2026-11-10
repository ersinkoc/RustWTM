//! Read-only device telemetry with explicit source, identity and missing values.
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub value: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    pub id: String,
    pub name: String,
    pub unit: String,
    pub value: Option<f64>,
    #[serde(default)]
    pub ceiling: Option<f64>,
    #[serde(default)]
    pub counter: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub at: u64,
    pub fields: Vec<Field>,
    pub metrics: Vec<Metric>,
    #[serde(default)]
    pub warning: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    pub devices: Vec<Device>,
    pub warnings: Vec<String>,
}

pub fn parse(value: serde_json::Value) -> Result<Report, String> {
    let report: Report = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if report.devices.len() > 128 || report.warnings.len() > 32 {
        return Err("Device response exceeds its limit".into());
    }
    let mut ids = std::collections::HashSet::new();
    for d in &report.devices {
        if d.id.is_empty()
            || !ids.insert(&d.id)
            || d.fields.len() > 32
            || d.metrics.len() > 24
            || [&d.id, &d.kind, &d.name, &d.warning]
                .iter()
                .any(|s| s.len() > 1024)
            || d.fields
                .iter()
                .any(|f| f.name.len() > 128 || f.value.len() > 1024)
            || d.metrics.iter().any(|m| {
                m.id.len() > 128
                    || m.name.len() > 128
                    || m.unit.len() > 32
                    || m.value.is_some_and(|v| !v.is_finite())
                    || m.ceiling.is_some_and(|v| !v.is_finite() || v <= 0.0)
            })
        {
            return Err("Invalid or oversized device response".into());
        }
        let mut metrics = std::collections::HashSet::new();
        if d.metrics.iter().any(|m| !metrics.insert(&m.id)) {
            return Err("Duplicate device metric".into());
        }
    }
    Ok(report)
}

pub fn collect() -> Result<Report, String> {
    let mut report = parse(crate::probe::json(
        include_str!("devices.ps1"),
        &serde_json::Value::Null,
        Duration::from_secs(90),
    )?)?;
    if let Ok(adapters) = wtm_sys::gpu::adapters() {
        for adapter in adapters {
            let prefix = format!("gpu:{}:", adapter.luid);
            for device in report
                .devices
                .iter_mut()
                .filter(|d| d.id.starts_with(&prefix))
            {
                device.name = adapter.name.clone();
                device.fields.push(Field {
                    name: "Dedicated memory capacity".into(),
                    value: crate::format_bytes(adapter.dedicated_bytes as f64),
                });
                device.fields.push(Field {
                    name: "Shared memory capacity".into(),
                    value: crate::format_bytes(adapter.shared_bytes as f64),
                });
            }
        }
    }
    for device in report.devices.iter_mut().filter(|d| {
        d.kind == "Disk"
            && d.fields
                .iter()
                .any(|f| f.name == "Bus" && f.value.eq_ignore_ascii_case("NVMe"))
    }) {
        let number = device
            .fields
            .iter()
            .find(|f| f.name == "Disk number")
            .and_then(|f| f.value.parse::<u32>().ok());
        if let Some(number) = number {
            match wtm_sys::nvme_health::read(number) {
                Ok(health) => {
                    device.fields.push(Field {
                        name: "NVMe critical warning bits".into(),
                        value: format!("0x{:02x}", health.critical),
                    });
                    for (name, value) in [
                        ("Data read units (512000 bytes)", health.read_units),
                        ("Data written units (512000 bytes)", health.written_units),
                        ("NVMe power cycles", health.power_cycles),
                        ("NVMe power-on hours", health.hours),
                        ("Unsafe shutdowns", health.unsafe_shutdowns),
                        ("Media errors", health.media_errors),
                    ] {
                        device.fields.push(Field {
                            name: name.into(),
                            value: value.to_string(),
                        });
                    }
                    device.metrics.push(Metric {
                        id: "nvme-remaining".into(),
                        name: "NVMe remaining endurance".into(),
                        unit: "%".into(),
                        value: Some(f64::from(100u8.saturating_sub(health.used))),
                        ceiling: Some(100.0),
                        counter: false,
                    });
                    device.metrics.push(Metric {
                        id: "nvme-spare".into(),
                        name: "NVMe available spare".into(),
                        unit: "%".into(),
                        value: health.spare.map(f64::from),
                        ceiling: Some(100.0),
                        counter: false,
                    });
                    if let Some(value) = health.temperature {
                        device.metrics.push(Metric {
                            id: "nvme-temperature".into(),
                            name: "NVMe temperature".into(),
                            unit: "C".into(),
                            value: Some(value),
                            ceiling: None,
                            counter: false,
                        });
                    }
                    if health.critical != 0 {
                        device.warning = "NVMe controller reports a critical health warning".into();
                    }
                }
                Err(error) => device.fields.push(Field {
                    name: "NVMe health query".into(),
                    value: format!("Unavailable: {error}"),
                }),
            }
        }
    }
    Ok(report)
}

/// Counter reset, a long gap or an unknown sample never becomes a zero-rate reading.
pub fn rate(before: Option<(u64, f64)>, at: u64, total: Option<f64>) -> Option<f64> {
    let ((old_at, old), now) = (before?, total?);
    let elapsed = at.checked_sub(old_at)?;
    if !(500..=120_000).contains(&elapsed) || now < old {
        return None;
    }
    Some((now - old) * 1000.0 / elapsed as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_capacity_math_uses_scalars_without_allocating_memory() {
        let helpers = include_str!("devices.ps1")
            .split("# Native providers")
            .next()
            .unwrap();
        let script = format!(
            "{helpers}\n@{{used=(UsedMemory 137438953472 34359738368);negative=(UsedMemory 10 20)}} | ConvertTo-Json -Compress"
        );
        let result = crate::probe::json(
            &script,
            &serde_json::Value::Null,
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(result["used"].as_f64(), Some(103079215104.0));
        assert_eq!(result["negative"].as_f64(), Some(0.0));
    }
    #[test]
    fn device_rates_preserve_unknown_reset_and_time_gaps() {
        assert_eq!(rate(Some((1000, 100.0)), 2000, Some(1124.0)), Some(1024.0));
        assert_eq!(rate(Some((1000, 100.0)), 2000, Some(99.0)), None);
        assert_eq!(rate(Some((1000, 100.0)), 1000, Some(200.0)), None);
        assert_eq!(rate(Some((1000, 100.0)), 200_000, Some(200.0)), None);
        assert_eq!(rate(None, 2000, Some(200.0)), None);
        assert_eq!(rate(Some((1000, 100.0)), 2000, None), None);
    }
    #[test]
    fn duplicate_identity_and_infinite_metrics_are_not_accepted() {
        let device = serde_json::json!({"id":"disk:0","kind":"Disk","name":"SSD","at":1000,"fields":[],"metrics":[]});
        assert!(
            parse(serde_json::json!({"devices":[device.clone(),device],"warnings":[]})).is_err()
        );
        let r =
            parse(serde_json::json!({"devices":[],"warnings":["Battery unavailable"]})).unwrap();
        assert!(r.devices.is_empty());
        assert_eq!(r.warnings.len(), 1);
    }
}
#[cfg(test)]
mod native_review {
    #[test]
    #[ignore = "Read-only developer hardware query; run explicitly"]
    fn native_device_query() {
        let report = super::collect().expect("bounded device query");
        let mut counts = std::collections::BTreeMap::new();
        for d in &report.devices {
            *counts.entry(d.kind.as_str()).or_insert(0usize) += 1;
        }
        println!("Device kinds: {counts:?}; warnings: {:?}", report.warnings);
        assert!(
            !report.devices.is_empty(),
            "No native device collector produced data: {:?}",
            report.warnings
        );
    }
}

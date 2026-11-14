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

/// One attribute of an ATA SMART table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SmartAttribute {
    id: u8,
    /// Normalized value; higher is healthier.
    current: u8,
    /// 48-bit raw value, little-endian on the wire.
    raw: u64,
}

/// The attributes of an ATA SMART table given as hex: a 2-byte version,
/// then 30 entries of 12 bytes (id, flags, current, worst, 6 raw, reserved).
fn smart_attributes(hex: &str) -> Vec<SmartAttribute> {
    let bytes: Vec<u8> = hex
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .filter_map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect();
    if bytes.len() * 2 != hex.len() {
        return Vec::new();
    }
    bytes
        .get(2..)
        .unwrap_or_default()
        .as_chunks::<12>()
        .0
        .iter()
        .take(30)
        .filter(|e| e[0] != 0)
        .map(|e| SmartAttribute {
            id: e[0],
            current: e[3],
            raw: e[5..11]
                .iter()
                .rev()
                .fold(0u64, |acc, b| (acc << 8) | u64::from(*b)),
        })
        .collect()
}

/// Adds the SMART readings of a SATA disk and removes the raw table.
fn apply_smart(device: &mut Device) {
    let Some(at) = device
        .fields
        .iter()
        .position(|f| f.name == "ATA SMART table")
    else {
        return;
    };
    let table = device.fields.remove(at).value;
    let attributes = smart_attributes(&table);
    if attributes.is_empty() {
        return;
    }
    let get = |id: u8| attributes.iter().find(|a| a.id == id);
    for (id, metric, name) in [
        (5, "smart-reallocated", "Reallocated sectors"),
        (197, "smart-pending", "Pending sectors"),
        (198, "smart-uncorrectable", "Offline uncorrectable sectors"),
    ] {
        if let Some(a) = get(id) {
            device.metrics.push(Metric {
                id: metric.into(),
                name: name.into(),
                unit: String::new(),
                value: Some(a.raw as f64),
                ceiling: None,
                counter: false,
            });
        }
    }
    // Remaining life: Media Wearout Indicator, SSD Life Left, or Samsung's
    // wear leveling count, whichever the drive reports; normalized 0-100.
    if let Some(a) = [233, 231, 202, 177].into_iter().find_map(get)
        && a.current <= 100
    {
        device.metrics.push(Metric {
            id: "smart-life".into(),
            name: "Remaining life (SMART)".into(),
            unit: "%".into(),
            value: Some(f64::from(a.current)),
            ceiling: Some(100.0),
            counter: false,
        });
    }
    if let Some(a) = get(194)
        && let Some(t) = device
            .metrics
            .iter_mut()
            .find(|m| m.id == "temperature" && m.value.is_none())
    {
        // The low byte is the current temperature in °C.
        t.value = Some(f64::from((a.raw & 0xff) as u8));
    }
    for (id, name) in [
        (9, "SMART power-on hours"),
        (4, "Start/stop count"),
        (12, "Power cycle count"),
        (241, "Total host writes (vendor units)"),
        (242, "Total host reads (vendor units)"),
    ] {
        if let Some(a) = get(id) {
            let value = if id == 9 { a.raw & 0xffff_ffff } else { a.raw };
            device.fields.push(Field {
                name: name.into(),
                value: value.to_string(),
            });
        }
    }
}

/// The display driver class; one numbered subkey per installed adapter driver.
const DISPLAY_CLASS: &str =
    r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

/// Whether a driver's `MatchingDeviceId` names this PCI vendor and device.
fn matches_hardware(matching_id: &str, vendor: u32, device: u32) -> bool {
    matching_id
        .to_ascii_lowercase()
        .contains(&format!("ven_{vendor:04x}&dev_{device:04x}"))
}

/// `M-D-YYYY` as the registry stores driver dates, as `YYYY-MM-DD`.
fn iso_date(date: &str) -> Option<String> {
    let mut parts = date.trim().split('-').map(|p| p.parse::<u32>().ok());
    let (m, d, y) = (parts.next()??, parts.next()??, parts.next()??);
    (parts.next().is_none() && (1..=12).contains(&m) && (1..=31).contains(&d) && y >= 1990)
        .then(|| format!("{y:04}-{m:02}-{d:02}"))
}

/// `D3D_FEATURE_LEVEL_x_y` as `x_y`.
fn feature_level_text(level: u32) -> Option<String> {
    let (major, minor) = ((level >> 12) & 0xf, (level >> 8) & 0xf);
    (major >= 9 && level & 0xff == 0 && level >> 16 == 0).then(|| format!("{major}_{minor}"))
}

/// The highest Direct3D 12 feature level Windows recorded for the adapter
/// with this LUID. Read from the registry: creating a Direct3D device here
/// would load the graphics driver into this process.
fn max_feature_level(luid: u64) -> Option<String> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    let directx = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"SOFTWARE\Microsoft\DirectX", KEY_READ)
        .ok()?;
    directx
        .enum_keys()
        .filter_map(Result::ok)
        .take(64)
        .filter_map(|name| directx.open_subkey_with_flags(name, KEY_READ).ok())
        .find(|key| key.get_value::<u64, _>("AdapterLuid").ok() == Some(luid))
        .and_then(|key| key.get_value::<u32, _>("MaxD3D12FeatureLevel").ok())
        .and_then(feature_level_text)
}

/// Version and date of the installed driver for this adapter.
fn display_driver(vendor: u32, device: u32) -> Option<(String, Option<String>)> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    let class = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(DISPLAY_CLASS, KEY_READ)
        .ok()?;
    class
        .enum_keys()
        .filter_map(Result::ok)
        .filter(|name| name.len() == 4 && name.bytes().all(|b| b.is_ascii_digit()))
        .take(64)
        .filter_map(|name| class.open_subkey_with_flags(name, KEY_READ).ok())
        .find_map(|key| {
            let id: String = key.get_value("MatchingDeviceId").ok()?;
            if !matches_hardware(&id, vendor, device) {
                return None;
            }
            let version: String = key.get_value("DriverVersion").ok()?;
            let date = key
                .get_value::<String, _>("DriverDate")
                .ok()
                .and_then(|d| iso_date(&d));
            Some((version, date))
        })
}

/// What the Performance pages list under a GPU's graphs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GpuFacts {
    /// `{high:08x}_{low:08x}`, as the GPU usage counters name it.
    pub luid: String,
    pub name: String,
    pub dedicated_total: u64,
    pub shared_total: u64,
    pub driver_version: Option<String>,
    pub driver_date: Option<String>,
    /// The highest Direct3D 12 feature level, such as "12_2".
    pub directx: Option<String>,
}

/// Every hardware GPU, in DXGI order (Task Manager's GPU 0, GPU 1, ...).
/// Reads the registry, so call it seldom.
pub fn gpu_facts() -> Vec<GpuFacts> {
    wtm_sys::gpu::adapters()
        .unwrap_or_default()
        .into_iter()
        .map(|a| {
            let driver = display_driver(a.vendor_id, a.device_id);
            GpuFacts {
                directx: max_feature_level(a.luid_value),
                driver_version: driver.as_ref().map(|d| d.0.clone()),
                driver_date: driver.and_then(|d| d.1),
                luid: a.luid,
                name: a.name,
                dedicated_total: a.dedicated_bytes,
                shared_total: a.shared_bytes,
            }
        })
        .collect()
}

/// SMART readings are asked for at most this often; between reads the last
/// ones are shown, so spun-down disks are not woken on every refresh.
const SMART_EVERY: Duration = Duration::from_secs(10 * 60);

/// Whether a SMART read is due, given when the last one was asked for.
fn smart_due(last: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    last.is_none_or(|at| now.saturating_duration_since(at) >= SMART_EVERY)
}

/// SMART fields from the last read, per disk ID.
type SmartCache = std::collections::HashMap<String, Vec<Field>>;

fn is_smart_field(f: &Field) -> bool {
    f.name == "ATA SMART table" || f.name == "SMART attributes"
}

/// Keeps the SMART fields of a read, or puts the remembered ones back on the
/// disks of a report that skipped the read.
fn carry_smart(report: &mut Report, cache: &mut SmartCache, read: bool) {
    let disks = report.devices.iter_mut().filter(|d| d.kind == "Disk");
    if read {
        cache.clear();
        for d in disks {
            let fields: Vec<Field> = d
                .fields
                .iter()
                .filter(|f| is_smart_field(f))
                .cloned()
                .collect();
            if !fields.is_empty() {
                cache.insert(d.id.clone(), fields);
            }
        }
    } else {
        for d in disks {
            if let Some(fields) = cache.get(&d.id)
                && !d.fields.iter().any(is_smart_field)
            {
                d.fields.extend(fields.iter().cloned());
            }
        }
    }
}

pub fn collect() -> Result<Report, String> {
    use std::sync::{Mutex, OnceLock};
    static SMART: OnceLock<Mutex<(Option<std::time::Instant>, SmartCache)>> = OnceLock::new();
    let smart = SMART.get_or_init(Default::default);
    let now = std::time::Instant::now();
    let read = smart.lock().map(|s| smart_due(s.0, now)).unwrap_or(false);
    let mut report = parse(crate::probe::json(
        include_str!("devices.ps1"),
        &serde_json::json!({ "smart": read }),
        Duration::from_secs(90),
    )?)?;
    if let Ok(mut smart) = smart.lock() {
        if read {
            smart.0 = Some(now);
        }
        carry_smart(&mut report, &mut smart.1, read);
    }
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
                if let Some((version, date)) = display_driver(adapter.vendor_id, adapter.device_id)
                {
                    device.fields.push(Field {
                        name: "Driver version".into(),
                        value: version,
                    });
                    if let Some(date) = date {
                        device.fields.push(Field {
                            name: "Driver date".into(),
                            value: date,
                        });
                    }
                }
                device.fields.push(Field {
                    name: "DirectX feature level".into(),
                    value: max_feature_level(adapter.luid_value)
                        .unwrap_or_else(|| "Unavailable".into()),
                });
            }
        }
    }
    for device in report.devices.iter_mut().filter(|d| d.kind == "Disk") {
        apply_smart(device);
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
    fn smart_is_read_every_ten_minutes_and_remembered_in_between() {
        let start = std::time::Instant::now();
        assert!(smart_due(None, start));
        assert!(!smart_due(Some(start), start + Duration::from_secs(60)));
        assert!(smart_due(Some(start), start + SMART_EVERY));

        let disk = |fields: Vec<Field>| Device {
            id: "disk:1".into(),
            kind: "Disk".into(),
            name: "SATA".into(),
            at: 0,
            fields,
            metrics: Vec::new(),
            warning: String::new(),
        };
        let table = Field {
            name: "ATA SMART table".into(),
            value: "00".into(),
        };
        let mut cache = SmartCache::new();
        let mut read = Report {
            devices: vec![disk(vec![table.clone()])],
            warnings: Vec::new(),
        };
        carry_smart(&mut read, &mut cache, true);
        let mut skipped = Report {
            devices: vec![disk(Vec::new())],
            warnings: Vec::new(),
        };
        carry_smart(&mut skipped, &mut cache, false);
        assert_eq!(skipped.devices[0].fields.len(), 1);
        assert_eq!(skipped.devices[0].fields[0].name, table.name);
    }

    fn smart_hex(entries: &[(u8, u8, u64)]) -> String {
        let mut bytes = vec![0x10, 0x00];
        for &(id, current, raw) in entries {
            let mut e = [0u8; 12];
            e[0] = id;
            e[3] = current;
            e[5..11].copy_from_slice(&raw.to_le_bytes()[..6]);
            bytes.extend(e);
        }
        bytes.resize(362, 0);
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn ata_smart_tables_become_disk_readings() {
        let hex = smart_hex(&[
            (5, 100, 8),
            (9, 99, 0x0001_0000_1234),
            (194, 70, 0x0028_0014_001e),
            (197, 100, 2),
            (233, 97, 0),
            (241, 99, 123_456_789),
        ]);
        let attributes = smart_attributes(&hex);
        assert_eq!(attributes.len(), 6, "empty entries are skipped");
        assert_eq!(attributes[1].raw, 0x0001_0000_1234);
        assert!(smart_attributes("zz").is_empty());
        assert!(smart_attributes("abc").is_empty());

        let mut disk: Device = serde_json::from_value(serde_json::json!({
            "id": "disk:1", "kind": "Disk", "name": "SATA SSD", "at": 0,
            "fields": [{"name": "ATA SMART table", "value": hex}],
            "metrics": [{"id": "temperature", "name": "Temperature", "unit": "C", "value": null}]
        }))
        .unwrap();
        apply_smart(&mut disk);
        let metric = |id: &str| {
            disk.metrics
                .iter()
                .find(|m| m.id == id)
                .and_then(|m| m.value)
        };
        let field = |name: &str| {
            disk.fields
                .iter()
                .find(|f| f.name == name)
                .map(|f| f.value.clone())
        };
        assert_eq!(metric("smart-reallocated"), Some(8.0));
        assert_eq!(metric("smart-pending"), Some(2.0));
        assert_eq!(metric("smart-uncorrectable"), None);
        assert_eq!(metric("smart-life"), Some(97.0));
        assert_eq!(metric("temperature"), Some(30.0));
        assert_eq!(field("SMART power-on hours").as_deref(), Some("4660"));
        assert_eq!(
            field("Total host writes (vendor units)").as_deref(),
            Some("123456789")
        );
        assert_eq!(field("ATA SMART table"), None, "the raw table is not shown");
    }

    #[test]
    fn feature_levels_read_as_major_and_minor() {
        assert_eq!(feature_level_text(0xc200).as_deref(), Some("12_2"));
        assert_eq!(feature_level_text(0xb100).as_deref(), Some("11_1"));
        assert_eq!(feature_level_text(0), None);
        assert_eq!(feature_level_text(0xc201), None);
    }

    #[test]
    fn display_drivers_match_by_pci_ids_and_dates_read_as_iso() {
        assert!(matches_hardware(r"PCI\VEN_10DE&DEV_2684", 0x10de, 0x2684));
        assert!(matches_hardware(
            r"pci\ven_8086&dev_a7a0&subsys_0b1a1028",
            0x8086,
            0xa7a0
        ));
        assert!(!matches_hardware(r"pci\ven_10de&dev_2685", 0x10de, 0x2684));
        assert_eq!(iso_date("11-5-2025").as_deref(), Some("2025-11-05"));
        assert_eq!(iso_date("13-5-2025"), None);
        assert_eq!(iso_date("2025-11-05"), None);
        assert_eq!(iso_date("1-2-3-4"), None);
    }
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
        for d in report.devices.iter().filter(|d| d.kind == "Disk") {
            let bus = d
                .fields
                .iter()
                .find(|f| f.name == "Bus")
                .map(|f| f.value.as_str());
            let smart: Vec<_> = d
                .fields
                .iter()
                .filter(|f| f.name.contains("SMART") || f.name.contains("Start/stop"))
                .map(|f| format!("{}={}", f.name, f.value))
                .collect();
            let m: Vec<_> = d
                .metrics
                .iter()
                .filter(|m| m.id.starts_with("smart"))
                .map(|m| format!("{}={:?}", m.id, m.value))
                .collect();
            println!("{} bus={bus:?} {smart:?} {m:?}", d.name);
        }
        for d in report.devices.iter().filter(|d| d.kind == "GPU") {
            for f in d
                .fields
                .iter()
                .filter(|f| f.name.contains("Driver") || f.name.contains("DirectX"))
            {
                println!("{}: {} = {}", d.name, f.name, f.value);
            }
        }
        assert!(
            !report.devices.is_empty(),
            "No native device collector produced data: {:?}",
            report.warnings
        );
    }
}

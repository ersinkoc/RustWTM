//! One bounded, read-only CIM inventory per window; never on the UI thread.
use eframe::egui::{self, Ui};
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_REPLY: usize = 512 * 1024;
const SCRIPT: &str = r#"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$ErrorActionPreference = 'Stop'
$items = @()
foreach ($spec in @(
 @('CPU','Win32_Processor',@('Name','Manufacturer')),
 @('Motherboard','Win32_BaseBoard',@('Manufacturer','Product','Version')),
 @('RAM','Win32_PhysicalMemory',@('Manufacturer','PartNumber','Capacity','ConfiguredClockSpeed','DeviceLocator')),
 @('Disk','Win32_DiskDrive',@('Index','Manufacturer','Model','Size','InterfaceType')),
 @('GPU','Win32_VideoController',@('Name','AdapterCompatibility','DriverVersion'))
)) {
 try {
  $rows = @(Get-CimInstance -ClassName $spec[1] -Property $spec[2] -OperationTimeoutSec 4 | Select-Object -First 64)
  foreach ($row in $rows) {
   $parts = @()
   foreach ($prop in $spec[2]) {
    $v = [string]$row.$prop
    if ($v.Trim()) { $parts += ($prop + ': ' + $v.Trim()) }
   }
   if ($parts.Count) { $items += @{kind=$spec[0]; fields=$parts} }
  }
 } catch { $items += @{kind=$spec[0]; fields=@()} }
}
ConvertTo-Json -InputObject @($items) -Depth 4 -Compress
"#;

#[derive(Clone, Debug, serde::Deserialize)]
struct Item {
    kind: String,
    fields: Vec<String>,
}
type Inventory = Result<Vec<Item>, String>;
type Shared = Arc<Mutex<Option<Inventory>>>;

fn parse(bytes: &[u8]) -> Result<Vec<Item>, String> {
    if bytes.len() >= MAX_REPLY {
        return Err("Inventory response exceeds its limit".into());
    }
    let items: Vec<Item> = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if items.len() > 320
        || items
            .iter()
            .any(|i| i.fields.len() > 8 || i.fields.iter().any(|s| s.len() > 1024))
    {
        return Err("Inventory response exceeds its limit".into());
    }
    Ok(items)
}

fn collect() -> Inventory {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    let exe = wtm_sys::shell::system_directory()
        .map_err(|e| e.to_string())?
        .join("WindowsPowerShell/v1.0/powershell.exe");
    wtm_sys::security::only_admins_control(&exe).map_err(|e| e.to_string())?;
    let mut child = Command::new(exe)
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().expect("piped stdout");
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let reader = std::thread::Builder::new()
        .name("inventory-output".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout
                .take(MAX_REPLY as u64)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = send.send(result);
        });
    if let Err(error) = reader {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.to_string());
    }
    let start = Instant::now();
    let result = loop {
        match receive.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(bytes)) => break parse(&bytes),
            Ok(Err(e)) => break Err(e.to_string()),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                break Err("Inventory reader stopped".into());
            }
            Err(_) if start.elapsed() >= Duration::from_secs(25) => {
                break Err("Inventory timed out".into());
            }
            Err(_) => {}
        }
    };
    // Also handles a process which keeps running after closing stdout or filling the cap.
    // EOF may precede the normal process exit by a few milliseconds.
    for _ in 0..10 {
        if !matches!(child.try_wait(), Ok(None)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if matches!(child.try_wait(), Ok(None)) {
        let _ = child.kill();
    }
    let status = child.wait();
    let _ = reader.unwrap().join();
    match (result, status) {
        (Ok(items), Ok(status)) if status.success() => Ok(items),
        (Ok(_), _) => Err("Inventory command failed".into()),
        (Err(error), _) => Err(error),
    }
}

fn state(ui: &Ui) -> Shared {
    let id = egui::Id::unique("hardware-inventory");
    if let Some(state) = ui.data(|d| d.get_temp::<Shared>(id)) {
        return state;
    }
    let state: Shared = Arc::new(Mutex::new(None));
    let output = state.clone();
    let context = ui.ctx().clone();
    // Synthetic screen tests never query the developer's hardware.
    #[cfg(not(test))]
    let result = std::thread::Builder::new()
        .name("hardware-inventory".into())
        .spawn(move || {
            let result = collect();
            *output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
            context.request_repaint();
        });
    #[cfg(test)]
    let result: std::io::Result<()> = {
        let _ = (&output, &context, collect as fn() -> Inventory);
        *state.lock().unwrap() = Some(Ok(vec![Item {
            kind: "CPU".into(),
            fields: vec!["Name: Example processor".into()],
        }]));
        Ok(())
    };
    if let Err(error) = result {
        *state.lock().unwrap() = Some(Err(error.to_string()));
    }
    ui.data_mut(|d| d.insert_temp(id, state.clone()));
    state
}

/// Identities are attached to matching live resource cards, never matched by row order.
#[derive(Default)]
pub struct Identities {
    pub cpu: String,
    pub memory: String,
    pub gpu: String,
    pub motherboard: String,
    pub disks: std::collections::BTreeMap<u32, String>,
}

fn field<'a>(item: &'a Item, key: &str) -> Option<&'a str> {
    item.fields
        .iter()
        .filter_map(|f| f.split_once(": "))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.trim())
        .filter(|v| {
            !v.is_empty()
                && ![
                    "unknown",
                    "default string",
                    "to be filled by o.e.m.",
                    "system manufacturer",
                    "system product name",
                    "(Standard disk drives)",
                ]
                .iter()
                .any(|p| v.eq_ignore_ascii_case(p))
        })
}

fn model(item: &Item, key: &str) -> String {
    let name = field(item, key).unwrap_or_default();
    let maker = field(item, "Manufacturer").unwrap_or_default();
    if maker.is_empty() || name.to_lowercase().contains(&maker.to_lowercase()) || key == "Name" {
        name.to_owned()
    } else {
        format!("{maker} {name}").trim().to_owned()
    }
}

fn identities(items: &[Item]) -> Identities {
    let mut out = Identities::default();
    let mut cpu = Vec::new();
    let mut gpu = Vec::new();
    let mut boards = Vec::new();
    let mut modules: Vec<(String, usize)> = Vec::new();
    for item in items {
        match item.kind.as_str() {
            "CPU" => cpu.push(model(item, "Name")),
            "GPU" => gpu.push(model(item, "Name")),
            "Motherboard" => boards.push(model(item, "Product")),
            "Disk" => {
                if let Some(index) = field(item, "Index").and_then(|s| s.parse().ok()) {
                    out.disks.insert(index, model(item, "Model"));
                }
            }
            "RAM" => {
                let mut name = model(item, "PartNumber");
                if let Some(bytes) = field(item, "Capacity")
                    .and_then(|s| s.parse::<u64>().ok())
                    .filter(|v| *v > 0)
                {
                    if !name.is_empty() {
                        name.push_str(" · ");
                    }
                    name.push_str(&wtm_core::format_bytes(bytes as f64));
                }
                if let Some(clock) = field(item, "ConfiguredClockSpeed").filter(|v| *v != "0") {
                    if !name.is_empty() {
                        name.push_str(" · ");
                    }
                    name.push_str(&format!("{clock} MHz"));
                }
                if !name.is_empty() {
                    if let Some((_, count)) = modules.iter_mut().find(|(s, _)| s == &name) {
                        *count += 1;
                    } else {
                        modules.push((name, 1));
                    }
                }
            }
            _ => {}
        }
    }
    fn join(mut names: Vec<String>) -> String {
        names.retain(|s| !s.is_empty());
        let mut unique = Vec::new();
        for name in names {
            if !unique.contains(&name) {
                unique.push(name);
            }
        }
        unique.join("\n")
    }
    out.cpu = join(cpu);
    out.gpu = join(gpu);
    out.motherboard = join(boards);
    out.memory = modules
        .into_iter()
        .map(|(name, count)| {
            if count > 1 {
                format!("{count} × {name}")
            } else {
                name
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    out
}

pub fn resource_identities(ui: &Ui) -> Identities {
    let shared = state(ui);
    let inventory = shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match inventory.as_ref() {
        Some(Ok(items)) => identities(items),
        _ => Identities::default(),
    }
}

pub fn show(ui: &mut Ui) {
    ui.add_space(4.0);
    ui.strong(crate::i18n::tr("Hardware details"));
    let state = state(ui);
    let inventory = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match inventory.as_ref() {
        None => {
            ui.weak(crate::i18n::tr("Reading hardware details…"));
        }
        Some(Err(error)) => {
            ui.weak(crate::i18n::tr("Hardware details unavailable"))
                .on_hover_text(error);
        }
        Some(Ok(items)) => {
            for item in items {
                ui.add_space(5.0);
                ui.vertical(|ui| {
                    ui.strong(crate::i18n::tr(&item.kind));
                    if item.fields.is_empty() {
                        ui.weak(crate::i18n::tr("Hardware details unavailable"));
                    }
                    for field in &item.fields {
                        let (key, value) = field.split_once(": ").unwrap_or(("", field));
                        let value = value.trim();
                        if key == "Index" {
                            continue;
                        }
                        if [
                            "unknown",
                            "default string",
                            "to be filled by o.e.m.",
                            "system manufacturer",
                            "system product name",
                            "(Standard disk drives)",
                        ]
                        .iter()
                        .any(|p| value.eq_ignore_ascii_case(p))
                        {
                            continue;
                        }
                        let label = match key {
                            "Name" | "Product" | "Model" | "PartNumber" => "Model",
                            "Manufacturer" | "AdapterCompatibility" => "Manufacturer",
                            "Capacity" | "Size" => "Capacity",
                            "ConfiguredClockSpeed" => "Memory speed",
                            "DeviceLocator" => "Slot",
                            "DriverVersion" => "Driver version",
                            "InterfaceType" => "Interface",
                            _ => "Version",
                        };
                        let display = if matches!(key, "Capacity" | "Size") {
                            value
                                .parse::<u64>()
                                .ok()
                                .filter(|n| *n > 0)
                                .map(|n| wtm_core::format_bytes(n as f64))
                                .unwrap_or_else(|| "—".into())
                        } else if key == "ConfiguredClockSpeed" {
                            if value == "0" {
                                "—".into()
                            } else {
                                format!("{value} MHz")
                            }
                        } else {
                            value.to_owned()
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.weak(crate::i18n::tr(label));
                            ui.label(display);
                        });
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "reads local CIM inventory; run explicitly for a read-only hardware smoke"]
    fn local_inventory_smoke() {
        let items = collect().expect("bounded local inventory");
        assert!(!items.is_empty());
        for item in items {
            println!("{}: {}", item.kind, item.fields.join(" | "));
        }
    }

    #[test]
    fn resource_cards_group_modules_and_match_disks_by_index() {
        let items = parse(br#"[
            {"kind":"CPU","fields":["Name: Example CPU"]},
            {"kind":"Disk","fields":["Index: 2","Model: Disk Two"]},
            {"kind":"Disk","fields":["Index: 0","Model: Disk Zero"]},
            {"kind":"Disk","fields":["Model: Unmapped disk"]},
            {"kind":"RAM","fields":["Manufacturer: Kingston","PartNumber: KF5","Capacity: 34359738368","ConfiguredClockSpeed: 3600"]},
            {"kind":"RAM","fields":["Manufacturer: Kingston","PartNumber: KF5","Capacity: 34359738368","ConfiguredClockSpeed: 3600"]},
            {"kind":"GPU","fields":["Name: GPU One"]},
            {"kind":"GPU","fields":["Name: GPU Two"]},
            {"kind":"Motherboard","fields":["Manufacturer: Board Maker","Product: Mainboard"]}
        ]"#).unwrap();
        let ids = identities(&items);
        assert_eq!(ids.cpu, "Example CPU");
        assert_eq!(ids.disks.get(&0).unwrap(), "Disk Zero");
        assert_eq!(ids.disks.get(&2).unwrap(), "Disk Two");
        assert_eq!(ids.disks.len(), 2);
        assert_eq!(ids.memory, "2 × Kingston KF5 · 32.0 GiB · 3600 MHz");
        assert_eq!(ids.gpu, "GPU One\nGPU Two");
        assert_eq!(ids.motherboard, "Board Maker Mainboard");
        assert!(identities(&[]).cpu.is_empty());
    }

    #[test]
    fn inventory_parsing_bounds_and_partial_results() {
        assert!(
            parse(br#"[{"kind":"RAM","fields":[]},{"kind":"CPU","fields":["Name: Processor"]}]"#)
                .is_ok()
        );
        assert!(parse(b"invalid").is_err());
        assert!(parse(&vec![b' '; MAX_REPLY]).is_err());
    }
}

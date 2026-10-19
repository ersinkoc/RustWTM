//! Overview tab: the machine at a glance, one-click protection, the busiest
//! apps with quick actions, and what the service did recently.

use std::collections::HashMap;

use eframe::egui::{self, Color32, RichText, Ui};
use wtm_core::control::{EcoQos, ProcessRef};
use wtm_core::format_bytes;
use wtm_ipc::Command;
use wtm_policy::{Config, Preset, set_preset};

use crate::backend::{Data, Mode, UiCommand};
use crate::processes::Intent;

const GOOD: Color32 = Color32::from_rgb(80, 180, 100);
const WARN: Color32 = Color32::from_rgb(230, 140, 40);
const BAD: Color32 = Color32::from_rgb(220, 70, 70);

fn load_color(percent: f32) -> Color32 {
    match percent {
        p if p >= 90.0 => BAD,
        p if p >= 70.0 => WARN,
        _ => GOOD,
    }
}

/// Half the machine is alarming; one full core is worth noticing.
fn app_color(cpu: f32, logical_cpus: u32) -> Color32 {
    if cpu >= 50.0 {
        BAD
    } else if cpu >= 100.0 / logical_cpus.max(1) as f32 {
        WARN
    } else {
        Color32::GRAY
    }
}

fn gauge(ui: &mut Ui, title: &str, percent: f32, detail: String) {
    ui.vertical(|ui| {
        ui.set_width(260.0);
        ui.label(RichText::new(title).strong());
        ui.label(
            RichText::new(format!("{percent:.0}%"))
                .size(30.0)
                .color(load_color(percent)),
        );
        ui.add(
            egui::ProgressBar::new(percent / 100.0)
                .fill(load_color(percent))
                .desired_width(240.0),
        );
        ui.weak(detail);
    });
}

/// One app: all processes with the same image name.
struct App<'a> {
    name: &'a str,
    cpu: f32,
    memory: u64,
    instances: Vec<ProcessRef>,
}

fn busiest_apps(data: &Data, n: usize) -> Vec<App<'_>> {
    let mut apps: HashMap<String, App<'_>> = HashMap::new();
    for p in data.snapshot.iter().flat_map(|s| &s.processes) {
        if p.pid == 0 || wtm_core::protection_reason(p.pid, &p.name).is_some() {
            continue;
        }
        let a = apps.entry(p.name.to_lowercase()).or_insert_with(|| App {
            name: &p.name,
            cpu: 0.0,
            memory: 0,
            instances: Vec::new(),
        });
        a.cpu += p.cpu_percent;
        a.memory += p.private_working_set;
        a.instances.push(ProcessRef::from(p));
    }
    let mut out: Vec<_> = apps.into_values().collect();
    out.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.memory.cmp(&a.memory)));
    out.truncate(n);
    out
}

/// Returns commands for the backend and intents for the app.
pub fn overview(ui: &mut Ui, data: &Data) -> (Vec<UiCommand>, Vec<Intent>) {
    let mut commands = Vec::new();
    let mut intents = Vec::new();

    if let Some(snap) = &data.snapshot {
        let s = &snap.system;
        let mem = if s.memory_total > 0 {
            s.memory_used() as f32 / s.memory_total as f32 * 100.0
        } else {
            0.0
        };
        ui.horizontal(|ui| {
            gauge(
                ui,
                "CPU",
                s.cpu_percent,
                format!(
                    "{} logical processors, {} processes",
                    s.logical_cpus, s.process_count
                ),
            );
            gauge(
                ui,
                "Memory",
                mem,
                format!(
                    "{} of {} in use",
                    format_bytes(s.memory_used() as f64),
                    format_bytes(s.memory_total as f64)
                ),
            );
            gauge(ui, "GPU", s.gpu_percent, "busiest engine".into());
        });
    } else {
        ui.spinner();
    }
    ui.add_space(10.0);
    ui.separator();

    protection(ui, data, &mut commands);
    ui.add_space(10.0);
    ui.separator();

    ui.heading("Busiest apps right now");
    let apps = busiest_apps(data, 8);
    egui::Grid::new("overview_apps")
        .num_columns(5)
        .striped(true)
        .show(ui, |ui| {
            for h in ["App", "CPU", "Memory", "Processes", ""] {
                ui.strong(h);
            }
            ui.end_row();
            for a in &apps {
                ui.horizontal(|ui| {
                    let first = &a.instances[0];
                    let icon = data
                        .image_paths
                        .get(&(first.pid, first.create_time))
                        .and_then(|path| Some((path.as_str(), data.icons.get(path)?.as_deref()?)));
                    crate::processes::app_icon(ui, icon);
                    ui.label(a.name);
                });
                let cpus = data.snapshot.as_ref().map_or(1, |s| s.system.logical_cpus);
                ui.label(RichText::new(format!("{:.1}%", a.cpu)).color(app_color(a.cpu, cpus)));
                ui.label(format_bytes(a.memory as f64));
                ui.label(a.instances.len().to_string());
                ui.horizontal(|ui| {
                    if ui
                        .button("Efficiency mode")
                        .on_hover_text("Run every process of this app in efficiency mode (EcoQoS).")
                        .clicked()
                    {
                        for t in &a.instances {
                            intents.push(Intent::Command(t.clone(), Command::EcoQos(EcoQos::On)));
                        }
                    }
                    if ui
                        .button("Free memory")
                        .on_hover_text(
                            "Move this app's memory to the standby list; it comes back as needed.",
                        )
                        .clicked()
                    {
                        for t in &a.instances {
                            intents.push(Intent::Command(t.clone(), Command::TrimMemory));
                        }
                    }
                    if ui
                        .button("Limit…")
                        .on_hover_text("Give this app a permanent CPU / memory budget.")
                        .clicked()
                    {
                        intents.push(Intent::LimitApp(a.name.to_owned()));
                    }
                    if ui.button("Details").clicked() {
                        intents.push(Intent::Reveal(a.instances[0].clone()));
                    }
                });
                ui.end_row();
            }
        });
    ui.add_space(10.0);
    ui.separator();

    ui.heading("Recent automatic actions");
    let recent: Vec<_> = data
        .events
        .iter()
        .rev()
        .filter(|e| e.notify)
        .take(8)
        .collect();
    if recent.is_empty() {
        ui.weak("Nothing yet. RustWTM steps in when the machine is overloaded or a rule matches.");
    }
    for e in recent {
        let when = clock(e.unix_ms);
        ui.label(format!("{when}  {}", e.message));
    }
    (commands, intents)
}

/// `HH:MM:SS` in local time.
fn clock(unix_ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(unix_ms as i64)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}
fn protection(ui: &mut Ui, data: &Data, commands: &mut Vec<UiCommand>) {
    ui.heading("Automatic protection");
    if !matches!(data.mode, Mode::Service { .. }) {
        let error = data.service_error.as_deref().unwrap_or_default();
        if error.contains("protocol") {
            ui.colored_label(
                WARN,
                format!(
                    "The installed RustWTM service is a different version ({error}), so nothing is \
                     limited automatically. Update it with the installer of version {}.",
                    env!("CARGO_PKG_VERSION")
                ),
            );
            ui.hyperlink_to(
                "Download the latest release",
                format!("{}/releases/latest", crate::about::REPO),
            );
        } else {
            ui.colored_label(
                WARN,
                "The RustWTM service is not running, so nothing is limited automatically.",
            );
            ui.label("Install and start it from an elevated prompt:");
            ui.code("wtm-service install\nwtm-service start");
        }
        return;
    }
    let admin = matches!(data.mode, Mode::Service { admin: true, .. });
    let Some(Ok((_, text))) = &data.config else {
        ui.spinner();
        return;
    };
    let Ok(config) = Config::from_toml(text) else {
        ui.colored_label(BAD, "The configuration is invalid; fix it in Settings.");
        return;
    };

    ui.horizontal(|ui| {
        if config.dry_run {
            ui.label(
                RichText::new("● Monitoring only — nothing is limited")
                    .size(18.0)
                    .color(WARN),
            );
            let turn_on = ui.add_enabled(
                admin,
                egui::Button::new(RichText::new("Turn protection on").strong()),
            );
            if turn_on.clicked()
                && let Some(new) = crate::settings::set_dry_run(text, false)
            {
                commands.push(UiCommand::SaveConfig(new));
            }
        } else {
            ui.label(RichText::new("● Protection is on").size(18.0).color(GOOD));
            if ui
                .add_enabled(admin, egui::Button::new("Switch to monitoring only"))
                .clicked()
                && let Some(new) = crate::settings::set_dry_run(text, true)
            {
                commands.push(UiCommand::SaveConfig(new));
            }
        }
        if !admin {
            ui.weak("Changing it needs administrator rights.");
            if ui.small_button("🛡 Restart as administrator").clicked() {
                commands.push(UiCommand::RestartElevated);
            }
        }
    });

    let current = Preset::of(&config.governor);
    ui.horizontal(|ui| {
        ui.label("CPU protection level:");
        for p in Preset::ALL {
            let button = egui::Button::selectable(current == Some(p), p.label());
            if ui
                .add_enabled(admin, button)
                .on_hover_text(p.description())
                .clicked()
                && current != Some(p)
                && let Ok(new) = set_preset(text, p)
            {
                commands.push(UiCommand::SaveConfig(new));
            }
        }
        if current.is_none() {
            ui.weak("Custom (edited in Settings)");
        }
    });
    if let Some(p) = current {
        ui.weak(p.description());
    }

    if let Some(status) = &data.status {
        let mut facts = vec![format!(
            "{} processes limited right now",
            status.managed.len()
        )];
        if !config.limits.is_empty() {
            let processes: usize = status.app_limits.iter().map(|g| g.processes).sum();
            facts.push(format!(
                "{} app limits covering {processes} processes",
                config.limits.len()
            ));
        }
        if config.governor.protect_foreground
            && let Some(app) = &status.foreground
        {
            facts.push(format!("{app} is in use and never throttled"));
        }
        if config.memory_guard.enabled {
            facts.push(format!(
                "memory guard above {:.0}% RAM",
                config.memory_guard.high_percent
            ));
        }
        if !config.block.is_empty() {
            facts.push(format!("{} programs blocked", config.block.len()));
        }
        ui.label(facts.join("  ·  "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_and_times() {
        assert_eq!(load_color(95.0), BAD);
        assert_eq!(load_color(75.0), WARN);
        assert_eq!(load_color(10.0), GOOD);
        assert_eq!(clock(0).len(), 8);
        assert_eq!(app_color(60.0, 8), BAD);
        assert_eq!(app_color(20.0, 8), WARN);
        assert_eq!(app_color(5.0, 8), Color32::GRAY);
    }
}

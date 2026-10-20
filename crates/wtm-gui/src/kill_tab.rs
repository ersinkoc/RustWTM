//! Kill list tab and the panic button: processes that may be ended, either
//! automatically when they keep eating CPU or all at once on demand.

use eframe::egui::{self, Color32, RichText, Ui};
use wtm_core::control::ProcessRef;
use wtm_ipc::Command;
use wtm_policy::{KillEntry, remove_kill_entry, set_kill_entry};

use crate::backend::{Data, Mode, Panel, UiCommand};
use crate::processes::Intent;

const BAD: Color32 = Color32::from_rgb(220, 70, 70);
const WARN: Color32 = Color32::from_rgb(230, 140, 40);

/// A process the panic button would end.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub process: ProcessRef,
    pub cpu: f32,
    /// The kill-list entry that matched, as a label.
    pub entry: String,
    /// Started by a matching process.
    pub child: bool,
}

/// What the panic button ends right now.
pub fn targets(data: &Data) -> Vec<Target> {
    let Some(snap) = &data.snapshot else {
        return Vec::new();
    };
    let lines = &data.command_lines;
    wtm_policy::panic_targets(&data.kill_list, &snap.processes, &mut |p| {
        lines.get(&(p.pid, p.create_time)).cloned()
    })
    .into_iter()
    .map(|t| Target {
        process: ProcessRef::from(t.process),
        cpu: t.process.cpu_percent,
        entry: data.kill_list[t.entry].label(),
        child: t.child,
    })
    .collect()
}

/// The panic confirmation: lists what will be ended. One click ends it all.
pub fn panic_dialog(ctx: &egui::Context, data: &Data, open: &mut bool, intents: &mut Vec<Intent>) {
    if !*open {
        return;
    }
    let targets = targets(data);
    let modal = egui::Modal::new(egui::Id::unique("panic")).show(ctx, |ui| {
        ui.set_width(620.0);
        ui.label(RichText::new("🚨 Panic: end the kill list").size(22.0).color(BAD));
        if targets.is_empty() {
            ui.label("Nothing on the kill list is running right now.");
            if data.kill_list.is_empty() {
                ui.weak("Add programs in the Kill list tab, or right-click a process → Add to kill list….");
            }
            if ui.button("Close").clicked() {
                *open = false;
            }
            return;
        }
        ui.label(format!(
            "These {} processes will be ended immediately. Unsaved work in them is lost.",
            targets.len()
        ));
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            egui::Grid::new("panic_targets")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    for h in ["Process", "PID", "CPU", "Kill-list entry"] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for t in &targets {
                        let name = if t.child {
                            format!("  └ {}", t.process.name)
                        } else {
                            t.process.name.clone()
                        };
                        ui.label(name);
                        ui.label(t.process.pid.to_string());
                        ui.label(format!("{:.1}%", t.cpu));
                        ui.weak(&t.entry);
                        ui.end_row();
                    }
                });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .button(
                    RichText::new(format!("End {} processes now", targets.len()))
                        .strong()
                        .color(BAD),
                )
                .clicked()
            {
                intents.extend(
                    targets
                        .iter()
                        .map(|t| Intent::Command(t.process.clone(), Command::Kill)),
                );
                *open = false;
            }
            if ui.button("Cancel").clicked() {
                *open = false;
            }
        });
    });
    if modal.should_close() {
        *open = false;
    }
}

/// The add / change form.
#[derive(Debug, Clone, PartialEq)]
pub struct KillView {
    name: String,
    cmdline: String,
    automatic: bool,
    cpu: f32,
    seconds: u64,
    busy_pc: bool,
    system_cpu: f32,
    tree: bool,
    /// Index of the entry being changed.
    editing: Option<usize>,
    error: Option<String>,
}

impl Default for KillView {
    fn default() -> Self {
        Self {
            name: String::new(),
            cmdline: String::new(),
            automatic: true,
            cpu: 50.0,
            seconds: 60,
            busy_pc: false,
            system_cpu: 85.0,
            tree: true,
            editing: None,
            error: None,
        }
    }
}

impl KillView {
    /// A new entry for `name` (from a process's context menu).
    pub fn prefill(&mut self, name: &str, cmdline: &str) {
        *self = Self {
            name: name.to_owned(),
            cmdline: cmdline.to_owned(),
            ..Self::default()
        };
    }

    fn edit(&mut self, index: usize, e: &KillEntry) {
        *self = Self {
            name: e.name.clone(),
            cmdline: e.cmdline.clone().unwrap_or_default(),
            automatic: e.cpu_above.is_some(),
            cpu: e.cpu_above.unwrap_or(50.0),
            seconds: e.for_secs,
            busy_pc: e.system_cpu_above.is_some(),
            system_cpu: e.system_cpu_above.unwrap_or(85.0),
            tree: e.tree,
            editing: Some(index),
            error: None,
        };
    }

    fn entry(&self) -> KillEntry {
        let cmdline = self.cmdline.trim();
        KillEntry {
            name: self.name.trim().to_owned(),
            cmdline: (!cmdline.is_empty()).then(|| cmdline.to_owned()),
            cpu_above: self.automatic.then_some(self.cpu),
            for_secs: self.seconds,
            system_cpu_above: (self.automatic && self.busy_pc).then_some(self.system_cpu),
            tree: self.tree,
        }
    }
}

pub fn kill_tab(ui: &mut Ui, data: &Data, view: &mut KillView, panic: &mut bool) -> Vec<UiCommand> {
    let mut commands = Vec::new();
    ui.label(
        "Programs that may be ended. With a CPU threshold the service ends them by itself \
         once they keep using that much and do not recover; the panic button ends everything \
         listed here at once.",
    );
    ui.add_space(6.0);
    let running = targets(data);
    let panic_label = format!("🚨 Panic: end {} processes now", running.len());
    if ui
        .add_enabled(
            !running.is_empty(),
            egui::Button::new(RichText::new(panic_label).strong().color(BAD)),
        )
        .on_hover_text("Shows what will be ended first (Ctrl+Shift+K from anywhere)")
        .clicked()
    {
        *panic = true;
    }
    ui.add_space(8.0);

    let text = match &data.config {
        None => {
            ui.spinner();
            return commands;
        }
        Some(Err(e)) => {
            ui.colored_label(BAD, e);
            if ui.button("Retry").clicked() {
                commands.push(UiCommand::Load(Panel::Config));
            }
            return commands;
        }
        Some(Ok((_, text))) => text,
    };
    let admin = matches!(data.mode, Mode::Service { admin: true, .. });
    if data.status.as_ref().is_some_and(|s| s.dry_run) {
        ui.colored_label(
            WARN,
            "Dry-run mode: automatic ends are only logged. The panic button always works.",
        );
    }
    if !admin {
        ui.horizontal(|ui| {
            ui.weak("Run RustWTM as administrator to change the kill list.");
            if !data.elevated && ui.small_button("🛡 Restart as administrator").clicked() {
                commands.push(UiCommand::RestartElevated);
            }
        });
    }

    if !data.kill_list.is_empty() {
        egui::Grid::new("kill_list")
            .num_columns(5)
            .striped(true)
            .show(ui, |ui| {
                for h in ["Program", "Ended", "Its children", "Running now", ""] {
                    ui.strong(h);
                }
                ui.end_row();
                for (i, e) in data.kill_list.iter().enumerate() {
                    ui.label(e.label());
                    ui.label(e.describe());
                    ui.label(if e.tree { "too" } else { "no" });
                    let label = e.label();
                    let n = running.iter().filter(|t| t.entry == label).count();
                    ui.label(n.to_string());
                    ui.horizontal(|ui| {
                        if ui.add_enabled(admin, egui::Button::new("Edit")).clicked() {
                            view.edit(i, e);
                        }
                        if ui.add_enabled(admin, egui::Button::new("Remove")).clicked() {
                            match remove_kill_entry(text, i) {
                                Ok(new) => commands.push(UiCommand::SaveConfig(new)),
                                Err(e) => view.error = Some(e),
                            }
                        }
                    });
                    ui.end_row();
                }
            });
    } else {
        ui.weak("The kill list is empty. Add a program below, or right-click a process → Add to kill list…");
    }

    ui.add_space(12.0);
    ui.heading(if view.editing.is_some() {
        "Change entry"
    } else {
        "Add to the kill list"
    });
    egui::Grid::new("kill_form").num_columns(2).show(ui, |ui| {
        ui.label("Program");
        ui.add(
            egui::TextEdit::singleline(&mut view.name).hint_text("e.g. node.exe or python*.exe"),
        );
        ui.end_row();
        ui.label("Command line contains");
        ui.add(
            egui::TextEdit::singleline(&mut view.cmdline)
                .hint_text("optional, e.g. vitest (only those node.exe)"),
        );
        ui.end_row();
        ui.label("");
        ui.horizontal(|ui| {
            ui.checkbox(&mut view.automatic, "End it automatically at");
            ui.add_enabled(
                view.automatic,
                egui::DragValue::new(&mut view.cpu)
                    .range(1.0..=100.0)
                    .suffix("% CPU"),
            );
            ui.label("for");
            ui.add_enabled(
                view.automatic,
                egui::DragValue::new(&mut view.seconds)
                    .range(1..=3600)
                    .suffix(" s"),
            );
        });
        ui.end_row();
        ui.label("");
        // A share of the whole machine: one busy core is little on big PCs.
        let cpus = data
            .snapshot
            .as_ref()
            .map_or(1, |s| s.system.logical_cpus.max(1));
        ui.weak(format!(
            "{:.0}% of this PC = {:.1} of its {cpus} logical processors (one fully busy core is {:.1}%)",
            view.cpu,
            view.cpu / 100.0 * cpus as f32,
            100.0 / cpus as f32
        ));
        ui.end_row();
        ui.label("");
        ui.add_enabled_ui(view.automatic, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut view.busy_pc, "only while the PC is above");
                ui.add_enabled(
                    view.busy_pc,
                    egui::DragValue::new(&mut view.system_cpu)
                        .range(1.0..=100.0)
                        .suffix("%"),
                );
            });
        });
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut view.tree, "Also end the processes it started");
        ui.end_row();
    });

    let entry = view.entry();
    if !entry.name.is_empty()
        && let Some(snap) = &data.snapshot
    {
        let lines = &data.command_lines;
        let matching: Vec<String> = snap
            .processes
            .iter()
            .filter(|p| entry.matches(p, &mut |p| lines.get(&(p.pid, p.create_time)).cloned()))
            .map(|p| format!("{} ({})", p.name, p.pid))
            .collect();
        ui.weak(if matching.is_empty() {
            "Matches no running process right now.".to_owned()
        } else {
            format!(
                "Matches {} running now: {}",
                matching.len(),
                matching.join(", ")
            )
        });
    }
    ui.horizontal(|ui| {
        let label = if view.editing.is_some() {
            "Save entry"
        } else {
            "Add entry"
        };
        if ui.add_enabled(admin, egui::Button::new(label)).clicked() {
            match set_kill_entry(text, &entry, view.editing) {
                Ok(new) => {
                    commands.push(UiCommand::SaveConfig(new));
                    *view = KillView::default();
                }
                Err(e) => view.error = Some(e),
            }
        }
        if view.editing.is_some() && ui.button("Cancel").clicked() {
            *view = KillView::default();
        }
    });
    if let Some(e) = &view.error {
        ui.colored_label(BAD, e);
    }
    commands
}

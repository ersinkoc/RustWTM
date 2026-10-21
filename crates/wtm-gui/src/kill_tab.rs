//! Kill list tab and the panic button: processes that may be ended, either
//! automatically when they keep eating CPU or all at once on demand.

use eframe::egui::{self, RichText, Ui};
use wtm_core::control::ProcessRef;
use wtm_ipc::Command;
use wtm_policy::{KillEntry, remove_kill_entry, set_kill_entry};

use crate::backend::{Data, Mode, Panel, UiCommand};
use crate::processes::Intent;

use crate::theme::{BAD, WARN, right};

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
        ui.label(RichText::new(crate::i18n::tr("🚨 Panic: end the kill list")).size(22.0).color(crate::theme::text_color(ui, BAD)));
        if targets.is_empty() {
            ui.label(crate::i18n::tr("Nothing on the kill list is running right now."));
            if data.kill_list.is_empty() {
                ui.weak(crate::i18n::tr("Add programs in the Kill list tab, or right-click a process → Add to kill list…."));
            }
            if ui.button(crate::i18n::tr("Close")).clicked() {
                *open = false;
            }
            return;
        }
        ui.label({ let __wtm_i18n_0 = &(targets.len()); crate::i18n::format("These {0} processes will be ended immediately. Unsaved work in them is lost.", &[format!("{}", __wtm_i18n_0)]) });
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            egui::Grid::new("panic_targets")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    ui.strong(crate::i18n::tr("Process"));
                    for h in ["PID", "CPU"] {
                        right(ui, RichText::new(h).strong());
                    }
                    ui.strong(crate::i18n::tr("Kill-list entry"));
                    ui.end_row();
                    for t in &targets {
                        let name = if t.child {
                            format!("  └ {}", t.process.name)
                        } else {
                            t.process.name.clone()
                        };
                        ui.label(name);
                        right(ui, t.process.pid.to_string());
                        right(ui, format!("{:.1}%", t.cpu));
                        ui.weak(&t.entry);
                        ui.end_row();
                    }
                });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .button(
                    RichText::new({ let __wtm_i18n_0 = &(targets.len()); crate::i18n::format("End {0} processes now", &[format!("{}", __wtm_i18n_0)]) })
                        .strong()
                        .color(crate::theme::text_color(ui, BAD)),
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
            if ui.button(crate::i18n::tr("Cancel")).clicked() {
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
    ui.label(crate::i18n::tr(
        "Programs that may be ended. With a CPU threshold the service ends them by itself \
         once they keep using that much and do not recover; the panic button ends everything \
         listed here at once.",
    ));
    ui.add_space(6.0);
    let running = targets(data);
    let panic_label = {
        let __wtm_i18n_0 = &(running.len());
        crate::i18n::format(
            "🚨 Panic: end {0} processes now",
            &[format!("{}", __wtm_i18n_0)],
        )
    };
    if ui
        .add_enabled(
            !running.is_empty(),
            egui::Button::new(
                RichText::new(panic_label)
                    .strong()
                    .color(crate::theme::text_color(ui, BAD)),
            ),
        )
        .on_hover_text(crate::i18n::tr(
            "Shows what will be ended first (Ctrl+Shift+K from anywhere)",
        ))
        .clicked()
    {
        *panic = true;
    }
    ui.add_space(8.0);

    let text = match &data.config {
        None => {
            crate::theme::loading(ui, crate::i18n::tr("Loading the service settings…"));
            return commands;
        }
        Some(Err(e)) => {
            crate::theme::colored(ui, BAD, e);
            if ui.button(crate::i18n::tr("Retry")).clicked() {
                commands.push(UiCommand::Load(Panel::Config));
            }
            return commands;
        }
        Some(Ok((_, text))) => text,
    };
    let admin = matches!(data.mode, Mode::Service { admin: true, .. });
    if data.status.as_ref().is_some_and(|s| s.dry_run) {
        crate::theme::colored(
            ui,
            WARN,
            crate::i18n::tr(
                "Dry-run mode: automatic ends are only logged. The panic button always works.",
            ),
        );
    }
    if !admin {
        ui.horizontal(|ui| {
            ui.weak(crate::i18n::tr(
                "Run RustWTM as administrator to change the kill list.",
            ));
            if !data.elevated
                && ui
                    .small_button(crate::i18n::tr("Restart as administrator"))
                    .clicked()
            {
                commands.push(UiCommand::RestartElevated);
            }
        });
    }

    if !data.kill_list.is_empty() {
        crate::theme::panel(ui, |ui| {
            egui::Grid::new("kill_list")
                .num_columns(5)
                .striped(true)
                .show(ui, |ui| {
                    for h in [
                        crate::i18n::tr("Program"),
                        crate::i18n::tr("Ended"),
                        crate::i18n::tr("Its children"),
                    ] {
                        ui.strong(h);
                    }
                    right(ui, RichText::new(crate::i18n::tr("Running now")).strong());
                    ui.end_row();
                    for (i, e) in data.kill_list.iter().enumerate() {
                        ui.label(e.label());
                        ui.label(e.describe());
                        ui.label(crate::i18n::tr(if e.tree { "yes" } else { "no" }));
                        let label = e.label();
                        let n = running.iter().filter(|t| t.entry == label).count();
                        right(ui, n.to_string());
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(admin, egui::Button::new(crate::i18n::tr("Edit")))
                                .clicked()
                            {
                                view.edit(i, e);
                            }
                            if ui
                                .add_enabled(admin, egui::Button::new(crate::i18n::tr("Remove")))
                                .clicked()
                            {
                                match remove_kill_entry(text, i) {
                                    Ok(new) => commands.push(UiCommand::SaveConfig(new)),
                                    Err(e) => view.error = Some(e),
                                }
                            }
                            crate::theme::fill_row(ui);
                        });
                        ui.end_row();
                    }
                });
        });
    } else {
        ui.weak(crate::i18n::tr("The kill list is empty. Add a program below, or right-click a process → Add to kill list…"));
    }

    ui.add_space(12.0);
    crate::theme::section(
        ui,
        if view.editing.is_some() {
            crate::i18n::tr("Change entry")
        } else {
            crate::i18n::tr("Add to the kill list")
        },
    );
    egui::Grid::new("kill_form").num_columns(2).show(ui, |ui| {
        ui.label(crate::i18n::tr("Program"));
        ui.add(
            egui::TextEdit::singleline(&mut view.name)
                .hint_text(crate::i18n::tr("e.g. node.exe or python*.exe")),
        );
        ui.end_row();
        ui.label(crate::i18n::tr("Command line contains"));
        ui.add(
            egui::TextEdit::singleline(&mut view.cmdline).hint_text(crate::i18n::tr(
                "optional, e.g. vitest (only those node.exe)",
            )),
        );
        ui.end_row();
        ui.label("");
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut view.automatic,
                crate::i18n::tr("End it automatically at"),
            );
            ui.add_enabled(
                view.automatic,
                egui::DragValue::new(&mut view.cpu)
                    .range(1.0..=100.0)
                    .suffix(crate::i18n::tr("% CPU")),
            );
            ui.label(crate::i18n::tr("for"));
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
        ui.weak({
            let __wtm_i18n_0 = &(view.cpu);
            let __wtm_i18n_1 = &(view.cpu / 100.0 * cpus as f32);
            let __wtm_i18n_2 = &(100.0 / cpus as f32);
            crate::i18n::format(
                "{0}% of this PC = {1} of its {2} logical processors (one fully busy core is {3}%)",
                &[
                    format!("{:.0}", __wtm_i18n_0),
                    format!("{:.1}", __wtm_i18n_1),
                    format!("{}", cpus),
                    format!("{:.1}", __wtm_i18n_2),
                ],
            )
        });
        ui.end_row();
        ui.label("");
        ui.add_enabled_ui(view.automatic, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut view.busy_pc,
                    crate::i18n::tr("only while the PC is above"),
                );
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
        ui.checkbox(
            &mut view.tree,
            crate::i18n::tr("Also end the processes it started"),
        );
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
            crate::i18n::tr("Matches no running process right now.").to_owned()
        } else {
            {
                let __wtm_i18n_0 = &(matching.len());
                let __wtm_i18n_1 = &(matching.join(", "));
                crate::i18n::format(
                    "Matches {0} running now: {1}",
                    &[format!("{}", __wtm_i18n_0), __wtm_i18n_1.to_string()],
                )
            }
        });
    }
    ui.horizontal(|ui| {
        let label = if view.editing.is_some() {
            crate::i18n::tr("Save entry")
        } else {
            crate::i18n::tr("Add entry")
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
        if view.editing.is_some() && ui.button(crate::i18n::tr("Cancel")).clicked() {
            *view = KillView::default();
        }
    });
    if let Some(e) = &view.error {
        crate::theme::colored(ui, BAD, e);
    }
    commands
}

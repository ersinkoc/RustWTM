//! Settings tab: the service's main settings as a form, and its
//! `config.toml` as text, with live validation.

use eframe::egui::{self, Color32, RichText, Ui};

use crate::backend::{Data, UiCommand};

#[derive(Debug, Default)]
pub struct ConfigView {
    /// Text being edited.
    pub text: String,
    /// The service's text the edit started from.
    original: Option<String>,
    /// The last validated text and its result, so parsing runs only when
    /// the text changes.
    check: Option<(String, Result<wtm_policy::Config, String>)>,
}

impl ConfigView {
    fn dirty(&self) -> bool {
        self.original.as_deref() != Some(self.text.as_str())
    }

    /// Takes the service's text unless the user has unsaved edits.
    fn sync(&mut self, service_text: &str) {
        if self.original.as_deref() == Some(service_text) {
            return;
        }
        if !self.dirty() || self.original.is_none() {
            self.text = service_text.to_owned();
        }
        self.original = Some(service_text.to_owned());
    }

    fn validate(&mut self) -> Result<wtm_policy::Config, String> {
        match &self.check {
            Some((text, result)) if *text == self.text => result.clone(),
            _ => {
                let result = wtm_policy::Config::from_toml(&self.text).map_err(|e| e.to_string());
                self.check = Some((self.text.clone(), result.clone()));
                result
            }
        }
    }
}

/// Rewrites the top-level `dry_run = ...` line; `None` when there is none.
pub fn set_dry_run(text: &str, dry_run: bool) -> Option<String> {
    let mut found = false;
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            let key = line.split('=').next().unwrap_or("").trim();
            if !found && key == "dry_run" {
                found = true;
                let comment = line.find('#').map_or("", |i| &line[i..]);
                let value = format!("dry_run = {dry_run}");
                if comment.is_empty() {
                    value
                } else {
                    format!("{value}  {comment}")
                }
            } else {
                line.to_owned()
            }
        })
        .collect();
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    found.then_some(out)
}

/// Returns the commands to send (reload, save).
pub fn settings(ui: &mut Ui, data: &Data, view: &mut ConfigView) -> Vec<UiCommand> {
    let mut commands = Vec::new();
    let reload = |commands: &mut Vec<UiCommand>| {
        commands.push(UiCommand::Load(crate::backend::Panel::Config));
    };
    let path = match &data.config {
        None => {
            ui.spinner();
            return commands;
        }
        Some(Err(e)) => {
            ui.colored_label(Color32::RED, e);
            if ui.button("Retry").clicked() {
                reload(&mut commands);
            }
            return commands;
        }
        Some(Ok((path, text))) => {
            view.sync(text);
            path
        }
    };
    let admin = matches!(data.mode, crate::backend::Mode::Service { admin: true, .. });
    let dirty = view.dirty();
    let valid = view.validate();

    ui.horizontal(|ui| {
        ui.label(RichText::new(path).monospace());
        if dirty {
            ui.colored_label(Color32::from_rgb(230, 140, 40), "● unsaved changes");
        }
    });
    ui.horizontal(|ui| {
        let save = ui.add_enabled(admin && dirty && valid.is_ok(), egui::Button::new("Save"));
        if save.clicked() {
            commands.push(UiCommand::SaveConfig(view.text.clone()));
        }
        if ui
            .add_enabled(dirty, egui::Button::new("Discard changes"))
            .clicked()
        {
            view.original = None;
            reload(&mut commands);
        }
        if !admin {
            ui.weak("Run RustWTM as administrator to save.");
        }
    });
    match &valid {
        Ok(_) => ui.colored_label(Color32::from_rgb(80, 180, 100), "✔ Valid configuration"),
        Err(e) => ui.colored_label(Color32::from_rgb(220, 70, 70), format!("✖ {e}")),
    };
    ui.add_space(4.0);
    if let Ok(config) = &valid {
        crate::theme::card(
            ui,
            RichText::new("Service settings").size(16.0).strong(),
            |ui| {
                if let Some(new) = form(ui, config, &view.text) {
                    view.text = new;
                }
            },
        );
        ui.add_space(4.0);
    }
    egui::CollapsingHeader::new("Advanced: edit config.toml")
        .id_salt("config_text")
        .default_open(valid.is_err())
        .show(ui, |ui| {
            ui.weak(
                "Rules, app limits, the kill list and every other option. See docs/configuration.md.",
            );
            ui.add(
                egui::TextEdit::multiline(&mut view.text)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(30),
            );
        });
    commands
}

/// The settings most people change, as controls; returns the edited text.
fn form(ui: &mut Ui, c: &wtm_policy::Config, text: &str) -> Option<String> {
    let mut out: Option<String> = None;
    let mut set = |table: &str, key: &str, value: toml_edit_value::V| {
        let base = out.as_deref().unwrap_or(text);
        if let Ok(new) = value.write(base, table, key) {
            out = Some(new);
        }
    };
    let g = &c.governor;
    egui::Grid::new("service_form")
        .num_columns(2)
        .spacing([16.0, 10.0])
        .show(ui, |ui| {
            ui.strong("Mode");
            ui.horizontal(|ui| {
                if ui.selectable_label(!c.dry_run, "Enforcing").clicked() && c.dry_run {
                    set("", "dry_run", toml_edit_value::V::Bool(false));
                }
                if ui
                    .selectable_label(c.dry_run, "Monitoring only")
                    .on_hover_text("Logs what it would do without touching any process")
                    .clicked()
                    && !c.dry_run
                {
                    set("", "dry_run", toml_edit_value::V::Bool(true));
                }
            });
            ui.end_row();

            ui.strong("CPU governor");
            ui.vertical(|ui| {
                let mut on = g.enabled;
                if ui
                    .checkbox(
                        &mut on,
                        "Slow down the busiest apps when the CPU is overloaded",
                    )
                    .changed()
                {
                    set("governor", "enabled", toml_edit_value::V::Bool(on));
                }
                ui.add_enabled_ui(g.enabled, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Act when the CPU stays above");
                        let mut high = g.high_cpu_percent;
                        if ui
                            .add(
                                egui::DragValue::new(&mut high)
                                    .range(50.0..=100.0)
                                    .suffix("%"),
                            )
                            .changed()
                        {
                            set(
                                "governor",
                                "high_cpu_percent",
                                toml_edit_value::V::Float(high),
                            );
                        }
                        ui.label("for");
                        let mut secs = g.sustain_secs;
                        if ui
                            .add(egui::DragValue::new(&mut secs).range(1..=300).suffix(" s"))
                            .changed()
                        {
                            set("governor", "sustain_secs", toml_edit_value::V::Int(secs));
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Undo once it stays below");
                        let mut low = g.recover_cpu_percent;
                        if ui
                            .add(
                                egui::DragValue::new(&mut low)
                                    .range(10.0..=99.0)
                                    .suffix("%"),
                            )
                            .changed()
                        {
                            set(
                                "governor",
                                "recover_cpu_percent",
                                toml_edit_value::V::Float(low),
                            );
                        }
                        ui.label("for");
                        let mut secs = g.recover_secs;
                        if ui
                            .add(egui::DragValue::new(&mut secs).range(1..=600).suffix(" s"))
                            .changed()
                        {
                            set("governor", "recover_secs", toml_edit_value::V::Int(secs));
                        }
                    });
                    ui.horizontal(|ui| {
                        let label = ui.label("Slow down up to");
                        let mut n = g.max_offenders as u64;
                        if ui
                            .add(egui::DragValue::new(&mut n).range(1..=20))
                            .labelled_by(label.id)
                            .changed()
                        {
                            set("governor", "max_offenders", toml_edit_value::V::Int(n));
                        }
                        ui.label("apps at a time");
                    });
                });
            });
            ui.end_row();

            let m = &c.memory_guard;
            ui.strong("Memory guard");
            ui.vertical(|ui| {
                let mut on = m.enabled;
                if ui
                    .checkbox(&mut on, "Free memory of the largest apps when RAM runs out")
                    .on_hover_text("Moves their pages to the standby list; nothing is lost")
                    .changed()
                {
                    set("memory_guard", "enabled", toml_edit_value::V::Bool(on));
                }
                ui.add_enabled_ui(m.enabled, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("When memory use stays above");
                        let mut high = m.high_percent;
                        if ui
                            .add(
                                egui::DragValue::new(&mut high)
                                    .range(50.0..=99.0)
                                    .suffix("%"),
                            )
                            .changed()
                        {
                            set(
                                "memory_guard",
                                "high_percent",
                                toml_edit_value::V::Float(high),
                            );
                        }
                        ui.label("for");
                        let mut secs = m.sustain_secs;
                        if ui
                            .add(egui::DragValue::new(&mut secs).range(1..=600).suffix(" s"))
                            .changed()
                        {
                            set(
                                "memory_guard",
                                "sustain_secs",
                                toml_edit_value::V::Int(secs),
                            );
                        }
                    });
                });
            });
            ui.end_row();

            let s = &c.swarm;
            ui.strong("Swarm warning");
            ui.horizontal(|ui| {
                let mut on = s.enabled;
                let check = ui.checkbox(&mut on, "Warn when one program runs");
                if check.changed() {
                    set("swarm", "enabled", toml_edit_value::V::Bool(on));
                }
                let mut n = s.min_count as u64;
                if ui
                    .add_enabled(s.enabled, egui::DragValue::new(&mut n).range(10..=10_000))
                    .labelled_by(check.id)
                    .changed()
                {
                    set("swarm", "min_count", toml_edit_value::V::Int(n));
                }
                ui.label("copies or more");
            });
            ui.end_row();

            ui.strong("History");
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Keep per-app usage for");
                    let mut days = u64::from(c.history_days);
                    if ui
                        .add(
                            egui::DragValue::new(&mut days)
                                .range(1..=365)
                                .suffix(" days"),
                        )
                        .changed()
                    {
                        set("", "history_days", toml_edit_value::V::Int(days));
                    }
                });
                let mut on = c.insights.enabled;
                if ui
                    .checkbox(
                        &mut on,
                        "Flag apps with constant CPU use or steadily growing memory",
                    )
                    .changed()
                {
                    set("insights", "enabled", toml_edit_value::V::Bool(on));
                }
            });
            ui.end_row();
        });
    out
}

/// A value for `set_config_value`, without the GUI depending on toml_edit.
mod toml_edit_value {
    pub enum V {
        Bool(bool),
        Int(u64),
        Float(f32),
    }

    impl V {
        pub fn write(self, text: &str, table: &str, key: &str) -> Result<String, String> {
            match self {
                V::Bool(b) => wtm_policy::set_config_value(text, table, key, b),
                V::Int(n) => wtm_policy::set_config_value(text, table, key, n as i64),
                // Rounded so 90.0 stays 90.0 rather than 90.00000381.
                V::Float(f) => {
                    let f = (f64::from(f) * 10.0).round() / 10.0;
                    wtm_policy::set_config_value(text, table, key, f)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_dry_run_rewrites_only_the_first_key() {
        let text = "# dry_run = x\ndry_run = true  # safe\n[a]\ndry_run = true\n";
        assert_eq!(
            set_dry_run(text, false).unwrap(),
            "# dry_run = x\ndry_run = false  # safe\n[a]\ndry_run = true\n"
        );
        assert_eq!(
            set_dry_run("dry_run=false", true).unwrap(),
            "dry_run = true"
        );
        assert_eq!(set_dry_run("x = 1", true), None);
    }

    #[test]
    fn sync_keeps_unsaved_edits() {
        let mut v = ConfigView::default();
        v.sync("a");
        assert_eq!(v.text, "a");
        assert!(!v.dirty());
        v.text = "edited".into();
        v.sync("b");
        assert_eq!(v.text, "edited", "edits survive a refresh");
        assert!(v.dirty());
        v.text = "b".into();
        v.sync("c");
        assert_eq!(v.text, "c", "clean views follow the service");
    }

    #[test]
    fn validation_is_cached_until_the_text_changes() {
        let mut v = ConfigView::default();
        v.sync(wtm_policy::DEFAULT_CONFIG_TOML);
        assert!(v.validate().is_ok());
        v.text = "dry_run = 3".into();
        assert!(v.validate().is_err());
        assert!(v.validate().is_err());
    }
}

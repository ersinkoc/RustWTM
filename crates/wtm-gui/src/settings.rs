//! Settings tab: edits the service's `config.toml` with live validation.

use eframe::egui::{self, Color32, RichText, Ui};

use crate::backend::{Data, UiCommand};

#[derive(Debug, Default)]
pub struct ConfigView {
    /// Text being edited.
    pub text: String,
    /// The service's text the edit started from.
    original: Option<String>,
    /// The last validated text and its result (the parsed `dry_run`), so
    /// parsing runs only when the text changes.
    check: Option<(String, Result<bool, String>)>,
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

    fn validate(&mut self) -> Result<bool, String> {
        match &self.check {
            Some((text, result)) if *text == self.text => result.clone(),
            _ => {
                let result = wtm_policy::Config::from_toml(&self.text)
                    .map(|c| c.dry_run)
                    .map_err(|e| e.to_string());
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
        ui.separator();
        if let Ok(dry_run) = valid {
            let (label, to) = if dry_run {
                ("Switch to enforcing", false)
            } else {
                ("Switch to dry-run", true)
            };
            if ui.button(label).clicked()
                && let Some(new) = set_dry_run(&view.text, to)
            {
                view.text = new;
            }
        }
        if !admin {
            ui.weak("Run RustWTM as administrator to save.");
        }
    });
    match &valid {
        Ok(_) => ui.colored_label(Color32::from_rgb(80, 180, 100), "✔ Valid configuration"),
        Err(e) => ui.colored_label(Color32::from_rgb(220, 70, 70), format!("✖ {e}")),
    };
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("config_editor")
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut view.text)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(30),
            );
        });
    commands
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

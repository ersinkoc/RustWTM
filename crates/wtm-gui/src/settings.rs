//! Settings tab: its pages, the service's main settings as a form, and its
//! `config.toml` as text, with live validation.

use eframe::egui::{self, RichText, Ui};

use crate::backend::{Data, UiCommand};

/// A page of the Settings tab, chosen from the bar at its top.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    #[default]
    Window,
    Service,
    File,
    Telegram,
}

impl Page {
    /// The text editor fills the page and scrolls itself.
    pub fn scrolls(self) -> bool {
        self != Self::File
    }
}

/// The bar of Settings pages.
pub fn page_bar(ui: &mut Ui, page: &mut Page) {
    ui.horizontal(|ui| {
        for (p, label) in [
            (Page::Window, crate::i18n::tr("This window")),
            (Page::Service, crate::i18n::tr("Service settings")),
            (Page::File, "config.toml"),
            (Page::Telegram, "Telegram"),
        ] {
            ui.selectable_value(page, p, RichText::new(label).size(15.0));
        }
    });
    ui.separator();
}

#[derive(Debug, Default)]
pub struct ConfigView {
    pub page: Page,
    /// Text being edited.
    pub text: String,
    /// The service's text the edit started from.
    original: Option<String>,
    /// The service's text changed (another page saved, or the file was
    /// edited) while this one had unsaved edits: saving would undo that.
    conflict: bool,
    /// The last validated text and its result, so parsing runs only when
    /// the text changes.
    check: Option<(String, Result<wtm_policy::Config, String>)>,
}

impl ConfigView {
    fn dirty(&self) -> bool {
        self.original.as_deref() != Some(self.text.as_str())
    }

    /// Takes the service's text unless the user has unsaved edits; then a
    /// change made elsewhere meanwhile is a conflict to show, not to lose.
    fn sync(&mut self, service_text: &str) {
        if self.original.as_deref() == Some(service_text) {
            return;
        }
        if !self.dirty() || self.original.is_none() || self.text == service_text {
            // Nothing of ours to keep, or our own save coming back.
            self.text = service_text.to_owned();
            self.conflict = false;
        } else {
            self.conflict = true;
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
            crate::theme::loading(ui, crate::i18n::tr("Loading the service settings…"));
            return commands;
        }
        Some(Err(e)) => {
            crate::theme::colored(ui, crate::theme::BAD, e);
            if ui.button(crate::i18n::tr("Retry")).clicked() {
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
            crate::theme::colored(ui, crate::theme::WARN, crate::i18n::tr("● unsaved changes"));
        }
    });
    if let Some(e) = data.status.as_ref().and_then(|s| s.config_error.as_deref()) {
        crate::theme::colored(
            ui,
            crate::theme::BAD,
            crate::i18n::format(
                "The service could not load this file and keeps its previous or default settings: {0}",
                &[e.to_owned()],
            ),
        );
    }
    if view.conflict && dirty {
        crate::theme::colored(
            ui,
            crate::theme::WARN,
            crate::i18n::tr(
                "The configuration was changed elsewhere (another page, or the file itself) \
                 while you were editing. Saving replaces those changes; discard your edits to \
                 see them.",
            ),
        );
    }
    ui.horizontal(|ui| {
        let label = if view.conflict {
            crate::i18n::tr("Save and replace the other changes")
        } else {
            crate::i18n::tr("Save")
        };
        let save = ui.add_enabled(admin && dirty && valid.is_ok(), egui::Button::new(label));
        if save.clicked() {
            commands.push(UiCommand::SaveConfig(view.text.clone()));
        }
        if ui
            .add_enabled(dirty, egui::Button::new(crate::i18n::tr("Discard changes")))
            .clicked()
        {
            view.original = None;
            reload(&mut commands);
        }
        if !admin {
            ui.weak(crate::i18n::tr("Run RustWTM as administrator to save."));
        }
    });
    match &valid {
        Ok(_) => crate::theme::colored(
            ui,
            crate::theme::GOOD,
            crate::i18n::tr("✔ Valid configuration"),
        ),
        Err(e) => crate::theme::colored(ui, crate::theme::BAD, format!("✖ {e}")),
    };
    ui.add_space(4.0);
    if view.page == Page::File {
        ui.weak(crate::i18n::tr(
            "Rules, app limits, the kill list and every other option. See docs/configuration.md.",
        ));
        let line = ui.text_style_height(&egui::TextStyle::Monospace);
        let rows = ((ui.available_height() - 12.0) / line).max(8.0) as usize;
        let mut area = egui::ScrollArea::vertical()
            .id_salt("config_text")
            .auto_shrink([false, false]);
        if let Some(y) = crate::theme::take_scroll(ui, "config_text") {
            area = area.vertical_scroll_offset(y);
        }
        let out = area.show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut view.text)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(rows),
            );
        });
        let fill = ui.visuals().text_edit_bg_color();
        crate::theme::more_below(ui, "config_text", &out, fill);
        return commands;
    }
    match &valid {
        Ok(config) => crate::theme::panel(ui, |ui| {
            if let Some(new) = form(ui, config, &view.text) {
                view.text = new;
            }
        }),
        Err(_) => {
            ui.horizontal(|ui| {
                ui.label(crate::i18n::tr(
                    "Fix the errors in config.toml to use this form.",
                ));
                if ui.button("config.toml").clicked() {
                    view.page = Page::File;
                }
            });
        }
    }
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
            ui.strong(crate::i18n::tr("Mode"));
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(!c.dry_run, crate::i18n::tr("Enforcing"))
                    .clicked()
                    && c.dry_run
                {
                    set("", "dry_run", toml_edit_value::V::Bool(false));
                }
                if ui
                    .selectable_label(c.dry_run, crate::i18n::tr("Monitoring only"))
                    .on_hover_text(crate::i18n::tr(
                        "Logs what it would do without touching any process",
                    ))
                    .clicked()
                    && !c.dry_run
                {
                    set("", "dry_run", toml_edit_value::V::Bool(true));
                }
            });
            ui.end_row();

            ui.strong(crate::i18n::tr("CPU governor"));
            ui.vertical(|ui| {
                let mut on = g.enabled;
                if ui
                    .checkbox(
                        &mut on,
                        crate::i18n::tr("Slow down the busiest apps when the CPU is overloaded"),
                    )
                    .changed()
                {
                    set("governor", "enabled", toml_edit_value::V::Bool(on));
                }
                ui.add_enabled_ui(g.enabled, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::tr("Act when the CPU stays above"));
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
                        ui.label(crate::i18n::tr("for"));
                        let mut secs = g.sustain_secs;
                        if ui
                            .add(egui::DragValue::new(&mut secs).range(1..=300).suffix(" s"))
                            .changed()
                        {
                            set("governor", "sustain_secs", toml_edit_value::V::Int(secs));
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::tr("Undo once it stays below"));
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
                        ui.label(crate::i18n::tr("for"));
                        let mut secs = g.recover_secs;
                        if ui
                            .add(egui::DragValue::new(&mut secs).range(1..=600).suffix(" s"))
                            .changed()
                        {
                            set("governor", "recover_secs", toml_edit_value::V::Int(secs));
                        }
                    });
                    ui.horizontal(|ui| {
                        let label = ui.label(crate::i18n::tr("Slow down up to"));
                        let mut n = g.max_offenders as u64;
                        if ui
                            .add(egui::DragValue::new(&mut n).range(1..=20))
                            .labelled_by(label.id)
                            .changed()
                        {
                            set("governor", "max_offenders", toml_edit_value::V::Int(n));
                        }
                        ui.label(crate::i18n::tr("apps at a time"));
                    });
                });
            });
            ui.end_row();

            let m = &c.memory_guard;
            ui.strong(crate::i18n::tr("Memory guard"));
            ui.vertical(|ui| {
                let mut on = m.enabled;
                if ui
                    .checkbox(
                        &mut on,
                        crate::i18n::tr("Free memory of the largest apps when RAM runs out"),
                    )
                    .on_hover_text(crate::i18n::tr(
                        "Moves their pages to the standby list; nothing is lost",
                    ))
                    .changed()
                {
                    set("memory_guard", "enabled", toml_edit_value::V::Bool(on));
                }
                ui.add_enabled_ui(m.enabled, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::tr("When memory use stays above"));
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
                        ui.label(crate::i18n::tr("for"));
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

            let g = &c.game_mode;
            ui.strong(crate::i18n::tr("Game mode"));
            ui.vertical(|ui| {
                let mut on = g.enabled;
                if ui
                    .checkbox(
                        &mut on,
                        crate::i18n::tr("Calm busy background apps while a full-screen app is in front"),
                    )
                    .on_hover_text(crate::i18n::tr(
                        "Lower priority and efficiency mode, rolled back when it ends",
                    ))
                    .changed()
                {
                    set("game_mode", "enabled", toml_edit_value::V::Bool(on));
                }
                ui.add_enabled_ui(g.enabled, |ui| {
                    let mut plan = g.power_plan;
                    if ui
                        .checkbox(
                            &mut plan,
                            crate::i18n::tr("Switch to the High performance power plan meanwhile"),
                        )
                        .changed()
                    {
                        set("game_mode", "power_plan", toml_edit_value::V::Bool(plan));
                    }
                });
            });
            ui.end_row();

            let r = &c.reserve;
            ui.strong(crate::i18n::tr("Reserved for RustWTM"));
            ui.vertical(|ui| {
                let mut on = r.rescue_core;
                if ui
                    .checkbox(
                        &mut on,
                        crate::i18n::tr("Keep one core free for RustWTM when the CPU is overloaded"),
                    )
                    .on_hover_text(crate::i18n::tr(
                        "Other apps are moved off that core until the load drops, so RustWTM always responds",
                    ))
                    .changed()
                {
                    set("reserve", "rescue_core", toml_edit_value::V::Bool(on));
                }
                ui.add_enabled_ui(r.rescue_core, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::tr("When CPU stays above"));
                        let mut high = r.trigger_cpu_percent;
                        let range = (r.release_cpu_percent + 1.0).max(50.0)..=100.0;
                        if ui
                            .add(egui::DragValue::new(&mut high).range(range).suffix("%"))
                            .changed()
                        {
                            set(
                                "reserve",
                                "trigger_cpu_percent",
                                toml_edit_value::V::Float(high),
                            );
                        }
                        ui.label(crate::i18n::tr("for"));
                        let mut secs = r.sustain_secs;
                        if ui
                            .add(egui::DragValue::new(&mut secs).range(1..=120).suffix(" s"))
                            .changed()
                        {
                            set("reserve", "sustain_secs", toml_edit_value::V::Int(secs));
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::tr("Service memory never paged out"));
                    let mut mb = r.working_set_mb;
                    if ui
                        .add(egui::DragValue::new(&mut mb).range(0..=1024).suffix(" MB"))
                        .changed()
                    {
                        set("reserve", "working_set_mb", toml_edit_value::V::Int(mb));
                    }
                });
            });
            ui.end_row();

            let s = &c.swarm;
            ui.strong(crate::i18n::tr("Swarm warning"));
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    let mut on = s.enabled;
                    let check = ui.checkbox(&mut on, crate::i18n::tr("Warn when one program runs"));
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
                    ui.label(crate::i18n::tr("copies or more"));
                });
                ui.add_enabled_ui(s.enabled, |ui| {
                    ui.horizontal(|ui| {
                        let label = ui.label(crate::i18n::tr("End copies that run longer than"));
                        let mut mins = s.end_after_mins;
                        if ui
                            .add(
                                egui::DragValue::new(&mut mins)
                                    .range(0..=10_080)
                                    .suffix(crate::i18n::tr(" min")),
                            )
                            .labelled_by(label.id)
                            .on_hover_text(crate::i18n::tr(
                                "With what they started. A helper that should take seconds is \
                                 stuck by then. 0 only warns.",
                            ))
                            .changed()
                        {
                            set("swarm", "end_after_mins", toml_edit_value::V::Int(mins));
                        }
                        ui.weak(if s.end_after_mins == 0 {
                            crate::i18n::tr("(off: only warn)")
                        } else {
                            crate::i18n::tr("(and what they started)")
                        });
                    });
                });
            });
            ui.end_row();

            ui.strong(crate::i18n::tr("History"));
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::tr("Keep per-app usage for"));
                    let mut days = u64::from(c.history_days);
                    if ui
                        .add(
                            egui::DragValue::new(&mut days)
                                .range(1..=365)
                                .suffix(crate::i18n::tr(" days")),
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
                        crate::i18n::tr(
                            "Flag apps with constant CPU use or steadily growing memory",
                        ),
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
    fn a_change_made_elsewhere_while_editing_is_a_conflict() {
        let mut v = ConfigView::default();
        v.sync("dry_run = true\n");
        assert!(!v.dirty() && !v.conflict);
        v.text = "dry_run = false\n".into();
        // The kill list page saved meanwhile.
        v.sync("dry_run = true\n[[kill]]\nname = \"x.exe\"\n");
        assert!(v.conflict, "saving now would drop the kill entry");
        assert_eq!(v.text, "dry_run = false\n", "the edit is kept");
        // Our own save coming back is no conflict.
        let mut v = ConfigView::default();
        v.sync("a = 1\n");
        v.text = "a = 2\n".into();
        v.sync("a = 2\n");
        assert!(!v.conflict && !v.dirty());
        // Without edits the new text is simply taken.
        v.sync("a = 3\n");
        assert!(!v.conflict && v.text == "a = 3\n");
    }

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

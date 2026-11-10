//! Original visual policy workspace; preview executes only the pure policy engine.
use eframe::egui::{self, Ui};
use wtm_policy::{Action, Condition, Config, Matcher, Preset, PriorityLevel, Rule};
#[derive(Default)]
pub struct View {
    draft: Option<Rule>,
    original_name: Option<String>,
    preview: Vec<String>,
    previewed: bool,
    pending: Option<(String, String, String)>,
    error: Option<String>,
}
fn pattern(ui: &mut Ui, label: &str, value: &mut Option<String>) {
    let mut text = value.clone().unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        let label = ui.label(crate::i18n::tr(label));
        if ui
            .add(
                egui::TextEdit::singleline(&mut text)
                    .desired_width(320.0)
                    .char_limit(512),
            )
            .labelled_by(label.id)
            .changed()
        {
            *value = (!text.trim().is_empty()).then_some(text);
        }
    });
}
fn threshold(ui: &mut Ui, label: &str, value: &mut Option<f32>) {
    let mut enabled = value.is_some();
    ui.horizontal_wrapped(|ui| {
        let label = ui.checkbox(&mut enabled, crate::i18n::tr(label));
        if label.changed() {
            *value = enabled.then_some(20.0);
        }
        if let Some(value) = value {
            ui.add(egui::Slider::new(value, 0.1..=100.0).suffix("%"))
                .labelled_by(label.id);
        }
    });
}
impl View {
    pub fn show(
        &mut self,
        ui: &mut Ui,
        data: &crate::backend::Data,
    ) -> Option<crate::backend::UiCommand> {
        let Some(Ok((_, text))) = &data.config else {
            ui.weak(crate::i18n::tr(
                "Load the service configuration to edit policies.",
            ));
            return None;
        };
        let config = match Config::from_toml(text) {
            Ok(c) => c,
            Err(e) => {
                ui.label(e.to_string());
                return None;
            }
        };
        let admin = matches!(data.mode, crate::backend::Mode::Service { admin: true, .. });
        let mut proposed = None;
        crate::theme::card(ui, crate::i18n::tr("Protection profiles"), |ui| {
            ui.weak(crate::i18n::tr("Profiles change RustWTM policy settings after review; they do not silently change Windows power plans."));
            ui.horizontal_wrapped(|ui| {
                for (label, preset) in [
                    ("Work", Preset::Balanced),
                    ("Game", Preset::Gentle),
                    ("Battery", Preset::Strict),
                ] {
                    if ui.button(crate::i18n::tr(label)).clicked() {
                        proposed = Some(wtm_policy::set_preset(text, preset).map(|next| {
                            (
                                next,
                                format!(
                                    "{}: {}",
                                    crate::i18n::tr(label),
                                    crate::i18n::tr(preset.description())
                                ),
                            )
                        }));
                    }
                }
            });
        });
        crate::theme::card(ui, crate::i18n::tr("Saved rules"), |ui| {
            if config.rules.is_empty() {
                ui.weak(crate::i18n::tr("No saved process rules."));
            }
            for rule in &config.rules {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(&rule.name);
                    ui.weak(
                        rule.actions
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(" · "),
                    );
                    if ui.button(crate::i18n::tr("Edit rule")).clicked() {
                        self.draft = Some(rule.clone());
                        self.original_name = Some(rule.name.clone());
                        self.preview.clear();
                        self.previewed = false;
                    }
                    if ui.button(crate::i18n::tr("Remove rule")).clicked() {
                        proposed = Some(wtm_policy::rule_editor::remove(text, &rule.name).map(
                            |next| {
                                (
                                    next,
                                    format!("{}: {}", crate::i18n::tr("Remove rule"), rule.name),
                                )
                            },
                        ));
                    }
                });
            }
            if ui.button(crate::i18n::tr("New rule")).clicked() {
                self.original_name = None;
                self.draft = Some(Rule {
                    name: String::new(),
                    matcher: Matcher::default(),
                    when: None,
                    actions: vec![Action::EcoQos],
                });
                self.preview.clear();
                self.previewed = false;
            }
        });
        if let Some(rule) = &mut self.draft {
            let mut preview = false;
            let mut save = false;
            let mut remove = None;
            let mut add = None;
            crate::theme::card(ui, crate::i18n::tr("Rule editor"), |ui| {
                ui.horizontal_wrapped(|ui| {
                    let label = ui.label(crate::i18n::tr("Rule name"));
                    ui.add(
                        egui::TextEdit::singleline(&mut rule.name)
                            .desired_width(320.0)
                            .char_limit(128),
                    )
                    .labelled_by(label.id);
                });
                pattern(ui, "Executable pattern", &mut rule.matcher.name);
                pattern(ui, "Full path pattern", &mut rule.matcher.path);
                pattern(ui, "Command line pattern", &mut rule.matcher.cmdline);
                let mut conditional = rule.when.is_some();
                if ui
                    .checkbox(
                        &mut conditional,
                        crate::i18n::tr("Wait for sustained resource usage"),
                    )
                    .changed()
                {
                    rule.when = conditional.then_some(Condition {
                        cpu_above: Some(20.0),
                        for_secs: 10,
                        ..Default::default()
                    });
                }
                if let Some(when) = &mut rule.when {
                    threshold(ui, "CPU above", &mut when.cpu_above);
                    threshold(ui, "GPU above", &mut when.gpu_above);
                    let mut enabled = when.memory_above_mb.is_some();
                    ui.horizontal_wrapped(|ui| {
                        let label = ui.checkbox(&mut enabled, crate::i18n::tr("Memory above"));
                        if label.changed() {
                            when.memory_above_mb = enabled.then_some(1024);
                        }
                        if let Some(v) = &mut when.memory_above_mb {
                            ui.add(egui::DragValue::new(v).range(1..=1_048_576).suffix(" MiB"))
                                .labelled_by(label.id);
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        let label = ui.label(crate::i18n::tr("Sustained for"));
                        ui.add(
                            egui::DragValue::new(&mut when.for_secs)
                                .range(1..=86400)
                                .suffix(" s"),
                        )
                        .labelled_by(label.id);
                    });
                }
                ui.separator();
                ui.strong(crate::i18n::tr("Actions"));
                for (index, action) in rule.actions.iter_mut().enumerate() {
                    ui.horizontal_wrapped(|ui| {
                        let label = ui.label(action.to_string());
                        match action {
                            Action::CpuCap(v) => {
                                ui.add(egui::Slider::new(v, 0.1..=100.0).suffix("%"))
                                    .labelled_by(label.id);
                            }
                            Action::CpuCapRelative(v) => {
                                ui.add(egui::Slider::new(v, 0.01..=1.0))
                                    .labelled_by(label.id);
                            }
                            Action::MemoryCapMb(v) => {
                                ui.add(egui::DragValue::new(v).range(1..=1_048_576).suffix(" MiB"))
                                    .labelled_by(label.id);
                            }
                            Action::Priority(v) => {
                                egui::ComboBox::from_id_salt(("rule-priority", index))
                                    .selected_text(format!("{v:?}"))
                                    .show_ui(ui, |ui| {
                                        for p in [
                                            PriorityLevel::Idle,
                                            PriorityLevel::BelowNormal,
                                            PriorityLevel::Normal,
                                            PriorityLevel::AboveNormal,
                                        ] {
                                            ui.selectable_value(v, p, format!("{p:?}"));
                                        }
                                    })
                                    .response
                                    .labelled_by(label.id);
                            }
                            _ => {}
                        }
                        if ui.small_button(crate::i18n::tr("Remove action")).clicked() {
                            remove = Some(index);
                        }
                    });
                }
                ui.menu_button(crate::i18n::tr("Add action"), |ui| {
                    for action in [
                        Action::EcoQos,
                        Action::Priority(PriorityLevel::BelowNormal),
                        Action::CpuCap(25.0),
                        Action::CpuCapRelative(0.5),
                        Action::MemoryCapMb(1024),
                        Action::TrimMemory,
                        Action::Suspend,
                        Action::Kill,
                    ] {
                        if ui.button(action.to_string()).clicked() {
                            add = Some(action);
                            ui.close();
                        }
                    }
                });
                if wtm_policy::rule_editor::destructive(rule) {
                    crate::theme::colored(
                        ui,
                        crate::theme::WARN,
                        crate::i18n::tr(
                            "This rule can suspend or terminate matching programs. Review its scope carefully.",
                        ),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    preview = ui
                        .button(crate::i18n::tr("Preview on current sample"))
                        .clicked();
                    save = ui.button(crate::i18n::tr("Review rule changes")).clicked();
                });
            });
            if let Some(index) = remove {
                rule.actions.remove(index);
            }
            if let Some(action) = add
                && rule.actions.len() < 8
            {
                rule.actions.push(action);
            }
            if preview {
                self.preview.clear();
                self.previewed = false;
                match wtm_policy::rule_editor::replace(text, self.original_name.as_deref(), rule) {
                    Err(e) => self.error = Some(e),
                    Ok(_) => {
                        self.error = None;
                        if let Some(snapshot) = &data.snapshot {
                            self.previewed = true;
                            let mut engine = wtm_policy::RuleEngine::new(vec![rule.clone()]);
                            let now = std::time::Instant::now();
                            let mut lookup =
                                |p: &wtm_core::ProcessInfo, detail: wtm_policy::Detail| {
                                    let key = (p.pid, p.create_time);
                                    match detail {
                                        wtm_policy::Detail::Path => {
                                            data.image_paths.get(&key).cloned()
                                        }
                                        wtm_policy::Detail::CommandLine => {
                                            data.command_lines.get(&key).cloned()
                                        }
                                    }
                                };
                            let mut decisions = engine.tick(now, &snapshot.processes, &mut lookup);
                            decisions.extend(
                                engine.tick(
                                    now + std::time::Duration::from_secs(
                                        rule.when
                                            .as_ref()
                                            .map_or(0, |w| w.for_secs)
                                            .saturating_add(1),
                                    ),
                                    &snapshot.processes,
                                    &mut lookup,
                                ),
                            );
                            self.preview = decisions
                                .iter()
                                .take(64)
                                .filter_map(|d| match d {
                                    wtm_policy::Decision::Apply { target, action, .. } => {
                                        Some(format!(
                                            "{} · PID {} → {}",
                                            target.name, target.pid, action
                                        ))
                                    }
                                    _ => None,
                                })
                                .collect();
                        }
                    }
                }
            }
            if save {
                proposed = Some(
                    wtm_policy::rule_editor::replace(text, self.original_name.as_deref(), rule)
                        .map(|next| (next, crate::i18n::tr("Save process rule").into())),
                );
            }
            ui.weak(crate::i18n::tr("Preview executes no actions. It assumes this sample persists for the configured duration and uses only cached paths and command lines."));
            if self.previewed && self.preview.is_empty() {
                ui.weak(crate::i18n::tr(
                    "No matching programs in the cached sample.",
                ));
            }
            for row in &self.preview {
                ui.label(row);
            }
        }
        if let Some(result) = proposed {
            match result {
                Ok((next, description)) => {
                    self.pending = Some((text.clone(), next, description));
                    self.error = None
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(e) = &self.error {
            crate::theme::colored(ui, crate::theme::WARN, e);
        }
        let mut command = None;
        if let Some((base, next, description)) = self.pending.clone() {
            let mut save = false;
            let mut cancel = false;
            let modal=egui::Modal::new(egui::Id::unique("policy-review")).show(ui.ctx(),|ui|{
    ui.set_max_width(680.0);ui.heading(crate::i18n::tr("Review policy changes"));ui.label(&description);
    ui.weak(crate::i18n::tr("Existing safety checks and dry-run mode still apply. Matching rules may act on running programs immediately after saving."));
    egui::ScrollArea::vertical().max_height(280.0).show(ui,|ui|{ui.monospace(&next);});
    ui.horizontal(|ui|{save=crate::theme::primary_action(ui,admin&&text==&base,crate::i18n::tr("Save reviewed configuration")).clicked();cancel=ui.button(crate::i18n::tr("Cancel")).clicked();});
    if text!=&base{ui.label(crate::i18n::tr("Configuration changed. Close this review and prepare it again."));}
   });
            if save {
                command = Some(crate::backend::UiCommand::SaveConfig(next));
                self.pending = None;
                self.draft = None;
                self.original_name = None;
                self.preview.clear();
                self.previewed = false;
            } else if cancel || modal.should_close() {
                self.pending = None;
            }
        }
        command
    }
}

#[cfg(test)]
impl View {
    pub fn fixture() -> Self {
        Self {
            draft: Some(Rule {
                name: "Background compiler budget".into(),
                matcher: Matcher {
                    name: Some("compiler.exe".into()),
                    path: None,
                    cmdline: None,
                },
                when: Some(Condition {
                    cpu_above: Some(25.0),
                    for_secs: 10,
                    ..Default::default()
                }),
                actions: vec![Action::EcoQos, Action::CpuCap(25.0)],
            }),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    #[test]
    fn empty_rule_preview_reports_no_matches_without_saving() {
        let data = crate::backend::Data {
            config: Some(Ok(("fixture".into(), "dry_run=true".into()))),
            snapshot: Some(std::sync::Arc::new(wtm_core::Snapshot {
                processes: vec![],
                system: Default::default(),
                interval: None,
                taken_at_ms: 0,
                net_traced: false,
            })),
            ..Default::default()
        };
        let mut h = egui_kittest::Harness::builder()
            .allow_missing_glyphs()
            .with_size([1100.0, 1800.0])
            .build_ui_state(
                move |ui, v: &mut View| {
                    assert!(v.show(ui, &data).is_none());
                },
                View::fixture(),
            );
        h.run();
        h.get_by_label("Preview on current sample").click();
        h.run();
        assert!(
            h.query_by_label("No matching programs in the cached sample.")
                .is_some()
        );
        assert!(h.state().pending.is_none());
    }
}

//! Limits tab: per-application CPU / memory budgets kept in the service's
//! configuration, with their live usage.

use eframe::egui::{self, Color32, Ui};
use wtm_core::format_bytes;
use wtm_policy::{AppLimit, Config, LimitScope, remove_app_limit, set_app_limit};

use crate::backend::{Data, Mode, Panel, UiCommand};
use crate::processes::budget_label;
use crate::sort::{Sort, header, num};

const WARN: Color32 = Color32::from_rgb(230, 140, 40);

#[derive(Debug)]
pub struct LimitsView {
    name: String,
    cpu: bool,
    cores: f32,
    memory: bool,
    gb: f32,
    scope: LimitScope,
    /// The entry being edited, replaced on save.
    editing: Option<(String, LimitScope)>,
    error: Option<String>,
}

impl Default for LimitsView {
    fn default() -> Self {
        Self {
            name: String::new(),
            cpu: true,
            cores: 2.0,
            memory: false,
            gb: 8.0,
            scope: LimitScope::Group,
            editing: None,
            error: None,
        }
    }
}

impl LimitsView {
    /// Starts a new limit for `app` (from the process context menu).
    pub fn prefill(&mut self, app: &str) {
        *self = Self {
            name: app.to_owned(),
            ..Self::default()
        };
    }

    fn edit(&mut self, l: &AppLimit) {
        *self = Self {
            name: l.name.clone(),
            cpu: l.cpu_cores.is_some(),
            cores: l.cpu_cores.unwrap_or(2.0),
            memory: l.memory_gb.is_some(),
            gb: l.memory_gb.unwrap_or(8.0),
            scope: l.scope,
            editing: Some((l.name.clone(), l.scope)),
            error: None,
        };
    }

    fn limit(&self) -> AppLimit {
        AppLimit {
            name: self.name.trim().to_owned(),
            cpu_cores: self.cpu.then_some(self.cores),
            memory_gb: self.memory.then_some(self.gb),
            scope: self.scope,
        }
    }

    /// The configuration text with the form's limit saved into it.
    fn apply(&self, text: &str) -> Result<String, String> {
        let text = match &self.editing {
            Some((name, scope)) => remove_app_limit(text, name, *scope)?,
            None => text.to_owned(),
        };
        set_app_limit(&text, &self.limit())
    }
}

fn scope_label(scope: LimitScope) -> &'static str {
    match scope {
        LimitScope::Group => "all processes together",
        LimitScope::Each => "each instance separately",
    }
}

pub fn limits_tab(ui: &mut Ui, data: &Data, view: &mut LimitsView) -> Vec<UiCommand> {
    let mut commands = Vec::new();
    let text = match &data.config {
        None => {
            ui.spinner();
            return commands;
        }
        Some(Err(e)) => {
            ui.colored_label(Color32::RED, e);
            if ui.button("Retry").clicked() {
                commands.push(UiCommand::Load(Panel::Config));
            }
            return commands;
        }
        Some(Ok((_, text))) => text,
    };
    let config = match Config::from_toml(text) {
        Ok(c) => c,
        Err(e) => {
            ui.colored_label(
                Color32::RED,
                format!("The configuration is invalid ({e}); fix it in Settings."),
            );
            return commands;
        }
    };
    let admin = matches!(data.mode, Mode::Service { admin: true, .. });
    let cpus = data
        .snapshot
        .as_ref()
        .map_or(1, |s| s.system.logical_cpus.max(1));
    let usage = data.status.as_ref().map_or(&[][..], |s| &s.app_limits);

    ui.label(
        "Every process of an app, and everything it starts, shares the app's budget. \
         Programs that reach the memory budget get out-of-memory errors.",
    );
    if config.dry_run {
        ui.colored_label(
            WARN,
            "Dry-run mode: limits are only previewed in the event log. Switch to enforcing in Settings.",
        );
    }
    if !admin {
        ui.weak("Run RustWTM as administrator to change limits.");
    }
    ui.add_space(6.0);

    struct Row<'a> {
        limit: &'a AppLimit,
        instances: usize,
        processes: usize,
        cpu: f32,
        mem: u64,
    }
    let mut rows: Vec<Row<'_>> = config
        .limits
        .iter()
        .map(|l| {
            let groups: Vec<_> = usage
                .iter()
                .filter(|u| u.group.app.eq_ignore_ascii_case(&l.name) && u.group.scope == l.scope)
                .collect();
            Row {
                limit: l,
                instances: groups.len(),
                processes: groups.iter().map(|g| g.processes).sum(),
                cpu: groups.iter().map(|g| g.cpu_percent).sum(),
                mem: groups.iter().map(|g| g.memory_bytes).sum(),
            }
        })
        .collect();
    let mut sort = Sort::load(ui, "app_limits", Sort::new(0, false));
    sort.apply(&mut rows, |a, b, col| match col {
        0 => crate::sort::text(&a.limit.name, &b.limit.name),
        1 => num(
            a.limit.cpu_cores.unwrap_or(0.0).into(),
            b.limit.cpu_cores.unwrap_or(0.0).into(),
        )
        .then(num(
            a.limit.memory_gb.unwrap_or(0.0).into(),
            b.limit.memory_gb.unwrap_or(0.0).into(),
        )),
        2 => crate::sort::text(scope_label(a.limit.scope), scope_label(b.limit.scope)),
        3 => a.processes.cmp(&b.processes),
        4 => num(a.cpu.into(), b.cpu.into()),
        _ => a.mem.cmp(&b.mem),
    });
    egui::Grid::new("app_limits_grid")
        .num_columns(7)
        .striped(true)
        .show(ui, |ui| {
            for (col, (h, numeric)) in [
                ("App", false),
                ("Budget", true),
                ("Applies to", false),
                ("Processes", true),
                ("CPU now", true),
                ("Memory now", true),
            ]
            .into_iter()
            .enumerate()
            {
                header(ui, h, col, numeric, &mut sort);
            }
            ui.label("");
            ui.end_row();
            for r in &rows {
                let l = r.limit;
                ui.label(&l.name);
                let mut budget = budget_label(l);
                if let Some(p) = l.cpu_percent(cpus) {
                    budget.push_str(&format!(" ({p:.1}% of machine)"));
                }
                ui.label(budget);
                ui.label(scope_label(l.scope));
                ui.label(match l.scope {
                    LimitScope::Each if r.instances > 0 => {
                        format!("{} in {} instances", r.processes, r.instances)
                    }
                    _ => r.processes.to_string(),
                });
                ui.label(format!("{:.2} cores", r.cpu / 100.0 * cpus as f32));
                ui.label(format_bytes(r.mem as f64));
                ui.horizontal(|ui| {
                    if ui.add_enabled(admin, egui::Button::new("Edit")).clicked() {
                        view.edit(l);
                    }
                    if ui.add_enabled(admin, egui::Button::new("Remove")).clicked() {
                        match remove_app_limit(text, &l.name, l.scope) {
                            Ok(new) => commands.push(UiCommand::SaveConfig(new)),
                            Err(e) => view.error = Some(e),
                        }
                    }
                });
                ui.end_row();
            }
        });
    sort.store(ui, "app_limits");
    if config.limits.is_empty() {
        ui.weak("No app limits yet. Add one below, or right-click a process and choose \"Limit this app…\".");
    }

    ui.add_space(12.0);
    ui.heading(if view.editing.is_some() {
        "Change limit"
    } else {
        "Add limit"
    });
    egui::Grid::new("app_limit_form")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Program");
            ui.add(
                egui::TextEdit::singleline(&mut view.name)
                    .hint_text("e.g. node.exe or chrome*.exe"),
            );
            ui.end_row();

            ui.checkbox(&mut view.cpu, "CPU");
            ui.add_enabled(
                view.cpu,
                egui::DragValue::new(&mut view.cores)
                    .range(0.1..=cpus as f32)
                    .speed(0.05)
                    .max_decimals(2)
                    .suffix(" cores"),
            );
            ui.end_row();

            ui.checkbox(&mut view.memory, "Memory");
            ui.add_enabled(
                view.memory,
                egui::DragValue::new(&mut view.gb)
                    .range(0.1..=4096.0)
                    .speed(0.1)
                    .max_decimals(2)
                    .suffix(" GB"),
            );
            ui.end_row();

            ui.label("Applies to");
            ui.horizontal(|ui| {
                ui.radio_value(&mut view.scope, LimitScope::Group, "All processes together")
                    .on_hover_text(
                        "One shared budget for every process of the app and their children.",
                    );
                ui.radio_value(
                    &mut view.scope,
                    LimitScope::Each,
                    "Each instance separately",
                )
                .on_hover_text(
                    "Every top-level instance gets its own budget, shared with its children.",
                );
            });
            ui.end_row();
        });
    let candidate = view.limit();
    let valid = !candidate.name.is_empty()
        && (candidate.cpu_cores.is_some() || candidate.memory_gb.is_some());
    ui.horizontal(|ui| {
        if ui
            .add_enabled(admin && valid, egui::Button::new("Save limit"))
            .clicked()
        {
            match view.apply(text) {
                Ok(new) => {
                    commands.push(UiCommand::SaveConfig(new));
                    *view = LimitsView::default();
                }
                Err(e) => view.error = Some(e),
            }
        }
        if view.editing.is_some() && ui.button("Cancel").clicked() {
            *view = LimitsView::default();
        }
    });
    if let Some(e) = &view.error {
        ui.colored_label(Color32::RED, e);
    }
    commands
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_round_trips_and_replaces_the_edited_entry() {
        let text =
            "dry_run = false\n[[limit]]\nname = \"zen.exe\"\nmemory_gb = 8.0\nscope = \"each\"\n";
        let config = Config::from_toml(text).unwrap();
        let mut v = LimitsView::default();
        v.edit(&config.limits[0]);
        assert_eq!(v.limit(), config.limits[0]);
        // Renaming replaces the old entry instead of adding a second one.
        v.name = "zen-browser.exe".into();
        v.scope = LimitScope::Group;
        v.cpu = true;
        let new = Config::from_toml(&v.apply(text).unwrap()).unwrap();
        assert_eq!(new.limits.len(), 1);
        assert_eq!(new.limits[0].name, "zen-browser.exe");
        assert_eq!(new.limits[0].cpu_cores, Some(2.0));

        v.prefill("node.exe");
        assert_eq!(v.limit().name, "node.exe");
        assert!(v.editing.is_none());
        let two = Config::from_toml(&v.apply(text).unwrap()).unwrap();
        assert_eq!(two.limits.len(), 2);
        assert!(v.apply("x = [").is_err());
    }
}

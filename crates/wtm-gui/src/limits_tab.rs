//! Limits tab: per-application CPU / memory budgets kept in the service's
//! configuration, with their live usage.

use eframe::egui::{self, Color32, Ui};
use wtm_core::format_bytes;
use wtm_policy::{AppLimit, Config, LimitScope, remove_app_limit, set_app_limit};

use crate::backend::{Data, Mode, Panel, UiCommand};
use crate::processes::budget_label;
use crate::sort::{Sort, header, num};

const WARN: Color32 = Color32::from_rgb(230, 140, 40);
const BAD: Color32 = Color32::from_rgb(220, 70, 70);
const GOOD: Color32 = Color32::from_rgb(80, 180, 100);

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

    /// Starts a new CPU limit for `app` with a suggested budget.
    pub fn prefill_cores(&mut self, app: &str, cores: f32) {
        self.prefill(app);
        self.cores = cores;
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

/// Image names of running processes with their total CPU and memory, the
/// busiest first.
fn running_apps(data: &Data) -> Vec<(String, f32, u64)> {
    let mut apps: std::collections::HashMap<String, (String, f32, u64)> =
        std::collections::HashMap::new();
    for p in data.snapshot.iter().flat_map(|s| &s.processes) {
        if p.pid == 0 || wtm_core::protection_reason(p.pid, &p.name).is_some() {
            continue;
        }
        let e = apps
            .entry(p.name.to_lowercase())
            .or_insert_with(|| (p.name.clone(), 0.0, 0));
        e.1 += p.cpu_percent;
        e.2 += p.private_bytes;
    }
    let mut out: Vec<_> = apps.into_values().collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.cmp(&a.2)));
    out
}

fn scope_label(scope: LimitScope) -> &'static str {
    match scope {
        LimitScope::Group => crate::i18n::tr("all processes together"),
        LimitScope::Each => crate::i18n::tr("each instance separately"),
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
            if ui.button(crate::i18n::tr("Retry")).clicked() {
                commands.push(UiCommand::Load(Panel::Config));
            }
            return commands;
        }
        Some(Ok((_, text))) => text,
    };
    let config = match Config::from_toml(text) {
        Ok(c) => c,
        Err(e) => {
            ui.colored_label(Color32::RED, {
                crate::i18n::format(
                    "The configuration is invalid ({0}); fix it in Settings.",
                    &[format!("{}", e)],
                )
            });
            return commands;
        }
    };
    let admin = matches!(data.mode, Mode::Service { admin: true, .. });
    let cpus = data
        .snapshot
        .as_ref()
        .map_or(1, |s| s.system.logical_cpus.max(1));
    let usage = data.status.as_ref().map_or(&[][..], |s| &s.app_limits);

    ui.label(crate::i18n::tr(
        "Every process of an app, and everything it starts, shares the app's budget. \
         Programs that reach the memory budget get out-of-memory errors.",
    ));
    if config.dry_run {
        ui.colored_label(
            WARN,
            crate::i18n::tr("Dry-run mode: limits are only previewed in the event log. Switch to enforcing in Settings."),
        );
    }
    if !admin {
        ui.weak(crate::i18n::tr(
            "Run RustWTM as administrator to change limits.",
        ));
    }
    ui.add_space(6.0);

    struct Row<'a> {
        limit: &'a AppLimit,
        instances: usize,
        processes: usize,
        outside: usize,
        cpu: f32,
        mem: u64,
        /// Fullest budget (instance) as a share of its limit, 0..=1.
        cpu_fill: Option<f32>,
        mem_fill: Option<f32>,
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
                outside: groups.iter().map(|g| g.outside).sum(),
                cpu: groups.iter().map(|g| g.cpu_percent).sum(),
                mem: groups.iter().map(|g| g.memory_bytes).sum(),
                cpu_fill: l.cpu_percent(cpus).filter(|&c| c > 0.0).map(|limit| {
                    groups
                        .iter()
                        .map(|g| g.cpu_percent / limit)
                        .fold(0.0, f32::max)
                }),
                mem_fill: l.memory_bytes().filter(|&m| m > 0).map(|limit| {
                    groups
                        .iter()
                        .map(|g| g.memory_bytes as f32 / limit as f32)
                        .fold(0.0, f32::max)
                }),
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
                (crate::i18n::tr("App"), false),
                (crate::i18n::tr("Budget"), true),
                (crate::i18n::tr("Applies to"), false),
                (crate::i18n::tr("Processes"), true),
                (crate::i18n::tr("CPU now"), true),
                (crate::i18n::tr("Memory now"), true),
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
                    budget.push_str(&{
                        crate::i18n::format(" ({0}% of machine)", &[format!("{:.1}", p)])
                    });
                }
                ui.label(budget);
                ui.label(scope_label(l.scope));
                let count = match l.scope {
                    LimitScope::Each if r.instances > 0 => {
                        let __wtm_i18n_0 = &(r.processes);
                        let __wtm_i18n_1 = &(r.instances);
                        crate::i18n::format(
                            "{0} in {1} instances",
                            &[format!("{}", __wtm_i18n_0), format!("{}", __wtm_i18n_1)],
                        )
                    }
                    _ => r.processes.to_string(),
                };
                if r.outside > 0 {
                    ui.label({
                        let __wtm_i18n_0 = &(r.outside);
                        crate::i18n::format(
                            "{0} ({1} outside)",
                            &[count.to_string(), format!("{}", __wtm_i18n_0)],
                        )
                    })
                    .on_hover_text(crate::i18n::tr(
                        "These processes run in a job object of their own (a sandbox or a build \
                         tool) and Windows does not let them share the budget. They join it once \
                         the app is restarted.",
                    ));
                } else {
                    ui.label(count);
                }
                let cores = {
                    let __wtm_i18n_0 = &(r.cpu / 100.0 * cpus as f32);
                    crate::i18n::format("{0} cores", &[format!("{:.2}", __wtm_i18n_0)])
                };
                usage_cell(ui, cores, r.cpu_fill);
                usage_cell(ui, format_bytes(r.mem as f64), r.mem_fill);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(admin, egui::Button::new(crate::i18n::tr("Edit")))
                        .clicked()
                    {
                        view.edit(l);
                    }
                    if ui
                        .add_enabled(admin, egui::Button::new(crate::i18n::tr("Remove")))
                        .clicked()
                    {
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
        ui.weak(crate::i18n::tr("No app limits yet. Add one below, or right-click a process and choose \"Limit this app…\"."));
    }

    ui.add_space(12.0);
    ui.heading(if view.editing.is_some() {
        crate::i18n::tr("Change limit")
    } else {
        crate::i18n::tr("Add limit")
    });
    egui::Grid::new("app_limit_form")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label(crate::i18n::tr("Program"));
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut view.name)
                        .hint_text(crate::i18n::tr("e.g. node.exe or chrome*.exe")),
                );
                egui::ComboBox::from_label(crate::i18n::tr("or a running app"))
                    .selected_text(crate::i18n::tr("Pick…"))
                    .height(400.0)
                    .show_ui(ui, |ui| {
                        for (name, cpu, mem) in running_apps(data) {
                            let label = {
                                let __wtm_i18n_0 = &(format_bytes(mem as f64));
                                crate::i18n::format(
                                    "{0}  ({1}% CPU, {2})",
                                    &[
                                        name.to_string(),
                                        format!("{:.1}", cpu),
                                        __wtm_i18n_0.to_string(),
                                    ],
                                )
                            };
                            if ui.selectable_label(view.name == name, label).clicked() {
                                view.name = name;
                            }
                        }
                    });
            });
            ui.end_row();

            ui.checkbox(&mut view.cpu, "CPU");
            ui.add_enabled(
                view.cpu,
                egui::DragValue::new(&mut view.cores)
                    .range(0.1..=cpus as f32)
                    .speed(0.05)
                    .max_decimals(2)
                    .suffix(crate::i18n::tr(" cores")),
            );
            ui.end_row();

            ui.checkbox(&mut view.memory, crate::i18n::tr("Memory"));
            ui.add_enabled(
                view.memory,
                egui::DragValue::new(&mut view.gb)
                    .range(0.1..=4096.0)
                    .speed(0.1)
                    .max_decimals(2)
                    .suffix(crate::i18n::tr(" GB")),
            );
            ui.end_row();

            ui.label(crate::i18n::tr("Applies to"));
            ui.horizontal(|ui| {
                ui.radio_value(
                    &mut view.scope,
                    LimitScope::Group,
                    crate::i18n::tr("All processes together"),
                )
                .on_hover_text(crate::i18n::tr(
                    "One shared budget for every process of the app and their children.",
                ));
                ui.radio_value(
                    &mut view.scope,
                    LimitScope::Each,
                    crate::i18n::tr("Each instance separately"),
                )
                .on_hover_text(crate::i18n::tr(
                    "Every top-level instance gets its own budget, shared with its children.",
                ));
            });
            ui.end_row();
        });
    let candidate = view.limit();
    let valid = !candidate.name.is_empty()
        && (candidate.cpu_cores.is_some() || candidate.memory_gb.is_some());
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                admin && valid,
                egui::Button::new(crate::i18n::tr("Save limit")),
            )
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
        if view.editing.is_some() && ui.button(crate::i18n::tr("Cancel")).clicked() {
            *view = LimitsView::default();
        }
    });
    if let Some(e) = &view.error {
        ui.colored_label(Color32::RED, e);
    }
    commands
}

/// Current use, as a bar against the budget when there is one. Red from 95%
/// (memory allocations start failing), orange from 75%.
fn usage_cell(ui: &mut Ui, text: String, fill: Option<f32>) {
    let Some(fill) = fill else {
        ui.label(text);
        return;
    };
    let color = if fill >= 0.95 {
        BAD
    } else if fill >= 0.75 {
        WARN
    } else {
        GOOD
    };
    ui.add(
        egui::ProgressBar::new(fill.clamp(0.0, 1.0))
            .desired_width(150.0)
            .fill(color.gamma_multiply(0.8))
            .text(format!("{text} ({:.0}%)", fill * 100.0)),
    )
    .on_hover_text(crate::i18n::tr(
        "Use of the budget; for one budget per instance, the fullest instance",
    ));
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

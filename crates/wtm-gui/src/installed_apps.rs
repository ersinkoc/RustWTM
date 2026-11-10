//! Desktop application management with a bounded background inventory.
use eframe::egui::{self, Ui};
use egui_extras::Column;
use std::sync::mpsc::{self, Receiver};
use wtm_core::installed::{AppId, InstalledApp, LaunchPlan, Report, Scope};

#[derive(Debug)]
enum Outcome {
    Inventory(Report),
    Uninstall(Result<(), String>),
}

#[derive(Default)]
pub struct View {
    report: Option<Result<Report, String>>,
    worker: Option<Receiver<Outcome>>,
    query: String,
    scope: u8,
    selected: Option<AppId>,
    confirm: Option<(InstalledApp, LaunchPlan)>,
    message: Option<String>,
    #[cfg(test)]
    requested: Option<InstalledApp>,
}

fn scope_label(scope: Scope) -> &'static str {
    crate::i18n::tr(match scope {
        Scope::User => "Current user",
        Scope::Machine => "All users",
    })
}

impl View {
    fn start(&mut self, ctx: &egui::Context, app: Option<InstalledApp>) {
        if self.worker.is_some() {
            return;
        }
        self.message = None;
        self.confirm = None;
        #[cfg(test)]
        {
            let _ = ctx;
            if let Some(app) = app {
                self.requested = Some(app);
            } else {
                self.report = Some(Ok(Report::default()));
            }
        }
        #[cfg(not(test))]
        {
            let (tx, rx) = mpsc::sync_channel(1);
            let ctx = ctx.clone();
            match std::thread::Builder::new()
                .name("installed-apps".into())
                .spawn(move || {
                    let outcome = if let Some(app) = app {
                        Outcome::Uninstall(wtm_core::installed::uninstall(&app))
                    } else {
                        Outcome::Inventory(wtm_core::installed::list())
                    };
                    let _ = tx.send(outcome);
                    ctx.request_repaint();
                }) {
                Ok(_) => self.worker = Some(rx),
                Err(e) => self.message = Some(e.to_string()),
            }
        }
    }

    fn poll(&mut self) {
        let Some(rx) = &self.worker else {
            return;
        };
        match rx.try_recv() {
            Ok(outcome) => {
                self.worker = None;
                match outcome {
                    Outcome::Inventory(report) => self.report = Some(Ok(report)),
                    Outcome::Uninstall(result) => self.message = Some(match result {
                        Ok(()) => crate::i18n::tr("Uninstaller started. Refresh after it finishes; removal is not yet confirmed.").into(),
                        Err(e) => crate::i18n::format("Cannot start uninstaller: {0}", &[crate::i18n::tr(&e).to_owned()]),
                    }),
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.worker = None;
                self.report = Some(Err(crate::i18n::tr(
                    "Application inventory worker stopped.",
                )
                .into()));
            }
        }
    }

    /// Returns a request to open the fixed Windows Apps settings URI.
    pub fn show(&mut self, ui: &mut Ui) -> bool {
        self.poll();
        if self.report.is_none() && self.worker.is_none() && self.message.is_none() {
            self.start(ui.ctx(), None);
        }
        let mut settings = false;
        let mut refresh = false;
        ui.horizontal_wrapped(|ui| {
            let label = ui.label(crate::i18n::tr("Search:"));
            ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .char_limit(256)
                    .desired_width(250.0)
                    .hint_text(crate::i18n::tr("Name, publisher or version")),
            )
            .labelled_by(label.id);
            refresh = ui
                .add_enabled(
                    self.worker.is_none(),
                    egui::Button::new(crate::i18n::tr("Refresh")),
                )
                .clicked();
            settings = ui
                .button(crate::i18n::tr("Windows Apps settings"))
                .clicked();
        });
        ui.horizontal_wrapped(|ui| {
            for (value, label) in [(0, "All apps"), (1, "Current user"), (2, "All users")] {
                crate::theme::choice(ui, &mut self.scope, value, crate::i18n::tr(label));
            }
        });
        if refresh {
            self.start(ui.ctx(), None);
        }
        if self.worker.is_some() {
            ui.weak(crate::i18n::tr(
                "Reading or updating application information…",
            ));
        }
        if let Some(message) = &self.message {
            ui.label(message);
        }
        ui.weak(crate::i18n::tr("Registered desktop apps only. Store and portable apps are not included yet. Sizes are publisher estimates."));
        if let Some(Err(error)) = &self.report {
            ui.label(error);
        }
        let Some(Ok(report)) = &self.report else {
            return settings;
        };
        if report.partial {
            ui.colored_label(crate::theme::text_color(ui, crate::theme::WARN), crate::i18n::tr("Partial inventory: some entries could not be read or a scan limit was reached."));
        }
        let query = self.query.trim().to_lowercase();
        let mut rows: Vec<_> = report
            .apps
            .iter()
            .filter(|app| {
                (self.scope == 0 || (self.scope == 1) == (app.id.scope == Scope::User))
                    && format!("{} {} {}", app.name, app.publisher, app.version)
                        .to_lowercase()
                        .contains(&query)
            })
            .collect();
        ui.weak(crate::i18n::format(
            "{0} of {1} apps",
            &[rows.len().to_string(), report.apps.len().to_string()],
        ));
        let selected = self
            .selected
            .as_ref()
            .and_then(|id| report.apps.iter().find(|app| &app.id == id))
            .cloned();
        if let Some(app) = &selected {
            crate::theme::panel(ui, |ui| {
                ui.strong(&app.name);
                ui.horizontal_wrapped(|ui| {
                    ui.label(&app.publisher);
                    ui.weak(&app.version);
                    ui.weak(scope_label(app.id.scope));
                });
                if !app.location.is_empty() {
                    ui.label(&app.location);
                }
                let plan = wtm_core::installed::plan(app);
                if ui
                    .add_enabled(
                        self.worker.is_none() && plan.is_ok(),
                        egui::Button::new(crate::i18n::tr("Uninstall…")),
                    )
                    .clicked()
                    && let Ok(plan) = plan.as_ref()
                {
                    self.confirm = Some((app.clone(), plan.clone()));
                }
                if let Err(reason) = plan {
                    ui.weak(crate::i18n::tr(reason));
                }
            });
        }
        if rows.is_empty() {
            ui.strong(crate::i18n::tr("No matching applications"));
            return settings;
        }
        let mut sort =
            crate::sort::Sort::load(ui, "installed_apps", crate::sort::Sort::new(0, false));
        // Cell text belongs to the row action; selectable labels consume clicks.
        ui.style_mut().interaction.selectable_labels = false;
        sort.apply(&mut rows, |a, b, col| match col {
            1 => crate::sort::text(&a.publisher, &b.publisher),
            2 => crate::sort::text(&a.version, &b.version),
            3 => a.estimated_bytes.cmp(&b.estimated_bytes),
            4 => scope_label(a.id.scope).cmp(scope_label(b.id.scope)),
            _ => crate::sort::text(&a.name, &b.name),
        });
        crate::theme::table_panel(ui, "installed_apps", 850.0, |ui| {
            let table_height = (ui.ctx().content_rect().height() * 0.52).clamp(160.0, 600.0);
            crate::theme::table(ui)
                .max_scroll_height(table_height)
                .min_scrolled_height(0.0)
                .sense(egui::Sense::click())
                .striped(true)
                .resizable(true)
                .column(Column::initial(300.0).at_least(180.0).clip(true))
                .column(Column::initial(200.0).clip(true))
                .column(Column::initial(120.0).clip(true))
                .column(Column::initial(110.0).clip(true))
                .column(Column::remainder().at_least(100.0).clip(true))
                .header(32.0, |mut header| {
                    for (i, label) in ["Name", "Publisher", "Version", "Estimated size", "Scope"]
                        .iter()
                        .enumerate()
                    {
                        header.col(|ui| {
                            crate::theme::table_cell(ui, |ui| {
                                crate::sort::header(ui, label, i, i == 3, &mut sort)
                            })
                        });
                    }
                })
                .body(|body| {
                    body.rows(32.0, rows.len(), |mut row| {
                        let app = rows[row.index()];
                        row.set_selected(self.selected.as_ref() == Some(&app.id));
                        let size = app
                            .estimated_bytes
                            .map(|n| wtm_core::format_bytes(n as f64))
                            .unwrap_or_else(|| "—".into());
                        row.col(|ui| {
                            crate::theme::table_cell(ui, |ui| {
                                let response = ui
                                    .add_sized(
                                        [ui.available_width(), 28.0],
                                        egui::Button::new("")
                                            .left_text(&app.name)
                                            .frame(false)
                                            .truncate(),
                                    )
                                    .on_hover_text(&app.name);
                                response.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::Role::Button,
                                        true,
                                        self.selected.as_ref() == Some(&app.id),
                                        &app.name,
                                    )
                                });
                                if response.clicked() {
                                    self.selected = Some(app.id.clone());
                                }
                            })
                        });
                        for text in [
                            app.publisher.as_str(),
                            app.version.as_str(),
                            size.as_str(),
                            scope_label(app.id.scope),
                        ] {
                            row.col(|ui| {
                                crate::theme::table_cell(ui, |ui| {
                                    ui.add(egui::Label::new(text).truncate())
                                        .on_hover_text(text);
                                })
                            });
                        }
                        if row.response().clicked() {
                            self.selected = Some(app.id.clone());
                        }
                    })
                });
        });
        sort.store(ui, "installed_apps");
        settings
    }

    pub fn dialog(&mut self, ctx: &egui::Context) {
        let Some((app, plan)) = self.confirm.clone() else {
            return;
        };
        let mut start = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::unique("uninstall_app")).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width()-64.0).clamp(200.0,520.0));
            ui.heading(crate::i18n::tr("Uninstall application?"));
            ui.strong(&app.name);
            ui.label(format!("{} · {} · {}",app.publisher,app.version,scope_label(app.id.scope)));
            ui.label(crate::i18n::tr("This opens the registered uninstaller. It may ask for administrator rights. Review its prompts before continuing."));
            ui.label(&plan.executable);
            if !plan.arguments.is_empty() { ui.label(&plan.arguments); }
            ui.horizontal_wrapped(|ui| {
                start = ui.button(crate::i18n::tr("Start uninstaller")).clicked();
                cancel = ui.button(crate::i18n::tr("Cancel")).clicked();
            });
        });
        if start {
            self.start(ctx, Some(app));
        } else if cancel || modal.should_close() {
            self.confirm = None;
        }
    }
}

#[cfg(test)]
pub fn fixture() -> View {
    let apps = [
        ("Example Editor", "Example Labs", "2.4", Scope::User),
        ("Example Tools", "Utility Maker", "1.8", Scope::Machine),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (name, publisher, version, scope))| InstalledApp {
        id: AppId {
            scope,
            view32: false,
            key: format!("example-{i}"),
        },
        name: name.into(),
        publisher: publisher.into(),
        version: version.into(),
        estimated_bytes: Some((i + 1) as u64 * 256 * 1024 * 1024),
        location: r"C:\Program Files\Example".into(),
        uninstall: r#""C:\Program Files\Example\uninstall.exe""#.into(),
        msi: false,
        no_remove: false,
    })
    .collect();
    View {
        report: Some(Ok(Report {
            apps,
            partial: false,
        })),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    #[test]
    fn filters_selection_and_confirmation_do_not_uninstall_until_confirmed() {
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([1000.0, 800.0])
            .build_ui_state(
                |ui, v: &mut View| {
                    v.show(ui);
                    v.dialog(ui.ctx());
                },
                fixture(),
            );
        h.run_steps(2);
        h.get_by_label("Example Editor").click();
        h.run_steps(2);
        assert!(
            h.state().selected.is_some(),
            "clicking an application must select its stable registration"
        );
        h.get_by_label("Uninstall…").click();
        h.run_steps(2);
        assert!(h.state().requested.is_none());
        h.get_by_label("Cancel").click();
        h.run_steps(2);
        assert!(h.state().requested.is_none());
        h.get_by_label("Uninstall…").click();
        h.run_steps(2);
        h.get_by_label("Start uninstaller").click();
        h.run_steps(2);
        assert_eq!(h.state().requested.as_ref().unwrap().name, "Example Editor");
        h.state_mut().query = "utility".into();
        h.run_steps(2);
        assert!(h.query_by_label("Example Tools").is_some());
        h.state_mut().scope = 1;
        h.run_steps(2);
        assert!(h.query_by_label("No matching applications").is_some());
    }
    #[test]
    fn background_outcomes_keep_failure_and_handoff_distinct() {
        let (mut tx, rx) = {
            let (tx, rx) = mpsc::sync_channel(1);
            (Some(tx), rx)
        };
        let mut view = View {
            worker: Some(rx),
            ..fixture()
        };
        tx.take()
            .unwrap()
            .send(Outcome::Uninstall(Err("cancelled".into())))
            .unwrap();
        view.poll();
        assert!(view.message.unwrap().contains("cancelled"));
        let (tx, rx) = mpsc::sync_channel(1);
        view = View {
            worker: Some(rx),
            ..fixture()
        };
        tx.send(Outcome::Inventory(Report::default())).unwrap();
        view.poll();
        assert_eq!(view.report.unwrap().unwrap().apps.len(), 0);
    }
}

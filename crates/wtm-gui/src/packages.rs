//! Exact package identities and reviewable installer requests.
use eframe::egui::{self, Ui};
use std::collections::BTreeSet;
use wtm_core::packages::{Inventory, UpdatePlan, UpdateResult};
#[derive(Default)]
pub struct View {
    scan: crate::background::Task<Inventory>,
    update: crate::background::Task<Vec<UpdateResult>>,
    report: Option<Result<Inventory, String>>,
    query: String,
    selected: BTreeSet<(String, String)>,
    confirm: Option<UpdatePlan>,
    results: Option<Result<Vec<UpdateResult>, String>>,
    error: Option<String>,
}
impl View {
    pub fn show(&mut self, ui: &mut Ui, store: bool) -> bool {
        if let Some(r) = self.scan.poll() {
            self.report = Some(r);
            self.selected.clear();
        }
        if let Some(r) = self.update.poll() {
            self.results = Some(r);
            self.selected.clear();
        }
        let busy = self.scan.busy() || self.update.busy();
        let mut windows = false;
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .char_limit(256)
                    .hint_text(crate::i18n::tr("Name, publisher or package ID"))
                    .desired_width(280.0),
            );
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(crate::i18n::tr("Check applications and updates")),
                )
                .clicked()
            {
                self.scan.start(ui.ctx(), wtm_core::packages::inventory);
            }
            windows = ui
                .button(crate::i18n::tr("Windows Apps settings"))
                .clicked();
            if busy {
                ui.spinner();
            }
        });
        if self.report.is_none() && !busy {
            self.scan.start(ui.ctx(), wtm_core::packages::inventory);
        }
        ui.weak(crate::i18n::tr(if store {"Current-user Store registrations. Windows manages package removal."}else{"Packages are matched by exact source and ID. Registry app names are never guessed into updates."}));
        if self.update.busy() {
            ui.strong(crate::i18n::tr(
                "Updating selected packages. Installer prompts remain visible.",
            ));
        }
        if let Some(result) = &self.report {
            match result {
                Err(e) => {
                    crate::theme::colored(ui, crate::theme::WARN, e);
                }
                Ok(report) => {
                    for warning in &report.warnings {
                        ui.weak(warning);
                    }
                    let query = self.query.to_lowercase();
                    if store {
                        if report.store.is_empty() {
                            ui.weak(crate::i18n::tr("No Store packages were reported."));
                        }
                        for app in report.store.iter().filter(|p| {
                            format!("{} {} {}", p.name, p.publisher, p.id)
                                .to_lowercase()
                                .contains(&query)
                        }) {
                            egui::CollapsingHeader::new(format!(
                                "{}  ·  {}",
                                app.name, app.version
                            ))
                            .id_salt(&app.id)
                            .show(ui, |ui| {
                                crate::theme::panel(ui, |ui| {
                                    ui.label(&app.publisher);
                                    ui.monospace(&app.id);
                                    ui.label(&app.location);
                                    if !app.removable {
                                        ui.weak(crate::i18n::tr(
                                            "Windows marks this package as non-removable.",
                                        ));
                                    }
                                });
                            });
                        }
                    } else {
                        let updates: Vec<_> = report
                            .packages
                            .iter()
                            .filter(|p| p.target.is_some())
                            .collect();
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(crate::i18n::format(
                                "{0} updates available",
                                &[updates.len().to_string()],
                            ));
                            if ui
                                .add_enabled(
                                    !busy && !self.selected.is_empty(),
                                    egui::Button::new(crate::i18n::tr("Review selected updates")),
                                )
                                .clicked()
                            {
                                let selected: Vec<_> = updates
                                    .iter()
                                    .filter(|p| {
                                        self.selected.contains(&(p.source.clone(), p.id.clone()))
                                    })
                                    .map(|p| (*p).clone())
                                    .collect();
                                match UpdatePlan::new(&selected) {
                                    Ok(plan) => {
                                        self.confirm = Some(plan);
                                        self.error = None
                                    }
                                    Err(e) => self.error = Some(e),
                                }
                            }
                        });
                        if let Some(e) = &self.error {
                            crate::theme::colored(ui, crate::theme::WARN, e);
                        }
                        if updates.is_empty() {
                            ui.weak(crate::i18n::tr("No package updates were reported. See provider availability above."));
                        }
                        for p in updates.into_iter().filter(|p| {
                            format!("{} {} {}", p.name, p.id, p.source)
                                .to_lowercase()
                                .contains(&query)
                        }) {
                            let key = (p.source.clone(), p.id.clone());
                            let mut selected = self.selected.contains(&key);
                            crate::theme::panel(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    let supported =
                                        matches!(p.source.as_str(), "winget" | "msstore");
                                    if ui
                                        .add_enabled(
                                            !busy && supported,
                                            egui::Checkbox::new(&mut selected, &p.name),
                                        )
                                        .changed()
                                    {
                                        if selected {
                                            self.selected.insert(key.clone());
                                        } else {
                                            self.selected.remove(&key);
                                        }
                                    }
                                    ui.label(format!(
                                        "{} → {}",
                                        p.installed,
                                        p.target.as_deref().unwrap_or("—")
                                    ));
                                    ui.weak(format!("{} · {}", p.source, p.id));
                                });
                            });
                        }
                    }
                }
            }
        } else {
            crate::theme::loading(ui, crate::i18n::tr("Loading applications…"));
        }
        if let Some(result) = &self.results {
            match result {
                Err(e) => {
                    crate::theme::colored(ui, crate::theme::WARN, e);
                    ui.weak(crate::i18n::tr("An interrupted update may still have an installer running. Refresh before retrying."));
                }
                Ok(results) => {
                    for r in results {
                        ui.label(format!("{} · {}: {}", r.id, r.status, r.message));
                    }
                    ui.weak(crate::i18n::tr("Refresh to confirm installed versions; an installer result alone is not inventory verification."));
                }
            }
        }
        if let Some(plan) = self.confirm.clone() {
            let mut run = false;
            let mut cancel = false;
            let modal=egui::Modal::new(egui::Id::unique("package-update-review")).show(ui.ctx(),|ui|{
    ui.set_max_width(620.0);ui.heading(crate::i18n::tr("Review package updates"));
    ui.label(crate::i18n::tr("Only these package IDs and target versions will be requested. Each package is checked again before its installer starts."));
    egui::ScrollArea::vertical().max_height(280.0).show(ui,|ui|{for p in &plan.packages{ui.label(format!("{} · {}\n{}: {} → {}",p.name,p.source,p.id,p.installed,p.target.as_deref().unwrap_or("—")));crate::theme::divider(ui);}});
    ui.horizontal(|ui|{run=crate::theme::primary_action(ui,!busy,crate::i18n::tr("Start selected updates")).clicked();cancel=ui.button(crate::i18n::tr("Cancel")).clicked();});
   });
            if run {
                self.confirm = None;
                self.results = None;
                self.update
                    .start(ui.ctx(), move || wtm_core::packages::update(&plan));
            } else if cancel || modal.should_close() {
                self.confirm = None;
            }
        }
        windows
    }
}
#[cfg(test)]
impl View {
    pub fn fixture(review: bool) -> Self {
        let package = wtm_core::packages::Package {
            id: "Example.Editor".into(),
            source: "winget".into(),
            name: "Example Editor".into(),
            installed: "2.4.0".into(),
            target: Some("2.5.0".into()),
        };
        let report = Inventory {
            packages: vec![package.clone()],
            store: vec![wtm_core::packages::StoreApp {
                id: "Example.Notes_3.0_x64__fixture".into(),
                family: "Example.Notes_fixture".into(),
                name: "Example Notes".into(),
                publisher: "Example Publisher".into(),
                version: "3.0".into(),
                location: "C:\\Program Files\\WindowsApps\\Example.Notes".into(),
                removable: true,
            }],
            warnings: vec![],
        };
        Self {
            report: Some(Ok(report)),
            selected: BTreeSet::from([("winget".into(), "Example.Editor".into())]),
            confirm: review.then(|| UpdatePlan::new(&[package]).unwrap()),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    #[test]
    fn cancelling_package_review_never_starts_an_update() {
        let mut v = View {
            report: Some(Ok(Inventory::default())),
            ..Default::default()
        };
        v.confirm = Some(
            UpdatePlan::new(&[wtm_core::packages::Package {
                id: "Example.Editor".into(),
                source: "winget".into(),
                name: "Editor".into(),
                installed: "1".into(),
                target: Some("2".into()),
            }])
            .unwrap(),
        );
        let mut h = egui_kittest::Harness::builder()
            .allow_missing_glyphs()
            .with_size([1000.0, 800.0])
            .build_ui_state(
                |ui, v: &mut View| {
                    v.show(ui, false);
                },
                v,
            );
        h.run();
        h.get_by_label("Cancel").click();
        h.run();
        assert!(h.state().confirm.is_none());
        assert!(!h.state().update.busy());
    }
}

//! File-centric diagnostics; results are a snapshot, not an assertion a file is free.
use eframe::egui::{self, Ui};
use wtm_core::control::ProcessRef;
#[derive(Default)]
pub struct View {
    path: String,
    task: crate::background::Task<wtm_sys::file_locks::Report>,
    result: Option<Result<wtm_sys::file_locks::Report, String>>,
}
impl View {
    pub fn dropped(&mut self, path: &std::path::Path, ctx: &egui::Context) {
        self.path = path.to_string_lossy().into_owned();
        self.start(ctx);
    }
    fn start(&mut self, ctx: &egui::Context) {
        if !self.task.busy() {
            self.result = None;
            let path = std::path::PathBuf::from(&self.path);
            self.task
                .start(ctx, move || wtm_sys::file_locks::scan(&path));
        }
    }
    pub fn show(&mut self, ui: &mut Ui) -> Option<ProcessRef> {
        if let Some(result) = self.task.poll() {
            self.result = Some(result)
        }
        ui.weak(crate::i18n::tr(
            "Drop a local file or folder, or enter its full path. This query never closes handles.",
        ));
        let mut start = false;
        ui.horizontal_wrapped(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.path)
                    .desired_width(ui.available_width().min(520.0))
                    .char_limit(2048)
                    .hint_text(crate::i18n::tr("Full file or folder path")),
            );
            start = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            start |= ui
                .add_enabled(
                    !self.task.busy() && !self.path.trim().is_empty(),
                    egui::Button::new(crate::i18n::tr("Find file owners")),
                )
                .clicked();
        });
        if start {
            self.start(ui.ctx());
        }
        if self.task.busy() {
            ui.spinner();
        }
        let mut selected = None;
        if let Some(result) = &self.result {
            match result {
                Err(e) => {
                    crate::theme::colored(ui, crate::theme::WARN, e);
                }
                Ok(report) => {
                    crate::theme::card(ui, crate::i18n::tr("File owners"), |ui| {
                        let path = ui.label(&report.path);
                        crate::context_actions::copy_text(
                            &path,
                            ("file-owner-path", &report.path),
                            crate::i18n::tr("Path"),
                            &report.path,
                        );
                        ui.weak(crate::i18n::format(
                            "{0} files checked",
                            &[report.files_checked.to_string()],
                        ));
                        for warning in &report.warnings {
                            crate::theme::colored(ui, crate::theme::WARN, warning);
                        }
                        if report.lockers.is_empty() {
                            ui.label(crate::i18n::tr("No owners were reported. Protected processes, directories and mappings may not be reported by Windows."));
                        }
                        for locker in &report.lockers {
                            ui.horizontal_wrapped(|ui| {
                                let name = ui.strong(&locker.name).interact(egui::Sense::click());
                                crate::context_actions::menu(
                                    &name,
                                    ("file-owner", locker.pid, locker.create_time),
                                    |ui| {
                                        crate::context_actions::copy(ui, "PID", || {
                                            locker.pid.to_string()
                                        });
                                        if crate::context_actions::action(
                                            ui,
                                            crate::i18n::tr("Inspect process"),
                                        ) {
                                            selected = Some(ProcessRef {
                                                pid: locker.pid,
                                                create_time: locker.create_time,
                                                name: locker.name.clone(),
                                            });
                                        }
                                    },
                                );
                                ui.weak(format!("PID {}", locker.pid));
                                if !locker.service.is_empty() {
                                    ui.label(&locker.service);
                                }
                                if locker.reason == wtm_sys::file_locks::Hold::WorkingDirectory {
                                    ui.weak(crate::i18n::tr("Working directory")).on_hover_text(
                                        crate::i18n::tr(
                                            "It works in this folder or one inside it, so the folder cannot be removed or renamed until it moves elsewhere or ends.",
                                        ),
                                    );
                                }
                                if ui.button(crate::i18n::tr("Inspect process")).clicked() {
                                    selected = Some(ProcessRef {
                                        pid: locker.pid,
                                        create_time: locker.create_time,
                                        name: locker.name.clone(),
                                    });
                                }
                            });
                        }
                    });
                }
            }
        }
        selected
    }
}

#[cfg(test)]
impl View {
    pub fn fixture() -> Self {
        Self {
            path: r"C:\Example\report.xlsx".into(),
            result: Some(Ok(wtm_sys::file_locks::Report {
                path: r"C:\Example\report.xlsx".into(),
                files_checked: 1,
                partial: false,
                warnings: vec![],
                lockers: vec![wtm_sys::file_locks::Locker {
                    pid: 100,
                    create_time: 1000,
                    name: "Example spreadsheet".into(),
                    service: String::new(),
                    reason: wtm_sys::file_locks::Hold::OpenFile,
                }],
            })),
            ..Default::default()
        }
    }
}

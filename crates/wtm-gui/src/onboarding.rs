//! A version-aware, transactional setup for local window preferences.
use crate::{
    experience,
    i18n::{self, Language},
    numeric_tray,
    preferences::{Density, Display},
    processes::ViewMode,
    theme::{self, TableLines},
    workspace_inspector::Mode,
};
use eframe::egui::{self, Ui};

#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub language: Language,
    pub theme: egui::ThemePreference,
    pub display: Display,
    pub mode: ViewMode,
    pub inspector: Mode,
    pub resource_strip: bool,
    pub descriptions: bool,
    pub lines: TableLines,
    pub numeric_tray: numeric_tray::Preferences,
    pub tray_on_close: bool,
    pub check_updates: bool,
    pub experience: experience::Preferences,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    FirstRun,
    Updated,
    Manual,
}

struct Session {
    version: String,
    reason: Reason,
    step: usize,
    return_to_review: bool,
    draft: Draft,
}

#[derive(Default)]
pub struct View {
    pub seen_version: Option<String>,
    pending: Option<(String, Reason)>,
    session: Option<Session>,
    pub save_pending: bool,
    pub error: Option<String>,
}

pub enum Action {
    Apply(Draft),
    Skip,
}

pub fn modal_id() -> egui::Id {
    egui::Id::unique("quick-setup")
}

impl View {
    pub fn schedule(&mut self, version: &str) {
        if self.seen_version.as_deref() != Some(version)
            && self.session.is_none()
            && self.pending.is_none()
        {
            self.pending = Some((
                version.into(),
                if self.seen_version.is_some() {
                    Reason::Updated
                } else {
                    Reason::FirstRun
                },
            ));
        }
    }

    pub fn request_manual(&mut self, version: &str) {
        if self.session.is_none() {
            self.pending = Some((version.into(), Reason::Manual));
        }
    }

    pub fn needs_draft(&self, foreground: bool) -> bool {
        foreground && self.pending.is_some() && self.session.is_none()
    }

    pub fn begin(&mut self, draft: Draft) {
        if let Some((version, reason)) = self.pending.take() {
            self.error = None;
            self.session = Some(Session {
                version,
                reason,
                step: 0,
                return_to_review: false,
                draft,
            });
        }
    }

    pub fn acknowledge(&mut self) {
        if let Some(session) = self.session.take() {
            self.seen_version = Some(session.version);
            self.save_pending = true;
            self.error = None;
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<Action> {
        let session = self.session.as_mut()?;
        let mut action = None;
        let bounds = ctx.viewport_rect();
        let width = (bounds.width() - 72.0).clamp(240.0, 660.0);
        // Keep the modal and navigation anchored while moving between steps.
        let body_height = (bounds.height() - 295.0).clamp(100.0, 420.0);
        let frame = egui::Frame::popup(&ctx.style_of(ctx.theme())).inner_margin(18);
        let modal = egui::Modal::new(modal_id()).frame(frame).show(ctx, |ui| {
            ui.set_width(width);
            ui.horizontal_wrapped(|ui| {
                ui.heading(i18n::tr(match session.reason {
                    Reason::FirstRun => "Welcome to RustWTM",
                    Reason::Updated => "Welcome back",
                    Reason::Manual => "Quick setup",
                }));
                ui.weak(format!("v{}", session.version));
            });
            ui.label(i18n::tr(
                "Choose your quick settings. You can change them later in Settings.",
            ));
            ui.add_space(8.0);
            ui.add(
                egui::ProgressBar::new((session.step + 1) as f32 / 5.0)
                    .desired_height(3.0)
                    .fill(theme::ACCENT),
            );
            ui.horizontal(|ui| {
                ui.strong(i18n::tr(step_name(session.step)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(i18n::format(
                        "Step {0} of {1}",
                        &[(session.step + 1).to_string(), "5".into()],
                    ));
                });
            });
            theme::divider(ui);
            // Reserve the warning's measured height from the body budget so
            // retry feedback stays visible without moving the navigation.
            let warning_top = ui.cursor().top();
            if let Some(error) = &self.error {
                theme::strip(ui, theme::WARN, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("setup-save-error")
                        .max_height((body_height * 0.4).min(72.0))
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            theme::colored(ui, theme::WARN, error);
                        });
                });
            }
            let warning_height = ui.cursor().top() - warning_top;
            let content_height = (body_height - warning_height).max(0.0);
            let previous_step = session.step;
            egui::ScrollArea::vertical()
                .id_salt(("setup-content", session.step))
                .max_height(content_height)
                .min_scrolled_height(content_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    body(ui, session);
                });
            if session.step != previous_step {
                self.error = None;
            }
            theme::divider(ui);
            let (back, skip, advance) = navigation(ui, session.step, session.return_to_review);
            if back {
                session.step -= 1;
                self.error = None;
            }
            if skip {
                action = Some(Action::Skip);
            }
            if advance {
                if session.return_to_review {
                    session.step = 4;
                    session.return_to_review = false;
                } else if session.step < 4 {
                    session.step += 1;
                } else {
                    action = Some(Action::Apply(session.draft.clone()));
                }
            }
        });
        if modal.should_close() {
            action = Some(Action::Skip);
        }
        action
    }
}

fn navigation(ui: &mut Ui, step: usize, return_to_review: bool) -> (bool, bool, bool) {
    let utility = |ui: &mut Ui| {
        let back = ui
            .add_enabled(step > 0, egui::Button::new(i18n::tr("Back")))
            .clicked();
        let skip = ui.button(i18n::tr("Skip setup")).clicked();
        (back, skip)
    };
    let primary = |ui: &mut Ui| {
        theme::primary_action(
            ui,
            true,
            i18n::tr(if return_to_review {
                "Review your settings"
            } else if step < 4 {
                "Next"
            } else {
                "Apply and finish"
            }),
        )
        .clicked()
    };
    if ui.available_width() >= 600.0 {
        let ((back, skip), advance) = egui::containers::Sides::new().show(ui, utility, primary);
        (back, skip, advance)
    } else {
        ui.horizontal_wrapped(|ui| {
            let (back, skip) = utility(ui);
            (back, skip, primary(ui))
        })
        .inner
    }
}

fn step_name(step: usize) -> &'static str {
    match step {
        0 => "Language",
        1 => "Display and accessibility",
        2 => "Process workspace",
        3 => "Window behavior",
        _ => "Review your settings",
    }
}

fn body(ui: &mut Ui, session: &mut Session) {
    let d = &mut session.draft;
    match session.step {
        0 => {
            ui.label(i18n::tr(
                "Understand your PC, inspect changes and keep important work responsive.",
            ));
            theme::setting_row(ui, i18n::tr("Language"), |ui, label| {
                egui::ComboBox::from_id_salt("setup-language")
                    .selected_text(d.language.name())
                    .show_ui(ui, |ui| {
                        for language in Language::ALL {
                            ui.selectable_value(&mut d.language, language, language.name());
                        }
                    })
                    .response
                    .labelled_by(label);
            });
            for (title, detail) in [
                (
                    "Explore devices",
                    "Inspect device identity, health and recent readings.",
                ),
                (
                    "Manage applications",
                    "Review installed apps and choose package updates.",
                ),
                (
                    "Review recorded activity",
                    "Inspect resource history and recorded protection actions.",
                ),
            ] {
                theme::panel(ui, |ui| {
                    ui.strong(i18n::tr(title));
                    ui.weak(i18n::tr(detail));
                });
            }
        }
        1 => {
            theme::setting_row(ui, i18n::tr("Theme"), |ui, _| {
                ui.horizontal_wrapped(|ui| {
                    for (value, label) in [
                        (egui::ThemePreference::System, "System"),
                        (egui::ThemePreference::Light, "Light"),
                        (egui::ThemePreference::Dark, "Dark"),
                    ] {
                        theme::choice(ui, &mut d.theme, value, i18n::tr(label));
                    }
                });
            });
            theme::setting_row(ui, i18n::tr("Interface scale"), |ui, label| {
                ui.add(egui::Slider::new(&mut d.display.scale, 75..=150).suffix("%"))
                    .labelled_by(label);
            });
            theme::setting_row(ui, i18n::tr("Control and row height"), |ui, _| {
                ui.horizontal_wrapped(|ui| {
                    for (value, label) in [
                        (Density::Compact, "Compact"),
                        (Density::Standard, "Standard"),
                        (Density::Comfortable, "Comfortable"),
                    ] {
                        theme::choice(ui, &mut d.display.density, value, i18n::tr(label));
                    }
                });
            });
            ui.checkbox(
                &mut d.display.reduced_motion,
                i18n::tr("Reduce interface animation"),
            );
        }
        2 => {
            theme::setting_row(ui, i18n::tr("Process workspace"), |ui, _| {
                ui.horizontal_wrapped(|ui| {
                    for (value, label) in [
                        (ViewMode::Apps, "Apps"),
                        (ViewMode::Tree, "Tree"),
                        (ViewMode::List, "List"),
                    ] {
                        theme::choice(ui, &mut d.mode, value, i18n::tr(label));
                    }
                });
            });
            theme::setting_row(ui, i18n::tr("Inspector"), |ui, _| {
                ui.horizontal_wrapped(|ui| {
                    for (value, label) in [
                        (Mode::Auto, "Adaptive inspector"),
                        (Mode::Always, "Always show inspector"),
                        (Mode::Hidden, "Hide inspector"),
                    ] {
                        theme::choice(ui, &mut d.inspector, value, i18n::tr(label));
                    }
                });
            });
            for (value, label) in [
                (&mut d.resource_strip, "Show resource cards above processes"),
                (&mut d.descriptions, "Show process descriptions"),
                (&mut d.lines.rows, "Row separators"),
                (&mut d.lines.columns, "Column separators"),
            ] {
                ui.checkbox(value, i18n::tr(label));
            }
        }
        3 => {
            for (value, label) in [
                (
                    &mut d.tray_on_close,
                    "Keep running in the tray when the window is closed",
                ),
                (&mut d.numeric_tray.cpu, "CPU usage number"),
                (&mut d.numeric_tray.memory, "Memory usage number"),
                (&mut d.check_updates, "Check for updates automatically"),
            ] {
                ui.checkbox(value, i18n::tr(label));
            }
            for (label, value, max) in [
                (
                    "Visible window refresh",
                    &mut d.experience.active_seconds,
                    5,
                ),
                ("Tray window refresh", &mut d.experience.tray_seconds, 60),
            ] {
                theme::setting_row(ui, i18n::tr(label), |ui, id| {
                    ui.add(egui::Slider::new(value, 1..=max).suffix(" s"))
                        .labelled_by(id);
                });
            }
        }
        _ => {
            let theme_name = i18n::tr(match d.theme {
                egui::ThemePreference::System => "System",
                egui::ThemePreference::Light => "Light",
                egui::ThemePreference::Dark => "Dark",
            });
            let density = i18n::tr(match d.display.density {
                Density::Compact => "Compact",
                Density::Standard => "Standard",
                Density::Comfortable => "Comfortable",
            });
            let mode = i18n::tr(match d.mode {
                ViewMode::Apps => "Apps",
                ViewMode::Tree => "Tree",
                ViewMode::List => "List",
            });
            let inspector = i18n::tr(match d.inspector {
                Mode::Auto => "Adaptive inspector",
                Mode::Always => "Always show inspector",
                Mode::Hidden => "Hide inspector",
            });
            let mut edit = None;
            if review_group(ui, 0, "Language", d.language.name(), &[]) {
                edit = Some(0);
            }
            if review_group(
                ui,
                1,
                "Display and accessibility",
                &format!("{theme_name} · {}% · {density}", d.display.scale),
                &[("Reduce interface animation", d.display.reduced_motion)],
            ) {
                edit = Some(1);
            }
            if review_group(
                ui,
                2,
                "Process workspace",
                &format!("{mode} · {inspector}"),
                &[
                    ("Show resource cards above processes", d.resource_strip),
                    ("Show process descriptions", d.descriptions),
                    ("Row separators", d.lines.rows),
                    ("Column separators", d.lines.columns),
                ],
            ) {
                edit = Some(2);
            }
            let refresh = format!(
                "{}: {} s · {}: {} s",
                i18n::tr("Visible window refresh"),
                d.experience.active_seconds,
                i18n::tr("Tray window refresh"),
                d.experience.tray_seconds
            );
            if review_group(
                ui,
                3,
                "Window behavior",
                &refresh,
                &[
                    (
                        "Keep running in the tray when the window is closed",
                        d.tray_on_close,
                    ),
                    ("CPU usage number", d.numeric_tray.cpu),
                    ("Memory usage number", d.numeric_tray.memory),
                    ("Check for updates automatically", d.check_updates),
                ],
            ) {
                edit = Some(3);
            }
            if let Some(step) = edit {
                session.step = step;
                session.return_to_review = true;
            }
        }
    }
}

/// Keep the final review readable; details remain inspectable before committing.
fn review_group(
    ui: &mut Ui,
    step: usize,
    title: &str,
    summary: &str,
    details: &[(&str, bool)],
) -> bool {
    let mut edit = false;
    ui.push_id(("setup-review", step), |ui| {
        theme::panel(ui, |ui| {
            egui::containers::Sides::new().show(
                ui,
                |ui| {
                    ui.strong(i18n::tr(title));
                },
                |ui| {
                    let response = ui.small_button(i18n::tr("Edit"));
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Button,
                            ui.is_enabled(),
                            format!("{}: {}", i18n::tr("Edit"), i18n::tr(title)),
                        )
                    });
                    edit = response.clicked();
                },
            );
            ui.label(summary);
            if !details.is_empty() {
                egui::CollapsingHeader::new(i18n::tr("Details")).show(ui, |ui| {
                    for (label, enabled) in details {
                        ui.horizontal_wrapped(|ui| {
                            ui.weak(i18n::tr(label));
                            ui.label(i18n::tr(if *enabled { "Enabled" } else { "Disabled" }));
                        });
                    }
                });
            }
        });
    });
    edit
}

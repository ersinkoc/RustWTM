//! About tab: product, project maintainer and official links.

use eframe::egui::{self, Color32, RichText, Ui};

pub const REPO: &str = wtm_core::project::PUBLIC_REPOSITORY;
const ACCENT: Color32 = Color32::from_rgb(222, 120, 60);

const HIGHLIGHTS: [(&str, &str); 6] = [
    (
        "Never stuck at 100% CPU",
        "A background governor eases off the heaviest apps step by step — priority, \
         efficiency mode, then hard caps — and gives everything back once the machine recovers.",
    ),
    (
        "Budgets per app",
        "Two cores for node.exe, 8 GB for every browser window: limits follow an app and \
         everything it starts, enforced by Windows job objects.",
    ),
    (
        "The app you use stays fast",
        "The window in front of you is never throttled, so protection never gets in your way.",
    ),
    (
        "Learns from history",
        "Per-minute usage is kept for weeks; chronic hogs and memory leaks are found and \
         handled automatically.",
    ),
    (
        "Stops what must not run",
        "Blocked programs never start; rules end, suspend or cap anything you describe.",
    ),
    (
        "Safe by design",
        "Dry-run first, core Windows processes are untouchable, and every change is rolled \
         back when the service stops.",
    ),
];

/// Returns whether the user asked to check for updates.
pub fn about(
    ui: &mut Ui,
    update: &crate::backend::UpdateState,
    frames: Option<crate::frames::Summary>,
) -> bool {
    use crate::backend::UpdateState;
    let mut check = false;
    let mut area = egui::ScrollArea::vertical().id_salt("about");
    if let Some(y) = crate::theme::take_scroll(ui, "about") {
        area = area.vertical_scroll_offset(y);
    }
    let out = area.show(ui, |ui| {
        ui.add_space(0.0);
        crate::theme::panel(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                // Drawn and uploaded once, not on every frame.
                let id = egui::Id::unique("about-logo-texture");
                let logo = ui
                    .data(|d| d.get_temp::<egui::TextureHandle>(id))
                    .unwrap_or_else(|| {
                        let texture = ui.ctx().load_texture(
                            "rustwtm-logo",
                            egui::ColorImage::from_rgba_unmultiplied(
                                [128, 128],
                                &crate::logo::rgba(128),
                            ),
                            egui::TextureOptions::LINEAR,
                        );
                        ui.data_mut(|d| d.insert_temp(id, texture.clone()));
                        texture
                    });
                let size = if ui.available_width() < 760.0 {
                    64.0
                } else {
                    88.0
                };
                ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(size, size)));
                ui.label(RichText::new("RustWTM").size(34.0).strong().color(ACCENT));
                ui.label(
                    RichText::new(crate::i18n::tr(
                        "The Windows task manager that keeps your PC responsive",
                    ))
                    .size(18.0),
                );
                ui.weak({
                    let __wtm_i18n_0 = &(env!("CARGO_PKG_VERSION"));
                    crate::i18n::format(
                        "Version {0}  ·  MIT license  ·  written in Rust",
                        &[__wtm_i18n_0.to_string()],
                    )
                });
                ui.horizontal_wrapped(|ui| {
                    let status = match update {
                        UpdateState::Unknown => String::new(),
                        UpdateState::Checking => crate::i18n::tr("Checking for updates…").into(),
                        UpdateState::UpToDate => {
                            crate::i18n::tr("You have the latest version.").into()
                        }
                        UpdateState::Available(r) => {
                            let __wtm_i18n_0 = &(r.version_text());
                            crate::i18n::format(
                                "Version {0} is available (see the bar at the top).",
                                std::slice::from_ref(__wtm_i18n_0),
                            )
                        }
                        UpdateState::Installing(r) => {
                            let __wtm_i18n_0 = &(r.version_text());
                            crate::i18n::format(
                                "Installing {0}…",
                                std::slice::from_ref(__wtm_i18n_0),
                            )
                        }
                        UpdateState::Failed(e) => {
                            crate::i18n::format("Update check failed: {0}", std::slice::from_ref(e))
                        }
                    };
                    // Centre the button and the status.
                    let width = 360.0_f32.min(ui.available_width());
                    ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
                    if ui
                        .add_enabled(
                            !matches!(update, UpdateState::Checking | UpdateState::Installing(_)),
                            egui::Button::new(crate::i18n::tr("Check for updates")),
                        )
                        .clicked()
                    {
                        check = true;
                    }
                    ui.weak(status);
                });
                ui.add_space(0.0);

                ui.label(
                    RichText::new(crate::i18n::tr("An oxog.net project."))
                        .size(18.0)
                        .strong(),
                );
                ui.label(crate::i18n::format(
                    "Maintained by {0}, {1}.",
                    &[
                        wtm_core::project::MAINTAINER.to_owned(),
                        wtm_core::project::LOCATION.to_owned(),
                    ],
                ));
                ui.horizontal_wrapped(|ui| {
                    ui.hyperlink_to(
                        RichText::new("rustwtm.com").size(16.0),
                        wtm_core::project::HOMEPAGE,
                    );
                    ui.label("·");
                    ui.hyperlink_to(RichText::new("github.com/rustwtm/rustwtm").size(16.0), REPO);
                    ui.label("·");
                    ui.hyperlink_to(
                        RichText::new("oxog.net").size(16.0),
                        wtm_core::project::ORGANIZATION_URL,
                    );
                });
            });
        });
        ui.add_space(0.0);
        crate::theme::divider(ui);

        let feature = |ui: &mut Ui, title: &str, text: &str| {
            crate::theme::panel(ui, |ui| {
                ui.set_min_height(82.0);
                ui.label(RichText::new(crate::i18n::tr(title)).strong().color(ACCENT));
                ui.add(egui::Label::new(crate::i18n::tr(text)).wrap());
            });
        };
        if ui.available_width() >= 760.0 {
            for pair in HIGHLIGHTS.chunks(2) {
                ui.columns(2, |columns| {
                    for (column, (title, text)) in columns.iter_mut().zip(pair) {
                        feature(column, title, text);
                    }
                });
            }
        } else {
            for (title, text) in HIGHLIGHTS {
                feature(ui, title, text);
            }
        }
        ui.add_space(0.0);
        crate::theme::divider(ui);

        ui.horizontal_wrapped(|ui| {
            ui.hyperlink_to(crate::i18n::tr("Releases"), format!("{REPO}/releases"));
            ui.label("·");
            ui.hyperlink_to(
                crate::i18n::tr("Report a problem or request a feature"),
                format!("{REPO}/issues"),
            );
            ui.label("·");
            ui.hyperlink_to(
                crate::i18n::tr("Configuration reference"),
                format!("{REPO}/blob/main/docs/configuration.md"),
            );
            ui.label("·");
            ui.hyperlink_to(
                crate::i18n::tr("License (MIT)"),
                format!("{REPO}/blob/main/LICENSE"),
            );
        });
        ui.add_space(0.0);
        ui.weak(crate::i18n::tr("MIT-licensed open source software."));
        if let Some(f) = frames {
            ui.add_space(0.0);
            ui.weak(frame_timing(f));
        }
    });
    crate::theme::more_below(ui, "about", &out, ui.visuals().panel_fill);
    check
}

/// How quickly the window draws, from the last few hundred frames.
fn frame_timing(f: crate::frames::Summary) -> String {
    let ms = |d: std::time::Duration| format!("{:.1}", d.as_secs_f64() * 1000.0);
    crate::i18n::format(
        "Window drawing: {0} ms on average, {1} ms for 95% of frames, {2} ms at worst",
        &[ms(f.average), ms(f.p95), ms(f.slowest)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn about_identifies_the_project_company_and_official_links() {
        let mut h = egui_kittest::Harness::builder()
            .allow_missing_glyphs()
            .with_size([1100.0, 900.0])
            .build_ui(|ui| {
                about(ui, &crate::backend::UpdateState::UpToDate, None);
            });
        h.run();
        assert!(h.query_by_label("An oxog.net project.").is_some());
        assert!(
            h.query_by_label("Maintained by OXOGNET OÜ, Estonia.")
                .is_some()
        );
        assert!(h.query_by_label("rustwtm.com").is_some());
        assert!(h.query_by_label("github.com/rustwtm/rustwtm").is_some());
        assert!(h.query_by_label("oxog.net").is_some());
        assert!(h.query_by_label_contains("Ersin").is_none());
        assert!(
            h.query_by_label_contains(env!("CARGO_PKG_VERSION"))
                .is_some()
        );
        assert!(h.query_by_label_contains("Budgets per app").is_some());
        assert!(h.query_by_label("You have the latest version.").is_some());
    }

    #[test]
    fn about_shows_where_the_update_check_stands() {
        use crate::backend::UpdateState;
        for (state, text) in [
            (
                UpdateState::Failed("offline".into()),
                "Update check failed: offline",
            ),
            (UpdateState::Checking, "Checking for updates…"),
        ] {
            let mut h = egui_kittest::Harness::builder()
                .allow_missing_glyphs()
                .with_size([1100.0, 900.0])
                .build_ui(|ui| {
                    about(ui, &state, None);
                });
            h.run();
            assert!(h.query_by_label(text).is_some(), "{text}");
        }
        let mut h = egui_kittest::Harness::builder()
            .allow_missing_glyphs()
            .build_ui(|ui| {
                about(ui, &UpdateState::Unknown, None);
            });
        h.run();
        assert!(h.query_by_label("Check for updates").is_some());
    }
}

//! About tab: what RustWTM is, who made it, and where to find it.

use eframe::egui::{self, Color32, RichText, Ui};

pub const REPO: &str = "https://github.com/ersinkoc/RustWTM";
const AUTHOR: &str = "Ersin KOÇ";
const AUTHOR_URL: &str = "https://github.com/ersinkoc";
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
pub fn about(ui: &mut Ui, update: &crate::backend::UpdateState) -> bool {
    use crate::backend::UpdateState;
    let mut check = false;
    egui::ScrollArea::vertical()
        .id_salt("about")
        .show(ui, |ui| {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                let logo = ui.ctx().load_texture(
                    "rustwtm-logo",
                    egui::ColorImage::from_rgba_unmultiplied([128, 128], &crate::logo::rgba(128)),
                    egui::TextureOptions::LINEAR,
                );
                ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(128.0, 128.0)));
                ui.label(RichText::new("RustWTM").size(44.0).strong().color(ACCENT));
                ui.label(
                    RichText::new("The Windows task manager that keeps your PC responsive")
                        .size(18.0),
                );
                ui.weak(format!(
                    "Version {}  ·  MIT license  ·  written in Rust",
                    env!("CARGO_PKG_VERSION")
                ));
                ui.horizontal(|ui| {
                    let status = match update {
                        UpdateState::Unknown => String::new(),
                        UpdateState::Checking => "Checking for updates…".into(),
                        UpdateState::UpToDate => "You have the latest version.".into(),
                        UpdateState::Available(r) => {
                            format!(
                                "Version {} is available (see the bar at the top).",
                                r.version_text()
                            )
                        }
                        UpdateState::Installing(r) => format!("Installing {}…", r.version_text()),
                        UpdateState::Failed(e) => format!("Update check failed: {e}"),
                    };
                    // Centre the button and the status.
                    let width = 360.0_f32.min(ui.available_width());
                    ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
                    if ui
                        .add_enabled(
                            !matches!(update, UpdateState::Checking | UpdateState::Installing(_)),
                            egui::Button::new("Check for updates"),
                        )
                        .clicked()
                    {
                        check = true;
                    }
                    ui.weak(status);
                });
                ui.add_space(14.0);

                ui.label(
                    RichText::new(format!("Crafted by {AUTHOR}"))
                        .size(22.0)
                        .strong(),
                );
                ui.label(
                    "RustWTM is the work of Ersin KOÇ, who set out to build the task manager \
                     Windows never had: one that does not just show you what is slowing the \
                     machine down, but quietly fixes it — fast, safe and free for everyone.",
                );
                ui.horizontal(|ui| {
                    // Centre the two links.
                    let width = 360.0;
                    ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
                    ui.hyperlink_to(RichText::new("★ Star RustWTM on GitHub").size(16.0), REPO);
                    ui.label("  ");
                    ui.hyperlink_to(
                        RichText::new(format!("{AUTHOR} on GitHub")).size(16.0),
                        AUTHOR_URL,
                    );
                });
            });
            ui.add_space(18.0);
            ui.separator();

            egui::Grid::new("about_highlights")
                .num_columns(2)
                .spacing([24.0, 12.0])
                .show(ui, |ui| {
                    for (i, (title, text)) in HIGHLIGHTS.iter().enumerate() {
                        ui.vertical(|ui| {
                            ui.set_max_width(420.0);
                            ui.label(RichText::new(*title).strong().color(ACCENT));
                            ui.label(*text);
                        });
                        if i % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
            ui.add_space(18.0);
            ui.separator();

            ui.horizontal_wrapped(|ui| {
                ui.hyperlink_to("Releases", format!("{REPO}/releases"));
                ui.label("·");
                ui.hyperlink_to(
                    "Report a problem or request a feature",
                    format!("{REPO}/issues"),
                );
                ui.label("·");
                ui.hyperlink_to(
                    "Configuration reference",
                    format!("{REPO}/blob/main/docs/configuration.md"),
                );
                ui.label("·");
                ui.hyperlink_to("License (MIT)", format!("{REPO}/blob/main/LICENSE"));
            });
            ui.add_space(6.0);
            ui.weak(format!(
                "© {AUTHOR}. Free and open source software. If RustWTM saves your day, a star on \
                 GitHub is the best way to say thanks."
            ));
        });
    check
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn about_credits_the_author_and_links_the_repository() {
        let mut h = egui_kittest::Harness::builder()
            .allow_missing_glyphs()
            .with_size([1100.0, 900.0])
            .build_ui(|ui| {
                about(ui, &crate::backend::UpdateState::UpToDate);
            });
        h.run();
        assert!(h.query_by_label("Crafted by Ersin KOÇ").is_some());
        assert!(h.query_by_label("★ Star RustWTM on GitHub").is_some());
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
                    about(ui, &state);
                });
            h.run();
            assert!(h.query_by_label(text).is_some(), "{text}");
        }
        let mut h = egui_kittest::Harness::builder()
            .allow_missing_glyphs()
            .build_ui(|ui| {
                about(ui, &UpdateState::Unknown);
            });
        h.run();
        assert!(h.query_by_label("Check for updates").is_some());
    }
}

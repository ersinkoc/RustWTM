//! Secondary-click actions bound to the identity of the displayed item.
use eframe::egui::{self, Ui};
use std::hash::Hash;

pub fn menu(
    response: &egui::Response,
    item: impl Hash + std::fmt::Debug,
    add: impl FnOnce(&mut Ui),
) {
    egui::Popup::context_menu(response)
        .id(egui::Id::unique("workspace-context-menu").with(item))
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.min(24.0);
            let height = (ui.ctx().content_rect().height() - 20.0).max(60.0);
            // Measure against the window so the popup can reposition around
            // its pointer anchor before resorting to scrolling.
            ui.set_max_height(height);
            egui::ScrollArea::vertical()
                .max_height(height)
                .show(ui, add);
        });
}

pub fn action(ui: &mut Ui, label: &str) -> bool {
    if ui.button(label).clicked() {
        ui.close();
        true
    } else {
        false
    }
}

pub fn copy(ui: &mut Ui, field: &str, value: impl FnOnce() -> String) {
    if action(ui, &crate::i18n::format("Copy {0}", &[field.to_owned()])) {
        ui.ctx().copy_text(value());
    }
}

pub fn copy_text(
    response: &egui::Response,
    item: impl Hash + std::fmt::Debug,
    field: &str,
    value: &str,
) {
    if !value.is_empty() {
        menu(&response.interact(egui::Sense::CLICK), item, |ui| {
            copy(ui, field, || value.to_owned())
        });
    }
}

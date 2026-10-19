// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

mod app;
mod backend;
mod processes;
mod settings;
mod tray;
#[cfg(test)]
mod ui_tests;
mod views;

use eframe::egui;

fn options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RustWTM")
            .with_inner_size([1240.0, 780.0])
            .with_min_inner_size([820.0, 500.0]),
        ..Default::default()
    }
}

fn main() -> eframe::Result {
    eframe::run_native(
        "RustWTM",
        options(),
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn window_options() {
        let viewport = super::options().viewport;
        assert_eq!(viewport.title.as_deref(), Some("RustWTM"));
        assert_eq!(viewport.inner_size, Some(eframe::egui::vec2(1240.0, 780.0)));
        assert_eq!(
            viewport.min_inner_size,
            Some(eframe::egui::vec2(820.0, 500.0))
        );
    }
}

// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

mod app;
mod backend;
mod processes;
mod tray;
mod views;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RustWTM")
            .with_inner_size([1240.0, 780.0])
            .with_min_inner_size([820.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RustWTM",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}

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

/// Windows runs `rustwtm.exe --blocked <program> [args]` instead of a program
/// the service blocks; returns that program's file name.
fn blocked_program(mut args: impl Iterator<Item = String>) -> Option<String> {
    if args.next().as_deref() != Some("--blocked") {
        return None;
    }
    let program = args.next().unwrap_or_default();
    let name = std::path::Path::new(&program)
        .file_name()
        .map_or(program.clone(), |n| n.to_string_lossy().into_owned());
    Some(name)
}

fn main() -> eframe::Result {
    if let Some(name) = blocked_program(std::env::args().skip(1)) {
        backend::register_app_id();
        backend::notify_blocked(&name);
        std::process::exit(1);
    }
    eframe::run_native(
        "RustWTM",
        options(),
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    use super::blocked_program;

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn recognizes_blocked_launches() {
        assert_eq!(blocked_program(args(&[])), None);
        assert_eq!(blocked_program(args(&["--other"])), None);
        assert_eq!(
            blocked_program(args(&["--blocked", r"C:\Tools\xmrig.exe", "--donate"])).as_deref(),
            Some("xmrig.exe")
        );
        assert_eq!(blocked_program(args(&["--blocked"])).as_deref(), Some(""));
    }

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

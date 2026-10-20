// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

mod about;
mod alert;
mod app;
mod backend;
mod kill_tab;
mod limits_tab;
mod logo;
mod overview;
mod ports_tab;
mod processes;
mod settings;
mod sort;
mod telegram_settings;
mod theme;
mod tray;
#[cfg(test)]
mod ui_tests;
mod views;

use eframe::egui;

/// `visible: false` starts hidden in the tray (`--background`, used at sign-in).
fn options(visible: bool) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RustWTM")
            .with_visible(visible)
            .with_inner_size([1240.0, 780.0])
            .with_min_inner_size([820.0, 500.0])
            .with_icon(std::sync::Arc::new(egui::IconData {
                rgba: logo::rgba(64),
                width: 64,
                height: 64,
            })),
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let background = args.iter().any(|a| a == "--background");
    // One RustWTM per user: a second start shows the first one's window
    // (`--new-instance` starts another one anyway).
    if !args.iter().any(|a| a == "--new-instance")
        && let Some((window, _)) = wtm_core::window::find_other("RustWTM")
    {
        if !background {
            wtm_core::window::bring_to_front(window);
            wtm_core::window::release_topmost(window);
        }
        return Ok(());
    }
    eframe::run_native(
        "RustWTM",
        options(!background),
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, background)))),
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
        let viewport = super::options(true).viewport;
        assert_eq!(super::options(false).viewport.visible, Some(false));
        assert_eq!(viewport.title.as_deref(), Some("RustWTM"));
        assert_eq!(viewport.inner_size, Some(eframe::egui::vec2(1240.0, 780.0)));
        assert_eq!(
            viewport.min_inner_size,
            Some(eframe::egui::vec2(820.0, 500.0))
        );
        assert_eq!(viewport.icon.unwrap().width, 64);
    }
}

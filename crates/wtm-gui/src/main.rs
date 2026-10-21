// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

mod about;
mod alert;
mod app;
mod backend;
mod disks_tab;
mod frames;
mod i18n;
mod kill_tab;
mod limits_tab;
mod logo;
mod navigation;
mod overview;
mod ports_tab;
mod power_controls;
mod processes;
mod sensors;
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
        // OpenGL: the window needs nothing more, and it loads far fewer
        // driver libraries than wgpu's Direct3D/Vulkan back ends.
        renderer: eframe::Renderer::Glow,
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

/// Memory of the window that Windows never pages out, so it repaints at once
/// even when RAM is exhausted.
const UI_WORKING_SET: usize = 64 << 20;

/// Keeps the window responsive on an overloaded machine: ahead of ordinary
/// apps for CPU, with its memory resident and the UI thread ahead of the
/// background workers. Best effort; Windows may refuse any step.
fn harden() {
    let _ = wtm_core::control::harden_self(
        wtm_core::control::PriorityClass::AboveNormal,
        UI_WORKING_SET,
    );
    let _ = wtm_core::control::raise_current_thread();
}

fn main() -> eframe::Result {
    i18n::initialize();
    if let Some(name) = blocked_program(std::env::args().skip(1)) {
        backend::register_app_id();
        backend::notify_blocked(&name);
        std::process::exit(1);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let background = args.iter().any(|a| a == "--background");
    // "Restart as administrator": the copy that started us is closing; it
    // must be gone before we look for another instance.
    if let Some(pid) = replaced_pid(&args) {
        wtm_core::launch::wait_for_exit(pid, REPLACE_WAIT);
    }
    // One RustWTM per session: a second start shows the first one's window
    // (`--new-instance` starts another one anyway).
    let _instance = if args.iter().any(|a| a == "--new-instance") {
        None
    } else {
        match wtm_core::window::single_instance(INSTANCE_MUTEX) {
            Some(lock) => Some(lock),
            None => {
                if !background {
                    show_other_instance();
                }
                return Ok(());
            }
        }
    };
    harden();
    eframe::run_native(
        "RustWTM",
        options(!background),
        Box::new(move |cc| {
            // Signing out, shutting down or an installer replacing our files
            // closes RustWTM even while it waits in the tray.
            let ctx = cc.egui_ctx.clone();
            wtm_core::window::watch_session_end(move || tray::request_exit(&ctx));
            Ok(Box::new(app::App::new(cc, background)))
        }),
    )
}

/// Held by the running RustWTM window of this session.
const INSTANCE_MUTEX: &str = r"Local\RustWTM-GUI";

/// How long a new copy waits for the one it replaces to exit.
const REPLACE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// The process named by `--replace <pid>`.
fn replaced_pid(args: &[String]) -> Option<u32> {
    let at = args
        .iter()
        .position(|a| a == wtm_core::launch::REPLACE_ARG)?;
    args.get(at + 1)?.parse().ok()
}

/// Brings the running copy's window forward. It may still be starting, so
/// it is looked for a few times.
fn show_other_instance() {
    for _ in 0..10 {
        if let Some((window, _)) = wtm_core::window::find_other("RustWTM") {
            wtm_core::window::bring_to_front(window);
            wtm_core::window::release_topmost(window);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
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
    fn finds_the_process_to_replace() {
        let replaced = |list: &[&str]| super::replaced_pid(&args(list).collect::<Vec<_>>());
        assert_eq!(replaced(&[]), None);
        assert_eq!(replaced(&["--background"]), None);
        assert_eq!(replaced(&["--replace", "1234"]), Some(1234));
        assert_eq!(replaced(&["--new-instance", "--replace", "7"]), Some(7));
        assert_eq!(replaced(&["--replace"]), None);
        assert_eq!(replaced(&["--replace", "x"]), None);
    }

    #[test]
    fn window_options() {
        assert!(matches!(
            super::options(true).renderer,
            eframe::Renderer::Glow
        ));
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

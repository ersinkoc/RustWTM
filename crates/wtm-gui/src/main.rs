// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

mod about;
mod ai_assistant;
mod alert;
mod app;
mod backend;
mod background;
mod charts;
mod containers;
mod context_actions;
mod device_facts;
mod devices;
mod diagnostics;
mod disk_details;
mod disks_tab;
mod docker_worker;
mod experience;
mod file_locks;
mod frames;
mod hold;
mod i18n;
mod incidents;
mod installed_apps;
mod inventory;
mod kill_tab;
mod limits_tab;
mod logo;
mod marks;
mod memory_view;
mod navigation;
mod numeric_tray;
mod onboarding;
mod overview;
mod packages;
mod perf_pages;
mod performance_view;
mod ports_tab;
mod power_controls;
mod preferences;
mod process_columns;
mod processes;
mod release_notes;
mod resource_strip;
mod rules;
mod sensors;
mod settings;
mod sort;
mod telegram_settings;
mod theme;
mod tray;
#[cfg(test)]
mod ui_tests;
mod views;
mod workspace_inspector;

use eframe::egui;

const MIN_WINDOW_SIZE: egui::Vec2 = egui::vec2(820.0, 500.0);
const DEFAULT_WINDOW_SIZE: egui::Vec2 = egui::vec2(1240.0, 780.0);

/// eframe restores saved geometry before calling this hook. Win32 does not
/// enforce the user-resize minimum on that programmatic initial size.
fn restore_window_bounds(viewport: egui::ViewportBuilder) -> egui::ViewportBuilder {
    let saved = viewport.inner_size.unwrap_or(DEFAULT_WINDOW_SIZE);
    let dimension = |value: f32, minimum: f32, fallback: f32| {
        if value.is_finite() {
            value.max(minimum)
        } else {
            fallback
        }
    };
    viewport.with_inner_size(egui::vec2(
        dimension(saved.x, MIN_WINDOW_SIZE.x, DEFAULT_WINDOW_SIZE.x),
        dimension(saved.y, MIN_WINDOW_SIZE.y, DEFAULT_WINDOW_SIZE.y),
    ))
}

/// `visible: false` starts hidden in the tray (`--background`, used at sign-in).
fn options(visible: bool) -> eframe::NativeOptions {
    eframe::NativeOptions {
        // OpenGL: the window needs nothing more, and it loads far fewer
        // driver libraries than wgpu's Direct3D/Vulkan back ends.
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_title("RustWTM")
            .with_visible(visible)
            .with_inner_size(DEFAULT_WINDOW_SIZE)
            .with_min_inner_size(MIN_WINDOW_SIZE)
            .with_icon(std::sync::Arc::new(egui::IconData {
                rgba: logo::rgba(64),
                width: 64,
                height: 64,
            })),
        window_builder: Some(Box::new(restore_window_bounds)),
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
    let args = launch_arguments(std::env::args().skip(1).collect());
    // A helper copy started through UAC only to open the real Task Manager.
    if args.iter().any(|a| a == wtm_core::task_manager::NATIVE_ARG) {
        std::process::exit(i32::from(wtm_core::task_manager::open_native().is_err()));
    }
    if let Some(name) = blocked_program(args.clone().into_iter()) {
        backend::register_app_id();
        backend::notify_blocked(&name);
        std::process::exit(1);
    }
    let background = args.iter().any(|a| a == "--background");
    let scheduled = args
        .iter()
        .any(|a| a == wtm_core::gui_startup::SCHEDULED_OPEN);
    if scheduled {
        wtm_core::gui_startup::wait_launcher();
    }
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
    let startup_error =
        if !wtm_core::launch::is_elevated() && wtm_core::gui_startup::settings().administrator {
            let restarted = if scheduled {
                wtm_core::launch::restart_with_uac(background)
            } else {
                wtm_core::launch::restart_elevated_with(background)
            };
            match restarted {
                Ok(()) => return Ok(()),
                Err(error) => Some(error),
            }
        } else {
            None
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
            Ok(Box::new(app::App::new(cc, background, startup_error)))
        }),
    )
}

/// IFEO appends the original Task Manager executable and its arguments.
/// They belong to Windows, never to RustWTM's startup/control switches.
fn launch_arguments(args: Vec<String>) -> Vec<String> {
    if args
        .first()
        .is_some_and(|a| a == wtm_core::task_manager::LAUNCH_ARG)
    {
        Vec::new()
    } else {
        args
    }
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

    #[test]
    fn task_manager_launch_is_visible_and_ignores_forwarded_switches() {
        let forwarded = [
            "--task-manager",
            r"C:\Windows\System32\taskmgr.exe",
            "--background",
            "--replace",
            "4",
            "--new-instance",
            "--blocked",
        ];
        assert!(super::launch_arguments(args(&forwarded).collect()).is_empty());
        for ordinary in [
            vec![],
            vec!["--background".to_owned()],
            vec!["--replace".into(), "42".into()],
        ] {
            assert_eq!(super::launch_arguments(ordinary.clone()), ordinary);
        }
    }

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

    #[test]
    fn startup_window_restoration_clamps_stale_small_geometry() {
        use eframe::egui;
        let mut options = super::options(true);
        // eframe replaces inner_size with the saved geometry before invoking
        // the window-builder hook. This is the observed 64x64 startup state.
        let saved = options
            .viewport
            .clone()
            .with_inner_size([64.0, 64.0])
            .with_position([0.0, 0.0]);
        let restored = match options.window_builder.take() {
            Some(hook) => hook(saved),
            None => saved,
        };
        assert_eq!(restored.inner_size, Some(egui::vec2(820.0, 500.0)));
        assert_eq!(restored.position, Some(egui::pos2(0.0, 0.0)));
    }

    #[test]
    fn startup_window_restoration_keeps_valid_geometry_and_window_flags() {
        use eframe::egui;
        for (size, expected) in [
            ([1600.0, 900.0], [1600.0, 900.0]),
            ([64.0, 900.0], [820.0, 900.0]),
            ([1600.0, 64.0], [1600.0, 500.0]),
            ([0.0, 0.0], [820.0, 500.0]),
            ([f32::NAN, f32::INFINITY], [1240.0, 780.0]),
        ] {
            let viewport = egui::ViewportBuilder::default()
                .with_inner_size(size)
                .with_position([42.0, 64.0])
                .with_maximized(true)
                .with_visible(false);
            let restored = super::restore_window_bounds(viewport);
            assert_eq!(restored.inner_size, Some(expected.into()));
            assert_eq!(restored.position, Some(egui::pos2(42.0, 64.0)));
            assert_eq!(restored.maximized, Some(true));
            assert_eq!(restored.visible, Some(false));
        }
    }
}

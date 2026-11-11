//! Local experience preferences and discoverable, non-mutating first-run guidance.
use eframe::egui::{self, Ui};
use std::sync::{
    Arc,
    atomic::{AtomicIsize, Ordering},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub active_seconds: u16,
    pub minimized_seconds: u16,
    pub tray_seconds: u16,
    pub global_shortcut: bool,
    pub function_key: u8,
    pub network_bits: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            active_seconds: 1,
            minimized_seconds: 5,
            tray_seconds: 10,
            global_shortcut: false,
            function_key: 10,
            network_bits: false,
        }
    }
}
#[derive(Clone, Copy)]
pub enum Destination {
    Hardware,
    Applications,
    Rules,
    History,
}
#[derive(Default)]
pub struct View {
    pub prefs: Preferences,
    pub guide: bool,
    pub quick_setup: bool,
    pub help: bool,
    hotkey: Option<wtm_sys::hotkey::Hotkey>,
    requested: Option<(bool, u8)>,
    error: Option<String>,
}
pub fn cadence(ctx: &egui::Context, window: isize) -> u64 {
    let p = ctx
        .data(|d| d.get_temp::<Preferences>(egui::Id::unique("experience-preferences")))
        .unwrap_or_default();
    u64::from(if window == 0 {
        p.active_seconds
    } else if !wtm_core::window::is_visible(window) {
        p.tray_seconds
    } else if wtm_sys::window::is_minimized(window) {
        p.minimized_seconds
    } else {
        p.active_seconds
    })
    .clamp(1, 60)
}
pub fn network_bits(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Preferences>(egui::Id::unique("experience-preferences")))
        .unwrap_or_default()
        .network_bits
}
impl View {
    pub fn apply(&mut self, ctx: &egui::Context, window: Arc<AtomicIsize>) {
        self.prefs.active_seconds = self.prefs.active_seconds.clamp(1, 5);
        self.prefs.minimized_seconds = self.prefs.minimized_seconds.clamp(1, 60);
        self.prefs.tray_seconds = self.prefs.tray_seconds.clamp(1, 60);
        self.prefs.function_key = self.prefs.function_key.clamp(2, 12);
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("experience-preferences"), self.prefs));
        let requested = (self.prefs.global_shortcut, self.prefs.function_key);
        if self.requested != Some(requested) {
            self.hotkey = None;
            self.error = None;
            self.requested = Some(requested);
            if requested.0 {
                #[cfg(not(test))]
                {
                    let context = ctx.clone();
                    match wtm_sys::hotkey::Hotkey::new(requested.1, move || {
                        let raw = window.load(Ordering::Relaxed);
                        if raw == 0 {
                            return;
                        }
                        if wtm_core::window::is_visible(raw) {
                            wtm_core::window::hide(raw);
                            context.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                        } else {
                            wtm_core::window::bring_to_front(raw);
                            context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            context.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                            context.request_repaint();
                        }
                    }) {
                        Ok(h) => self.hotkey = Some(h),
                        Err(e) => self.error = Some(e),
                    }
                }
            }
        }
        #[cfg(test)]
        let _ = (&window, Ordering::Relaxed);
    }
    pub fn settings(&mut self, ui: &mut Ui) {
        crate::theme::card(
            ui,
            crate::i18n::tr("Interface activity and shortcuts"),
            |ui| {
                ui.weak(crate::i18n::tr("These intervals affect window refresh requests, not service protection or sampling."));
                for (label, value, max) in [
                    ("Visible window refresh", &mut self.prefs.active_seconds, 5),
                    (
                        "Minimized window refresh",
                        &mut self.prefs.minimized_seconds,
                        60,
                    ),
                    ("Tray window refresh", &mut self.prefs.tray_seconds, 60),
                ] {
                    crate::theme::setting_row(ui, crate::i18n::tr(label), |ui, id| {
                        ui.add(egui::Slider::new(value, 1..=max).suffix(" s"))
                            .labelled_by(id);
                    });
                }
                ui.checkbox(
                    &mut self.prefs.network_bits,
                    crate::i18n::tr("Show Mbps in network device charts"),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(
                        &mut self.prefs.global_shortcut,
                        crate::i18n::tr("Global show / hide shortcut"),
                    );
                    let key_label = ui.label("Ctrl + Alt + F");
                    ui.add(egui::DragValue::new(&mut self.prefs.function_key).range(2..=12))
                        .labelled_by(key_label.id);
                });
                if let Some(e) = &self.error {
                    crate::theme::colored(ui, crate::theme::WARN, e);
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button(crate::i18n::tr("Quick setup")).clicked() {
                        self.quick_setup = true;
                    }
                    if ui.button(crate::i18n::tr("Open welcome guide")).clicked() {
                        self.guide = true;
                    }
                    if ui.button(crate::i18n::tr("Keyboard shortcuts")).clicked() {
                        self.help = true;
                    }
                });
            },
        );
    }
    pub fn overlays(&mut self, ctx: &egui::Context) -> Option<Destination> {
        let mut destination = None;
        if self.guide {
            let mut close = false;
            let modal=egui::Modal::new(egui::Id::unique("welcome-guide")).show(ctx,|ui|{
   ui.set_max_width(620.0);ui.heading(crate::i18n::tr("Welcome to RustWTM"));ui.label(crate::i18n::tr("Understand your PC, inspect changes and keep important work responsive."));
   for (title,description,target) in [("Explore devices","Inspect device identity, health and recent readings.",Destination::Hardware),("Manage applications","Review installed apps and choose package updates.",Destination::Applications),("Build a protection rule","Preview the matching programs before saving a policy.",Destination::Rules),("Review recorded activity","Inspect resource history and recorded protection actions.",Destination::History)]{
    crate::theme::panel(ui,|ui|{if ui.button(crate::i18n::tr(title)).clicked(){destination=Some(target);close=true;}ui.weak(crate::i18n::tr(description));});
   }
   ui.weak(crate::i18n::tr("Start in monitoring mode. Changes and installers require your explicit review."));close|=ui.button(crate::i18n::tr("Continue to overview")).clicked();
  });
            if close || modal.should_close() {
                self.guide = false;
            }
        }
        if self.help {
            let mut close = false;
            let modal = egui::Modal::new(egui::Id::unique("shortcut-reference")).show(ctx, |ui| {
                ui.set_max_width(560.0);
                ui.heading(crate::i18n::tr("Keyboard shortcuts"));
                for (key, action) in [
                    ("Ctrl+K", "Quick navigation"),
                    ("Ctrl+F", "Search"),
                    ("Ctrl+Tab", "Next page"),
                    ("Ctrl+Shift+Tab", "Previous page"),
                    ("F6", "Pause process updates"),
                    ("Esc", "Close details"),
                    ("F1", "Keyboard shortcuts"),
                ] {
                    ui.horizontal(|ui| {
                        ui.monospace(key);
                        ui.label(crate::i18n::tr(action));
                    });
                }
                if self.prefs.global_shortcut {
                    ui.label(format!(
                        "Ctrl+Alt+F{} · {}",
                        self.prefs.function_key,
                        crate::i18n::tr("Global show / hide shortcut")
                    ));
                }
                close = ui.button(crate::i18n::tr("Close")).clicked();
            });
            if close || modal.should_close() {
                self.help = false;
            }
        }
        destination
    }
}

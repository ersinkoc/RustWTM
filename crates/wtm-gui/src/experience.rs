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
    /// Let Windows take this window's memory first once it has been
    /// minimized or in the tray for `background_memory_minutes`.
    pub background_memory: bool,
    pub background_memory_minutes: u16,
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
            background_memory: false,
            background_memory_minutes: 10,
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
    /// Since when the window is minimized or hidden, and whether its memory
    /// priority is lowered for that time.
    hidden_since: Option<(std::time::Instant, bool)>,
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
        self.prefs.background_memory_minutes = self.prefs.background_memory_minutes.clamp(1, 120);
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("experience-preferences"), self.prefs));
        let raw = window.load(Ordering::Relaxed);
        let hidden =
            raw != 0 && (!wtm_core::window::is_visible(raw) || wtm_sys::window::is_minimized(raw));
        if let Some(low) = self.memory_priority_change(hidden, std::time::Instant::now()) {
            #[cfg(not(test))]
            let _ = wtm_sys::control::set_own_memory_priority(low);
            #[cfg(test)]
            let _ = low;
        }
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
    /// `Some(true)` when this tick should lower the memory priority (the
    /// window stayed hidden for the chosen minutes), `Some(false)` when it
    /// should restore it, `None` when nothing changes.
    fn memory_priority_change(&mut self, hidden: bool, now: std::time::Instant) -> Option<bool> {
        if !hidden || !self.prefs.background_memory {
            let lowered = self.hidden_since.take().is_some_and(|(_, low)| low);
            return lowered.then_some(false);
        }
        let (since, lowered) = self.hidden_since.get_or_insert((now, false));
        let wait =
            std::time::Duration::from_secs(u64::from(self.prefs.background_memory_minutes) * 60);
        if *lowered || now.duration_since(*since) < wait {
            return None;
        }
        *lowered = true;
        Some(true)
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
                ui.checkbox(
                    &mut self.prefs.background_memory,
                    crate::i18n::tr("Yield memory when hidden"),
                )
                .on_hover_text(crate::i18n::tr(
                    "After the window stays minimized or in the tray, Windows may reclaim its memory first. Sampling, history and protection continue.",
                ));
                let enabled = self.prefs.background_memory;
                crate::theme::setting_row(ui, crate::i18n::tr("Yield memory after"), |ui, id| {
                    ui.add_enabled(
                        enabled,
                        egui::Slider::new(&mut self.prefs.background_memory_minutes, 1..=120)
                            .suffix(" min"),
                    )
                    .labelled_by(id);
                });
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn memory_priority_drops_once_after_the_delay_and_returns_when_shown() {
        let mut view = View::default();
        // Off unless chosen: a lowered priority can make the window slower to
        // come back exactly when the PC is short of memory.
        assert_eq!(view.memory_priority_change(true, Instant::now()), None);
        view.prefs.background_memory = true;
        let start = Instant::now();
        let minutes = |m: u64| start + Duration::from_secs(m * 60);
        assert_eq!(view.memory_priority_change(true, start), None);
        assert_eq!(view.memory_priority_change(true, minutes(9)), None);
        assert_eq!(view.memory_priority_change(true, minutes(10)), Some(true));
        assert_eq!(view.memory_priority_change(true, minutes(30)), None, "once");
        assert_eq!(view.memory_priority_change(false, minutes(31)), Some(false));
        assert_eq!(view.memory_priority_change(false, minutes(32)), None);
        // Shown before the delay: nothing to restore.
        assert_eq!(view.memory_priority_change(true, minutes(33)), None);
        assert_eq!(view.memory_priority_change(false, minutes(34)), None);
        view.prefs.background_memory = false;
        assert_eq!(view.memory_priority_change(true, minutes(100)), None);
    }
}

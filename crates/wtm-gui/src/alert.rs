//! Pops the window up when the CPU stays overloaded, so the user can calm
//! things down before the machine becomes unusable.

use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use wtm_core::control::{EcoQos, Limits, PriorityClass, ProcessRef};
use wtm_ipc::Command;

use crate::backend::{Data, Mode, UiCommand};
use crate::overview::busiest_apps;
use crate::processes::Intent;

/// Quiet time after the user closes an alert.
pub const QUIET_AFTER_CLOSE: Duration = Duration::from_secs(5 * 60);
/// "Snooze" button.
pub const SNOOZE: Duration = Duration::from_secs(60 * 60);
/// How long the load must stay back to normal before the alert closes by
/// itself (unless the user keeps it open).
pub const AUTO_CLOSE: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AlertSettings {
    pub enabled: bool,
    /// System CPU, percent.
    pub threshold: f32,
    /// How long the CPU must stay at or above the threshold.
    pub seconds: u32,
}

impl Default for AlertSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 90.0,
            seconds: 5,
        }
    }
}

/// An alert on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Alert {
    /// Latest system CPU, percent.
    pub cpu: f32,
    pub started: Instant,
}

#[derive(Debug, Default)]
pub struct AlertWatch {
    pub settings: AlertSettings,
    /// Since when the CPU is continuously at or above the threshold.
    above_since: Option<Instant>,
    quiet_until: Option<Instant>,
    pub active: Option<Alert>,
    /// Since when the CPU is back below the threshold, while an alert shows.
    normal_since: Option<Instant>,
    /// The user stopped the countdown: the alert stays until closed.
    pub held: bool,
}

impl AlertWatch {
    /// Missing or stale monitoring breaks a continuous overload observation.
    pub fn interrupt(&mut self) {
        self.above_since = None;
    }

    /// Feeds one CPU sample; `true` when an alert has just been raised.
    /// An alert whose load stays back to normal for [`AUTO_CLOSE`] closes
    /// by itself, unless the user held it; a new overload restarts the wait.
    pub fn update(&mut self, now: Instant, cpu: f32) -> bool {
        if let Some(a) = &mut self.active {
            a.cpu = cpu;
            if cpu < self.settings.threshold {
                let since = *self.normal_since.get_or_insert(now);
                if !self.held && now.saturating_duration_since(since) >= AUTO_CLOSE {
                    self.close(now, QUIET_AFTER_CLOSE);
                }
            } else {
                self.normal_since = None;
            }
            return false;
        }
        if !self.settings.enabled || cpu < self.settings.threshold {
            self.above_since = None;
            return false;
        }
        let since = *self.above_since.get_or_insert(now);
        if self.quiet_until.is_some_and(|q| now < q)
            || now.saturating_duration_since(since)
                < Duration::from_secs(u64::from(self.settings.seconds))
        {
            return false;
        }
        self.active = Some(Alert { cpu, started: now });
        self.normal_since = None;
        self.held = false;
        true
    }

    /// Time left before the alert closes by itself, while it counts down.
    pub fn closes_in(&self, now: Instant) -> Option<Duration> {
        if self.active.is_none() || self.held {
            return None;
        }
        let since = self.normal_since?;
        Some(AUTO_CLOSE.saturating_sub(now.saturating_duration_since(since)))
    }

    /// The user closed the alert: stay quiet for `quiet`.
    pub fn close(&mut self, now: Instant, quiet: Duration) {
        self.active = None;
        self.normal_since = None;
        self.held = false;
        self.above_since = None;
        self.quiet_until = Some(now + quiet);
    }
}

use crate::theme::{BAD, GOOD, WARN, text_color};
/// Apps offered in the alert.
const APPS_SHOWN: usize = 5;

/// Brings an app down without stopping it (undo: right-click → Normal mode).
const CALM: [Command; 2] = [
    Command::EcoQos(EcoQos::On),
    Command::Priority(PriorityClass::BelowNormal),
];

fn calm(t: &ProcessRef) -> impl Iterator<Item = Intent> + '_ {
    CALM.into_iter().map(|c| Intent::Command(t.clone(), c))
}

/// The alert, over everything else. Returns what the user chose.
pub fn dialog(
    ctx: &egui::Context,
    data: &Data,
    settings: &mut AlertSettings,
    panic: &mut bool,
    commands: &mut Vec<UiCommand>,
    intents: &mut Vec<Intent>,
) {
    let Some(alert) = data.alert_watch.active else {
        return;
    };
    let cpus = data
        .snapshot
        .as_ref()
        .map_or(1, |s| s.system.logical_cpus.max(1));
    let me = std::process::id();
    let apps: Vec<_> = busiest_apps(data, APPS_SHOWN + 1)
        .into_iter()
        .filter(|a| a.cpu >= 1.0 && a.instances.iter().all(|p| p.pid != me))
        .take(APPS_SHOWN)
        .collect();
    let foreground = data.status.as_ref().and_then(|s| s.foreground.as_deref());
    // The app in front: as the service names it, or holding the window
    // without the service.
    let in_use = |a: &crate::overview::App<'_>| {
        Some(a.name) == foreground
            || data
                .foreground_pid
                .is_some_and(|pid| a.instances.iter().any(|p| p.pid == pid))
    };
    let mut close = None;

    let modal = egui::Modal::new(egui::Id::unique("cpu_alert")).show(ctx, |ui| {
        ui.set_width(640.0);
        let color = text_color(
            ui,
            if alert.cpu >= settings.threshold {
                BAD
            } else {
                GOOD
            },
        );
        ui.label(
            RichText::new({
                let __wtm_i18n_0 = &(alert.cpu);
                crate::i18n::format(
                    "⚠ Your PC is overloaded: CPU {0}%",
                    &[format!("{:.0}", __wtm_i18n_0)],
                )
            })
            .size(22.0)
            .color(color),
        );
        if alert.cpu < settings.threshold {
            ui.horizontal_wrapped(|ui| {
                crate::theme::colored(ui, GOOD, crate::i18n::tr("The load is back to normal."));
                if let Some(left) = data.alert_watch.closes_in(std::time::Instant::now()) {
                    ui.label(crate::i18n::format(
                        "Closes by itself in {0} s.",
                        &[left.as_secs().max(1).to_string()],
                    ));
                    if ui
                        .button(crate::i18n::tr("Keep it open"))
                        .on_hover_text(crate::i18n::tr(
                            "Stops the countdown; the alert stays until you close it",
                        ))
                        .clicked()
                    {
                        commands.push(UiCommand::HoldAlert);
                    }
                    ui.ctx().request_repaint_after(Duration::from_millis(500));
                }
            });
        } else {
            ui.label(crate::i18n::tr(
                "These apps use the most CPU. One click brings them down; nothing is lost.",
            ));
        }
        ui.add_space(0.0);

        if !apps.is_empty() {
            let background: Vec<&ProcessRef> = apps
                .iter()
                .filter(|a| !in_use(a))
                .flat_map(|a| &a.instances)
                .collect();
            if ui
                .button(
                    RichText::new(crate::i18n::tr("⚡ Calm them all down"))
                        .size(16.0)
                        .strong(),
                )
                .on_hover_text(crate::i18n::tr(
                    "Efficiency mode and lower priority for these apps (except the one you are \
                     using): they keep running, but stop crowding out everything else.",
                ))
                .clicked()
            {
                for t in background {
                    intents.extend(calm(t));
                }
            }
            ui.add_space(0.0);
        }

        egui::Grid::new("alert_apps")
            .num_columns(3)
            .striped(true)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                for a in &apps {
                    let using = in_use(a);
                    ui.label(RichText::new(a.name).strong())
                        .on_hover_text(if using {
                            crate::i18n::tr("The app you are using")
                        } else {
                            ""
                        });
                    ui.label(
                        RichText::new(format!("{:.0}%", a.cpu))
                            .color(text_color(ui, if a.cpu >= 50.0 { BAD } else { WARN })),
                    );
                    ui.horizontal(|ui| {
                        let all = |c: Command| {
                            a.instances
                                .iter()
                                .map(move |t| Intent::Command(t.clone(), c))
                        };
                        if ui
                            .button(crate::i18n::tr("Efficiency mode"))
                            .on_hover_text(crate::i18n::tr(
                                "Runs it on efficient cores at lower priority",
                            ))
                            .clicked()
                        {
                            for t in &a.instances {
                                intents.extend(calm(t));
                            }
                        }
                        let one_core = 100.0 / cpus as f32;
                        if ui
                            .button(crate::i18n::tr("Limit to 1 core"))
                            .on_hover_text(crate::i18n::tr(
                                "A hard cap for each of its processes, until you remove it",
                            ))
                            .clicked()
                        {
                            intents.extend(all(Command::Limit(Limits {
                                cpu_percent: Some(one_core),
                                memory_bytes: None,
                            })));
                        }
                        if ui
                            .button(crate::i18n::tr("Pause"))
                            .on_hover_text(crate::i18n::tr(
                                "Stops it completely; right-click → Resume brings it back",
                            ))
                            .clicked()
                        {
                            intents.extend(all(Command::Suspend));
                        }
                        if ui
                            .button(
                                RichText::new(crate::i18n::tr("End")).color(text_color(ui, BAD)),
                            )
                            .clicked()
                        {
                            intents.push(Intent::ConfirmMany(a.instances.clone(), Command::Kill));
                        }
                    });
                    crate::theme::end_row(ui);
                }
            });
        if apps.is_empty() {
            ui.weak(crate::i18n::tr(
                "No single app stands out; many small ones add up.",
            ));
        }
        let doomed = crate::kill_tab::targets(data).len();
        if doomed > 0 {
            ui.add_space(0.0);
            if ui
                .button(
                    RichText::new({
                        crate::i18n::format(
                            "🚨 End the kill list ({0} processes)",
                            &[format!("{}", doomed)],
                        )
                    })
                    .strong()
                    .color(text_color(ui, BAD)),
                )
                .on_hover_text(crate::i18n::tr("Shows them first"))
                .clicked()
            {
                *panic = true;
            }
        }

        ui.add_space(0.0);
        match &data.config {
            Some(Ok((_, text))) if data.status.as_ref().is_some_and(|s| s.dry_run) => {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::tr("Automatic protection is off."));
                    let admin = matches!(data.mode, Mode::Service { admin: true, .. });
                    if ui
                        .add_enabled(admin, egui::Button::new(crate::i18n::tr("Turn it on")))
                        .on_hover_text(crate::i18n::tr("RustWTM then steps in by itself next time"))
                        .clicked()
                        && let Some(new) = crate::settings::set_dry_run(text, false)
                    {
                        commands.push(UiCommand::SaveConfig(new));
                    }
                });
            }
            _ if !matches!(data.mode, Mode::Service { .. }) => {
                ui.weak(crate::i18n::tr(
                    "Install the RustWTM service and it steps in automatically next time.",
                ));
            }
            _ => {}
        }

        ui.add_space(0.0);
        ui.horizontal(|ui| {
            if ui.button(crate::i18n::tr("Close")).clicked() {
                close = Some(QUIET_AFTER_CLOSE);
            }
            if ui.button(crate::i18n::tr("Snooze for 1 hour")).clicked() {
                close = Some(SNOOZE);
            }
            if ui
                .button(crate::i18n::tr("Turn off these alerts"))
                .clicked()
            {
                settings.enabled = false;
                close = Some(QUIET_AFTER_CLOSE);
            }
        });
    });
    if modal.should_close() {
        close.get_or_insert(QUIET_AFTER_CLOSE);
    }
    if let Some(quiet) = close {
        commands.push(UiCommand::CloseAlert(quiet));
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_after_a_sustained_overload_and_respects_quiet_times() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut w = AlertWatch::default();
        // A short spike does not count.
        assert!(!w.update(at(0), 95.0));
        assert!(!w.update(at(3), 95.0));
        assert!(!w.update(at(4), 50.0));
        assert!(!w.update(at(5), 95.0));
        assert!(!w.update(at(9), 95.0));
        assert!(w.update(at(10), 97.0), "5 s at or above 90%");
        assert_eq!(w.active.map(|a| a.cpu), Some(97.0));
        // While shown it only follows the load.
        assert!(!w.update(at(11), 99.0));
        assert_eq!(w.active.map(|a| a.cpu), Some(99.0));

        w.close(at(12), QUIET_AFTER_CLOSE);
        assert!(w.active.is_none());
        assert!(!w.update(at(13), 99.0));
        assert!(!w.update(at(12 + 299), 99.0), "quiet for five minutes");
        assert!(w.update(at(12 + 300), 99.0), "still overloaded afterwards");

        // Turned off: never.
        let mut off = AlertWatch {
            settings: AlertSettings {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!off.update(at(0), 100.0));
        assert!(!off.update(at(60), 100.0));
    }

    #[test]
    fn closes_itself_once_the_load_stays_normal_unless_held() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let raised = || {
            let mut w = AlertWatch {
                settings: AlertSettings {
                    seconds: 0,
                    ..Default::default()
                },
                ..Default::default()
            };
            assert!(w.update(at(0), 99.0));
            w
        };
        let mut w = raised();
        assert_eq!(w.closes_in(at(0)), None, "no countdown while overloaded");
        assert!(!w.update(at(1), 40.0));
        assert_eq!(w.closes_in(at(1)), Some(AUTO_CLOSE));
        assert_eq!(
            w.closes_in(at(6)),
            Some(AUTO_CLOSE - Duration::from_secs(5))
        );
        // The overload comes back: the countdown starts over later.
        assert!(!w.update(at(5), 95.0));
        assert_eq!(w.closes_in(at(5)), None);
        assert!(!w.update(at(6), 40.0));
        assert!(!w.update(at(6 + 14), 40.0));
        assert!(w.active.is_some(), "not yet");
        assert!(!w.update(at(6 + 15), 40.0));
        assert!(w.active.is_none(), "closed by itself");
        assert!(!w.update(at(30), 99.0), "and quiet as after a close");

        // Held: it stays, without a countdown, until closed.
        let mut w = raised();
        assert!(!w.update(at(1), 40.0));
        w.held = true;
        assert_eq!(w.closes_in(at(2)), None);
        assert!(!w.update(at(100), 40.0));
        assert!(w.active.is_some());
        w.close(at(101), QUIET_AFTER_CLOSE);
        assert!(!w.held, "the next alert counts down again");
    }

    #[test]
    fn the_dialog_shows_the_countdown_and_can_be_kept_open() {
        use egui_kittest::kittest::Queryable;
        let mut data = Data::default();
        data.alert_watch.settings.seconds = 0;
        let now = Instant::now();
        assert!(data.alert_watch.update(now, 99.0));
        let render = |ui: &mut egui::Ui, (data, commands): &mut (Data, Vec<UiCommand>)| {
            let mut settings = data.alert_watch.settings;
            let (mut panic, mut intents) = (false, Vec::new());
            dialog(
                ui.ctx(),
                data,
                &mut settings,
                &mut panic,
                commands,
                &mut intents,
            );
        };
        // Still overloaded: no countdown.
        let mut h = egui_kittest::Harness::builder()
            .allow_missing_glyphs()
            .with_size([900.0, 700.0])
            .build_ui_state(render, (data, Vec::new()));
        h.run_steps(2);
        assert!(h.query_by_label(crate::i18n::tr("Keep it open")).is_none());
        // Back to normal: the countdown and the button to stop it.
        h.state_mut().0.alert_watch.update(now, 40.0);
        h.run_steps(2);
        h.get_by_label(crate::i18n::tr("Keep it open")).click();
        h.run_steps(2);
        assert!(
            h.state()
                .1
                .iter()
                .any(|c| matches!(c, UiCommand::HoldAlert)),
            "the button asks to hold the alert"
        );
        // Held: the countdown is gone.
        h.state_mut().0.alert_watch.held = true;
        h.run_steps(2);
        assert!(h.query_by_label(crate::i18n::tr("Keep it open")).is_none());
    }
}

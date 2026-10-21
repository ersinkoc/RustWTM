//! Settings → Telegram bot: token, allowed chats, alerts and control.

use eframe::egui::{self, Ui};
use wtm_policy::{TelegramConfig, parse_chats, token_looks_valid};

use crate::backend::{Data, Mode, Panel, UiCommand};
use crate::theme::{BAD, GOOD, WARN, card};

#[derive(Debug, Default)]
pub struct TelegramView {
    /// Being edited; `None` until the service's settings arrive.
    form: Option<TelegramConfig>,
    chats: String,
    /// The settings the form started from.
    original: Option<TelegramConfig>,
    show_token: bool,
    requested: bool,
}

impl TelegramView {
    /// The form as settings, or why it cannot be saved.
    fn edited(&self) -> Option<Result<TelegramConfig, String>> {
        let form = self.form.as_ref()?;
        Some(parse_chats(&self.chats).and_then(|chats| {
            let cfg = TelegramConfig {
                chats,
                token: form.token.trim().to_owned(),
                ..form.clone()
            };
            cfg.validate().map(|()| cfg)
        }))
    }
}

/// Returns the commands to send (load, save, test).
pub fn telegram(ui: &mut Ui, data: &Data, view: &mut TelegramView) -> Vec<UiCommand> {
    let mut commands = Vec::new();
    card(ui, crate::i18n::tr("🔔 Telegram bot"), |ui| {
        ui.label(crate::i18n::tr(
            "Check on this PC and control it from your phone: status, busiest processes, ports \
             and services; end, suspend and resume processes; get alerts when the CPU is \
             overloaded or a program runs hundreds of copies.",
        ));
        ui.add_space(4.0);
        match data.mode {
            Mode::Service { admin: true, .. } => {}
            Mode::Service { admin: false, .. } => {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        crate::theme::text_color(ui, WARN),
                        crate::i18n::tr("Setting up the bot needs administrator rights."),
                    );
                    if ui
                        .small_button(crate::i18n::tr("Restart as administrator"))
                        .clicked()
                    {
                        commands.push(UiCommand::RestartElevated);
                    }
                });
                return;
            }
            Mode::Starting | Mode::Local => {
                let other_version = data
                    .service_error
                    .as_deref()
                    .is_some_and(|e| e.contains("protocol"));
                ui.colored_label(
                    crate::theme::text_color(ui, WARN),
                    if other_version {
                        { let __wtm_i18n_0 = &(env!("CARGO_PKG_VERSION")); crate::i18n::format("The bot runs in the RustWTM service, and the installed one is a different version. Update it with the installer of version {0}.", &[__wtm_i18n_0.to_string()]) }
                    } else {
                        crate::i18n::tr("The bot runs in the RustWTM service, which is not running. Install \
                         RustWTM with the MSI to use it.")
                            .to_owned()
                    },
                );
                return;
            }
        }
        match &data.telegram {
            None => {
                if !view.requested {
                    view.requested = true;
                    commands.push(UiCommand::Load(Panel::Telegram));
                }
                crate::theme::loading(ui, crate::i18n::tr("Loading the bot settings…"));
                return;
            }
            Some(Err(e)) => {
                ui.colored_label(crate::theme::text_color(ui, BAD), e);
                if ui.button(crate::i18n::tr("Retry")).clicked() {
                    commands.push(UiCommand::Load(Panel::Telegram));
                }
                return;
            }
            Some(Ok(saved)) => {
                if view.original.as_ref() != Some(saved) {
                    view.chats = saved
                        .chats
                        .iter()
                        .map(i64::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    view.form = Some(saved.clone());
                    view.original = Some(saved.clone());
                }
            }
        }
        steps(ui);
        let Some(form) = view.form.as_mut() else {
            return;
        };
        ui.checkbox(&mut form.enabled, crate::i18n::tr("Enabled"));
        egui::Grid::new("telegram_form")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label(crate::i18n::tr("Bot token"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut form.token)
                            .password(!view.show_token)
                            .hint_text(crate::i18n::tr("123456789:ABC… from @BotFather"))
                            .desired_width(340.0),
                    );
                    let eye = if view.show_token {
                        crate::i18n::tr("Hide")
                    } else {
                        crate::i18n::tr("Show")
                    };
                    if ui
                        .small_button(eye)
                        .on_hover_text(crate::i18n::tr("Show or hide the token"))
                        .clicked()
                    {
                        view.show_token = !view.show_token;
                    }
                });
                ui.end_row();
                ui.label(crate::i18n::tr("Allowed chat IDs"));
                ui.add(
                    egui::TextEdit::singleline(&mut view.chats)
                        .hint_text(crate::i18n::tr("e.g. 123456789, 987654321"))
                        .desired_width(340.0),
                )
                .on_hover_text(crate::i18n::tr(
                    "Only these chats may use the bot; separate several with commas",
                ));
                ui.end_row();
            });
        ui.checkbox(
            &mut form.notify,
            crate::i18n::tr("Send alerts (CPU overload, swarms, programs ended by the kill list)"),
        );
        ui.checkbox(
            &mut form.control,
            crate::i18n::tr("Allow ending, suspending and resuming processes and services"),
        )
        .on_hover_text(crate::i18n::tr(
            "Without it the bot only reports; protected Windows processes are never ended",
        ));
        if !form.token.trim().is_empty() && !token_looks_valid(form.token.trim()) {
            crate::theme::colored(
                ui,
                WARN,
                crate::i18n::tr("This does not look like a bot token (123456789:ABC…)."),
            );
        }
        let edited = view.edited();
        let dirty = match &edited {
            Some(Ok(cfg)) => view.original.as_ref() != Some(cfg),
            _ => true,
        };
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let valid = matches!(edited, Some(Ok(_)));
            if ui
                .add_enabled(valid && dirty, egui::Button::new(crate::i18n::tr("Save")))
                .clicked()
                && let Some(Ok(cfg)) = edited.clone()
            {
                commands.push(UiCommand::SaveTelegram(cfg));
            }
            let saved_token = view
                .original
                .as_ref()
                .is_some_and(|c| token_looks_valid(&c.token));
            if ui
                .add_enabled(
                    saved_token && !dirty,
                    egui::Button::new(crate::i18n::tr("Send test message")),
                )
                .on_hover_text(crate::i18n::tr(
                    "Checks the saved token and messages every allowed chat",
                ))
                .on_disabled_hover_text(crate::i18n::tr("Save a bot token first"))
                .clicked()
            {
                commands.push(UiCommand::TestTelegram);
            }
            match (&edited, &view.original) {
                (Some(Err(e)), _) => {
                    ui.colored_label(crate::theme::text_color(ui, BAD), format!("✖ {e}"));
                }
                (_, _) if dirty => {
                    ui.colored_label(
                        crate::theme::text_color(ui, WARN),
                        crate::i18n::tr("● unsaved changes"),
                    );
                }
                (_, Some(c)) if c.enabled && c.chats.is_empty() => {
                    crate::theme::colored(
                        ui,
                        WARN,
                        crate::i18n::tr(
                            "On, but no chat is allowed yet: message the bot to get your ID.",
                        ),
                    );
                }
                (_, Some(c)) if c.enabled => {
                    ui.colored_label(crate::theme::text_color(ui, GOOD), {
                        let __wtm_i18n_0 = &(c.chats.len());
                        crate::i18n::format("✔ On for {0} chat(s)", &[format!("{}", __wtm_i18n_0)])
                    });
                }
                _ => {
                    ui.weak(crate::i18n::tr("Off"));
                }
            }
        });
    });
    commands
}

fn steps(ui: &mut Ui) {
    egui::CollapsingHeader::new(crate::i18n::tr("How to set it up"))
        .id_salt("telegram_steps")
        .default_open(false)
        .show(ui, |ui| {
            ui.label(crate::i18n::tr(
                "1. In Telegram, open @BotFather, send /newbot and follow the questions.",
            ));
            ui.label(crate::i18n::tr(
                "2. Paste the token it gives you below, tick Enabled and Save.",
            ));
            ui.label(crate::i18n::tr(
                "3. Send your new bot any message: it answers with your chat ID.",
            ));
            ui.label(crate::i18n::tr(
                "4. Add that ID to Allowed chat IDs and Save. Send /help to the bot.",
            ));
            ui.weak(crate::i18n::tr(
                "The token is stored in %ProgramData%\\RustWTM\\telegram.toml, readable only by \
                 administrators. Anyone with the token can read what the bot sends, so keep it \
                 private; chats not on the list cannot give commands.",
            ));
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    const TOKEN: &str = "123456789:AAFh3kq_Zx-9yLmnopQRstuVWxyz012345";

    fn run(data: Data, view: &mut TelegramView) -> Vec<UiCommand> {
        let mut out = Vec::new();
        {
            let mut harness = Harness::builder()
                .allow_missing_glyphs()
                .build_ui(|ui| out.extend(telegram(ui, &data, view)));
            harness.step();
        }
        out
    }

    fn admin() -> Data {
        Data {
            mode: Mode::Service {
                admin: true,
                verified: true,
            },
            ..Data::default()
        }
    }

    #[test]
    fn loads_once_and_asks_non_admins_to_elevate() {
        let mut view = TelegramView::default();
        let cmds = run(admin(), &mut view);
        assert!(matches!(
            cmds.as_slice(),
            [UiCommand::Load(Panel::Telegram)]
        ));
        assert!(run(admin(), &mut view).is_empty(), "requested only once");

        let data = Data {
            mode: Mode::Service {
                admin: false,
                verified: true,
            },
            ..Data::default()
        };
        let mut view = TelegramView::default();
        let mut harness = Harness::builder().allow_missing_glyphs().build_ui(|ui| {
            telegram(ui, &data, &mut view);
        });
        harness.run();
        harness.get_by_label("Restart as administrator");
    }

    #[test]
    fn edits_are_checked_and_saved() {
        let mut data = admin();
        data.telegram = Some(Ok(TelegramConfig::default()));
        let mut view = TelegramView::default();
        run(
            Data {
                telegram: data.telegram.clone(),
                ..admin()
            },
            &mut view,
        );
        let form = view.form.as_mut().unwrap();
        form.enabled = true;
        form.token = "nonsense".into();
        assert!(matches!(view.edited(), Some(Err(_))));
        view.form.as_mut().unwrap().token = format!(" {TOKEN} ");
        view.chats = "42, x".into();
        assert!(view.edited().unwrap().unwrap_err().contains('x'));
        view.chats = "42, 43".into();
        let cfg = view.edited().unwrap().unwrap();
        assert_eq!(
            (cfg.token.as_str(), cfg.chats.as_slice()),
            (TOKEN, &[42, 43][..])
        );

        let mut saved = Vec::new();
        {
            let mut harness = Harness::builder().allow_missing_glyphs().build_ui(|ui| {
                for c in telegram(ui, &data, &mut view) {
                    if let UiCommand::SaveTelegram(c) = c {
                        saved.push(c);
                    }
                }
            });
            harness.run();
            harness.get_by_label("● unsaved changes");
            harness.get_by_label("Save").click();
            harness.run();
        }
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].chats, [42, 43]);
    }
}

//! The AI assistant: Settings → AI (provider, model, API key, consent) and
//! the window that asks about a process or the whole PC.
//!
//! Requests go from this window, with the user's own key, never from the
//! service. One request at a time, on a background thread; the model only
//! advises, it is never given control.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use eframe::egui::{self, RichText, Ui};
use wtm_ai::{AiConfig, KeySource, ProviderKind, Topic};
use wtm_core::control::ProcessRef;

use crate::background::Task;
use crate::theme::{BAD, GOOD, WARN, card};

/// The saved settings; defaults in tests, which never read the user's file.
fn load_config() -> Result<AiConfig, String> {
    #[cfg(test)]
    return Ok(AiConfig::default());
    #[cfg(not(test))]
    AiConfig::load()
}

/// The language answers come in when the settings name none.
fn ui_language() -> String {
    crate::i18n::Language::current().name().to_owned()
}

#[derive(Default)]
pub struct AiSettingsView {
    /// Being edited; loaded when the page is first shown.
    form: Option<AiConfig>,
    /// What is on disk.
    saved: Option<AiConfig>,
    load_error: Option<String>,
    /// A key typed but not yet stored.
    key: String,
    show_key: bool,
    models: Vec<String>,
    models_task: Task<Vec<String>>,
    test_task: Task<String>,
    /// The outcome of the last save, key change, test or model listing.
    status: Option<Result<String, String>>,
    /// Where the key comes from, worked out once per provider and stored
    /// key: opening the key every frame would keep it in memory for nothing.
    key_status: Option<((ProviderKind, Option<String>), String)>,
}

/// A button of the AI settings page.
#[derive(Debug, Clone, Copy)]
enum Action {
    Save,
    Forget,
    Models,
    Test,
}

impl AiSettingsView {
    fn act(&mut self, ctx: &egui::Context, action: Action) {
        let Some(form) = self.form.as_mut() else {
            return;
        };
        match action {
            Action::Save => {
                let key = std::mem::take(&mut self.key);
                if !key.trim().is_empty()
                    && let Err(e) = form.set_key(&key)
                {
                    self.status = Some(Err(e));
                    return;
                }
                self.save();
            }
            Action::Forget => {
                form.forget_key();
                self.save();
            }
            Action::Models => {
                let cfg = form.clone();
                self.status = None;
                self.models_task
                    .start(ctx, move || wtm_ai::Client::from_config(cfg)?.models());
            }
            Action::Test => {
                let cfg = form.clone();
                self.status = None;
                self.test_task.start(ctx, move || {
                    let client = wtm_ai::Client::from_config(cfg)?;
                    let answer =
                        client.complete("Reply with the single word OK.", "Connection test.")?;
                    Ok(format!("{} — {}", answer.model, answer.text))
                });
            }
        }
    }

    fn ensure_loaded(&mut self) {
        if self.form.is_some() || self.load_error.is_some() {
            return;
        }
        match load_config() {
            Ok(cfg) => {
                self.saved = Some(cfg.clone());
                self.form = Some(cfg);
            }
            Err(e) => self.load_error = Some(e),
        }
    }

    fn save(&mut self) {
        let Some(form) = self.form.as_mut() else {
            return;
        };
        form.model = form.model.trim().to_owned();
        form.base_url = form.base_url.trim().to_owned();
        form.language = form.language.trim().to_owned();
        #[cfg(not(test))]
        let saved = form.save();
        #[cfg(test)]
        let saved: Result<(), String> = Ok(());
        self.status = Some(match saved {
            Ok(()) => {
                self.saved = Some(form.clone());
                Ok(crate::i18n::tr("Saved.").to_owned())
            }
            Err(e) => Err(e),
        });
    }
}

/// Settings → AI assistant.
pub fn settings(ui: &mut Ui, view: &mut AiSettingsView) {
    view.ensure_loaded();
    if let Some(result) = view.models_task.poll() {
        view.status = Some(match result {
            Ok(models) if models.is_empty() => {
                Err(crate::i18n::tr("The provider listed no models.").to_owned())
            }
            Ok(models) => {
                let count = models.len();
                view.models = models;
                Ok(crate::i18n::format(
                    "{0} models available.",
                    &[count.to_string()],
                ))
            }
            Err(e) => Err(e),
        });
    }
    if let Some(result) = view.test_task.poll() {
        view.status = Some(result);
    }
    card(ui, crate::i18n::tr("AI assistant"), |ui| {
        ui.label(crate::i18n::tr(
            "Ask an AI model what a process is, whether it is safe, and how to make this PC faster. You bring the API key; requests go straight from this window to the provider you choose.",
        ));
        if let Some(e) = &view.load_error {
            ui.colored_label(crate::theme::text_color(ui, BAD), e);
            return;
        }
        let Some(form) = view.form.as_mut() else {
            return;
        };
        ui.add_space(4.0);
        ui.checkbox(
            &mut form.enabled,
            crate::i18n::tr("Send process facts to the AI provider when I ask"),
        );
        ui.label(
            RichText::new(crate::i18n::tr(
                "When you ask, the provider receives facts about the processes concerned: names, paths, command lines, user names, resource use, signatures, services, startup entries and network endpoints. If you let it look things up, it can also read any process, the network connections, services, startup programs and history of this PC. Secrets and your account name are masked (best effort). The AI only advises: it cannot change anything on this PC.",
            ))
            .small()
            .weak(),
        );
        ui.add_space(4.0);
        let field = (ui.available_width() - 200.0).clamp(160.0, 420.0);
        let mut provider_changed = false;
        egui::Grid::new("ai_form")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                let label = ui.label(crate::i18n::tr("Provider"));
                egui::ComboBox::from_id_salt("ai_provider")
                    .width(field)
                    .selected_text(form.provider.label())
                    .show_ui(ui, |ui| {
                        for p in ProviderKind::ALL {
                            if ui.selectable_label(form.provider == p, p.label()).clicked()
                                && form.provider != p
                            {
                                form.provider = p;
                                provider_changed = true;
                            }
                        }
                    })
                    .response
                    .labelled_by(label.id);
                ui.end_row();

                let label = ui.label(crate::i18n::tr("Model"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut form.model)
                            .desired_width(field - 110.0)
                            .hint_text(form.provider.default_model()),
                    )
                    .labelled_by(label.id);
                    if !view.models.is_empty() {
                        egui::ComboBox::from_id_salt("ai_models")
                            .width(100.0)
                            .selected_text(crate::i18n::tr("Choose"))
                            .show_ui(ui, |ui| {
                                for m in &view.models {
                                    if ui.selectable_label(form.model == *m, m).clicked() {
                                        form.model = m.clone();
                                    }
                                }
                            })
                            .response
                            .labelled_by(label.id);
                    }
                });
                ui.end_row();

                let label = ui.label(crate::i18n::tr("API address"));
                ui.add(
                    egui::TextEdit::singleline(&mut form.base_url)
                        .desired_width(field)
                        .hint_text(form.provider.default_base_url()),
                )
                .labelled_by(label.id);
                ui.end_row();

                let key_label = ui.label(crate::i18n::tr("API key"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut view.key)
                            .password(!view.show_key)
                            .desired_width(field - 70.0)
                            .hint_text(key_hint(form)),
                    )
                    .labelled_by(key_label.id);
                    let eye = if view.show_key { "🙈" } else { "👁" };
                    if ui
                        .small_button(eye)
                        .on_hover_text(crate::i18n::tr("Show or hide the key"))
                        .clicked()
                    {
                        view.show_key = !view.show_key;
                    }
                });
                ui.end_row();

                if form.provider.wire() == wtm_ai::Wire::Anthropic {
                    let label = ui.label(crate::i18n::tr("Effort"));
                    egui::ComboBox::from_id_salt("ai_effort")
                        .width(field)
                        .selected_text(effort_label(&form.effort))
                        .show_ui(ui, |ui| {
                            for e in ["", "low", "medium", "high", "xhigh", "max"] {
                                if ui
                                    .selectable_label(form.effort == e, effort_label(e))
                                    .clicked()
                                {
                                    form.effort = e.to_owned();
                                }
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    ui.end_row();
                }

                let label = ui.label(crate::i18n::tr("Answer language"));
                ui.add(
                    egui::TextEdit::singleline(&mut form.language)
                        .desired_width(field)
                        .hint_text(ui_language()),
                )
                .labelled_by(label.id);
                ui.end_row();
            });
        if provider_changed {
            // Another provider's model and address mean nothing here.
            form.model.clear();
            form.base_url.clear();
            view.models.clear();
            view.key.clear();
        }
        ui.checkbox(
            &mut form.send_command_lines,
            crate::i18n::tr("Send command lines (secrets in them are masked)"),
        );
        ui.checkbox(
            &mut form.use_tools,
            crate::i18n::tr("Let the AI look up more details when it needs them (read-only)"),
        );
        let which = (form.provider, form.keys.get(form.provider.id()).cloned());
        if view.key_status.as_ref().is_none_or(|(w, _)| *w != which) {
            view.key_status = Some((which, key_status(form)));
        }
        if let Some((_, text)) = &view.key_status {
            ui.label(RichText::new(text).small().weak());
        }
        if let Some(problem) = form.problem() {
            ui.colored_label(crate::theme::text_color(ui, WARN), problem);
        }

        ui.add_space(4.0);
        let dirty = view.saved.as_ref() != Some(&*form) || !view.key.trim().is_empty();
        let has_key = form.has_stored_key();
        let busy = view.models_task.busy() || view.test_task.busy();
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            if crate::theme::primary_action(ui, dirty, crate::i18n::tr("Save")).clicked() {
                action = Some(Action::Save);
            }
            if has_key
                && ui
                    .button(crate::i18n::tr("Forget the stored key"))
                    .clicked()
            {
                action = Some(Action::Forget);
            }
            if ui
                .add_enabled(
                    !busy && !dirty,
                    egui::Button::new(crate::i18n::tr("List models")),
                )
                .on_disabled_hover_text(crate::i18n::tr("Save first"))
                .clicked()
            {
                action = Some(Action::Models);
            }
            if ui
                .add_enabled(
                    !busy && !dirty,
                    egui::Button::new(crate::i18n::tr("Test connection")),
                )
                .on_disabled_hover_text(crate::i18n::tr("Save first"))
                .clicked()
            {
                action = Some(Action::Test);
            }
            if busy {
                ui.spinner();
            }
        });
        if let Some(action) = action {
            view.act(ui.ctx(), action);
        }
        match &view.status {
            Some(Ok(text)) => {
                ui.colored_label(crate::theme::text_color(ui, GOOD), text);
            }
            Some(Err(text)) => {
                ui.colored_label(crate::theme::text_color(ui, BAD), text);
            }
            None => {}
        }
    });
    card(
        ui,
        crate::i18n::tr("Use it from other AI apps (MCP)"),
        |ui| {
            ui.label(crate::i18n::tr(
            "Claude Desktop, Claude Code and other MCP clients can query RustWTM directly. Add this command as a stdio MCP server:",
        ));
            // wtm.exe is installed beside this window's executable.
            let wtm = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join("wtm.exe")))
                .map_or_else(|| "wtm.exe".to_owned(), |p| p.display().to_string());
            let command = format!("\"{wtm}\" mcp");
            ui.horizontal_wrapped(|ui| {
                ui.monospace(&command);
                if ui.small_button(crate::i18n::tr("Copy")).clicked() {
                    ui.ctx().copy_text(command.clone());
                }
            });
            ui.label(
            RichText::new(crate::i18n::tr(
                "Read-only by default; add --allow-control to let the assistant end, suspend or deprioritize processes.",
            ))
            .small()
            .weak(),
        );
        },
    );
}

fn key_hint(cfg: &AiConfig) -> String {
    if cfg.has_stored_key() {
        crate::i18n::tr("Stored — type a new one to replace it").to_owned()
    } else if cfg.provider.key_optional() {
        crate::i18n::tr("Not needed for a local server").to_owned()
    } else {
        crate::i18n::tr("Paste your API key").to_owned()
    }
}

fn key_status(cfg: &AiConfig) -> String {
    match cfg.key() {
        Ok(Some((_, KeySource::Stored))) => {
            crate::i18n::tr("Key stored, sealed for this Windows account.").to_owned()
        }
        Ok(Some((_, KeySource::Override))) => crate::i18n::format(
            "Key from the {0} environment variable.",
            &["RUSTWTM_AI_KEY".into()],
        ),
        Ok(Some((_, KeySource::Environment(var)))) => {
            crate::i18n::format("Key from the {0} environment variable.", &[var.to_owned()])
        }
        Ok(None) if cfg.provider.key_optional() => crate::i18n::tr("No key needed.").to_owned(),
        Ok(None) => crate::i18n::tr("No key yet.").to_owned(),
        Err(e) => e,
    }
}

fn effort_label(effort: &str) -> String {
    match effort {
        "" => crate::i18n::tr("Model default").to_owned(),
        e => e.to_owned(),
    }
}

/// What a request ends with: the opening it belongs to, and the answer
/// with the text that was sent, or why there is none.
type Outcome = (u64, Result<wtm_ai::Asked, String>);

/// Ready-made questions about the whole PC.
fn system_presets() -> [&'static str; 4] {
    [
        crate::i18n::tr("What slows this PC down?"),
        crate::i18n::tr("Which startup programs can I safely disable?"),
        crate::i18n::tr("Explain what RustWTM did recently"),
        crate::i18n::tr("Does anything look suspicious?"),
    ]
}

/// Ready-made questions about one process.
fn process_presets() -> [&'static str; 3] {
    [
        crate::i18n::tr("Is it safe to end?"),
        crate::i18n::tr("Why does it use so much CPU or memory?"),
        crate::i18n::tr("Could it be malware?"),
    ]
}

/// The question window.
#[derive(Default)]
pub struct AiChat {
    open: bool,
    topic: Option<Topic>,
    question: String,
    /// At most one request at a time, even across topics: a new one waits
    /// for the previous to finish, and that one's answer is dropped.
    task: Task<Outcome>,
    /// Which opening of the window a request belongs to.
    generation: u64,
    answer: Option<Result<wtm_ai::Asked, String>>,
    /// Stops the running request between steps.
    cancel: Arc<AtomicBool>,
    /// The tools the model called so far in the running request.
    progress: Arc<Mutex<Vec<String>>>,
    sent: Option<String>,
    show_sent: bool,
    /// Ask as soon as the window shows (it was opened from a menu).
    ask_now: bool,
    /// The settings, read when the window opened.
    cfg: Option<Result<AiConfig, String>>,
}

impl AiChat {
    pub fn open_process(&mut self, target: ProcessRef) {
        self.open_topic(Topic::Process(target));
    }

    pub fn open_system(&mut self) {
        self.open_topic(Topic::System);
    }

    fn open_topic(&mut self, topic: Topic) {
        // A request still running belongs to the previous topic: it is told
        // to stop, left to finish (never two at once) and its answer dropped.
        self.cancel.store(true, Ordering::Relaxed);
        self.generation += 1;
        self.topic = Some(topic);
        self.question.clear();
        self.answer = None;
        self.sent = None;
        self.show_sent = false;
        self.open = true;
        self.ask_now = true;
        self.cfg = Some(load_config());
    }

    fn title(&self) -> String {
        match &self.topic {
            Some(Topic::Process(t)) => crate::i18n::format(
                "Ask AI about {0} (PID {1})",
                &[t.name.clone(), t.pid.to_string()],
            ),
            _ => crate::i18n::tr("Ask AI about this PC").to_owned(),
        }
    }

    fn ask(&mut self, ctx: &egui::Context, cfg: AiConfig) {
        let Some(topic) = self.topic.clone() else {
            return;
        };
        let question = self.question.clone();
        let language = ui_language();
        let generation = self.generation;
        if self.task.busy() {
            return;
        }
        self.answer = None;
        self.sent = None;
        self.cancel = Arc::new(AtomicBool::new(false));
        self.progress = Arc::default();
        let (cancel, progress, repaint) = (self.cancel.clone(), self.progress.clone(), ctx.clone());
        self.task.start(ctx, move || {
            let result = wtm_ai::Client::from_config(cfg).and_then(|client| {
                let mut tools = wtm_ai::Toolbox::new(false);
                wtm_ai::ask(
                    &client,
                    &mut tools,
                    &topic,
                    &question,
                    &language,
                    &cancel,
                    &mut |name| {
                        progress
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(name.to_owned());
                        repaint.request_repaint();
                    },
                )
            });
            Ok((generation, result))
        });
    }
}

/// Draws the question window while it is open. Returns `true` when the
/// user wants the AI settings.
pub fn chat_window(ctx: &egui::Context, chat: &mut AiChat) -> bool {
    if !chat.open {
        return false;
    }
    match chat.task.poll() {
        Some(Ok((generation, result))) if generation == chat.generation => match result {
            Ok(asked) => {
                chat.sent = Some(asked.sent.clone());
                chat.answer = Some(Ok(asked));
            }
            Err(e) => chat.answer = Some(Err(e)),
        },
        Some(Err(e)) => chat.answer = Some(Err(e)),
        // An earlier topic's answer, or nothing yet.
        _ => {}
    }
    let cfg = chat.cfg.clone().unwrap_or_else(load_config);
    let ready = match &cfg {
        Ok(c) if !c.enabled => Err(crate::i18n::tr(
            "The AI assistant is off. Choose a provider and agree to send process facts in Settings → AI assistant.",
        )
        .to_owned()),
        Ok(c) => c.problem().map_or(Ok(()), Err),
        Err(e) => Err(e.clone()),
    };
    if chat.ask_now && !chat.task.busy() {
        chat.ask_now = false;
        if let (Ok(()), Ok(c)) = (&ready, &cfg) {
            chat.ask(ctx, c.clone());
        }
    }
    let mut open_settings = false;
    let mut close = false;
    let title = chat.title();
    let modal = egui::Modal::new(egui::Id::unique("ai-assistant")).show(ctx, |ui| {
        ui.set_width(ui.ctx().content_rect().width().clamp(320.0, 760.0) - 40.0);
        ui.heading(title);
        if let Err(why) = &ready {
            ui.colored_label(crate::theme::text_color(ui, WARN), why);
            ui.horizontal(|ui| {
                if ui.button(crate::i18n::tr("Open AI settings")).clicked() {
                    open_settings = true;
                }
                close = ui.button(crate::i18n::tr("Close")).clicked();
            });
            return;
        }
        let cfg = cfg.as_ref().expect("ready");
        ui.label(
            RichText::new(format!("{} · {}", cfg.provider.label(), cfg.model()))
                .small()
                .weak(),
        );
        ui.add(
            egui::TextEdit::multiline(&mut chat.question)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .hint_text(crate::i18n::tr(
                    "Your question (leave empty for a general explanation)",
                )),
        );
        let busy = chat.task.busy();
        let presets: Vec<&str> = match chat.topic {
            Some(Topic::Process(_)) => process_presets().to_vec(),
            _ => system_presets().to_vec(),
        };
        ui.horizontal_wrapped(|ui| {
            for preset in presets {
                if ui
                    .add_enabled(!busy, egui::Button::new(RichText::new(preset).small()))
                    .clicked()
                {
                    chat.question = preset.to_owned();
                    chat.ask(ui.ctx(), cfg.clone());
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            if crate::theme::primary_action(ui, !busy, crate::i18n::tr("Ask")).clicked() {
                chat.ask(ui.ctx(), cfg.clone());
            }
            if busy {
                if ui.button(crate::i18n::tr("Stop")).clicked() {
                    chat.cancel.store(true, Ordering::Relaxed);
                }
                ui.spinner();
                let looked = chat
                    .progress
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .join(", ");
                if looked.is_empty() {
                    ui.weak(crate::i18n::tr(
                        "Gathering facts and waiting for the answer…",
                    ));
                } else {
                    ui.weak(crate::i18n::format("Looked up: {0}", &[looked]));
                }
            }
        });
        ui.separator();
        match &chat.answer {
            Some(Ok(asked)) => {
                let answer = &asked.answer;
                egui::ScrollArea::vertical()
                    .max_height((ui.ctx().content_rect().height() * 0.55).max(200.0))
                    .show(ui, |ui| render_answer(ui, &answer.text));
                if answer.truncated {
                    ui.colored_label(
                        crate::theme::text_color(ui, WARN),
                        crate::i18n::tr("The answer was cut short at the token limit."),
                    );
                }
                let mut info = answer.model.clone();
                if let (Some(i), Some(o)) = (answer.input_tokens, answer.output_tokens) {
                    info.push_str(&format!(" · {i} → {o} tokens"));
                }
                ui.label(RichText::new(info).small().weak());
                if !asked.lookups.is_empty() {
                    ui.label(
                        RichText::new(crate::i18n::format(
                            "Looked up: {0}",
                            &[asked.lookups.join(", ")],
                        ))
                        .small()
                        .weak(),
                    );
                }
                if asked.tools_unsupported {
                    ui.label(
                        RichText::new(crate::i18n::tr(
                            "This model cannot look things up; it answered from the facts sent.",
                        ))
                        .small()
                        .weak(),
                    );
                }
            }
            Some(Err(e)) => {
                ui.colored_label(crate::theme::text_color(ui, BAD), e);
            }
            None if !busy => {
                ui.weak(crate::i18n::tr("No answer yet."));
            }
            None => {}
        }
        if chat.show_sent
            && let Some(sent) = &chat.sent
        {
            ui.separator();
            ui.label(RichText::new(crate::i18n::tr("Sent to the provider:")).strong());
            egui::ScrollArea::vertical()
                .id_salt("ai-sent")
                .max_height(220.0)
                .show(ui, |ui| {
                    ui.monospace(sent);
                });
        }
        ui.horizontal(|ui| {
            if let Some(Ok(asked)) = &chat.answer
                && ui.button(crate::i18n::tr("Copy answer")).clicked()
            {
                ui.ctx().copy_text(asked.answer.text.clone());
            }
            if chat.sent.is_some() {
                ui.toggle_value(&mut chat.show_sent, crate::i18n::tr("Show what was sent"));
            }
            close = ui.button(crate::i18n::tr("Close")).clicked();
        });
        ui.label(
            RichText::new(crate::i18n::tr(
                "AI answers can be wrong. Check before ending or removing anything.",
            ))
            .small()
            .weak(),
        );
    });
    if close || modal.should_close() {
        chat.open = false;
        chat.cancel.store(true, Ordering::Relaxed);
    }
    if open_settings {
        chat.open = false;
    }
    open_settings
}

/// The answer's light Markdown: headings, bullets and bold markers.
fn render_answer(ui: &mut Ui, text: &str) {
    let mut in_code = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            ui.monospace(line);
            continue;
        }
        let plain = trimmed.replace("**", "").replace('`', "");
        if let Some(heading) = plain.strip_prefix('#') {
            ui.add_space(4.0);
            ui.label(
                RichText::new(heading.trim_start_matches('#').trim())
                    .strong()
                    .size(15.0),
            );
        } else if let Some(item) = plain
            .strip_prefix("- ")
            .or_else(|| plain.strip_prefix("* "))
        {
            let indent = (line.len() - trimmed.len()) as f32 * 4.0;
            ui.horizontal_wrapped(|ui| {
                ui.add_space(indent);
                ui.label(format!("•  {item}"));
            });
        } else if plain.is_empty() {
            ui.add_space(4.0);
        } else {
            ui.add(egui::Label::new(plain).wrap());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn harness(chat: AiChat) -> egui_kittest::Harness<'static, AiChat> {
        egui_kittest::Harness::builder()
            .allow_missing_glyphs()
            .with_size([900.0, 700.0])
            .build_ui_state(
                |ui, chat: &mut AiChat| {
                    chat_window(ui.ctx(), chat);
                },
                chat,
            )
    }

    #[test]
    fn the_window_sends_nothing_while_ai_is_off() {
        use egui_kittest::kittest::Queryable;
        let mut chat = AiChat::default();
        chat.open_system();
        // Tests read the defaults: AI off.
        let mut h = harness(chat);
        h.run();
        h.get_by_label(crate::i18n::tr("Open AI settings"));
        assert!(!h.state().task.busy(), "nothing was asked");
        assert!(h.state().open);
    }

    #[test]
    fn the_window_asks_once_and_shows_why_it_failed() {
        use egui_kittest::kittest::Queryable;
        let mut chat = AiChat::default();
        chat.open_process(ProcessRef {
            pid: 42,
            create_time: 1,
            name: "app.exe".into(),
        });
        chat.cfg = Some(Ok(AiConfig {
            enabled: true,
            ..AiConfig::default()
        }));
        let mut h = harness(chat);
        h.run();
        // The synthetic background task fails at once; its error is shown.
        assert!(!h.state().ask_now);
        let error = h.state().answer.clone();
        assert!(matches!(error, Some(Err(_))), "{error:?}");
        h.get_by_label(crate::i18n::tr("Ask"));
        h.get_by_label(crate::i18n::tr("Close")).click();
        h.run();
        assert!(!h.state().open);
    }

    #[test]
    fn the_settings_page_renders_for_every_provider() {
        use egui_kittest::kittest::Queryable;
        for provider in ProviderKind::ALL {
            let mut view = AiSettingsView::default();
            view.ensure_loaded();
            if let Some(form) = view.form.as_mut() {
                form.provider = provider;
            }
            let mut h = egui_kittest::Harness::builder()
                .allow_missing_glyphs()
                .with_size([760.0, 900.0])
                .build_ui_state(|ui, view: &mut AiSettingsView| settings(ui, view), view);
            h.run();
            h.get_by_label(crate::i18n::tr("Provider"));
            h.get_by_label(crate::i18n::tr("Test connection"));
            assert_eq!(
                h.query_by_label(crate::i18n::tr("Effort")).is_some(),
                provider == ProviderKind::Anthropic,
                "{provider:?}"
            );
        }
    }

    #[test]
    fn opening_a_topic_resets_the_window_and_asks() {
        let mut chat = AiChat {
            question: "old".into(),
            answer: Some(Err("old".into())),
            ..AiChat::default()
        };
        chat.open_process(ProcessRef {
            pid: 42,
            create_time: 1,
            name: "app.exe".into(),
        });
        assert!(chat.open && chat.ask_now);
        assert!(chat.question.is_empty() && chat.answer.is_none());
        assert!(chat.title().contains("app.exe") && chat.title().contains("42"));
        chat.open_system();
        assert!(matches!(chat.topic, Some(Topic::System)));
    }
}

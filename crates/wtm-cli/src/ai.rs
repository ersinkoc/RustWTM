//! `wtm ai`: ask an AI provider about a process or the whole PC.
//!
//! Settings and the DPAPI-sealed key live in `%APPDATA%\RustWTM\ai.toml`,
//! shared with the RustWTM window. Keys are read from standard input,
//! never from the command line, where every program could read them.

use std::io::{self, BufRead, IsTerminal, Write};
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::Subcommand;
use wtm_ai::{AiConfig, Client, KeySource, ProviderKind, Toolbox, Topic};
use wtm_core::control::ProcessRef;

#[derive(Subcommand)]
pub enum AiCommand {
    /// Show the provider, model and where the API key comes from
    Status,
    /// Choose the provider and model, and agree to send process facts to it
    Setup {
        /// anthropic, openai, gemini, openrouter, groq, deepseek, mistral,
        /// xai, ollama, lmstudio or custom
        #[arg(long)]
        provider: String,
        /// Model ID (see `wtm ai models`); empty keeps the provider default
        #[arg(long)]
        model: Option<String>,
        /// API address, for custom or self-hosted servers
        #[arg(long, value_name = "URL")]
        base_url: Option<String>,
        /// Anthropic effort: low, medium, high, xhigh, max ("" to omit)
        #[arg(long)]
        effort: Option<String>,
        /// Longest answer in tokens
        #[arg(long)]
        max_tokens: Option<u32>,
        /// Language of the answers, e.g. "Türkçe" (default: Windows')
        #[arg(long)]
        language: Option<String>,
        /// Never send command lines (they are masked for secrets otherwise)
        #[arg(long, conflicts_with = "command_lines")]
        no_command_lines: bool,
        /// Send command lines again, masked for secrets
        #[arg(long)]
        command_lines: bool,
        /// Let the model look up more with read-only tools (any process,
        /// connections, services, startup programs, history)
        #[arg(long, value_name = "on|off", value_parser = on_off)]
        lookups: Option<bool>,
    },
    /// Store an API key for the current provider, sealed for this Windows
    /// account (typed or piped in; never put it on the command line)
    Key {
        /// Remove the stored key instead
        #[arg(long)]
        forget: bool,
    },
    /// List the models the provider offers to this key
    Models,
    /// Send a tiny request to check the key and the model
    Test,
    /// Explain a process: what it is, whether it is safe, whether to end it
    Explain {
        /// PID or image name
        target: String,
        /// Your own question about it
        #[arg(long, short)]
        question: Option<String>,
        /// Print what would be sent, without sending it
        #[arg(long)]
        dry_run: bool,
    },
    /// Advice for the whole PC, or an answer to your question about it
    Advise {
        /// Your question, e.g. "why is my fan loud?"
        question: Option<String>,
        /// Print what would be sent, without sending it
        #[arg(long)]
        dry_run: bool,
    },
    /// Stop using AI (settings and keys stay until `wtm ai key --forget`)
    Off,
}

pub fn run(command: AiCommand) -> anyhow::Result<ExitCode> {
    let mut cfg = AiConfig::load().map_err(anyhow::Error::msg)?;
    let out = &mut io::stdout().lock();
    match command {
        AiCommand::Status => status(&cfg, out)?,
        AiCommand::Setup {
            provider,
            model,
            base_url,
            effort,
            max_tokens,
            language,
            no_command_lines,
            command_lines,
            lookups,
        } => {
            let provider = ProviderKind::from_id(&provider).with_context(|| {
                format!(
                    "unknown provider '{provider}'; one of: {}",
                    ProviderKind::ALL.map(ProviderKind::id).join(", ")
                )
            })?;
            setup(
                &mut cfg,
                provider,
                model,
                base_url,
                effort,
                max_tokens,
                language,
                no_command_lines
                    .then_some(false)
                    .or(command_lines.then_some(true)),
                lookups,
            )?;
            cfg.save().map_err(anyhow::Error::msg)?;
            writeln!(out, "{}", consent_text(&cfg))?;
            if !cfg.has_stored_key()
                && !cfg.provider.key_optional()
                && cfg.key().ok().flatten().is_none()
            {
                writeln!(out, "Next: store your API key with `wtm ai key`.")?;
            }
            if let Some(problem) = cfg.problem() {
                writeln!(out, "Still needed: {problem}.")?;
            }
        }
        AiCommand::Key { forget } => {
            if forget {
                cfg.forget_key();
                cfg.save().map_err(anyhow::Error::msg)?;
                writeln!(out, "The {} key is removed.", cfg.provider.label())?;
            } else {
                let key = read_key(cfg.provider)?;
                if key.is_empty() {
                    bail!("no key entered");
                }
                cfg.set_key(&key).map_err(anyhow::Error::msg)?;
                cfg.save().map_err(anyhow::Error::msg)?;
                writeln!(
                    out,
                    "The {} key is stored, sealed for this Windows account. Check it with `wtm ai test`.",
                    cfg.provider.label()
                )?;
            }
        }
        AiCommand::Models => {
            let client = client(&cfg, false)?;
            for id in client.models().map_err(anyhow::Error::msg)? {
                let mark = if id == cfg.model() {
                    "  (selected)"
                } else {
                    ""
                };
                writeln!(out, "{id}{mark}")?;
            }
        }
        AiCommand::Test => {
            let client = client(&cfg, false)?;
            let answer = client
                .complete("Reply with the single word OK.", "Connection test.")
                .map_err(anyhow::Error::msg)?;
            writeln!(
                out,
                "{} answered with {}: {}",
                cfg.provider.label(),
                if answer.model.is_empty() {
                    cfg.model()
                } else {
                    &answer.model
                },
                answer.text
            )?;
        }
        AiCommand::Explain {
            target,
            question,
            dry_run,
        } => {
            let mut tools = Toolbox::new(false);
            let snap = tools.snapshot().map_err(anyhow::Error::msg)?;
            let matches = crate::actions::matching(std::slice::from_ref(&target), &snap.processes)?;
            // Several with that name: the busiest, which is usually the one asked about.
            let chosen = matches
                .iter()
                .max_by(|a, b| a.cpu_percent.total_cmp(&b.cpu_percent))
                .expect("matching returns at least one");
            if matches.len() > 1 {
                eprintln!(
                    "{} processes match '{target}'; explaining the busiest, PID {}.",
                    matches.len(),
                    chosen.pid
                );
            }
            let topic = Topic::Process(ProcessRef::from(*chosen));
            ask(
                &cfg,
                &mut tools,
                &topic,
                question.as_deref().unwrap_or(""),
                dry_run,
                out,
            )?;
        }
        AiCommand::Advise { question, dry_run } => {
            let mut tools = Toolbox::new(false);
            ask(
                &cfg,
                &mut tools,
                &Topic::System,
                question.as_deref().unwrap_or(""),
                dry_run,
                out,
            )?;
        }
        AiCommand::Off => {
            cfg.enabled = false;
            cfg.save().map_err(anyhow::Error::msg)?;
            writeln!(
                out,
                "AI is off; nothing is sent until `wtm ai setup` again."
            )?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Applies `wtm ai setup` to `cfg` and turns AI on.
#[allow(clippy::too_many_arguments)]
fn setup(
    cfg: &mut AiConfig,
    provider: ProviderKind,
    model: Option<String>,
    base_url: Option<String>,
    effort: Option<String>,
    max_tokens: Option<u32>,
    language: Option<String>,
    command_lines: Option<bool>,
    lookups: Option<bool>,
) -> anyhow::Result<()> {
    if provider != cfg.provider {
        // Another provider's model and address mean nothing here.
        cfg.model.clear();
        cfg.base_url.clear();
    }
    cfg.provider = provider;
    if let Some(m) = model {
        cfg.model = m.trim().to_owned();
    }
    if let Some(u) = base_url {
        cfg.base_url = u.trim().to_owned();
    }
    if let Some(e) = effort {
        let e = e.trim().to_ascii_lowercase();
        if !matches!(e.as_str(), "" | "low" | "medium" | "high" | "xhigh" | "max") {
            bail!("effort must be low, medium, high, xhigh or max");
        }
        cfg.effort = e;
    }
    if let Some(t) = max_tokens {
        cfg.max_tokens = t.clamp(256, 64_000);
    }
    if let Some(l) = language {
        cfg.language = l.trim().to_owned();
    }
    if let Some(c) = command_lines {
        cfg.send_command_lines = c;
    }
    if let Some(l) = lookups {
        cfg.use_tools = l;
    }
    cfg.enabled = true;
    Ok(())
}

/// What is sent where, said once when AI is turned on.
fn consent_text(cfg: &AiConfig) -> String {
    let destination = if cfg.provider.is_local(cfg.base_url()) {
        format!("{} on this PC ({})", cfg.provider.label(), cfg.base_url())
    } else {
        format!("{} at {}", cfg.provider.label(), cfg.base_url())
    };
    format!(
        "AI is on: {} with model {}.\n\
         When you ask, RustWTM sends {destination}: process names, paths, {}user names, \
         resource use, signatures, services, startup programs and network endpoints of the \
         processes concerned.{} Secrets in them are masked and your account name is hidden, \
         but masking is best effort. The model only advises; it cannot act on this PC.",
        cfg.provider.label(),
        if cfg.model().is_empty() {
            "(none chosen)"
        } else {
            cfg.model()
        },
        if cfg.send_command_lines {
            "command lines, "
        } else {
            ""
        },
        if cfg.use_tools {
            " The model may also look up more on its own: any process, network connections, \
             services, startup programs and history of this PC (read-only; turn it off with \
             `--lookups off`)."
        } else {
            ""
        },
    )
}

/// `on` / `off` (also yes/no, true/false).
fn on_off(text: &str) -> Result<bool, String> {
    match text.trim().to_ascii_lowercase().as_str() {
        "on" | "yes" | "true" | "1" => Ok(true),
        "off" | "no" | "false" | "0" => Ok(false),
        _ => Err("expected on or off".into()),
    }
}

fn status(cfg: &AiConfig, out: &mut impl Write) -> anyhow::Result<()> {
    writeln!(out, "AI:        {}", if cfg.enabled { "on" } else { "off" })?;
    writeln!(
        out,
        "provider:  {} ({})",
        cfg.provider.label(),
        cfg.provider.id()
    )?;
    writeln!(
        out,
        "model:     {}",
        if cfg.model().is_empty() {
            "(none chosen)"
        } else {
            cfg.model()
        }
    )?;
    writeln!(out, "address:   {}", cfg.base_url())?;
    let key = match cfg.key() {
        Ok(Some((_, KeySource::Override))) => "from RUSTWTM_AI_KEY".to_owned(),
        Ok(Some((_, KeySource::Stored))) => "stored, sealed for this Windows account".to_owned(),
        Ok(Some((_, KeySource::Environment(var)))) => format!("from {var}"),
        Ok(None) if cfg.provider.key_optional() => "none (not needed)".to_owned(),
        Ok(None) => "MISSING (`wtm ai key`)".to_owned(),
        Err(e) => format!("unusable: {e}"),
    };
    writeln!(out, "key:       {key}")?;
    writeln!(
        out,
        "command lines: {}",
        if cfg.send_command_lines {
            "sent, secrets masked"
        } else {
            "never sent"
        }
    )?;
    writeln!(
        out,
        "lookups:   {}",
        if cfg.use_tools {
            "on (read-only tools)"
        } else {
            "off"
        }
    )?;
    if let Some(path) = AiConfig::path() {
        writeln!(out, "settings:  {}", path.display())?;
    }
    if let Some(problem) = cfg.problem() {
        writeln!(out, "problem:   {problem}")?;
    }
    Ok(())
}

/// A client for `cfg`; `needs_consent` refuses while AI is off.
fn client(cfg: &AiConfig, needs_consent: bool) -> anyhow::Result<Client> {
    if needs_consent && !cfg.enabled {
        bail!("AI is off; turn it on with `wtm ai setup --provider …` (it says what is sent)");
    }
    Client::from_config(cfg.clone()).map_err(anyhow::Error::msg)
}

fn ask(
    cfg: &AiConfig,
    tools: &mut Toolbox,
    topic: &Topic,
    question: &str,
    dry_run: bool,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    if dry_run {
        let text = wtm_ai::preview(cfg, tools, topic, question).map_err(anyhow::Error::msg)?;
        writeln!(out, "{text}")?;
        return Ok(());
    }
    let client = client(cfg, true)?;
    eprintln!("Asking {} ({})…", cfg.provider.label(), cfg.model());
    let language = windows_language();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let asked = wtm_ai::ask(
        &client,
        tools,
        topic,
        question,
        &language,
        &cancel,
        &mut |name| eprintln!("  looking up {name}…"),
    )
    .map_err(anyhow::Error::msg)?;
    if asked.tools_unsupported {
        eprintln!("(this model takes no tools; it answered from the facts sent)");
    }
    let answer = asked.answer;
    writeln!(out, "{}", answer.text)?;
    if answer.truncated {
        writeln!(
            out,
            "\n(cut short at the token limit; raise it with `wtm ai setup --max-tokens`)"
        )?;
    }
    if let (Some(i), Some(o)) = (answer.input_tokens, answer.output_tokens) {
        eprintln!(
            "[{}: {i} tokens in, {o} out]",
            if answer.model.is_empty() {
                cfg.model()
            } else {
                &answer.model
            }
        );
    }
    Ok(())
}

/// The Windows display language as a tag the model understands (`tr-TR`).
fn windows_language() -> String {
    wtm_core::system_ui_language()
}

/// Reads a key from standard input: hidden while typed at a console,
/// one line when piped.
fn read_key(provider: ProviderKind) -> anyhow::Result<String> {
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        return Ok(line.trim().to_owned());
    }
    eprint!("{} API key (input hidden): ", provider.label());
    io::stderr().flush()?;
    let key = read_hidden()?;
    eprintln!();
    Ok(key.trim().to_owned())
}

fn read_hidden() -> anyhow::Result<String> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::terminal;

    struct Raw;
    impl Drop for Raw {
        fn drop(&mut self) {
            let _ = terminal::disable_raw_mode();
        }
    }
    terminal::enable_raw_mode()?;
    let _raw = Raw;
    let mut key = String::new();
    loop {
        match event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::Enter => break,
                KeyCode::Esc => return Ok(String::new()),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(String::new());
                }
                KeyCode::Backspace => {
                    key.pop();
                }
                KeyCode::Char(c) => key.push(c),
                _ => {}
            },
            // A pasted key arrives in one piece when bracketed paste is on.
            Event::Paste(text) => key.push_str(&text),
            _ => {}
        }
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_switches_provider_and_turns_ai_on() {
        let mut cfg = AiConfig {
            model: "claude-sonnet-5-5".into(),
            base_url: "https://proxy.example/v1".into(),
            ..AiConfig::default()
        };
        setup(
            &mut cfg,
            ProviderKind::Groq,
            Some(" llama-x ".into()),
            None,
            None,
            Some(10),
            Some("Türkçe".into()),
            Some(false),
            Some(false),
        )
        .unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.provider, ProviderKind::Groq);
        assert_eq!(cfg.model, "llama-x");
        assert_eq!(cfg.base_url, "", "the old provider's address is dropped");
        assert_eq!(cfg.max_tokens, 256, "clamped");
        assert_eq!(cfg.language, "Türkçe");
        assert!(!cfg.send_command_lines);
        assert!(!cfg.use_tools);
        assert_eq!(on_off("ON"), Ok(true));
        assert!(on_off("maybe").is_err());
        assert!(
            setup(
                &mut cfg,
                ProviderKind::Groq,
                None,
                None,
                Some("huge".into()),
                None,
                None,
                None,
                None
            )
            .is_err()
        );
        // The same provider keeps its model.
        setup(
            &mut cfg,
            ProviderKind::Groq,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(cfg.model, "llama-x");
    }

    #[test]
    fn consent_names_what_is_sent_and_where() {
        let cfg = AiConfig {
            enabled: true,
            ..AiConfig::default()
        };
        let text = consent_text(&cfg);
        assert!(text.contains("https://api.anthropic.com/v1"), "{text}");
        assert!(text.contains("command lines"), "{text}");
        assert!(text.contains("cannot act on this PC"), "{text}");
        assert!(text.contains("look up more on its own"), "{text}");
        let local = AiConfig {
            provider: ProviderKind::Ollama,
            send_command_lines: false,
            ..cfg
        };
        let text = consent_text(&local);
        assert!(text.contains("on this PC"), "{text}");
        assert!(!text.contains("command lines"), "{text}");
    }

    #[test]
    fn asking_needs_consent() {
        let cfg = AiConfig::default();
        let e = client(&cfg, true).err().unwrap().to_string();
        assert!(e.contains("AI is off"), "{e}");
    }
}

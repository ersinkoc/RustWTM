//! AI assistance for RustWTM.
//!
//! - [`tools`]: what RustWTM can tell about this PC, as named read-only
//!   tools (plus control tools when allowed). `wtm mcp` serves them to AI
//!   assistants over the Model Context Protocol.
//! - [`Client`]: asks a provider (Anthropic, OpenAI or any OpenAI-compatible
//!   server, local ones included) with facts gathered by the tools; with
//!   [`agent`], the model may look up more with read-only tools in a bounded
//!   conversation. The model only advises; it is never given control.
//! - [`AiConfig`]: the provider, model and DPAPI-sealed API keys, per user.
//! - [`redact`]: masks secrets and the account name before anything is sent.

#![forbid(unsafe_code)]

pub mod agent;
pub mod config;
pub mod prompt;
pub mod provider;
pub mod redact;
pub mod tools;

use std::sync::atomic::AtomicBool;

pub use agent::{Limits, ToolCall};
pub use config::{AiConfig, KeySource, ProviderKind, Wire};
pub use prompt::Topic;
pub use provider::{Answer, Client, Transport, WinHttp};
pub use redact::Redactor;
pub use tools::{READ_ONLY_TOOLS, Toolbox};

/// The most one tool result may add to the conversation.
const MAX_RESULT: usize = 20_000;

/// What [`ask`] got.
#[derive(Debug, Clone, PartialEq)]
pub struct Asked {
    pub answer: Answer,
    /// Everything sent to the provider: the question with its facts, then
    /// each lookup and its result.
    pub sent: String,
    /// The tools the model called, in order.
    pub lookups: Vec<String>,
    /// Lookups were on, but the provider or model has no tools.
    pub tools_unsupported: bool,
}

/// Asks the configured provider about `topic`: gathers the facts, masks
/// secrets and the account name (local providers too), and sends them.
/// With `use_tools` on, the model may look up more with read-only tools,
/// each result masked and fenced like the facts; `progress` hears the name
/// of each. `cancel` stops between steps. `question` empty: the default
/// question for the topic.
pub fn ask<T: Transport>(
    client: &Client<T>,
    tools: &mut Toolbox,
    topic: &Topic,
    question: &str,
    language: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<Asked, String> {
    let cfg = client.config();
    tools.command_lines = cfg.send_command_lines;
    let facts = prompt::gather(tools, topic)?;
    let redactor = Redactor::for_current_user();
    let question = match question.trim() {
        "" => prompt::default_question(topic),
        q => q.to_owned(),
    };
    let language = match cfg.language.trim() {
        "" => language,
        l => l,
    };
    let user = prompt::user_message(&question, &facts, Some(&redactor));
    let offered = if cfg.use_tools {
        tools.read_only_tools()
    } else {
        Vec::new()
    };
    let system = prompt::system_prompt(language, !offered.is_empty());
    let mut run = |name: &str, args: &serde_json::Value| -> Result<String, String> {
        // The loop offers only these; checked again here, by name.
        if !READ_ONLY_TOOLS.contains(&name) {
            return Err(format!("{name} is not available to the assistant"));
        }
        progress(name);
        match tools.run(name, args) {
            Ok(text) => Ok(prompt::fenced_capped(&text, Some(&redactor), MAX_RESULT)),
            Err(e) => Err(redactor.redact(&e)),
        }
    };
    let done = client.converse(
        &system,
        &user,
        &offered,
        &mut run,
        cancel,
        Limits::default(),
    )?;
    let mut sent = user;
    for call in &done.calls {
        sent.push_str(&format!(
            "\n\n--- lookup: {} {}\n{}",
            call.name, call.arguments, call.result
        ));
    }
    Ok(Asked {
        lookups: done.calls.iter().map(|c| c.name.clone()).collect(),
        answer: done.answer,
        sent,
        tools_unsupported: done.tools_unsupported,
    })
}

/// [`ask`] without sending: the text that would go to the provider first.
pub fn preview(
    cfg: &AiConfig,
    tools: &mut Toolbox,
    topic: &Topic,
    question: &str,
) -> Result<String, String> {
    tools.command_lines = cfg.send_command_lines;
    let facts = prompt::gather(tools, topic)?;
    let question = match question.trim() {
        "" => prompt::default_question(topic),
        q => q.to_owned(),
    };
    Ok(prompt::user_message(
        &question,
        &facts,
        Some(&Redactor::for_current_user()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::cell::RefCell;

    /// A provider that looks up this process once, then answers.
    struct TwoRounds {
        bodies: RefCell<Vec<Value>>,
    }

    impl Transport for &TwoRounds {
        fn send(
            &self,
            _: &str,
            _: &str,
            _: &[(&str, &str)],
            body: Option<&[u8]>,
        ) -> Result<(u16, Vec<u8>), String> {
            let body: Value = serde_json::from_slice(body.unwrap()).unwrap();
            let round = self.bodies.borrow().len();
            self.bodies.borrow_mut().push(body);
            let reply = if round == 0 {
                json!({ "stop_reason": "tool_use", "content": [
                    { "type": "tool_use", "id": "t1", "name": "process_tree", "input": { "pid": std::process::id() } },
                    { "type": "tool_use", "id": "t2", "name": "end_process", "input": { "pid": 1 } },
                ] })
            } else {
                json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": "ok" }] })
            };
            Ok((200, reply.to_string().into_bytes()))
        }
    }

    #[test]
    fn lookups_are_read_only_masked_fenced_and_shown_as_sent() {
        let provider = TwoRounds {
            bodies: RefCell::default(),
        };
        let cfg = AiConfig {
            enabled: true,
            ..AiConfig::default()
        };
        let client = Client::new(cfg, Some("k".into()), &provider).unwrap();
        let mut tools = Toolbox::new(false);
        tools.use_service = false;
        let mut heard = Vec::new();
        let asked = ask(
            &client,
            &mut tools,
            &Topic::System,
            "",
            "English",
            &AtomicBool::new(false),
            &mut |name| heard.push(name.to_owned()),
        )
        .unwrap();
        assert_eq!(asked.answer.text, "ok");
        assert_eq!(heard, ["process_tree"], "end_process never ran");
        assert_eq!(asked.lookups, ["process_tree", "end_process"]);
        assert!(asked.sent.contains("--- lookup: process_tree"));
        let bodies = provider.bodies.borrow();
        let offered: Vec<&str> = bodies[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(offered.contains(&"process_report") && !offered.contains(&"end_process"));
        assert!(
            bodies[0]["system"]
                .as_str()
                .unwrap()
                .contains("read-only tools")
        );
        let results = bodies[1]["messages"][2]["content"].as_array().unwrap();
        let tree = results[0]["content"].as_str().unwrap();
        assert!(tree.starts_with("<rustwtm-data>") && tree.ends_with("</rustwtm-data>"));
        assert!(!tree.contains(&std::env::var("USERNAME").unwrap_or("\u{0}".into())));
        assert_eq!(results[1]["is_error"], true);
    }
}

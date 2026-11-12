//! A bounded conversation in which the model may look things up: it asks
//! for read-only tools by name, RustWTM runs them and sends the results
//! back, until the model answers. Only tools in the offered list run (any
//! other name gets an error result), the number of rounds, the size of
//! what is resent and the total time are capped, and the caller can cancel
//! between steps.
//!
//! Each assistant turn goes back to the provider exactly as it came
//! (thinking blocks, signatures and tool calls untouched): providers check
//! that the history they wrote is unchanged.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::config::Wire;
use crate::provider::{Answer, Client, Transport, anthropic_request, openai_request};
use crate::provider::{parse_anthropic, parse_openai};

/// How far a conversation may go.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Requests in all, the last of which may not call tools.
    pub rounds: usize,
    /// Characters of all messages resent each round; past it, the model
    /// must answer with what it has.
    pub transcript: usize,
    /// Wall-clock time for the whole conversation.
    pub deadline: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            rounds: 6,
            transcript: 150_000,
            deadline: Duration::from_secs(300),
        }
    }
}

/// One tool the model called.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
    /// What was sent back to the model.
    pub result: String,
    pub failed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    pub answer: Answer,
    pub calls: Vec<ToolCall>,
    /// The provider had no tools; the answer came without them.
    pub tools_unsupported: bool,
}

/// Runs a tool: `Ok(text)` or `Err(text)`, both sent to the model.
pub type Runner<'a> = dyn FnMut(&str, &Value) -> Result<String, String> + 'a;

const CANCELLED: &str = "cancelled";

impl<T: Transport> Client<T> {
    /// Asks with `tools` (MCP descriptors: name, description, inputSchema)
    /// on offer. `run` executes the ones the model calls; `cancel` stops
    /// the conversation between steps.
    pub fn converse(
        &self,
        system: &str,
        user: &str,
        tools: &[Value],
        run: &mut Runner<'_>,
        cancel: &AtomicBool,
        limits: Limits,
    ) -> Result<Conversation, String> {
        if let Some(problem) = self.config().problem() {
            return Err(problem);
        }
        if tools.is_empty() {
            let answer = self.complete(system, user)?;
            return Ok(Conversation {
                answer,
                calls: Vec::new(),
                tools_unsupported: false,
            });
        }
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        let mut step = Step {
            names: &names,
            run,
            cancel,
            calls: Vec::new(),
            started: Instant::now(),
            limits,
        };
        match self.config().provider.wire() {
            Wire::Anthropic => self.converse_anthropic(system, user, tools, &mut step),
            Wire::OpenAi => self.converse_openai(system, user, tools, &mut step),
        }
    }

    fn converse_anthropic(
        &self,
        system: &str,
        user: &str,
        tools: &[Value],
        step: &mut Step<'_, '_>,
    ) -> Result<Conversation, String> {
        let (base, _) = anthropic_request(self.config(), system, user);
        let betas = self.anthropic_betas();
        let url = format!("{}/messages", self.config().base_url());
        let offered: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "name": t["name"],
                    "description": t["description"],
                    "input_schema": t["inputSchema"],
                })
            })
            .collect();
        let mut messages = vec![json!({ "role": "user", "content": user })];
        let mut usage = (0u64, 0u64);
        for round in 0..step.limits.rounds.max(1) {
            step.check()?;
            let last = step.last_round(round, &messages);
            let mut body = base.clone();
            body["messages"] = json!(messages);
            body["tools"] = json!(offered);
            body["tool_choice"] = if last {
                json!({ "type": "none" })
            } else {
                json!({ "type": "auto" })
            };
            // The growing history is resent each round: cache it.
            body["cache_control"] = json!({ "type": "ephemeral" });
            let reply = self
                .send_status("POST", &url, Some(&body), &betas)
                .map_err(|(_, e)| e)?;
            usage.0 += reply["usage"]["input_tokens"].as_u64().unwrap_or(0);
            usage.1 += reply["usage"]["output_tokens"].as_u64().unwrap_or(0);
            let stop = reply["stop_reason"].as_str().unwrap_or("");
            let uses: Vec<&Value> = reply["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| b["type"] == "tool_use")
                .collect();
            // A refusal or a cut-off reply is never acted on.
            if stop != "tool_use" || uses.is_empty() || last {
                let mut answer = parse_anthropic(&reply)?;
                answer.input_tokens = Some(usage.0);
                answer.output_tokens = Some(usage.1);
                return Ok(step.finish(answer, false));
            }
            // The turn as it came, then every result in one message.
            messages.push(json!({ "role": "assistant", "content": reply["content"] }));
            let mut results = Vec::new();
            for u in uses {
                let (text, failed) = step.call(
                    u["name"].as_str().unwrap_or(""),
                    u.get("input").cloned().unwrap_or(json!({})),
                )?;
                let mut result = json!({
                    "type": "tool_result",
                    "tool_use_id": u["id"],
                    "content": text,
                });
                if failed {
                    result["is_error"] = json!(true);
                }
                results.push(result);
            }
            messages.push(json!({ "role": "user", "content": results }));
        }
        Err("the conversation ended without an answer".into())
    }

    fn converse_openai(
        &self,
        system: &str,
        user: &str,
        tools: &[Value],
        step: &mut Step<'_, '_>,
    ) -> Result<Conversation, String> {
        let base = openai_request(self.config(), system, user);
        let url = format!("{}/chat/completions", self.config().base_url());
        let offered: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t["name"],
                        "description": t["description"],
                        "parameters": t["inputSchema"],
                    },
                })
            })
            .collect();
        let mut messages = base["messages"].as_array().cloned().unwrap_or_default();
        let mut usage = (0u64, 0u64);
        for round in 0..step.limits.rounds.max(1) {
            step.check()?;
            let last = step.last_round(round, &messages);
            let mut body = base.clone();
            body["messages"] = json!(messages);
            body["tools"] = json!(offered);
            if last {
                body["tool_choice"] = json!("none");
            }
            let reply = match self.send_status("POST", &url, Some(&body), &[]) {
                Ok(reply) => reply,
                // A server without tools (many local models) refuses the
                // first request; it is asked once more without them. Never
                // on a key, permission or quota error.
                Err((Some(400 | 422), _)) if round == 0 => {
                    step.check()?;
                    let answer = self.complete(system, user)?;
                    return Ok(step.finish(answer, true));
                }
                Err((_, e)) => return Err(e),
            };
            usage.0 += reply["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
            usage.1 += reply["usage"]["completion_tokens"].as_u64().unwrap_or(0);
            let message = &reply["choices"][0]["message"];
            let calls: Vec<&Value> = message["tool_calls"]
                .as_array()
                .into_iter()
                .flatten()
                .collect();
            if calls.is_empty() || last {
                let mut answer = parse_openai(&reply)?;
                answer.input_tokens = Some(usage.0);
                answer.output_tokens = Some(usage.1);
                return Ok(step.finish(answer, false));
            }
            // The message as it came (a null content, tool calls and any
            // provider fields kept), then one tool message per call.
            messages.push(message.clone());
            for c in calls {
                let name = c["function"]["name"].as_str().unwrap_or("");
                let raw = c["function"]["arguments"].as_str().unwrap_or("");
                let (text, _) = match parse_arguments(raw, &c["function"]["arguments"]) {
                    Ok(args) => step.call(name, args)?,
                    Err(e) => step.refuse(name, &e),
                };
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": c["id"],
                    "content": text,
                }));
            }
        }
        Err("the conversation ended without an answer".into())
    }
}

/// Arguments arrive as JSON text (some servers send an object).
fn parse_arguments(raw: &str, value: &Value) -> Result<Value, String> {
    if value.is_object() {
        return Ok(value.clone());
    }
    if raw.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str::<Value>(raw)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| "the arguments are not a valid JSON object".to_owned())
}

/// State shared by every round of one conversation.
struct Step<'n, 'r> {
    names: &'n [&'n str],
    run: &'n mut Runner<'r>,
    cancel: &'n AtomicBool,
    calls: Vec<ToolCall>,
    started: Instant,
    limits: Limits,
}

impl Step<'_, '_> {
    fn check(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.into());
        }
        if self.started.elapsed() > self.limits.deadline {
            return Err("the AI took too long; ask a narrower question".into());
        }
        Ok(())
    }

    /// The last round: no more tools, answer with what there is.
    fn last_round(&self, round: usize, messages: &[Value]) -> bool {
        let size: usize = messages.iter().map(|m| m.to_string().len()).sum();
        round + 1 >= self.limits.rounds
            || size > self.limits.transcript
            || self.started.elapsed() > self.limits.deadline / 2
    }

    /// Runs one call if it is on offer; the text and whether it failed.
    fn call(&mut self, name: &str, arguments: Value) -> Result<(String, bool), String> {
        self.check()?;
        if !self.names.contains(&name) {
            return Ok(self.refuse(name, "no such tool here; use one of the tools offered"));
        }
        let (result, failed) = match (self.run)(name, &arguments) {
            Ok(text) => (text, false),
            Err(text) => (text, true),
        };
        self.calls.push(ToolCall {
            name: name.to_owned(),
            arguments,
            result: result.clone(),
            failed,
        });
        Ok((result, failed))
    }

    fn refuse(&mut self, name: &str, why: &str) -> (String, bool) {
        self.calls.push(ToolCall {
            name: name.to_owned(),
            arguments: Value::Null,
            result: why.to_owned(),
            failed: true,
        });
        (why.to_owned(), true)
    }

    fn finish(&mut self, answer: Answer, tools_unsupported: bool) -> Conversation {
        Conversation {
            answer,
            calls: std::mem::take(&mut self.calls),
            tools_unsupported,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AiConfig, ProviderKind};
    use std::cell::RefCell;

    /// Replies from a script, one per request, recording each body.
    struct Script {
        replies: RefCell<Vec<(u16, Value)>>,
        bodies: RefCell<Vec<Value>>,
    }

    impl Script {
        fn new(replies: Vec<(u16, Value)>) -> Self {
            Self {
                replies: RefCell::new(replies),
                bodies: RefCell::default(),
            }
        }
    }

    impl Transport for &Script {
        fn send(
            &self,
            _method: &str,
            _url: &str,
            _headers: &[(&str, &str)],
            body: Option<&[u8]>,
        ) -> Result<(u16, Vec<u8>), String> {
            if let Some(b) = body {
                self.bodies
                    .borrow_mut()
                    .push(serde_json::from_slice(b).unwrap());
            }
            let (status, reply) = self.replies.borrow_mut().remove(0);
            Ok((status, reply.to_string().into_bytes()))
        }
    }

    fn client(provider: ProviderKind, script: &Script) -> Client<&Script> {
        let cfg = AiConfig {
            enabled: true,
            provider,
            model: if provider == ProviderKind::Anthropic {
                String::new()
            } else {
                "m".into()
            },
            ..AiConfig::default()
        };
        Client::new(cfg, Some("k".into()), script).unwrap()
    }

    fn tools() -> Vec<Value> {
        vec![json!({
            "name": "process_report",
            "description": "one process",
            "inputSchema": { "type": "object", "properties": { "pid": { "type": "integer" } } },
        })]
    }

    fn runner(
        log: &RefCell<Vec<String>>,
    ) -> impl FnMut(&str, &Value) -> Result<String, String> + '_ {
        move |name, args| {
            log.borrow_mut().push(format!("{name} {args}"));
            if args["pid"] == 0 {
                Err("no process with PID 0".into())
            } else {
                Ok(format!(
                    "<rustwtm-data>report of {}</rustwtm-data>",
                    args["pid"]
                ))
            }
        }
    }

    #[test]
    fn anthropic_turns_are_echoed_and_results_sent_together() {
        let thinking = json!({ "type": "thinking", "thinking": "", "signature": "sig-1" });
        let script = Script::new(vec![
            (
                200,
                json!({
                    "stop_reason": "tool_use",
                    "content": [
                        thinking,
                        { "type": "tool_use", "id": "tu1", "name": "process_report", "input": { "pid": 7 } },
                        { "type": "tool_use", "id": "tu2", "name": "process_report", "input": { "pid": 0 } },
                        { "type": "tool_use", "id": "tu3", "name": "end_process", "input": { "pid": 7 } },
                    ],
                    "usage": { "input_tokens": 100, "output_tokens": 10 },
                }),
            ),
            (
                200,
                json!({
                    "model": "claude-opus-5-5",
                    "stop_reason": "end_turn",
                    "content": [{ "type": "text", "text": "Safe." }],
                    "usage": { "input_tokens": 150, "output_tokens": 5 },
                }),
            ),
        ]);
        let log = RefCell::default();
        let mut run = runner(&log);
        let c = client(ProviderKind::Anthropic, &script);
        let done = c
            .converse(
                "sys",
                "q",
                &tools(),
                &mut run,
                &AtomicBool::new(false),
                Limits::default(),
            )
            .unwrap();
        assert_eq!(done.answer.text, "Safe.");
        assert_eq!(
            (done.answer.input_tokens, done.answer.output_tokens),
            (Some(250), Some(15))
        );
        // The control tool was refused without running.
        assert_eq!(log.borrow().len(), 2, "{:?}", log.borrow());
        assert_eq!(done.calls.len(), 3);
        assert!(done.calls[1].failed && done.calls[2].failed);

        let bodies = script.bodies.borrow();
        assert_eq!(bodies[0]["tool_choice"]["type"], "auto");
        assert_eq!(bodies[0]["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(bodies[0]["cache_control"]["type"], "ephemeral");
        let history = bodies[1]["messages"].as_array().unwrap();
        assert_eq!(history.len(), 3);
        // The assistant turn comes back byte for byte, thinking included.
        assert_eq!(history[1]["content"][0], thinking);
        let results = history[2]["content"].as_array().unwrap();
        assert_eq!(results.len(), 3, "every result in one message");
        assert_eq!(results[0]["tool_use_id"], "tu1");
        assert!(results[0].get("is_error").is_none());
        assert_eq!(results[1]["is_error"], true);
        assert_eq!(results[2]["is_error"], true);
        assert!(
            results[2]["content"]
                .as_str()
                .unwrap()
                .contains("no such tool")
        );
    }

    #[test]
    fn the_last_round_forbids_tools_and_gets_an_answer() {
        let wants_tool = json!({
            "stop_reason": "tool_use",
            "content": [{ "type": "tool_use", "id": "a", "name": "process_report", "input": { "pid": 1 } }],
        });
        let script = Script::new(vec![
            (200, wants_tool.clone()),
            (
                200,
                json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": "Done." }] }),
            ),
        ]);
        let log = RefCell::default();
        let mut run = runner(&log);
        let limits = Limits {
            rounds: 2,
            ..Limits::default()
        };
        let done = client(ProviderKind::Anthropic, &script)
            .converse(
                "s",
                "q",
                &tools(),
                &mut run,
                &AtomicBool::new(false),
                limits,
            )
            .unwrap();
        assert_eq!(done.answer.text, "Done.");
        assert_eq!(script.bodies.borrow()[1]["tool_choice"]["type"], "none");

        // Too much history: the next round is the last at once.
        let script = Script::new(vec![(
            200,
            json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": "Short." }] }),
        )]);
        let limits = Limits {
            transcript: 10,
            ..Limits::default()
        };
        let mut run = runner(&log);
        client(ProviderKind::Anthropic, &script)
            .converse(
                "s",
                "a long question",
                &tools(),
                &mut run,
                &AtomicBool::new(false),
                limits,
            )
            .unwrap();
        assert_eq!(script.bodies.borrow()[0]["tool_choice"]["type"], "none");
    }

    #[test]
    fn refusals_are_not_acted_on_and_cancelling_stops() {
        let script = Script::new(vec![(
            200,
            json!({ "stop_reason": "refusal", "content": [] }),
        )]);
        let log = RefCell::default();
        let mut run = runner(&log);
        let e = client(ProviderKind::Anthropic, &script)
            .converse(
                "s",
                "q",
                &tools(),
                &mut run,
                &AtomicBool::new(false),
                Limits::default(),
            )
            .unwrap_err();
        assert!(e.contains("declined"), "{e}");
        let script = Script::new(vec![]);
        let e = client(ProviderKind::Anthropic, &script)
            .converse(
                "s",
                "q",
                &tools(),
                &mut run,
                &AtomicBool::new(true),
                Limits::default(),
            )
            .unwrap_err();
        assert_eq!(e, CANCELLED);
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn openai_tool_calls_bad_arguments_and_servers_without_tools() {
        let call = |id: &str, args: &str| json!({ "id": id, "type": "function", "function": { "name": "process_report", "arguments": args } });
        let script = Script::new(vec![
            (
                200,
                json!({ "choices": [{ "finish_reason": "tool_calls", "message": {
                    "role": "assistant", "content": null, "extra": "kept",
                    "tool_calls": [call("c1", "{\"pid\":5}"), call("c2", "{not json")],
                } }] }),
            ),
            (
                200,
                json!({ "choices": [{ "finish_reason": "stop", "message": { "content": "Fine." } }] }),
            ),
        ]);
        let log = RefCell::default();
        let mut run = runner(&log);
        let done = client(ProviderKind::OpenAi, &script)
            .converse(
                "s",
                "q",
                &tools(),
                &mut run,
                &AtomicBool::new(false),
                Limits::default(),
            )
            .unwrap();
        assert_eq!(done.answer.text, "Fine.");
        assert_eq!(log.borrow().len(), 1, "bad arguments never run");
        let bodies = script.bodies.borrow();
        assert_eq!(bodies[0]["tools"][0]["function"]["name"], "process_report");
        let history = bodies[1]["messages"].as_array().unwrap();
        assert_eq!(
            history[2]["extra"], "kept",
            "the message goes back as it came"
        );
        assert!(history[2]["content"].is_null());
        assert_eq!(history[3]["tool_call_id"], "c1");
        assert_eq!(history[4]["tool_call_id"], "c2");
        assert!(
            history[4]["content"]
                .as_str()
                .unwrap()
                .contains("not a valid JSON")
        );

        // A server without tools: asked again without them, once.
        let script = Script::new(vec![
            (
                400,
                json!({ "error": { "message": "model does not support tools" } }),
            ),
            (
                200,
                json!({ "choices": [{ "finish_reason": "stop", "message": { "content": "Plain." } }] }),
            ),
        ]);
        let done = client(ProviderKind::Ollama, &script)
            .converse(
                "s",
                "q",
                &tools(),
                &mut run,
                &AtomicBool::new(false),
                Limits::default(),
            )
            .unwrap();
        assert!(done.tools_unsupported);
        assert!(script.bodies.borrow()[1].get("tools").is_none());
        // A key error is not retried.
        let script = Script::new(vec![(401, json!({ "error": { "message": "bad key" } }))]);
        let e = client(ProviderKind::OpenAi, &script)
            .converse(
                "s",
                "q",
                &tools(),
                &mut run,
                &AtomicBool::new(false),
                Limits::default(),
            )
            .unwrap_err();
        assert!(e.contains("401"), "{e}");
        assert_eq!(script.bodies.borrow().len(), 1);
    }
}

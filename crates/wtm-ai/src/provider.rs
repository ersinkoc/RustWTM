//! One question, one answer, from the configured provider: Anthropic's
//! Messages API or OpenAI-style Chat Completions. Nothing here runs tools
//! or acts on the machine; the model only reads what it is given.

use serde_json::{Value, json};

use crate::config::{AiConfig, ProviderKind, Wire, is_loopback_url};

/// The Messages API version this client is written for.
pub const ANTHROPIC_VERSION: &str = "2026-10-21";
/// Server-side refusal fallbacks (`fallbacks: "default"`).
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// How long one answer may take: thinking models can be slow.
pub const TIMEOUT_MS: i32 = 180_000;
const USER_AGENT: &str = concat!("RustWTM/", env!("CARGO_PKG_VERSION"));

/// Sends one HTTP request; a seam so tests never reach a real provider.
pub trait Transport {
    /// `(status, body)` of `method url` with `headers` and a JSON `body`.
    fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<(u16, Vec<u8>), String>;
}

/// WinHTTP, with the system proxy; redirects are never followed, so the
/// key never goes to a host it was not meant for.
pub struct WinHttp;

impl Transport for WinHttp {
    fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<(u16, Vec<u8>), String> {
        wtm_sys::http::call(
            method,
            url,
            headers,
            body.map(|b| ("application/json", b)),
            USER_AGENT,
            TIMEOUT_MS,
        )
        .map_err(|e| format!("cannot reach the provider: {e}"))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub text: String,
    /// The model that answered (a fallback model may have stepped in).
    pub model: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Cut off at `max_tokens`.
    pub truncated: bool,
}

pub struct Client<T: Transport = WinHttp> {
    cfg: AiConfig,
    key: Option<String>,
    transport: T,
}

impl Client<WinHttp> {
    /// A client for the saved settings, with their key.
    pub fn from_config(cfg: AiConfig) -> Result<Self, String> {
        let key = cfg.key()?.map(|(k, _)| k);
        Self::new(cfg, key, WinHttp)
    }
}

impl<T: Transport> Client<T> {
    pub fn new(cfg: AiConfig, key: Option<String>, transport: T) -> Result<Self, String> {
        // No model yet is fine: listing the models is how one is chosen.
        if let Some(problem) = cfg.connection_problem() {
            return Err(problem);
        }
        if key.is_none() && !cfg.provider.key_optional() {
            let hint = cfg
                .provider
                .key_env()
                .map(|v| format!(" (or set {v})"))
                .unwrap_or_default();
            return Err(format!(
                "no API key for {}: add one in Settings → AI or with `wtm ai key`{hint}",
                cfg.provider.label()
            ));
        }
        // A key over plain HTTP would cross the network readable.
        if key.is_some()
            && cfg.base_url().starts_with("http://")
            && !is_loopback_url(cfg.base_url())
        {
            return Err("an API key is only sent over https:// (or to this PC)".into());
        }
        Ok(Self {
            cfg,
            key,
            transport,
        })
    }

    pub fn config(&self) -> &AiConfig {
        &self.cfg
    }

    fn headers(&self) -> Vec<(&'static str, String)> {
        let mut h = Vec::new();
        match self.cfg.provider.wire() {
            Wire::Anthropic => {
                if let Some(k) = &self.key {
                    h.push(("x-api-key", k.clone()));
                }
                h.push(("anthropic-version", ANTHROPIC_VERSION.to_owned()));
            }
            Wire::OpenAi => {
                if let Some(k) = &self.key {
                    h.push(("Authorization", format!("Bearer {k}")));
                }
                if self.cfg.provider == ProviderKind::OpenRouter {
                    // OpenRouter's attribution headers, optional.
                    h.push(("X-Title", "RustWTM".to_owned()));
                }
            }
        }
        h
    }

    fn send(
        &self,
        method: &str,
        url: &str,
        body: Option<&Value>,
        extra: &[(&str, &str)],
    ) -> Result<Value, String> {
        self.send_status(method, url, body, extra)
            .map_err(|(_, e)| e)
    }

    /// [`Self::send`], with the HTTP status of a refused request, so the
    /// tool loop can tell "this server has no tools" (400) from the rest.
    pub(crate) fn send_status(
        &self,
        method: &str,
        url: &str,
        body: Option<&Value>,
        extra: &[(&str, &str)],
    ) -> Result<Value, (Option<u16>, String)> {
        let owned = self.headers();
        let mut headers: Vec<(&str, &str)> = owned.iter().map(|(n, v)| (*n, v.as_str())).collect();
        headers.extend_from_slice(extra);
        let bytes = body.map(|b| b.to_string().into_bytes());
        let (status, reply) = self
            .transport
            .send(method, url, &headers, bytes.as_deref())
            .map_err(|e| (None, e))?;
        if (300..400).contains(&status) {
            return Err((
                Some(status),
                format!(
                    "the provider answered with a redirect (HTTP {status}); check the API address"
                ),
            ));
        }
        if !(200..300).contains(&status) {
            return Err((Some(status), error_text(status, &reply)));
        }
        serde_json::from_slice(&reply).map_err(|_| {
            (
                Some(status),
                format!(
                    "the provider's answer is not JSON: {}",
                    snippet(&String::from_utf8_lossy(&reply))
                ),
            )
        })
    }

    /// The Messages beta headers `model` needs.
    pub(crate) fn anthropic_betas(&self) -> Vec<(&'static str, &'static str)> {
        let (_, betas) = anthropic_request(&self.cfg, "", "");
        betas.into_iter().map(|b| ("anthropic-beta", b)).collect()
    }

    /// Asks once: `system` sets the role, `user` carries the question and data.
    pub fn complete(&self, system: &str, user: &str) -> Result<Answer, String> {
        if let Some(problem) = self.cfg.problem() {
            return Err(problem);
        }
        match self.cfg.provider.wire() {
            Wire::Anthropic => {
                let (body, beta) = anthropic_request(&self.cfg, system, user);
                let extra: Vec<(&str, &str)> =
                    beta.iter().map(|b| ("anthropic-beta", *b)).collect();
                let url = format!("{}/messages", self.cfg.base_url());
                parse_anthropic(&self.send("POST", &url, Some(&body), &extra)?)
            }
            Wire::OpenAi => {
                let body = openai_request(&self.cfg, system, user);
                let url = format!("{}/chat/completions", self.cfg.base_url());
                parse_openai(&self.send("POST", &url, Some(&body), &[])?)
            }
        }
    }

    /// The model IDs the provider offers to this key.
    pub fn models(&self) -> Result<Vec<String>, String> {
        let url = format!("{}/models", self.cfg.base_url());
        let reply = self.send("GET", &url, None, &[])?;
        let mut ids: Vec<String> = reply["data"]
            .as_array()
            .or_else(|| reply["models"].as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|m| m["id"].as_str().or_else(|| m["name"].as_str()))
                    // Gemini names them `models/…`; requests take the bare ID.
                    .map(|id| id.strip_prefix("models/").unwrap_or(id).to_owned())
                    .collect()
            })
            .unwrap_or_default();
        ids.sort();
        ids.dedup();
        Ok(ids)
    }
}

/// Whether a Claude model takes `output_config.effort`.
fn takes_effort(model: &str) -> bool {
    [
        "claude-fable-",
        "claude-mythos-",
        "claude-opus-5",
        "claude-opus-4-8",
        "claude-opus-4-7",
        "claude-opus-4-6",
        "claude-opus-4-5",
        "claude-sonnet-5",
        "claude-sonnet-4-6",
        "claude-haiku-5",
    ]
    .iter()
    .any(|p| model.starts_with(p))
}

/// Whether a Claude model takes server-side refusal fallbacks.
fn takes_fallbacks(model: &str) -> bool {
    [
        "claude-fable-5-1",
        "claude-fable-5-2",
        "claude-mythos-5-1",
        "claude-mythos-5-2",
        "claude-opus-5",
        "claude-sonnet-5-5",
    ]
    .iter()
    .any(|p| model.starts_with(p))
}

/// The Messages request body and the beta headers it needs. No sampling
/// parameters (rejected since 2026-10-21) and no `thinking` setting: the
/// current models think adaptively by default, and `effort` sets how much.
pub fn anthropic_request(cfg: &AiConfig, system: &str, user: &str) -> (Value, Vec<&'static str>) {
    let model = cfg.model();
    let mut body = json!({
        "model": model,
        "max_tokens": cfg.max_tokens.clamp(256, 64_000),
        "system": system,
        "messages": [{ "role": "user", "content": user }],
    });
    let effort = cfg.effort.trim();
    if !effort.is_empty() && takes_effort(model) {
        body["output_config"] = json!({ "effort": effort });
    }
    let mut betas = Vec::new();
    if takes_fallbacks(model) {
        body["fallbacks"] = json!("default");
        betas.push(FALLBACK_BETA);
    }
    (body, betas)
}

pub fn parse_anthropic(reply: &Value) -> Result<Answer, String> {
    let stop = reply["stop_reason"].as_str().unwrap_or("");
    if stop == "refusal" {
        let category = reply["stop_details"]["category"]
            .as_str()
            .map(|c| format!(" ({c})"))
            .unwrap_or_default();
        return Err(format!("the model declined to answer{category}"));
    }
    // Thinking blocks come back empty; only text is the answer.
    let text: Vec<&str> = reply["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    let text = text.join("\n").trim().to_owned();
    if text.is_empty() {
        return Err(match stop {
            "max_tokens" => {
                "the answer ran out of room before it began; raise the token limit".into()
            }
            _ => "the model returned no text".into(),
        });
    }
    Ok(Answer {
        text,
        model: reply["model"].as_str().unwrap_or_default().to_owned(),
        input_tokens: reply["usage"]["input_tokens"].as_u64(),
        output_tokens: reply["usage"]["output_tokens"].as_u64(),
        truncated: stop == "max_tokens",
    })
}

pub fn openai_request(cfg: &AiConfig, system: &str, user: &str) -> Value {
    let mut body = json!({
        "model": cfg.model(),
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    });
    body[cfg.provider.max_tokens_field()] = json!(cfg.max_tokens.clamp(256, 64_000));
    body
}

pub fn parse_openai(reply: &Value) -> Result<Answer, String> {
    let choice = &reply["choices"][0];
    let finish = choice["finish_reason"].as_str().unwrap_or("");
    if finish == "content_filter" {
        return Err("the provider's content filter withheld the answer".into());
    }
    let message = &choice["message"];
    let text = match &message["content"] {
        Value::String(s) => s.clone(),
        // Some servers send content as parts.
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    };
    let text = strip_think(&text).trim().to_owned();
    if text.is_empty() {
        if let Some(refusal) = message["refusal"].as_str() {
            return Err(format!(
                "the model declined to answer: {}",
                snippet(refusal)
            ));
        }
        return Err(match finish {
            "length" => "the answer ran out of room before it began; raise the token limit".into(),
            _ => "the model returned no text".into(),
        });
    }
    Ok(Answer {
        text,
        model: reply["model"].as_str().unwrap_or_default().to_owned(),
        input_tokens: reply["usage"]["prompt_tokens"].as_u64(),
        output_tokens: reply["usage"]["completion_tokens"].as_u64(),
        truncated: finish == "length",
    })
}

/// Local reasoning models (DeepSeek-R1, Qwen…) put their thinking in
/// `<think>…</think>` ahead of the answer.
fn strip_think(text: &str) -> &str {
    let trimmed = text.trim_start();
    match trimmed.strip_prefix("<think>") {
        Some(rest) => rest.split_once("</think>").map_or(text, |(_, after)| after),
        None => text,
    }
}

/// A provider error as one readable line, never the whole body.
fn error_text(status: u16, body: &[u8]) -> String {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let message = parsed.as_ref().and_then(|v| {
        // `{"error":{"message":…}}`, a list of those, or `{"error":"…"}`.
        let e = match v {
            Value::Array(list) => &list.first()?["error"],
            _ => &v["error"],
        };
        e["message"]
            .as_str()
            .or_else(|| e.as_str())
            .or_else(|| v["message"].as_str())
            .map(str::to_owned)
    });
    let what = match status {
        400 => "the request was refused",
        401 => "the API key was not accepted",
        403 => "this key may not use that model or API",
        404 => "not found (check the model name and the API address)",
        408 | 504 => "the provider timed out",
        413 => "the request is too large",
        429 => "rate limit or quota reached; try again later",
        500..=599 => "the provider had an error",
        _ => "the request failed",
    };
    let detail = message
        .or_else(|| (!body.is_empty()).then(|| String::from_utf8_lossy(body).into_owned()))
        .map(|m| format!(": {}", snippet(&m)))
        .unwrap_or_default();
    format!("{what} (HTTP {status}){detail}")
}

fn snippet(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match one_line.char_indices().nth(300) {
        Some((cut, _)) => format!("{}…", &one_line[..cut]),
        None => one_line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Method, URL, headers and JSON body of one request.
    type Seen = (String, String, Vec<(String, String)>, Option<Value>);

    /// Answers every request with one canned reply and records the request.
    struct Fake {
        reply: (u16, String),
        seen: RefCell<Vec<Seen>>,
    }

    impl Fake {
        fn new(status: u16, body: Value) -> Self {
            Self {
                reply: (status, body.to_string()),
                seen: RefCell::default(),
            }
        }
    }

    impl Transport for &Fake {
        fn send(
            &self,
            method: &str,
            url: &str,
            headers: &[(&str, &str)],
            body: Option<&[u8]>,
        ) -> Result<(u16, Vec<u8>), String> {
            self.seen.borrow_mut().push((
                method.into(),
                url.into(),
                headers
                    .iter()
                    .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                    .collect(),
                body.map(|b| serde_json::from_slice(b).unwrap()),
            ));
            Ok((self.reply.0, self.reply.1.clone().into_bytes()))
        }
    }

    fn cfg(provider: ProviderKind, model: &str) -> AiConfig {
        AiConfig {
            enabled: true,
            provider,
            model: model.into(),
            ..AiConfig::default()
        }
    }

    #[test]
    fn anthropic_requests_carry_version_effort_and_fallbacks() {
        let fake = Fake::new(
            200,
            json!({
                "model": "claude-opus-5-5",
                "stop_reason": "end_turn",
                "content": [
                    { "type": "thinking", "thinking": "", "signature": "x" },
                    { "type": "text", "text": "It is the Windows shell." },
                ],
                "usage": { "input_tokens": 120, "output_tokens": 9 },
            }),
        );
        let client = Client::new(
            cfg(ProviderKind::Anthropic, ""),
            Some("sk-ant-key".into()),
            &fake,
        )
        .unwrap();
        let answer = client
            .complete("system text", "what is explorer.exe?")
            .unwrap();
        assert_eq!(answer.text, "It is the Windows shell.");
        assert_eq!(
            (answer.input_tokens, answer.output_tokens),
            (Some(120), Some(9))
        );
        assert!(!answer.truncated);

        let seen = fake.seen.borrow();
        let (method, url, headers, body) = &seen[0];
        assert_eq!(
            (method.as_str(), url.as_str()),
            ("POST", "https://api.anthropic.com/v1/messages")
        );
        let header = |n: &str| {
            headers
                .iter()
                .find(|(k, _)| k == n)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(header("x-api-key"), Some("sk-ant-key"));
        assert_eq!(header("anthropic-version"), Some(ANTHROPIC_VERSION));
        assert_eq!(header("anthropic-beta"), Some(FALLBACK_BETA));
        let body = body.as_ref().unwrap();
        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(body["system"], "system text");
        assert_eq!(body["messages"][0]["content"], "what is explorer.exe?");
        assert_eq!(body["output_config"]["effort"], "medium");
        assert_eq!(body["fallbacks"], "default");
        for removed in ["temperature", "top_p", "top_k", "thinking"] {
            assert!(body.get(removed).is_none(), "{removed} must not be sent");
        }
    }

    #[test]
    fn older_claude_models_get_neither_effort_nor_fallbacks() {
        let (body, betas) =
            anthropic_request(&cfg(ProviderKind::Anthropic, "claude-haiku-4-5"), "s", "u");
        assert!(body.get("output_config").is_none());
        assert!(body.get("fallbacks").is_none());
        assert!(betas.is_empty());
        let (body, _) =
            anthropic_request(&cfg(ProviderKind::Anthropic, "claude-sonnet-5"), "s", "u");
        assert_eq!(body["output_config"]["effort"], "medium");
        assert!(body.get("fallbacks").is_none());
    }

    #[test]
    fn refusals_truncation_and_empty_answers_are_told_apart() {
        let refusal = json!({ "stop_reason": "refusal", "stop_details": { "category": "cyber" }, "content": [] });
        assert_eq!(
            parse_anthropic(&refusal).unwrap_err(),
            "the model declined to answer (cyber)"
        );
        let cut =
            json!({ "stop_reason": "max_tokens", "content": [{ "type": "text", "text": "Part" }] });
        assert!(parse_anthropic(&cut).unwrap().truncated);
        let nothing = json!({ "stop_reason": "max_tokens", "content": [{ "type": "thinking", "thinking": "" }] });
        assert!(
            parse_anthropic(&nothing)
                .unwrap_err()
                .contains("token limit")
        );
    }

    #[test]
    fn openai_compatible_requests_and_answers() {
        let fake = Fake::new(
            200,
            json!({
                "model": "gemini-x",
                "choices": [{ "finish_reason": "stop", "message": { "role": "assistant", "content": "<think>hmm</think>\nSafe to end." } }],
                "usage": { "prompt_tokens": 50, "completion_tokens": 4 },
            }),
        );
        let client = Client::new(
            cfg(ProviderKind::Gemini, "gemini-x"),
            Some("AIzaKEY".into()),
            &fake,
        )
        .unwrap();
        let answer = client.complete("sys", "q").unwrap();
        assert_eq!(answer.text, "Safe to end.");
        let seen = fake.seen.borrow();
        let (_, url, headers, body) = &seen[0];
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
        );
        assert!(headers.contains(&("Authorization".into(), "Bearer AIzaKEY".into())));
        let body = body.as_ref().unwrap();
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "q");
        assert!(body["max_tokens"].is_u64());

        // OpenAI itself takes the newer field.
        let body = openai_request(&cfg(ProviderKind::OpenAi, "m"), "s", "u");
        assert!(body["max_completion_tokens"].is_u64() && body.get("max_tokens").is_none());

        let filtered = json!({ "choices": [{ "finish_reason": "content_filter", "message": { "content": null } }] });
        assert!(
            parse_openai(&filtered)
                .unwrap_err()
                .contains("content filter")
        );
        let refused = json!({ "choices": [{ "finish_reason": "stop", "message": { "content": null, "refusal": "No." } }] });
        assert!(parse_openai(&refused).unwrap_err().contains("declined"));
    }

    #[test]
    fn errors_name_the_cause_without_dumping_the_body() {
        let fake = Fake::new(
            401,
            json!({ "type": "error", "error": { "type": "authentication_error", "message": "invalid x-api-key" } }),
        );
        let client =
            Client::new(cfg(ProviderKind::Anthropic, ""), Some("bad".into()), &fake).unwrap();
        let e = client.complete("s", "u").unwrap_err();
        assert_eq!(
            e,
            "the API key was not accepted (HTTP 401): invalid x-api-key"
        );
        // Gemini's compatibility layer may wrap errors in a list.
        let listed = br#"[{"error":{"code":429,"message":"Resource exhausted"}}]"#;
        assert!(error_text(429, listed).ends_with("Resource exhausted"));
        let html = format!("<html>{}</html>", "x".repeat(2000));
        let e = error_text(502, html.as_bytes());
        assert!(e.chars().count() < 400, "{e}");
        let fake = Fake::new(307, json!({}));
        let client =
            Client::new(cfg(ProviderKind::Anthropic, ""), Some("k".into()), &fake).unwrap();
        assert!(client.complete("s", "u").unwrap_err().contains("redirect"));
    }

    #[test]
    fn keys_are_required_and_never_sent_in_the_clear() {
        let fake = Fake::new(200, json!({}));
        let e = Client::new(cfg(ProviderKind::OpenAi, "m"), None, &fake)
            .err()
            .unwrap();
        assert!(e.contains("OPENAI_API_KEY"), "{e}");
        // Local servers need none.
        assert!(Client::new(cfg(ProviderKind::Ollama, "llama"), None, &fake).is_ok());
        let mut plain = cfg(ProviderKind::Custom, "m");
        plain.base_url = "http://ai.example.com/v1".into();
        let e = Client::new(plain.clone(), Some("k".into()), &fake)
            .err()
            .unwrap();
        assert!(e.contains("https"), "{e}");
        plain.base_url = "http://127.0.0.1:8000/v1".into();
        assert!(Client::new(plain.clone(), Some("k".into()), &fake).is_ok());
        // The host after `@` is the real one: refused like any other.
        plain.base_url = "http://127.0.0.1@ai.example.com/v1".into();
        assert!(Client::new(plain, Some("k".into()), &fake).is_err());
    }

    #[test]
    fn models_are_listed_before_one_is_chosen_but_not_asked() {
        let fake = Fake::new(200, json!({ "data": [{ "id": "llama-3" }] }));
        let client = Client::new(cfg(ProviderKind::Groq, ""), Some("k".into()), &fake).unwrap();
        assert_eq!(client.models().unwrap(), ["llama-3"]);
        let e = client.complete("s", "u").unwrap_err();
        assert!(e.contains("choose a model"), "{e}");
        assert_eq!(fake.seen.borrow().len(), 1, "nothing sent without a model");
        // Plain Fable 5 takes no fallbacks; its successors do.
        let (body, _) =
            anthropic_request(&cfg(ProviderKind::Anthropic, "claude-fable-5"), "s", "u");
        assert!(body.get("fallbacks").is_none());
        let (body, _) =
            anthropic_request(&cfg(ProviderKind::Anthropic, "claude-fable-5-2"), "s", "u");
        assert_eq!(body["fallbacks"], "default");
    }

    #[test]
    fn lists_models_from_either_shape() {
        let fake = Fake::new(
            200,
            json!({ "data": [{ "id": "b" }, { "id": "models/a" }, { "id": "b" }] }),
        );
        let client = Client::new(cfg(ProviderKind::Gemini, "x"), Some("k".into()), &fake).unwrap();
        assert_eq!(client.models().unwrap(), ["a", "b"]);
        let seen = fake.seen.borrow();
        assert_eq!(seen[0].0, "GET");
        assert!(seen[0].1.ends_with("/openai/models"));
        assert!(seen[0].3.is_none());
    }
}

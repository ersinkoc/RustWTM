//! Which AI provider to ask, kept per Windows user in
//! `%APPDATA%\RustWTM\ai.toml`. API keys are sealed with DPAPI for that
//! user, one per provider, so the file alone opens nothing.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How a provider is spoken to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// Anthropic's Messages API.
    Anthropic,
    /// OpenAI Chat Completions, which most other providers also speak.
    OpenAi,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
    Gemini,
    #[serde(rename = "openrouter")]
    OpenRouter,
    Groq,
    #[serde(rename = "deepseek")]
    DeepSeek,
    Mistral,
    Xai,
    /// Local, no key.
    Ollama,
    /// Local, no key.
    #[serde(rename = "lmstudio")]
    LmStudio,
    /// Any other OpenAI-compatible endpoint (`base_url` required).
    Custom,
}

impl ProviderKind {
    pub const ALL: [Self; 11] = [
        Self::Anthropic,
        Self::OpenAi,
        Self::Gemini,
        Self::OpenRouter,
        Self::Groq,
        Self::DeepSeek,
        Self::Mistral,
        Self::Xai,
        Self::Ollama,
        Self::LmStudio,
        Self::Custom,
    ];

    /// The name used in `ai.toml` and on the command line.
    pub fn id(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
            Self::OpenRouter => "openrouter",
            Self::Groq => "groq",
            Self::DeepSeek => "deepseek",
            Self::Mistral => "mistral",
            Self::Xai => "xai",
            Self::Ollama => "ollama",
            Self::LmStudio => "lmstudio",
            Self::Custom => "custom",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|p| p.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic (Claude)",
            Self::OpenAi => "OpenAI",
            Self::Gemini => "Google Gemini",
            Self::OpenRouter => "OpenRouter",
            Self::Groq => "Groq",
            Self::DeepSeek => "DeepSeek",
            Self::Mistral => "Mistral",
            Self::Xai => "xAI (Grok)",
            Self::Ollama => "Ollama (local)",
            Self::LmStudio => "LM Studio (local)",
            Self::Custom => "OpenAI-compatible (custom URL)",
        }
    }

    pub fn wire(self) -> Wire {
        match self {
            Self::Anthropic => Wire::Anthropic,
            _ => Wire::OpenAi,
        }
    }

    /// Where the API lives unless `base_url` says otherwise.
    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com/v1",
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Gemini => "https://generativelanguage.googleapis.com/v1beta/openai",
            Self::OpenRouter => "https://openrouter.ai/api/v1",
            Self::Groq => "https://api.groq.com/openai/v1",
            Self::DeepSeek => "https://api.deepseek.com",
            Self::Mistral => "https://api.mistral.ai/v1",
            Self::Xai => "https://api.x.ai/v1",
            Self::Ollama => "http://localhost:11434/v1",
            Self::LmStudio => "http://localhost:1234/v1",
            Self::Custom => "",
        }
    }

    /// A model to start with. Only Anthropic's is filled in: other
    /// providers rename theirs often, so the user picks one from
    /// [`crate::Client::models`].
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Anthropic => "claude-opus-5-5",
            _ => "",
        }
    }

    /// The environment variable the provider's own tools read the key from.
    pub fn key_env(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("ANTHROPIC_API_KEY"),
            Self::OpenAi => Some("OPENAI_API_KEY"),
            Self::Gemini => Some("GEMINI_API_KEY"),
            Self::OpenRouter => Some("OPENROUTER_API_KEY"),
            Self::Groq => Some("GROQ_API_KEY"),
            Self::DeepSeek => Some("DEEPSEEK_API_KEY"),
            Self::Mistral => Some("MISTRAL_API_KEY"),
            Self::Xai => Some("XAI_API_KEY"),
            Self::Ollama | Self::LmStudio | Self::Custom => None,
        }
    }

    /// Whether requests go without a key (local servers).
    pub fn key_optional(self) -> bool {
        matches!(self, Self::Ollama | Self::LmStudio | Self::Custom)
    }

    /// The field that caps the answer's length. OpenAI deprecated
    /// `max_tokens` on Chat Completions; the others still read it.
    pub fn max_tokens_field(self) -> &'static str {
        match self {
            Self::OpenAi => "max_completion_tokens",
            _ => "max_tokens",
        }
    }

    /// Whether the data stays on this machine.
    pub fn is_local(self, base_url: &str) -> bool {
        matches!(self, Self::Ollama | Self::LmStudio) || is_loopback_url(base_url)
    }
}

/// `http(s)://localhost…`, `127.x`, `[::1]`.
///
/// Strict, so a key sent "to this PC" over plain HTTP cannot reach another
/// host: an address with user information (`http://127.0.0.1@evil.example`,
/// where the real host is `evil.example`) is never loopback, and the host
/// must be exactly `localhost` or an IP literal that is a loopback address.
pub fn is_loopback_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#', '\\']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let port_ok = |port: &str| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit());
    let host = match authority.strip_prefix('[') {
        Some(v6) => match v6.split_once(']') {
            Some((host, "")) => host,
            Some((host, after)) if after.strip_prefix(':').is_some_and(port_ok) => host,
            _ => return false,
        },
        None => match authority.split_once(':') {
            Some((host, port)) if port_ok(port) => host,
            Some(_) => return false,
            None => authority,
        },
    };
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    /// The user agreed that process facts are sent to the provider.
    pub enabled: bool,
    pub provider: ProviderKind,
    /// Empty: the provider's [`ProviderKind::default_model`].
    pub model: String,
    /// Empty: the provider's [`ProviderKind::default_base_url`].
    pub base_url: String,
    /// Longest answer, in tokens.
    pub max_tokens: u32,
    /// Anthropic's `output_config.effort` (`low` … `max`); empty: not sent.
    pub effort: String,
    /// Send command lines (secrets in them are masked). Off: never sent.
    pub send_command_lines: bool,
    /// Let the model look up more with read-only tools (any process,
    /// connections, services, startup programs, history).
    pub use_tools: bool,
    /// Language of the answers, e.g. `Türkçe`; empty: the UI's.
    pub language: String,
    /// DPAPI-sealed keys by provider id, as hex.
    pub keys: BTreeMap<String, String>,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: ProviderKind::Anthropic,
            model: String::new(),
            base_url: String::new(),
            max_tokens: 16_000,
            effort: "medium".into(),
            send_command_lines: true,
            use_tools: true,
            language: String::new(),
            keys: BTreeMap::new(),
        }
    }
}

/// What every sealed key is bound to, so DPAPI opens it for this purpose only.
const ENTROPY: &[u8] = b"RustWTM AI provider key v1";

/// Where an API key came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// `RUSTWTM_AI_KEY`.
    Override,
    /// Sealed in `ai.toml`.
    Stored,
    /// The provider's own variable, e.g. `ANTHROPIC_API_KEY`.
    Environment(&'static str),
}

impl AiConfig {
    /// `%APPDATA%\RustWTM\ai.toml`.
    pub fn path() -> Option<PathBuf> {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("RustWTM").join("ai.toml"))
    }

    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e: toml::de::Error| match e.span() {
            Some(span) => {
                let line = text[..span.start.min(text.len())].lines().count().max(1);
                format!("ai.toml: line {line} is not valid")
            }
            None => "ai.toml is not valid TOML".to_owned(),
        })
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }

    /// The saved settings; defaults (off) when there are none.
    pub fn load() -> Result<Self, String> {
        let Some(path) = Self::path() else {
            return Ok(Self::default());
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or("APPDATA is not set")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        // Written beside, then moved over: a crash never leaves half a file.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml())
            .and_then(|()| std::fs::rename(&tmp, &path))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    pub fn model(&self) -> &str {
        match self.model.trim() {
            "" => self.provider.default_model(),
            m => m,
        }
    }

    pub fn base_url(&self) -> &str {
        match self.base_url.trim() {
            "" => self.provider.default_base_url(),
            u => u,
        }
        .trim_end_matches('/')
    }

    /// Seals `key` for this user and keeps it for the current provider.
    pub fn set_key(&mut self, key: &str) -> Result<(), String> {
        let key = key.trim();
        if key.is_empty() {
            self.keys.remove(self.provider.id());
            return Ok(());
        }
        let sealed = wtm_sys::dpapi::protect(key.as_bytes(), ENTROPY)
            .map_err(|e| format!("cannot protect the key: {e}"))?;
        self.keys
            .insert(self.provider.id().to_owned(), hex(&sealed));
        Ok(())
    }

    pub fn forget_key(&mut self) {
        self.keys.remove(self.provider.id());
    }

    pub fn has_stored_key(&self) -> bool {
        self.keys.contains_key(self.provider.id())
    }

    /// The key to send, and where it came from: `RUSTWTM_AI_KEY` first,
    /// then the stored one, then the provider's own variable.
    pub fn key(&self) -> Result<Option<(String, KeySource)>, String> {
        if let Some(k) = env_nonempty("RUSTWTM_AI_KEY") {
            return Ok(Some((k, KeySource::Override)));
        }
        if let Some(sealed) = self.keys.get(self.provider.id()) {
            let bytes = unhex(sealed).ok_or("the stored key is damaged; set it again")?;
            let open = wtm_sys::dpapi::unprotect(&bytes, ENTROPY).map_err(|_| {
                "the stored key cannot be opened (it was saved by another Windows \
                 account or machine); set it again"
                    .to_owned()
            })?;
            let key = String::from_utf8(open).map_err(|_| "the stored key is damaged")?;
            return Ok(Some((key, KeySource::Stored)));
        }
        Ok(self
            .provider
            .key_env()
            .and_then(|var| env_nonempty(var).map(|k| (k, KeySource::Environment(var)))))
    }

    /// Why the settings cannot be used yet, if they cannot.
    pub fn problem(&self) -> Option<String> {
        if self.model().is_empty() {
            return Some(format!(
                "choose a model for {} (list them with `wtm ai models`)",
                self.provider.label()
            ));
        }
        self.connection_problem()
    }

    /// Why the provider cannot be reached at all; a missing model still
    /// lets its models be listed.
    pub fn connection_problem(&self) -> Option<String> {
        if self.base_url().is_empty() {
            return Some("enter the API address (base URL) of the server".into());
        }
        if !self.base_url().starts_with("https://") && !self.base_url().starts_with("http://") {
            return Some("the API address must start with https://".into());
        }
        None
    }
}

fn env_nonempty(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml_and_keeps_keys_sealed() {
        let mut cfg = AiConfig {
            enabled: true,
            provider: ProviderKind::OpenRouter,
            model: "some/model".into(),
            ..AiConfig::default()
        };
        cfg.set_key("  sk-or-secret-value  ").unwrap();
        let text = cfg.to_toml();
        assert!(!text.contains("sk-or-secret-value"), "{text}");
        assert!(text.contains("provider = \"openrouter\""), "{text}");
        let back = AiConfig::from_toml(&text).unwrap();
        assert_eq!(back, cfg);
        // Only this provider's key opens; the others have none.
        let sealed = back.keys.get("openrouter").unwrap();
        let open = wtm_sys::dpapi::unprotect(&unhex(sealed).unwrap(), ENTROPY).unwrap();
        assert_eq!(open, b"sk-or-secret-value");
        let other = AiConfig {
            provider: ProviderKind::Mistral,
            ..back.clone()
        };
        assert!(!other.has_stored_key());
        let mut cleared = back;
        cleared.forget_key();
        assert!(!cleared.has_stored_key());
    }

    #[test]
    fn a_damaged_key_is_named_not_sent() {
        let mut cfg = AiConfig {
            provider: ProviderKind::Xai,
            ..AiConfig::default()
        };
        cfg.keys.insert("xai".into(), "zz".into());
        // RUSTWTM_AI_KEY would take precedence; tests never set it.
        if std::env::var_os("RUSTWTM_AI_KEY").is_none() {
            assert!(cfg.key().unwrap_err().contains("damaged"));
            cfg.keys.insert("xai".into(), "00ff".into());
            assert!(cfg.key().unwrap_err().contains("cannot be opened"));
        }
    }

    #[test]
    fn defaults_and_problems() {
        let cfg = AiConfig::default();
        assert!(!cfg.enabled, "nothing is sent until the user agrees");
        assert_eq!(cfg.model(), "claude-opus-5-5");
        assert_eq!(cfg.base_url(), "https://api.anthropic.com/v1");
        assert_eq!(cfg.problem(), None);
        let groq = AiConfig {
            provider: ProviderKind::Groq,
            ..AiConfig::default()
        };
        assert!(groq.problem().unwrap().contains("choose a model"));
        let custom = AiConfig {
            provider: ProviderKind::Custom,
            model: "m".into(),
            ..AiConfig::default()
        };
        assert!(custom.problem().unwrap().contains("base URL"));
        let slash = AiConfig {
            base_url: "https://proxy.example/v1///".into(),
            ..AiConfig::default()
        };
        assert_eq!(slash.base_url(), "https://proxy.example/v1");
        // Unknown fields from a newer version are ignored, bad ones named.
        assert!(AiConfig::from_toml("future = 1\nprovider = \"groq\"").is_ok());
        let e = AiConfig::from_toml("provider = \"nope\"").unwrap_err();
        assert!(e.contains("line 1"), "{e}");
    }

    #[test]
    fn providers_round_trip_by_id_and_know_local_urls() {
        for p in ProviderKind::ALL {
            assert_eq!(ProviderKind::from_id(p.id()), Some(p));
            let text = toml::to_string(&AiConfig {
                provider: p,
                ..AiConfig::default()
            })
            .unwrap();
            assert!(
                text.contains(&format!("provider = \"{}\"", p.id())),
                "{text}"
            );
        }
        assert!(is_loopback_url("http://localhost:11434/v1"));
        assert!(is_loopback_url("http://127.0.0.1:1234/v1"));
        assert!(is_loopback_url("http://[::1]:8080/v1"));
        assert!(!is_loopback_url("https://api.openai.com/v1"));
        assert!(!is_loopback_url("http://localhost.evil.example/v1"));
        // User information hides the real host: never loopback.
        assert!(!is_loopback_url("http://127.0.0.1@evil.example/v1"));
        assert!(!is_loopback_url("http://localhost:1@evil.example/v1"));
        assert!(!is_loopback_url("http://127.0.0.1.evil.example/v1"));
        assert!(!is_loopback_url("http://127.0.0.1:80:81/v1"));
        assert!(!is_loopback_url("http://[::1]evil/v1"));
        assert!(!is_loopback_url("ftp://127.0.0.1/"));
        assert!(is_loopback_url("HTTP://LOCALHOST/v1"));
        assert!(is_loopback_url("http://127.0.0.2/v1"));
        assert!(ProviderKind::Ollama.is_local(""));
        assert!(!ProviderKind::Custom.is_local("https://x.example"));
    }
}

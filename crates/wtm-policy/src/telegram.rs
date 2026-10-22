//! Telegram bot settings, kept in `telegram.toml` next to the config: the
//! bot token is a secret, so only SYSTEM and administrators may read it.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Who may read and write `telegram.toml`.
pub const TELEGRAM_SDDL: &str = "D:P(A;;FA;;;SY)(A;;FA;;;BA)";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TelegramConfig {
    pub enabled: bool,
    /// From @BotFather, e.g. `123456:ABC-DEF…`.
    pub token: String,
    /// Chat IDs the bot answers; everyone else is told their ID and ignored.
    pub chats: Vec<i64>,
    /// Forward alerts (CPU overload, swarms, kill list) to the chats.
    pub notify: bool,
    /// Allow ending, suspending and resuming processes from Telegram.
    pub control: bool,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            token: String::new(),
            chats: Vec::new(),
            notify: true,
            control: true,
        }
    }
}

impl TelegramConfig {
    /// The settings in `text`. A parse error names its line and nothing of
    /// its content: the line, or a value quoted in the message (a token in
    /// the wrong field), may be the bot token, and errors reach the event
    /// log, which every user can read.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(text).map_err(|e: toml::de::Error| match e.span() {
            Some(span) => {
                let line = text[..span.start.min(text.len())].lines().count().max(1);
                format!("line {line} is not valid (check its quotes and the kind of value)")
            }
            None => "the settings are not valid TOML".to_owned(),
        })?;
        config.validate()?;
        Ok(config)
    }

    /// The settings in `path`; defaults (disabled) when there is no file.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.enabled && !token_looks_valid(&self.token) {
            return Err("the bot token should look like 123456789:ABC… (from @BotFather)".into());
        }
        Ok(())
    }
}

/// `<bot id>:<secret>`, the shape @BotFather hands out.
pub fn token_looks_valid(token: &str) -> bool {
    token.split_once(':').is_some_and(|(id, secret)| {
        !id.is_empty()
            && id.bytes().all(|b| b.is_ascii_digit())
            && secret.len() >= 20
            && secret
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

/// Chat IDs from text such as `123, -100456 789`.
pub fn parse_chats(text: &str) -> Result<Vec<i64>, String> {
    let mut chats = Vec::new();
    for part in text
        .split([',', ' ', ';', '\n'])
        .filter(|s| !s.trim().is_empty())
    {
        let id = part
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("`{}` is not a chat ID (a number)", part.trim()))?;
        if !chats.contains(&id) {
            chats.push(id);
        }
    }
    Ok(chats)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "123456789:AAFh3kq_Zx-9yLmnopQRstuVWxyz012345";

    #[test]
    fn a_parse_error_never_quotes_the_token() {
        // The token without its quotes: not TOML.
        let text = format!("enabled = true\ntoken = {TOKEN}\n");
        let e = TelegramConfig::from_toml(&text).unwrap_err();
        assert!(!e.contains("AAFh3kq"), "{e}");
        assert!(e.starts_with("line 2 "), "{e}");
        // The token in a field of another kind: serde would quote it.
        let text = format!(
            "enabled = true
chats = [\"{TOKEN}\"]
"
        );
        let e = TelegramConfig::from_toml(&text).unwrap_err();
        assert!(!e.contains("AAFh3kq"), "{e}");
    }

    #[test]
    fn round_trips_and_defaults_to_disabled() {
        let c = TelegramConfig {
            enabled: true,
            token: TOKEN.into(),
            chats: vec![42, -100123],
            notify: false,
            control: true,
        };
        assert_eq!(TelegramConfig::from_toml(&c.to_toml()).unwrap(), c);
        assert_eq!(
            TelegramConfig::from_toml("").unwrap(),
            TelegramConfig::default()
        );
        assert!(!TelegramConfig::default().enabled);
        let missing = std::env::temp_dir().join("wtm-telegram-missing.toml");
        assert_eq!(
            TelegramConfig::load(&missing).unwrap(),
            TelegramConfig::default()
        );
    }

    #[test]
    fn tokens_and_settings_are_checked() {
        assert!(token_looks_valid(TOKEN));
        for bad in [
            "",
            "123",
            "abc:AAFh3kq_Zx-9yLmnopQRstuVWxyz",
            "123:short",
            "1:has space in it xxxxxxxxx",
        ] {
            assert!(!token_looks_valid(bad), "{bad}");
        }
        assert!(TelegramConfig::from_toml("enabled = true\ntoken = \"x\"").is_err());
        assert!(
            TelegramConfig::from_toml("token = \"x\"").is_ok(),
            "only checked when enabled"
        );
        assert!(TelegramConfig::from_toml("unknown = 1").is_err());
    }

    #[test]
    fn chat_ids_are_parsed_from_text() {
        assert_eq!(
            parse_chats("123, -100456 789,123").unwrap(),
            [123, -100456, 789]
        );
        assert_eq!(parse_chats("  ").unwrap(), Vec::<i64>::new());
        assert!(parse_chats("12, abc").unwrap_err().contains("abc"));
    }
}

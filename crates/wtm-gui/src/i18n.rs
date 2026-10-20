//! Offline presentation catalog. Process names, paths and configuration values
//! never go through this lookup. Formatted values are inserted in one pass.
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Language {
    #[default]
    System,
    English,
    Turkish,
    German,
    French,
    Spanish,
}

impl Language {
    pub const ALL: [Self; 6] = [
        Self::System,
        Self::English,
        Self::Turkish,
        Self::German,
        Self::French,
        Self::Spanish,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::System => tr("Windows language"),
            Self::English => "English",
            Self::Turkish => "Türkçe",
            Self::German => "Deutsch",
            Self::French => "Français",
            Self::Spanish => "Español",
        }
    }
    fn index(self) -> u8 {
        match self {
            Self::System => system_language().index(),
            Self::English => 0,
            Self::Turkish => 1,
            Self::German => 2,
            Self::French => 3,
            Self::Spanish => 4,
        }
    }
    pub fn from_tag(tag: &str) -> Self {
        match tag
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "tr" => Self::Turkish,
            "de" => Self::German,
            "fr" => Self::French,
            "es" => Self::Spanish,
            _ => Self::English,
        }
    }
}

const REGISTRY: &str = r"Software\RustWTM";
static ACTIVE: AtomicU8 = AtomicU8::new(0);
thread_local! { static LOCAL: Cell<Option<u8>> = const { Cell::new(None) }; }
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Translations {
    tr: String,
    de: String,
    fr: String,
    es: String,
}
impl Translations {
    fn texts(&self) -> [&str; 4] {
        [&self.tr, &self.de, &self.fr, &self.es]
    }
}
type Catalog = BTreeMap<String, Translations>;
fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../locales/messages.json"))
            .expect("validated embedded translation catalog")
    })
}
fn system_language() -> Language {
    Language::from_tag(&wtm_core::system_ui_language())
}
pub fn preference() -> Language {
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(REGISTRY)
        .ok();
    read_preference(key.as_ref())
}
fn read_preference(key: Option<&winreg::RegKey>) -> Language {
    key.and_then(|k| k.get_value::<String, _>("Language").ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}
pub fn initialize() {
    activate(preference());
}
pub fn activate(language: Language) {
    ACTIVE.store(language.index(), Ordering::Relaxed);
}
pub fn save(language: Language) -> std::io::Result<()> {
    let (key, _) =
        winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).create_subkey(REGISTRY)?;
    write_preference(&key, language)?;
    activate(language);
    Ok(())
}
fn write_preference(key: &winreg::RegKey, language: Language) -> std::io::Result<()> {
    key.set_value(
        "Language",
        &serde_json::to_string(&language).expect("language serialization"),
    )
}
/// Render/test scopes restore their previous language even when unwinding.
#[must_use]
pub struct Scope(Option<u8>, std::marker::PhantomData<std::rc::Rc<()>>);
pub fn scope(language: Language) -> Scope {
    Scope(
        LOCAL.with(|c| c.replace(Some(language.index()))),
        std::marker::PhantomData,
    )
}
impl Drop for Scope {
    fn drop(&mut self) {
        LOCAL.with(|c| c.set(self.0));
    }
}
fn index() -> usize {
    LOCAL
        .with(|c| c.get())
        .unwrap_or_else(|| ACTIVE.load(Ordering::Relaxed)) as usize
}
pub fn tr(english: &str) -> &str {
    let i = index();
    if i == 0 {
        return english;
    }
    catalog()
        .get(english)
        .and_then(|v| v.texts().get(i - 1).copied())
        .filter(|s| !s.is_empty())
        .unwrap_or(english)
}

pub fn debug(value: impl std::fmt::Debug) -> String {
    tr(&format!("{value:?}")).to_owned()
}

/// Old service logs remain unchanged. Recognize complete known message templates
/// only at the presentation boundary; unknown/native diagnostics remain verbatim.
pub fn message(english: &str) -> std::borrow::Cow<'_, str> {
    message_at_depth(english, 0)
}
fn message_at_depth(english: &str, depth: usize) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    if index() == 0 {
        return Cow::Borrowed(english);
    }
    let exact = tr(english);
    if exact != english {
        return Cow::Borrowed(exact);
    }
    if english.len() > 16384 {
        return Cow::Borrowed(english);
    }
    static PATTERNS: OnceLock<Vec<&'static str>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        let mut keys: Vec<_> = catalog()
            .keys()
            .filter(|k| {
                k.contains("{0}")
                    && (k.chars().filter(char::is_ascii_alphabetic).count() >= 4
                        || k.starts_with("Rule:"))
            })
            .map(String::as_str)
            .collect();
        keys.sort_by_key(|k| {
            std::cmp::Reverse((k.chars().filter(char::is_ascii_alphabetic).count(), k.len()))
        });
        keys
    });
    for key in patterns {
        if let Some(mut values) = captures(key, english) {
            // These fields are actions/reasons, never program names or paths.
            if depth < 2
                && ["Governor:", "Rule:", "MemoryGuard:", "Insight:", "dry-run:"]
                    .iter()
                    .any(|p| key.starts_with(p))
            {
                for value in &mut values {
                    *value = message_at_depth(value, depth + 1).into_owned();
                }
            }
            if *key == "priority {0}" {
                values[0] = tr(&values[0]).to_owned();
            }
            return Cow::Owned(format(key, &values));
        }
    }
    Cow::Borrowed(english)
}

fn captures(template: &str, text: &str) -> Option<Vec<String>> {
    let mut rest = text;
    let mut pattern = template;
    let mut values = Vec::new();
    while let Some(open) = pattern.find('{') {
        rest = rest.strip_prefix(&pattern[..open])?;
        let tail = &pattern[open + 1..];
        let close = tail.find('}')?;
        let index = tail[..close].parse::<usize>().ok()?;
        if index != values.len() {
            return None;
        }
        pattern = &tail[close + 1..];
        let literal_end = pattern.find('{').unwrap_or(pattern.len());
        let delimiter = &pattern[..literal_end];
        // Adjacent placeholders cannot be recovered unambiguously from old logs.
        if delimiter.is_empty() && !pattern.is_empty() {
            return None;
        }
        let end = if pattern.is_empty() {
            rest.len()
        } else if literal_end == pattern.len() {
            rest.len()
                .checked_sub(delimiter.len())
                .filter(|_| rest.ends_with(delimiter))?
        } else {
            rest.find(delimiter)?
        };
        values.push(rest[..end].to_owned());
        rest = &rest[end..];
    }
    (rest == pattern).then_some(values)
}
pub fn format(english: &str, values: &[String]) -> String {
    let text = tr(english);
    let mut result =
        String::with_capacity(text.len() + values.iter().map(String::len).sum::<usize>());
    let mut rest = text;
    while let Some(i) = rest.find(['{', '}']) {
        result.push_str(&rest[..i]);
        rest = &rest[i..];
        if rest.starts_with("{{") || rest.starts_with("}}") {
            result.push(rest.chars().next().unwrap());
            rest = &rest[2..];
        } else if rest.starts_with('{')
            && let Some(end) = rest.find('}')
            && let Ok(n) = rest[1..end].parse::<usize>()
            && let Some(v) = values.get(n)
        {
            result.push_str(v);
            rest = &rest[end + 1..];
        } else {
            result.push(rest.chars().next().unwrap());
            rest = &rest[1..];
        }
    }
    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(text: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            if rest[open..].starts_with("{{") {
                rest = &rest[open + 2..];
                continue;
            }
            let tail = &rest[open + 1..];
            let end = tail.find('}').expect("unclosed placeholder");
            assert!(
                tail[..end].parse::<usize>().is_ok(),
                "invalid placeholder in {text}"
            );
            out.push(&tail[..end]);
            rest = &tail[end + 1..];
        }
        out.sort_unstable();
        out
    }
    #[test]
    fn every_translation_preserves_all_fields() {
        assert!(!catalog().is_empty());
        for (key, value) in catalog() {
            let expected = placeholders(key);
            for translated in value.texts() {
                assert!(!translated.trim().is_empty(), "empty translation: {key}");
                assert_eq!(
                    placeholders(translated),
                    expected,
                    "field mismatch: {key} -> {translated}"
                );
            }
        }
    }
    #[test]
    fn lookup_switches_in_all_five_languages_and_falls_back() {
        for (language, text) in [
            (Language::English, "Settings"),
            (Language::Turkish, "Ayarlar"),
            (Language::German, "Einstellungen"),
            (Language::French, "Paramètres"),
            (Language::Spanish, "Configuración"),
        ] {
            let _scope = scope(language);
            assert_eq!(tr("Settings"), text);
            assert_eq!(tr("an unknown future message"), "an unknown future message");
            assert_eq!(
                format("User: {0}", &["{1} çğİıöşü.exe".into()]),
                format!(
                    "{}{1}",
                    match language {
                        Language::English => "User: ",
                        Language::Turkish => "Kullanıcı: ",
                        Language::German => "Benutzer: ",
                        Language::French => "Utilisateur : ",
                        Language::Spanish => "Usuario: ",
                        Language::System => unreachable!(),
                    },
                    "{1} çğİıöşü.exe"
                )
            );
        }
    }
    #[test]
    fn scoped_languages_restore_after_unwinding_and_do_not_cross_threads() {
        let _scope = scope(Language::English);
        let _ = std::panic::catch_unwind(|| {
            let _s = scope(Language::Turkish);
            assert_eq!(tr("Settings"), "Ayarlar");
            panic!("scope test");
        });
        assert_eq!(tr("Settings"), "Settings");
        std::thread::scope(|s| {
            s.spawn(|| {
                let _s = scope(Language::French);
                assert_eq!(tr("Settings"), "Paramètres");
            })
            .join()
            .unwrap();
        });
        assert_eq!(tr("Settings"), "Settings");
    }
    #[test]
    fn region_tags_and_unknown_locales_resolve() {
        for (tag, language) in [
            ("tr-TR", Language::Turkish),
            ("FR_ca", Language::French),
            ("de-DE", Language::German),
            ("es-MX", Language::Spanish),
            ("en-GB", Language::English),
            ("ja-JP", Language::English),
            ("", Language::English),
        ] {
            assert_eq!(Language::from_tag(tag), language);
        }
    }
    #[test]
    fn persisted_language_round_trips_and_invalid_preferences_use_windows() {
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let path = format!(r"Software\RustWTM-tests-language-{}", std::process::id());
        let (key, _) = root.create_subkey(&path).unwrap();
        for language in Language::ALL {
            write_preference(&key, language).unwrap();
            assert_eq!(read_preference(Some(&key)), language);
        }
        let readonly = root.open_subkey(&path).unwrap();
        assert!(write_preference(&readonly, Language::Turkish).is_err());
        key.set_value("Language", &"broken JSON").unwrap();
        assert_eq!(read_preference(Some(&key)), Language::System);
        drop(readonly);
        drop(key);
        root.delete_subkey(&path).unwrap();
        assert_eq!(read_preference(None), Language::System);
    }
    #[test]
    fn known_service_messages_translate_without_altering_unknown_diagnostics() {
        let _scope = scope(Language::Turkish);
        assert_eq!(
            message("blocked from starting: çğİıöşü-{0}.exe"),
            "Başlaması engellendi: çğİıöşü-{0}.exe"
        );
        assert_eq!(
            message("0x80004005: native driver details"),
            "0x80004005: native driver details"
        );
        assert_eq!(
            captures("prefix {0} suffix", "prefix a suffix suffix"),
            Some(vec!["a suffix".into()])
        );
        assert_eq!(captures("{0}{1}", "ambiguous"), None);
        assert_eq!(
            message("Governor: priority BelowNormal (system CPU 98%, process at 40.0% (step 1/3))"),
            "Kaynak yöneticisi: öncelik Normalin altında (sistem CPU %98, işlem %40.0 (adım 1/3))"
        );
        assert_eq!(
            message("Rule: kill (rule 'Idle')"),
            "Kural: sonlandır (kural 'Idle')"
        );
    }
    #[test]
    fn literal_ui_lookups_have_catalog_entries() {
        let sources = [
            include_str!("app.rs"),
            include_str!("about.rs"),
            include_str!("alert.rs"),
            include_str!("backend.rs"),
            include_str!("kill_tab.rs"),
            include_str!("limits_tab.rs"),
            include_str!("overview.rs"),
            include_str!("ports_tab.rs"),
            include_str!("power_controls.rs"),
            include_str!("processes.rs"),
            include_str!("sensors.rs"),
            include_str!("settings.rs"),
            include_str!("sort.rs"),
            include_str!("telegram_settings.rs"),
            include_str!("tray.rs"),
            include_str!("views.rs"),
        ];
        for source in sources {
            let source = source.split("\n#[cfg(test)]\nmod tests").next().unwrap();
            for marker in ["crate::i18n::tr(", "crate::i18n::format("] {
                for tail in source.split(marker).skip(1) {
                    let literal = tail.trim_start();
                    if !literal.starts_with('"') {
                        continue;
                    }
                    let mut escaped = false;
                    let end = literal[1..]
                        .char_indices()
                        .find_map(|(i, c)| {
                            if escaped {
                                escaped = false;
                                None
                            } else if c == '\\' {
                                escaped = true;
                                None
                            } else if c == '"' {
                                Some(i + 2)
                            } else {
                                None
                            }
                        })
                        .unwrap();
                    let mut raw = literal[..end].replace("\r\n", "\n");
                    while let Some(start) = raw.find("\\\n") {
                        let after = &raw[start + 2..];
                        let end = start + 2 + after.len() - after.trim_start().len();
                        raw.replace_range(start..end, "");
                    }
                    let key = serde_json::from_str::<String>(&raw).unwrap();
                    assert!(catalog().contains_key(&key), "missing locale entry: {key}");
                }
            }
        }
    }
}

//! Offline presentation catalog. Process names, paths and configuration values
//! never go through this lookup. Formatted values are inserted in one pass.
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
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
    // Variant names are stored in the registry: append, never rename.
    ChineseSimplified,
    ChineseTraditional,
    Hindi,
    Bengali,
    Portuguese,
    Russian,
    Indonesian,
    Japanese,
    Marathi,
    Vietnamese,
    Telugu,
    Swahili,
    Filipino,
    Tamil,
    Korean,
    Italian,
    Gujarati,
    Kannada,
    Polish,
    Ukrainian,
    Dutch,
    Punjabi,
}

/// Languages by catalog index; English (0) is the source text.
const BY_INDEX: [Language; 27] = [
    Language::English,
    Language::Turkish,
    Language::German,
    Language::French,
    Language::Spanish,
    Language::ChineseSimplified,
    Language::ChineseTraditional,
    Language::Hindi,
    Language::Bengali,
    Language::Portuguese,
    Language::Russian,
    Language::Indonesian,
    Language::Japanese,
    Language::Marathi,
    Language::Vietnamese,
    Language::Telugu,
    Language::Swahili,
    Language::Filipino,
    Language::Tamil,
    Language::Korean,
    Language::Italian,
    Language::Gujarati,
    Language::Kannada,
    Language::Polish,
    Language::Ukrainian,
    Language::Dutch,
    Language::Punjabi,
];
/// Languages from index 5 on keep one `locales/<tag>.json` file each,
/// in `BY_INDEX` order.
const LOCALE_FILES: [(&str, &str); 22] = [
    ("zh-Hans", include_str!("../locales/zh-Hans.json")),
    ("zh-Hant", include_str!("../locales/zh-Hant.json")),
    ("hi", include_str!("../locales/hi.json")),
    ("bn", include_str!("../locales/bn.json")),
    ("pt-BR", include_str!("../locales/pt-BR.json")),
    ("ru", include_str!("../locales/ru.json")),
    ("id", include_str!("../locales/id.json")),
    ("ja", include_str!("../locales/ja.json")),
    ("mr", include_str!("../locales/mr.json")),
    ("vi", include_str!("../locales/vi.json")),
    ("te", include_str!("../locales/te.json")),
    ("sw", include_str!("../locales/sw.json")),
    ("fil", include_str!("../locales/fil.json")),
    ("ta", include_str!("../locales/ta.json")),
    ("ko", include_str!("../locales/ko.json")),
    ("it", include_str!("../locales/it.json")),
    ("gu", include_str!("../locales/gu.json")),
    ("kn", include_str!("../locales/kn.json")),
    ("pl", include_str!("../locales/pl.json")),
    ("uk", include_str!("../locales/uk.json")),
    ("nl", include_str!("../locales/nl.json")),
    ("pa", include_str!("../locales/pa.json")),
];
const FIRST_LOCALE_FILE: usize = 5;

impl Language {
    /// Picker order: Latin-script names alphabetically, then other scripts.
    pub const ALL: [Self; 28] = [
        Self::System,
        Self::Indonesian,
        Self::German,
        Self::English,
        Self::Spanish,
        Self::Filipino,
        Self::French,
        Self::Italian,
        Self::Swahili,
        Self::Dutch,
        Self::Polish,
        Self::Portuguese,
        Self::Vietnamese,
        Self::Turkish,
        Self::Russian,
        Self::Ukrainian,
        Self::Marathi,
        Self::Hindi,
        Self::Bengali,
        Self::Punjabi,
        Self::Gujarati,
        Self::Tamil,
        Self::Telugu,
        Self::Kannada,
        Self::ChineseSimplified,
        Self::ChineseTraditional,
        Self::Japanese,
        Self::Korean,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::System => tr("Windows language"),
            Self::English => "English",
            Self::Turkish => "Türkçe",
            Self::German => "Deutsch",
            Self::French => "Français",
            Self::Spanish => "Español",
            Self::ChineseSimplified => "简体中文",
            Self::ChineseTraditional => "繁體中文",
            Self::Hindi => "हिन्दी",
            Self::Bengali => "বাংলা",
            Self::Portuguese => "Português (Brasil)",
            Self::Russian => "Русский",
            Self::Indonesian => "Bahasa Indonesia",
            Self::Japanese => "日本語",
            Self::Marathi => "मराठी",
            Self::Vietnamese => "Tiếng Việt",
            Self::Telugu => "తెలుగు",
            Self::Swahili => "Kiswahili",
            Self::Filipino => "Filipino",
            Self::Tamil => "தமிழ்",
            Self::Korean => "한국어",
            Self::Italian => "Italiano",
            Self::Gujarati => "ગુજરાતી",
            Self::Kannada => "ಕನ್ನಡ",
            Self::Polish => "Polski",
            Self::Ukrainian => "Українська",
            Self::Dutch => "Nederlands",
            Self::Punjabi => "ਪੰਜਾਬੀ",
        }
    }
    fn index(self) -> u8 {
        if self == Self::System {
            return system_language().index();
        }
        BY_INDEX
            .iter()
            .position(|&l| l == self)
            .expect("every language has a catalog index") as u8
    }
    /// Every language with a catalog, English first.
    #[cfg(test)]
    pub const TRANSLATED: [Self; 27] = BY_INDEX;
    /// The language text is shown in now, with the Windows choice resolved.
    pub fn current() -> Self {
        BY_INDEX[index()]
    }
    /// Short language tag, e.g. for naming review captures.
    #[cfg(test)]
    pub fn code(self) -> &'static str {
        match self.index() as usize {
            0 => "en",
            1 => "tr",
            2 => "de",
            3 => "fr",
            4 => "es",
            i => LOCALE_FILES[i - FIRST_LOCALE_FILE].0,
        }
    }
    pub fn from_tag(tag: &str) -> Self {
        let tag = tag.to_ascii_lowercase();
        let mut parts = tag.split(['-', '_']);
        let primary = parts.next().unwrap_or("");
        let mut rest = parts;
        match primary {
            "tr" => Self::Turkish,
            "de" => Self::German,
            "fr" => Self::French,
            "es" => Self::Spanish,
            "zh" if rest.any(|p| matches!(p, "hant" | "tw" | "hk" | "mo")) => {
                Self::ChineseTraditional
            }
            "zh" => Self::ChineseSimplified,
            "hi" => Self::Hindi,
            "bn" => Self::Bengali,
            "pt" => Self::Portuguese,
            "ru" => Self::Russian,
            // "in" is the withdrawn code that older systems still report.
            "id" | "in" => Self::Indonesian,
            "ja" => Self::Japanese,
            "mr" => Self::Marathi,
            "vi" => Self::Vietnamese,
            "te" => Self::Telugu,
            "sw" => Self::Swahili,
            "fil" | "tl" => Self::Filipino,
            "ta" => Self::Tamil,
            "ko" => Self::Korean,
            "it" => Self::Italian,
            "gu" => Self::Gujarati,
            "kn" => Self::Kannada,
            "pl" => Self::Polish,
            "uk" => Self::Ukrainian,
            "nl" => Self::Dutch,
            // The catalog is Gurmukhi; Shahmukhi (Pakistan) is right-to-left.
            "pa" if rest.any(|p| matches!(p, "arab" | "pk")) => Self::English,
            "pa" => Self::Punjabi,
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
/// Every English key with its Turkish, German, French and Spanish text.
fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../locales/messages.json"))
            .expect("validated embedded translation catalog")
    })
}
type LocaleCatalog = HashMap<String, String>;
/// A per-language file, parsed on first use so only the languages
/// actually shown take memory.
fn locale_catalog(index: usize) -> Option<&'static LocaleCatalog> {
    static LOADED: [OnceLock<LocaleCatalog>; LOCALE_FILES.len()] =
        [const { OnceLock::new() }; LOCALE_FILES.len()];
    let slot = index.checked_sub(FIRST_LOCALE_FILE)?;
    let (tag, json) = LOCALE_FILES.get(slot)?;
    Some(LOADED[slot].get_or_init(|| {
        serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("validated embedded locale {tag}: {e}"))
    }))
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
    let text = match locale_catalog(i) {
        Some(locale) => locale.get(english).map(String::as_str),
        None => catalog()
            .get(english)
            .and_then(|v| v.texts().get(i - 1).copied()),
    };
    text.filter(|s| !s.is_empty()).unwrap_or(english)
}

pub fn debug(value: impl std::fmt::Debug) -> String {
    tr(&format!("{value:?}")).to_owned()
}

/// Old service logs remain unchanged. Recognize complete known message templates
/// only at the presentation boundary; unknown/native diagnostics remain verbatim.
pub fn message(english: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    use std::cell::RefCell;
    use std::collections::HashMap;
    /// Messages kept per thread; the screen shows far fewer at a time.
    const KEPT: usize = 1024;
    // Matching a message against every template takes long enough to show
    // when a frame does it for each visible event, so results are kept for
    // the current language: `None` when the text stays as it is.
    thread_local! {
        static SEEN: RefCell<(usize, HashMap<String, Option<String>>)> =
            RefCell::new((0, HashMap::new()));
    }
    let language = index();
    if language == 0 {
        return Cow::Borrowed(english);
    }
    let known = SEEN.with(|seen| {
        let seen = seen.borrow();
        (seen.0 == language)
            .then(|| seen.1.get(english).cloned())
            .flatten()
    });
    if let Some(known) = known {
        return known.map_or(Cow::Borrowed(english), Cow::Owned);
    }
    let text = message_at_depth(english, 0);
    SEEN.with(|seen| {
        let mut seen = seen.borrow_mut();
        if seen.0 != language || seen.1.len() >= KEPT {
            *seen = (language, HashMap::new());
        }
        let translated = match &text {
            Cow::Borrowed(t) if std::ptr::eq(*t, english) => None,
            t => Some(t.clone().into_owned()),
        };
        seen.1.insert(english.to_owned(), translated);
    });
    text
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
    fn every_locale_file_translates_exactly_the_catalog_keys() {
        let edges = |text: &str| {
            let start = text.len() - text.trim_start().len();
            let end = text.trim_end().len();
            (text[..start].to_owned(), text[end..].to_owned())
        };
        for (slot, (tag, _)) in LOCALE_FILES.iter().enumerate() {
            let index = FIRST_LOCALE_FILE + slot;
            assert_eq!(BY_INDEX[index].index() as usize, index, "{tag}");
            let locale = locale_catalog(index).unwrap();
            let missing: Vec<_> = catalog()
                .keys()
                .filter(|k| !locale.contains_key(*k))
                .collect();
            assert!(missing.is_empty(), "{tag} is missing {missing:?}");
            let extra: Vec<_> = locale
                .keys()
                .filter(|k| !catalog().contains_key(*k))
                .collect();
            assert!(extra.is_empty(), "{tag} has unknown keys {extra:?}");
            for (key, translated) in locale {
                assert!(
                    !translated.trim().is_empty(),
                    "{tag}: empty translation: {key}"
                );
                assert_eq!(
                    placeholders(translated),
                    placeholders(key),
                    "{tag}: field mismatch: {key} -> {translated}"
                );
                assert_eq!(edges(translated), edges(key), "{tag}: spacing: {key:?}");
            }
        }
        assert_eq!(BY_INDEX.len(), Language::ALL.len() - 1);
        for language in BY_INDEX {
            assert!(
                Language::ALL.contains(&language),
                "{language:?} not offered"
            );
        }
    }
    #[test]
    fn every_language_translates_and_reports_itself_as_current() {
        for language in BY_INDEX {
            let _scope = scope(language);
            assert_eq!(Language::current(), language);
            if language != Language::English {
                assert_ne!(tr("Settings"), "Settings", "{language:?}");
            }
            assert_eq!(tr("an unknown future message"), "an unknown future message");
        }
        let _scope = scope(Language::Russian);
        assert_eq!(tr("Settings"), "Параметры");
        assert_eq!(
            format("User: {0}", &["{1} çğİıöşü.exe".into()]),
            tr("User: {0}").replace("{0}", "{1} çğİıöşü.exe")
        );
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
                        _ => unreachable!(),
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
            ("ja-JP", Language::Japanese),
            ("zh-CN", Language::ChineseSimplified),
            ("zh-Hans-SG", Language::ChineseSimplified),
            ("zh-TW", Language::ChineseTraditional),
            ("zh_HK", Language::ChineseTraditional),
            ("zh-Hant", Language::ChineseTraditional),
            ("pt-PT", Language::Portuguese),
            ("ru-RU", Language::Russian),
            ("uk-UA", Language::Ukrainian),
            ("in-ID", Language::Indonesian),
            ("tl-PH", Language::Filipino),
            ("fil-PH", Language::Filipino),
            ("ko-KR", Language::Korean),
            ("hi-IN", Language::Hindi),
            ("pa-IN", Language::Punjabi),
            ("pa-Guru-IN", Language::Punjabi),
            ("pa-Arab-PK", Language::English),
            ("ar-SA", Language::English),
            ("he-IL", Language::English),
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
            include_str!("numeric_tray.rs"),
            include_str!("devices.rs"),
            include_str!("packages.rs"),
            include_str!("incidents.rs"),
            include_str!("file_locks.rs"),
            include_str!("rules.rs"),
            include_str!("experience.rs"),
            include_str!("app.rs"),
            include_str!("charts.rs"),
            include_str!("performance_view.rs"),
            include_str!("about.rs"),
            include_str!("alert.rs"),
            include_str!("disks_tab.rs"),
            include_str!("containers.rs"),
            include_str!("backend.rs"),
            include_str!("kill_tab.rs"),
            include_str!("limits_tab.rs"),
            include_str!("overview.rs"),
            include_str!("ports_tab.rs"),
            include_str!("power_controls.rs"),
            include_str!("processes.rs"),
            include_str!("process_columns.rs"),
            include_str!("resource_strip.rs"),
            include_str!("workspace_inspector.rs"),
            include_str!("sensors.rs"),
            include_str!("settings.rs"),
            include_str!("preferences.rs"),
            include_str!("inventory.rs"),
            include_str!("sort.rs"),
            include_str!("telegram_settings.rs"),
            include_str!("theme.rs"),
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

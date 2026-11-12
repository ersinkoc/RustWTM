//! Removes secrets from text before it leaves the machine. Command lines
//! carry passwords, tokens and keys more often than they should
//! (`--password=…`, `https://user:pass@host`, `Authorization: Bearer …`),
//! and paths carry the Windows account name.
//!
//! Best effort, by shape: it cannot know every secret, so the consent text
//! says command lines are sent and offers to leave them out entirely.

const MASK: &str = "[redacted]";

/// Key names whose value is a secret, compared without `-`, `_`, `.` and
/// case, when the name contains one of these…
const SECRET_PARTS: [&str; 13] = [
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "apikey",
    "accesskey",
    "privatekey",
    "credential",
    "auth",
    "cookie",
    "session",
    "connectionstring",
];
/// …or is exactly one of these.
const SECRET_NAMES: [&str; 6] = ["pass", "pw", "key", "sig", "sas", "p"];

/// Prefixes of well-known API keys and tokens.
const TOKEN_PREFIXES: [&str; 16] = [
    "sk-",
    "sk_live_",
    "sk_test_",
    "rk_live_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xapp-",
    "npm_",
    "hf_",
];

fn is_secret_name(name: &str) -> bool {
    let key: String = name
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | '.'))
        .flat_map(char::to_lowercase)
        .collect();
    !key.is_empty()
        && (SECRET_NAMES.contains(&key.as_str()) || SECRET_PARTS.iter().any(|p| key.contains(p)))
}

/// Characters of a `name` before `=`.
fn name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// Where a value after `=` ends inside one token.
fn value_end(c: char) -> bool {
    matches!(c, '&' | ';' | ',' | '"' | '\'')
}

/// What [`redact`] does, with the account name to hide given explicitly.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    /// Replaced by `<user>` wherever it appears (3 characters or more).
    pub user_name: Option<String>,
}

impl Redactor {
    /// Hides the current Windows account (`%USERNAME%`) too.
    pub fn for_current_user() -> Self {
        Self {
            user_name: std::env::var("USERNAME").ok(),
        }
    }

    pub fn redact(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut secret_next = false;
        let mut rest = text;
        while !rest.is_empty() {
            let space = rest.len() - rest.trim_start().len();
            out.push_str(&rest[..space]);
            rest = &rest[space..];
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let token = &rest[..end];
            rest = &rest[end..];
            if token.is_empty() {
                continue;
            }
            if secret_next && !token.starts_with('-') {
                out.push_str(MASK);
                secret_next = false;
                continue;
            }
            let (masked, flag) = token_redacted(token);
            secret_next = flag;
            out.push_str(&masked);
        }
        self.hide_user(&out)
    }

    fn hide_user(&self, text: &str) -> String {
        let mut text = user_profile_paths(text);
        if let Some(name) = self.user_name.as_deref().filter(|n| n.chars().count() >= 3) {
            text = replace_ignore_case(&text, name, "<user>");
        }
        text
    }
}

/// `redact` with the current account hidden.
pub fn redact(text: &str) -> String {
    Redactor::for_current_user().redact(text)
}

/// One whitespace-free token, masked; and whether the next token is the
/// value of a secret flag (`--password hunter2`, `Bearer abc`).
fn token_redacted(token: &str) -> (String, bool) {
    let bare = token.trim_matches(|c| c == '"' || c == '\'');
    // A flag naming a secret, its value in the next token.
    let flag = bare.trim_start_matches(['-', '/']);
    if bare.len() != flag.len() && !flag.contains(['=', ':']) && is_secret_name(flag) {
        return (token.to_owned(), true);
    }
    if bare.eq_ignore_ascii_case("bearer") || bare.eq_ignore_ascii_case("basic") {
        return (token.to_owned(), true);
    }
    if looks_like_token(bare) {
        return (token.replacen(bare, MASK, 1), false);
    }
    let token = url_credentials(token);
    let token = assignments(&token);
    (colon_flag(&token), false)
}

/// `scheme://user:pass@host` without the user and password.
fn url_credentials(token: &str) -> String {
    let mut out = String::new();
    let mut rest = token;
    while let Some(at) = rest.find("://") {
        let start = at + 3;
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        if let Some(user_end) = rest[..authority_end].rfind('@') {
            out.push_str(MASK);
            rest = &rest[user_end..];
        }
    }
    out.push_str(rest);
    out
}

/// `name=value` pairs (also inside query strings) with secret names masked.
fn assignments(token: &str) -> String {
    let mut out = String::with_capacity(token.len());
    let chars: Vec<(usize, char)> = token.char_indices().collect();
    let mut i = 0;
    let mut copied = 0;
    while i < chars.len() {
        let (pos, c) = chars[i];
        if c == '=' {
            let mut j = i;
            while j > 0 && name_char(chars[j - 1].1) {
                j -= 1;
            }
            let name = &token[chars[j].0.min(pos)..pos];
            if is_secret_name(name.trim_start_matches(['-', '/'])) {
                let mut k = i + 1;
                while k < chars.len() && !value_end(chars[k].1) {
                    k += 1;
                }
                if k > i + 1 {
                    out.push_str(&token[copied..=pos]);
                    out.push_str(MASK);
                    copied = chars.get(k).map_or(token.len(), |c| c.0);
                    i = k;
                    continue;
                }
            }
        }
        i += 1;
    }
    out.push_str(&token[copied..]);
    out
}

/// `/password:hunter2` (Windows style).
fn colon_flag(token: &str) -> String {
    let Some(rest) = token.strip_prefix('/') else {
        return token.to_owned();
    };
    match rest.split_once(':') {
        Some((name, value)) if !value.is_empty() && is_secret_name(name) => {
            format!("/{name}:{MASK}")
        }
        _ => token.to_owned(),
    }
}

/// A well-known key or token, a JWT, or a long opaque string.
fn looks_like_token(s: &str) -> bool {
    let s = s.trim_end_matches([',', ';', ')']);
    if TOKEN_PREFIXES
        .iter()
        .any(|p| s.starts_with(p) && s.len() >= p.len() + 12)
    {
        return true;
    }
    let upper_alnum = |s: &str| {
        s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    };
    if (s.starts_with("AKIA") || s.starts_with("ASIA")) && s.len() == 20 && upper_alnum(s) {
        return true;
    }
    if s.starts_with("AIza") && s.len() >= 35 {
        return true;
    }
    if s.starts_with("eyJ") && s.matches('.').count() == 2 && s.len() >= 30 {
        return true;
    }
    // Long opaque blobs: letters and digits mixed, no path or name shape.
    s.len() >= 40
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'_' | b'-'))
        && s.bytes().any(|b| b.is_ascii_digit())
        && s.bytes().any(|b| b.is_ascii_alphabetic())
        && !s.starts_with('-')
        && !s.starts_with('/')
        && !is_guid(s)
}

fn is_guid(s: &str) -> bool {
    let parts: Vec<&str> = s.trim_matches(['{', '}']).split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// `…\Users\<name>\…` with the account folder hidden (not Public / Default).
fn user_profile_paths(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut from = 0;
    while let Some(found) = lower[from..].find("\\users\\") {
        let start = from + found + "\\users\\".len();
        let end = lower[start..]
            .find(['\\', '"', '\'', ' ', '\n'])
            .map_or(lower.len(), |e| start + e);
        let name = &lower[start..end];
        let keep = name.is_empty()
            || matches!(
                name,
                "public" | "default" | "all users" | "default user" | "<user>"
            );
        if !keep && end < lower.len() && text.as_bytes()[end] == b'\\' {
            out.push_str(&text[copied..start]);
            out.push_str("<user>");
            copied = end;
        }
        from = end.max(start);
    }
    out.push_str(&text[copied..]);
    out
}

fn replace_ignore_case(text: &str, needle: &str, with: &str) -> String {
    let lower = text.to_lowercase();
    let needle_lower = needle.to_lowercase();
    // Lowercasing changed a length: fall back to an exact replace.
    if lower.len() != text.len() || needle_lower.len() != needle.len() {
        return text.replace(needle, with);
    }
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (pos, _) in lower.match_indices(&needle_lower) {
        if pos < copied {
            continue;
        }
        out.push_str(&text[copied..pos]);
        out.push_str(with);
        copied = pos + needle.len();
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(text: &str) -> String {
        Redactor {
            user_name: Some("ersin".into()),
        }
        .redact(text)
    }

    #[test]
    fn masks_secret_assignments_and_flags() {
        assert_eq!(
            r("app.exe --password=hunter2 --port=80"),
            "app.exe --password=[redacted] --port=80"
        );
        assert_eq!(
            r("app.exe --api-key abc123 -v"),
            "app.exe --api-key [redacted] -v"
        );
        assert_eq!(r("tool /pwd:s3cret /quiet"), "tool /pwd:[redacted] /quiet");
        assert_eq!(r("x \"--token=abc def\""), "x \"--token=[redacted] def\"");
        assert_eq!(
            r("curl -H Authorization: Bearer abc.def.ghi"),
            "curl -H Authorization: Bearer [redacted]"
        );
        // A flag followed by another flag has no value to hide.
        assert_eq!(r("svc --password --verbose"), "svc --password --verbose");
    }

    #[test]
    fn masks_url_credentials_and_query_secrets() {
        assert_eq!(
            r("git fetch https://bob:pa55@example.com/repo.git"),
            "git fetch https://[redacted]@example.com/repo.git"
        );
        assert_eq!(
            r("open https://h.example/cb?code=1&access_token=xyz&state=2"),
            "open https://h.example/cb?code=1&access_token=[redacted]&state=2"
        );
        assert_eq!(
            r("db Server=x;User Id=sa;Password=p4ss;Database=d"),
            "db Server=x;User Id=sa;Password=[redacted];Database=d"
        );
    }

    #[test]
    fn masks_well_known_tokens_and_long_blobs() {
        let key = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
        assert_eq!(r(&format!("run {key}")), "run [redacted]");
        assert_eq!(r("AKIAIOSFODNN7EXAMPLE"), "[redacted]");
        assert_eq!(r("ghp_abcdefghijklmnopqrstuvwxyz0123456789"), "[redacted]");
        let blob = "Zm9vYmFyYmF6cXV4MTIzNDU2Nzg5MGFiY2RlZmdoaWprbG1ub3A=";
        assert_eq!(r(&format!("x {blob}")), "x [redacted]");
        // GUIDs, paths and ordinary flags stay readable.
        let guid = "{8A7B1C2D-1111-2222-3333-444455556666}";
        assert_eq!(r(guid), guid);
        let chrome = r#""C:\Program Files\Google\Chrome\Application\chrome.exe" --type=renderer --field-trial-handle=1234,i,5678"#;
        assert_eq!(r(chrome), chrome);
        assert_eq!(
            r("svchost.exe -k netsvcs -p -s Schedule"),
            "svchost.exe -k netsvcs -p -s Schedule"
        );
    }

    #[test]
    fn hides_the_account_name() {
        assert_eq!(
            r(r"C:\Users\Ersin\AppData\Local\app.exe"),
            r"C:\Users\<user>\AppData\Local\app.exe"
        );
        assert_eq!(
            r(r"C:\Users\someone\x.exe and C:\Users\Public\y.exe"),
            r"C:\Users\<user>\x.exe and C:\Users\Public\y.exe"
        );
        assert_eq!(r(r"user: DESKTOP\ERSIN"), r"user: DESKTOP\<user>");
        // Short names are not replaced everywhere (too many false hits).
        let short = Redactor {
            user_name: Some("al".into()),
        };
        assert_eq!(short.redact("alpha"), "alpha");
    }

    #[test]
    fn keeps_whitespace_and_lines() {
        let text = "line one\n  indented --secret=x\n\nend";
        assert_eq!(r(text), "line one\n  indented --secret=[redacted]\n\nend");
    }
}

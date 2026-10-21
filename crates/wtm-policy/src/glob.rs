/// Case-insensitive wildcard match supporting `*` (any run) and `?` (one char).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    // Image names and paths are nearly always ASCII: compare the bytes in
    // place instead of lowercasing both sides into new buffers on every call.
    if pattern.is_ascii() && text.is_ascii() {
        return wildcard(pattern.as_bytes(), text.as_bytes(), b'*', b'?', |a, b| {
            a.eq_ignore_ascii_case(&b)
        });
    }
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    wildcard(&p, &t, '*', '?', |a, b| a == b)
}

/// Whether `text` contains `pattern`, ignoring case.
pub(crate) fn contains_ignore_case(text: &str, pattern: &str) -> bool {
    if text.is_ascii() && pattern.is_ascii() {
        let (t, p) = (text.as_bytes(), pattern.as_bytes());
        return p.is_empty() || t.windows(p.len()).any(|w| w.eq_ignore_ascii_case(p));
    }
    text.to_lowercase().contains(&pattern.to_lowercase())
}

/// Whether `name.to_lowercase() == lower`, where `lower` is already
/// lowercased; ASCII names are compared without allocating.
pub(crate) fn eq_lowercase(name: &str, lower: &str) -> bool {
    if name.is_ascii() {
        name.eq_ignore_ascii_case(lower)
    } else {
        name.to_lowercase() == lower
    }
}

fn wildcard<T: Copy + PartialEq>(
    p: &[T],
    t: &[T],
    any: T,
    one: T,
    eq: impl Fn(T, T) -> bool,
) -> bool {
    let (mut pi, mut ti) = (0, 0);
    // Position of the last `*` and the text index it is currently matched up to.
    let mut star: Option<(usize, usize)> = None;

    while ti < t.len() {
        if pi < p.len() && (p[pi] == one || eq(p[pi], t[ti])) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == any {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == any)
}

#[cfg(test)]
mod tests {
    use super::{contains_ignore_case, glob_match, wildcard};

    #[test]
    fn wildcards() {
        assert!(glob_match("chrome.exe", "Chrome.EXE"));
        assert!(glob_match("*.exe", "a.exe"));
        assert!(glob_match("xmrig*", "xmrig-6.21.exe"));
        assert!(glob_match("c:\\games\\*\\*.exe", "C:\\Games\\Foo\\bar.exe"));
        assert!(glob_match("a?c", "abc"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("a?c", "ac"));
        assert!(!glob_match("*.exe", "a.dll"));
        assert!(!glob_match("chrome.exe", "chrome.exe.bak"));
    }

    /// The matcher before the ASCII fast path: lowercase both, then compare.
    fn reference(pattern: &str, text: &str) -> bool {
        let p: Vec<char> = pattern.to_lowercase().chars().collect();
        let t: Vec<char> = text.to_lowercase().chars().collect();
        wildcard(&p, &t, '*', '?', |a, b| a == b)
    }

    #[test]
    fn fast_path_agrees_with_lowercasing() {
        let patterns = [
            "",
            "*",
            "?",
            "**",
            "a*b?c",
            "*.EXE",
            "x*y*z",
            "Çay*",
            "*\u{212A}*",
            "*?",
            "a*",
        ];
        let texts = [
            "",
            "a",
            "abc",
            "aXbYc",
            "A.exe",
            "xyz",
            "XaYbZ",
            "çaykur.exe",
            "K.exe",
            "*",
            "?",
            "\u{212A}.exe",
            "ÇAY.EXE",
        ];
        for p in patterns {
            for t in texts {
                assert_eq!(glob_match(p, t), reference(p, t), "{p:?} vs {t:?}");
                assert_eq!(
                    contains_ignore_case(t, p),
                    t.to_lowercase().contains(&p.to_lowercase()),
                    "{t:?} contains {p:?}"
                );
            }
        }
        // Non-ASCII goes the lowercasing way, so the Kelvin sign is a `k`.
        assert!(glob_match("\u{212A}.exe", "K.EXE"));
        assert!(glob_match("çay*", "ÇAYKUR.exe"));
    }
}

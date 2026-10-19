/// Case-insensitive wildcard match supporting `*` (any run) and `?` (one char).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    // Position of the last `*` and the text index it is currently matched up to.
    let mut star: Option<(usize, usize)> = None;

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
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
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::glob_match;

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
}

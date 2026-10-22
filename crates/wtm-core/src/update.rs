//! Update checks against the GitHub releases of RustWTM, and installing a
//! newer version: the MSI is downloaded, verified against its published
//! SHA-256, and handed to Windows Installer.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The latest release, as the GitHub API describes it.
pub const LATEST_URL: &str = "https://api.github.com/repos/ersinkoc/RustWTM/releases/latest";
/// The release page for people without an installer (zip, developer builds).
pub const RELEASES_PAGE: &str = "https://github.com/ersinkoc/RustWTM/releases/latest";

pub type Version = (u32, u32, u32);

/// `0.7.0` or `v0.7.0`; pre-release suffixes (`-rc1`) are ignored.
pub fn parse_version(s: &str) -> Option<Version> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u32>().ok());
    let v = (
        parts.next()??,
        parts.next()??,
        parts.next().unwrap_or(Some(0))?,
    );
    parts.next().is_none().then_some(v)
}

/// The version of this build.
pub fn current() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).expect("the crate version is valid")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    /// The release page.
    pub page: String,
    /// The installer and its `.sha256` file, when published.
    pub msi: Option<(String, String)>,
}

impl Release {
    pub fn is_newer_than(&self, version: Version) -> bool {
        self.version > version
    }

    pub fn version_text(&self) -> String {
        let (a, b, c) = self.version;
        format!("{a}.{b}.{c}")
    }
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

/// Reads the API's answer for one release.
pub fn parse_release(json: &[u8]) -> Result<Release, String> {
    let r: ApiRelease =
        serde_json::from_slice(json).map_err(|e| format!("unexpected answer: {e}"))?;
    if r.draft || r.prerelease {
        return Err(format!("{} is not a final release", r.tag_name));
    }
    let version =
        parse_version(&r.tag_name).ok_or_else(|| format!("unexpected tag {}", r.tag_name))?;
    let url = |name: &str| {
        r.assets
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
            .map(|a| a.browser_download_url.clone())
    };
    let msi = r
        .assets
        .iter()
        .find(|a| a.name.to_ascii_lowercase().ends_with(".msi"))
        .and_then(|a| {
            Some((
                a.browser_download_url.clone(),
                url(&format!("{}.sha256", a.name))?,
            ))
        });
    Ok(Release {
        version,
        tag: r.tag_name,
        page: r.html_url,
        msi,
    })
}

const USER_AGENT: &str = concat!("RustWTM/", env!("CARGO_PKG_VERSION"));

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    match wtm_sys::http::get(url, USER_AGENT) {
        Ok((200, body)) => Ok(body),
        Ok((status, _)) => Err(format!("{url} answered HTTP {status}")),
        Err(e) => Err(format!("cannot reach {url}: {e}")),
    }
}

/// The latest published release.
pub fn latest() -> Result<Release, String> {
    latest_from(LATEST_URL)
}

fn latest_from(url: &str) -> Result<Release, String> {
    parse_release(&fetch(url)?)
}

/// The hash in a `sha256sum`-style line: `<64 hex digits>  <file name>`.
pub fn parse_sha256(text: &str) -> Option<[u8; 32]> {
    let hex = text.split_whitespace().next()?;
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// A downloaded installer and the SHA-256 it was checked against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub path: PathBuf,
    pub sha256: [u8; 32],
}

/// Downloads the release's installer into `dir` and checks it against the
/// published SHA-256. Nothing is written when the check fails.
pub fn download(release: &Release, dir: &Path) -> Result<Download, String> {
    let (msi_url, sha_url) = release
        .msi
        .as_ref()
        .ok_or("this release has no installer")?;
    let expected = parse_sha256(&String::from_utf8_lossy(&fetch(sha_url)?))
        .ok_or("the published checksum is unreadable")?;
    let msi = fetch(msi_url)?;
    let actual = wtm_sys::http::sha256(&msi).map_err(|e| e.to_string())?;
    if actual != expected {
        return Err(
            "the download does not match its published checksum; nothing was installed".into(),
        );
    }
    let name = msi_url
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or("RustWTM.msi");
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(name);
    std::fs::write(&path, msi).map_err(|e| format!("cannot save {}: {e}", path.display()))?;
    Ok(Download {
        path,
        sha256: expected,
    })
}

/// Whether `exe` runs from the folder the MSI installed RustWTM into (only
/// then can an installer update it).
pub fn installed_by_msi(exe: &Path) -> bool {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let dir: Option<String> = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"Software\RustWTM")
        .and_then(|k| k.get_value("InstallDir"))
        .ok();
    dir.is_some_and(|d| same_dir(Path::new(&d), exe))
}

fn same_dir(install_dir: &Path, exe: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches('\\').to_lowercase();
    exe.parent().is_some_and(|p| norm(p) == norm(install_dir))
}

/// Runs the installer (Windows asks for administrator rights) and starts
/// `relaunch` once it is done. The caller should exit right away so its
/// files can be replaced.
///
/// The download sits in the user's temp folder, where anything running as
/// the user could swap it after it was checked. So the elevated step copies
/// it to a folder only administrators can reach, checks the copy against
/// the hash again and installs that.
pub fn install(msi: &Download, relaunch: &Path) -> Result<(), String> {
    let args = format!(
        "-NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand {}",
        encode_command(&launcher(msi, relaunch))
    );
    wtm_sys::shell::launch_hidden("powershell.exe", &args).map_err(|e| e.to_string())
}

/// Asks for elevation to run [`elevated_install`], then starts `relaunch`
/// as the user, also when elevation is declined.
fn launcher(msi: &Download, relaunch: &Path) -> String {
    format!(
        "try {{ Start-Process powershell.exe -Verb RunAs -Wait -WindowStyle Hidden \
         -ArgumentList '-NoProfile','-NonInteractive','-EncodedCommand','{}' }} \
         finally {{ Start-Process -FilePath {} }}",
        encode_command(&elevated_install(msi)),
        ps_quote(&relaunch.to_string_lossy())
    )
}

/// Copies the installer to a fresh folder under the system's temp folder
/// that only SYSTEM and administrators can reach, checks it and installs it.
fn elevated_install(msi: &Download) -> String {
    let hash: String = msi.sha256.iter().map(|b| format!("{b:02X}")).collect();
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $dir = Join-Path $env:SystemRoot ('Temp\\rustwtm-update-' + [Guid]::NewGuid().ToString('N'))\n\
         New-Item -ItemType Directory -Path $dir | Out-Null\n\
         try {{\n\
         $acl = New-Object Security.AccessControl.DirectorySecurity\n\
         $acl.SetSecurityDescriptorSddlForm('O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)')\n\
         Set-Acl -LiteralPath $dir -AclObject $acl\n\
         $msi = Join-Path $dir 'RustWTM.msi'\n\
         Copy-Item -LiteralPath {} -Destination $msi\n\
         if ((Get-FileHash -Algorithm SHA256 -LiteralPath $msi).Hash -ne '{hash}') {{ exit 1 }}\n\
         $p = Start-Process msiexec.exe -ArgumentList '/i', ('\"' + $msi + '\"'), '/passive', '/norestart' -Wait -PassThru\n\
         exit $p.ExitCode\n\
         }} finally {{ cmd.exe /d /c rd /s /q \"$dir\" }}\n",
        ps_quote(&msi.path.to_string_lossy())
    )
}

/// `text` as a single-quoted PowerShell string. PowerShell ends one at any
/// of ' ‘ ’ ‚ ‛ (as in C:\Users\O’Brien), so each is doubled.
fn ps_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for c in text.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// A script for `powershell -EncodedCommand`: base64 of its UTF-16LE.
fn encode_command(script: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const JSON: &str = r#"{
        "tag_name": "v0.8.1", "html_url": "https://github.com/x/releases/tag/v0.8.1",
        "draft": false, "prerelease": false,
        "assets": [
            {"name": "RustWTM-0.8.1-x64.msi", "browser_download_url": "https://dl/RustWTM-0.8.1-x64.msi"},
            {"name": "RustWTM-0.8.1-x64.msi.sha256", "browser_download_url": "https://dl/RustWTM-0.8.1-x64.msi.sha256"},
            {"name": "rustwtm.zip", "browser_download_url": "https://dl/rustwtm.zip"}
        ]
    }"#;

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(parse_version("v0.7.0"), Some((0, 7, 0)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("V2.10.3-rc1"), Some((2, 10, 3)));
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("x1"), None);
        assert_eq!(parse_version(""), None);
        assert!((0, 10, 0) > (0, 9, 9));
        assert_eq!(current(), parse_version(env!("CARGO_PKG_VERSION")).unwrap());
    }

    #[test]
    fn reads_a_release_and_its_installer() {
        let r = parse_release(JSON.as_bytes()).unwrap();
        assert_eq!(r.version, (0, 8, 1));
        assert_eq!(r.version_text(), "0.8.1");
        assert_eq!(
            r.msi,
            Some((
                "https://dl/RustWTM-0.8.1-x64.msi".into(),
                "https://dl/RustWTM-0.8.1-x64.msi.sha256".into()
            ))
        );
        assert!(r.is_newer_than((0, 7, 0)) && !r.is_newer_than((0, 8, 1)));
        // An installer without a checksum is not offered.
        let unsigned = JSON.replace("RustWTM-0.8.1-x64.msi.sha256", "other.sha256");
        assert_eq!(parse_release(unsigned.as_bytes()).unwrap().msi, None);
        let pre = JSON.replace(r#""prerelease": false"#, r#""prerelease": true"#);
        assert!(
            parse_release(pre.as_bytes())
                .unwrap_err()
                .contains("not a final release")
        );
        let odd = JSON.replace("v0.8.1\",", "nightly\",");
        assert!(
            parse_release(odd.as_bytes())
                .unwrap_err()
                .starts_with("unexpected tag")
        );
        assert!(
            parse_release(b"<html>")
                .unwrap_err()
                .starts_with("unexpected answer")
        );
    }

    /// The inverse of [`encode_command`], for the tests.
    fn decode_command(text: &str) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bits: Vec<u8> = text
            .bytes()
            .filter(|b| *b != b'=')
            .map(|b| ALPHABET.iter().position(|a| *a == b).unwrap() as u8)
            .collect();
        let mut bytes = Vec::new();
        for chunk in bits.chunks(4) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, b)| n | u32::from(*b) << (18 - 6 * i));
            for i in 0..chunk.len() - 1 {
                bytes.push((n >> (16 - 8 * i)) as u8);
            }
        }
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16(&units).unwrap()
    }

    #[test]
    fn commands_are_encoded_as_powershell_expects() {
        // `powershell -EncodedCommand` takes base64 of UTF-16LE.
        assert_eq!(encode_command("a"), "YQA=");
        assert_eq!(encode_command("ab"), "YQBiAA==");
        assert_eq!(encode_command("abc"), "YQBiAGMA");
        for text in ["", "x", "Get-Date; 'ç'", "{}[]\"'"] {
            assert_eq!(decode_command(&encode_command(text)), text);
        }
    }

    #[test]
    fn the_installer_is_checked_again_where_only_administrators_reach() {
        let msi = Download {
            path: PathBuf::from(r"C:\Users\o'neil\AppData\Local\Temp\RustWTM-update\a.msi"),
            sha256: [0xab; 32],
        };
        let launcher = launcher(&msi, Path::new(r"C:\Program Files\RustWTM\rustwtm.exe"));
        assert!(launcher.contains("-Verb RunAs -Wait"), "{launcher}");
        assert!(launcher.contains(
            r"finally { Start-Process -FilePath 'C:\Program Files\RustWTM\rustwtm.exe' }"
        ));
        let encoded = launcher.split('\'').nth(7).unwrap();
        let elevated = decode_command(encoded);
        assert_eq!(elevated, elevated_install(&msi));
        assert!(
            elevated.contains(r"Copy-Item -LiteralPath 'C:\Users\o''neil\AppData"),
            "{elevated}"
        );
        assert!(elevated.contains(&format!("-ne '{}'", "AB".repeat(32))));
        assert!(elevated.contains("O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"));
        // The copy is installed, never the download.
        assert!(elevated.contains("'/i', ('\"' + $msi + '\"')"));
        let args = format!("-EncodedCommand {}", encode_command(&launcher));
        assert!(args.len() < 32_000, "fits a command line: {}", args.len());

        // PowerShell itself reads both scripts, passed the way they will be.
        for script in [&launcher, &elevated] {
            let check = format!(
                "$e = $null; [void][Management.Automation.Language.Parser]::ParseInput({}, \
                 [ref]$null, [ref]$e); exit $e.Count",
                ps_quote(script)
            );
            let status = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
                .arg(encode_command(&check))
                .status()
                .unwrap();
            assert!(status.success(), "{script}");
        }
    }

    #[test]
    fn checksum_lines() {
        let line = format!("{}  RustWTM.msi\n", "ab".repeat(32));
        assert_eq!(parse_sha256(&line), Some([0xab; 32]));
        assert_eq!(parse_sha256("abc  x"), None);
        assert_eq!(parse_sha256(&"zz".repeat(32)), None);
        assert_eq!(parse_sha256(""), None);
    }

    /// Serves `bodies` by path on a local port, then stops.
    fn serve(routes: Vec<(&'static str, u16, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        std::thread::spawn(move || {
            for _ in 0..routes.len() {
                let Ok((mut s, _)) = listener.accept() else {
                    return;
                };
                let mut buf = [0u8; 2048];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let path = req.split_whitespace().nth(1).unwrap_or("").to_owned();
                let (status, body) = routes
                    .iter()
                    .find(|r| r.0 == path)
                    .map_or((404, Vec::new()), |r| (r.1, r.2.clone()));
                let _ = write!(
                    s,
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(&body);
            }
        });
        base
    }

    fn release(base: &str) -> Release {
        Release {
            version: (9, 9, 9),
            tag: "v9.9.9".into(),
            page: base.into(),
            msi: Some((format!("{base}/a.msi"), format!("{base}/a.msi.sha256"))),
        }
    }

    #[test]
    fn downloads_only_a_verified_installer() {
        let msi = b"fake installer".to_vec();
        let hash: String = wtm_sys::http::sha256(&msi)
            .unwrap()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let dir = std::env::temp_dir().join(format!("wtm-update-{}", std::process::id()));
        let base = serve(vec![
            (
                "/a.msi.sha256",
                200,
                format!("{hash}  a.msi\n").into_bytes(),
            ),
            ("/a.msi", 200, msi.clone()),
        ]);
        let got = download(&release(&base), &dir).unwrap();
        assert_eq!(got.path, dir.join("a.msi"));
        assert_eq!(got.sha256, wtm_sys::http::sha256(&msi).unwrap());
        assert_eq!(std::fs::read(&got.path).unwrap(), msi);
        std::fs::remove_file(&got.path).unwrap();

        // A tampered download is refused and not saved.
        let base = serve(vec![
            (
                "/a.msi.sha256",
                200,
                format!("{}  a.msi\n", "00".repeat(32)).into_bytes(),
            ),
            ("/a.msi", 200, msi.clone()),
        ]);
        let err = download(&release(&base), &dir).unwrap_err();
        assert!(err.starts_with("the download does not match"), "{err}");
        assert!(!dir.join("a.msi").exists());

        let base = serve(vec![("/a.msi.sha256", 200, b"garbage".to_vec())]);
        assert_eq!(
            download(&release(&base), &dir).unwrap_err(),
            "the published checksum is unreadable"
        );
        let base = serve(vec![("/a.msi.sha256", 404, Vec::new())]);
        assert!(
            download(&release(&base), &dir)
                .unwrap_err()
                .ends_with("answered HTTP 404")
        );
        let none = Release {
            msi: None,
            ..release(&base)
        };
        assert_eq!(
            download(&none, &dir).unwrap_err(),
            "this release has no installer"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Talks to GitHub; run by hand with `--ignored`.
    #[test]
    #[ignore]
    fn the_real_latest_release_has_a_verified_installer() {
        let r = latest().unwrap();
        assert!(r.version >= (0, 7, 0), "{r:?}");
        let dir = std::env::temp_dir().join(format!("wtm-update-real-{}", std::process::id()));
        let path = download(&r, &dir).unwrap().path;
        assert!(std::fs::metadata(&path).unwrap().len() > 1_000_000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checks_the_latest_release() {
        let base = serve(vec![("/latest", 200, JSON.as_bytes().to_vec())]);
        assert_eq!(
            latest_from(&format!("{base}/latest")).unwrap().version,
            (0, 8, 1)
        );
        assert!(
            latest_from("http://127.0.0.1:1/")
                .unwrap_err()
                .starts_with("cannot reach")
        );
    }

    #[test]
    fn knows_whether_it_runs_from_the_installed_folder() {
        let dir = Path::new(r"C:\Program Files\RustWTM\");
        assert!(same_dir(
            dir,
            Path::new(r"c:\program files\rustwtm\rustwtm.exe")
        ));
        assert!(!same_dir(
            dir,
            Path::new(r"D:\src\target\debug\rustwtm.exe")
        ));
        // A test binary is never the installed one.
        assert!(!installed_by_msi(&std::env::current_exe().unwrap()));
    }

    #[test]
    fn every_kind_of_single_quote_is_doubled() {
        assert_eq!(ps_quote("O'Brien"), "'O''Brien'");
        assert_eq!(
            ps_quote("C:\\Users\\O\u{2019}Brien \u{2018}\u{201A}\u{201B}"),
            "'C:\\Users\\O\u{2019}\u{2019}Brien \u{2018}\u{2018}\u{201A}\u{201A}\u{201B}\u{201B}'"
        );
        assert_eq!(ps_quote("\u{201C}x\u{201D}"), "'\u{201C}x\u{201D}'");
    }
}

//! Task Manager's "Startup impact": how much CPU and disk each program used
//! in the minutes after the user signed in. Windows writes this per user to
//! `%SystemRoot%\System32\WDI\LogFiles\StartupInfo\<SID>_StartupInfo<n>.xml`
//! (UTF-16 XML, readable by administrators only):
//!
//! ```xml
//! <StartupData IntervalStartMs="10000" IntervalEndMs="100000">
//!   <Process Name="C:\Program Files\App\app.exe" PID="2000" StartedInTraceSec="12.5">
//!     <StartTime>2026/01/01:08:00:12.5000000</StartTime>
//!     <CommandLine><![CDATA["C:\Program Files\App\app.exe" -tray]]></CommandLine>
//!     <DiskUsage Units="bytes">1000000</DiskUsage>
//!     <CpuUsage Units="us">50000</CpuUsage>
//!     <ParentPID>1000</ParentPID>
//!     <ParentStartTime>2026/01/01:08:00:00.0000000</ParentStartTime>
//!     <ParentName>explorer.exe</ParentName>
//!   </Process>
//! </StartupData>
//! ```
//!
//! A program's usage includes the processes it started during that window.
//! Only these sums leave this module: never command lines.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// CPU and disk a program used during startup.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootUsage {
    /// Processor time, in microseconds.
    pub cpu_us: u64,
    /// Bytes read and written.
    pub disk_bytes: u64,
}

/// Task Manager's rating, by Microsoft's published limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Impact {
    /// Under 300 ms of CPU and under 300 KB of disk.
    Low,
    /// 300 ms to 1 s of CPU, or 300 KB to 3 MB of disk.
    Medium,
    /// More than 1 s of CPU or more than 3 MB of disk.
    High,
}

impl Impact {
    pub fn of(u: BootUsage) -> Self {
        if u.cpu_us > 1_000_000 || u.disk_bytes > 3 * 1024 * 1024 {
            Self::High
        } else if u.cpu_us >= 300_000 || u.disk_bytes >= 300 * 1024 {
            Self::Medium
        } else {
            Self::Low
        }
    }
}

/// Largest report read.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Most processes considered, and programs returned.
const MAX_PROCESSES: usize = 4096;
const MAX_PROGRAMS: usize = 512;
/// Longest parent chain followed.
const MAX_DEPTH: usize = 64;

/// Where Windows keeps the reports.
pub fn default_dir() -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    Path::new(&root).join(r"System32\WDI\LogFiles\StartupInfo")
}

/// Whether `sid` is a plain SID string ("S-1-5-21-…"), safe to put in a
/// file name.
pub fn valid_sid(sid: &str) -> bool {
    let Some(rest) = sid.strip_prefix("S-1-") else {
        return false;
    };
    sid.len() <= 184
        && rest
            .split('-')
            .all(|g| !g.is_empty() && g.len() <= 10 && g.bytes().all(|b| b.is_ascii_digit()))
}

/// Usage per program (lower-case image path) from the newest report of the
/// user `sid` in `dir`. Empty when Windows measured nothing for that user.
pub fn read_for(dir: &Path, sid: &str) -> Result<Vec<(String, BootUsage)>, String> {
    if !valid_sid(sid) {
        return Err("Invalid account identifier".into());
    }
    let Some(path) = latest_report(dir, sid)? else {
        return Ok(Vec::new());
    };
    let len = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    if len > MAX_FILE_BYTES {
        return Err("The startup report is too large".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let text = decode(&bytes).ok_or("The startup report is not UTF-16 text")?;
    Ok(parse(&text))
}

/// The most recently written `<sid>_StartupInfo<n>.xml` in `dir`.
fn latest_report(dir: &Path, sid: &str) -> Result<Option<PathBuf>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let prefix = format!("{sid}_StartupInfo").to_ascii_lowercase();
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.take(1024).filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if !name.starts_with(&prefix) || !name.ends_with(".xml") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(at, _)| modified > *at) {
            best = Some((modified, entry.path()));
        }
    }
    Ok(best.map(|(_, p)| p))
}

/// UTF-16 LE text with its byte-order mark.
pub fn decode(bytes: &[u8]) -> Option<String> {
    let body = bytes.strip_prefix(&[0xFF, 0xFE])?;
    if body.len() % 2 != 0 || bytes.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    let units: Vec<u16> = body
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16(&units).ok()
}

/// `text` without its `<![CDATA[…]]>` sections (command lines), which may
/// hold anything, markup included. An unterminated one ends the text.
fn without_cdata(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<![CDATA[") {
        out.push_str(&rest[..start]);
        match rest[start..].find("]]>") {
            Some(end) => rest = &rest[start + end + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// XML's predefined entities.
fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The value of attribute `name` in a start tag.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    let end = tag[start..].find('"')?;
    Some(unescape(&tag[start..start + end]))
}

/// The start tag and text of the first `<name …>text</name>` in `body`.
fn element<'a>(body: &'a str, name: &str) -> Option<(&'a str, &'a str)> {
    let open = body.find(&format!("<{name}"))?;
    let tag_end = open + body[open..].find('>')?;
    let close = tag_end + body[tag_end..].find(&format!("</{name}>"))?;
    Some((&body[open..tag_end], body[tag_end + 1..close].trim()))
}

fn number(body: &str, name: &str) -> Option<(u64, Option<String>)> {
    let (tag, text) = element(body, name)?;
    Some((text.parse().ok()?, attribute(tag, "Units")))
}

struct Process {
    path: String,
    id: (u32, String),
    parent: Option<(u32, String)>,
    usage: BootUsage,
}

/// Usage per program in a report, children added to the program that
/// started them; sorted by path.
pub fn parse(text: &str) -> Vec<(String, BootUsage)> {
    let text = without_cdata(text);
    let mut procs = Vec::new();
    let mut rest = text.as_str();
    while procs.len() < MAX_PROCESSES {
        let Some(open) = rest.find("<Process ") else {
            break;
        };
        rest = &rest[open..];
        let Some(tag_end) = rest.find('>') else {
            break;
        };
        let Some(close) = rest.find("</Process>") else {
            break;
        };
        let (tag, body) = (&rest[..tag_end], &rest[tag_end.min(close)..close]);
        rest = &rest[close + "</Process>".len()..];
        let (Some(path), Some(pid)) = (
            attribute(tag, "Name"),
            attribute(tag, "PID").and_then(|p| p.parse::<u32>().ok()),
        ) else {
            continue;
        };
        let text_of = |name| element(body, name).map(|(_, t)| t.to_owned());
        let cpu_us = match number(body, "CpuUsage") {
            Some((v, Some(u))) if u == "ms" => v.saturating_mul(1000),
            Some((v, _)) => v,
            None => 0,
        };
        let disk_bytes = number(body, "DiskUsage").map_or(0, |(v, _)| v);
        let parent = text_of("ParentPID")
            .and_then(|p| p.parse::<u32>().ok())
            .map(|pid| (pid, text_of("ParentStartTime").unwrap_or_default()));
        procs.push(Process {
            path: path.to_lowercase(),
            id: (pid, text_of("StartTime").unwrap_or_default()),
            parent,
            usage: BootUsage { cpu_us, disk_bytes },
        });
    }
    // A child counts toward the topmost of its ancestors in the report.
    let index: HashMap<&(u32, String), usize> =
        procs.iter().enumerate().map(|(i, p)| (&p.id, i)).collect();
    let mut by_program: HashMap<&str, BootUsage> = HashMap::new();
    for p in &procs {
        let mut root = p;
        for _ in 0..MAX_DEPTH {
            match root.parent.as_ref().and_then(|k| index.get(k)) {
                Some(&i) if !std::ptr::eq(&procs[i], root) => root = &procs[i],
                _ => break,
            }
        }
        let sum = by_program.entry(root.path.as_str()).or_default();
        sum.cpu_us = sum.cpu_us.saturating_add(p.usage.cpu_us);
        sum.disk_bytes = sum.disk_bytes.saturating_add(p.usage.disk_bytes);
    }
    let mut out: Vec<(String, BootUsage)> = by_program
        .into_iter()
        .map(|(path, usage)| (path.to_owned(), usage))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.truncate(MAX_PROGRAMS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(
        name: &str,
        pid: u32,
        start: &str,
        parent: (u32, &str),
        cpu: u64,
        disk: u64,
    ) -> String {
        format!(
            "\t<Process Name=\"{name}\" PID=\"{pid}\" StartedInTraceSec=\"1.0\">\r\n\
             \t\t<StartTime>{start}</StartTime>\r\n\
             \t\t<CommandLine><![CDATA[\"{name}\" --x </Process> <Process Name=\"evil\" PID=\"1\">]]></CommandLine>\r\n\
             \t\t<DiskUsage Units=\"bytes\">{disk}</DiskUsage>\r\n\
             \t\t<CpuUsage Units=\"us\">{cpu}</CpuUsage>\r\n\
             \t\t<ParentPID>{}</ParentPID>\r\n\
             \t\t<ParentStartTime>{}</ParentStartTime>\r\n\
             \t\t<ParentName>explorer.exe</ParentName>\r\n\
             \t</Process>\r\n",
            parent.0, parent.1
        )
    }

    fn report(processes: &[String]) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\r\n<StartupData IntervalStartMs=\"1\" IntervalEndMs=\"2\">\r\n{}</StartupData>\r\n",
            processes.concat()
        )
    }

    fn utf16(text: &str) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        bytes
    }

    const EXPLORER: (u32, &str) = (100, "2026/11/04:14:38:02.0");

    #[test]
    fn children_count_toward_the_program_that_started_them() {
        let text = report(&[
            process(r"C:\Apps\Tray.exe", 200, "t1", EXPLORER, 500_000, 1_000),
            process(
                r"C:\Apps\Agent.exe",
                300,
                "t2",
                (200, "t1"),
                600_000,
                4_000_000,
            ),
            process(r"C:\Apps\Crash.exe", 400, "t3", (300, "t2"), 1, 1),
            // PID 200 reused later by an unrelated program: its child is
            // not Tray's.
            process(r"C:\Other\Late.exe", 500, "t4", (200, "t9"), 7, 7),
            process(
                r"C:\Program Files\A &amp; B\ab.exe",
                600,
                "t5",
                EXPLORER,
                10,
                20,
            ),
        ]);
        let found: HashMap<String, BootUsage> = parse(&text).into_iter().collect();
        assert_eq!(
            found[r"c:\apps\tray.exe"],
            BootUsage {
                cpu_us: 1_100_001,
                disk_bytes: 4_001_001
            }
        );
        assert_eq!(found[r"c:\other\late.exe"].cpu_us, 7);
        assert!(
            found.contains_key(r"c:\program files\a & b\ab.exe"),
            "entities"
        );
        // Markup inside command lines is never read as a process.
        assert!(!found.contains_key("evil"));
        assert_eq!(found.len(), 3);
        assert_eq!(Impact::of(found[r"c:\apps\tray.exe"]), Impact::High);
    }

    #[test]
    fn ratings_follow_microsofts_limits() {
        let u = |cpu_us, disk_bytes| Impact::of(BootUsage { cpu_us, disk_bytes });
        assert_eq!(u(299_999, 300 * 1024 - 1), Impact::Low);
        assert_eq!(u(300_000, 0), Impact::Medium);
        assert_eq!(u(0, 300 * 1024), Impact::Medium);
        assert_eq!(u(1_000_000, 3 * 1024 * 1024), Impact::Medium);
        assert_eq!(u(1_000_001, 0), Impact::High);
        assert_eq!(u(0, 3 * 1024 * 1024 + 1), Impact::High);
    }

    #[test]
    fn decodes_only_utf16_with_its_mark() {
        assert_eq!(decode(&utf16("<a/>")).as_deref(), Some("<a/>"));
        assert_eq!(decode(b"<a/>"), None);
        let mut odd = utf16("ab");
        odd.push(0);
        assert_eq!(decode(&odd), None);
    }

    #[test]
    fn sids_are_checked_before_they_name_a_file() {
        assert!(valid_sid("S-1-5-21-3390786495-1479563812-1918021463-1001"));
        for bad in [
            "",
            "S-1-",
            "S-1-5-..\\x",
            "S-1-5-21-a",
            "S-1-5--1",
            r"..\S-1-5",
        ] {
            assert!(!valid_sid(bad), "{bad:?}");
        }
    }

    #[test]
    fn reads_the_newest_report_of_that_user_only() {
        let dir = std::env::temp_dir().join(format!("wtm-startup-impact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let me = "S-1-5-21-1-2-3-1001";
        let other = "S-1-5-21-1-2-3-1002";
        let write = |name: &str, text: &str| std::fs::write(dir.join(name), utf16(text)).unwrap();
        write(
            &format!("{me}_StartupInfo1.xml"),
            &report(&[process(r"C:\Old.exe", 1, "a", EXPLORER, 1, 1)]),
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        write(
            &format!("{me}_StartupInfo2.xml"),
            &report(&[process(r"C:\New.exe", 1, "a", EXPLORER, 1, 1)]),
        );
        write(
            &format!("{other}_StartupInfo1.xml"),
            &report(&[process(r"C:\Theirs.exe", 1, "a", EXPLORER, 1, 1)]),
        );
        let mine = read_for(&dir, me).unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].0, r"c:\new.exe");
        // A report without processes: nothing measured.
        write(&format!("{other}_StartupInfo2.xml"), &report(&[]));
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(
            dir.join(format!("{other}_StartupInfo3.xml")),
            utf16(&report(&[])),
        )
        .unwrap();
        assert!(read_for(&dir, other).unwrap().is_empty());
        // No report at all, or no folder: nothing measured.
        assert!(read_for(&dir, "S-1-5-21-9").unwrap().is_empty());
        assert!(read_for(&dir.join("missing"), me).unwrap().is_empty());
        assert!(read_for(&dir, "..").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

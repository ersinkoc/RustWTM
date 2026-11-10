//! Bounded, non-interactive system queries. Scripts are compiled-in, never user code.
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const MAX_REPLY: usize = 1024 * 1024;
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
struct Slot;
impl Drop for Slot {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) fn json(
    script: &str,
    input: &serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let input = serde_json::to_vec(input).map_err(|e| e.to_string())?;
    if input.len() > 64 * 1024 {
        return Err("Query input exceeds its limit".into());
    }
    ACTIVE
        .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            (n < 2).then_some(n + 1)
        })
        .map_err(|_| "Another system query is busy; retry shortly".to_owned())?;
    let _slot = Slot;
    let exe = wtm_sys::shell::system_directory()
        .map_err(|e| e.to_string())?
        .join("WindowsPowerShell/v1.0/powershell.exe");
    wtm_sys::security::only_admins_control(&exe).map_err(|e| e.to_string())?;
    let script = format!(
        "[Console]::InputEncoding = New-Object System.Text.UTF8Encoding($false);\n$env:PSModulePath = (Join-Path $PSHOME 'Modules') + ';' + (Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'WindowsPowerShell/Modules');\n{script}"
    );
    let mut child = Command::new(exe)
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    if let Err(e) = std::thread::Builder::new()
        .name("system-query-output".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout
                .take(MAX_REPLY as u64)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = tx.send(result);
        })
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e.to_string());
    }
    let mut stdin = child.stdin.take().expect("piped stdin");
    if let Err(e) = std::thread::Builder::new()
        .name("system-query-input".into())
        .spawn(move || {
            let _ = stdin.write_all(&input);
        })
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e.to_string());
    }
    let start = Instant::now();
    let result = loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(bytes)) if bytes.len() < MAX_REPLY => {
                break serde_json::from_slice(&bytes)
                    .map_err(|e| format!("Invalid system query response: {e}"));
            }
            Ok(Ok(_)) => break Err("System query response exceeds its limit".into()),
            Ok(Err(e)) => break Err(e.to_string()),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                break Err("System query stopped".into());
            }
            Err(_) if start.elapsed() >= timeout => break Err("System query timed out".into()),
            Err(_) => {}
        }
    };
    // A query may close stdout without exiting; never leave its process behind.
    for _ in 0..10 {
        if !matches!(child.try_wait(), Ok(None)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if matches!(child.try_wait(), Ok(None)) {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() && result.is_ok() {
        return Err(format!("System query exited with {status}"));
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn query_input_roundtrips_unicode_without_using_the_console_code_page() {
        let value = serde_json::json!({"version":"1.0-β","label":"Türkçe 測試"});
        let output=super::json("[Console]::OutputEncoding=New-Object System.Text.UTF8Encoding($false); [Console]::In.ReadToEnd() | ConvertFrom-Json | ConvertTo-Json -Compress",&value,std::time::Duration::from_secs(10)).unwrap();
        assert_eq!(output, value);
        assert!(
            super::json(
                "throw 'must not run'",
                &serde_json::json!("x".repeat(70_000)),
                std::time::Duration::from_secs(1)
            )
            .unwrap_err()
            .contains("input exceeds")
        );
    }
}

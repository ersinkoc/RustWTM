//! Per-user administrator preference and fixed, protected GUI launch tasks.
//! Never stores passwords or registers an arbitrary executable/argument list.
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

const KEY: &str = r"Software\RustWTM";
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const MARKER: &str = "RustWTM administrator launch v1";
pub const SCHEDULED_OPEN: &str = "--scheduled-open";
const SCHEDULED_BACKGROUND: &str = "--scheduled-open --background";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Settings {
    pub administrator: bool,
    pub autostart: bool,
}
#[derive(Debug, Clone, Copy)]
pub enum Change {
    Initialize { first_run: bool },
    Set(Settings),
}

pub fn settings() -> Settings {
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey(KEY).ok();
    let flag = |name| key.as_ref().and_then(|k| k.get_value::<u32, _>(name).ok()) == Some(1);
    let regular = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(RUN)
        .and_then(|k| k.get_value::<String, _>("RustWTM"))
        .is_ok();
    Settings {
        administrator: flag("PreferAdministrator"),
        autostart: flag("AdministratorLogon") || regular,
    }
}

fn flag(name: &str, value: bool) -> Result<(), String> {
    RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(KEY)
        .and_then(|(k, _)| k.set_value(name, &u32::from(value)))
        .map_err(|e| e.to_string())
}

fn regular_start(exe: &Path, on: bool) -> Result<(), String> {
    regular_start_in(RUN, exe, on)
}

fn regular_start_in(registry_path: &str, exe: &Path, on: bool) -> Result<(), String> {
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(registry_path)
        .map_err(|e| e.to_string())?;
    if on {
        key.set_value("RustWTM", &format!("\"{}\" --background", exe.display()))
            .map_err(|e| e.to_string())
    } else {
        match key.delete_value("RustWTM") {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r.map_err(|e| e.to_string()),
        }
    }
}

fn gui_path() -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(current
        .parent()
        .ok_or("Missing program directory")?
        .join("rustwtm.exe"))
}

fn protected_gui() -> Result<PathBuf, String> {
    let exe = gui_path()?;
    check_program(&exe)?;
    Ok(exe)
}

fn check_program(exe: &Path) -> Result<(), String> {
    if !wtm_sys::security::only_admins_control(exe).map_err(|e| e.to_string())? {
        return Err("Automatic administrator startup requires RustWTM in an administrator-protected installation folder".into());
    }
    Ok(())
}

fn run(script: &str) -> Result<(), String> {
    let exe = wtm_sys::shell::system_directory()
        .map_err(|e| e.to_string())?
        .join("WindowsPowerShell/v1.0/powershell.exe");
    if !wtm_sys::security::only_admins_control(&exe).map_err(|e| e.to_string())? {
        return Err("Untrusted Windows PowerShell path".into());
    }
    let mut child = Command::new(exe)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-OutputFormat",
            "Text",
            "-EncodedCommand",
            &crate::update::encode_command(script),
        ])
        .creation_flags(0x0800_0000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(if cfg!(test) {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .spawn()
        .map_err(|e| e.to_string())?;
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("Windows Task Scheduler request failed ({status})"))
                };
            }
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(25)),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match other {
                    Err(e) => e.to_string(),
                    _ => "Windows Task Scheduler request timed out".into(),
                });
            }
        }
    }
}

fn script(exe: &Path, operation: &str) -> String {
    format!(
        r#"$ErrorActionPreference='Stop'
$exe={exe}
$marker='{MARKER}'
$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$service=New-Object -ComObject Schedule.Service
$service.Connect()
$folder=$service.GetFolder('\')
function UserSid($value) {{
    if($value -match '^S-[0-9]-') {{return [System.Security.Principal.SecurityIdentifier]::new($value).Value}}
    return [System.Security.Principal.NTAccount]::new($value).Translate([System.Security.Principal.SecurityIdentifier]).Value
}}
function Owned($name,$arguments) {{
    try {{ $task=$folder.GetTask($name) }} catch {{
        $e=$_.Exception; while($e.InnerException){{$e=$e.InnerException}}
        if($e.HResult -ne -2147024894){{throw}}
        return $null
    }}
    $d=$task.Definition
    if($d.RegistrationInfo.Description -cne $marker -or (UserSid $d.Principal.UserId) -cne $sid -or $d.Principal.LogonType -ne 3 -or $d.Principal.RunLevel -ne 1 -or $d.Actions.Count -ne 1 -or $d.Actions.Item(1).Path -ine $exe -or $d.Actions.Item(1).Arguments -cne $arguments) {{throw 'Existing task is not this RustWTM launch task'}}
    $security=[System.Security.AccessControl.RawSecurityDescriptor]::new($task.GetSecurityDescriptor(7))
    if($security.Owner.Value -cnotin @('S-1-5-32-544','S-1-5-18') -or -not ($security.ControlFlags -band 4096)) {{throw 'Launch task is not administrator-owned and protected'}}
    foreach($ace in $security.DiscretionaryAcl) {{
        $who=$ace.SecurityIdentifier.Value
        if($ace.AceType -ne 0 -or $who -cnotin @('S-1-5-32-544','S-1-5-18',$sid)) {{throw 'Unexpected launch-task access rule'}}
        $mask=([long]$ace.AccessMask -band 0xffffffffL)
        if($who -ceq $sid -and ($mask -band (-bnot 0xa01200a9L))) {{throw 'Launch task is writable by the ordinary user'}}
    }}
    return $task
}}
function SetTask($kind,$arguments,$enabled,$logon) {{
    if($sid -in @('S-1-5-18','S-1-5-19','S-1-5-20')) {{throw 'An interactive user is required'}}
    $name='RustWTM-Administrator-'+$kind+'-'+$sid
    $existing=Owned $name $arguments
    if(-not $enabled) {{if($existing){{$folder.DeleteTask($name,0)}};return}}
    $d=$service.NewTask(0)
    $d.RegistrationInfo.Author='RustWTM';$d.RegistrationInfo.Description=$marker
    $d.Principal.UserId=$sid;$d.Principal.LogonType=3;$d.Principal.RunLevel=1
    $d.Settings.Enabled=$true;$d.Settings.AllowDemandStart=$true
    $d.Settings.MultipleInstances=0;$d.Settings.ExecutionTimeLimit='PT0S'
    $d.Settings.DisallowStartIfOnBatteries=$false;$d.Settings.StopIfGoingOnBatteries=$false
    if($logon){{$t=$d.Triggers.Create(9);$t.UserId=$sid}}
    $a=$d.Actions.Create(0);$a.Path=$exe;$a.Arguments=$arguments
    $acl='O:BAG:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGX;;;'+$sid+')'
    $folder.RegisterTaskDefinition($name,$d,22,$sid,$null,3,$acl) | Out-Null
}}
{operation}
"#,
        exe = crate::update::ps_quote(&exe.to_string_lossy())
    )
}

pub fn configure(change: Change) -> Result<Settings, String> {
    let old = settings();
    let elevated = crate::launch::is_elevated();
    if matches!(change, Change::Initialize { first_run: false }) && !elevated {
        return Ok(old);
    }
    let desired = match change {
        Change::Initialize { first_run } => Settings {
            administrator: old.administrator || elevated,
            autostart: old.autostart || first_run,
        },
        Change::Set(value) => value,
    };
    if !elevated && (desired.administrator || old.administrator) {
        return if matches!(change, Change::Initialize { .. }) {
            Ok(old)
        } else {
            Err("Restart RustWTM as administrator to change administrator startup".into())
        };
    }
    // The remembered preference is safe even for a portable build: it can only
    // request ordinary UAC. Persistent highest-privilege tasks require the ACL guard.
    if desired.administrator {
        flag("PreferAdministrator", true)?;
    }
    if desired.administrator || old.administrator {
        let exe = if desired.administrator {
            protected_gui()?
        } else {
            gui_path()?
        };
        let op = format!(
            "SetTask 'Open' '{SCHEDULED_OPEN}' ${} $false\nSetTask 'Logon' '{SCHEDULED_BACKGROUND}' ${} $true",
            desired.administrator,
            desired.administrator && desired.autostart
        );
        run(&script(&exe, &op))?;
    }
    let exe = gui_path()?;
    regular_start(&exe, desired.autostart && !desired.administrator)?;
    flag(
        "AdministratorLogon",
        desired.administrator && desired.autostart,
    )?;
    if !desired.administrator {
        flag("PreferAdministrator", false)?;
    }
    Ok(settings())
}

/// Uses only the pre-registered fixed task; failed validation falls back to UAC
/// at the caller, never to an arbitrary scheduled command.
pub fn launch_task(background: bool) -> Result<(), String> {
    if !settings().administrator {
        return Err("Administrator preference is off".into());
    }
    let exe = protected_gui()?;
    let (kind, args) = if background {
        ("Logon", SCHEDULED_BACKGROUND)
    } else {
        ("Open", SCHEDULED_OPEN)
    };
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(KEY)
        .map_err(|e| e.to_string())?;
    key.set_value("AdministratorLauncher", &std::process::id())
        .map_err(|e| e.to_string())?;
    run(&script(
        &exe,
        &format!(
            "$task=Owned ('RustWTM-Administrator-{kind}-'+$sid) '{args}';if(-not $task){{throw 'Launch task is missing'}};$task.Run($null) | Out-Null"
        ),
    ))
}

pub fn wait_launcher() {
    let pid = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(KEY, winreg::enums::KEY_READ | winreg::enums::KEY_WRITE)
        .map(|key| {
            let pid = key
                .get_value::<u32, _>("AdministratorLauncher")
                .unwrap_or(0);
            let _ = key.delete_value("AdministratorLauncher");
            pid
        })
        .unwrap_or(0);
    if pid > 4 && pid != std::process::id() {
        crate::launch::wait_for_exit(pid, Duration::from_secs(10));
    }
}

/// Uninstall removes only this product's fixed task definitions, for all users.
pub fn cleanup_tasks(exe: &Path) -> Result<(), String> {
    let operation = format!(
        r#"foreach($task in @($folder.GetTasks(1))) {{
        $d=$task.Definition
        if($d.RegistrationInfo.Description -cne $marker) {{continue}}
        try {{$owner=UserSid $d.Principal.UserId}} catch {{continue}}
        if($task.Name -cin @(('RustWTM-Administrator-Open-'+$owner),('RustWTM-Administrator-Logon-'+$owner)) -and $d.Principal.LogonType -eq 3 -and $d.Principal.RunLevel -eq 1 -and $d.Actions.Count -eq 1 -and $d.Actions.Item(1).Path -ieq $exe -and $d.Actions.Item(1).Arguments -cin @('{SCHEDULED_OPEN}','{SCHEDULED_BACKGROUND}')) {{$folder.DeleteTask($task.Name,0)}}
    }}"#
    );
    run(&script(exe, &operation))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn autostart_entry_is_quoted_reversible_and_idempotent() {
        let path = format!(r"Software\RustWTM-test-gui-startup-{}", std::process::id());
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let exe = Path::new(r"C:\Program Files\RustWTM\rustwtm.exe");
        regular_start_in(&path, exe, true).unwrap();
        let value: String = root
            .open_subkey(&path)
            .unwrap()
            .get_value("RustWTM")
            .unwrap();
        assert_eq!(value, format!("\"{}\" --background", exe.display()));
        regular_start_in(&path, exe, false).unwrap();
        assert!(
            root.open_subkey(&path)
                .unwrap()
                .get_value::<String, _>("RustWTM")
                .is_err()
        );
        regular_start_in(&path, exe, false).unwrap();
        root.delete_subkey_all(path).unwrap();
    }
    #[test]
    fn writable_development_copy_cannot_be_used_for_privileged_tasks() {
        let exe = std::env::current_exe().unwrap();
        assert!(!wtm_sys::security::only_admins_control(&exe).unwrap());
        assert!(
            check_program(&exe)
                .unwrap_err()
                .contains("administrator-protected")
        );
        let control = wtm_sys::shell::system_directory()
            .unwrap()
            .join("WindowsPowerShell/v1.0/powershell.exe");
        assert!(check_program(&control).is_ok());
    }
    #[test]
    fn generated_task_script_parses_and_keeps_the_path_literal() {
        let path = Path::new("C:\\Program Files\\it's ’ tricky\\rustwtm.exe");
        let s = script(path, "SetTask 'Open' '--scheduled-open' $true $false");
        let proof = format!(
            r#"
$source=[Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('{source}'))
$expected=[Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('{path}'))
$errors=$null
$ast=[System.Management.Automation.Language.Parser]::ParseInput($source,[ref]$null,[ref]$errors)
if($errors.Count){{throw 'Generated task script does not parse'}}
$assignment=@($ast.FindAll({{param($n) $n -is [System.Management.Automation.Language.AssignmentStatementAst] -and $n.Left.VariablePath.UserPath -ceq 'exe'}},$true))
if($assignment.Count -ne 1){{throw 'Executable assignment is missing or duplicated'}}
$literal=$assignment[0].Right.Expression
if($literal -isnot [System.Management.Automation.Language.StringConstantExpressionAst] -or $literal.Value -cne $expected){{throw 'Executable path was not preserved as one literal'}}
$normalize=$ast.Find({{param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -ceq 'UserSid'}},$true)
Invoke-Expression $normalize.Extent.Text
$identity=[System.Security.Principal.WindowsIdentity]::GetCurrent()
foreach($value in @($identity.User.Value,$identity.Name)) {{
    if((UserSid $value) -cne $identity.User.Value) {{throw 'Task principal account name and SID did not resolve to the same user'}}
}}
"#,
            source = crate::update::encode_command(&s),
            path = crate::update::encode_command(&path.to_string_lossy())
        );
        run(&proof).unwrap();
    }
}

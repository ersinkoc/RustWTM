<#
.SYNOPSIS
    CI only: installs RustWTM for real (MSI or install script), verifies the
    installation and the running service, then uninstalls and verifies the
    cleanup. Requires an elevated, disposable machine.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('msi', 'script')][string]$Mode,
    [string]$Msi
)

$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -cne 'true' -or $env:RUNNER_OS -cne 'Windows') {
    throw 'This machine-changing verification runs only on a disposable GitHub Windows runner.'
}
$InstallDir = Join-Path $env:ProgramFiles 'RustWTM'
$DataDir = Join-Path $env:ProgramData 'RustWTM'

function Assert([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "ASSERTION FAILED: $Message" }
    Write-Host "ok - $Message" -ForegroundColor Green
}

function Wait-Until([scriptblock]$Condition, [string]$What, [int]$Seconds = 60) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while (-not (& $Condition)) {
        if ((Get-Date) -gt $deadline) { throw "Timed out waiting for $What" }
        Start-Sleep -Milliseconds 500
    }
}

# --- install -----------------------------------------------------------------
if ($Mode -eq 'msi') {
    $p = Start-Process msiexec.exe -ArgumentList '/i', "`"$((Resolve-Path $Msi).Path)`"", '/qn', '/l*v', 'install.log' -Wait -PassThru
    Assert ($p.ExitCode -eq 0) "msiexec /i exited with 0 (got $($p.ExitCode))"
} else {
    & (Join-Path $PSScriptRoot '..\install.ps1') -FromSource (Join-Path $PSScriptRoot '..\..')
}

# --- verify ------------------------------------------------------------------
foreach ($exe in 'rustwtm.exe', 'wtm.exe', 'wtm-service.exe') {
    Assert (Test-Path (Join-Path $InstallDir $exe)) "$exe installed"
}
Assert (Test-Path (Join-Path $InstallDir 'hardware/wtm-hardware.exe')) 'hardware provider installed'
Assert (Test-Path (Join-Path $InstallDir 'hardware/PROVIDER-NOTICES.txt')) 'hardware provider license notices installed'
Assert (Test-Path (Join-Path $InstallDir 'docs/hardware.md')) 'hardware guide installed'
Assert (Test-Path (Join-Path $InstallDir 'docs/cpu-controls.md')) 'CPU topology and power guide installed'
Assert (Test-Path (Join-Path $InstallDir 'docs/localization.md')) 'Language support guide installed'
Assert (Test-Path (Join-Path $InstallDir 'docs/docker.md')) 'Docker monitoring guide installed'
Assert (Test-Path (Join-Path $InstallDir 'docs/task-manager.md')) 'Task Manager integration guide installed'
if ($Mode -eq 'script') {
    # Only SYSTEM and administrators may change what the service runs.
    $sddl = (Get-Acl -LiteralPath $InstallDir).Sddl
    Assert ($sddl -match '^O:(BA|SY).*D:PA?I?\(A;OICI;FA;;;SY\)\(A;OICI;FA;;;BA\)\(A;OICI;0x1200a9;;;BU\)$') "install dir is admin-only ($sddl)"
}
$machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine') -split ';'
Assert ($machinePath -contains $InstallDir -or $machinePath -contains "$InstallDir\") 'install dir is on the machine PATH'
$shortcut = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'RustWTM.lnk'
Assert (Test-Path $shortcut) 'Start Menu shortcut exists'

Wait-Until { (Get-Service RustWTM -ErrorAction SilentlyContinue).Status -eq 'Running' } 'service to run'
Assert $true 'service is running'
$svc = Get-CimInstance Win32_Service -Filter "Name='RustWTM'"
Assert ($svc.StartMode -eq 'Auto') 'service starts automatically'
Assert ($svc.StartName -eq 'LocalSystem') 'service runs as LocalSystem'
$recovery = sc.exe qfailure RustWTM | Out-String
Assert ($recovery -match 'RESTART') 'service restarts on failure'

Wait-Until { Test-Path (Join-Path $DataDir 'config.toml') } 'default config'
Assert ((Get-Content (Join-Path $DataDir 'config.toml') -Raw) -match 'dry_run = true') 'default config is dry-run'
$acl = (Get-Acl $DataDir)
Assert ($acl.AreAccessRulesProtected) 'data directory does not inherit permissions'
$writers = $acl.Access | Where-Object {
    $_.FileSystemRights -match 'FullControl|Modify|Write' -and $_.AccessControlType -eq 'Allow'
} | ForEach-Object { $_.IdentityReference.Value }
Assert (-not ($writers | Where-Object { $_ -match 'Users$|Everyone' })) "standard users cannot write the data directory ($($writers -join ', '))"

# The CLI talks to the real (verified) service over the pipe.
$wtm = Join-Path $InstallDir 'wtm.exe'
& (Join-Path $PSScriptRoot 'enable-software-opengl.ps1') -InstallDir $InstallDir
& $wtm task-manager --enable
Assert ($LASTEXITCODE -eq 0) 'Task Manager replacement enabled from the protected installation'
$taskManagerKey = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe'
Assert ((Get-ItemPropertyValue $taskManagerKey -Name Debugger) -ceq "`"$InstallDir\rustwtm.exe`" --task-manager") 'Ctrl+Shift+Esc replacement points to the fixed GUI command'
# Exercise Windows' real IFEO launch on this disposable VM. Forwarded taskmgr
# arguments must not hide the replacement or create a second GUI instance.
# This disposable profile reproduces a stale saved window that previously
# opened at 64x64 and caused negative-width layout panics on native startup.
$guiProfile = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'RustWTM/data/app.ron'
Assert (-not (Test-Path -LiteralPath $guiProfile)) 'GUI geometry fixture owns a clean profile'
New-Item -ItemType Directory -Path (Split-Path $guiProfile -Parent) -Force | Out-Null
@'
{
    "window": "(normal_geometry:(outer_position_pixels:Some((x:0.0,y:0.0)),scale_factor:1.0,inner_size_points:(x:64.0,y:64.0)),fullscreen:false,maximized:false)",
}
'@ | Set-Content -LiteralPath $guiProfile -Encoding utf8NoBOM
$taskmgr = Join-Path $env:WINDIR 'System32/taskmgr.exe'
$launch = Start-Process -FilePath $taskmgr -ArgumentList '--background' -PassThru `
    -RedirectStandardOutput task-manager-launch.stdout.log -RedirectStandardError task-manager-launch.stderr.log
try {
    Wait-Until {
        @(Get-Process rustwtm -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 }).Count -eq 1
    } 'visible replacement GUI after launching Windows Task Manager' 30
} catch {
    $launch.Refresh()
    Write-Host "Task Manager launch: pid=$($launch.Id), exited=$($launch.HasExited), session=$((Get-Process -Id $PID).SessionId), interactive=$([Environment]::UserInteractive)"
    if ($launch.HasExited) { Write-Host "Launch exit code: $($launch.ExitCode)" }
    foreach ($log in 'task-manager-launch.stdout.log','task-manager-launch.stderr.log') {
        if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log | Write-Host }
    }
    Get-CimInstance Win32_Process -Filter "Name='rustwtm.exe' OR Name='taskmgr.exe'" |
        Select-Object ProcessId, ParentProcessId, SessionId, ExecutablePath, CommandLine | Format-List | Out-Host
    throw
}
$gui = @(Get-Process rustwtm -ErrorAction SilentlyContinue)
Assert ($gui.Count -eq 1 -and $gui[0].Path -ieq (Join-Path $InstallDir 'rustwtm.exe')) 'Windows launched the installed replacement visibly'
Add-Type @'
using System;
using System.Runtime.InteropServices;
[StructLayout(LayoutKind.Sequential)]
public struct RustWtmClientRect { public int Left, Top, Right, Bottom; }
public static class RustWtmNativeGeometry {
    [DllImport("user32.dll", SetLastError=true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetClientRect(IntPtr window, out RustWtmClientRect rect);
}
'@
$client = [RustWtmClientRect]::new()
Assert ([RustWtmNativeGeometry]::GetClientRect($gui[0].MainWindowHandle, [ref]$client)) 'read native GUI client bounds'
Assert (($client.Right - $client.Left) -ge 820 -and ($client.Bottom - $client.Top) -ge 500) 'stale 64x64 window restored to at least 820x500'
$secondLaunch = Start-Process -FilePath $taskmgr -PassThru
Assert ($secondLaunch.WaitForExit(30000)) 'second launch exits after handing off to the original GUI'
$reopened = @(Get-Process rustwtm -ErrorAction SilentlyContinue)
Assert ($reopened.Count -eq 1 -and $reopened[0].Id -eq $gui[0].Id) 'repeated Task Manager launch reuses the original GUI'
# End only the owned disposable-test GUI before the installer lifecycle checks.
Stop-Process -Id $gui[0].Id -Force
Wait-Until { -not (Get-Process -Id $gui[0].Id -ErrorAction SilentlyContinue) } 'test GUI exit'
& $wtm task-manager --restore
Assert ($LASTEXITCODE -eq 0) 'Windows Task Manager restored explicitly'
Assert ($null -eq (Get-ItemProperty $taskManagerKey -Name Debugger -ErrorAction SilentlyContinue)) 'owned debugger removed on restore'
& $wtm task-manager --enable
Assert ($LASTEXITCODE -eq 0) 'replacement reenabled for uninstall cleanup verification'
Wait-Until { & $wtm history --hours 1 -n 3 *> $null; $LASTEXITCODE -eq 0 } 'wtm history via the service' 30
Assert $true 'wtm history reaches the service'

# What the service leaves outside its files: a blocked program and a firewall
# rule, as RustWTM makes them. Uninstalling must remove both.
$ifeo = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\rustwtm-cleanup-test.exe'
New-Item -Path $ifeo -Force | Out-Null
New-ItemProperty -Path $ifeo -Name Debugger -Value "`"$InstallDir\rustwtm.exe`" --blocked" | Out-Null
New-ItemProperty -Path $ifeo -Name RustWTMBlock -Value 1 -PropertyType DWord | Out-Null
New-NetFirewallRule -DisplayName 'RustWTM: block outbound for rustwtm-cleanup-test.exe' -Group 'RustWTM' `
    -Direction Outbound -Action Block -Program 'C:\rustwtm-cleanup-test.exe' | Out-Null
Assert ([bool](Get-NetFirewallRule -Group 'RustWTM' -ErrorAction SilentlyContinue)) 'test firewall rule added'

# --- uninstall ---------------------------------------------------------------
if ($Mode -eq 'msi') {
    $p = Start-Process msiexec.exe -ArgumentList '/x', "`"$((Resolve-Path $Msi).Path)`"", '/qn', '/l*v', 'uninstall.log' -Wait -PassThru
    Assert ($p.ExitCode -eq 0) "msiexec /x exited with 0 (got $($p.ExitCode))"
} else {
    & (Join-Path $InstallDir 'uninstall.ps1')
}

Wait-Until { -not (Get-Service RustWTM -ErrorAction SilentlyContinue) } 'service removal'
Assert $true 'service removed'
Assert (-not (Test-Path (Join-Path $InstallDir 'wtm-service.exe'))) 'binaries removed'
Assert (-not (Test-Path $shortcut)) 'shortcut removed'
Assert ($null -eq (Get-ItemProperty $taskManagerKey -Name Debugger -ErrorAction SilentlyContinue)) 'Windows Task Manager restored on uninstall'
$machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine') -split ';'
Assert (-not ($machinePath -contains $InstallDir)) 'PATH entry removed'
Assert (Test-Path (Join-Path $DataDir 'config.toml')) 'configuration kept after uninstall'
Assert (-not (Test-Path $ifeo)) 'blocked-program entries removed'
Assert (-not (Get-NetFirewallRule -Group 'RustWTM' -ErrorAction SilentlyContinue)) 'RustWTM firewall rules removed'
Write-Host "Install / uninstall verification passed ($Mode)." -ForegroundColor Green

<#
.SYNOPSIS
    Removes RustWTM.

.DESCRIPTION
    Stops and removes the RustWTM service (which rolls back every limit it
    applied) along with its blocked-program entries and firewall rules,
    deletes the Start Menu shortcut, removes the install directory from the
    machine PATH and deletes it. Configuration and usage history in
    %ProgramData%\RustWTM are kept unless -RemoveData is given.

.PARAMETER InstallDir
    Directory RustWTM was installed to. Default: C:\Program Files\RustWTM.

.PARAMETER RemoveData
    Also delete %ProgramData%\RustWTM (config, history, logs).
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:ProgramFiles 'RustWTM'),
    [switch]$RemoveData
)

$ErrorActionPreference = 'Stop'
$ServiceName = 'RustWTM'

function Write-Step([string]$Message) {
    Write-Host "==> $Message" -ForegroundColor Cyan
}

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this script from an elevated PowerShell (Run as administrator).'
}

# When run from inside the install directory, continue from a temp copy so the
# directory can be deleted.
$self = $MyInvocation.MyCommand.Path
if ($self -and $self.StartsWith($InstallDir, [StringComparison]::OrdinalIgnoreCase) -and -not $env:RUSTWTM_UNINSTALL_COPY) {
    $copy = Join-Path ([IO.Path]::GetTempPath()) 'rustwtm-uninstall.ps1'
    Copy-Item $self $copy -Force
    $env:RUSTWTM_UNINSTALL_COPY = '1'
    $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $copy, '-InstallDir', $InstallDir)
    if ($RemoveData) { $arguments += '-RemoveData' }
    & powershell.exe @arguments
    exit $LASTEXITCODE
}

if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
    Write-Step 'Stopping and removing the RustWTM service (limits, program blocks and firewall rules are removed)'
    $svcExe = Join-Path $InstallDir 'wtm-service.exe'
    if (Test-Path $svcExe) {
        & $svcExe uninstall
    } else {
        Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue
        sc.exe delete $ServiceName | Out-Null
    }
}

Get-Process -Name 'rustwtm', 'wtm' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -like "$InstallDir*" } |
    Stop-Process -Force

$link = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'RustWTM.lnk'
if (Test-Path $link) {
    Remove-Item $link -Force
    Write-Step 'Removed Start Menu shortcut'
}

$path = [Environment]::GetEnvironmentVariable('Path', 'Machine')
$parts = $path -split ';' | Where-Object { $_ -and ($_.TrimEnd('\') -ne $InstallDir.TrimEnd('\')) }
[Environment]::SetEnvironmentVariable('Path', ($parts -join ';'), 'Machine')

if (Test-Path $InstallDir) {
    Remove-Item -Recurse -Force $InstallDir
    Write-Step "Removed $InstallDir"
}

$data = Join-Path $env:ProgramData 'RustWTM'
if ($RemoveData -and (Test-Path $data)) {
    Remove-Item -Recurse -Force $data
    Write-Step "Removed $data"
} elseif (Test-Path $data) {
    Write-Host "Kept configuration and history in $data (use -RemoveData to delete)."
}

Write-Host 'RustWTM has been removed.' -ForegroundColor Green

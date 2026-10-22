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
    # A fresh folder under the system's temp folder, which only SYSTEM and
    # administrators can reach: a fixed name in the user's temp folder could
    # be replaced by anything running as the user, then run elevated.
    $copyDir = Join-Path $env:SystemRoot ('Temp\rustwtm-uninstall-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $copyDir | Out-Null
    $acl = New-Object Security.AccessControl.DirectorySecurity
    $acl.SetSecurityDescriptorSddlForm('O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)')
    Set-Acl -LiteralPath $copyDir -AclObject $acl
    $copy = Join-Path $copyDir 'uninstall.ps1'
    Copy-Item $self $copy -Force
    $env:RUSTWTM_UNINSTALL_COPY = '1'
    $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $copy, '-InstallDir', $InstallDir)
    if ($RemoveData) { $arguments += '-RemoveData' }
    & powershell.exe @arguments
    $code = $LASTEXITCODE
    cmd.exe /c rd /s /q "$copyDir"
    exit $code
}

# rd never follows a link inside the folder; Remove-Item in Windows
# PowerShell may, deleting what it points to.
function Remove-Tree([string]$Path) {
    cmd.exe /c rd /s /q "$Path"
    if (Test-Path -LiteralPath $Path) { throw "Could not remove $Path." }
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
    Remove-Tree $InstallDir
    Write-Step "Removed $InstallDir"
}

$data = Join-Path $env:ProgramData 'RustWTM'
if ($RemoveData -and (Test-Path $data)) {
    Remove-Tree $data
    Write-Step "Removed $data"
} elseif (Test-Path $data) {
    Write-Host "Kept configuration and history in $data (use -RemoveData to delete)."
}

Write-Host 'RustWTM has been removed.' -ForegroundColor Green

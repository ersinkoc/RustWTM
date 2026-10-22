<#
.SYNOPSIS
    Installs RustWTM (GUI, CLI and governor service).

.DESCRIPTION
    Downloads a release from GitHub (or builds a local checkout), verifies its
    SHA-256 checksum, copies the binaries to the install directory, adds it to
    the machine PATH, creates a Start Menu shortcut and installs and starts the
    RustWTM service. The service starts in dry-run mode: it only logs what it
    would do until `dry_run = false` is set in %ProgramData%\RustWTM\config.toml.

.PARAMETER Version
    Release tag such as v0.1.0, or "latest" (default).

.PARAMETER InstallDir
    Target directory. Default: C:\Program Files\RustWTM.

.PARAMETER FromSource
    Path to a RustWTM checkout to build with `cargo build --release` instead of
    downloading a release.

.PARAMETER NoService
    Install the GUI and CLI only.

.PARAMETER DownloadOnly
    Only download and extract the binaries into InstallDir (portable use).
    Does not need administrator rights.

.EXAMPLE
    irm https://raw.githubusercontent.com/ersinkoc/RustWTM/main/scripts/install.ps1 | iex
#>
[CmdletBinding()]
param(
    [string]$Version = 'latest',
    [string]$InstallDir = (Join-Path $env:ProgramFiles 'RustWTM'),
    [string]$FromSource,
    [switch]$NoService,
    [switch]$DownloadOnly
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = 'ersinkoc/RustWTM'
$Asset = 'rustwtm-x86_64-pc-windows-msvc.zip'
$Binaries = 'rustwtm.exe', 'wtm.exe', 'wtm-service.exe'
$ServiceName = 'RustWTM'
# The download is checked and copied from a folder only SYSTEM and
# administrators can reach.
$WorkDirSddl = 'O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)'

function Write-Step([string]$Message) {
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Set-Sddl([string]$Path, [string]$Sddl) {
    $acl = New-Object Security.AccessControl.DirectorySecurity
    $acl.SetSecurityDescriptorSddlForm($Sddl)
    Set-Acl -LiteralPath $Path -AclObject $acl
}

# Makes $Dir a folder only SYSTEM and administrators can change, as the
# service runs as SYSTEM from it. wtm-service does it through handles,
# never following a link (checking for one first and then changing the
# folder by name would leave a moment to swap it), sets aside anything in it
# that is not only administrators', and refuses a folder that others could
# move away from under its name.
function Protect-InstallDir([string]$Dir, [string]$Source) {
    & (Join-Path $Source 'wtm-service.exe') prepare-install-dir $Dir
    if ($LASTEXITCODE -ne 0) { throw "Could not make $Dir an administrators-only folder." }
    # A DLL next to the programs would be loaded by them, the service too;
    # RustWTM ships none.
    $stray = Get-ChildItem -LiteralPath $Dir -Filter '*.dll' -File -Force
    if ($stray) {
        throw "$Dir holds DLLs RustWTM does not ship ($($stray.Name -join ', ')); remove them or choose another folder."
    }
}

function Test-Admin {
    $principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Get-ReleaseFiles([string]$Tag, [string]$Destination) {
    $api = if ($Tag -eq 'latest') {
        "https://api.github.com/repos/$Repo/releases/latest"
    } else {
        "https://api.github.com/repos/$Repo/releases/tags/$Tag"
    }
    $release = Invoke-RestMethod -Uri $api -Headers @{ 'User-Agent' = 'RustWTM-installer' }
    $zip = $release.assets | Where-Object name -EQ $Asset
    $sum = $release.assets | Where-Object name -EQ "$Asset.sha256"
    if (-not $zip -or -not $sum) {
        throw "Release $($release.tag_name) has no $Asset (or its .sha256)."
    }
    Write-Step "Downloading RustWTM $($release.tag_name)"
    $zipPath = Join-Path $Destination $Asset
    Invoke-WebRequest -Uri $zip.browser_download_url -OutFile $zipPath -UseBasicParsing
    # Windows PowerShell returns binary content types as byte[].
    $raw = (Invoke-WebRequest -Uri $sum.browser_download_url -UseBasicParsing).Content
    if ($raw -is [byte[]]) { $raw = [Text.Encoding]::ASCII.GetString($raw) }
    $expected = ($raw.Trim() -split '\s+')[0].ToUpperInvariant()
    $actual = (Get-FileHash -Algorithm SHA256 $zipPath).Hash
    if ($actual -ne $expected) {
        throw "Checksum mismatch for ${Asset}: expected $expected, got $actual."
    }
    Write-Step 'Checksum verified'
    $extract = Join-Path $Destination 'extract'
    Expand-Archive -Path $zipPath -DestinationPath $extract -Force
    $extract
}

function Build-FromSource([string]$Path) {
    $root = (Resolve-Path $Path).Path
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw 'cargo was not found. Install Rust from https://rustup.rs first.'
    }
    Write-Step "Building $root (release)"
    Push-Location $root
    try {
        cargo build --release --workspace
        if ($LASTEXITCODE -ne 0) { throw 'cargo build failed.' }
        & (Join-Path $root 'scripts/build-hardware.ps1') -OutputDir 'target/release/hardware' | Out-Host
    } finally {
        Pop-Location
    }
    $out = Join-Path $root 'target\release'
    # Ship the scripts with a source build too.
    Copy-Item (Join-Path $root 'scripts\uninstall.ps1') $out -Force
    foreach ($name in 'README.md', 'LICENSE', 'CHANGELOG.md', 'docs') {
        Copy-Item -LiteralPath (Join-Path $root $name) -Destination $out -Recurse -Force
    }
    $out
}

function Add-ToMachinePath([string]$Dir) {
    $path = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $parts = $path -split ';' | Where-Object { $_ }
    if ($parts -notcontains $Dir) {
        [Environment]::SetEnvironmentVariable('Path', (($parts + $Dir) -join ';'), 'Machine')
        Write-Step "Added $Dir to the machine PATH (open a new terminal to use it)"
    }
}

function New-StartMenuShortcut([string]$Target) {
    $programs = [Environment]::GetFolderPath('CommonPrograms')
    $link = Join-Path $programs 'RustWTM.lnk'
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($link)
    $shortcut.TargetPath = $Target
    $shortcut.WorkingDirectory = Split-Path $Target
    $shortcut.Description = 'RustWTM task manager'
    $shortcut.Save()
    Write-Step "Created Start Menu shortcut $link"
}

if (-not $DownloadOnly -and -not (Test-Admin)) {
    throw 'Run this script from an elevated PowerShell (Run as administrator), or use -DownloadOnly.'
}

# A plain absolute path: what is checked is what is installed to.
$InstallDir = [IO.Path]::GetFullPath($InstallDir).TrimEnd('\')

# Elevated, the download is checked and copied from a folder only
# administrators can change: in the user's own temp folder, anything running
# as the user could swap the files between the checksum and the copy.
$temp = if ($DownloadOnly) { [IO.Path]::GetTempPath() } else { Join-Path $env:SystemRoot 'Temp' }
$work = Join-Path $temp ("rustwtm-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
if (-not $DownloadOnly) { Set-Sddl $work $WorkDirSddl }
try {
    $source = if ($FromSource) { Build-FromSource $FromSource } else { Get-ReleaseFiles $Version $work }
    foreach ($b in $Binaries) {
        if (-not (Test-Path (Join-Path $source $b))) { throw "$b is missing from $source." }
    }

    $service = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
    if ($service -and $service.Status -ne 'Stopped' -and -not $DownloadOnly) {
        Write-Step 'Stopping the running service (limits are rolled back)'
        Stop-Service -Name $ServiceName -Force
        (Get-Service $ServiceName).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
    }
    # A running GUI or CLI would lock its executable.
    Get-Process -Name 'rustwtm' -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -like "$InstallDir*" } |
        Stop-Process -Force

    Write-Step "Installing to $InstallDir"
    if ($DownloadOnly) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    } else {
        Protect-InstallDir $InstallDir $source
    }
    $files = $Binaries + 'uninstall.ps1', 'README.md', 'LICENSE', 'CHANGELOG.md'
    foreach ($f in $files) {
        $from = Join-Path $source $f
        if (Test-Path $from) { Copy-Item $from $InstallDir -Force }
    }
    foreach ($dir in 'hardware', 'docs') {
        $from = Join-Path $source $dir
        if (Test-Path -LiteralPath $from) {
            # Replaced whole, so nothing of an older version stays behind.
            $to = Join-Path $InstallDir $dir
            if (Test-Path -LiteralPath $to) { cmd.exe /c rd /s /q "$to" }
            Copy-Item -LiteralPath $from -Destination $InstallDir -Recurse -Force
        }
    }

    if ($DownloadOnly) {
        Write-Host "`nRustWTM extracted to $InstallDir (portable; service not installed)." -ForegroundColor Green
        return
    }

    Add-ToMachinePath $InstallDir
    New-StartMenuShortcut (Join-Path $InstallDir 'rustwtm.exe')

    if (-not $NoService) {
        $svcExe = Join-Path $InstallDir 'wtm-service.exe'
        if (-not (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue)) {
            Write-Step 'Installing the RustWTM service'
            & $svcExe install
            if ($LASTEXITCODE -ne 0) { throw 'Service installation failed.' }
        }
        Write-Step 'Starting the RustWTM service'
        & $svcExe start
        if ($LASTEXITCODE -ne 0) { throw 'Service start failed.' }
    }

    $config = Join-Path $env:ProgramData 'RustWTM\config.toml'
    Write-Host ''
    Write-Host 'RustWTM is installed.' -ForegroundColor Green
    Write-Host "  GUI:     Start Menu > RustWTM   (or: rustwtm)"
    Write-Host "  CLI:     wtm --help"
    if (-not $NoService) {
        Write-Host "  Config:  $config"
        Write-Host '  The service runs in DRY-RUN mode: it only logs decisions. Review the'
        Write-Host '  Events tab, then set dry_run = false in the config to enforce.'
    }
} finally {
    # rd never follows a link inside; Remove-Item in Windows PowerShell may.
    if (Test-Path -LiteralPath $work) { cmd.exe /c rd /s /q "$work" }
}

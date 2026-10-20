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

function Write-Step([string]$Message) {
    Write-Host "==> $Message" -ForegroundColor Cyan
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

$work = Join-Path ([IO.Path]::GetTempPath()) ("rustwtm-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
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
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $files = $Binaries + 'uninstall.ps1', 'README.md', 'LICENSE', 'CHANGELOG.md'
    foreach ($f in $files) {
        $from = Join-Path $source $f
        if (Test-Path $from) { Copy-Item $from $InstallDir -Force }
    }
    $hardware = Join-Path $source 'hardware'
    if (Test-Path -LiteralPath $hardware) {
        Copy-Item -LiteralPath $hardware -Destination $InstallDir -Recurse -Force
    }
    $docs = Join-Path $source 'docs'
    if (Test-Path -LiteralPath $docs) {
        Copy-Item -LiteralPath $docs -Destination $InstallDir -Recurse -Force
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
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

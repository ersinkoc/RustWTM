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
Wait-Until { & $wtm history --hours 1 -n 3 *> $null; $LASTEXITCODE -eq 0 } 'wtm history via the service' 30
Assert $true 'wtm history reaches the service'

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
$machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine') -split ';'
Assert (-not ($machinePath -contains $InstallDir)) 'PATH entry removed'
Assert (Test-Path (Join-Path $DataDir 'config.toml')) 'configuration kept after uninstall'
Write-Host "Install / uninstall verification passed ($Mode)." -ForegroundColor Green

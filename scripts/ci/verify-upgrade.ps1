<#
.SYNOPSIS
    Installs real services ONLY on a disposable GitHub Actions Windows VM.
    Proves baseline -> candidate upgrade, config/history preservation, repair,
    downgrade rejection and uninstall retention. Never run on a developer PC.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$BaselineMsi,
    [Parameter(Mandatory)][string]$CandidateMsi,
    [Parameter(Mandatory)][version]$BaselineVersion,
    [Parameter(Mandatory)][version]$CandidateVersion,
    [switch]$DisposableMachine
)
$ErrorActionPreference = 'Stop'
if (-not $DisposableMachine -or $env:GITHUB_ACTIONS -cne 'true' -or
    $env:RUSTWTM_DISPOSABLE_INSTALL_TESTS -cne '1') {
    throw 'Real installer tests require an explicitly enabled disposable GitHub Actions VM.'
}
if ($CandidateVersion -le $BaselineVersion) { throw 'Candidate must be newer than baseline' }
$baseline = (Resolve-Path -LiteralPath $BaselineMsi).Path
$candidate = (Resolve-Path -LiteralPath $CandidateMsi).Path
$install = Join-Path $env:ProgramFiles 'RustWTM'
$data = Join-Path $env:ProgramData 'RustWTM'
$config = Join-Path $data 'config.toml'
$database = Join-Path $data 'history.sqlite'
if ((Get-Service RustWTM -ErrorAction SilentlyContinue) -or
    (Test-Path -LiteralPath $install) -or (Test-Path -LiteralPath $config) -or
    (Test-Path -LiteralPath $database)) {
    throw 'Upgrade test requires a clean VM without an existing RustWTM installation or data.'
}
$fixture = 'rustwtm-upgrade-' + [Guid]::NewGuid().ToString('N') + '.exe'
function Assert([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "ASSERTION FAILED: $Message" }
    Write-Host "ok - $Message"
}
function Wait-Until([scriptblock]$Condition, [string]$What) {
    $until = (Get-Date).AddSeconds(60)
    while (-not (& $Condition)) {
        if ((Get-Date) -ge $until) { throw "Timed out: $What" }
        Start-Sleep -Milliseconds 500
    }
}
function Msi([string]$Operation, [string]$File, [string]$Log, [switch]$ExpectFailure) {
    $p = Start-Process msiexec.exe -WindowStyle Hidden -ArgumentList $Operation, "`"$File`"", '/qn', '/norestart', '/l*v', $Log -Wait -PassThru
    if ($ExpectFailure) { Assert ($p.ExitCode -eq 1603) "downgrade rejected with 1603 (got $($p.ExitCode))" }
    else { Assert ($p.ExitCode -eq 0) "$Operation completed without reboot (got $($p.ExitCode))" }
}
function Installed([version]$Version) {
    Wait-Until { (Get-Service RustWTM -ErrorAction SilentlyContinue).Status -eq 'Running' } 'service running'
    $cli = Join-Path $install 'wtm.exe'
    $actual = & $cli --version
    Assert ($LASTEXITCODE -eq 0 -and $actual -ceq "wtm $Version") "installed CLI version $Version"
    $actual = & (Join-Path $install 'wtm-service.exe') --version
    Assert ($LASTEXITCODE -eq 0 -and $actual -ceq "wtm-service $Version") "installed service binary version $Version"
    Wait-Until { & $cli history --hours 1 -n 10000 *> $null; $LASTEXITCODE -eq 0 } 'verified service IPC'
}
$sqlite = @'
import sqlite3,sys,time
db,key,action=sys.argv[1:]
with sqlite3.connect(db, timeout=10) as conn:
    if action=='seed':
        minute=int(time.time())//60-2
        conn.execute('INSERT INTO app_minute (app,minute,cpu_avg,cpu_peak,mem_avg,mem_peak,processes,gpu_avg,handles_avg,threads_avg,samples) VALUES (?,?,?,?,?,?,?,?,?,?,?)', (key,minute,1.25,2.5,4096,8192,1,0,3,2,1))
    rows=conn.execute('SELECT cpu_avg,cpu_peak,mem_avg,mem_peak,processes,samples FROM app_minute WHERE app=?',(key,)).fetchall()
    assert rows==[(1.25,2.5,4096,8192,1,1)], rows
print('PASS: owned history row is intact')
'@
function Check-Preserved {
    Assert ((Get-FileHash -LiteralPath $config -Algorithm SHA256).Hash -ceq $script:configHash) 'configuration bytes retained'
    & python -c $sqlite $database $fixture 'assert'
    Assert ($LASTEXITCODE -eq 0) 'persistent history values retained'
    $history = & (Join-Path $install 'wtm.exe') history --hours 1 -n 10000
    Assert ($LASTEXITCODE -eq 0 -and [bool]($history | Select-String -SimpleMatch $fixture)) 'history fixture readable over service IPC'
}
Msi '/i' $baseline 'upgrade-baseline.log'
Installed $BaselineVersion
Wait-Until { Test-Path -LiteralPath $database } 'history database creation'
Stop-Service RustWTM
Add-Content -LiteralPath $config -Value "`n# Owned upgrade retention fixture: $fixture"
$script:configHash = (Get-FileHash -LiteralPath $config -Algorithm SHA256).Hash
& python -c $sqlite $database $fixture 'seed'
Assert ($LASTEXITCODE -eq 0) 'history fixture seeded while service stopped'
Start-Service RustWTM
Installed $BaselineVersion
Check-Preserved
Msi '/i' $candidate 'upgrade-candidate.log'
Installed $CandidateVersion
Check-Preserved
Msi '/fa' $candidate 'upgrade-repair.log'
Installed $CandidateVersion
Check-Preserved
Msi '/i' $baseline 'upgrade-downgrade.log' -ExpectFailure
Installed $CandidateVersion
Check-Preserved
Msi '/x' $candidate 'upgrade-uninstall.log'
Wait-Until { -not (Get-Service RustWTM -ErrorAction SilentlyContinue) } 'service removal'
Assert (Test-Path -LiteralPath $config) 'configuration retained after uninstall'
Assert ((Get-FileHash -LiteralPath $config -Algorithm SHA256).Hash -ceq $script:configHash) 'configuration unchanged after uninstall'
& python -c $sqlite $database $fixture 'assert'
Assert ($LASTEXITCODE -eq 0) 'history retained after uninstall'
Assert (-not (Test-Path -LiteralPath (Join-Path $install 'wtm-service.exe'))) 'installed service binary removed'
Write-Host 'Upgrade / repair / downgrade rejection / uninstall retention verification passed.'

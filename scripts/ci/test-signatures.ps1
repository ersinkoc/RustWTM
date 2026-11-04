# Read-only trust control plus one owned unsigned fixture. No certificates are
# installed and no executable fixture is launched.
$ErrorActionPreference = 'Stop'
$verify = Join-Path $PSScriptRoot 'verify-signatures.ps1'
$control = Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe'
$signature = Get-AuthenticodeSignature -LiteralPath $control
if ($signature.Status -ne 'Valid') { throw 'Signed Windows control is unavailable' }
$publisher = $signature.SignerCertificate.GetNameInfo(
    [System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
& $verify -Path $control -ExpectedPublisher $publisher -RequireTimestamp
$rejected = $false
try { & $verify -Path $control -ExpectedPublisher 'RustWTM intentionally incorrect publisher' }
catch { if ($_.Exception.Message -notlike 'Publisher mismatch:*') { throw }; $rejected = $true }
if (-not $rejected) { throw 'A different publisher was accepted' }
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('rustwtm-unsigned-' + [Guid]::NewGuid().ToString('N') + '.exe')
try {
    [IO.File]::WriteAllText($fixture, 'Owned unsigned signature-validation fixture, never executed.')
    $rejected = $false
    try { & $verify -Path $fixture -ExpectedPublisher $publisher -RequireTimestamp }
    catch { if ($_.Exception.Message -notlike 'Signature is not valid:*') { throw }; $rejected = $true }
    if (-not $rejected) { throw 'An unsigned artifact was accepted' }
} finally { Remove-Item -LiteralPath $fixture -Force -ErrorAction SilentlyContinue }
Write-Host 'PASS: trusted control accepted, wrong publisher and unsigned artifact rejected'

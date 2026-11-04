<# Verifies Authenticode trust and publisher identity; does not sign or install. #>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string[]]$Path,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$ExpectedPublisher,
    [switch]$RequireTimestamp
)
$ErrorActionPreference = 'Stop'
foreach ($file in $Path) {
    $item = Get-Item -LiteralPath $file -ErrorAction Stop
    if ($item.PSIsContainer) { throw "Expected a file: $file" }
    $signature = Get-AuthenticodeSignature -LiteralPath $item.FullName
    if ($signature.Status -ne 'Valid' -or $null -eq $signature.SignerCertificate) {
        throw "Signature is not valid: $($item.Name) ($($signature.Status))"
    }
    $publisher = $signature.SignerCertificate.GetNameInfo(
        [System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
    if (-not [string]::Equals($publisher, $ExpectedPublisher, [StringComparison]::Ordinal)) {
        throw "Publisher mismatch: $($item.Name); expected '$ExpectedPublisher', got '$publisher'"
    }
    if ($RequireTimestamp -and $null -eq $signature.TimeStamperCertificate) {
        throw "Timestamp is missing: $($item.Name)"
    }
    Write-Host "PASS: trusted signature and publisher on $($item.Name)"
}

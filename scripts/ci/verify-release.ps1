param(
    [Parameter(Mandatory)][string]$Tag,
    [string]$Root = (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent)
)
$ErrorActionPreference = 'Stop'
if ($Tag -cnotmatch '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
    throw "Expected a final release tag vMAJOR.MINOR.PATCH, got '$Tag'."
}
$version = $Tag.Substring(1)
$manifest = Get-Content (Join-Path $Root 'Cargo.toml') -Raw
$workspace = [regex]::Match($manifest, '(?ms)^\[workspace\.package\]\s*\r?\n(.*?)(?=^\[|\z)').Groups[1].Value
$actual = [regex]::Match($workspace, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if ($actual -cne $version) { throw "Tag $Tag disagrees with workspace version '$actual'." }
[xml]$hardware = Get-Content (Join-Path $Root 'hardware/Wtm.Hardware/Wtm.Hardware.csproj') -Raw
if ($hardware.Project.PropertyGroup.Version -cne $version) {
    throw "Hardware provider version disagrees with $Tag."
}
$changelog = Get-Content (Join-Path $Root 'CHANGELOG.md') -Raw
$entry = [regex]::Match($changelog, "(?ms)^## \[$([regex]::Escape($version))\] - [0-9]{4}-[0-9]{2}-[0-9]{2}\r?\n(.*?)(?=^## \[|\z)")
if (-not $entry.Success -or -not $entry.Groups[1].Value.Trim()) {
    throw "Missing dated, nonempty changelog entry for $Tag."
}
Write-Output "PASS: $Tag matches the workspace, hardware provider and changelog."

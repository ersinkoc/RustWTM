param(
    [string]$OutputDir = 'target/debug/hardware',
    [switch]$FrameworkDependent
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$project = Join-Path $root 'hardware/Wtm.Hardware/Wtm.Hardware.csproj'
$output = if ([IO.Path]::IsPathRooted($OutputDir)) { $OutputDir } else { Join-Path $root $OutputDir }
dotnet publish $project -c Release -r win-x64 --self-contained ($FrameworkDependent.IsPresent -eq $false).ToString().ToLowerInvariant() -p:RestoreLockedMode=true -o $output
if ($LASTEXITCODE -ne 0) { throw 'Hardware provider build failed.' }
$notices = @('RustWTM hardware provider: LibreHardwareMonitorLib 0.9.6 (MPL-2.0).',
    'Unmodified upstream library. Source: https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/tree/3d331e3370efb858411f19511373eff65a218701',
    'The following upstream and .NET runtime license notices accompany this optional provider.')
foreach ($file in Get-ChildItem (Join-Path $root 'hardware/licenses') -Filter '*.txt') {
    $notices += "`n--- $($file.Name) ---`n"
    $notices += Get-Content -LiteralPath $file.FullName -Raw
}
$packageRoot = if ($env:NUGET_PACKAGES) { $env:NUGET_PACKAGES } else { Join-Path $env:USERPROFILE '.nuget/packages' }
$lock = Get-Content (Join-Path (Split-Path $project -Parent) 'packages.lock.json') -Raw | ConvertFrom-Json
$seen = @{}
foreach ($group in $lock.dependencies.PSObject.Properties) {
    foreach ($entry in $group.Value.PSObject.Properties) {
        if ($seen.ContainsKey($entry.Name)) { continue }
        $seen[$entry.Name] = $true
        $path = Join-Path $packageRoot "$($entry.Name.ToLowerInvariant())/$($entry.Value.resolved)"
        $nuspec = Get-ChildItem -LiteralPath $path -Filter '*.nuspec' | Select-Object -First 1
        if (-not $nuspec) { throw "Missing package metadata: $path" }
        [xml]$spec = Get-Content -LiteralPath $nuspec.FullName -Raw
        $meta = $spec.package.metadata
        $notices += "`n--- Package $($entry.Name) $($entry.Value.resolved) ---"
        $notices += "Authors: $($meta.authors). $($meta.copyright)"
        $notices += "License: $($meta.license.InnerText) $($meta.licenseUrl)"
        $notices += "Source/project: $($meta.projectUrl) $($meta.repository.url) $($meta.repository.commit)"
        if ($meta.license.type -eq 'file') {
            $notices += Get-Content -LiteralPath (Join-Path $path $meta.license.InnerText) -Raw
        }
    }
}
$runtime = Join-Path $packageRoot 'microsoft.netcore.app.runtime.win-x64/8.0.31'
foreach ($name in 'LICENSE.TXT', 'THIRD-PARTY-NOTICES.TXT') {
    $file = Join-Path $runtime $name
    if (Test-Path -LiteralPath $file) { $notices += "`n--- .NET $name ---`n"; $notices += Get-Content -LiteralPath $file -Raw }
    elseif (-not $FrameworkDependent) { throw "Required runtime license notice missing: $file" }
}
$notices | Set-Content -LiteralPath (Join-Path $output 'PROVIDER-NOTICES.txt') -Encoding utf8
& (Join-Path $output 'wtm-hardware.exe') --self-test
if ($LASTEXITCODE -ne 0) { throw 'Hardware provider self-test failed.' }

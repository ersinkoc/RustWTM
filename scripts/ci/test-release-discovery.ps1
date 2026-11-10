# Exercise only Get-ReleaseFiles with injected HTTP/hash/extraction seams.
# Never evaluate the installer entry point, write payloads or install anything.
$ErrorActionPreference = 'Stop'
$source = Join-Path (Split-Path $PSScriptRoot -Parent) 'install.ps1'
$text = Get-Content -LiteralPath $source -Raw
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($text, [ref]$tokens, [ref]$errors)
if ($errors) { throw 'Installer syntax is invalid' }
$definition = $ast.Find({ param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Get-ReleaseFiles'
}, $true)
if (-not $definition) { throw 'Missing release discovery function' }
. ([ScriptBlock]::Create($definition.Extent.Text))
$RepositoryId = [long][regex]::Match($text, '(?m)^\$RepositoryId = ([0-9]+)').Groups[1].Value
$Asset = [regex]::Match($text, "(?m)^\`$Asset = '([^']+)'").Groups[1].Value
if ($RepositoryId -ne 1425760656 -or $Asset -ne 'rustwtm-x86_64-pc-windows-msvc.zip') { throw 'Unexpected official repository or asset identity' }
$script:requests = [Collections.Generic.List[string]]::new()
$script:hash = 'A' * 64
$script:extracts = 0

function Write-Step([string]$Message) { }
function Invoke-RestMethod { param($Uri, $Headers)
    if ($Uri -cne $script:expectedApi) { throw "Unexpected discovery route: $Uri" }
    $script:requests.Add($Uri)
    $script:release
}
function Invoke-WebRequest { param($Uri, $OutFile, [switch]$UseBasicParsing)
    $script:requests.Add($Uri)
    if ($OutFile) {
        if ($Uri -cne $script:zipUrl) { throw 'Installer download used a different release source' }
        return
    }
    if ($Uri -cne ($script:zipUrl + '.sha256')) { throw 'Checksum used a different release source' }
    [pscustomobject]@{ Content = (('a' * 64) + '  ' + $Asset) }
}
function Get-FileHash { param($Algorithm, $Path) [pscustomobject]@{ Hash = $script:hash } }
function Expand-Archive { param($Path, $DestinationPath, [switch]$Force) $script:extracts++ }

foreach ($repository in 'ersinkoc/RustWTM','rustwtm/rustwtm') {
    $script:zipUrl = "https://github.com/$repository/releases/download/v0.28.0/$Asset"
    $script:release = [pscustomobject]@{
        tag_name = 'v0.28.0'
        assets = @(
            [pscustomobject]@{ name = $Asset; browser_download_url = $script:zipUrl },
            [pscustomobject]@{ name = "$Asset.sha256"; browser_download_url = ($script:zipUrl + '.sha256') }
        )
    }
    foreach ($tag in 'latest','v0.28.0') {
        $suffix = if ($tag -eq 'latest') { 'latest' } else { "tags/$tag" }
        $script:expectedApi = "https://api.github.com/repositories/1425760656/releases/$suffix"
        $script:requests.Clear()
        $script:extracts = 0
        $null = Get-ReleaseFiles $tag (Join-Path $env:TEMP 'rustwtm-discovery-never-written')
        if ($script:requests.Count -ne 3 -or $script:extracts -ne 1) { throw 'Incomplete verified release handoff' }
    }
    $script:hash = 'B' * 64
    $script:extracts = 0
    $rejected = $false
    try { $null = Get-ReleaseFiles 'v0.28.0' (Join-Path $env:TEMP 'rustwtm-discovery-never-written') }
    catch { if ($_.Exception.Message -notlike 'Checksum mismatch*') { throw }; $rejected = $true }
    if (-not $rejected -or $script:extracts -ne 0) { throw 'Bad checksum reached extraction' }
    $script:hash = 'A' * 64
    $script:release.assets = @($script:release.assets[0])
    $script:requests.Clear()
    $rejected = $false
    try { $null = Get-ReleaseFiles 'v0.28.0' (Join-Path $env:TEMP 'rustwtm-discovery-never-written') }
    catch { if ($_.Exception.Message -notlike '*has no*sha256*') { throw }; $rejected = $true }
    if (-not $rejected -or $script:requests.Count -ne 1) { throw 'Missing checksum reached download' }
}
Write-Output 'PASS: identity-based latest/tag discovery before/after transfer, paired assets, checksum and missing-asset rejection; no install or payload writes'

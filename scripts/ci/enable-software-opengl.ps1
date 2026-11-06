# CI only: the hosted VM exposes OpenGL 1.1. Supply an application-local
# llvmpipe runtime for native-window tests; never alter Windows system DLLs.
# Source/deployment documentation: https://github.com/pal1000/mesa-dist-win
param([Parameter(Mandatory)][string]$InstallDir)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -cne 'true' -or $env:RUNNER_OS -cne 'Windows') {
    throw 'Software OpenGL setup is limited to disposable GitHub Windows runners.'
}
$expected = [IO.Path]::GetFullPath((Join-Path $env:ProgramFiles 'RustWTM'))
if ([IO.Path]::GetFullPath($InstallDir) -ine $expected -or
    -not (Test-Path -LiteralPath (Join-Path $InstallDir 'rustwtm.exe'))) {
    throw 'Expected the protected disposable RustWTM installation.'
}
$stage = Join-Path $env:RUNNER_TEMP 'rustwtm-ci-mesa-26.2.6'
New-Item -ItemType Directory -Path $stage -Force | Out-Null
$archive = Join-Path $stage 'mesa3d-26.2.6-release-mingw.7z'
$url = 'https://github.com/pal1000/mesa-dist-win/releases/download/26.2.6/mesa3d-26.2.6-release-mingw.7z'
Invoke-WebRequest -Uri $url -OutFile $archive
$actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -cne 'da0f71d9181b672c1918f3c342f11118c687f6059a46b16502845160733cc388') {
    throw 'Pinned software OpenGL archive checksum mismatch.'
}
$files = @('opengl32.dll', 'libgallium_wgl.dll', 'libdlclose-skip.dll')
$entries = $files | ForEach-Object { "x64\$_" }
& 7z x $archive "-o$stage/unpacked" @entries -y
if ($LASTEXITCODE -ne 0) { throw 'Software OpenGL extraction failed.' }
foreach ($name in $files) {
    $target = Join-Path $InstallDir $name
    if (Test-Path -LiteralPath $target) { throw "Refusing to overwrite an installed file: $name" }
    Copy-Item -LiteralPath (Join-Path $stage "unpacked/x64/$name") -Destination $target
}
$env:GALLIUM_DRIVER = 'llvmpipe'
$env:LIBGL_ALWAYS_SOFTWARE = '1'
$env:LP_NUM_THREADS = '2'
Write-Output 'PASS: digest-pinned Mesa 26.2.6 llvmpipe installed only beside the disposable test GUI; release artifacts unchanged.'

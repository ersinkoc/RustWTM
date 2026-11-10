[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$request=[Console]::In.ReadToEnd() | ConvertFrom-Json
$packages=@();$store=@();$warnings=@()
function LoadClient {
 $roots=@((Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'WindowsPowerShell/Modules/Microsoft.WinGet.Client'))
 $admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
 if(-not $admin){
  $documents=[Environment]::GetFolderPath('MyDocuments')
  $roots+=(Join-Path $documents 'WindowsPowerShell/Modules/Microsoft.WinGet.Client')
  $roots+=(Join-Path $documents 'PowerShell/Modules/Microsoft.WinGet.Client')
 }
 foreach($root in $roots){
  if(-not (Test-Path -LiteralPath $root)){continue}
  foreach($version in @(Get-ChildItem -LiteralPath $root -Directory | Where-Object {$_.Name -match '^\d+\.\d+\.\d+$'} | Sort-Object {[version]$_.Name} -Descending | Select-Object -First 8)){
   $directory=Join-Path $version.FullName 'net48';$binary=Join-Path $directory 'Microsoft.WinGet.Client.Cmdlets.dll'
   if(-not (Test-Path -LiteralPath $binary)){continue}
   $files=@(Get-ChildItem -LiteralPath $directory -Recurse -File -Filter '*.dll' | Select-Object -First 129)
   if($files.Count -gt 128 -or $files.Count -eq 0){continue}
   $valid=$true
   foreach($file in $files){$s=Get-AuthenticodeSignature -LiteralPath $file.FullName;if($s.Status -ne 'Valid' -or $s.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation(,|$)'){$valid=$false;break}}
   if(-not $valid){continue}
   # Import only the signed binary, never a user-controlled manifest or script.
   Import-Module -Name $binary -ErrorAction Stop
   return
  }
 }
 throw 'Microsoft.WinGet.Client is unavailable or could not be verified. Install the official module; elevated windows require an all-users installation.'
}
function SourceName($package){
 $source=[string]$package.Source
 if($source -eq 'winget' -or $source -eq 'msstore'){return $source}
 if($package.Source.Name){return [string]$package.Source.Name}
 return $source
}
if($request.operation -eq 'inventory'){
 try {
  foreach($app in @(Get-AppxPackage -PackageTypeFilter Main -ErrorAction Stop | Where-Object {-not $_.IsFramework -and -not $_.IsResourcePackage} | Select-Object -First 1024)){
   $store+=@{id=[string]$app.PackageFullName;family=[string]$app.PackageFamilyName;name=[string]$app.Name;publisher=[string]$app.Publisher;version=[string]$app.Version;location=[string]$app.InstallLocation;removable=(-not [bool]$app.NonRemovable)}
  }
 }catch{$warnings+='Store inventory: '+$_.Exception.Message}
 try{
  LoadClient
  foreach($p in @(Get-WinGetPackage -Count 1000 -ErrorAction Stop)){
   $target=$null
   if($p.IsUpdateAvailable -and $p.AvailableVersions.Count -gt 0){$target=[string]$p.AvailableVersions[0]}
   $packages+=@{id=[string]$p.Id;source=(SourceName $p);name=[string]$p.Name;installed=[string]$p.InstalledVersion;target=$target}
  }
 }catch{$warnings+='WinGet: '+$_.Exception.Message}
 @{packages=@($packages);store=@($store);warnings=@($warnings)} | ConvertTo-Json -Depth 6 -Compress
}elseif($request.operation -eq 'update'){
 LoadClient
 $results=@()
 foreach($p in @($request.packages)){
  $status='Failed';$message=''
  try{
   if($p.source -notin @('winget','msstore') -or $p.id -notmatch '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,199}$' -or -not $p.target){throw 'Invalid package identity'}
   $current=@(Get-WinGetPackage -Id $p.id -Source $p.source -MatchOption Equals -Count 2 -ErrorAction Stop)
   if($current.Count -ne 1 -or [string]$current[0].InstalledVersion -ne $p.installed -or -not $current[0].IsUpdateAvailable -or @($current[0].AvailableVersions) -notcontains $p.target){throw 'Package identity or versions changed. Refresh and review a new plan.'}
   $r=Update-WinGetPackage -Id $p.id -Source $p.source -MatchOption Equals -Version $p.target -Mode Interactive -Confirm:$false -ErrorAction Stop
   $status=[string]$r.Status
   $message='Installer result: '+$status+'; reboot required: '+[string]$r.RebootRequired
  }catch{$message=$_.Exception.Message}
  $results+=@{id=[string]$p.id;source=[string]$p.source;status=$status;message=$message}
 }
 ConvertTo-Json -InputObject @($results) -Depth 4 -Compress
}else{throw 'Unsupported package operation'}

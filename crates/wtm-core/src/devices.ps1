[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$devices=@(); $warnings=@()
function Field($name,$value) { if ($null -ne $value -and [string]$value -ne '') { @{name=$name;value=[string]$value} } }
function Metric($id,$name,$unit,$value,$ceiling=$null,$counter=$false) {
 @{id=$id;name=$name;unit=$unit;value=$(if ($null -ne $value) {[double]$value} else {$null});ceiling=$ceiling;counter=$counter}
}
function Stamp { [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() }
function UsedMemory($total,$available) { [math]::Max([double]0,([double]$total-[double]$available)) }
# Native providers
try {
 $physical=@(); try { $physical=@(Get-PhysicalDisk -ErrorAction Stop | Select-Object -First 32) } catch { $warnings+='Physical disk health access unavailable' }
 $perf=@(Get-CimInstance Win32_PerfFormattedData_PerfDisk_PhysicalDisk -OperationTimeoutSec 8 | Where-Object Name -ne '_Total' | Select-Object -First 32)
 foreach($disk in @(Get-Disk -ErrorAction Stop | Select-Object -First 32)) {
  $id=[string]$disk.UniqueId
  if (-not $id) { $id='session-disk-'+$disk.Number }
  $fields=@((Field 'Disk number' $disk.Number),(Field 'Model' $disk.FriendlyName),(Field 'Capacity (bytes)' $disk.Size),(Field 'Bus' $disk.BusType),(Field 'Partition style' $disk.PartitionStyle),(Field 'Firmware' $disk.FirmwareVersion)) | Where-Object {$_}
  $matched=@($physical | Where-Object { $_.UniqueId -and [string]$_.UniqueId -eq [string]$disk.UniqueId })
  $reliability=$null; $warning=''
  if ($matched.Count -eq 1) {
   $fields+=Field 'Storage health' $matched[0].HealthStatus
   try {$reliability=$matched[0] | Get-StorageReliabilityCounter -ErrorAction Stop} catch {$warning='Reliability counters unavailable or access denied'}
  } else {$warning='Health identity could not be matched uniquely'}
  $p=@($perf | Where-Object {($_.Name -split ' ')[0] -eq [string]$disk.Number})
  $p=if($p.Count -eq 1){$p[0]}else{$null}
  $metrics=@((Metric 'active' 'Active time' '%' $(if($p){[math]::Max(0.0,[math]::Min(100.0,100.0-[double]$p.PercentIdleTime))}else{$null}) 100),
   (Metric 'read' 'Read' 'B/s' $p.DiskReadBytesPersec), (Metric 'write' 'Write' 'B/s' $p.DiskWriteBytesPersec),
   (Metric 'temperature' 'Temperature' 'C' $reliability.Temperature), (Metric 'wear' 'Wear used' '%' $reliability.Wear 100),
   (Metric 'hours' 'Power-on time' 'h' $reliability.PowerOnHours), (Metric 'read-errors' 'Uncorrected read errors' '' $reliability.ReadErrorsUncorrected),
   (Metric 'write-errors' 'Uncorrected write errors' '' $reliability.WriteErrorsUncorrected))
  $devices+=@{id='disk:'+$id;kind='Disk';name='Disk '+$disk.Number+' - '+$disk.FriendlyName;at=(Stamp);fields=@($fields);metrics=@($metrics);warning=$warning}
 }
} catch {$warnings+='Disk telemetry: '+$_.Exception.Message}
try {
 foreach($n in @(Get-NetAdapter -IncludeHidden -ErrorAction Stop | Where-Object {$_.HardwareInterface -or $_.Status -eq 'Up'} | Select-Object -First 16)) {
  $stats=$null;try{$stats=$n | Get-NetAdapterStatistics -ErrorAction Stop}catch{}
  $devices+=@{id='network:'+[string]$n.InterfaceGuid;kind='Network';name=[string]$n.Name;at=(Stamp);warning='';
   fields=@((Field 'Adapter' $n.InterfaceDescription),(Field 'Status' $n.Status),(Field 'Link speed' $n.LinkSpeed)) | Where-Object {$_};
   metrics=@((Metric 'receive' 'Receive' 'B/s' $stats.ReceivedBytes $null $true),(Metric 'send' 'Send' 'B/s' $stats.SentBytes $null $true))}
 }
} catch {$warnings+='Network telemetry: '+$_.Exception.Message}
try {
 $static=@(Get-CimInstance -Namespace root/wmi -ClassName BatteryStaticData -OperationTimeoutSec 8 -ErrorAction Stop)
 $full=@(Get-CimInstance -Namespace root/wmi -ClassName BatteryFullChargedCapacity -OperationTimeoutSec 8 -ErrorAction Stop)
 $cycles=@();try{$cycles=@(Get-CimInstance -Namespace root/wmi -ClassName BatteryCycleCount -OperationTimeoutSec 8 -ErrorAction Stop)}catch{}
 foreach($battery in @(Get-CimInstance -Namespace root/wmi -ClassName BatteryStatus -OperationTimeoutSec 8 -ErrorAction Stop | Select-Object -First 4)) {
  $identity=@($static|Where-Object InstanceName -eq $battery.InstanceName)
  $capacity=@($full|Where-Object InstanceName -eq $battery.InstanceName)
  $cycle=@($cycles|Where-Object InstanceName -eq $battery.InstanceName)
  $info=if($identity.Count -eq 1){$identity[0]}else{$null}
  $charged=if($capacity.Count -eq 1){$capacity[0].FullChargedCapacity}else{$null}
  $health=if($info.DesignedCapacity -gt 0 -and $null -ne $charged){100.0*$charged/$info.DesignedCapacity}else{$null}
  $charge=if($charged -gt 0){100.0*$battery.RemainingCapacity/$charged}else{$null}
  $watts=if($battery.Discharging -and $battery.DischargeRate -ne 4294967295){-[double]$battery.DischargeRate/1000}else{if($battery.Charging -and $battery.ChargeRate -ne 4294967295){[double]$battery.ChargeRate/1000}else{$null}}
  $devices+=@{id='battery:'+[string]$battery.InstanceName;kind='Battery';name=$(if($info.DeviceName){[string]$info.DeviceName}else{'Battery'});at=(Stamp);warning='';
   fields=@((Field 'Manufacturer' $info.ManufactureName),(Field 'Design capacity (mWh)' $info.DesignedCapacity),(Field 'Full capacity (mWh)' $charged),(Field 'AC connected' $battery.PowerOnline),(Field 'Charging' $battery.Charging));
   metrics=@((Metric 'charge' 'Charge' '%' $charge 100),(Metric 'health' 'Capacity health' '%' $health 100),(Metric 'power' 'Charge / discharge' 'W' $watts),(Metric 'voltage' 'Voltage' 'V' $(if($battery.Voltage -ne 4294967295){$battery.Voltage/1000.0}else{$null})),(Metric 'cycles' 'Cycles' '' $(if($cycle.Count -eq 1){$cycle[0].CycleCount}else{$null})))}
 }
} catch {$warnings+='Battery telemetry unavailable (including desktops without a battery)'}
try {
 $engines=@(Get-CimInstance -ClassName Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine -OperationTimeoutSec 8 -ErrorAction Stop | Select-Object -First 2048)
 $memory=@(Get-CimInstance -ClassName Win32_PerfFormattedData_GPUPerformanceCounters_GPUAdapterMemory -OperationTimeoutSec 8 -ErrorAction Stop | Select-Object -First 64)
 $groups=@{}
 foreach($engine in $engines) {
  if($engine.Name -match 'luid_0x([0-9a-fA-F]+)_0x([0-9a-fA-F]+)_phys_(\d+)_eng_(\d+)_engtype_(.+)$') {
   $key=$Matches[1].ToLower().PadLeft(8,'0')+'_'+$Matches[2].ToLower().PadLeft(8,'0')+':'+$Matches[3]
   $channel=$Matches[4]+':'+$Matches[5]
   if(-not $groups.ContainsKey($key)){$groups[$key]=@{}}
   if(-not $groups[$key].ContainsKey($channel)){$groups[$key][$channel]=0.0}
   $groups[$key][$channel]+=[double]$engine.UtilizationPercentage
  }
 }
 foreach($m in $memory) {
  if($m.Name -match 'luid_0x([0-9a-fA-F]+)_0x([0-9a-fA-F]+)_phys_(\d+)$') {
   $key=$Matches[1].ToLower().PadLeft(8,'0')+'_'+$Matches[2].ToLower().PadLeft(8,'0')+':'+$Matches[3]
   $metrics=@((Metric 'dedicated' 'Dedicated memory' 'bytes' $m.DedicatedUsage),(Metric 'shared' 'Shared memory' 'bytes' $m.SharedUsage))
   $busy=$null
   if($groups.ContainsKey($key)){
    foreach($channel in @($groups[$key].Keys | Sort-Object | Select-Object -First 16)){
     $load=[math]::Min(100.0,$groups[$key][$channel]);if($null -eq $busy -or $load -gt $busy){$busy=$load}
     $metrics+=Metric ('engine:'+$channel) ($channel -split ':',2)[1] '%' $load 100
    }
   }
   $metrics=@((Metric 'load' 'Busiest engine' '%' $busy 100))+$metrics
   $devices+=@{id='gpu:'+$key;kind='GPU';name='GPU '+$key;at=(Stamp);fields=@((Field 'History scope' 'Current Windows session'));metrics=@($metrics);warning=''}
  }
 }
} catch {$warnings+='GPU adapter counters unavailable'}

try {
 $processors=@(Get-CimInstance -ClassName Win32_Processor -OperationTimeoutSec 8 -ErrorAction Stop | Select-Object -First 8)
 $cpu=Get-CimInstance -ClassName Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'" -OperationTimeoutSec 8 -ErrorAction Stop
 $devices+=@{id='cpu:system';kind='CPU';name='All processors';at=(Stamp);warning='';fields=@((Field 'Models' (($processors|ForEach-Object Name) -join ' / ')),(Field 'Logical processors' (($processors|Measure-Object NumberOfLogicalProcessors -Sum).Sum)),(Field 'Scope' 'Whole system'));metrics=@((Metric 'load' 'CPU usage' '%' $cpu.PercentProcessorTime 100))}
 $memory=Get-CimInstance -ClassName Win32_PerfFormattedData_PerfOS_Memory -OperationTimeoutSec 8 -ErrorAction Stop
 $system=Get-CimInstance -ClassName Win32_ComputerSystem -Property TotalPhysicalMemory -OperationTimeoutSec 8 -ErrorAction Stop
 $total=[double]$system.TotalPhysicalMemory;$used=$null;if($total -gt 0 -and $null -ne $memory.AvailableBytes){$used=UsedMemory $total $memory.AvailableBytes}
 $devices+=@{id='memory:system';kind='Memory';name='System memory';at=(Stamp);warning='';fields=@((Field 'Capacity (bytes)' $system.TotalPhysicalMemory));metrics=@((Metric 'used' 'In use' 'bytes' $used $total),(Metric 'available' 'Available' 'bytes' $memory.AvailableBytes $total),(Metric 'commit' 'Committed' 'bytes' $memory.CommittedBytes $memory.CommitLimit),(Metric 'cache' 'Cached' 'bytes' $memory.CacheBytes))}
}catch{$warnings+='CPU or memory counters unavailable: '+$_.Exception.Message}

@{devices=@($devices);warnings=@($warnings)} | ConvertTo-Json -Depth 7 -Compress

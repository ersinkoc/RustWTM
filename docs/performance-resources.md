# Focused Performance views

Performance has a persistent resource selector: **All resources**, **CPU**,
**Memory**, **GPU**, **Network**, and **Disk & I/O**. The combined overview
retains its responsive one/two-column layout; a focused view gives one
resource the full content width. The selector stays above the scroll area.

The last resource is saved with window preferences. Older preferences default
to All resources, and an unknown future resource falls back to that overview.
Overview and process resource cards open the corresponding focused view.
Changing resources resets the content scroll to the top but retains the
shared timeline window and selected timestamp. Inspection is transient and
does not survive a restart. See [timeline controls](performance-timeline.md).

Previews: [CPU tier filter](../screenshots/performance-cpu.png),
[GPU engine search](../screenshots/performance-gpu.png),
[compact CPU](../screenshots/performance-cpu-compact.png),
[compact GPU search](../screenshots/performance-gpu-compact.png).

## CPU tiers

The focused CPU view offers All CPU tiers and the relative Windows performance
classes present in the current topology. Typical labels are P, E and LP-E;
topologies with more than three classes retain numeric class labels. These are
relative performance classes, not vendor model identifiers.

The filter applies to logical-processor tiles only. It preserves each tile's
original CPU index and matching history. The main CPU graph remains total
CPU usage, and a visible explanation describes that scope. This is a display
filter; it does not change affinity, policy or CPU scheduling.

Filtering requires a hybrid topology that maps every usage-array index exactly
once within one processor group. Missing, multi-group, duplicate-index or
mismatched topology falls back to all processors with an explanation. A class
that disappears also resets the selection. Filters are session-only.

## Devices

Like Windows Task Manager, Performance lists each physical disk ("Disk 0
(C:)"), each network adapter that is up ("Wi-Fi", "Ethernet") and each GPU
("GPU 0", numbered in DXGI order) in the rail and in the compact chip row, each
with its own sparkline. The rail scrolls when the devices outgrow it. The
selected device is saved with the window preferences (`performance_device`).
A device that goes away falls back to the combined view of its kind when that
is listed, otherwise to the first device of that kind. Combined network and
GPU entries appear only when Windows reports no single adapter; the combined
Disk & I/O entry always remains, because it graphs process I/O, which no
single disk measures.

Device history lives in the window (`History::devices`), one series per
measurement aligned with the shared timeline. A device absent from a sample
leaves a gap there, never a guessed value; series that hold only gaps are
dropped, and at most 512 are kept. The service's backfill has no device
history, so device graphs start when the window connects.

Every page lists Task Manager's details under its graphs: live readings as
large values, fixed facts beside them, and a Copy details button.

- **CPU**: utilization, current speed (`% Processor Performance` × nominal
  frequency), processes, threads, handles, up time, kernel, DPC and interrupt
  time, interrupts, context switches and system calls per second, processor
  queue; model, base speed, sockets, cores, logical processors,
  virtualization, L1/L2/L3 cache (`GetLogicalProcessorInformationEx`).
- **Memory**: compressed memory (the Memory Compression working set) and, from
  the SMBIOS memory devices, speed, slots used, form factor, memory type and
  each module, beside the existing breakdown.
- **Disk**: active time and transfer-rate graphs; average response time and
  queue length from the PhysicalDisk counters; model, SSD/HDD, bus and
  capacity from read-only storage queries on a handle opened without access
  rights; formatted size, system disk, page file and volumes. For internal
  solid-state drives (NVMe, or no seek penalty on a non-USB bus), while the
  page is open, every 5 seconds: the drive temperature with its warning and
  critical limits (`StorageDeviceTemperatureProperty`) and, for NVMe drives,
  the health log (remaining endurance, spare, power-on hours, data read and
  written, unsafe shutdowns, media errors). Hard disks and USB drives are not
  asked: on a spinning drive the query becomes an ATA command that can spin
  it up for seconds on the worker that also carries out commands.
- **Network adapter**: its own receive/send graph, bit rates, link
  utilization, adapter name, connection type, link speed, MAC address,
  addresses and DNS suffix (`GetAdaptersAddresses`); Wi-Fi adds SSID, signal and standard from
  the WLAN service. From Windows 11 24H2 these Wi-Fi details depend on location
  access, and Windows may ask for it the first time an app reads them, so they
  are read only while a Wi-Fi adapter's page is open; without access they are
  left out.
- **GPU**: utilization (busiest engine type of that GPU), four engine graphs
  (3D, Copy, Video Encode, Video Decode first), dedicated and shared memory
  graphs scaled to the card's capacity (`GPU Adapter Memory` counters),
  temperature, fan speed and memory clock with the rated maximums (the
  driver's performance data, `D3DKMTQueryAdapterInfo` with
  `KMTQAITYPE_ADAPTERPERFDATA`, read with each sample as Task Manager does;
  drivers that report none show none), model, driver version and date,
  DirectX feature level and the engine search. The rail shows the temperature
  beside the load.

The sample carries each adapter's traffic and each GPU's load and memory
(IPC protocol 17). The facts are read by the window's worker, and only from
sources that answer at once, since the same thread carries out commands:
processor and memory-module facts once; addresses every 5 seconds; disk
identity and the page files (`ExistingPageFiles` in the registry) when the set
of disks changes; the free space of the local disks' own drive letters every
minute (never network drives); GPU facts every minute or when the set of GPUs
changes. None of them needs administrator rights, and all lists are bounded.

## Remaining scope

Persistent system and device history (M4), user-selectable GPU engine graphs,
NPU pages, rate-axis hysteresis and a choice of network units remain pending.
Battery and motherboard sensor readings stay on the Sensors and Devices pages.

## Local validation

The 2026-11-09 GUI suite passed 217 tests with two existing ignored visual
tests. The added interactive checks cover resource switching with a pinned
timeline, saved/default/future preferences, P/LP-E filtering, ambiguous topology
fallback, case-insensitive GPU search, idle engines, no matches and missing
engine counters. Existing Overview and process-card navigation also passed.

Clippy for all GUI targets/features passed with warnings denied, as did
formatting, GUI build and private-item documentation with warnings denied.
The serial GPU review passed with 180 images: five languages, two themes,
two sizes, all five focused resources and the existing live/inspect/gap/stale
states. Compact fixtures scroll to the CPU filter and GPU search, assert that
the controls are on screen, and verify that clicking/typing changes state.
Unrelated power-control renders were explicitly excluded from this scoped
run. Fixtures use synthetic measurements and never exhaust host resources.

These are source/local GUI checks; installed application, service, hosted CI
and release verification remain separate.

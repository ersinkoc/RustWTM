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

## GPU engine types

The focused GPU view displays all current engine types, including idle ones,
ordered by utilization, and offers a case-insensitive name search. No matches
and unavailable counters have separate messages. The combined overview still
shows the busiest four types and summarizes the remainder.

Engine counters aggregate by engine type across GPUs; the main history follows
the busiest engine. This view does not identify physical GPU adapters or
provide per-device engine history or VRAM measurements. Search is session-only.

## Memory, network and disk

Memory retains its aggregate usage history and current available/committed
figures. Network explicitly identifies combined adapter traffic. Disk & I/O
explicitly distinguishes total process I/O history from the current physical
disk counters below it. Changing the selected resource does not relabel
aggregate samples as device-specific measurements.

## Remaining scope

This completes an initial focused-resource presentation within M3. Physical
device selection and stable device identities, per-adapter/per-device history,
GPU engine history grids and VRAM, rate-axis hysteresis and alternative network
units remain pending. M4 still owns persistent system history. No collectors,
IPC contracts, service settings or privileged operations were added here.

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

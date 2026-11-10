# Desktop capability review — 2026-11-10

Baseline: RustWTM 0.27.0 plus the current workspace changes. This review
separates presentation improvements from missing measurement and persistence
capabilities. A decorative card does not implement its underlying data source.

| Area | Current state | Concrete next work and invariant |
| --- | --- | --- |
| Empty inspector | System overview implemented in this workspace | Saved adaptive/always/hidden modes; only snapshot facts, no guessed model names |
| Global context | Resource strip and system totals implemented | Preserve full system totals under process filtering and stale-data warnings |
| Table hierarchy | Larger values, grouped view selector, quiet rows and two-level headers implemented | Keep sortable labels visible, selected rows reachable and keyboard names stable |
| Column movement | Profiles, searchable editor, keyboard ordering and persistence exist | Add pointer drag ordering with an equivalent keyboard path; preserve hidden-column order |
| Sticky identity | Process names still scroll with the full table | Pin name/selection/expansion in one coordinated vertical viewport; test horizontal scrolling with a selected row |
| App categories | Apps groups instances by executable image name | Discover real user-facing window ownership; distinguish apps, background and system processes without changing collection or policy membership |
| Group inspector | Individual process identity is supported | Add a separate app-group selection key, live member list, member navigation and explicit aggregate semantics; do not use one PID as group identity |
| CPU inventory | CPU model and manufacturer, logical counts and relative performance classes exist | Add capability-aware clock/thermal readings through the existing provider |
| Memory inventory | Physical use, availability, commit and bounded CIM module/model/speed inventory exist | Add total-slot and memory-type metadata where available; unknown fields remain unknown |
| GPU devices | Adapter models, aggregate busiest-engine data and engine-type search exist | Add stable adapter keys, per-device utilization/VRAM and independently timestamped engine series before device grids |
| GPU chart layout | Aggregate plot is available | Focus/grid/rows layouts and bounded busy-engine selection after real per-engine histories exist |
| Disk identity | Physical counters, index-matched disk models and separate process-I/O aggregate exist | Stable physical device keys, volumes and reconnect fallback; never relabel process I/O as one disk's throughput |
| Disk health | No read-only health capability exposed | Provider-owned ATA/NVMe/SCSI queries with bounded byte decoders and explicit unsupported/access-denied/stale states |
| Battery | No battery overview | Device identity, absent/present/unknown, charge versus wear, power direction and remaining-time uncertainty |
| Chart motion | Timestamp cursor, pinning, zoom and gaps exist | Viewport motion separate from samples; reduced-motion choice and no interpolation across missing data |
| System history | Only recent aggregate samples persist in memory | Additive SQLite system-minute schema, bounded worker requests, retention and coverage; retain one recorder/database owner |
| Historical attribution | Per-app minute history exists | Select a recorded minute and show its busiest apps with real identity/coverage; no reconstruction of unrecorded times |
| File ownership | File I/O tracing and diagnosis exist, ownership scan does not | Restart Manager plus bounded helper-based handle lookup; cancellation, time budgets, creation-time identity and explicit partial results |
| Folder/drop entry | Disk analysis exists | Route a dropped path to ownership versus space analysis explicitly; cap enumeration and protect the GUI from blocked filesystem calls |
| Background refresh | Service/governor and GUI polling already exist | Separate visibility-aware telemetry from enforcement cadence; protect rescue/governor responsiveness |
| Window hotkey | Existing tray/window behavior | Owned native registration, conflict feedback, configurable chord, key-repeat handling and cleanup |
| Onboarding | Settings and capability messages exist | Explain service/elevation and monitoring versus protection before any opt-in changes; handle missing providers |
| Update presentation | Verified updater and release checks exist | Add accessible release-note presentation and explicit channel policy; preserve digest/provenance gates |
| Native surface | Existing egui renderer, palette and theme switching | Improve hierarchy, spacing and contrast in the current stack; native backdrop is an optional capability, not a framework migration requirement |

## Implementation order

1. Finish process identity usability: sticky name, proper group inspection and
   window-based categories. Validate PID reuse, exits, filters and open menus.
2. Extend inventory/capability models for CPU, DIMMs, adapters, disks and battery.
   Add pure decoder/provider seams before physical-device presentation.
3. Extend SQLite history and historical attribution with explicit coverage,
   clock-change and service-version behavior.
4. Add bounded file ownership and drop workflows through dedicated workers and
   disposable helpers; never enumerate foreign handles on the GUI thread.
5. Improve background telemetry, hotkey ownership, onboarding and release notes.

Each stage needs its own executable evidence. Host resource exhaustion,
unbounded thread/handle creation and machine-changing installer proofs are
excluded from developer-host validation.

The current workspace implementation is described in
[Process workspace](process-workspace.md); the broader acceptance gates remain
in the [desktop experience plan](desktop-experience-plan.md).

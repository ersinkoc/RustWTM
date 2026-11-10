# Hardware experience and product direction

The later [inventory and application-management sequence](inventory-app-management.md)
updates the delivery order: specifications, installed apps and uninstall, then
package updates. The disk-health/battery/device-graph stages below remain a
hardware backlog, not the immediate next delivery.

Baseline: 0.27.2, reviewed 2026-11-10. The next major feature priority is hardware:
disk health, battery and device-specific graphs. This plan extends the
[desktop experience plan](desktop-experience-plan.md) and supersedes its delivery
order for hardware work. Proposed items below are not shipped capabilities.

## Product outcome

A person should be able to select a physical device, recognize its model, see
what is measured and when, understand an abnormal reading, and open the relevant
details without losing the current selection. Design must make those answers
clear before adding more counters.

Keep the current Rust/egui renderer and square geometry. Replacing the UI
framework is not a dependency. Use real data and original components.

## Delivery sequence

| Stage | Visible result | Implementation boundary | Acceptance |
| --- | --- | --- | --- |
| 0: Find and identify | Searchable page navigation; RAM generation and module form factor in existing system identity cards and copied summary | Local GUI state; bounded existing inventory worker; no IPC changes | Ctrl+K, click, arrows, Enter, Escape; all five languages; drafts preserved; unknown memory codes remain unknown |
| 1: Hardware workspace | Device list with CPU, memory modules, disks, adapters and battery; selected device shows model, capabilities and source | Stable typed device IDs; separate slow discovery from samples; reconcile refresh by ID, never row order | Two identical model names, reorder, unplug/replug, unavailable provider, permission denial and stale data fixtures |
| 2: Disk health | Per-drive Windows health and supported reliability readings, including temperature/wear/error counters when available | Read-only Windows storage provider worker; explicit mapping of storage identity to physical disk counters; no model-name joins; no storage mutations | Healthy/warning/unhealthy/unknown remain distinct; unsupported SMART is not healthy; USB/RAID/virtual drives and partial fields covered |
| 3: Battery | Present/absent/unknown state, charge, AC/charging state and trustworthy remaining-time estimate; optional capacity/wear details | System power status for current state; separate battery device metadata; unknown sentinels decoded before UI; per-pack data kept distinct from aggregate | Desktop with no battery, multiple packs, AC transition, fully charged, unknown estimate, missing capacity and failure seams |
| 4: Device graphs | Select one disk, adapter or GPU; plots and metadata refer to that exact device; overview/focus/grid layouts | Timestamped bounded series keyed by device and metric; per-adapter GPU engine and memory ownership first; protocol version negotiation for new shared data | Switching devices never reuses another history; removal and gaps are explicit; scaling/units remain stable; keyboard and pointer paths equivalent |
| 5: Saved hardware layout | Remember selected device, visible metrics, density and chart layout; reset to defaults | Additive preferences with stable IDs and migration defaults; no translated keys | Old preference payload, removed device, malformed value, reset and restart |

Stage 0 is implemented and locally verified in the current workspace: 242 GUI
tests passed, workspace Clippy and private rustdoc passed, the native GUI build
passed, and 24 new navigation screenshots passed PNG validation. The separate
read-only inventory smoke returned actual DDR5/DIMM metadata on this machine.
The initial navigation test failed before implementation; the unchanged panic
shortcut test also caught an extra-modifier collision, which was fixed. Stages 1–5 remain planned. Disk health, battery and per-device GPU
graphs are not implemented by the stage 0 changes.

### First major delivery: disk health

1. Introduce a typed `DeviceId`, discovery result and capability status in the
   core hardware model. Use separate `available`, `unsupported`, `denied`,
   `failed` and `stale` states. Keep error detail separate from display identity.
2. Build a bounded storage-provider query behind a replaceable read seam. The
   GUI consumes cached results; it never queries a drive during a frame. Use a
   single in-flight discovery, at most 64 devices, a 512-KiB response cap and a
   10-second request deadline as initial limits. Cancel on shutdown; retry by
   explicit refresh, not an unbounded queue.
3. Read selected properties from `MSFT_PhysicalDisk` and available reliability
   counters. Preserve provider units and validity. Windows health is a provider
   assessment, not proof of complete SMART coverage or remaining lifetime.
4. Resolve the mapping to physical performance counters through a documented
   device association/native identity. Until it is proven, show health in its
   own device list rather than attaching it to a guessed disk index.
5. Add a device details view: identity header, concise health explanation,
   source/update status, supported readings and expandable raw diagnostics.
   Unsupported counters stay absent/unavailable, not zero.
6. If shared through the service, add an additive protocol payload and version
   handshake tests before the GUI starts relying on it. Retain one telemetry
   owner; no second database recorder.

### Battery semantics

Charge percentage and battery wear are different values. Derive wear only when
both design and full-charge capacities are valid and refer to the same pack.
Do not derive a lifetime estimate from a single sample. Keep OS estimates marked
as estimates. Disappearance, API failure and a confirmed no-battery machine are
three different states. Initial battery delivery is read-only.

## Visual changes paired with these features

| Surface | Intended treatment | Review gate |
| --- | --- | --- |
| Page shell | One heading, relevant actions and a concise source/freshness line | Same left edge and hierarchy across all views |
| Hardware navigation | Device names, short live readings and small resource-colour markers; one clear selected device | Long names, identical models, narrow rail and large DPI |
| Device details | Identity at the top, primary reading next, graphs, then technical facts in a labeled grid | No repeated model blocks, no enormous empty graph used as decoration |
| Charts | Fine grid, restrained fill, legible units, time cursor, gap/stale treatment | Shared scale semantics; zero and unavailable visually distinct |
| Settings | Descriptive rows grouped by task, aligned controls, switches for booleans and sliders with editable values | Five languages, keyboard focus, compact layout and disabled reasons |
| Tables | Stable identity column, full-cell sortable headers, useful row actions and reduced line noise | Horizontal scrolling, selection, open menus and refresh together |
| Empty/error states | Explain what is missing and the action that can help | No empty boxes, unexplained dash or invented success state |

Do not repaint the whole product blindly. Review each main screen in both themes
at 1440×1000 and 820×500; review new dialogs down to 420 points. Use 100%, 150%
and 200% DPI with representative long translated labels. Capture normal,
loading, empty, failed and stale states, then fix visible defects before moving
to the next surface. Automatic pixel comparison is not human visual acceptance.

## Later product work

After the hardware sequence: stable process-name column and app-group inspection,
historical attribution of slow periods, previewed support export and configurable
dashboard cards. Existing process limits, monitoring/protection and cooling
controls retain their authorization and confirmation rules.

## Verification and delivery

- Run decoder fixtures and provider failure seams before native probes. Never
  exhaust the developer host or enable installer mutation tests locally.
- GUI tests use at most two threads; GPU captures run serially. Keep synthetic
  fixtures bounded. No physical fan, drive or power-setting writes are needed.
- Run format, workspace Clippy, targeted/full GUI tests, private rustdoc and a
  native build. Record actual results separately from the plan.
- Read-only hardware smoke proves only this machine's reported capabilities;
  unsupported models require their own evidence.
- A future feature release requires exact-commit hosted CI, VM installation and
  upgrade checks, published asset checksums/provenance and downloaded smoke.

## Primary implementation references

- [Windows physical memory fields](https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-physicalmemory): WMI form factor, configured clock units and raw SMBIOS memory type.
- [SMBIOS type definitions maintained by TianoCore](https://github.com/tianocore/edk2/blob/master/MdePkg/Include/IndustryStandard/SmBios.h): raw memory-type codes; WMI form-factor codes use a different enum.
- [Windows physical disk properties](https://learn.microsoft.com/en-us/windows-hardware/drivers/storage/msft-physicaldisk): provider health and operational states.
- [Windows system power status](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getsystempowerstatus): current power/battery state; per-pack wear needs additional metadata.

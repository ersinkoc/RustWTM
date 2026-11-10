# Product expansion implementation

All work is original. This plan records requested scope and verification status;
a planned item is not an implemented feature.

| Workstream | Deliverable | Status |
|---|---|---|
| Device workspaces | Device selection, recent CPU/RAM/disk/GPU/network/battery charts, source/freshness | Implemented; final review in progress |
| Hardware health | Read-only Windows reliability counters and native NVMe health log; battery capacities/rate/cycles | Implemented; battery and controller coverage require additional hardware |
| Applications | Current-user Store inventory, exact-ID WinGet discovery and reviewed selected updates | Implemented; discovery verified locally, installer execution requires disposable VM validation |
| Incident history | Bounded persistent system/top-process replay, protection-event correlation, reviewed JSON copy | Implemented; persistence and failure-path tests included |
| File locks | File/folder input/drop, Restart Manager owners and process navigation | Implemented with explicit partial-result limits; full handle/current-directory enumeration remains future work |
| Policies | Visual rule editor, pure-engine preview, reviewed Work/Game/Battery profiles | Implemented; profile labels describe RustWTM policies, not Windows power plans |
| Experience | First-run guide, shortcut help, opt-in global show/hide, window refresh preferences, chart network units | Implemented; native global shortcut activation is not automated on the developer desktop |
| Numeric tray | Independently enabled CPU/RAM percentages and selectable temperature, hidden-window updates, saved preferences | Implemented; selection/render/settings tests, native shell display remains a manual check |

Existing inventory, application removal, process lists, services, startup controls,
endpoint closing and the protection engine remain the base. Reuse those flows
instead of presenting duplicates as new features.

## Behavior and limits

- Hardware graphs retain up to 180 samples per metric while the page is sampled.
  Automatic five-second refresh is opt-in. Provider queries run in a background
  worker; a failed provider does not turn an unavailable reading into zero.
  Network rates require two monotonic counter samples. Disk identity matching
  is exact; ambiguous Windows storage identities do not inherit health data.
- NVMe uses only the read-only storage-property query for the SMART/health log.
  Controller support and permissions vary. Lifetime data-unit counters are
  displayed exactly; percentage used may exceed 100, while remaining endurance
  is clamped at zero. Windows reliability counters also cover supported SATA
  drives. No universal SATA SMART pass-through or physical health guarantee.
- The device selector includes a bounded sparkline and current reading. Disk
  Details separates health, endurance, temperature, available spare, lifetime
  counters, hardware identity and raw health fields. A nonzero critical-warning
  bitset overrides a healthy Windows status in the health summary. GPU Overview
  has stable engine positions and separate dedicated/shared-memory graphs;
  selecting a metric opens its focused chart. Memory composition requires both
  used and available values; unavailable components are never fabricated.
  NVMe byte conversion uses checked multiplication, with the original data-unit
  counter retained in the health details. The log layout and units follow
  [Microsoft's NVME_HEALTH_INFO_LOG documentation](https://learn.microsoft.com/en-us/windows/win32/api/nvme/ns-nvme-nvme_health_info_log).
- WinGet uses the official signed Microsoft.WinGet.Client binary. It is optional;
  discovery reports its absence. Elevated windows require an all-users module.
  Updates are limited to 16 reviewed exact source/ID/version selections and are
  rechecked immediately before the installer request. Installer results require
  a subsequent inventory refresh. No silent or unattended bulk updating.
- Service incident frames are recorded every five seconds, retained for seven
  days and bounded by a 256 MiB frame-body budget. At most 21 contributors are
  retained per moment. Replay returns at most 600 representative moments and
  the latest 512 action markers; truncation and gaps are visible. It records
  observations, not causal diagnoses. No command lines or paths are recorded.
- File investigation scans local files, up to 128 files in a folder. Reparse
  points and network locations are excluded. Restart Manager has a fixed
  512-owner ceiling. Native calls can outlast the cooperative traversal budget;
  the UI stays responsive. Results are a snapshot and never prove a file is free.
- Rule simulation uses the pure policy engine and cached process details. It
  assumes the selected sample persists for the configured duration. Saving
  still requires service administrator access and a review of unchanged base
  configuration. Existing dry-run and scope validation remain in effect.
- Window refresh preferences affect repaint requests, not service sampling or
  enforcement. Network-unit selection currently applies to device charts.
  Global Ctrl+Alt+F2–F12 is disabled by default and reports registration conflicts.
- Settings / This window includes optional numeric tray icons. CPU and RAM show
  integer percentages, temperature shows integer Celsius, and unavailable/stale
  readings show `--`. Automatic temperature uses a reported CPU sensor; an exact
  sensor ID can be selected instead. CPU temperature requires supported provider
  telemetry. The enabled icon refresh interval (1–30 seconds) requests background
  logic ticks while hidden without forcing hidden-window painting. A temperature
  icon requests existing read-only sensor refreshes every two seconds. Each icon
  owns one shell registration, updates its bitmap only when the integer changes,
  and is dropped when disabled. Windows manages pinned/overflow placement. Failed
  creation is reported and retried when toggled. The logo remains available if
  no number icon exists. Native shell activation is not automated on the host.

Acceptance: tests use synthetic fixtures and injectable boundaries. No host
resource exhaustion, real automated uninstall/update, hardware writes or service
installation. Native read-only smoke is separate from mutation proofs; privileged
installation/update tests belong in disposable VMs. Each page must handle empty,
loading, partial, unavailable, denied and stale states, narrow widths, both themes,
keyboard operation and the supported interface languages. Update discovery retains
repository identity 1425760656 and existing installer/service identities.

The regular headless workspace review covers the localized layout matrix.
The optional GPU review defaults to four captures on the developer machine.
Set `RUSTWTM_VISUAL_VM=1` only in a disposable VM for a complete GPU matrix;
never combine a long native graphics capture run with local compilation. A
compiler allocation failure was observed during concurrent capture/compilation;
the capture completed, but that is not evidence that the combined workload is
safe for this host.

Focused visual review can select pages with `WTM_EXPANSION_PAGES` and exact
`theme:size:page` cases with `WTM_EXPANSION_CASES`, for example
`light:wide:gpu,dark:compact:memory`. Both use comma-separated values. Ordinary
headless coverage still exercises the complete default layout matrix.

Native TCP enumeration keeps unknown state zero unless the same local endpoint,
remote endpoint and PID also appear in the OS-filtered listener table. This uses
the documented [TCP_TABLE_OWNER_PID_LISTENER contract](https://learn.microsoft.com/en-us/windows/win32/api/iprtrmib/ne-iprtrmib-tcp_table_class),
not a port/address heuristic. Failed confirmation stays unknown. Table buffers
and resize attempts are bounded; failure-path proofs use injected replies.

## Local verification, 2026-11-10

The device layout update was checked on base `2568bcb` plus the working changes:

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`: passed.
- `cargo test --locked --workspace --no-fail-fast -- --test-threads=2`:
  779 passed, 22 ignored, zero failures. Both previously failing native listener
  and port-enumeration tests passed without changing their assertions.
- Synthetic GPU captures checked disk details in narrow/light and wide/dark
  layouts, GPU overview in wide/light and memory in narrow/dark. After spacing
  corrections, the wide GPU and disk captures were rendered and inspected again.
  These captures use fixtures, not a claim of controller or sensor compatibility.
- A separately named x64 development preview was built and its source hashes,
  PE architecture, version metadata and SHA-256 recorded under
  `.temp_files/product-expansion/device-preview-receipt.json`. It was not
  launched and is not a published release or installer validation.

Verification logs remain in the ignored `.temp_files/product-expansion/`
directory. Hardware coverage, native tray interaction and installer execution
retain the limits described above.

# CPU topology, affinity and frequency policy

Implemented in 0.13.1, addressing
[issue #1](https://github.com/ersinkoc/RustWTM/issues/1). IPC protocol 11 adds
core-tier control; upgrade GUI and service together. The CLI also works locally.

## Discover and select cores

```powershell
wtm cores
wtm cores --json
wtm affinity 1234 --pcores
wtm affinity 1234 --ecores
wtm affinity 1234 --lp-ecores
wtm affinity 1234 --all-cores
```

The topology includes CPU Set IDs, processor groups, logical/physical core
indices, raw EfficiencyClass values, parking and reservation state. Higher
Windows efficiency classes indicate faster, less power-efficient processors.
RustWTM ranks distinct classes instead of assuming values 0 and 1. For two
tiers it labels P/E; for three tiers P/E/LP-E. These are relative performance
labels, not verified vendor silicon model names. More than three classes are
shown by class number; ambiguous efficient-tier shortcuts are unavailable.

On three-tier CPUs, `--ecores` selects the middle tier; `--lp-ecores` selects
the lowest tier. Homogeneous CPUs still support listing and `--all-cores`,
but hide P/E controls. In Processes, right-click a process → CPU core type.
Performance charts show tier labels when the sample maps to a single group;
multi-group charts retain untyped logical-processor labels.

Affinity uses the target process's actual processor group and system mask.
It excludes CPU Sets reserved for other processes, retains SMT logical
processors, validates PID creation time and rejects protected/critical
processes. A process spanning multiple groups receives an explicit unsupported
error: RustWTM never folds a group-1 index into a group-0 mask. `--all-cores`
selects all currently eligible processors in the process's group; it does not
restore a previous custom affinity. These are one-shot process settings.

## Prefer efficient execution for one app

EcoQoS and automatic name-based EcoQoS rules already existed before this issue:

```powershell
wtm eco 1234 on
wtm eco 1234 auto
```

```toml
[[rule]]
name = "Efficient background workers"
match = { name = "python.exe", cmdline = "worker" }
actions = ["eco_qos"]
```

Use this route for a per-process efficiency preference. A restrictive affinity
mask still constrains where Windows can schedule the process.

## Shared frequency policy

Sensors & cooling → CPU frequency policy shows the active plan's AC/DC values.
The same controls are available through:

```powershell
wtm power show
wtm power show --json
wtm power limit --class 0 --source ac --mhz 1800 --yes
wtm power restore --yes
```

Writes require administrator rights and explicit confirmation. `--source` is
`ac`, `dc` or `both`; class is Windows power policy class 0 or 1. The supported
request range is 100–64000 MHz. Existing zero values are shown as Windows
default and are preserved for restoration. Unsupported settings stay unavailable.
Policy class 1 is not automatically E-cores, and CPU Set efficiency classes
are not assumed to map directly onto these two power settings.

This changes a shared Windows power plan and can affect every application
scheduled on the affected processors. It is not a per-process MHz cap, and
firmware may ignore or constrain the request. These explicit local controls
are independent of the governor's monitoring/enforcing mode. No frequency
setting is applied merely by starting RustWTM or changing an editor.

Before writing, RustWTM locks its operation file and saves original settings
to `%ProgramData%\RustWTM\PowerLimits\original.json`, with administrator/SYSTEM
write access. Only one saved limit is allowed at a time. The request is bound
to the displayed active plan; a plan change before execution is rejected.
Partial failures attempt rollback; if restoration fails the journal is kept
for a later Restore. Restore operates on the saved plan without switching to
it if another plan is active. Detected external value changes cause restoration
to stop rather than overwrite those values. Windows exposes no transaction
shared with other power-management apps, so external races cannot be eliminated.

Limits persist after the app closes. Restore before uninstalling; the saved
ProgramData journal is retained with the application's data. No power-plan
setting was changed on the development workstation during implementation.

## Validation boundaries

Tests cover two/three-tier and homogeneous topologies, group-relative masks,
malformed/forward-compatible CPU Set records, reservations, PID protection,
CLI conflicts, GUI selection/confirmation/privilege checks, durable original
values, partial rollback, rollback retry, cross-plan restore, external edits
and concurrent operation exclusion. Live discovery and reading the current
power plan are read-only. Physical hybrid P/E mapping, multi-group hardware
and the firmware's response to MHz requests still require hardware validation.

Primary references: [CPU Set information](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-system_cpu_set_information),
[CPU Sets and affinity](https://learn.microsoft.com/en-us/windows/win32/procthread/cpu-sets),
[EcoQoS](https://learn.microsoft.com/en-us/windows/win32/procthread/quality-of-service),
[maximum frequency settings](https://learn.microsoft.com/en-us/windows-hardware/customize/power-settings/options-for-perf-state-engine-maxfrequency).

### Development verification, 2026-10-21

Starting revision: `19f80df` (published 0.13.0), initially clean. Local development
version: 0.13.1. The development workstation reports 16 physical / 32 logical
processors, one processor group and one efficiency class (0), so live P/E
selection cannot be validated on it. AC/DC settings for both power policy
classes read as 0; no real frequency setting was changed.

| Check | Observed result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --workspace --quiet` | 382 passed, 0 failed, 3 ignored |
| `cargo test -p wtm-gui --features visual-tests render_ -- --nocapture` | 3 passed after combining GPU reviews into one test; 24 PNGs under `target/visual-review` |
| Live `wtm cores --json` and `wtm power show --json` | PASS, read-only |
| `wtm power limit` without `--yes` | Rejected before attempting a write |
| Hardware-provider locked publish/self-test | PASS; fake leases only |

The initial parallel GPU review exited with native status `0xc0000005`.
The same reviews passed with one test thread. Keeping GPU review contexts in
one test also passed with the default test runner; no production GPU change
was made. The exact native crash origin was not established.

Ignored tests cover live GitHub updater verification, asset regeneration and
live Telegram. SCM lifecycle is gated off locally; installed-service-dependent
test bodies may return early. Real MHz writes/firmware response, hybrid hardware,
multi-group hardware and a matching-service upgrade are **NOT RUN**. Local
release and package hashes are recorded in `target/issue-1-review/verification.json`
after the final build; this document does not certify installation or publication.

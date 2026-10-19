# RustWTM — Implementation Plan

A full-featured Windows Task Manager written in Rust, with a background
governor that keeps the machine from being pinned at 100% CPU by throttling,
limiting, suspending or killing offending processes, and that can block
processes from running at all.

## Decisions

| Topic | Decision |
|---|---|
| Toolchain | Rust 1.99.1 (pinned in `rust-toolchain.toml`), edition 2024 |
| GUI | `eframe` / `egui` + `egui_plot` + `egui_extras` |
| UI language | English |
| Scope | Single local machine (no remote management) |
| Process blocking | ETW start-trace + IFEO first; kernel driver deferred (optional, Phase 8) |
| Win32 bindings | `windows` crate (official), all `unsafe` confined to `wtm-sys` |

## Architecture

```
┌────────────────────┐   Named pipe IPC   ┌────────────────────────────┐
│  wtm-gui (egui)    │ ◄────────────────► │  wtm-service (SYSTEM)      │
│  user session      │   (serde/postcard) │  • Process sampler         │
└────────────────────┘                    │  • CPU governor            │
┌────────────────────┐                    │  • Rule engine             │
│  wtm-cli           │ ◄────────────────► │  • Blocking (ETW / IFEO)   │
└────────────────────┘                    │  • Event log (SQLite)      │
                                          └────────────────────────────┘
```

Enforcement lives in the service so it keeps working when the GUI is closed.

### Workspace crates

| Crate | Responsibility |
|---|---|
| `wtm-sys` | Thin, safe wrappers over Win32 / NT APIs. The only crate with `unsafe`. |
| `wtm-core` | Process model, sampler (CPU %, rates), process tree, actions |
| `wtm-rules` | Rule engine, TOML config, hot reload |
| `wtm-ipc` | Protocol types, named-pipe client/server |
| `wtm-service` | Windows service, governor, watchdog |
| `wtm-gui` | egui front-end |
| `wtm-cli` | `wtm list`, `wtm top`, `wtm kill`, `wtm limit`, ... |

## Data collection

- Process list: `NtQuerySystemInformation(SystemProcessInformation)` — one call
  returns every process with CPU times, memory, handle/thread counts and I/O
  counters (same source Windows Task Manager uses).
- CPU %: Δ(kernel + user time) / Δ(wall time) / logical CPU count.
  Processes are keyed by `(pid, create_time)` to survive PID reuse.
- System CPU: `GetSystemTimes`; per-core: `SystemProcessorPerformanceInformation`.
- Memory: `GlobalMemoryStatusEx`, `GetPerformanceInfo`.
- Details: image path, command line, owner (token), signature (`WinVerifyTrust`),
  modules, threads, handles.
- Network: `GetExtendedTcpTable` / `GetExtendedUdpTable`; bandwidth via ETW.
- GPU: PDH "GPU Engine" counters.
- History: in-memory ring buffers for charts; SQLite for long-term history.

## Control actions (escalation ladder)

| Level | Mechanism | API |
|---|---|---|
| 1 | Lower priority | `SetPriorityClass` |
| 2 | Efficiency mode (EcoQoS) | `SetProcessInformation(ProcessPowerThrottling)` |
| 3 | Restrict cores | `SetProcessAffinityMask` / CPU Sets |
| 4 | Hard CPU cap | Job object + `JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP` |
| 5 | Memory cap | Job object `ProcessMemoryLimit` |
| 6 | Suspend / resume | `NtSuspendProcess` / `NtResumeProcess` |
| 7 | Kill / kill tree | `TerminateProcess` + tree walk |

## CPU governor ("never 100%")

1. **Reservation (optional):** non-whitelisted processes are restricted via CPU
   Sets so one core always stays free for the system, the service and the GUI.
2. **Reactive governor:** sliding-window detection (e.g. system > 90% for 5 s),
   select top offenders, apply the ladder step by step with hysteresis and
   cooldowns, and roll limits back when load recovers. The service itself runs
   at high priority.
3. **Safety:** a hard-coded protected list (System, smss, csrss, wininit,
   services, lsass, winlogon, dwm, RustWTM itself); PPL processes handled
   gracefully; **dry-run mode is the default** on first install; configurable
   policy for the foreground app.

## Rule engine (TOML)

```toml
[[rule]]
name = "Limit Chrome renderers"
match = { name = "chrome.exe", cmdline_contains = "--type=renderer" }
when  = { cpu_above = 40, for_secs = 10 }
actions = ["eco_qos", { cpu_cap = 25 }]

[[rule]]
name = "Block miners"
match = { path_glob = "**\\xmrig*.exe" }
actions = ["block"]
```

Matchers: name, path, SHA-256, signer, user, parent, command line.

## Process blocking

| Method | Effect | Phase |
|---|---|---|
| ETW `Microsoft-Windows-Kernel-Process` start event → terminate | Reactive (ms) | 5 |
| IFEO `Debugger` registry value | True prevention by image name | 5 |
| AppLocker / WDAC policy | Enterprise-grade | optional |
| Kernel driver (`PsSetCreateProcessNotifyRoutineEx`) | Pre-start prevention, needs EV signing | 8 (optional) |

## GUI tabs

Processes (tree, grouping, search, all actions in context menu) · Performance
(CPU per core, memory, disk, network, GPU) · App history · Startup apps ·
Users · Details · Services · Rules & Governor · Event log · tray icon and
toast notifications.

## Roadmap

| Phase | Scope | Deliverable |
|---|---|---|
| 0 | Workspace, lints, toolchain, `wtm-sys` skeleton | Builds clean |
| 1 | Process enumeration, CPU/memory/disk sampling | `wtm list`, `wtm top` |
| 2 | Actions: kill, kill tree, suspend, priority, affinity, EcoQoS, job caps | `wtm limit <pid> 20` |
| 3 | Windows service, governor, rule engine, dry-run | MVP: machine never pinned |
| 4 | IPC + egui GUI: Processes, Performance, Details | Usable task manager |
| 5 | Blocking (ETW + IFEO), event log, notifications | Blocking works |
| 6 | Services, Startup, Users, history, disk/net/GPU per process | Feature complete |
| 7 | MSI installer (`cargo-wix`), signing, IPC ACLs, self-protection | Release |
| 8 | (optional) Kernel driver via `windows-drivers-rs` | Pre-start blocking |

## Status

- [x] Phase 0
- [x] Phase 1
- [x] Phase 2 — `wtm info|kill [--tree]|suspend|resume|priority|affinity|eco|limit`
- [x] Phase 3 — `wtm-policy` (config, rules, governor) + `wtm-service` (SCM, console mode, hot reload, rollback on stop)
- [x] Phase 4 — per-source executor (governor/rule/manual limits compose), `wtm-ipc` named-pipe protocol, egui GUI (`rustwtm`)
- [x] Phase 5 — fast start watcher (250 ms) for unconditional rules, notable events + desktop notifications, tray icon.
  Pre-launch prevention is delegated to Windows AppLocker / WDAC policies.
- [x] Phase 6 — Users, Services (start/stop/startup type) and Network tabs done; startup-apps backend in `wtm-core::startup` (no UI yet).
  App history: service records per-minute per-app usage to `history.sqlite` (retention `history_days`);
  `[insights]` flags chronic CPU and steady memory growth (slope + R² fit) and applies actions (source `Insight`);
  GUI "App history" tab and `wtm history`.
  GPU: per-process busiest-engine % and dedicated memory via PDH (Processes column, Performance graph, `wtm list`).
  Startup apps tab: list Run keys / Startup folders, enable/disable via StartupApproved (like Task Manager).
- [~] Phase 7 — done: data-dir ACL, service recovery actions, pipe server verification,
  notification AppUserModelID, install/uninstall scripts, CI and release workflows.
  Done: MSI package (WiX 5), coverage reporting in CI (98% lines; the rest is
  console/window entry points and Win32 failures that need fault injection).
  Pending: code signing (needs a certificate).

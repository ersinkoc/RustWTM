<p align="center">
  <img src="assets/logo.svg" width="128" height="128" alt="RustWTM logo">
</p>

<h1 align="center">RustWTM</h1>

<p align="center">
  <b>The Windows task manager that keeps your PC responsive.</b><br>
  Built in Rust by <a href="https://github.com/ersinkoc">Ersin KOÇ</a>.
</p>

A Windows task manager written in Rust, with a background governor that keeps
the machine responsive: it throttles, caps, suspends or terminates processes
that overload the CPU, gives apps CPU and memory budgets, frees RAM under
pressure, never slows down the app you are using, and learns from long-term
usage history.

[![CI](https://github.com/ersinkoc/RustWTM/actions/workflows/ci.yml/badge.svg)](https://github.com/ersinkoc/RustWTM/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

> **Status:** early release (0.5). Everything works, but expect rough edges.
> Enforcement is **off by default** (dry-run); review the event log first.

## Features

**Task manager (GUI `rustwtm`, CLI `wtm`)**

- Overview start page: load at a glance, one-click protection on/off and
  protection levels, the busiest apps with quick actions.

- Processes: list, tree or Apps view (one row per app, acting on all of its
  processes), program icons, search (Ctrl+F), sort by any column, choose columns,
  details panel (path, command line, user, priority, affinity, efficiency
  mode, uptime, I/O), open file location.
- Actions: end task, end process tree, suspend / resume, priority, CPU
  affinity, efficiency mode (EcoQoS), hard CPU cap and memory cap (job objects).
- Performance graphs (CPU with every logical processor, memory, GPU, network,
  disk I/O), App history, Startup apps,
  Users, Services (start / stop / startup type), Network connections, Events;
  every list sorts by any column.
- Run new task (optionally as administrator) and one-click restart as
  administrator; the window remembers its size, tab, columns and sorts.
- Settings: edit the service's configuration with live validation and a
  one-click switch between dry-run and enforcing.
- Tray icon and desktop notifications.

**Governor service (`wtm-service`)**

- **CPU governor**: when the whole system stays above a threshold, the top
  consumers are escalated step by step (lower priority → efficiency mode →
  hard CPU cap) and rolled back once the system recovers.
- **Memory guard**: when RAM stays above 90%, the biggest background apps
  release their working sets (nothing is closed or lost).
- **The app you use stays fast**: the foreground app and its child processes
  are never throttled (`protect_foreground`).
- **Rules**: match processes by name or path (wildcards), optionally require a
  sustained condition (`cpu_above`, `memory_above_mb`, `gpu_above`), then apply actions —
  including `kill`. Unconditional rules react within ~250 ms of a process
  starting.
- **App limits**: give an app a CPU and/or memory budget (`node.exe`: 2 cores;
  `zen.exe`: 8 GB per instance) shared by all of its processes and their
  children. Edit them in the **Limits** tab or with `wtm app-limit`.
- **Blocking**: programs on the `block` list are prevented from starting at
  all (Image File Execution Options), with a notification to the user.
- **Long-term insights**: per-minute, per-app usage is recorded to SQLite.
  Apps with chronically high CPU or steadily growing memory (linear trend with
  R² fit) are flagged, reported, and optionally acted on.
- **Safe by design**: dry-run by default; core Windows processes, critical
  processes and RustWTM itself are never touched; every source (governor,
  rules, insights, manual) is tracked separately and everything is rolled back
  when the service stops.

## Requirements

- Windows 10 1809 or later / Windows 11, 64-bit.
- Administrator rights to install the service and to control processes of
  other users and services.

## Install

### One-liner (latest release)

From an **elevated** PowerShell:

```powershell
irm https://raw.githubusercontent.com/ersinkoc/RustWTM/main/scripts/install.ps1 | iex
```

This downloads the latest release, verifies its SHA-256 checksum, installs to
`C:\Program Files\RustWTM`, adds it to `PATH`, creates a Start Menu shortcut,
and installs and starts the `RustWTM` service (in dry-run mode).

Options (download the script first to pass them):

```powershell
.\install.ps1 -Version v0.1.0          # a specific release
.\install.ps1 -NoService               # GUI and CLI only
.\install.ps1 -FromSource .            # build this checkout instead of downloading
.\install.ps1 -DownloadOnly -InstallDir D:\Tools\RustWTM   # portable, no admin needed
```

### MSI package

Each release also ships `RustWTM-<version>-x64.msi` (with a `.sha256`). It
installs to `C:\Program Files\RustWTM`, registers and starts the service
(with automatic restart on failure), adds the folder to `PATH` and creates a
Start Menu shortcut. Uninstall it from *Settings → Apps*; configuration and
history in `%ProgramData%\RustWTM` are kept.

```powershell
msiexec /i RustWTM-0.5.0-x64.msi            # interactive
msiexec /i RustWTM-0.5.0-x64.msi /qn        # silent
```

### Uninstall

```powershell
& "$env:ProgramFiles\RustWTM\uninstall.ps1"            # keeps config and history
& "$env:ProgramFiles\RustWTM\uninstall.ps1" -RemoveData
```

### From source

```powershell
git clone https://github.com/ersinkoc/RustWTM
cd RustWTM
cargo build --release      # the toolchain is pinned in rust-toolchain.toml
```

Binaries land in `target\release\`: `rustwtm.exe` (GUI), `wtm.exe` (CLI),
`wtm-service.exe` (service).

## Usage

### GUI

Start **RustWTM** from the Start Menu. Right-click a process for actions.
The badge in the top-right corner shows whether the GUI is connected to the
service; run it as administrator to send commands through the service.

### CLI

```text
wtm list --sort mem -n 20          one-shot listing
wtm top                            live view (c/m/d/p/n to sort, q to quit)
wtm info chrome                    details of every chrome.exe
wtm kill 1234 --tree               end a process and its descendants
wtm suspend notepad / wtm resume notepad
wtm trim chrome                    free RAM held by every chrome.exe
wtm priority 1234 below-normal
wtm affinity 1234 0-3,6
wtm eco 1234 on
wtm limit 1234 --cpu 10 --memory 2G   hard caps (Ctrl+C lifts them)
wtm history --hours 24             long-term per-app usage and findings
wtm app-limit set node.exe --cores 2 --memory 8G   persistent app budget
wtm app-limit list                 configured app limits and their usage
wtm config show                    the service's configuration
wtm config check my.toml           validate a file
wtm config apply my.toml           validate and apply it (elevated)
```

Targets are PIDs or image names (`.exe` optional, case-insensitive).

### Service

```text
wtm-service install | uninstall | start | stop
wtm-service console --data-dir .\data    run the engine in the foreground
```

## Configuration

The service reads `%ProgramData%\RustWTM\config.toml` (created with commented
defaults on first start) and reloads it automatically when it changes. Edit it
in the GUI's **Settings** tab or with `wtm config apply` (both validate first
and need an elevated client), or by hand.

```toml
dry_run = true            # log decisions only; set to false to enforce
block = ["xmrig.exe"]     # never starts at all

[governor]
high_cpu_percent = 90.0   # act when the system stays above this...
sustain_secs = 5          # ...for this long
recover_cpu_percent = 70.0
ladder = [{ priority = "below_normal" }, "eco_qos", { cpu_cap_relative = 0.5 }]

[insights]
window_hours = 24
chronic_cpu_percent = 10.0
chronic_actions = [{ priority = "below_normal" }, "eco_qos"]
growth_mb_per_hour = 200.0
growth_actions = []        # report only

[[rule]]
name = "Kill miner variants"
match = { name = "xmrig*.exe" }
actions = ["kill"]
```

See [docs/configuration.md](docs/configuration.md) for every option.

> CPU percentages are a share of the **whole machine** (all logical
> processors), the same scale the process list uses.

## Security model

- The service runs as LocalSystem. Its data directory is restricted to
  SYSTEM and Administrators (users can read).
- The GUI/CLI talk to it over the local named pipe `\\.\pipe\RustWTM`
  (remote clients rejected). Anyone can read status; **control requests require
  an elevated administrator**. Detail queries run with the caller's own rights.
- Clients verify that the pipe is served by the installed `RustWTM` service.

Report vulnerabilities as described in [SECURITY.md](SECURITY.md).

## Architecture

| Crate | Purpose |
|---|---|
| `wtm-sys` | Safe wrappers over Win32 / NT APIs — the only crate with `unsafe` |
| `wtm-core` | Process model, sampler, process tree, control actions, startup apps |
| `wtm-policy` | Config, governor, rule engine, long-term insights (pure logic) |
| `wtm-ipc` | Named-pipe protocol and client |
| `wtm-service` | Windows service: engine loop, executor, history, IPC server |
| `wtm-gui` | egui front-end (`rustwtm.exe`) |
| `wtm-cli` | Command-line interface (`wtm.exe`) |

## Development

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

See [CONTRIBUTING.md](CONTRIBUTING.md) and the roadmap in [PLAN.md](PLAN.md).

## License

[MIT](LICENSE)

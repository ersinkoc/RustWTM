# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Heat map** in the process list: CPU, memory, disk and GPU cells are
  shaded by how much of the machine they use (Columns → Heat map turns it off).
- Keyboard: ↑ / ↓ move the selection in the process list, Delete asks to end
  the selected process.
- The details panel graphs the selected process's CPU and memory over the
  last two minutes (recorded for every process, so it shows at once).
- The tray icon shows the CPU load as a bar (green, orange from 60%, red from
  90%); its tooltip adds memory use.
- Performance: activity, read and write rate of every physical disk, and the
  load of every GPU engine (3D, Copy, Video decode, ...).
- The Overview says in one sentence how the machine is doing and which app
  is behind a busy CPU or a full memory.
- `wtm list` and `wtm top` add a line with GPU, network and disk rates and
  the busiest disk's activity.
- App limits warn (event and desktop notification) when a budget uses 95% of
  its memory, since its processes then fail to allocate memory; once more
  only after it dropped below 85%.
- The Limits tab shows CPU and memory use as bars against the budget (the
  fullest instance for per-instance budgets), orange from 75%, red from 95%.

### Changed

- The Performance tab is more compact: two sections side by side on wide
  windows, shorter graphs, and only the four busiest GPU engines (the rest on
  hover).
- The Limits, Settings and App history tabs say when the installed service is
  another version (instead of "not running").
- IPC protocol version 7 (system statistics carry disks and GPU engines);
  update the GUI, CLI and service together.

## [0.4.0] - 2026-10-20

### Added

- **Apps view** in the process list (next to List and Tree): one row per app
  with the totals of all its processes, expandable to the processes, with a
  menu that acts on all of them (efficiency mode, free memory, suspend /
  resume, limit, end all with one confirmation).
- Keyboard: Ctrl+F jumps to the process search, Escape clears it.
- **Performance** tab: load of every logical processor, network receive /
  send rate and total disk / I/O read and write rate, each with a history
  graph.
- The window remembers its size and position, the last tab, the navigation
  state, the process list view, sort and columns, and the sort of every other
  table (stored in `%APPDATA%\RustWTM\data\app.ron`).
- **Run new task** (status bar): start a program, folder, document or website,
  optionally with administrator rights.
- **Restart as administrator** (status bar and Overview) when the window runs
  without administrator rights, so every process can be controlled.

### Changed

- IPC protocol version 6 (system statistics carry the new counters); update
  the GUI, CLI and service together.

## [0.3.1] - 2026-10-20

### Fixed

- App limits no longer flood the event log with `Access is denied`: processes
  that already run in a job of their own (sandboxes, build tools such as
  cargo/rustc/git/bun started by an IDE or agent) cannot join an app budget;
  this is now noted once per app and shown as *N outside* in the Limits tab.
  Exited processes are ignored silently.
- Every RustWTM job object allows `CREATE_BREAKAWAY_FROM_JOB`, so limiting
  browsers, IDEs and sandboxed tools never prevents them from starting child
  processes.

### Added

- Collapsible navigation with icons: an icon-only rail on narrow windows (or
  on request), scrollable on short ones.

- Program icons in the process list and on the Overview, read from each
  executable once and cached.
- Left-hand navigation grouped into monitoring, protection and system,
  replacing the crowded tab bar; CPU / memory and the connection badge moved
  to the status bar.
- The Overview explains when the installed service is a different version
  and links the latest release.

## [0.3.0] - 2026-10-20

### Added

- **Overview** tab, the new start page: CPU / memory / GPU at a glance,
  one-click *Turn protection on*, CPU protection levels (Off, Gentle,
  Balanced, Strict), the busiest apps with quick actions (efficiency mode,
  limit, details) and the latest automatic actions.
- **Foreground protection** (`governor.protect_foreground`, on by default): the
  app you are working in, and everything it started, is never throttled and is
  released at once when you switch to it. The RustWTM window reports the
  foreground app; the service accepts reports only for the client's own
  session (IPC protocol 5).
- **About** tab with credits, version and links.
- **Logo and icons**: a gauge logo drawn in code, used for the window, the tray,
  the executables (with version information), the MSI / Apps list and the
  Start Menu shortcut, and the README (`assets/`).
- App history: *Limit <app>…* opens the Limits tab with a CPU budget suggested
  from the app's p95 usage.
- **Memory guard** (`[memory_guard]`, on by default): when RAM stays above
  90%, the biggest background apps have their working sets trimmed (nothing
  is closed or lost); never the foreground app. Manual *Free memory* in the
  process menu and on the Overview, `wtm trim <target>`.
- Limits tab: pick the program from the running apps (busiest first).
- Process list: *Command line* column (shown by default); the search box also
  matches command lines. Command lines are read by the GUI with the user's own
  rights, never sent by the service.
- `wtm list --cmdline` (`-c`) adds a COMMAND LINE column.

### Changed

- IPC protocol 5: the GUI, CLI and service must all be 0.3.

## [0.2.0] - 2026-10-19

### Added

- **App limits** (`[[limit]]`): a CPU (in cores) and/or memory budget per app,
  shared by all of its processes and everything they start (`scope = "group"`),
  or per top-level instance (`scope = "each"`), enforced with job objects by
  the service. New **Limits** tab in the GUI (with *Limit all <name>…* in the
  process menu) and `wtm app-limit set | remove | list`; the process list's
  Limits column shows app-limit membership.
- **Program blocking** (`block = ["app.exe"]`): listed programs never start
  (Image File Execution Options); Windows launches `rustwtm.exe --blocked`
  instead, which notifies the user. Only RustWTM's own entries are touched,
  and they are removed when the service stops.
- **Sorting everywhere**: every list sorts by any column (click a header, click
  again to reverse): Users, Network, Services, Startup apps, App history,
  Limits, managed processes, and the process list (now also by Limits).
- Process list: optional Commit, Working set, GPU memory, Session, Base
  priority and Running-for columns, chosen in the new *Columns* menu.
- **Settings** tab: edit the service's configuration with live validation,
  switch between dry-run and enforcing, save (elevated).
- `wtm config show | default | check <file> | apply <file>`.
- Process details show the command line and the user account (GUI details
  panel with *Copy command line*, `wtm info`); *Open file location* in the
  details panel and the context menu.
- Long-term history records per-app GPU usage; App history (GUI and
  `wtm history`) shows and plots it, and insights flag chronic GPU users
  (`chronic_gpu_percent`, `chronic_gpu_actions`; report only by default).
  Existing history databases are upgraded in place.
- Rule condition `gpu_above`, e.g. to suspend a background GPU miner.
- IPC protocol 4: `GetConfig` and `SetConfig` (admin only; validated, written
  atomically). Clients and service must both be 0.2.

### Changed

- Test line coverage raised to 98% (from 91%); the SCM integration test also
  collects coverage from the installed service process.

### Fixed

- Tree view: processes whose parents form a cycle (reused PIDs with identical
  start times) were missing from the list.
## [0.1.1] - 2026-10-19

### Fixed

- Memory-only job limits (`wtm limit --memory`, GUI *Limit memory*) failed
  with "process no longer exists": the limiter tried to disable a CPU cap that
  was never enabled. Errors other than a missing process are now reported as
  such.
- Clicking a process name (or any text) in a table now selects the row; selectable
  text previously swallowed the click.
- Tables in different tabs no longer share layout state.

### Added

- MSI package (WiX 5) attached to every release: installs the service with
  failure recovery, PATH entry and Start Menu shortcut.
- Headless GUI tests (egui_kittest), engine/IPC end-to-end tests and CLI
  integration tests, and real MSI / install-script installs in CI; line coverage
  is about 90%.

## [0.1.0] - 2026-10-19

First public release.

### Added

- **Process monitoring** via `NtQuerySystemInformation`: CPU %, private
  working set, commit, threads, handles, disk I/O rate, keyed by PID and
  creation time so PID reuse never mixes up processes.
- **GPU usage** per process (busiest engine %, dedicated memory) from the
  GPU performance counters.
- **Process control**: end task, end process tree, suspend / resume,
  priority, CPU affinity, efficiency mode (EcoQoS), hard CPU cap and memory
  cap through job objects. Core Windows processes, critical processes and
  RustWTM itself are always refused.
- **CLI `wtm`**: `list`, `top`, `info`, `kill`, `suspend`, `resume`,
  `priority`, `affinity`, `eco`, `limit`, `history`.
- **Service `wtm-service`** (LocalSystem, auto-start, restarts on failure):
  - CPU governor with sustain / recovery thresholds, step cooldown and an
    escalation ladder.
  - Rule engine (name / path wildcards, sustained CPU / memory conditions);
    unconditional rules react within ~250 ms of a process starting.
  - Long-term insights: per-minute per-app history in SQLite, detection of
    chronic CPU use and steady memory growth (slope + R² fit), automatic
    actions with rollback when a finding clears.
  - Per-source tracking (governor, rule, insight, manual) so one source never
    undoes another's limits; everything is rolled back on stop.
  - Dry-run by default, hot-reloaded `config.toml`, rolling log files,
    in-memory event log.
- **IPC** over a local named pipe with an explicit DACL, anti-squatting,
  server identity verification and admin-only control requests.
- **GUI `rustwtm`**: Processes, Performance, Governor, App history, Startup
  apps, Users, Services, Network and Events tabs; tray icon; desktop
  notifications for notable events.
- **Installer**: `scripts/install.ps1` / `scripts/uninstall.ps1`, release
  archives with SHA-256 checksums built by GitHub Actions.

[Unreleased]: https://github.com/ersinkoc/RustWTM/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/ersinkoc/RustWTM/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/ersinkoc/RustWTM/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/ersinkoc/RustWTM/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/ersinkoc/RustWTM/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/ersinkoc/RustWTM/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/ersinkoc/RustWTM/releases/tag/v0.1.0

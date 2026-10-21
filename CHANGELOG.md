# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.15.0] - 2026-10-21

### Added

- A monitoring-data receipt age on every GUI page, with a stale-data warning
  after a sampling failure or a delayed update. The last received values remain
  visible. The timeout allows slower sampling intervals; successful updates
  clear the warning. All five interface languages are supported.
- Resources reserved for RustWTM (`[reserve]`): while the CPU is overloaded a
  rescue core is kept free for RustWTM and Windows by moving other programs off
  it with CPU Sets, and handed back when the load recovers. The service keeps
  `working_set_mb` resident and opts out of EcoQoS; the window runs above
  normal priority with 64 MB resident. Shown on the Overview and in the status
  bar, configurable in Settings.
- Window drawing times on the About page, and `RUSTWTM_FRAME_REPORT=<file>`
  to benchmark every page and write per-page frame timings.

### Changed

- The window reads command lines and image paths of new processes without
  holding the data lock it needs for every frame, so a burst of new processes
  no longer stalls drawing. The About logo is uploaded once instead of every
  frame, and the Overview cards line up at equal height.
- IPC protocol version 12 (status carries the rescue core).

## [0.14.0] - 2026-10-21

### Added

- GUI and desktop notifications in English, Turkish, German, French and Spanish.
  The language selector applies immediately and persists per Windows user;
  the default follows the Windows UI language. Missing translations fall back
  to English, while unknown/native diagnostic details retain their source text.
- Translation coverage and placeholder checks in the regular test suite,
  five-language page/navigation tests and optional visual reviews in both themes
  at wide and compact sizes.
- 96 and 128 px sizes in the embedded product icon, alongside the existing
  16, 20, 24, 32, 48, 64 and 256 px sizes; executable resource verification.

### Changed

- IPC remains protocol 11.
- Navigation width follows translated labels; saved actions, process identities
  and service configuration keys remain language-independent.

## [0.13.1] - 2026-10-21

### Added

- Hybrid CPU topology from Windows CPU Sets: `wtm cores [--json]`, processor
  group/core identifiers and relative P/E/LP-E tiers without hard-coded class values.
- `wtm affinity <target> --pcores|--ecores|--lp-ecores|--all-cores` and a CPU core
  type menu in Processes. PID identity/protection checks remain enforced;
  processes spanning multiple groups are rejected rather than truncated.
- Performance graphs label hybrid tiers; uniform CPUs omit the hybrid controls.
- Explicit shared Windows frequency limits in Sensors & cooling and
  `wtm power show|limit|restore`. Administrator access and confirmation are
  required for writes. Original values are saved before mutation, partial
  failures attempt rollback, and detected external changes are preserved.
  These are power-plan settings, not per-PID MHz caps; class 1 is not assumed
  to mean E-cores.

### Changed

- IPC protocol 11 adds core-tier requests.
  Update the GUI and service together.

## [0.13.0] - 2026-10-20

### Added

- **Sensors & cooling**: device-grouped temperatures, RPM, power, voltage and
  clocks, current/min/max, recent samples, filtering and explicit stale/missing
  values; `wtm sensors [--json]` provides local read-only telemetry.
- Optional, packaged LibreHardwareMonitor 0.9.6 provider, with NVIDIA driver
  telemetry as a read-only fallback when the helper is absent.
- Capability-based manual duty and editable temperature curves, Quiet/Balanced/
  Performance presets, thermal full-speed override and hardware-default mode.
  Writes require administrator access, an enforcing service, fresh temperature
  and explicit confirmation. Drafts persist without reapplying on startup.
  Provider leases attempt default restoration after missed renewals/closed input;
  physical write and failure-recovery validation remains pending per hardware.
- **Pause process updates** (button or F6): inspect a stable process snapshot
  while monitoring, history, protection and alerts continue. Search, sorting
  and process actions remain available; actions retain PID-reuse checks.
- **Quick process filters**: high CPU (at least 5%, excluding System Idle),
  high memory (at least 512 MiB), and managed processes. Filters combine with
  search, include a matching-process count and offer a clear-filter empty state.
- Optional GPU visual review: `cargo test -p wtm-gui --features visual-tests
  render_release_review -- --nocapture` renders five pages in two themes and
  two window sizes to `target/visual-review` using synthetic data.
- **Stuck swarms are cleaned up** (opt-in): with `[swarm] end_after_mins`
  (Settings → Service settings → Swarm warning) the service ends copies of a
  swarm that have run longer than that, together with what they started
  (their console hosts), up to 100 per second, with one notification per
  round; in dry run it only reports them. The swarm card offers the same by
  hand: *End the N running over 10 min*.

### Changed

- Navigation uses consistent vector icons with accessible names, labelled
  groups and a clear active marker; page headings explain each view.
- Refined typography and light/dark surfaces, Windows UI and symbol font
  fallbacks, readable resource-text colours and named theme selectors.
- Overview and configuration pages scroll in short windows; the last row of
  overview metrics fills the available width. Processes supports horizontal
  column scrolling and a scrollable, closeable details panel.
- Process search also matches executable paths.
- Named-pipe protocol is now version 10; update the GUI, CLI and service together.

### Fixed

- Delete in a focused search field no longer opens an End task confirmation
  for the selected process. The process table owns the shortcut and respects
  text fields, menus and modals.
- A process start time far in the future no longer overflows.
- App-limit integration fixtures now wait for the ping child to be ready before
  changing its jobs or ending its parent, avoiding an intermittent startup race.

## [0.12.0] - 2026-10-20

### Added

- **Tell me when it exits**: right-click a process and pick *Notify me when
  it exits* (a long build, a download, a test run) for a desktop
  notification when it ends; in Telegram, `/wait 1234` or the button under
  `/info` sends a message instead (`/unwait` cancels). Waiting works in a
  report-only bot too.
- Telegram: `/restartsvc NAME` restarts a service.

### Changed

- **Overview**: five cards across the window (CPU, memory, GPU, disk,
  network), each with its last minute as a sparkline; the verdict sits in a
  coloured strip and also warns about swarms; protection, the busiest apps
  and recent actions are cards, the last two side by side on wide windows.
- **Graphs start full**: the service keeps the last five minutes of
  system-wide load, and the window fills its graphs from it when it
  connects instead of starting empty.
- **Settings**: the main service settings (enforcing or monitoring, CPU
  governor thresholds, memory guard, swarm warning, history) are a form;
  editing `config.toml` by hand moved under *Advanced*, and comments in the
  file are kept.
- **Performance**: slimmer disk and GPU engine bars with their figures beside
  them; the system summary moved to the shorter column.

### Fixed

- The swarm card and its notification had a run of spaces in the middle of a
  sentence.

## [0.11.0] - 2026-10-20

### Added

- **Telegram bot**: watch and control the PC from your phone. Set it up in
  Settings → Telegram (a token from @BotFather and the chat IDs allowed to
  use it; the bot tells strangers their ID). `/status`, `/top`, `/mem`,
  `/find`, `/info`, `/ports`, `/services`, `/swarms` and `/events` report;
  `/kill` (by PID or name), `/tree`, `/suspend`, `/resume`, `/free PORT`,
  `/panic`, `/startsvc` and `/stopsvc` act, ending only after a tap on
  *End*, with *End* buttons under every list. Alerts (CPU overload, swarms,
  the kill list) arrive as messages; `/mute` pauses them. The bot runs in
  the service, so it works with no window open; the token is kept in
  `%ProgramData%\RustWTM\telegram.toml`, readable only by administrators,
  and control can be switched off for a report-only bot. Protected Windows
  processes and service hosts are never ended.
- Everything Task Manager does is here too: **Create memory dump file** and
  **Search online** in the process menu (`wtm dump 1234` writes a full
  dump), **Restart** for services, and **Disconnect** / **Sign off** for
  users, each asking first.
- Processes: the details panel lists the ports the selected process listens
  on, with the tool they belong to and who can reach them, and opens the
  Ports tab.

## [0.10.0] - 2026-10-20

### Added

- **Swarm warning**: when one program runs in 150 copies or more (a tool
  that keeps starting helpers and never waits for them, such as a status
  probe launching `powershell.exe` every few seconds), the service sends a
  desktop notification and Overview shows who started them, how much memory
  they hold, and buttons to end them all, only those whose parent exited, or
  to add the program to the kill list. `svchost.exe` and other normal crowds
  are ignored; set `[swarm] min_count` and `exempt` in the configuration.

## [0.9.0] - 2026-10-20

### Added

- **Ports tab**: every listening TCP port (and bound UDP port, on request)
  with its program, PID, CPU and memory, the addresses it listens on, who can
  reach it (*This PC only*, *One network*, *All networks*) and how many
  clients are connected. Sort by any column, search, right-click to end the
  program or its tree, show it in Processes, open `http://localhost:<port>`,
  copy the address or add it to the kill list.
- **Developer tools & servers**: the usual ports of about 75 dev servers,
  databases, caches, queues, search engines, container tools, monitoring and
  AI tools, with whether they are in use and by whom; *Open* and *End* per
  row, *Show free ports too* for the whole catalog, and ports of your own to
  watch (remembered between runs).
- **Free a port**: type a port and end what keeps it busy, every program
  holding it at once. Windows' own ports (System, service hosts) are refused
  with the reason; programs that only relay a port for Docker or WSL carry a
  warning.
- `wtm ports [--udp] [--known]` and `wtm free-port <port> [--yes] [--tree]`.

## [0.8.0] - 2026-10-20

### Added

- **Updates inside the app**: RustWTM checks GitHub for a newer release at
  start and twice a day and shows a bar with *Update now*, which downloads
  the installer, verifies it against the SHA-256 published with the release
  (nothing is installed on a mismatch), runs it (Windows asks for
  administrator rights) and starts the new version. *What's new*, *Later*
  and *Skip this version* are on the bar; *Check for updates* is in About,
  and automatic checks can be turned off in Settings → This window. Copies
  not installed with the MSI get a download link instead.
- Theme choice (System / Light / Dark) in Settings → This window.

### Changed

- **A new look**: rounder, airier widgets in the logo's orange, refined
  light and dark themes, sections on cards.
- Performance: every resource in its own colour with filled graphs, a small
  history graph for each logical processor (as Task Manager shows them),
  coloured engine and disk bars, and a balanced two-column layout.
- Overview: ring gauges for CPU, memory and GPU.

### Fixed

- The taskbar and Alt+Tab show the RustWTM logo instead of the default
  window icon, also when RustWTM starts in the tray or as administrator.

## [0.7.0] - 2026-10-20

### Added

- **Kill list** (new tab, `[[kill]]` in the configuration, `wtm kill-list`):
  programs that may be ended, matched by name and optionally by command line
  (`node.exe` running `vitest`, but no other Node program). With a CPU
  threshold the service ends one that keeps using that much for a while and
  does not recover, optionally only while the whole PC is busy, together with
  the processes it started.
- **Panic button** (status bar, Ctrl+Shift+K, the overload alert,
  `wtm panic --yes`): ends everything on the kill list at once after showing
  what will be ended.
- Rules match on the command line too: `match = { name = "node.exe",
  cmdline = "vitest" }`.
- Right-click a process → *Add to kill list…*.

## [0.6.0] - 2026-10-20

### Added

- **Overload alert**: when the CPU stays at or above 90% for 5 seconds,
  RustWTM pops up above all windows (also from the tray) with the busiest
  apps and one-click fixes: *Calm them all down* (efficiency mode and lower
  priority for everything but the app in use), efficiency mode, limit to one
  core, pause or end per app, and turning on automatic protection. RustWTM
  raises its own priority while the alert is open, so it stays responsive.
  Close (quiet for 5 minutes), snooze for an hour, or turn alerts off;
  threshold and duration are in Settings → This window.
- The window's close button keeps RustWTM running in the tray (Exit is in the
  tray menu); RustWTM starts in the tray at sign-in (`rustwtm --background`),
  both switchable in Settings → This window.
- Starting RustWTM again shows the running one instead of a second copy
  (`--new-instance` starts another anyway).

## [0.5.0] - 2026-10-20

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

[Unreleased]: https://github.com/ersinkoc/RustWTM/compare/v0.15.0...HEAD
[0.15.0]: https://github.com/ersinkoc/RustWTM/compare/v0.14.0...v0.15.0
[0.14.0]: https://github.com/ersinkoc/RustWTM/compare/v0.13.1...v0.14.0
[0.13.1]: https://github.com/ersinkoc/RustWTM/compare/v0.13.0...v0.13.1
[0.13.0]: https://github.com/ersinkoc/RustWTM/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/ersinkoc/RustWTM/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/ersinkoc/RustWTM/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/ersinkoc/RustWTM/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/ersinkoc/RustWTM/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/ersinkoc/RustWTM/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/ersinkoc/RustWTM/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/ersinkoc/RustWTM/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/ersinkoc/RustWTM/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/ersinkoc/RustWTM/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/ersinkoc/RustWTM/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/ersinkoc/RustWTM/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/ersinkoc/RustWTM/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/ersinkoc/RustWTM/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/ersinkoc/RustWTM/releases/tag/v0.1.0

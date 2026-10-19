# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/ersinkoc/RustWTM/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/ersinkoc/RustWTM/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/ersinkoc/RustWTM/releases/tag/v0.1.0

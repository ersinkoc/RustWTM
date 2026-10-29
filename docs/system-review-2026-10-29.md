# System review and UX improvements — 2026-10-29

This records Round 44. The [Round 45 continuation](system-review-round45-2026-10-29.md)
contains subsequent development findings. The [0.22.0 release review](0.22.0-review.md)
records the release that incorporates both rounds.

The review produced three reproduced fixes and a direct path from Overview
resource cards to their performance graphs. No dependencies, configuration
formats, IPC messages, or version numbers changed.

## Scope and safety

The structural review covered all seven Rust crates and the optional .NET
hardware provider. Source inspection concentrated on GUI navigation, resource
cards, status messages, process actions, monitoring freshness, sampling,
governor timing, memory guard, rules and app limits, service execution and IPC,
history aggregation, CLI/MCP entry points, and cooling policy and leases.
This is a review of those paths and their existing validation, not a claim
that every possible defect or hardware combination has been eliminated.

`CLAUDE.md` and `AGENTS.md` were read before testing. Builds use two workers;
ordinary tests use at most two test threads per binary. New failure proofs use
synthetic snapshots, a supplied clock, and the offline GUI backend. No proof
exhausts memory, address space, handles, threads, kernel pool, or ETW loggers.
Machine-changing installer/SCM checks, ignored benchmarks, and whole-drive
scans were not enabled. No real exhaustion test was written or run.

The pre-existing `.claude/settings.json` edit is preserved. There was no commit,
push, tag, installation, service replacement, or physical fan/frequency change.

## Round 44 — monitoring, status layout, and governor lifecycle

### Summary

Three findings confirmed and fixed, with permanent regression coverage.
The live hardware and installation boundaries remain outside this local review.

### Toolchain

- Rust 2024 edition, Rust 1.99.1 from `rust-toolchain.toml` and root `Cargo.toml`.
- Cargo workspace and built-in Rust test runner; `egui_kittest` 0.37.0 for
  offline GUI interaction and optional GPU rendering, from
  `crates/wtm-gui/Cargo.toml`.
- Project gates: rustfmt, Clippy with warnings denied, rustdoc with warnings
  denied, workspace tests and builds, as defined in `CONTRIBUTING.md` and CI.
- No race detector or sanitizer was enabled for this run.

### Findings

#### F4401 — bug — Medium — cached CPU data can raise a new overload alert

- Location: `crates/wtm-gui/src/backend.rs`, `Worker::watch_cpu`.
- Root cause: each worker tick advanced the overload timer using the current
  time even when the CPU value was an old or failed monitoring sample.
- Proof before: `.temp_files/prove_F4401_stale_alert.ps1` and
  `.temp_files/F4401-before.log`: a 60-second-old 99% CPU sample raised an
  alert; the assertion failed with `stale CPU is not current overload`.
- Patch: require current data, use its measurement time for sustained-load
  observation, and interrupt the observation when monitoring fails or expires.
  Clock corrections cannot underflow the duration calculation.
- Verification: stale data, repeated cached readings, an injected sampling
  error, recovery, a backward measurement-time correction, and genuinely
  sustained new readings. The existing overload/quiet-time test remains the
  unaffected control.
- Regression tests: `stale_cpu_samples_do_not_raise_an_overload_alert` and
  `repeated_cpu_samples_and_monitoring_gaps_do_not_sustain_an_alert` in
  `backend.rs`.

#### F4402 — bug — Medium — long status messages hide essential actions

- Location: `crates/wtm-gui/src/app.rs`, bottom status panel.
- Root cause: the message took unbounded horizontal space before the action
  group was laid out.
- Proof before: `.temp_files/prove_F4402_status_layout.ps1` and
  `.temp_files/F4402-before.log`: in an 820-pixel window, **Run new task**
  occupied x=2411–2510, entirely outside the viewport.
- Patch: wrap the actions to the available width and put the message on a
  separate, truncated row. Hover reveals its full text. The resource summary
  also stays on one line, with complete values available on hover.
- Verification: both task-launch and elevation buttons remain within the
  820×500 viewport and task launch opens its dialog in English, Turkish,
  German, French, and Spanish. Wide and compact GPU screenshots also cover
  ordinary and long-message states in both themes.
- A second visual-driven proof, `.temp_files/F4402-summary-before.log`,
  reproduced a wrapped summary with a 44-pixel-high label. The same permanent
  test now checks that the summary remains a single line in all five languages.
- Regression test:
  `status_actions_stay_inside_a_compact_window_with_a_long_message` in `app.rs`.

#### F4403 — bug — Medium — disabled time counts toward governor overload

- Location: `crates/wtm-policy/src/governor.rs`, `Governor::set_config`.
- Root cause: switching enabled state retained `high_since` and `low_since`.
- Proof before: `.temp_files/prove_F4403_governor_restart.ps1` and
  `.temp_files/F4403-before.log`: the first sample after re-enabling produced
  a priority-lowering decision without a new sustained observation.
- Patch: reset both timing windows when enabled state changes; retain normal
  tracking of processes already managed by the governor.
- Verification: sustained-load control, rollback while disabled, re-enable,
  the exact sustain boundary, and off/on between samples.
- Regression test: `reenabling_governor_starts_a_new_sustain_window` in
  `governor.rs`.

### Not reproduced (dropped)

None of the three selected candidates was dropped. Source observations outside
these findings were not promoted to defects or speculative production patches.

## UX improvement — actionable resource cards

Overview's CPU, Memory, GPU, Disk, and Network cards now open the matching
performance section and bring its graph into view. The cards retain the
resource palette, add an opening indicator, a pointer cursor, a hover/focus
outline, and an accessible label containing the current value. Enter activates
a focused card. The explanatory tooltip is translated into all five languages.

`overview_resource_cards_open_and_scroll_to_their_graphs` exercises every card
in both themes, including keyboard activation, and checks the target graph's
on-screen position. Existing whole-app navigation and five-language rendering
tests continue to cover the surrounding pages.

## Validation

| Check | Result |
| --- | --- |
| Initial workspace baseline | 622 passed, 0 failed, 16 ignored |
| Final `cargo test --workspace --locked -j 2 -- --test-threads=2` | 627 passed, 0 failed, 16 ignored |
| Three finding-specific `verify_F440*.ps1` scripts | Each passed three consecutive runs; `FIX VERIFIED` |
| Original `prove_F440*.ps1` scripts after the patches | All print `PROBLEM NOT REPRODUCED` |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --workspace --all-targets --locked -j 2 -- -D warnings` | Passed |
| `cargo doc --workspace --locked --no-deps --document-private-items -j 2`, `RUSTDOCFLAGS=-D warnings` | Passed |
| `scripts/build-hardware.ps1 -OutputDir target/system-review/hardware` | Passed; simulated leases, renewal, expiry, rollback retry, shutdown, duty bounds and unavailable readings |
| `git diff --check` | Passed |
| Repository PowerShell script parser checks | Passed |
| `cargo build --workspace --release --locked -j 2` | Passed; final optimized GUI rebuild completed in 3m 05s |
| Release executable smoke | `wtm 0.21.0`, `wtm-service 0.21.0`; read-only `wtm list --json --limit 3 --sample-ms 150` returned three rows and valid CPU topology |
| Final GPU screenshot generation | Passed: 80 English images and 24 each in Turkish and German, across light/dark themes and 1440×1000 / 820×500 layouts |

The final renderer was run with:

```powershell
$env:WTM_REVIEW_LANGS = 'en'
$env:WTM_REVIEW_TABS = $null
cargo test -p wtm-gui --locked -j 2 --features visual-tests render_release_review -- --test-threads=1 --nocapture
$env:WTM_REVIEW_LANGS = 'tr,de'
$env:WTM_REVIEW_TABS = 'Overview,Performance,Processes'
cargo test -p wtm-gui --locked -j 2 --features visual-tests render_release_review -- --test-threads=1 --nocapture
```

English rendering covers 16 major pages plus configuration-file, stale-data,
long-message and power-control states. The focused Turkish/German run covers
Overview, Processes, Performance and the related diagnostic states. The About
page and all five languages are covered by the headless whole-app suite.
Representative normal, light/dark, compact, translated and long-message images
were inspected manually. In particular, the final Turkish/German compact
images keep the actions inside the viewport and the memory unit with its value.
All 128 final images remain in `target/visual-review/locales/`; selected images
are also copied to `target/system-review/after/`.

The new interaction test first exposed a keyboard-focus issue in the new card
implementation. Giving the card's button a separate interaction ID from its
hover-only frame corrected it; the final test activates Memory with Enter and
clicks every other resource, then checks the matching graph's position.

Logs are retained as `.temp_files/system-review-*.log`,
`.temp_files/F440*-before.log`, `.temp_files/F440*-after.log`, and
`.temp_files/F440*-reproof.log`. Before-fix screenshots are preserved in
`target/system-review/before/`.

The Round-44 GUI artifact was built at `target/release/rustwtm.exe`; later
rebuilds replace that file. The version stays 0.21.0 because this work does not
publish a release. The following digests identify the Round-44 build:

| Executable | SHA-256 |
| --- | --- |
| `rustwtm.exe` | `e742c6c046c20eef693ba527a0b3af47480ee80986c5ba64c0dc5d9022fe0cc5` |
| `wtm.exe` | `40ea80dd5ab432bb82322ceb274db22533ee59d2232b2ac7529bd8c864f61db0` |
| `wtm-service.exe` | `5ab9538c8754e297432c1d876d9b29d8dd60b587e70f54d6bd80ca816a36c47b` |

## Boundaries

Offline screenshots use deterministic fixture data; their resource values are
not measurements of the developer's PC. Physical cooling writes, real CPU
frequency changes, MSI installation, service replacement, hosted CI, and a new
release are not part of this verification. The running installed application
must be distinguished from the newly built source artifacts.

## Working tree

Task changes are limited to GUI code and its locale/test support, governor
timing and its regression, this review, and the Unreleased changelog.
The pre-existing `.claude/settings.json` modification remains untouched.
Reproduction scripts, three-run verifiers, and logs are retained under the
ignored `.temp_files` directory.

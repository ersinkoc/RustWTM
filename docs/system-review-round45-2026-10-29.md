# Round 45 — process navigation, session layout and event delivery

This records the development snapshot before 0.22.0 versioning. See the
[0.22.0 release review](0.22.0-review.md) for the combined release verification.

This continues the [system review](system-review-2026-10-29.md). Three additional
findings were reproduced and fixed, with five permanent regression tests. The
changes preserve the existing themes, IPC protocol, configuration and version.

## Summary

- Process selections stay visible in long lists. Opening Details from Overview
  clears stale searches and filters, resumes a paused list, and expands the
  selected app group. Later manual scrolling is preserved.
- The first new notification after an empty service backlog is delivered.
  History backfill happens once per connection, and reconnection still avoids
  replaying old notifications.
- The Users table scrolls horizontally and vertically, so long account names
  and many sessions do not make their actions unreachable.

## Toolchain and scope

Scope: `crates/wtm-gui/src/processes.rs`, `app.rs`, `backend.rs`, `views.rs`,
`ui_tests.rs`, the Unreleased changelog and review documentation.

Rust 1.99.1 is pinned in `rust-toolchain.toml`; the workspace uses edition 2024
in `Cargo.toml`. Tests use Cargo's Rust runner and egui_kittest 0.37.0, as
declared in `crates/wtm-gui/Cargo.toml`. Formatting and linting use rustfmt and
Clippy, with warnings denied. No race detector or sanitizer was enabled.

`CLAUDE.md` and `AGENTS.md` were read first. New proofs use at most 80 synthetic
process rows, 24 synthetic sessions, or a small isolated test pipe with fake
notification callbacks. No real sign-out, notification, resource exhaustion,
service replacement or physical hardware write is performed. Builds use two
workers; ordinary test binaries use at most two test threads.

## Findings

### F4501 — bug — Medium — a requested process remains outside the visible list

- Location: `ProcessView::show`, `ProcessView::reveal`, and the app's
  `Intent::Reveal` handling.
- Root cause: changing the selection never scrolled the virtualized table to
  its row. A cross-page request also retained old search/filter/pause state
  that could exclude the requested process altogether.
- Proof before: `.temp_files/F4501-before.log` reports the selected
  `app-025.exe` missing from the visible rows after 25 Down presses. The
  neighboring-row control passes. `.temp_files/F4501-filter-before.log` shows
  the Overview → Details request still filtered by `does-not-match`.
- Patch: reveal each new selection once using the table's row-scrolling API.
  A cross-page reveal clears transient blockers and opens the requested Apps
  group. Unchanged selections do not override manual scrolling or repeatedly
  search for an exited process.
- Verification: List and Tree navigation, an external selection further down,
  manual scrolling back to the top, and the actual Overview → Details flow
  with old filters, Apps mode and an empty paused snapshot.
- Regression tests: `keyboard_selection_stays_visible_in_a_long_process_list`
  in `ui_tests.rs`, and
  `overview_details_reveal_a_process_despite_old_filters_and_a_paused_list`
  in `app.rs`.

### F4502 — bug — Medium — an empty initial backlog silences the first new event

- Location: `Worker::tick` in `backend.rs`.
- Root cause: `last_event == 0` doubled as the connection-initialization flag.
  An empty backlog left that cursor at zero, so the first later live event was
  classified as old backlog and was not announced. Recent history was also
  requested again unnecessarily.
- Proof before: `.temp_files/F4502-before.log`: expected one notification for
  the first new event, actual zero. The empty-backlog control remains silent.
- Patch: track the initial successful fetch separately from the event cursor,
  and reset that state whenever a new connection is established.
- Verification: empty initial backlog, first live event, later empty polls,
  a non-notifying event, one history request per connection, and a reconnect
  that suppresses its backlog but announces the next new event. Request/reply
  ordering is driven by sequential test-pipe exchanges; no timing sleeps.
- Regression test:
  `an_empty_initial_backlog_does_not_silence_the_first_new_event` in `backend.rs`.

### F4503 — bug — Medium — narrow session tables hide action buttons

- Location: `views::users` in `views.rs`.
- Root cause: the Users grid could exceed the viewport without offering
  horizontal scrolling.
- Proof before: `.temp_files/F4503-before.log` places Sign off at
  x=777.3–841.7 in an 820-pixel window even after horizontal wheel input.
  The 1440-pixel-wide control succeeds. The fixture uses a valid 20-character
  username and 15-character domain.
- Patch: use the shared table panel with a visible horizontal scrollbar and a
  vertical scroll area for larger session lists.
- Verification: scroll to and click the action in compact/wide windows;
  scroll through 24 synthetic sessions and verify that the last action still
  selects session 123. Only the returned intent is examined; no user is signed
  out by the tests.
- Regression tests: `user_session_actions_are_reachable_in_a_narrow_window`
  and `many_user_sessions_can_be_scrolled_without_changing_their_actions`
  in `ui_tests.rs`.

## Not reproduced (dropped)

None of the three selected findings was dropped. No unrelated production
patches were added from source-only observations.

## Validation

The three `.temp_files/verify_F450*.ps1` scripts each passed three consecutive
runs and printed `FIX VERIFIED`. They include the original reproductions and
the additional edge cases listed above. The original reproductions now print
`PROBLEM NOT REPRODUCED`.

| Check | Result |
| --- | --- |
| Three finding verifiers, each run three times | Passed; `FIX VERIFIED` |
| GUI tests within the final workspace run | 166 passed, 0 failed, 2 ignored |
| `cargo test --workspace --locked -j 2 -- --test-threads=2` | 632 passed, 0 failed, 16 ignored |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --workspace --all-targets --locked -j 2 -- -D warnings` | Passed |
| `cargo doc --workspace --locked --no-deps --document-private-items -j 2`, `RUSTDOCFLAGS=-D warnings` | Passed |
| GPU fixture renderer | Passed; 48 process/session images and 12 existing power-control images |
| `git diff --check` | Passed |
| `cargo build --workspace --release --locked -j 2` | Passed; optimized GUI rebuild completed in 2m 33s |
| Release executable smoke | `wtm 0.21.0`, `wtm-service 0.21.0`; read-only JSON listing returned three rows and valid CPU topology |

Logs are retained in `.temp_files/round45-*.log`,
`.temp_files/F450*-before.log`, `.temp_files/F450*-after.log`, and
`.temp_files/F4501-filter-{before,after}.log`. They include the failed assertions
from the unfixed code and the passing controls and boundary checks.

## Visual verification

The renderer uses the offline backend and deterministic fixtures:

```powershell
$env:WTM_REVIEW_LANGS = 'en,tr,de'
$env:WTM_REVIEW_TABS = 'Processes,Users'
cargo test -p wtm-gui --locked -j 2 --features visual-tests render_release_review -- --test-threads=1 --nocapture
```

The test passed in 59.86 seconds. It renders normal and selected-process/session
states in both themes at 1440×1000 and 820×500, plus the existing power-control
fixtures. Representative English compact selection and Turkish/German session
images were inspected: the selected process is visible beside its details, and
the last session's actions are reachable after scrolling. These are fixture
values, not measurements or accounts from the developer's PC.

Images are under `target/visual-review/locales/{en,tr,de}/`, including
`*-ProcessesSelection.png` and `*-UsersSessions.png`. Selected images are copied
to `target/system-review/round45/`.

## Build identity

The GUI artifact produced by this round was `target/release/rustwtm.exe`,
still at version 0.21.0 before release versioning. Later rebuilds replace that
file. These digests identify the final local artifacts from this round:

| Executable | SHA-256 |
| --- | --- |
| `rustwtm.exe` | `276c61b9f02c578104d592c10a5ee85bd2a2c767be8820c04631e5621660045f` |
| `wtm.exe` | `40ea80dd5ab432bb82322ceb274db22533ee59d2232b2ac7529bd8c864f61db0` |
| `wtm-service.exe` | `5ab9538c8754e297432c1d876d9b29d8dd60b587e70f54d6bd80ca816a36c47b` |

## Boundaries

NOT RUN: installer/SCM lifecycle checks, real sign-out or process-control
actions, physical fan/frequency changes, hosted CI and publication. They are
outside this round's scope; machine-changing checks belong on disposable VMs.
The installed application and service have not been replaced.

## Working tree

The starting patch and status are retained in `.temp_files/round45-start.patch`
and `.temp_files/round45-start-status.txt`. Round-44 improvements are retained
and extended in place. The unrelated `.claude/settings.json` edit remains
untouched. Proofs, logs and the round ledger remain under `.temp_files/`.
No commit, reset, stash, cleanup, push, tag or installation was performed.

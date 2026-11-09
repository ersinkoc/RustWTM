# Process workspace visual implementation — 2026-11-09

Source baseline: `b7902ac`, workspace version 0.26.2, with the local GUI changes
described here. This is local implementation evidence, not a published release.

## Delivered behaviour

- Processes has a shared CPU, memory, disk/I/O, network and GPU strip. The CPU
  card is emphasized on wide windows; short windows use smaller cards.
- Cards use the current system snapshot and existing timestamped history.
  Keyboard-accessible card buttons open the corresponding Performance focus.
  Invalid/unpaired samples and long gaps do not become continuous curves.
- A per-user setting controls strip visibility and survives restart. Older
  preferences receive the default without discarding their other settings.
- The selected-process inspector puts identity, uptime, primary actions,
  CPU/private-memory measurements and existing history near the top. Process
  graphs retain their resource colours. Exit and identity checks remain intact.
- The process table has a distinct section surface. Compact workspaces use a
  quick-filter menu; recent-start presets remain available in More filters.
- Dark page/card/selection surfaces are coordinated. Existing resource hues
  and the orange brand accent remain. Added strings have all four translations
  alongside English and are included in catalog completeness checks.

## Local validation

| Check | Result |
| --- | --- |
| Baseline GUI suite before changes | 192 passed, 0 failed, 2 existing ignored |
| Final GUI suite, two test threads | 196 passed, 0 failed, 2 existing ignored |
| rustfmt | Passed |
| Locked GUI Clippy, all targets/features, warnings denied | Passed |
| GUI rustdoc, private items, warnings denied | Passed |
| Locked debug GUI build, two Cargo jobs | Passed |
| Process workspace GPU renders | Five languages, both themes, 1440x1000 and 820x500 at 1x |
| Additional GPU scaling renders | English, both themes, both sizes at 1.25x, 1.5x and 2x |

The renderer captures the process workspace, a selected-process inspector and
a selected row within an 80-row synthetic list. These are 96 images across the
stated combinations. Representative wide, compact, light, dark, translated and
scaled images were visually inspected. Automated interaction coverage includes
card navigation, missing initial data, selection preservation, preference
compatibility, compact filtering and dispatch to the selected process.

Committed previews: [dark](../screenshots/processes.png),
[light](../screenshots/processes-light.png),
[compact](../screenshots/processes-compact.png). All show synthetic data.
Other generated images are under `target/visual-review/`; local check logs
are under ignored `.temp_files/desktop-*` paths.

## Remaining boundaries

Subsequent same-day work completed stable live inspection (M2a); its separate
validation and current behaviour are in [Inspecting live processes](process-inspection.md).
The results below describe the boundary of this initial visual delivery.

This delivers the first visual subset of the
[desktop experience plan](desktop-experience-plan.md). Density/motion
preferences, stable live row sorting, column presets, group inspection,
persistent system history, file ownership and the later feature milestones
remain separate work. No milestone is marked fully complete by these checks.

No host exhaustion proof, installer, service replacement, system integration
change, physical hardware write, hosted CI run or release publication was
performed. The installed window was not replaced or restarted. The local
debug executable is `target/debug/rustwtm.exe`.

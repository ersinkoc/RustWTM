# Inspecting live processes

RustWTM keeps the relative order of existing process rows while the pointer is
over the table body, a menu is open or a modal dialog is open. CPU, memory and
other resource values still update. The process count line shows **Order held ·
values are live**; its tooltip explains how automatic sorting resumes.

Moving the pointer out of the table and closing open menus/dialogs resumes the
selected sort. If closing a dialog leaves the pointer over the table, the
pointer hold continues until it leaves. An explicit sort, search, filter or
view change applies immediately. Expanding an app group retains its position.

F6 is separate: it freezes the process-list snapshot itself. Monitoring,
protection and alerts keep running in either mode. The live-values indicator
is hidden during an F6 snapshot pause.

## Membership and identity

- Processes are matched by PID **and creation time**. A reused PID is a new
  process and cannot inherit an exited process's inspection position.
- Current filters still determine membership. Processes that exit or stop
  matching disappear; new matching processes follow the surviving entries.
  Removing rows can move later rows upward; this is not a frozen screenshot.
- Tree siblings retain their relative order and children remain below their
  parents. New children follow existing siblings. Parent cycles remain bounded
  and cannot hide processes or make the traversal loop indefinitely.
- Apps retain group order and keep their members together. New members follow
  existing members within that app; new groups follow existing groups.
- Menus and confirmations retain their existing identity checks. Holding order
  does not replace target validation or grant additional control rights.
- The ordering cache retains current identities/ranks, not old snapshots or an
  accumulating record of exited processes. Releasing inspection invalidates
  cached ordering even if no newer snapshot has arrived.

## Local verification — 2026-11-09

The initial interaction proof failed on the previous code: after CPU rankings
changed, `alpha.exe` moved from y=124.5 to y=147.5 while being inspected. The
unchanged assertions pass with identity-based ordering, live values and normal
sorting after the pointer leaves.

Six added interaction tests cover pointer inspection, new tree/app members,
exits/PID reuse/explicit sorting, cached release without new data, parent-cycle
enumeration changes, and modal/pointer hold precedence. Existing F6 and menu
target tests remain in the full GUI suite. All fixtures are synthetic; no host
resource exhaustion or service control is involved.

See the [desktop plan](desktop-experience-plan.md) for subsequent milestones.

| Final local gate | Result |
| --- | --- |
| Complete GUI suite, two test threads | 202 passed, 0 failed, 2 existing ignored |
| Locked GUI Clippy, all targets/features, warnings denied | Passed |
| rustfmt | Passed |
| GUI rustdoc with private items and warnings denied | Passed |
| Locked debug GUI build, two Cargo jobs | Passed |
| GPU rendering | 80 images: five languages, two themes, two sizes, four states |

The four render states are the process workspace, inspector, pointer inspection
and an automatically revealed selection in an 80-row synthetic list. English
wide/compact, Turkish compact and German light/compact inspection views were
visually inspected. This round did not repeat the previous DPI matrix.

Previews: [inspection](../screenshots/process-inspection.png) and
[compact inspection](../screenshots/process-inspection-compact.png).
Local logs are under ignored `.temp_files/inspection-*`; generated renders
are under `target/visual-review/locales/`. The installed GUI/service, hosted CI
and release assets were not changed or certified by these local checks.

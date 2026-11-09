# Process column layouts

Open **Processes → Columns** to choose a profile, toggle individual columns,
arrange their order or restore the original column selection.

| Profile | Focus |
| --- | --- |
| Basic | CPU, memory, I/O, network and GPU together |
| CPU | CPU, threads, priority and existing limits |
| Memory | Private working set, commit, full working set and handles |
| I/O | Existing process I/O rates, handles and command line |
| Network | Existing per-process network rates and command line |
| Diagnostics | Threads, handles, session, running time and command line |

Selecting a profile applies its columns and a relevant descending resource
sort; Diagnostics uses name order. Profiles do not change monitoring sources,
privileges, process limits or protection. Network readings still require the
existing collector and display their existing unavailable state otherwise.
Process I/O is not a new physical-disk measurement.

## Arrange columns

**Arrange columns…** opens an editor with two independently scrolling lists.
Search the available columns by their translated names and enable or disable
them. Use the named Up/Down buttons in Visible order to reorder enabled columns;
the buttons support normal keyboard focus and activation. Name cannot be
disabled or moved from first position. Hidden columns retain their place in
the full order and return there when enabled again.

**Apply** commits the draft. **Cancel**, Escape or clicking outside discards
it. Restoring defaults inside the editor changes only the draft until Apply;
the same command in the Columns menu applies immediately. Restoration concerns
column selection/order, not service settings or process actions.

The active layout is shared by List, Tree and Apps and saved with the window's
existing preferences. Hiding the active sort column falls back to Name,
ascending. Changing a layout reveals the selected process again if it still
matches the current filters. Table widths are kept per column layout so a PID
column cannot accidentally inherit a command-line column's saved width.
The former positional width cache is replaced by this layout-specific cache;
widths from earlier builds may initialize once from their defaults.

## Compatibility and scope

The saved representation retains the existing column identifiers and adds an
ordered list. Missing order uses the original order. Duplicate/unknown entries
are removed, Name is anchored first and missing known columns are appended.
Unknown future column IDs no longer discard unrelated window preferences;
an unknown saved sort falls back to Name. Malformed preference files retain
the existing fallback behaviour.

This delivers column profiles, selection, ordering and persistence from M2b.
Dragging headers, pinning Name during horizontal scrolling, user-named multiple
saved layouts and the remaining group-inspector changes are separate work.
See the [desktop plan](desktop-experience-plan.md).

## Verification

The compatibility proof failed before the change: a saved `FutureColumn`
caused the About page and German language preference to fall back to defaults.
It now preserves those settings while ignoring the unknown column. Interaction
coverage exercises all six profiles, draft isolation, Apply/Cancel, reset,
search, Escape, hidden-sort fallback and persisted order normalization.

The editor uses a fixed viewport-relative size: its search field remains in
place on the first click and when the results become empty. Both geometry and
focus are asserted by the interaction test. All data is synthetic; no host
resource exhaustion proof or service replacement is needed.

| Final local gate — 2026-11-09 | Result |
| --- | --- |
| Complete GUI suite, two test threads | 207 passed, 0 failed, 2 existing ignored |
| Locked GUI Clippy, all targets/features, warnings denied | Passed |
| rustfmt | Passed |
| GUI rustdoc with private items, warnings denied | Passed |
| Locked debug GUI build, two Cargo jobs | Passed |
| GPU review | 120 images: five languages, two themes, two sizes, six states |

The six states are the workspace, inspector, inspection hold, column editor,
Memory profile and revealed selection. Representative English wide, Turkish
compact and German light/compact editor views and the Memory profile were
visually inspected. Keyboard activation of an ordering button is exercised in
the Apply/Cancel test. This round does not repeat the previous DPI matrix or
claim installed-app, service, hosted CI or release certification.

Previews: [editor](../screenshots/process-columns.png),
[compact editor](../screenshots/process-columns-compact.png),
[Memory profile](../screenshots/process-memory-profile.png). All use synthetic
data. Local check logs are in ignored `.temp_files/columns-*` paths.

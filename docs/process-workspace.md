# Process workspace

The workspace keeps global system context visible while inspecting individual
processes. Its inspector uses existing snapshots and adds no sampler, native
query, service operation or hardware write.

Source previews: [wide workspace](../screenshots/workspace-system.png),
[pinned compact inspector](../screenshots/workspace-system-compact.png),
[Turkish light theme at 150% DPI](../screenshots/workspace-system-hidpi.png).

## Inspector

The **Inspector** menu in the footer has three saved modes:

- **Adaptive inspector**: system overview in windows at least 1180 logical
  pixels wide; selected-process details at any supported width.
- **Always show inspector**: keep the pane available, including compact windows.
- **Hide inspector**: hide the pane while retaining process selection.

With no selection, the overview shows logical CPU count and load, physical
memory use/availability, physical disk counters, busiest GPU engine and combined
adapter traffic. It reports only existing measurements, without inferring model
names, DIMM inventory, VRAM or disk health from unrelated counters. The page's
measurement-age warning still applies. **Copy system summary** copies a concise
text summary through the explicit button.

Selecting a process replaces system facts with the identity-bound process
inspector. **Close details** clears the selection and returns to the overview
when the visibility mode allows it. **Close system overview** hides the pane;
the footer menu reopens it. Each process identity and the system view have
separate scroll state. Hidden mode remains hidden until changed in the menu.

Older preferences default to Adaptive; unknown future mode identifiers also
fall back to Adaptive without discarding the other compatible preferences.

The system overview uses compact fact rows with vector badges and thin
utilization tracks. CPU, memory, disk, GPU and network facts share one layout;
process/thread/handle counts remain visible in the wide pane. Unknown CPU,
memory or GPU counters display an unavailable state, and invalid percentages
do not produce a meter. Copying the summary preserves those unknown states.
The help paragraph is available under **How inspection works**. The close
control is a compact icon with its localized accessible name and tooltip.

## Table hierarchy

Resource columns show two levels: the system total above the sortable column
name. Filters change process rows, not those totals. Memory is physical system
usage, disk is total process I/O and network covers all adapters; the tooltip
states these scopes. Missing memory capacity has no fabricated percentage.

The List/Tree/Apps selector is grouped visually. Process rows use a quiet
background with existing hover/selection and heat cells. Resource-strip values
are larger while preserving compact-window sizing and descriptive labels.
Numeric readings use larger type than their units, and long readings truncate
within their own cards. Table cells have explicit separator lines. The shared layout uses square edges,
zero outer margins between adjoining surfaces and padded content inside.
[Display preferences](display-preferences.md) expose row heights and table behavior.

## Implementation boundaries

These changes ship in 0.27.1. Application-group inspection and a sticky
process-name column remain separate work. Physical device identity/history,
SMBIOS memory inventory, battery state and read-only disk health require new
capability-aware data models. Persistent system history must extend the existing
SQLite recorder; file ownership needs a bounded worker with cancellation and
process creation-time identity. Presentation work does not implement those
backend features.

See the [desktop plan](desktop-experience-plan.md) and
[column layout guide](process-columns.md).

## Local validation — 2026-11-10

- GUI tests: 229 passed, two existing ignored tests.
- GUI Clippy (all targets/features), format, private-item documentation with
  warnings denied, and GUI build: passed.
- Current screen review: 232 English full-state renders and 144 Turkish
  primary/scroll renders at 150% DPI, both themes and viewport sizes. All 376
  passed PNG signature, dimensions, chunk CRC and bounded pixel decoding checks.
  See the [screen review](ui-screen-review.md) for the per-page checklist.
- Interaction checks cover copying, process/system transitions, saved/unknown
  modes, filtered global totals, non-overlapping headers, missing counters,
  preference persistence and reset, profiles/editor navigation, chart controls
  and compact settings actions at 150% interface scale in both themes.

Measurements are synthetic. These source changes have not been published as
a new release or installed on the developer machine.

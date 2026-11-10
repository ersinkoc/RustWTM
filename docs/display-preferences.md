# Display and workspace preferences

The current source uses square, adjoining surfaces throughout both themes.
Charts, tables, resource cards, toolbars, menus and dialogs have zero corner
radius. Neighbouring workspace surfaces join with no outer margin; their
contents have padding. Panels use 10×8 logical pixels, table cells 6 pixels
horizontally, controls 8×4 pixels, and related controls use small 6×4 gaps.
One-pixel borders and separators distinguish adjoining surfaces and table cells.
Forms use section cards and aligned labels rather than a box around every field.
See the [screen-by-screen review](ui-screen-review.md).
Chart axes, tree indentation, icon allocations and control/row heights retain
the space needed to convey their meaning and accept input.

## Appearance

Open **Settings → Appearance** for language, theme, interface scale (75–150%),
control/row height (Compact, Standard or Comfortable), reduced interface
animation and an automatic, expanded or collapsed sidebar. All height modes
keep square geometry and readable content padding. **Reset display preferences** restores
scale, heights, animation, resource-card height and automatic sidebar layout.
Language and theme keep their own existing persistence.

## Process workspace

**Settings → Process workspace** controls resource-card visibility and adaptive,
compact or full height, inspector visibility, heat shading, alternating row
backgrounds and holding live row order during inspection. Disabling the hold
option allows live sorting while inspecting; F6 still freezes a separate
snapshot. Choose List, Tree or Apps, apply any of the six column profiles, or
open the existing transactional column editor. Apply/Cancel and identity-based
process actions retain their existing behavior.

## Charts and history

**Settings → Charts and history** remembers the 1-, 2- or 5-minute chart range
and controls grid lines, area fills, recent-event markers and linked hover
cursors. A range change or chart reset returns the timeline to live. These
options use recorded samples; they do not extend collection or invent data.
Pinned inspection and sampling-gap behavior remain available.

The new preferences are stored with the local window state. Older state files
receive compatible defaults. Each settings page retains its own scroll offset.
Native startup, administrator and Task Manager integration settings remain
under **This window**. Service configuration and Telegram retain their existing
pages and permission checks.

Source previews use synthetic data and the actual egui renderer:
[process workspace](../screenshots/processes.png),
[appearance](../screenshots/settings-appearance.png),
[process preferences](../screenshots/settings-workspace.png),
[chart preferences](../screenshots/settings-charts.png).

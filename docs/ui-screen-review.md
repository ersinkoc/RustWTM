# Screen-by-screen UI review — 2026-11-10

The review covers all 18 main pages and all seven Settings subpages. Each
primary screen was inspected in dark/light themes at 1440×1000 and 820×500.
English captures use normal DPI; Turkish captures use 150% native DPI.
The views below are the actual egui implementation with synthetic data.

The fixes preserve square corners and adjoining data cells. Form rows no
longer receive arbitrary rectangle borders. Page gutters, editor cards,
wrapping control groups and consistent header/value padding replace loose
controls emitted directly against the shell.

## Main pages

| Screen and source preview | Review result |
| --- | --- |
| [Overview](../screenshots/overview.png) | Resource cards, protection summary and scrolled lower panels; retained the adjoining resource layout. |
| [Processes](../screenshots/processes.png) | Filters, sorting, system/process inspector and column editor; verified cell padding and narrow-window scrolling. |
| [Performance](../screenshots/performance.png) | CPU/memory/GPU/network/disk views, timeline, gaps and lower facts; retained bounded chart layouts. |
| [Disks](../screenshots/disks.png) | Separated back navigation from the analysis title; reviewed folders, largest files, types and disk activity. |
| [Sensors & cooling](../screenshots/sensors.png) | Added page gutters and toolbar spacing; removed boxes around form rows; reviewed cooling and frequency controls. |
| [App history](../screenshots/app-history.png) | Added bounded horizontal scrolling, padded headers and adaptive selected-chart heights; last column now reachable. |
| [Containers](../screenshots/containers.png) | Separated the count from its explanatory text; reviewed filter, table and detail/log layout. |
| [Governor](../screenshots/governor.png) | Aligned the page gutter; removed spreadsheet-style boxes around status facts. |
| [Limits](../screenshots/limits.png) | Grouped the editor in a section card; wrapped controls; added scrolling for populated budgets and row actions. |
| [Kill list](../screenshots/kill-list.png) | Grouped the editor in a section card; wrapped controls; added scrolling for populated entries and row actions. |
| [Events](../screenshots/events.png) | Applied missing numeric/header padding; reviewed long-message details and copy controls. |
| [Startup](../screenshots/startup.png) | Aligned toolbar and table gutter, completed header padding. |
| [Services](../screenshots/services.png) | Restored search spacing and separated PID/status headers and values. |
| [Users](../screenshots/users.png) | Aligned page gutter and session controls; retained horizontal/vertical list scrolling. |
| [Network](../screenshots/network.png) | Restored search/summary spacing and completed PID/protocol/header cell padding. |
| [Ports](../screenshots/ports.png) | Restored summary/filter spacing and header padding; made developer-tool scrollbars solid and discoverable. |
| [Settings](../screenshots/settings-window.png) | Reviewed every subpage separately, including native integration below the fold. |
| [About](../screenshots/about.png) | Replaced a loose text/grid stack with a hero surface and adaptive feature cards; removed nested page scrolling. |

## Settings pages

| Subpage and source preview | Review result |
| --- | --- |
| [This window](../screenshots/settings-window.png) | Language/theme, alerts, tray, startup and Task Manager integration; bottom controls checked. |
| [Appearance](../screenshots/settings-appearance.png) | Scale, row height, animation and sidebar cards; reset remains reachable. |
| [Process workspace](../screenshots/settings-workspace.png) | Inspector choices now form a wrapping horizontal group instead of a tall loose stack. |
| [Charts and history](../screenshots/settings-charts.png) | Range and chart options remain grouped in one section. |
| [Service settings](../screenshots/settings-service.png) | Replaced a two-column form with seven titled cards; bottom History section checked. |
| [config.toml](../screenshots/settings-file.png) | Added gutter and control spacing while retaining the dedicated scrolling code editor. |
| [Telegram](../screenshots/settings-telegram.png) | Removed field-row boxes and bounded text-field width for smaller viewports. |

## Additional states and validation

The review also includes configured Limits/Kill-list rows and scrolled action
columns, process/column inspectors, container and event details, all focused
performance resources, disk result modes, CPU frequency controls, long session
lists and scrolled page bottoms. Current-snapshot fixtures refresh their receipt
timestamp before capture; intentional stale-data warnings remain separate cases.

- GUI tests: **229 passed**, two existing ignored tests.
- The history-column and rule-action regressions failed before their fixes and
  passed afterward. The first exposed the hidden history Finding column; the
  second exposed a Limits Remove button beyond the compact viewport.
- English full review: **232 PNGs**. Turkish primary/scroll review at 150% DPI:
  **144 PNGs**. All **376** passed signature, dimensions, chunk CRC and bounded
  pixel-stream decoding checks. All main pages and Settings subpages were also
  visually inspected as four-state comparisons.
- Format, GUI Clippy across all targets/features, private-item documentation
  with warnings denied and GUI build passed.

[Compact budget actions](../screenshots/limits-actions-compact.png) ·
[Service settings bottom](../screenshots/settings-service-compact-bottom.png) ·
[Compact About cards](../screenshots/about-compact-bottom.png).

These are local source/build checks and synthetic renders. No new release,
installer run, service mutation, fan write or frequency write was performed.

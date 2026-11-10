# Desktop visual language

RustWTM uses square geometry, quiet neutral surfaces and resource colours that
identify data. A selected control, a warning and a chart should stand out for
different reasons. Routine controls should remain easy to find without competing
with the readings.

## Surfaces and controls

- Dark surfaces use a neutral charcoal hierarchy rather than blue tinted panels
  and default grey button blocks. Light surfaces use white controls and soft
  background contrast.
- Buttons explicitly share resting, hover, active and open colours in both themes.
  Hover does not enlarge the widget; focus remains visible.
- Resource card boundaries are neutral. Resource hues belong to readings, charts,
  gauges and small identity marks; hover/focus can accent a resource card.
- Sidebar boundaries are subdued. Group labels have their own inset and spacing.
- All pages use one workspace gutter, rather than a per-tab padding allowlist.
  Cards have internal padding and zero outer margins; their parent owns section
  spacing. Table cells remain joined with fine separators and square corners.
- Selected tabs and segmented choices share an accent underline. Primary save,
  add and analyze actions have a distinct brand treatment; utility buttons stay
  neutral. Header, tabs and settings form share one fixed reading column.
- User sessions and governor-managed processes use the same resizable, padded
  table component as the other lists, including horizontal/vertical scrolling.

## Type and density

Page headings lead the view. Inspector headings are smaller. Secondary labels use
12-point text; the footer has a consistent 12-point scale. Chart legends use
smaller text and spaced middle dots instead of crowded vertical bars.

Heat-map cells retain their ordering and thresholds with less opaque fills so
process names and numeric values remain the primary content. Enabled switches
use the brand colour with a white thumb; disabled switches stay neutral.

## Settings

Settings use a bounded reading column. Related options remain grouped, with
aligned control columns on wide screens. At narrow widths labels and controls
stack, preserving the whole interactive area. The raw configuration editor keeps
the available width. Percentage controls remain directly editable sliders. Settings
use short tab captions below 820 points, with full accessible names and tooltips.
Below 620 points, the seven tabs use balanced rows of four and three.

## Validation

The existing headless GUI suite exercises actions, sorting, scrolling, keyboard
navigation and localization. Resource and secondary text contrast checks cover
both themes. A layout regression exercises different title lengths at 960- and
420-point widths, checking common alignment and narrow-screen containment.

Visual review covers all 18 main views and all seven settings pages at 1440×1000
and 820×500 in light and dark themes, plus focused performance, dialogs and
inspection states. Synthetic fixtures do not modify services, startup entries,
hardware configuration or host resource limits.

The final GUI suite passes 238 tests, with three explicit opt-outs (benchmark,
asset regeneration and the opt-in hardware smoke). Clippy covers all GUI targets
and features with warnings denied. Evidence is retained locally under
`.temp_files/hardware-copy-layout/`: alignment and hardware-copy red/green results, contrast and interaction
checks, complete screen matrices and build logs. These are local checks, not a
published release or a test of a running installed application.

The last source snapshot produced 116 primary/settings captures across both
sizes and themes; PNG headers, chunk checksums and bounded pixel streams passed
validation. An earlier 204-state pass also covered the focused resource pages and
inspection/dialog fixtures. Repository preview images were refreshed from these
captures. The final native development build succeeded at `target/debug/rustwtm.exe`.

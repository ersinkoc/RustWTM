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
  neutral. Settings tabs and forms share the full workspace width and gutters.
- User sessions and governor-managed processes use the same resizable, padded
  table component as the other lists, including horizontal/vertical scrolling.

## Type and density

Page content starts at the workspace gutter. Inspector headings identify the selected item. Secondary labels use
12-point text; the footer has a consistent 12-point scale. Chart legends use
smaller text and spaced middle dots instead of crowded vertical bars.

Heat-map cells retain their ordering and thresholds with less opaque fills so
process names and numeric values remain the primary content. Enabled switches
use the brand colour with a white thumb; disabled switches stay neutral.

## Settings

Settings span the available workspace width. Related options remain grouped, with
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

The 0.27.2 visual review covered all 18 main views and all seven settings pages at 1440×1000
and 820×500 in light and dark themes, plus focused performance, dialogs and
inspection states. Synthetic fixtures do not modify services, startup entries,
hardware configuration or host resource limits.

That release's GUI suite passed 238 tests, with three explicit opt-outs (benchmark,
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

The later inventory/application work adds two main pages. Its 36 dedicated
captures cover hardware specifications and installed application selection,
confirmation and compact scrolling in English/Turkish, both themes and sizes.
The GUI suite also exercises navigation through all 20 pages in five languages.
This local development pass is recorded in
[inventory and application management](inventory-app-management.md).

## Desktop refinement, 2026-11-10

The shared layout follows the component composition and explicit gap/padding
approach described in the [GPUI layout examples](https://gpui.rs/examples/).
These changes are original implementations in the existing egui desktop. No
GPUI runtime dependency or renderer migration is included in this pass.

- Page titles sit on a distinct raised header surface. Their text shares the
  content gutter; decorative icons do not push the title off that alignment.
- The graphite dark palette separates workspace, raised panels, inputs and
  hover states. White panels on a soft neutral background provide the light
  equivalent. Status and resource colours retain their semantic meaning.
- Navigation has inset selection, an accent marker, quiet unselected rows and
  a visible Ctrl K hint. All destinations remain reachable in shorter windows.
- Overview resource cards share title, large reading, detail and graph bands.
  The resource identity line and graph use colour; ordinary readings use ink.
  High load retains warning/error colours. Cards still open resource details.
- Cards have 12 x 10 internal padding and explicit section dividers. Page
  gutters are 16 x 12; short windows use 12 x 8 and tighter status-strip padding to
  preserve selected details and actions. Corners remain square.
- Settings rows retain aligned control columns with separators. Service policy
  cards form two columns at 860 points of content width and stack below that.
  No configuration semantics or defaults change with the layout.
- Compact container selection reserves more room for its details and actions.

Validation uses the existing synthetic UI fixtures. Screenshots and interaction
checks do not control processes, install applications, modify hardware or
change the developer machine's services. Current local evidence is retained in
`.temp_files/desktop-polish/`; this is independent of an installed app update or
published release.

The final local pass passed 246 GUI tests (three explicit opt-outs), GUI Clippy
for all targets/features with warnings denied, formatting and a native GUI build.
The screen matrix contains 334 validated PNGs: all 20 main pages and seven
settings views in light/dark at wide/compact widths, focused process/performance
states, English/Turkish quick navigation, and a targeted Turkish page/settings
review. The full primary matrix was visually inspected through contact sheets,
with individual checks for resource, inspector and translated settings layouts.
The separate preview is `target/preview/rustwtm-desktop-polish.exe`.

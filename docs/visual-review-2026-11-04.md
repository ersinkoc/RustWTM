# Resource chart visual review — 2026-11-04

## Reproduced defect

The Overview CPU, memory and GPU gauges had flat cropped edges. The old gauge
reserved half the track stroke outside its radius, assuming a centered stroke.
In the pinned epaint 0.37.0 renderer, `CircleShape` strokes extend outside the
radius. The track therefore extended past the gauge's own clip rectangle.

A regression tessellates the real `ring_gauge` output and checks that painted
vertices fit its allocated rectangle. Before the fix, a 48-point gauge painted
a vertex at `[58.1, 32.0]` beyond its right boundary of `56.0`. The assertion
failed on the actual geometry, without screenshots or host-resource exhaustion.

The track now uses a centered closed path, with explicit room for stroke
feathering and scale marks. Partial fills have round caps; 100% closes the path
without overlapping caps. Arc detail scales with DPI and is capped at 256
segments. No background animation or new dependencies are introduced.

## Visual changes

- CPU, memory and GPU retain their resource hues until warning thresholds.
  Rings include an inner rim, quiet center tint, scale marks and a tonal fill.
- Overview cards have larger gauges and rate values, more room for histories
  and subtle horizontal reference lines.
- Shared sparklines have fading area fills and a latest-point marker where
  space permits. Their strokes stay inset from the plot edges at 0% and 100%.
- Performance plots use a quieter grid and background, slightly taller charts
  and stronger lines. Logical-processor tiles have a subtle load tint.
- The screenshot-only fixture now populates history timestamps, so Performance
  previews exercise the real time-based curves instead of showing empty axes.

## Evidence and scope

The geometry regression covers 48, 60, 72 and 100 points at 1x, 1.25x, 1.5x and
2x DPI, light/dark themes, and 0%, 1%, 42%, 99% and 100%: 160 combinations.
All passed after the fix. The full GUI suite passed 176 tests with 2 existing
ignored cases. Workspace all-target/all-feature Clippy passed with warnings
denied.

GPU review artifacts are generated under `target/visual-review/`: the gauge
gallery covers both themes at all four DPI scales; Overview, Performance and
Containers are rendered in all five supported languages at wide and compact
window sizes. Screenshots use synthetic fixtures and do not contain real
workload data. The final gate logs are under ignored `.temp_files/round47-*`.

| Final local gate | Result |
| --- | --- |
| rustfmt | PASS |
| Locked workspace Clippy, all targets/features, warnings denied | PASS |
| Complete GUI suite, two test threads | 176 passed, 0 failed, 2 existing ignored |
| GUI rustdoc, private items, warnings denied | PASS |
| GPU gauge gallery: two themes, four DPI scales | PASS; 10.32 seconds |
| GPU page review: five languages, two themes, two sizes | PASS; 135.24 seconds |
| Optimized GUI build, two Cargo jobs | PASS; 3m 24s |
| Diff whitespace and preservation of five user configuration files | PASS |

Wide English/Turkish Overview, wide Performance, compact German Overview,
compact French Performance, container details, and the light/dark gauge
galleries were visually inspected. The repository's README previews are updated
from these deterministic renders. Root/native tests, hosted CI and release
publication were not rerun for this UI-only change.

These UI checks were completed before 0.25.0 release preparation. Publication
and downloaded-artifact validation are separate gates. No installed application
or service is replaced by these local checks.

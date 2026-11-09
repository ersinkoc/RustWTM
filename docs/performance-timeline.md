# Performance timeline

The Performance page supports inspecting the recent CPU, memory, GPU, disk
and network history at a shared time. These interactions use the existing
bounded sample buffer; they do not start another collector.

## Controls

- Choose **1 min**, **2 min** or **5 min**. A range is enabled only when the
  recorded timestamps provide sufficient coverage. The buffer retains at most
  300 samples, so its duration depends on the sampling interval.
- Hover a main chart to show a shared horizontal-time cursor across charts.
  Click a chart to pin the time and enter **Inspecting** mode. The absolute
  window and selected time stay fixed as new samples arrive.
- Use **Inspect timeline** to start at the latest sample without a pointer.
  **Previous sample**, **Next sample**, and the left/right arrow keys step
  through actual recorded timestamps. Arrow shortcuts pause while editing
  text or using a popup or modal.
- **Zoom in** and **Zoom out** change the window around the selected time.
  **Reset zoom** restores the selected preset at the latest recorded time,
  while remaining in inspection mode. Vertical scrolling still scrolls the
  page; wheel/drag zoom is not enabled.
- **Back to live**, or a double-click on a chart, resumes the moving window.
- **Recent events in view** lists up to 32 recent events in the visible
  window. Selecting one pins its recorded time; event markers appear on the
  main charts. A marker indicates timing, not proof of causation.

The timestamp includes the local date, time and UTC offset. Chart axes use
local clock time. The values below the charts follow the selected sample;
chart headers, per-core tiles and device details continue showing current
readings. The page explicitly labels this distinction.

## Missing and expired data

Plots use real sample times rather than evenly spaced indices. Missing or
invalid values and sampling gaps break lines instead of inserting zeroes.
Readouts use the nearest real sample only within two estimated sampling
intervals and within the recorded extent; a selection inside a larger gap
shows an unavailable message and dashes. The selected timestamp shows the
actual sample time when one is available.

If the buffer evicts the selected sample, inspection retains its original
window and reports that no sample is available. It does not silently advance
the selection. Duplicate, reversed or invalid timestamps start a new valid
suffix. An empty buffer reserves the normal chart area and explains that no
samples have been recorded. The existing monitoring-age warning continues
to describe stale data independently of timeline selection.

## Scope and validation

This is the initial chart-interaction portion of M3. [Focused resource
views](performance-resources.md) are now available. Physical device selection,
device-selection persistence, rate-axis hysteresis, alternative network
units and persistent system history remain pending. Interaction state is
transient; it does not change service configuration or protocol contracts.

Local validation on 2026-11-09:

- GUI suite: 214 passed, two existing ignored visual tests.
- GUI Clippy, all targets and features: passed with warnings denied.
- Serial GPU review: 80 images across English, Turkish, German, French and
  Spanish; light/dark themes; wide/compact sizes; live, inspecting, gap and
  stale-data states. The scoped run excluded unrelated power-control renders.
- Interaction tests cover pinned-window stability, shared sample values,
  zoom/reset, keyboard stepping, event selection in a gap, return to live,
  double-click timing and existing Overview-to-Performance navigation.
- Timestamp tests cover bounded buffers, clock resets, gaps and invalid values.

All fixture measurements are synthetic. These checks do not certify the
installed application, service, hosted CI or a published release.

Previews: [wide inspection](../screenshots/performance-timeline.png),
[compact inspection](../screenshots/performance-timeline-compact.png),
[sampling gap](../screenshots/performance-gap.png).

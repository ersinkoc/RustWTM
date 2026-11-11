//! Bounded, timestamp-based interaction shared by the system history plots.

use crate::backend::Data;
use eframe::egui::{self, Color32, RichText, Ui};
use egui_plot::{Line, Plot, Points, VLine};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub start: u64,
    pub end: u64,
}

#[derive(Default)]
struct Timeline {
    times: Vec<u64>,
    offset: usize,
    cadence: u64,
    restarted: bool,
}

fn valid_time(at: u64) -> bool {
    i64::try_from(at)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .is_some()
}

impl Timeline {
    fn read(times: &VecDeque<u64>) -> Self {
        let retained_start = times.len().saturating_sub(crate::backend::HISTORY_LEN);
        let mut offset = retained_start;
        let mut restarted = false;
        for i in retained_start..times.len() {
            if !valid_time(times[i]) {
                offset = i + 1;
                restarted = true;
            } else if i > offset && times[i] <= times[i - 1] {
                offset = i;
                restarted = true;
            }
        }
        let times: Vec<_> = times.iter().skip(offset).copied().collect();
        let mut deltas: Vec<_> = times.windows(2).map(|p| p[1] - p[0]).collect();
        deltas.sort_unstable();
        let cadence = deltas
            .get(deltas.len().saturating_sub(1) / 2)
            .copied()
            .unwrap_or(1000)
            .clamp(1, 5000);
        Self {
            times,
            offset,
            cadence,
            restarted,
        }
    }

    fn extent(&self) -> Option<Window> {
        Some(Window {
            start: *self.times.first()?,
            end: *self.times.last()?,
        })
    }

    fn nearest(&self, at: u64) -> Option<(usize, u64)> {
        let extent = self.extent()?;
        if at < extent.start || at > extent.end {
            return None;
        }
        let (i, &time) = self
            .times
            .iter()
            .enumerate()
            .min_by_key(|(_, t)| t.abs_diff(at))?;
        (time.abs_diff(at) <= self.cadence.saturating_mul(2)).then_some((self.offset + i, time))
    }
}

#[derive(Clone, Copy)]
pub enum Unit {
    Percent,
    Rate,
}

impl Unit {
    fn valid(self, v: f32) -> bool {
        v.is_finite() && v >= 0.0 && (!matches!(self, Self::Percent) || v <= 100.0)
    }
    fn text(self, value: Option<f32>) -> String {
        value.filter(|v| self.valid(*v)).map_or_else(
            || "—".into(),
            |v| match self {
                Self::Percent => format!("{v:.1}%"),
                Self::Rate => format!("{}/s", wtm_core::format_bytes(f64::from(v))),
            },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Options {
    pub grid: bool,
    pub fill: bool,
    pub events: bool,
    pub link_hover: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            grid: true,
            fill: true,
            events: true,
            link_hover: true,
        }
    }
}

pub struct State {
    pub plot_height: f32,
    pub options: Options,
    range_secs: u32,
    pub window: Option<Window>,
    pub cursor: Option<u64>,
    timeline: Timeline,
    markers: Vec<(u64, Color32)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            plot_height: 128.0,
            options: Options::default(),
            range_secs: 300,
            window: None,
            cursor: None,
            timeline: Timeline::default(),
            markers: Vec::new(),
        }
    }
}

pub fn timestamp(at: u64) -> String {
    i64::try_from(at)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S %:z")
                .to_string()
        })
        .unwrap_or_else(|| "—".into())
}

fn short_time(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 || seconds * 1000.0 > i64::MAX as f64 {
        return "—".into();
    }
    chrono::DateTime::from_timestamp_millis((seconds * 1000.0) as i64)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "—".into())
}

impl State {
    pub fn range_seconds(&self) -> u32 {
        self.range_secs
    }
    pub fn set_range(&mut self, seconds: u32) {
        self.range_secs = if matches!(seconds, 60 | 120 | 300) {
            seconds
        } else {
            300
        };
        self.live();
    }
    fn live_window(&self) -> Option<Window> {
        let extent = self.timeline.extent()?;
        Some(Window {
            start: extent
                .end
                .saturating_sub(u64::from(self.range_secs) * 1000)
                .max(extent.start),
            end: extent.end,
        })
    }
    fn bounds(&self) -> Option<Window> {
        self.window.or_else(|| self.live_window())
    }
    fn live(&mut self) {
        self.window = None;
        self.cursor = None;
    }
    fn inspect(&mut self) {
        self.window = self.bounds();
        self.cursor = self.window.map(|w| w.end);
    }

    fn zoom(&mut self, inward: bool) {
        let (Some(current), Some(extent)) = (self.bounds(), self.timeline.extent()) else {
            return;
        };
        let full = extent.end.saturating_sub(extent.start);
        if full == 0 {
            return;
        }
        let old_width = current.end.saturating_sub(current.start);
        let width = if inward {
            (old_width / 2).max(1000)
        } else {
            old_width.saturating_mul(2)
        }
        .min(full);
        let center = self
            .cursor
            .unwrap_or(current.start + old_width / 2)
            .clamp(extent.start, extent.end);
        let start = center
            .saturating_sub(width / 2)
            .max(extent.start)
            .min(extent.end - width);
        self.window = Some(Window {
            start,
            end: start + width,
        });
        self.cursor = Some(center);
    }

    fn step(&mut self, next: bool) {
        let Some(at) = self.cursor else {
            return;
        };
        let time = if next {
            self.timeline.times.iter().copied().find(|t| *t > at)
        } else {
            self.timeline.times.iter().copied().rev().find(|t| *t < at)
        };
        let Some(time) = time else {
            return;
        };
        self.cursor = Some(time);
        if let Some(window) = &mut self.window {
            let width = window.end.saturating_sub(window.start);
            if time < window.start {
                window.start = time;
                window.end = time.saturating_add(width);
            }
            if time > window.end {
                window.end = time;
                window.start = time.saturating_sub(width);
            }
        }
    }

    pub fn controls(&mut self, ui: &mut Ui, data: &Data) {
        self.timeline = Timeline::read(&data.history.times);
        let extent = self.timeline.extent();
        if self.window.is_some()
            && !ui.ctx().text_edit_focused()
            && !egui::Popup::is_any_open(ui.ctx())
            && ui.memory(|m| m.top_modal_layer().is_none())
        {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)) {
                self.step(false);
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)) {
                self.step(true);
            }
        }
        let toggle_width = ui.fonts_mut(|f| {
            [
                crate::i18n::tr("Inspect timeline"),
                crate::i18n::tr("Back to live"),
            ]
            .iter()
            .map(|text| {
                f.layout_no_wrap(
                    (*text).into(),
                    egui::TextStyle::Button.resolve(ui.style()),
                    Color32::WHITE,
                )
                .size()
                .x
            })
            .fold(0.0, f32::max)
        }) + 2.0 * ui.spacing().button_padding.x
            + 4.0;
        ui.horizontal_wrapped(|ui| {
            ui.strong(crate::i18n::tr("Timeline"));
            for (secs, label) in [(60, "1 min"), (120, "2 min"), (300, "5 min")] {
                let recorded = extent.is_some_and(|e| {
                    e.end - e.start + self.timeline.cadence >= u64::from(secs) * 1000
                });
                if ui
                    .add_enabled(
                        recorded,
                        egui::Button::selectable(
                            recorded && self.range_secs == secs,
                            crate::i18n::tr(label),
                        ),
                    )
                    .on_hover_text(crate::i18n::tr("Only recorded ranges are available."))
                    .clicked()
                {
                    self.range_secs = secs;
                    self.live();
                }
            }
            let was_live = self.window.is_none();
            let label = crate::i18n::tr(if was_live {
                "Inspect timeline"
            } else {
                "Back to live"
            });
            if ui
                .add_enabled_ui(extent.is_some(), |ui| {
                    ui.add_sized(
                        [toggle_width, ui.spacing().interact_size.y],
                        egui::Button::new(label),
                    )
                })
                .inner
                .clicked()
            {
                if was_live {
                    self.inspect();
                } else {
                    self.live();
                }
            }
            let width = self.bounds().map_or(0, |w| w.end - w.start);
            if ui
                .add_enabled(width > 1000, egui::Button::new(crate::i18n::tr("Zoom in")))
                .clicked()
            {
                self.zoom(true);
            }
            if ui
                .add_enabled(
                    self.window.is_some(),
                    egui::Button::new(crate::i18n::tr("Zoom out")),
                )
                .clicked()
            {
                self.zoom(false);
            }
            if ui
                .add_enabled(
                    self.window.is_some(),
                    egui::Button::new(crate::i18n::tr("Reset zoom")),
                )
                .clicked()
            {
                self.window = self.live_window();
                self.cursor = self.window.map(|w| w.end);
            }
        });
        ui.horizontal_wrapped(|ui| {
            let count = self.timeline.times.len();
            let span = extent.map_or(0, |e| (e.end - e.start) / 1000);
            ui.weak(crate::i18n::format(
                "Recorded: {0}s · {1} samples",
                &[span.to_string(), count.to_string()],
            ));
            let inspecting = self.window.is_some();
            // Reserve the same mode-label width so pinning does not move a plot.
            ui.add_sized(
                [90.0, ui.spacing().interact_size.y],
                egui::Label::new(
                    RichText::new(crate::i18n::tr(if inspecting {
                        "Inspecting"
                    } else {
                        "Live"
                    }))
                    .small()
                    .color(crate::theme::text_color(
                        ui,
                        if inspecting {
                            crate::theme::ACCENT
                        } else {
                            crate::theme::GOOD
                        },
                    )),
                ),
            );
            let at = self
                .cursor
                .or_else(|| self.timeline.times.last().copied())
                .unwrap_or_default();
            if ui
                .add_enabled(
                    inspecting && self.timeline.times.iter().any(|t| *t < at),
                    egui::Button::new(crate::i18n::tr("Previous sample")),
                )
                .clicked()
            {
                self.step(false);
            }
            if ui
                .add_enabled(
                    inspecting && self.timeline.times.iter().any(|t| *t > at),
                    egui::Button::new(crate::i18n::tr("Next sample")),
                )
                .clicked()
            {
                self.step(true);
            }
            let requested = self.cursor.or_else(|| self.timeline.times.last().copied());
            let actual = requested.map(|at| self.timeline.nearest(at).map_or(at, |(_, t)| t));
            ui.label(
                RichText::new(actual.map_or_else(|| "—".into(), timestamp))
                    .small()
                    .monospace(),
            );
        });
        ui.add(egui::Label::new(RichText::new(crate::i18n::tr("Headers and device details show current readings. Values below charts follow the selected sample.")).small().weak()).wrap());
        let warning = if self.timeline.restarted {
            crate::i18n::tr("Only the latest increasing timestamp sequence is shown.")
        } else if self.window.is_some()
            && self
                .cursor
                .is_some_and(|at| self.timeline.nearest(at).is_none())
        {
            crate::i18n::tr("No recorded sample at this time.")
        } else {
            ""
        };
        ui.add_sized(
            [ui.available_width(), 14.0],
            egui::Label::new(
                RichText::new(warning)
                    .small()
                    .color(crate::theme::text_color(ui, crate::theme::WARN)),
            )
            .truncate(),
        );
        self.markers.clear();
        if let Some(window) = self.bounds() {
            let events: Vec<_> = data
                .events
                .iter()
                .rev()
                .filter(|e| e.unix_ms >= window.start && e.unix_ms <= window.end)
                .take(32)
                .collect();
            for event in &events {
                self.markers.push((
                    event.unix_ms,
                    match event.level {
                        wtm_ipc::EventLevel::Error => crate::theme::BAD,
                        wtm_ipc::EventLevel::Warn => crate::theme::WARN,
                        _ => crate::theme::ACCENT,
                    },
                ));
            }
            ui.add_enabled_ui(!events.is_empty(), |ui| {
                ui.menu_button(
                    crate::i18n::format("Recent events in view ({0})", &[events.len().to_string()]),
                    |ui| {
                        for e in events {
                            let message = crate::i18n::message(&e.message);
                            if ui
                                .add(
                                    egui::Button::new(format!(
                                        "{} · {}",
                                        timestamp(e.unix_ms),
                                        message.chars().take(100).collect::<String>()
                                    ))
                                    .truncate(),
                                )
                                .on_hover_text(message.as_ref())
                                .clicked()
                            {
                                self.window = Some(window);
                                self.cursor = Some(e.unix_ms);
                                ui.close();
                            }
                        }
                    },
                );
            });
        }
        ui.add_space(0.0);
    }

    fn value(&self, times: &VecDeque<u64>, values: &VecDeque<f32>, unit: Unit) -> Option<f32> {
        if times.len() != values.len() {
            return None;
        }
        let (i, _) = self.timeline.nearest(self.cursor?)?;
        values.get(i).copied().filter(|v| unit.valid(*v))
    }

    pub fn plot(
        &mut self,
        ui: &mut Ui,
        id: &str,
        times: &VecDeque<u64>,
        series: &[(&str, &VecDeque<f32>, Color32)],
        unit: Unit,
    ) {
        let Some(window) = self.bounds() else {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 146.0),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    ui.set_min_height(146.0);
                    ui.weak(crate::i18n::tr("No recorded samples yet."));
                },
            );
            return;
        };
        let start = window.start as f64 / 1000.0;
        let end = (window.end as f64 / 1000.0).max(start + 0.001);
        let mut plot = Plot::new(id)
            .height(self.plot_height)
            .show_background(false)
            .show_grid([self.options.grid, self.options.grid])
            .grid_color(ui.visuals().weak_text_color().gamma_multiply(0.35))
            .y_axis_min_width(64.0)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .allow_boxed_zoom(false)
            .x_axis_formatter(|mark, _| short_time(mark.value));
        if self.options.link_hover {
            plot = plot.link_cursor(egui::Id::unique("performance-time-cursor"), [true, false]);
        }
        plot = match unit {
            Unit::Percent => plot
                .include_y(0.0)
                .include_y(100.0)
                .y_axis_formatter(|m, _| format!("{:.0}%", m.value))
                .label_formatter(|hover| match hover {
                    egui_plot::HoverPosition::NearDataPoint {
                        plot_name,
                        position,
                        ..
                    } => Some(format!(
                        "{} · {plot_name}: {:.1}%",
                        short_time(position.x),
                        position.y
                    )),
                    _ => None,
                }),
            Unit::Rate => plot
                .include_y(0.0)
                .include_y(1024.0)
                .y_axis_formatter(|m, _| format!("{}/s", wtm_core::format_bytes(m.value.max(0.0))))
                .label_formatter(crate::app::rate_label),
        };
        let result = plot.show(ui, |plot| {
            plot.set_plot_bounds_x(start..=end);
            if matches!(unit, Unit::Percent) {
                plot.set_plot_bounds_y(0.0..=100.0);
            }
            for &(label, values, color) in series {
                for points in segments(&self.timeline, times, values, window, unit) {
                    if points.len() == 1 {
                        plot.points(Points::new(label, points).color(color).radius(2.5));
                    } else {
                        plot.line(
                            Line::new(label, points)
                                .color(color)
                                .width(2.0)
                                .fill(0.0)
                                .fill_alpha(if self.options.fill { 0.15 } else { 0.0 }),
                        );
                    }
                }
            }
            for &(at, color) in self.markers.iter().filter(|_| self.options.events) {
                plot.vline(
                    VLine::new("", at as f64 / 1000.0)
                        .color(color.gamma_multiply(0.5))
                        .allow_hover(false),
                );
            }
            if let Some(at) = self.cursor {
                plot.vline(
                    VLine::new("", at as f64 / 1000.0)
                        .color(crate::theme::ACCENT)
                        .allow_hover(false),
                );
            }
            plot.pointer_coordinate()
        });
        let accessible_name = crate::i18n::format(
            "{0} history graph",
            &[series.first().map_or("", |s| s.0).into()],
        );
        result.response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Image, ui.is_enabled(), &accessible_name)
        });
        if result.response.double_clicked() {
            self.live();
            ui.ctx().request_repaint();
        } else if result.response.clicked()
            && let Some(point) = result.inner
            && point.x.is_finite()
            && point.x >= start
            && point.x <= end
        {
            self.window = Some(window);
            self.cursor = Some((point.x * 1000.0) as u64);
            ui.ctx().request_repaint();
        }
        crate::context_actions::menu(&result.response, ("timeline-chart", id), |ui| {
            if crate::context_actions::action(ui, crate::i18n::tr("Live")) {
                self.live();
            }
            if crate::context_actions::action(ui, crate::i18n::tr("Reset zoom")) {
                self.live();
            }
            crate::context_actions::copy(ui, crate::i18n::tr("Current"), || {
                series
                    .iter()
                    .map(|(label, values, _)| {
                        let value = if self.cursor.is_some() {
                            self.value(times, values, unit)
                        } else {
                            values
                                .back()
                                .copied()
                                .filter(|v| unit.valid(*v) && values.len() == times.len())
                        };
                        format!("{label}: {}", unit.text(value))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            ui.checkbox(&mut self.options.grid, crate::i18n::tr("Show chart grid"));
        });
        result.response.on_hover_text(crate::i18n::tr(
            "Click a chart to inspect. Double-click to follow live.",
        ));
        ui.horizontal(|ui| {
            ui.set_min_height(18.0);
            if self.window.is_some() {
                for &(label, values, color) in series {
                    ui.label(
                        RichText::new(format!(
                            "{label}: {}",
                            unit.text(self.value(times, values, unit))
                        ))
                        .small()
                        .color(crate::theme::text_color(ui, color)),
                    );
                }
            } else if let Some(at) = self.timeline.times.last() {
                if series.iter().any(|(_, values, _)| {
                    values.len() == times.len() && values.back().is_some_and(|v| unit.valid(*v))
                }) {
                    ui.weak(crate::i18n::format(
                        "Latest sample: {0}",
                        &[short_time(*at as f64 / 1000.0)],
                    ));
                } else {
                    ui.weak(crate::i18n::tr("Latest sample unavailable"));
                }
            }
        });
    }
}

fn segments(
    timeline: &Timeline,
    times: &VecDeque<u64>,
    values: &VecDeque<f32>,
    window: Window,
    unit: Unit,
) -> Vec<Vec<[f64; 2]>> {
    if times.len() != values.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut current = Vec::new();
    let mut previous = None;
    for (i, &at) in timeline.times.iter().enumerate() {
        let value = values[timeline.offset + i];
        let valid = at >= window.start && at <= window.end && unit.valid(value);
        if (!valid
            || previous.is_some_and(|before| at - before > timeline.cadence.saturating_mul(3)))
            && !current.is_empty()
        {
            out.push(std::mem::take(&mut current));
        }
        if valid {
            current.push([at as f64 / 1000.0, f64::from(value)]);
            previous = Some(at);
        } else {
            previous = None;
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histories_preserve_real_times_break_gaps_and_reject_missing_values() {
        let times = VecDeque::from([1000, 2000, 3000, 20_000, 21_000]);
        let values = VecDeque::from([10.0, 20.0, 30.0, 40.0, 50.0]);
        let timeline = Timeline::read(&times);
        let window = timeline.extent().unwrap();
        let split = segments(&timeline, &times, &values, window, Unit::Percent);
        assert_eq!(split.len(), 2);
        assert_eq!(split[0], [[1.0, 10.0], [2.0, 20.0], [3.0, 30.0]]);
        assert_eq!(split[1], [[20.0, 40.0], [21.0, 50.0]]);
        let mut missing = values.clone();
        missing[1] = f32::NAN;
        assert_eq!(
            segments(&timeline, &times, &missing, window, Unit::Percent).len(),
            3
        );
        missing.pop_back();
        assert!(segments(&timeline, &times, &missing, window, Unit::Percent).is_empty());
        assert!(timeline.nearest(10_000).is_none());
    }

    #[test]
    fn clock_reversal_and_invalid_times_keep_only_an_aligned_bounded_suffix() {
        let times = VecDeque::from([1000, 2000, 1500, 2500]);
        let values = VecDeque::from([10.0, 20.0, 30.0, 40.0]);
        let timeline = Timeline::read(&times);
        assert!(timeline.restarted);
        let split = segments(
            &timeline,
            &times,
            &values,
            timeline.extent().unwrap(),
            Unit::Percent,
        );
        assert_eq!(split[0], [[1.5, 30.0], [2.5, 40.0]]);
        assert!(
            Timeline::read(&VecDeque::from([1000, u64::MAX]))
                .times
                .is_empty()
        );
        let large: VecDeque<_> = (0..1000).map(|i| i * 1000).collect();
        let bounded = Timeline::read(&large);
        assert_eq!(bounded.times.len(), crate::backend::HISTORY_LEN);
        assert_eq!(bounded.offset, 1000 - crate::backend::HISTORY_LEN);
    }

    #[test]
    fn inspection_is_anchored_and_expiry_does_not_fabricate_a_value() {
        let times: VecDeque<_> = (0..300).map(|i| i * 1000).collect();
        let values = VecDeque::from(vec![42.0; 300]);
        let mut state = State {
            timeline: Timeline::read(&times),
            ..Default::default()
        };
        state.inspect();
        state.zoom(true);
        state.step(false);
        let window = state.window;
        let cursor = state.cursor;
        assert_eq!(state.value(&times, &values, Unit::Percent), Some(42.0));
        let newer: VecDeque<_> = (100..400).map(|i| i * 1000).collect();
        state.timeline = Timeline::read(&newer);
        assert_eq!(state.window, window);
        assert_eq!(state.cursor, cursor);
        let expired: VecDeque<_> = (400..700).map(|i| i * 1000).collect();
        state.timeline = Timeline::read(&expired);
        assert!(state.value(&expired, &values, Unit::Percent).is_none());
        assert_eq!(state.window, window);
        state.live();
        assert_eq!(state.bounds().unwrap().end, 699_000);
        assert!(state.cursor.is_none());
    }
}

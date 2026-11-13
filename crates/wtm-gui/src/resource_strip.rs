//! Compact system context for the process workspace. Reads existing samples;
//! no polling, native queries or control actions live in this view.

use std::collections::VecDeque;

use eframe::egui::{self, Color32, Ui};
use wtm_core::format_bytes;

use crate::backend::Data;
use crate::overview::Resource;
use crate::theme;

struct Metric<'a> {
    resource: Resource,
    title: &'static str,
    value: String,
    detail: String,
    color: Color32,
    series: Vec<(&'a VecDeque<f32>, Color32)>,
    ceiling: Option<f32>,
}

/// Returns navigation only. Each card is also a keyboard-accessible button.
pub fn show(ui: &mut Ui, data: &Data, band: crate::preferences::Band) -> Option<Resource> {
    let snap = data.snapshot.as_ref()?;
    let s = &snap.system;
    let h = &data.history;
    let memory = s.memory_used() as f64 * 100.0 / s.memory_total.max(1) as f64;
    let metrics = [
        Metric {
            resource: Resource::Cpu,
            title: "CPU",
            value: format!("{:.0}%", s.cpu_percent),
            detail: crate::i18n::format("{0} logical processors", &[s.logical_cpus.to_string()]),
            color: theme::CPU,
            series: vec![(&h.cpu, theme::CPU)],
            ceiling: Some(100.0),
        },
        Metric {
            resource: Resource::Memory,
            title: crate::i18n::tr("Memory"),
            value: format_bytes(s.memory_used() as f64),
            detail: format!("{memory:.0}% · {}", format_bytes(s.memory_total as f64)),
            color: theme::MEMORY,
            series: vec![(&h.memory, theme::MEMORY)],
            ceiling: Some(100.0),
        },
        Metric {
            resource: Resource::Disk,
            title: crate::i18n::tr("Disk & I/O"),
            value: format!("{}/s", format_bytes(s.io_read_rate + s.io_write_rate)),
            detail: crate::i18n::tr("Read / Write").into(),
            color: theme::DISK_READ,
            series: vec![
                (&h.io_read, theme::DISK_READ),
                (&h.io_write, theme::DISK_WRITE),
            ],
            ceiling: None,
        },
        Metric {
            resource: Resource::Network,
            title: crate::i18n::tr("Network"),
            value: format!("{}/s", format_bytes(s.net_rx_rate + s.net_tx_rate)),
            detail: format!(
                "↓ {} · ↑ {}",
                format_bytes(s.net_rx_rate),
                format_bytes(s.net_tx_rate)
            ),
            color: theme::NET_RX,
            series: vec![(&h.net_rx, theme::NET_RX), (&h.net_tx, theme::NET_TX)],
            ceiling: None,
        },
        Metric {
            resource: Resource::Gpu,
            title: "GPU",
            value: format!("{:.0}%", s.gpu_percent),
            detail: crate::i18n::tr("busiest engine").into(),
            color: theme::GPU,
            series: vec![(&h.gpu, theme::GPU)],
            ceiling: Some(100.0),
        },
    ];
    let short = match band {
        crate::preferences::Band::Compact => true,
        crate::preferences::Band::Full => false,
        crate::preferences::Band::Adaptive => ui.ctx().content_rect().height() < 640.0,
    };
    let width = ui.available_width();
    let columns = if width >= 650.0 { 5 } else { 3 };
    // The cards of a row form one band: no gaps, a shared outline with
    // rounded ends and a hairline between neighbours.
    let gap = 0.0;
    let mut clicked = None;
    for chunk in metrics.chunks(columns) {
        let mut cards = Vec::with_capacity(chunk.len());
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let weighted = columns == 5 && width >= 1000.0;
            let unit = (width - gap * (columns - 1) as f32)
                / (columns as f32 + if weighted { 0.4 } else { 0.0 });
            for (index, m) in chunk.iter().enumerate() {
                let ends = |first: bool, last: bool| {
                    let r = |on: bool| if on { 8 } else { 0 };
                    egui::CornerRadius {
                        nw: r(first),
                        sw: r(first),
                        ne: r(last),
                        se: r(last),
                    }
                };
                let radius = ends(index == 0, index + 1 == chunk.len());
                let w = if weighted && m.resource == Resource::Cpu {
                    unit * 1.4
                } else {
                    unit
                };
                ui.push_id(m.resource as u8, |ui| {
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(w, if short { 78.0 } else { 126.0 }),
                        egui::Sense::click(),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Button,
                            ui.is_enabled(),
                            format!("{}: {}", m.title, m.value),
                        )
                    });
                    let painter = ui.painter().with_clip_rect(rect);
                    let fill = if response.hovered() {
                        ui.visuals().widgets.hovered.bg_fill
                    } else {
                        theme::card_fill(ui)
                    };
                    painter.rect_filled(rect, radius, fill);
                    cards.push(rect);
                    let inner = rect.shrink2(egui::vec2(10.0, 6.0));
                    let color = theme::text_color(ui, m.color);
                    painter.circle_filled(inner.left_top() + egui::vec2(3.0, 7.0), 2.5, color);
                    painter.text(
                        inner.left_top() + egui::vec2(12.0, 0.0),
                        egui::Align2::LEFT_TOP,
                        m.title,
                        egui::FontId::proportional(12.0),
                        ui.visuals().text_color(),
                    );
                    // Labels are clipped to each card, so large translations
                    // cannot overlap their neighbours or expand the workspace.
                    let mut value = value_job(
                        &m.value,
                        if short { 23.0 } else { 36.0 },
                        ui.visuals().strong_text_color(),
                        ui.visuals().weak_text_color(),
                    );
                    value.wrap = egui::text::TextWrapping::truncate_at_width(inner.width());
                    painter.galley(
                        inner.left_top() + egui::vec2(0.0, 18.0),
                        painter.layout_job(value),
                        ui.visuals().strong_text_color(),
                    );
                    if !short {
                        let mut job = egui::text::LayoutJob::simple_singleline(
                            m.detail.clone(),
                            egui::FontId::proportional(11.0),
                            ui.visuals().weak_text_color(),
                        );
                        job.wrap = egui::text::TextWrapping::truncate_at_width(inner.width());
                        painter.galley(
                            inner.left_top() + egui::vec2(0.0, 65.0),
                            painter.layout_job(job),
                            ui.visuals().weak_text_color(),
                        );
                    }
                    // Kept inside the band's rounded ends and outline.
                    let inset = |end: bool| if end { 6.0 } else { 1.0 };
                    let chart = egui::Rect::from_min_max(
                        egui::pos2(
                            rect.left() + inset(index == 0),
                            rect.bottom() - if short { 18.0 } else { 34.0 },
                        ),
                        egui::pos2(
                            rect.right() - inset(index + 1 == chunk.len()),
                            rect.bottom() - 1.0,
                        ),
                    );
                    let max = m.ceiling.unwrap_or_else(|| {
                        m.series
                            .iter()
                            .flat_map(|(v, _)| v.iter().rev().take(60).copied())
                            .filter(|v| v.is_finite())
                            .fold(1024.0, f32::max)
                    });
                    for (values, color) in &m.series {
                        draw_history(&painter, chart, &h.times, values, max, *color);
                    }
                    let response = response
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(format!(
                            "{}\n{}\n{}",
                            m.value,
                            m.detail,
                            crate::i18n::tr("View resource details")
                        ));
                    let mut open = response.clicked();
                    crate::context_actions::menu(
                        &response,
                        ("resource-strip", response.id),
                        |ui| {
                            crate::context_actions::copy(ui, crate::i18n::tr("Current"), || {
                                format!("{}: {} · {}", m.title, m.value, m.detail)
                            });
                            open |= crate::context_actions::action(ui, crate::i18n::tr("Details"));
                        },
                    );
                    if open {
                        clicked = Some(m.resource);
                    }
                    if response.hovered() || response.has_focus() {
                        ui.painter().rect_stroke(
                            rect,
                            radius,
                            egui::Stroke::new(1.5, m.color),
                            egui::StrokeKind::Inside,
                        );
                    }
                });
            }
        });
        let line = ui
            .visuals()
            .widgets
            .noninteractive
            .bg_stroke
            .color
            .gamma_multiply(0.75);
        if let (Some(first), Some(last)) = (cards.first(), cards.last()) {
            ui.painter().rect_stroke(
                first.union(*last),
                egui::CornerRadius::same(8),
                egui::Stroke::new(1.0, line),
                egui::StrokeKind::Inside,
            );
        }
        for card in cards.iter().skip(1) {
            let x = card.left().round() + 0.5;
            ui.painter().line_segment(
                [
                    egui::pos2(x, card.top() + 10.0),
                    egui::pos2(x, card.bottom() - 10.0),
                ],
                egui::Stroke::new(1.0, line),
            );
        }
    }
    ui.add_space(0.0);
    clicked
}

/// Numeric readings lead; smaller units stay on the same baseline.
fn value_job(value: &str, size: f32, ink: Color32, muted: Color32) -> egui::text::LayoutJob {
    let split = value
        .find(|c: char| c.is_whitespace() || c == '%')
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let mut job = egui::text::LayoutJob::default();
    job.append(
        number,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(size),
            color: ink,
            ..Default::default()
        },
    );
    job.append(
        unit,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(size * 0.55),
            color: muted,
            valign: egui::Align::Max,
            ..Default::default()
        },
    );
    job
}

/// A real 60-second viewport. Disconnected/invalid samples break the line.
fn draw_history(
    p: &egui::Painter,
    rect: egui::Rect,
    times: &VecDeque<u64>,
    values: &VecDeque<f32>,
    max: f32,
    color: Color32,
) {
    if times.len() != values.len() || max <= 0.0 {
        return;
    }
    let Some(&end) = times.back() else {
        return;
    };
    let start = end.saturating_sub(60_000);
    let mut previous: Option<(u64, egui::Pos2)> = None;
    for (&at, &v) in times
        .iter()
        .zip(values)
        .skip(times.len().saturating_sub(61))
    {
        if at < start || at > end || !v.is_finite() {
            previous = None;
            continue;
        }
        let point = egui::pos2(
            rect.left() + at.saturating_sub(start) as f32 / 60_000.0 * rect.width(),
            rect.bottom() - (v / max).clamp(0.0, 1.0) * rect.height(),
        );
        if let Some((before, a)) = previous
            && at > before
            && at - before <= 5_000
        {
            p.add(egui::Shape::convex_polygon(
                vec![
                    a,
                    point,
                    egui::pos2(point.x, rect.bottom()),
                    egui::pos2(a.x, rect.bottom()),
                ],
                color.gamma_multiply(0.09),
                egui::Stroke::NONE,
            ));
            p.line_segment([a, point], egui::Stroke::new(1.5, color));
        }
        previous = Some((at, point));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charts_break_at_missing_samples_and_reject_unpaired_history() {
        let color = Color32::from_rgb(31, 113, 197);
        for (times, values, expected_segments) in [
            (
                vec![0, 1_000, 10_000, 11_000],
                vec![10.0, 20.0, 30.0, 40.0],
                2,
            ),
            (vec![0, 1_000, 2_000], vec![10.0, f32::NAN, 30.0], 0),
            (vec![0, 1_000], vec![10.0], 0),
            (vec![0, 1_000, 100_000, 101_000], vec![10.0; 4], 1),
        ] {
            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(Default::default(), |ui| {
                draw_history(
                    ui.painter(),
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 60.0)),
                    &VecDeque::from(times.clone()),
                    &VecDeque::from(values.clone()),
                    100.0,
                    color,
                );
            });
            let segments = output
                .shapes
                .iter()
                .filter(|s| {
                    matches!(s.shape,
                egui::Shape::LineSegment { stroke, .. } if stroke.color == color)
                })
                .count();
            // This geometry-only test has no renderer to consume uploads.
            output.textures_delta.clear();
            assert_eq!(segments, expected_segments, "{times:?}");
        }
    }
}

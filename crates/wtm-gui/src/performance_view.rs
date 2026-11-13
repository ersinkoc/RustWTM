//! Resource navigation and presentation-only filters for Performance.
use std::collections::VecDeque;

use eframe::egui::{self, Ui};

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Page {
    Cpu,
    Memory,
    Gpu,
    Network,
    Disk,
    #[default]
    #[serde(other)]
    All,
}

impl From<crate::overview::Resource> for Page {
    fn from(value: crate::overview::Resource) -> Self {
        use crate::overview::Resource;
        match value {
            Resource::Cpu => Self::Cpu,
            Resource::Memory => Self::Memory,
            Resource::Gpu => Self::Gpu,
            Resource::Network => Self::Network,
            Resource::Disk => Self::Disk,
        }
    }
}

#[derive(Default)]
pub struct State {
    pub page: Page,
    pub class: Option<u8>,
    pub engine_search: String,
}

/// Content at least this wide shows resources in a rail with sparklines
/// instead of a row of tabs.
pub const RAIL_FROM: f32 = 860.0;
const RAIL_WIDTH: f32 = 208.0;
/// Samples drawn in a rail sparkline.
const RAIL_SAMPLES: usize = 60;

type Series = Vec<(VecDeque<f32>, egui::Color32)>;

/// The newest `RAIL_SAMPLES` values of a history series.
fn tail(values: &VecDeque<f32>) -> VecDeque<f32> {
    values
        .iter()
        .skip(values.len().saturating_sub(RAIL_SAMPLES))
        .copied()
        .collect()
}

fn rate(values: &VecDeque<f32>) -> String {
    format!(
        "{}/s",
        wtm_core::format_bytes(f64::from(values.back().copied().unwrap_or(0.0).max(0.0)))
    )
}

fn percent(values: &VecDeque<f32>) -> String {
    values
        .back()
        .map_or_else(|| "—".to_owned(), |v| format!("{v:.0}%"))
}

impl State {
    /// The resources as a vertical rail: each with its latest reading and a
    /// sparkline of the last minute. Returns whether the page changed.
    pub fn rail(&mut self, ui: &mut Ui, history: &crate::backend::History) -> bool {
        use crate::theme::{CPU, DISK_READ, DISK_WRITE, GPU, MEMORY, NET_RX, NET_TX};
        let before = self.page;
        let entries: [(Page, &str, String, Series, bool); 6] = [
            (
                Page::All,
                "All resources",
                format!(
                    "CPU {} · {} {}",
                    percent(&history.cpu),
                    crate::i18n::tr("Memory"),
                    percent(&history.memory)
                ),
                Vec::new(),
                true,
            ),
            (
                Page::Cpu,
                "CPU",
                percent(&history.cpu),
                vec![(tail(&history.cpu), CPU)],
                true,
            ),
            (
                Page::Memory,
                "Memory",
                percent(&history.memory),
                vec![(tail(&history.memory), MEMORY)],
                true,
            ),
            (
                Page::Gpu,
                "GPU",
                percent(&history.gpu),
                vec![(tail(&history.gpu), GPU)],
                true,
            ),
            (
                Page::Network,
                "Network",
                format!("↓ {}  ↑ {}", rate(&history.net_rx), rate(&history.net_tx)),
                vec![
                    (tail(&history.net_rx), NET_RX),
                    (tail(&history.net_tx), NET_TX),
                ],
                false,
            ),
            (
                Page::Disk,
                "Disk & I/O",
                format!(
                    "R {}  W {}",
                    rate(&history.io_read),
                    rate(&history.io_write)
                ),
                vec![
                    (tail(&history.io_read), DISK_READ),
                    (tail(&history.io_write), DISK_WRITE),
                ],
                false,
            ),
        ];
        ui.allocate_ui_with_layout(
            egui::vec2(RAIL_WIDTH, ui.available_height()),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(RAIL_WIDTH);
                ui.spacing_mut().item_spacing.y = 4.0;
                for (page, label, reading, series, percent_scale) in entries {
                    if rail_entry(
                        ui,
                        self.page == page,
                        label,
                        &reading,
                        &series,
                        percent_scale,
                    )
                    .clicked()
                    {
                        self.page = page;
                    }
                }
            },
        );
        self.page != before
    }

    /// Keep navigation outside the content scroll area, including on compact windows.
    pub fn navigation(&mut self, ui: &mut Ui) -> bool {
        let before = self.page;
        ui.horizontal_wrapped(|ui| {
            for (page, label) in [
                (Page::All, "All resources"),
                (Page::Cpu, "CPU"),
                (Page::Memory, "Memory"),
                (Page::Gpu, "GPU"),
                (Page::Network, "Network"),
                (Page::Disk, "Disk & I/O"),
            ] {
                let response =
                    crate::theme::choice(ui, &mut self.page, page, crate::i18n::tr(label));
                if page != Page::All {
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::accesskit::Role::Button,
                            ui.is_enabled(),
                            self.page == page,
                            crate::i18n::format(
                                "{0} performance view",
                                &[crate::i18n::tr(label).into()],
                            ),
                        )
                    });
                }
                if response.clicked() {
                    self.page = page;
                }
            }
        });
        ui.add_space(0.0);
        self.page != before
    }
}

/// One resource in the rail: a sparkline, its name and latest reading. The
/// selected one is raised and marked with the accent, like a chosen chip.
fn rail_entry(
    ui: &mut Ui,
    selected: bool,
    label: &str,
    reading: &str,
    series: &[(VecDeque<f32>, egui::Color32)],
    percent_scale: bool,
) -> egui::Response {
    let dark = ui.visuals().dark_mode;
    let height = 48.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    let name = crate::i18n::tr(label);
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::accesskit::Role::Button,
            ui.is_enabled(),
            selected,
            if label == "All resources" {
                name.to_owned()
            } else {
                crate::i18n::format("{0} performance view", &[name.into()])
            },
        )
    });
    let painter = ui.painter_at(rect);
    let radius = egui::CornerRadius::same(6);
    if selected {
        painter.rect(
            rect,
            radius,
            crate::theme::chip_fill(dark),
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
            egui::StrokeKind::Inside,
        );
    } else if response.hovered() {
        painter.rect_filled(rect, radius, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    if selected || response.has_focus() {
        let mark = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 2.5, rect.center().y),
            egui::vec2(3.0, 18.0),
        );
        painter.rect_filled(mark, egui::CornerRadius::same(2), crate::theme::ACCENT);
    }
    let text_left = if let Some((_, color)) = series.first() {
        let chart = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 10.0, rect.top() + 7.0),
            egui::vec2(60.0, height - 14.0),
        );
        painter.rect_stroke(
            chart,
            egui::CornerRadius::same(3),
            egui::Stroke::new(1.0, color.gamma_multiply(0.55)),
            egui::StrokeKind::Inside,
        );
        let max = if percent_scale {
            100.0
        } else {
            let peak = series
                .iter()
                .flat_map(|(values, _)| values.iter().copied())
                .fold(0.0f32, f32::max);
            crate::theme::rate_ceiling(f64::from(peak)) as f32
        };
        for (values, color) in series {
            crate::theme::sparkline(
                &painter,
                chart.shrink(1.0),
                values,
                max,
                RAIL_SAMPLES,
                *color,
            );
        }
        chart.right() + 10.0
    } else {
        rect.left() + 14.0
    };
    painter.text(
        egui::pos2(text_left, rect.center().y - 1.0),
        egui::Align2::LEFT_BOTTOM,
        name,
        egui::TextStyle::Body.resolve(ui.style()),
        if selected {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().text_color()
        },
    );
    painter.text(
        egui::pos2(text_left, rect.center().y + 2.0),
        egui::Align2::LEFT_TOP,
        reading,
        egui::TextStyle::Small.resolve(ui.style()),
        ui.visuals().weak_text_color(),
    );
    response
}

/// Flat usage arrays have no processor-group identity. Only use topology when
/// each array index maps uniquely to a CPU in one group.
pub fn matched_topology(
    topology: Option<&wtm_core::cpu::Topology>,
    count: usize,
) -> Option<&wtm_core::cpu::Topology> {
    topology.filter(|t| {
        t.cpus.len() == count
            && t.groups().len() == 1
            && (0..count).all(|i| t.cpus.iter().filter(|c| usize::from(c.index) == i).count() == 1)
    })
}

pub fn cpu_filter(
    ui: &mut Ui,
    class: &mut Option<u8>,
    topology: Option<&wtm_core::cpu::Topology>,
    count: usize,
) {
    let Some(t) = matched_topology(topology, count).filter(|t| t.hybrid()) else {
        *class = None;
        ui.weak(crate::i18n::tr(
            "CPU tier filtering requires matching single-group hybrid topology.",
        ));
        return;
    };
    let classes = t.classes();
    if class.is_some_and(|c| !classes.contains(&c)) {
        *class = None;
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .selectable_label(class.is_none(), crate::i18n::tr("All CPU tiers"))
            .clicked()
        {
            *class = None;
        }
        for c in classes.into_iter().rev() {
            let count = t
                .cpus
                .iter()
                .filter(|cpu| cpu.efficiency_class == c)
                .count();
            let label = crate::i18n::format("{0} tier ({1})", &[t.tier(c), count.to_string()]);
            if ui.selectable_label(*class == Some(c), label).clicked() {
                *class = Some(c);
            }
        }
    });
    ui.weak(crate::i18n::tr(
        "The tier filter affects logical-processor tiles; the main chart remains total CPU usage.",
    ));
}

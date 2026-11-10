//! Read-only system context for the process workspace; no native queries.
use crate::backend::Data;
use eframe::egui::{self, RichText, Ui};
use wtm_core::format_bytes;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Mode {
    Always,
    Hidden,
    #[default]
    #[serde(other)]
    Auto,
}

impl Mode {
    pub fn visible(self, width: f32, selected: bool) -> bool {
        match self {
            Self::Always => true,
            Self::Hidden => false,
            Self::Auto => selected || width >= 1180.0,
        }
    }

    pub fn menu(&mut self, ui: &mut Ui) {
        ui.menu_button(crate::i18n::tr("Inspector"), |ui| {
            for (mode, label) in [(Self::Auto, "Adaptive inspector"), (Self::Always, "Always show inspector"), (Self::Hidden, "Hide inspector")] {
                if crate::theme::choice(ui, self, mode, crate::i18n::tr(label)).clicked() {
                    ui.close();
                }
            }
            crate::theme::divider(ui);
            ui.weak(crate::i18n::tr("Adaptive mode shows system facts in wide windows and process details on selection."));
        });
    }
}

fn fact(
    ui: &mut Ui,
    title: &str,
    value: String,
    detail: String,
    color: egui::Color32,
    icon: crate::navigation::Icon,
    percent: Option<f32>,
) {
    let color = crate::theme::text_color(ui, color);
    egui::Frame::new()
        .fill(crate::theme::card_fill(ui))
        .stroke(egui::Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(0.0)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                crate::navigation::badge(ui, icon, color);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong(title);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(value).size(18.0).color(color));
                        });
                    });
                    for (index, line) in detail.lines().enumerate() {
                        let text = RichText::new(line).small();
                        let text = if index == 0 && detail.contains('\n') {
                            text
                        } else {
                            text.weak()
                        };
                        ui.add(egui::Label::new(text).wrap());
                    }
                    if let Some(percent) =
                        percent.filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
                    {
                        ui.add_space(0.0);
                        let (track, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 3.0),
                            egui::Sense::hover(),
                        );
                        ui.painter()
                            .rect_filled(track, 0.0, crate::theme::track_color(ui));
                        let filled = egui::Rect::from_min_size(
                            track.min,
                            egui::vec2(track.width() * percent / 100.0, track.height()),
                        );
                        ui.painter().rect_filled(filled, 0.0, color);
                    }
                });
            });
        });
    ui.add_space(0.0);
}

fn detail(identity: &str, counters: String) -> String {
    if identity.is_empty() {
        counters
    } else if counters.is_empty() {
        identity.into()
    } else {
        format!("{identity}\n{counters}")
    }
}

fn percent(value: f32, available: bool, digits: usize) -> String {
    if available && value.is_finite() && (0.0..=100.0).contains(&value) {
        format!("{value:.digits$}%")
    } else {
        "—".into()
    }
}

fn rate(value: f64) -> String {
    if value.is_finite() && value >= 0.0 {
        format!("{}/s", format_bytes(value))
    } else {
        "—".into()
    }
}

pub fn show(ui: &mut Ui, data: &Data) {
    use crate::navigation::Icon;
    ui.horizontal(|ui| {
        crate::navigation::badge(
            ui,
            Icon::Overview,
            crate::theme::text_color(ui, crate::theme::ACCENT),
        );
        ui.vertical(|ui| {
            ui.heading(RichText::new(crate::i18n::tr("System overview")).size(20.0));
            ui.label(
                RichText::new(crate::i18n::tr("Last received system facts"))
                    .small()
                    .weak(),
            );
        });
    });
    ui.add_space(0.0);
    let summary = crate::inventory::clipboard_summary(ui);
    let ready = matches!(summary, crate::inventory::ClipboardSummary::Ready(_));
    if ui
        .add_enabled(
            ready,
            egui::Button::new(crate::i18n::tr("Copy system summary")).small(),
        )
        .clicked()
        && let crate::inventory::ClipboardSummary::Ready(text) = &summary
    {
        ui.ctx().copy_text(text.clone());
    }
    match summary {
        crate::inventory::ClipboardSummary::Loading => {
            ui.weak(crate::i18n::tr("Reading hardware details…"));
        }
        crate::inventory::ClipboardSummary::Unavailable => {
            ui.weak(crate::i18n::tr("Hardware details unavailable"));
        }
        crate::inventory::ClipboardSummary::Ready(_) => {}
    }
    ui.add_space(0.0);
    let Some(snapshot) = &data.snapshot else {
        ui.weak(crate::i18n::tr("Waiting for monitoring data…"));
        return;
    };
    let s = &snapshot.system;
    let identities = crate::inventory::resource_identities(ui);
    let cpu_known = s.logical_cpus > 0;
    fact(
        ui,
        "CPU",
        percent(s.cpu_percent, cpu_known, 0),
        detail(
            &identities.cpu,
            if cpu_known {
                crate::i18n::format("{0} logical processors", &[s.logical_cpus.to_string()])
            } else {
                crate::i18n::tr("CPU counters are unavailable.").into()
            },
        ),
        crate::theme::CPU,
        Icon::Cpu,
        cpu_known.then_some(s.cpu_percent),
    );
    let memory_known = s.memory_total > 0 && s.memory_available <= s.memory_total;
    fact(
        ui,
        crate::i18n::tr("Memory"),
        if memory_known {
            format_bytes(s.memory_used() as f64)
        } else {
            "—".into()
        },
        detail(
            &identities.memory,
            if memory_known {
                crate::i18n::format(
                    "{0} available of {1}",
                    &[
                        format_bytes(s.memory_available as f64),
                        format_bytes(s.memory_total as f64),
                    ],
                )
            } else {
                crate::i18n::tr("Memory counters are unavailable.").into()
            },
        ),
        crate::theme::MEMORY,
        Icon::Memory,
        memory_known.then(|| s.memory_used() as f32 * 100.0 / s.memory_total as f32),
    );
    if s.disks.is_empty() {
        fact(
            ui,
            crate::i18n::tr("Physical disks"),
            "—".into(),
            crate::i18n::tr("Disk counters are unavailable.").into(),
            crate::theme::DISK_READ,
            Icon::Disk,
            None,
        );
    }
    for disk in &s.disks {
        fact(
            ui,
            &crate::i18n::format("Disk {0}", std::slice::from_ref(&disk.name)),
            percent(disk.active_percent, true, 0),
            detail(
                disk.name
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse::<u32>().ok())
                    .and_then(|index| identities.disks.get(&index))
                    .map(String::as_str)
                    .unwrap_or_default(),
                format!(
                    "{} {} · {} {}",
                    crate::i18n::tr("Read"),
                    rate(disk.read_rate),
                    crate::i18n::tr("Write"),
                    rate(disk.write_rate)
                ),
            ),
            crate::theme::DISK_READ,
            Icon::Disk,
            Some(disk.active_percent),
        );
    }
    let gpu_known = !s.gpu_engines.is_empty();
    fact(
        ui,
        "GPU",
        percent(s.gpu_percent, gpu_known, 0),
        detail(
            &identities.gpu,
            crate::i18n::tr(if gpu_known {
                "Busiest engine across all GPUs"
            } else {
                "GPU engine counters are unavailable."
            })
            .into(),
        ),
        crate::theme::GPU,
        Icon::Gpu,
        gpu_known.then_some(s.gpu_percent),
    );
    fact(
        ui,
        crate::i18n::tr("Network"),
        rate(s.net_rx_rate + s.net_tx_rate),
        format!("↓ {} · ↑ {}", rate(s.net_rx_rate), rate(s.net_tx_rate)),
        crate::theme::NET_RX,
        Icon::Network,
        None,
    );
    ui.add_space(0.0);
    ui.columns(3, |cols| {
        for (ui, (label, value)) in cols.iter_mut().zip([
            (crate::i18n::tr("Processes"), s.process_count as u64),
            (crate::i18n::tr("Threads"), s.thread_count),
            (crate::i18n::tr("Handles"), s.handle_count),
        ]) {
            ui.label(RichText::new(value.to_string()).size(18.0));
            ui.label(RichText::new(label).small().weak());
        }
    });
    if !identities.motherboard.is_empty() {
        fact(
            ui,
            crate::i18n::tr("Motherboard"),
            String::new(),
            identities.motherboard,
            crate::theme::ACCENT,
            Icon::Overview,
            None,
        );
    }
    ui.collapsing(crate::i18n::tr("How inspection works"), |ui| {
        ui.label(crate::i18n::tr("Select a process to inspect its identity, activity and actions. Clear the selection to return here."));
    });
}

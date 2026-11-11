//! Persistent observed system moments, contributors and nearby engine events.
use eframe::egui::{self, RichText, Ui};
use wtm_core::control::ProcessRef;
use wtm_core::incidents::Replay;
#[derive(Default)]
pub struct View {
    task: crate::background::Task<Replay>,
    frames: Option<Result<Replay, String>>,
    hours: u32,
    selected: usize,
    metric: u8,
    export: Option<String>,
}
impl View {
    pub fn show(&mut self, ui: &mut Ui, data: &crate::backend::Data) -> Option<ProcessRef> {
        if self.hours == 0 {
            self.hours = 24;
        }
        if let Some(result) = self.task.poll() {
            self.selected = result
                .as_ref()
                .map_or(0, |v| v.frames.len().saturating_sub(1));
            self.frames = Some(result);
        }
        ui.horizontal_wrapped(|ui| {
            for (hours, label) in [(1, "1 h"), (24, "24 h"), (168, "7 days")] {
                if crate::theme::choice(ui, &mut self.hours, hours, crate::i18n::tr(label))
                    .clicked()
                {
                    self.frames = None;
                }
            }
            if ui
                .add_enabled(
                    !self.task.busy(),
                    egui::Button::new(crate::i18n::tr("Refresh recorded history")),
                )
                .clicked()
            {
                self.frames = None;
            }
            if self.task.busy() {
                ui.spinner();
            }
        });
        if self.frames.is_none() && !self.task.busy() {
            let hours = self.hours;
            self.task.start(ui.ctx(), move || {
                let mut client =
                    wtm_ipc::Client::connect_to(wtm_ipc::PIPE_NAME).map_err(|e| e.to_string())?;
                match client
                    .call(&wtm_ipc::Request::Incidents { hours })
                    .map_err(|e| e.to_string())?
                {
                    wtm_ipc::Response::Incidents(frames) => Ok(frames),
                    wtm_ipc::Response::Error(e) => Err(e),
                    _ => Err("Incident history requires an updated RustWTM service".into()),
                }
            });
        }
        ui.weak(crate::i18n::tr("Recorded by the service every five seconds. At most 21 contributors per moment; missing intervals are not activity."));
        let mut reveal = None;
        if let Some(result) = &self.frames {
            match result {
                Err(e) => {
                    crate::theme::colored(ui, crate::theme::WARN, e);
                }
                Ok(replay) if replay.frames.is_empty() => {
                    ui.weak(crate::i18n::tr("No recorded moments in this range."));
                }
                Ok(replay) => {
                    let frames = &replay.frames;
                    if replay.actions_truncated {
                        ui.weak(crate::i18n::tr(
                            "Only the latest 512 action markers in this range are included.",
                        ));
                    }
                    let start = frames[0].at;
                    ui.horizontal_wrapped(|ui| {
                        for (metric, label) in [
                            (0, "CPU"),
                            (1, "Memory"),
                            (2, "GPU"),
                            (3, "Disk I/O"),
                            (4, "Network"),
                        ] {
                            crate::theme::choice(
                                ui,
                                &mut self.metric,
                                metric,
                                crate::i18n::tr(label),
                            );
                        }
                    });
                    let metric = self.metric;
                    let color = match metric {
                        1 => crate::theme::MEMORY,
                        2 => crate::theme::GPU,
                        3 => crate::theme::DISK_READ,
                        4 => crate::theme::NET_RX,
                        _ => crate::theme::CPU,
                    };
                    let percent = metric == 0 || metric == 2;
                    let mut segments = Vec::new();
                    let mut points = Vec::new();
                    let expected = u64::from(self.hours) * 3_600_000 / 600;
                    let mut last = None;
                    for f in frames {
                        if last.is_some_and(|at: u64| {
                            f.at <= at || f.at.saturating_sub(at) > expected.max(5000) * 3
                        }) && !points.is_empty()
                        {
                            segments.push(std::mem::take(&mut points));
                        }
                        let value = match metric {
                            1 => f.memory as f64,
                            2 => f64::from(f.gpu),
                            3 => f.io,
                            4 => f.network,
                            _ => f64::from(f.cpu),
                        };
                        points.push([f.at.saturating_sub(start) as f64 / 1000.0, value]);
                        last = Some(f.at);
                    }
                    if !points.is_empty() {
                        segments.push(points);
                    }
                    let plot = egui_plot::Plot::new("recorded-incident-history")
                        .height(220.0)
                        .show_background(false)
                        .grid_color(ui.visuals().weak_text_color().gamma_multiply(0.20))
                        .allow_drag(false)
                        .allow_scroll(false)
                        .allow_zoom(false)
                        .allow_boxed_zoom(false)
                        .y_axis_min_width(64.0)
                        .y_axis_formatter(|mark, _| {
                            if percent {
                                format!("{:.0}%", mark.value)
                            } else if metric == 1 {
                                wtm_core::format_bytes(mark.value.max(0.0))
                            } else {
                                format!("{}/s", wtm_core::format_bytes(mark.value.max(0.0)))
                            }
                        })
                        .include_y(0.0)
                        .include_y(if percent { 100.0 } else { 0.0 })
                        .show(ui, |plot| {
                            for (n, points) in segments.into_iter().enumerate() {
                                plot.line(
                                    egui_plot::Line::new(format!("{n}"), points)
                                        .color(color)
                                        .width(1.6)
                                        .fill(0.0),
                                );
                            }
                            if let Some(f) = frames.get(self.selected) {
                                plot.vline(
                                    egui_plot::VLine::new(
                                        "Selected moment",
                                        f.at.saturating_sub(start) as f64 / 1000.0,
                                    )
                                    .color(crate::theme::ACCENT),
                                );
                            }
                            plot.pointer_coordinate()
                        });
                    if plot.response.clicked()
                        && let Some(point) = plot.inner
                    {
                        let at = start.saturating_add((point.x.max(0.0) * 1000.0) as u64);
                        self.selected = frames
                            .iter()
                            .enumerate()
                            .min_by_key(|(_, f)| f.at.abs_diff(at))
                            .map_or(0, |(i, _)| i);
                    }
                    ui.add(
                        egui::Slider::new(&mut self.selected, 0..=frames.len().saturating_sub(1))
                            .text(crate::i18n::tr("Recorded moment")),
                    );
                    if let Some(f) = frames.get(self.selected) {
                        crate::theme::card(ui, crate::i18n::tr("Observed activity"), |ui| {
                            let stamp = chrono::DateTime::from_timestamp_millis(f.at as i64)
                                .map_or_else(
                                    || f.at.to_string(),
                                    |t| {
                                        t.with_timezone(&chrono::Local)
                                            .format("%Y-%m-%d %H:%M:%S")
                                            .to_string()
                                    },
                                );
                            ui.strong(stamp);
                            ui.label(format!(
                                "CPU {:.0}% · RAM {} / {} · GPU {:.0}%",
                                f.cpu,
                                wtm_core::format_bytes(f.memory as f64),
                                wtm_core::format_bytes(f.memory_total as f64),
                                f.gpu
                            ));
                            if f.partial {
                                crate::theme::colored(
                                    ui,
                                    crate::theme::WARN,
                                    crate::i18n::tr("Contributor sampling was limited."),
                                );
                            }
                            for p in &f.processes {
                                ui.horizontal_wrapped(|ui| {
                                    let name = ui
                                        .label(RichText::new(&p.target.name).strong())
                                        .interact(egui::Sense::click());
                                    ui.weak(format!(
                                        "PID {} · CPU {:.1}% · {}",
                                        p.target.pid,
                                        p.cpu,
                                        wtm_core::format_bytes(p.memory as f64)
                                    ));
                                    let live = data.snapshot.as_ref().is_some_and(|s| {
                                        s.processes.iter().any(|r| {
                                            r.pid == p.target.pid
                                                && r.create_time == p.target.create_time
                                        })
                                    });
                                    crate::context_actions::menu(
                                        &name,
                                        (
                                            "incident-process",
                                            f.at,
                                            p.target.pid,
                                            p.target.create_time,
                                        ),
                                        |ui| {
                                            crate::context_actions::copy(ui, "PID", || {
                                                p.target.pid.to_string()
                                            });
                                            crate::context_actions::copy(
                                                ui,
                                                crate::i18n::tr("Name"),
                                                || p.target.name.clone(),
                                            );
                                            if live
                                                && crate::context_actions::action(
                                                    ui,
                                                    crate::i18n::tr("Inspect process"),
                                                )
                                            {
                                                reveal = Some(p.target.clone());
                                            }
                                        },
                                    );
                                    if ui
                                        .add_enabled(
                                            live,
                                            egui::Button::new(crate::i18n::tr("Inspect process")),
                                        )
                                        .clicked()
                                    {
                                        reveal = Some(p.target.clone());
                                    }
                                });
                            }
                            ui.separator();
                            ui.strong(crate::i18n::tr("Nearby recorded actions"));
                            let nearby: Vec<_> = replay
                                .actions
                                .iter()
                                .filter(|e| e.at.abs_diff(f.at) <= 30_000)
                                .take(32)
                                .collect();
                            if nearby.is_empty() {
                                ui.weak(crate::i18n::tr(
                                    "No matching recorded actions are available for this moment.",
                                ));
                            }
                            for e in &nearby {
                                ui.label(crate::i18n::message(&e.message).as_ref());
                            }
                            if ui
                                .button(crate::i18n::tr("Review diagnostic snapshot"))
                                .clicked()
                            {
                                self.export=serde_json::to_string_pretty(&serde_json::json!({"moment":f,"available_actions":nearby,"scope":"Observed readings, not a causal diagnosis"})).ok();
                            }
                        });
                    }
                }
            }
        }
        if let Some(text) = self.export.clone() {
            let mut close = false;
            let modal=egui::Modal::new(egui::Id::unique("incident-export")).show(ui.ctx(),|ui|{
   ui.set_max_width(680.0);ui.heading(crate::i18n::tr("Diagnostic snapshot"));ui.weak(crate::i18n::tr("Review before sharing: process names, IDs, resource usage and available event messages are included."));
   egui::ScrollArea::vertical().max_height(320.0).show(ui,|ui|{ui.monospace(&text);});
   ui.horizontal(|ui|{if ui.button(crate::i18n::tr("Copy snapshot")).clicked(){ui.ctx().copy_text(text.clone());}close=ui.button(crate::i18n::tr("Close")).clicked();});
  });
            if close || modal.should_close() {
                self.export = None;
            }
        }
        reveal
    }
}

#[cfg(test)]
impl View {
    pub fn fixture() -> Self {
        let frames = (0..60)
            .map(|i| wtm_core::incidents::Frame {
                at: 1_700_000_000_000 + i * 5000,
                cpu: (45.0 + 30.0 * (i as f32 / 9.0).sin()),
                memory: 12 << 30,
                memory_total: 32 << 30,
                gpu: 12.0,
                network: 1000.0,
                io: 12000.0,
                processes: vec![wtm_core::incidents::Process {
                    target: ProcessRef {
                        pid: 100,
                        create_time: 1000,
                        name: "compiler.exe".into(),
                    },
                    cpu: 32.5,
                    memory: 2 << 30,
                    gpu: 0.0,
                    io: 12000.0,
                    network: 1000.0,
                }],
                partial: false,
                process_network_available: true,
            })
            .collect();
        Self {
            frames: Some(Ok(Replay {
                frames,
                actions: vec![wtm_core::incidents::RecordedAction {
                    at: 1_700_000_295_000,
                    pid: Some(100),
                    name: Some("compiler.exe".into()),
                    level: "Info".into(),
                    message: "Governor applied efficiency mode; system load recovered afterwards"
                        .into(),
                }],
                actions_truncated: false,
            })),
            hours: 1,
            selected: 59,
            ..Default::default()
        }
    }
}

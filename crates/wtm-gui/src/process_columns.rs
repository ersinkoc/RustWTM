//! Process column profiles and a transactional, keyboard-operable layout editor.

use crate::processes::{COLUMNS, Col};
use eframe::egui;
use std::collections::HashSet;

pub fn default_order() -> Vec<Col> {
    COLUMNS.iter().map(|c| c.0).collect()
}

/// Keep known IDs once, anchor Name, then append any newly introduced columns.
pub fn normalize_order(order: &mut Vec<Col>) {
    let mut seen = HashSet::from([Col::Name]);
    order.retain(|c| *c != Col::Unknown && seen.insert(*c));
    order.insert(0, Col::Name);
    for (col, ..) in COLUMNS {
        if seen.insert(col) {
            order.push(col);
        }
    }
}

#[derive(Clone)]
pub struct Selection {
    pub visible: HashSet<Col>,
    pub order: Vec<Col>,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            visible: COLUMNS.iter().filter(|c| c.4).map(|c| c.0).collect(),
            order: default_order(),
        }
    }
}

#[derive(Clone, Copy)]
pub enum Profile {
    Basic,
    Cpu,
    Memory,
    Io,
    Network,
    Diagnostics,
}

impl Profile {
    pub const ALL: [Self; 6] = [
        Self::Basic,
        Self::Cpu,
        Self::Memory,
        Self::Io,
        Self::Network,
        Self::Diagnostics,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Basic => crate::i18n::tr("Basic profile"),
            Self::Cpu => crate::i18n::tr("CPU profile"),
            Self::Memory => crate::i18n::tr("Memory profile"),
            Self::Io => crate::i18n::tr("I/O profile"),
            Self::Network => crate::i18n::tr("Network profile"),
            Self::Diagnostics => crate::i18n::tr("Diagnostics profile"),
        }
    }

    pub fn selection(self) -> Selection {
        use Col::*;
        let cols: &[Col] = match self {
            Self::Basic => &[Name, Pid, Cpu, Memory, Disk, Network, Gpu],
            Self::Cpu => &[Name, Pid, Cpu, CpuTime, Cycle, Threads, Priority, Limits],
            Self::Memory => &[
                Name,
                Pid,
                Memory,
                Commit,
                WorkingSet,
                SharedWorkingSet,
                PeakWorkingSet,
                PageFaultRate,
                PagedPool,
                NonPagedPool,
                Handles,
            ],
            Self::Io => &[
                Name,
                Pid,
                Disk,
                DiskRead,
                DiskWrite,
                IoReadBytes,
                IoWriteBytes,
                IoOtherBytes,
                Handles,
                CommandLine,
            ],
            Self::Network => &[Name, Pid, Network, CommandLine],
            Self::Diagnostics => &[
                Name,
                Pid,
                Status,
                User,
                Elevated,
                Architecture,
                Description,
                Publisher,
                FileVersion,
                Threads,
                Handles,
                Session,
                Started,
                CommandLine,
            ],
        };
        let mut order = cols.to_vec();
        normalize_order(&mut order);
        Selection {
            visible: cols.iter().copied().collect(),
            order,
        }
    }

    pub fn sort(self) -> Col {
        match self {
            Self::Basic | Self::Cpu => Col::Cpu,
            Self::Memory => Col::Memory,
            Self::Io => Col::Disk,
            Self::Network => Col::Network,
            Self::Diagnostics => Col::Name,
        }
    }
}

pub struct Editor {
    draft: Selection,
    search: String,
}

impl Editor {
    pub fn new(visible: &HashSet<Col>, order: &[Col]) -> Self {
        let mut draft = Selection {
            visible: visible.clone(),
            order: order.to_vec(),
        };
        draft.visible.remove(&Col::Unknown);
        draft.visible.insert(Col::Name);
        normalize_order(&mut draft.order);
        Self {
            draft,
            search: String::new(),
        }
    }

    fn move_visible(&mut self, col: Col, up: bool) {
        let visible: Vec<_> = self
            .draft
            .order
            .iter()
            .copied()
            .filter(|c| self.draft.visible.contains(c))
            .collect();
        let Some(i) = visible.iter().position(|c| *c == col) else {
            return;
        };
        let other = if up {
            i.checked_sub(1)
        } else {
            (i + 1 < visible.len()).then_some(i + 1)
        };
        let Some(other) = other.filter(|&other| other != 0 && i != 0) else {
            return;
        };
        let a = self.draft.order.iter().position(|c| *c == col).unwrap();
        let b = self
            .draft
            .order
            .iter()
            .position(|c| *c == visible[other])
            .unwrap();
        self.draft.order.swap(a, b);
    }
}

/// Changes remain in the draft until Apply. Escape/outside-click cancels.
pub fn show_editor(ctx: &egui::Context, editor: &mut Option<Editor>) -> Option<Selection> {
    let active = editor.as_mut()?;
    let mut apply = false;
    let mut cancel = false;
    let width = (ctx.content_rect().width() - 72.0).clamp(320.0, 720.0);
    let content_height = (ctx.content_rect().height() - 56.0).clamp(340.0, 640.0);
    let height = (content_height - 160.0).clamp(110.0, 480.0);
    let frame = egui::Frame::popup(&ctx.global_style());
    let id = egui::Id::unique("process-column-editor");
    let area = egui::Modal::default_area(id)
        .default_size(egui::vec2(width, content_height) + frame.total_margin().sum());
    let modal = egui::Modal::new(id)
        .area(area)
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.set_height(content_height);
            ui.spacing_mut().scroll = crate::theme::solid_scroll();
            ui.heading(crate::i18n::tr("Arrange columns…"));
            ui.weak(crate::i18n::tr(
                "Column order is shared by List, Tree and Apps.",
            ));
            ui.add_space(0.0);
            ui.columns(2, |cols| {
                cols[0].strong(crate::i18n::tr("Available columns"));
                let search = cols[0].add(
                    egui::TextEdit::singleline(&mut active.search)
                        .hint_text(crate::i18n::tr("Find a column")),
                );
                search.widget_info(|| {
                    let mut info = egui::WidgetInfo::text_edit(
                        search.enabled(),
                        &active.search,
                        &active.search,
                        crate::i18n::tr("Find a column"),
                    );
                    info.label = Some(crate::i18n::tr("Find a column").into());
                    info
                });
                let query = active.search.trim().to_lowercase();
                egui::ScrollArea::vertical()
                    .id_salt("available-columns")
                    .max_height(height)
                    .auto_shrink([false, false])
                    .show(&mut cols[0], |ui| {
                        for (col, label, ..) in COLUMNS {
                            let label = crate::i18n::tr(label);
                            if !query.is_empty() && !label.to_lowercase().contains(&query) {
                                continue;
                            }
                            let mut on = active.draft.visible.contains(&col);
                            if ui
                                .add_enabled(col != Col::Name, egui::Checkbox::new(&mut on, label))
                                .changed()
                            {
                                if on {
                                    active.draft.visible.insert(col);
                                } else {
                                    active.draft.visible.remove(&col);
                                }
                            }
                        }
                    });
                cols[1].strong(crate::i18n::tr("Visible order"));
                cols[1].weak(crate::i18n::tr("Name stays first."));
                let visible: Vec<_> = active
                    .draft
                    .order
                    .iter()
                    .copied()
                    .filter(|c| active.draft.visible.contains(c))
                    .collect();
                let mut movement = None;
                egui::ScrollArea::vertical()
                    .id_salt("ordered-columns")
                    .max_height(height)
                    .auto_shrink([false, false])
                    .show(&mut cols[1], |ui| {
                        for (i, col) in visible.iter().enumerate() {
                            let label =
                                crate::i18n::tr(COLUMNS.iter().find(|c| c.0 == *col).unwrap().1);
                            ui.push_id(col, |ui| {
                                ui.horizontal(|ui| {
                                    let name_width = (ui.available_width() - 68.0).max(40.0);
                                    ui.add_sized(
                                        [name_width, ui.spacing().interact_size.y],
                                        egui::Label::new(label).truncate(),
                                    );
                                    let up = crate::i18n::format("Move {0} up", &[label.into()]);
                                    let down =
                                        crate::i18n::format("Move {0} down", &[label.into()]);
                                    let response = ui
                                        .add_enabled(i > 1, egui::Button::new("↑"))
                                        .on_hover_text(&up);
                                    response.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::Button,
                                            response.enabled(),
                                            &up,
                                        )
                                    });
                                    if response.clicked() {
                                        movement = Some((*col, true));
                                    }
                                    // WidgetInfo supplies meaningful names to keyboard/assistive tests.
                                    let response = ui
                                        .add_enabled(
                                            i > 0 && i + 1 < visible.len(),
                                            egui::Button::new("↓"),
                                        )
                                        .on_hover_text(&down);
                                    response.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::Button,
                                            response.enabled(),
                                            &down,
                                        )
                                    });
                                    if response.clicked() {
                                        movement = Some((*col, false));
                                    }
                                });
                            });
                        }
                    });
                if let Some((col, up)) = movement {
                    active.move_visible(col, up);
                }
            });
            crate::theme::divider(ui);
            ui.horizontal(|ui| {
                if ui
                    .button(crate::i18n::tr("Restore default columns"))
                    .clicked()
                {
                    active.draft = Selection::default();
                }
                apply = ui.button(crate::i18n::tr("Apply")).clicked();
                cancel = ui.button(crate::i18n::tr("Cancel")).clicked();
            });
        });
    if apply {
        return editor.take().map(|e| e.draft);
    }
    if cancel || modal.should_close() {
        *editor = None;
    }
    None
}

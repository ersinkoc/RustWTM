//! Disks tab: every drive with its free space, what the disks are doing
//! right now and who is using them, and a space analysis of any drive or
//! folder (folder tree, largest files, file types).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, Layout, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use wtm_core::diskscan::{self, DirNode, Progress, ScanResult};
use wtm_core::{DriveKind, Volume, format_bytes};

use crate::backend::Data;
use crate::theme::{ACCENT, BAD, GOOD, WARN, card};

/// How often the drive list is read again.
const VOLUMES_EVERY: Duration = Duration::from_secs(10);
/// Subfolders listed per folder; the rest are summed in one row.
const CHILDREN_SHOWN: usize = 300;
/// Processes listed under "Who is using the disks".
const TOP_PROCESSES: usize = 8;
const TABLE_HEIGHT: f32 = 460.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ResultView {
    #[default]
    Folders,
    Largest,
    Types,
}

struct Scan {
    path: PathBuf,
    progress: Arc<Progress>,
    result: Arc<Mutex<Option<ScanResult>>>,
    started: Instant,
}

impl Scan {
    fn result(&self) -> Option<std::sync::MutexGuard<'_, Option<ScanResult>>> {
        let guard = self.result.lock().unwrap_or_else(PoisonError::into_inner);
        guard.is_some().then_some(guard)
    }
}

pub struct DisksView {
    volumes: Arc<Mutex<Option<Vec<Volume>>>>,
    loading: Arc<AtomicBool>,
    volumes_at: Option<Instant>,
    /// Off in tests, which supply the drives themselves.
    auto_refresh: bool,
    /// Folder to analyze.
    pub path: String,
    scan: Option<Scan>,
    pub view: ResultView,
    /// Expanded folders, as child-index paths from the root.
    expanded: HashSet<Vec<usize>>,
    error: Option<String>,
}

impl Default for DisksView {
    fn default() -> Self {
        Self {
            volumes: Arc::default(),
            loading: Arc::default(),
            volumes_at: None,
            auto_refresh: true,
            path: std::env::var("SystemDrive").map_or_else(|_| r"C:\".into(), |d| format!(r"{d}\")),
            scan: None,
            view: ResultView::Folders,
            expanded: HashSet::new(),
            error: None,
        }
    }
}

impl Drop for DisksView {
    fn drop(&mut self) {
        if let Some(scan) = &self.scan {
            scan.progress.cancel();
        }
    }
}

impl DisksView {
    #[cfg(test)]
    pub fn with_volumes(volumes: Vec<Volume>) -> Self {
        let mut view = Self::default();
        view.volumes = Arc::new(Mutex::new(Some(volumes)));
        view.auto_refresh = false;
        view
    }

    /// Shows a finished analysis, as if a scan had produced it.
    #[cfg(test)]
    pub fn with_result(mut self, result: ScanResult) -> Self {
        self.scan = Some(Scan {
            path: result.root_path.clone(),
            progress: Arc::default(),
            result: Arc::new(Mutex::new(Some(result))),
            started: Instant::now(),
        });
        self
    }

    pub fn scanning(&self) -> bool {
        self.scan.as_ref().is_some_and(|s| s.result().is_none())
    }

    /// Reads the drive list off the UI thread: a sleeping network drive can
    /// take seconds to answer.
    fn refresh_volumes(&mut self, ctx: &egui::Context) {
        if !self.auto_refresh
            || self.loading.load(Ordering::Relaxed)
            || self.volumes_at.is_some_and(|t| t.elapsed() < VOLUMES_EVERY)
        {
            return;
        }
        self.volumes_at = Some(Instant::now());
        self.loading.store(true, Ordering::Relaxed);
        let (volumes, loading, ctx) = (self.volumes.clone(), self.loading.clone(), ctx.clone());
        std::thread::spawn(move || {
            let list = wtm_core::volumes();
            *volumes.lock().unwrap_or_else(PoisonError::into_inner) = Some(list);
            loading.store(false, Ordering::Relaxed);
            ctx.request_repaint();
        });
    }

    /// Starts analyzing `path` in the background, replacing any earlier scan.
    pub fn start(&mut self, ctx: &egui::Context, path: &Path) {
        if !path.is_dir() {
            self.error = Some(crate::i18n::format(
                "{0} is not a folder that can be read",
                &[path.display().to_string()],
            ));
            return;
        }
        if let Some(old) = self.scan.take() {
            old.progress.cancel();
        }
        self.error = None;
        self.expanded.clear();
        let progress = Arc::new(Progress::default());
        let result = Arc::new(Mutex::new(None));
        let (p, r, root, ctx) = (
            progress.clone(),
            result.clone(),
            path.to_owned(),
            ctx.clone(),
        );
        // Deep folder trees recurse; background mode keeps the scan from
        // slowing down the machine or this window.
        let spawned = std::thread::Builder::new()
            .name("disk-scan".into())
            .stack_size(64 << 20)
            .spawn(move || {
                let _ = wtm_core::control::enter_background_mode();
                let scanned = diskscan::scan(&root, &p, diskscan::LARGEST_KEPT);
                *r.lock().unwrap_or_else(PoisonError::into_inner) = Some(scanned);
                ctx.request_repaint();
            });
        if let Err(e) = spawned {
            self.error = Some(e.to_string());
            return;
        }
        self.scan = Some(Scan {
            path: path.to_owned(),
            progress,
            result,
            started: Instant::now(),
        });
    }

    pub fn cancel(&mut self) {
        if let Some(scan) = &self.scan {
            scan.progress.cancel();
        }
    }
}

pub fn disks_tab(ui: &mut Ui, data: &Data, view: &mut DisksView) {
    view.refresh_volumes(ui.ctx());
    drives(ui, view);
    ui.add_space(6.0);
    activity(ui, data);
    ui.add_space(6.0);
    file_activity(ui, data);
    ui.add_space(6.0);
    card(
        ui,
        RichText::new(crate::i18n::tr("What takes up the space"))
            .size(16.0)
            .strong(),
        |ui| analysis(ui, view),
    );
}

fn used_color(percent: f32) -> Color32 {
    if percent >= 90.0 {
        BAD
    } else if percent >= 75.0 {
        WARN
    } else {
        GOOD
    }
}

fn kind_label(kind: DriveKind) -> &'static str {
    match kind {
        DriveKind::Fixed => crate::i18n::tr("Local disk"),
        DriveKind::Removable => crate::i18n::tr("Removable"),
        DriveKind::Network => crate::i18n::tr("Network drive"),
        DriveKind::Optical => crate::i18n::tr("Optical"),
        DriveKind::RamDisk => crate::i18n::tr("RAM disk"),
        DriveKind::Unknown => crate::i18n::tr("Drive"),
    }
}

/// A bar `fraction` full, `width` wide.
fn bar(ui: &mut Ui, fraction: f32, width: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 10.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    let mut fill = rect;
    fill.set_width(rect.width() * fraction.clamp(0.0, 1.0));
    painter.rect_filled(fill, 3.0, color);
}

fn drives(ui: &mut Ui, view: &mut DisksView) {
    let list = view
        .volumes
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    card(
        ui,
        RichText::new(crate::i18n::tr("Drives")).size(16.0).strong(),
        |ui| {
            let Some(list) = list else {
                ui.spinner();
                return;
            };
            if list.is_empty() {
                ui.weak(crate::i18n::tr("No drives could be read."));
                return;
            }
            let mut analyze = None;
            ui.horizontal_wrapped(|ui| {
                for v in &list {
                    egui::Frame::new()
                        .fill(ui.visuals().faint_bg_color)
                        .corner_radius(8)
                        .inner_margin(10)
                        .show(ui, |ui| {
                            ui.set_width(270.0);
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    let name = if v.label.is_empty() {
                                        v.root.trim_end_matches('\\').to_owned()
                                    } else {
                                        format!("{} ({})", v.label, v.root.trim_end_matches('\\'))
                                    };
                                    ui.label(RichText::new(name).strong());
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.weak(format!(
                                            "{} · {}",
                                            kind_label(v.kind),
                                            v.file_system
                                        ));
                                    });
                                });
                                let percent = v.used_percent();
                                ui.horizontal(|ui| {
                                    bar(ui, percent / 100.0, 226.0, used_color(percent));
                                    ui.label(
                                        RichText::new(format!("{percent:.0}%")).color(
                                            crate::theme::text_color(ui, used_color(percent)),
                                        ),
                                    );
                                });
                                ui.horizontal(|ui| {
                                    ui.label(crate::i18n::format(
                                        "{0} free of {1}",
                                        &[
                                            format_bytes(v.free as f64),
                                            format_bytes(v.total as f64),
                                        ],
                                    ));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui
                                            .small_button(crate::i18n::tr("Analyze"))
                                            .on_hover_text(crate::i18n::tr(
                                                "Find what takes up the space on this drive",
                                            ))
                                            .clicked()
                                        {
                                            analyze = Some(v.root.clone());
                                        }
                                    });
                                });
                            });
                        });
                }
            });
            if let Some(root) = analyze {
                view.path = root.clone();
                view.start(ui.ctx(), Path::new(&root));
            }
        },
    );
}

fn activity(ui: &mut Ui, data: &Data) {
    card(
        ui,
        RichText::new(crate::i18n::tr("Disk activity right now"))
            .size(16.0)
            .strong(),
        |ui| {
            let Some(snap) = &data.snapshot else {
                ui.spinner();
                return;
            };
            two_up(
                ui,
                |ui| {
                    ui.strong(crate::i18n::tr("Physical disks"));
                    if snap.system.disks.is_empty() {
                        ui.weak(crate::i18n::tr("Disk counters are not available."));
                    }
                    egui::Grid::new("disk_activity")
                        .num_columns(3)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            for d in &snap.system.disks {
                                ui.label(crate::i18n::format(
                                    "Disk {0}",
                                    std::slice::from_ref(&d.name),
                                ));
                                ui.horizontal(|ui| {
                                    bar(
                                        ui,
                                        d.active_percent / 100.0,
                                        120.0,
                                        crate::theme::load_color(d.active_percent),
                                    );
                                    ui.label(crate::i18n::format(
                                        "{0}% busy",
                                        &[format!("{:.0}", d.active_percent)],
                                    ));
                                });
                                ui.weak(crate::i18n::format(
                                    "read {0}/s · write {1}/s",
                                    &[format_bytes(d.read_rate), format_bytes(d.write_rate)],
                                ));
                                ui.end_row();
                            }
                        });
                },
                |ui| {
                    ui.strong(crate::i18n::tr("Who is using the disks"));
                    let mut busy: Vec<_> = snap
                        .processes
                        .iter()
                        .filter(|p| p.io_rate() >= 1.0)
                        .collect();
                    busy.sort_by(|a, b| b.io_rate().total_cmp(&a.io_rate()));
                    if busy.is_empty() {
                        ui.weak(crate::i18n::tr(
                            "No process is reading or writing right now.",
                        ));
                    }
                    egui::Grid::new("disk_processes")
                        .num_columns(3)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            for p in busy.into_iter().take(TOP_PROCESSES) {
                                ui.label(&p.name);
                                ui.label(crate::i18n::format(
                                    "read {0}/s",
                                    &[format_bytes(p.io_read_rate)],
                                ));
                                ui.label(crate::i18n::format(
                                    "write {0}/s",
                                    &[format_bytes(p.io_write_rate)],
                                ));
                                ui.end_row();
                            }
                        });
                    ui.weak(crate::i18n::tr(
                        "Includes file, network and device I/O of each process.",
                    ));
                },
            );
        },
    );
}

/// Side by side when there is room, otherwise one below the other.
fn two_up(ui: &mut Ui, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
    if ui.available_width() >= 900.0 {
        ui.columns(2, |cols| {
            left(&mut cols[0]);
            right(&mut cols[1]);
        });
    } else {
        left(ui);
        ui.add_space(8.0);
        right(ui);
    }
}

/// Files read and written right now, by program.
fn file_activity(ui: &mut Ui, data: &Data) {
    card(
        ui,
        RichText::new(crate::i18n::tr("Files being read and written"))
            .size(16.0)
            .strong(),
        |ui| {
            // A still label, not a spinner: nothing repaints while waiting.
            let Some(activity) = &data.disk_activity else {
                ui.weak(crate::i18n::tr("loading…"));
                return;
            };
            let (files, seconds) = match activity {
                Ok(a) => a,
                Err(e) => {
                    ui.weak(e);
                    return;
                }
            };
            if files.is_empty() {
                ui.weak(crate::i18n::tr(
                    "No disk reads or writes in the last seconds.",
                ));
                return;
            }
            let names: std::collections::HashMap<u32, &str> = data
                .snapshot
                .iter()
                .flat_map(|s| &s.processes)
                .map(|p| (p.pid, p.name.as_str()))
                .collect();
            let per_second = |b: u64| format_bytes(b as f64 / seconds.max(0.5));
            egui::Grid::new("file_activity")
                .num_columns(4)
                .striped(true)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    for label in [
                        crate::i18n::tr("Program"),
                        crate::i18n::tr("Read"),
                        crate::i18n::tr("Write"),
                        crate::i18n::tr("File"),
                    ] {
                        ui.strong(label);
                    }
                    ui.end_row();
                    for f in files.iter().take(20) {
                        ui.label(names.get(&f.pid).copied().unwrap_or("?"))
                            .on_hover_text(format!("PID {}", f.pid));
                        ui.label(format!("{}/s", per_second(f.read)));
                        ui.label(format!("{}/s", per_second(f.write)));
                        match &f.path {
                            Some(p) => ui.add(egui::Label::new(p).truncate()),
                            None => ui.weak(crate::i18n::tr("(name not known yet)")),
                        };
                        ui.end_row();
                    }
                });
        },
    );
}

fn analysis(ui: &mut Ui, view: &mut DisksView) {
    ui.horizontal(|ui| {
        ui.label(crate::i18n::tr("Folder:"));
        let edit = ui.add(
            egui::TextEdit::singleline(&mut view.path)
                .hint_text(r"C:\Users")
                .desired_width(360.0),
        );
        let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if view.scanning() {
            if ui.button(crate::i18n::tr("Stop")).clicked() {
                view.cancel();
            }
        } else if ui
            .button(crate::i18n::tr("Analyze"))
            .on_hover_text(crate::i18n::tr(
                "Runs in the background at low priority; the PC stays responsive",
            ))
            .clicked()
            || enter
        {
            let path = PathBuf::from(view.path.trim());
            view.start(ui.ctx(), &path);
        }
    });
    if let Some(e) = &view.error {
        ui.colored_label(BAD, e);
    }
    let Some(scan) = &view.scan else {
        ui.weak(crate::i18n::tr(
            "Choose a drive above, or type a folder, to see which folders and files take up the space.",
        ));
        return;
    };
    let shared = scan.result.clone();
    let guard = shared.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(result) = guard.as_ref() else {
        let p = &scan.progress;
        ui.ctx().request_repaint_after(Duration::from_millis(250));
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(crate::i18n::format(
                "Scanning {0}: {1} files, {2} in {3} folders ({4} s)",
                &[
                    scan.path.display().to_string(),
                    p.files.load(Ordering::Relaxed).to_string(),
                    format_bytes(p.bytes.load(Ordering::Relaxed) as f64),
                    p.folders.load(Ordering::Relaxed).to_string(),
                    scan.started.elapsed().as_secs().to_string(),
                ],
            ));
        });
        ui.add(egui::Label::new(RichText::new(p.current()).weak().small()).truncate());
        return;
    };
    summary(ui, result);
    ui.horizontal(|ui| {
        for (v, label) in [
            (ResultView::Folders, crate::i18n::tr("Folders")),
            (ResultView::Largest, crate::i18n::tr("Largest files")),
            (ResultView::Types, crate::i18n::tr("File types")),
        ] {
            ui.selectable_value(&mut view.view, v, label);
        }
    });
    ui.separator();
    let mut reveal = None;
    match view.view {
        ResultView::Folders => folders(ui, result, &mut view.expanded, &mut reveal),
        ResultView::Largest => largest(ui, result, &mut reveal),
        ResultView::Types => types(ui, result),
    }
    drop(guard);
    if let Some(path) = reveal
        && let Err(e) = crate::backend::reveal(&path)
    {
        view.error = Some(crate::i18n::format(
            "Cannot open {0}: {1}",
            &[path.display().to_string(), e.to_string()],
        ));
    }
}

fn summary(ui: &mut Ui, r: &ScanResult) {
    let mut text = crate::i18n::format(
        "{0} in {1} files and {2} folders, scanned in {3} s",
        &[
            format_bytes(r.root.size as f64),
            r.root.files.to_string(),
            r.folders.to_string(),
            format!("{:.1}", r.elapsed.as_secs_f64()),
        ],
    );
    if r.denied > 0 {
        text.push_str(&crate::i18n::format(
            " · {0} folders could not be read (restart as administrator to include them)",
            &[r.denied.to_string()],
        ));
    }
    ui.label(RichText::new(text).strong());
    if r.cancelled {
        ui.colored_label(
            WARN,
            crate::i18n::tr("Stopped early: the numbers cover only what was scanned."),
        );
    }
}

/// One visible line of the folder tree.
enum Line<'a> {
    Folder {
        node: &'a DirNode,
        index: Vec<usize>,
        depth: usize,
        path: PathBuf,
        parent_size: u64,
    },
    /// The files directly in a folder.
    Files {
        node: &'a DirNode,
        depth: usize,
        parent_size: u64,
    },
    /// Subfolders beyond `CHILDREN_SHOWN`.
    More {
        count: usize,
        size: u64,
        depth: usize,
        parent_size: u64,
    },
}

fn flatten<'a>(
    node: &'a DirNode,
    index: Vec<usize>,
    path: PathBuf,
    depth: usize,
    parent_size: u64,
    expanded: &HashSet<Vec<usize>>,
    out: &mut Vec<Line<'a>>,
) {
    let open = index.is_empty() || expanded.contains(&index);
    out.push(Line::Folder {
        node,
        index: index.clone(),
        depth,
        path: path.clone(),
        parent_size,
    });
    if !open {
        return;
    }
    for (i, child) in node.children.iter().enumerate().take(CHILDREN_SHOWN) {
        let mut idx = index.clone();
        idx.push(i);
        flatten(
            child,
            idx,
            path.join(&child.name),
            depth + 1,
            node.size,
            expanded,
            out,
        );
    }
    if node.children.len() > CHILDREN_SHOWN {
        let rest = &node.children[CHILDREN_SHOWN..];
        out.push(Line::More {
            count: rest.len(),
            size: rest.iter().map(|c| c.size).sum(),
            depth: depth + 1,
            parent_size: node.size,
        });
    }
    if node.own_files > 0 && !node.children.is_empty() {
        out.push(Line::Files {
            node,
            depth: depth + 1,
            parent_size: node.size,
        });
    }
}

fn share(size: u64, of: u64) -> f32 {
    if of == 0 {
        0.0
    } else {
        (size as f64 / of as f64) as f32
    }
}

fn size_cells(row: &mut egui_extras::TableRow<'_, '_>, size: u64, files: u64, parent: u64) {
    row.col(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(format_bytes(size as f64));
        });
    });
    row.col(|ui| {
        let s = share(size, parent);
        bar(ui, s, 90.0, ACCENT);
        ui.label(format!("{:.1}%", s * 100.0));
    });
    row.col(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(files.to_string());
        });
    });
}

fn folders(
    ui: &mut Ui,
    r: &ScanResult,
    expanded: &mut HashSet<Vec<usize>>,
    reveal: &mut Option<PathBuf>,
) {
    let mut lines = Vec::new();
    flatten(
        &r.root,
        Vec::new(),
        r.root_path.clone(),
        0,
        r.root.size,
        expanded,
        &mut lines,
    );
    let row_height = ui.spacing().interact_size.y;
    let mut toggle = None;
    let scroll = crate::theme::take_scroll(ui, "disk_folders");
    let table = TableBuilder::new(ui).id_salt("disk_folders");
    let table = match scroll {
        Some(y) => table.vertical_scroll_offset(y),
        None => table,
    };
    let out = table
        .striped(true)
        .resizable(true)
        .max_scroll_height(TABLE_HEIGHT)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::initial(420.0).clip(true))
        .column(Column::initial(100.0))
        .column(Column::initial(150.0))
        .column(Column::initial(90.0))
        .column(Column::remainder())
        .header(row_height, |mut h| {
            for label in [
                crate::i18n::tr("Folder"),
                crate::i18n::tr("Size"),
                crate::i18n::tr("Share"),
                crate::i18n::tr("Files"),
                "",
            ] {
                h.col(|ui| {
                    ui.strong(label);
                });
            }
        })
        .body(|body| {
            body.rows(row_height, lines.len(), |mut row| {
                match &lines[row.index()] {
                    Line::Folder {
                        node,
                        index,
                        depth,
                        path,
                        parent_size,
                    } => {
                        row.col(|ui| {
                            ui.add_space(*depth as f32 * 16.0);
                            let open = index.is_empty() || expanded.contains(index);
                            if node.children.is_empty() || index.is_empty() {
                                ui.add_space(22.0);
                            } else if ui.small_button(if open { "⏷" } else { "⏵" }).clicked() {
                                toggle = Some(index.clone());
                            }
                            let mut name = RichText::new(&node.name);
                            if node.denied {
                                name = name.color(WARN);
                            }
                            let label = ui.add(
                                egui::Label::new(name)
                                    .truncate()
                                    .sense(egui::Sense::click()),
                            );
                            if node.denied {
                                label.on_hover_text(crate::i18n::tr("Could not be read"));
                            } else if label.double_clicked()
                                && !node.children.is_empty()
                                && !index.is_empty()
                            {
                                toggle = Some(index.clone());
                            }
                        });
                        size_cells(&mut row, node.size, node.files, *parent_size);
                        row.col(|ui| {
                            if ui
                                .small_button(crate::i18n::tr("Show"))
                                .on_hover_text(crate::i18n::tr("Show in Explorer"))
                                .clicked()
                            {
                                *reveal = Some(path.clone());
                            }
                        });
                    }
                    Line::Files {
                        node,
                        depth,
                        parent_size,
                    } => {
                        row.col(|ui| {
                            ui.add_space(*depth as f32 * 16.0 + 22.0);
                            ui.label(
                                RichText::new(crate::i18n::tr("(files in this folder)"))
                                    .italics()
                                    .weak(),
                            );
                        });
                        size_cells(&mut row, node.own_size, node.own_files, *parent_size);
                        row.col(|_| {});
                    }
                    Line::More {
                        count,
                        size,
                        depth,
                        parent_size,
                    } => {
                        row.col(|ui| {
                            ui.add_space(*depth as f32 * 16.0 + 22.0);
                            ui.label(
                                RichText::new(crate::i18n::format(
                                    "{0} smaller folders",
                                    &[count.to_string()],
                                ))
                                .italics()
                                .weak(),
                            );
                        });
                        size_cells(&mut row, *size, 0, *parent_size);
                        row.col(|_| {});
                    }
                }
            });
        });
    crate::theme::more_below(ui, "disk_folders", &out, crate::theme::card_fill(ui));
    if let Some(index) = toggle
        && !expanded.remove(&index)
    {
        expanded.insert(index);
    }
}

fn largest(ui: &mut Ui, r: &ScanResult, reveal: &mut Option<PathBuf>) {
    if r.largest.is_empty() {
        ui.weak(crate::i18n::tr("No files found."));
        return;
    }
    let row_height = ui.spacing().interact_size.y;
    let scroll = crate::theme::take_scroll(ui, "disk_largest");
    let table = TableBuilder::new(ui).id_salt("disk_largest");
    let table = match scroll {
        Some(y) => table.vertical_scroll_offset(y),
        None => table,
    };
    let out = table
        .striped(true)
        .resizable(true)
        .max_scroll_height(TABLE_HEIGHT)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::initial(100.0))
        .column(Column::initial(130.0))
        .column(Column::initial(70.0))
        .column(Column::remainder().clip(true))
        .header(row_height, |mut h| {
            for label in [
                crate::i18n::tr("Size"),
                crate::i18n::tr("Modified"),
                "",
                crate::i18n::tr("File"),
            ] {
                h.col(|ui| {
                    ui.strong(label);
                });
            }
        })
        .body(|body| {
            body.rows(row_height, r.largest.len(), |mut row| {
                let f = &r.largest[row.index()];
                row.col(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(format_bytes(f.size as f64)).strong());
                    });
                });
                row.col(|ui| {
                    ui.label(f.modified.map_or_else(String::new, |m| {
                        chrono::DateTime::<chrono::Local>::from(m)
                            .format("%Y-%m-%d %H:%M")
                            .to_string()
                    }));
                });
                row.col(|ui| {
                    if ui
                        .small_button(crate::i18n::tr("Show"))
                        .on_hover_text(crate::i18n::tr("Show in Explorer"))
                        .clicked()
                    {
                        *reveal = Some(f.path.clone());
                    }
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(f.path.display().to_string()).truncate());
                });
            });
        });
    crate::theme::more_below(ui, "disk_largest", &out, crate::theme::card_fill(ui));
}

fn types(ui: &mut Ui, r: &ScanResult) {
    if r.types.is_empty() {
        ui.weak(crate::i18n::tr("No files found."));
        return;
    }
    let mut area = egui::ScrollArea::vertical()
        .id_salt("disk_types")
        .max_height(TABLE_HEIGHT);
    if let Some(y) = crate::theme::take_scroll(ui, "disk_types") {
        area = area.vertical_scroll_offset(y);
    }
    let out = area.show(ui, |ui| {
        egui::Grid::new("disk_types_grid")
            .num_columns(4)
            .striped(true)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                for label in [
                    crate::i18n::tr("Type"),
                    crate::i18n::tr("Size"),
                    crate::i18n::tr("Share"),
                    crate::i18n::tr("Files"),
                ] {
                    ui.strong(label);
                }
                ui.end_row();
                for t in &r.types {
                    ui.label(match t.extension.as_str() {
                        "" => crate::i18n::tr("(no extension)").to_owned(),
                        "*" => crate::i18n::tr("All other types").to_owned(),
                        e => format!(".{e}"),
                    });
                    ui.label(format_bytes(t.size as f64));
                    ui.horizontal(|ui| {
                        let s = share(t.size, r.root.size);
                        bar(ui, s, 160.0, ACCENT);
                        ui.label(format!("{:.1}%", s * 100.0));
                    });
                    ui.label(t.files.to_string());
                    ui.end_row();
                }
            });
    });
    crate::theme::more_below(ui, "disk_types", &out, crate::theme::card_fill(ui));
}

/// Drives and a finished analysis for the visual review.
#[cfg(all(test, feature = "visual-tests"))]
pub fn review_fixture() -> DisksView {
    let volume = |root: &str, label: &str, total: u64, free: u64| Volume {
        root: root.into(),
        label: label.into(),
        file_system: "NTFS".into(),
        kind: DriveKind::Fixed,
        total: total << 30,
        free: free << 30,
    };
    let dir = |name: &str, gib: u64, files: u64, children: Vec<DirNode>| DirNode {
        name: name.into(),
        size: gib << 30,
        files,
        own_size: 0,
        own_files: 0,
        children,
        denied: false,
    };
    let mut root = dir(
        r"C:\",
        412,
        1_204_311,
        vec![
            dir("Users", 214, 402_118, vec![dir("me", 210, 398_002, vec![])]),
            dir("Windows", 38, 512_004, vec![]),
            dir("Program Files", 61, 160_220, vec![]),
            dir("ProgramData", 22, 90_441, vec![]),
        ],
    );
    root.own_size = 77 << 30;
    root.own_files = 3;
    let mut view = DisksView::with_volumes(vec![
        volume(r"C:\", "Windows", 953, 541),
        volume(r"D:\", "Games", 1863, 112),
    ])
    .with_result(ScanResult {
        root_path: r"C:\".into(),
        root,
        largest: vec![diskscan::BigFile {
            path: r"C:\pagefile.sys".into(),
            size: 32 << 30,
            modified: None,
        }],
        types: vec![diskscan::FileType {
            extension: "vhdx".into(),
            size: 96 << 30,
            files: 2,
        }],
        folders: 301_772,
        denied: 0,
        elapsed: Duration::from_secs(41),
        cancelled: false,
    });
    view.path = r"C:\".into();
    view.expanded.insert(vec![0]);
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, size: u64, own: u64, children: Vec<DirNode>) -> DirNode {
        DirNode {
            name: name.into(),
            size,
            files: 1,
            own_size: own,
            own_files: u64::from(own > 0),
            children,
            denied: false,
        }
    }

    #[test]
    fn flattening_follows_expansion_and_caps_children() {
        let many: Vec<_> = (0..CHILDREN_SHOWN + 3)
            .map(|i| node(&format!("d{i}"), 1, 1, vec![]))
            .collect();
        let root = node(
            "C:\\",
            1000,
            10,
            vec![
                node("big", 600, 0, vec![node("inner", 600, 600, vec![])]),
                node("many", 390, 0, many),
            ],
        );
        let mut out = Vec::new();
        flatten(
            &root,
            vec![],
            "C:\\".into(),
            0,
            1000,
            &HashSet::new(),
            &mut out,
        );
        // Root, its two folders, and the root's own files.
        assert_eq!(out.len(), 4);
        assert!(matches!(out[3], Line::Files { .. }));

        let expanded: HashSet<_> = [vec![0], vec![1]].into_iter().collect();
        let mut out = Vec::new();
        flatten(&root, vec![], "C:\\".into(), 0, 1000, &expanded, &mut out);
        let Line::Folder {
            node, path, depth, ..
        } = &out[2]
        else {
            panic!("inner folder");
        };
        assert_eq!((node.name.as_str(), *depth), ("inner", 2));
        assert_eq!(path, &PathBuf::from(r"C:\big\inner"));
        assert!(out.iter().any(|l| matches!(
            l,
            Line::More {
                count: 3,
                size: 3,
                ..
            }
        )));
        assert_eq!(share(250, 1000), 0.25);
        assert_eq!(share(1, 0), 0.0);
    }

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    struct State {
        data: Data,
        view: DisksView,
    }

    fn harness(view: DisksView) -> Harness<'static, State> {
        let mut data = crate::ui_tests::data();
        let snap = std::sync::Arc::make_mut(data.snapshot.as_mut().unwrap());
        snap.processes[2].io_write_rate = 4.0 * 1024.0 * 1024.0;
        Harness::builder()
            .allow_missing_glyphs()
            .with_size([1300.0, 1400.0])
            .build_ui_state(
                |ui, s: &mut State| disks_tab(ui, &s.data, &mut s.view),
                State { data, view },
            )
    }

    fn drive() -> Volume {
        Volume {
            root: r"Z:\".into(),
            label: "Data".into(),
            file_system: "NTFS".into(),
            kind: DriveKind::Fixed,
            total: 1000 << 30,
            free: 50 << 30,
        }
    }

    #[test]
    fn shows_which_files_are_read_and_written() {
        let mut h = harness(DisksView::with_volumes(vec![]));
        h.state_mut().data.disk_activity = Some(Ok((
            vec![
                wtm_core::FileIo {
                    pid: 100,
                    path: Some("C:/data/app.db".into()),
                    read: 4 << 20,
                    write: 2 << 20,
                },
                wtm_core::FileIo {
                    pid: 200,
                    path: None,
                    read: 0,
                    write: 1024,
                },
            ],
            2.0,
        )));
        h.run();
        assert!(h.query_by_label("C:/data/app.db").is_some());
        assert!(h.query_by_label("2.0 MiB/s").is_some(), "4 MiB over 2 s");
        assert!(h.query_by_label("(name not known yet)").is_some());
        // PID 200 by name, here and among the busiest processes.
        assert!(h.get_all_by_label("beta.exe").count() >= 2);
        h.state_mut().data.disk_activity = Some(Err("needs rights".into()));
        h.run();
        assert!(h.query_by_label("needs rights").is_some());
    }

    #[test]
    fn shows_drives_activity_and_who_writes() {
        let mut h = harness(DisksView::with_volumes(vec![drive()]));
        h.run();
        assert!(h.query_by_label("Data (Z:)").is_some());
        assert!(h.query_by_label("50.0 GiB free of 1000.0 GiB").is_some());
        assert!(h.query_by_label("95%").is_some());
        assert!(h.query_by_label("Disk 0 C:").is_some());
        assert!(h.query_by_label("write 4.0 MiB/s").is_some());
        // Nothing analyzed yet.
        assert!(h.query_by_label_contains("Choose a drive above").is_some());
        // A path that is not a folder is refused with a message.
        h.state_mut().view.path = r"Z:\no\such\folder".into();
        // The drive card has its own Analyze button; the folder row's is last.
        h.get_all_by_label("Analyze").last().unwrap().click();
        h.run();
        assert!(
            h.query_by_label_contains("is not a folder that can be read")
                .is_some()
        );
    }

    #[test]
    fn analyzes_a_folder_and_browses_the_results() {
        let root = std::env::temp_dir().join(format!("wtm-disks-tab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("media/inner")).unwrap();
        std::fs::write(root.join("media/movie.mkv"), vec![0u8; 3 << 20]).unwrap();
        std::fs::write(root.join("media/inner/song.mp3"), vec![0u8; 1 << 20]).unwrap();
        std::fs::write(root.join("note.txt"), vec![0u8; 100]).unwrap();

        let mut h = harness(DisksView::with_volumes(vec![]));
        h.state_mut().view.path = root.display().to_string();
        h.get_by_label("Analyze").click();
        // The click is handled on the next frame, which starts the scan.
        h.step();
        assert!(h.state().view.scan.is_some(), "the scan started");
        let started = Instant::now();
        while h.state().view.scanning() {
            assert!(started.elapsed() < Duration::from_secs(20), "scan finishes");
            std::thread::sleep(Duration::from_millis(20));
        }
        // Frames requested while the scan ran (its spinner) still drain.
        h.run_steps(3);
        let _ = std::fs::remove_dir_all(&root);

        assert!(
            h.query_by_label_contains("4.0 MiB in 3 files and 3 folders")
                .is_some()
        );
        assert!(h.query_by_label("media").is_some());
        assert!(h.query_by_label("inner").is_none(), "collapsed");
        assert!(h.query_by_label("(files in this folder)").is_some());
        h.get_by_label("⏵").click();
        h.run();
        assert!(h.query_by_label("inner").is_some(), "expanded");

        h.get_by_label("Largest files").click();
        h.run();
        assert!(h.query_by_label("3.0 MiB").is_some());
        assert!(h.query_by_label_contains("movie.mkv").is_some());

        h.get_by_label("File types").click();
        h.run();
        assert!(h.query_by_label(".mkv").is_some());
        assert!(h.query_by_label("75.0%").is_some());
    }

    #[test]
    fn a_scan_can_be_stopped_and_says_so() {
        let result = ScanResult {
            root_path: r"Z:\".into(),
            root: node(r"Z:\", 0, 0, vec![]),
            largest: vec![],
            types: vec![],
            folders: 1,
            denied: 2,
            elapsed: Duration::from_millis(1500),
            cancelled: true,
        };
        let mut h = harness(DisksView::with_volumes(vec![]).with_result(result));
        h.run();
        assert!(h.query_by_label_contains("Stopped early").is_some());
        assert!(
            h.query_by_label_contains("2 folders could not be read")
                .is_some()
        );
        h.get_by_label("Largest files").click();
        h.run();
        assert!(h.query_by_label("No files found.").is_some());
    }

    #[test]
    fn colors_and_labels() {
        assert_eq!(used_color(95.0), BAD);
        assert_eq!(used_color(80.0), WARN);
        assert_eq!(used_color(10.0), GOOD);
        assert_eq!(kind_label(DriveKind::Fixed), "Local disk");
    }
}

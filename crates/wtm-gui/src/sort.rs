//! Click-to-sort table headers shared by every list.

use std::cmp::Ordering;

use eframe::egui::{self, Align, Id, Layout, RichText, Ui};

/// Sort column and direction of one table, remembered per table id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Sort {
    pub col: usize,
    pub desc: bool,
}

impl Sort {
    pub const fn new(col: usize, desc: bool) -> Self {
        Self { col, desc }
    }

    /// The sort the user chose for table `table`, or `default`. Kept across
    /// restarts with the rest of the egui memory.
    pub fn load(ui: &Ui, table: &str, default: Sort) -> Sort {
        ui.data_mut(|d| d.get_persisted(Id::unique(("sort", table))))
            .unwrap_or(default)
    }

    pub fn store(self, ui: &Ui, table: &str) {
        ui.data_mut(|d| d.insert_persisted(Id::unique(("sort", table)), self));
    }

    /// Sorts `rows` with `cmp(a, b, column)` in this direction.
    pub fn apply<T>(self, rows: &mut [T], cmp: impl Fn(&T, &T, usize) -> Ordering) {
        rows.sort_by(|a, b| {
            let o = cmp(a, b, self.col);
            if self.desc { o.reverse() } else { o }
        });
    }

    /// Sorts `rows` by the text `key` makes of each, in this direction. For
    /// formatted columns: each text is made once, not twice per comparison.
    pub fn apply_text<T>(self, rows: &mut Vec<T>, key: impl Fn(&T) -> String) {
        let mut keyed: Vec<(String, T)> = std::mem::take(rows)
            .into_iter()
            .map(|r| (key(&r), r))
            .collect();
        self.apply(&mut keyed, |a, b, _| text(&a.0, &b.0));
        rows.extend(keyed.into_iter().map(|(_, r)| r));
    }
}

/// A header cell: clicking sorts by `col`, clicking again reverses. Numeric
/// columns start with the largest value, text columns with A.
pub fn header(ui: &mut Ui, label: &str, col: usize, numeric: bool, sort: &mut Sort) {
    let current = (sort.col == col).then_some(sort.desc);
    let response = cell_response(ui, crate::i18n::tr(label), numeric, current);
    if response.clicked() {
        *sort = if sort.col == col {
            Sort::new(col, !sort.desc)
        } else {
            Sort::new(col, numeric)
        };
    }
    crate::context_actions::menu(&response, ("sort", response.id), |ui| {
        if crate::context_actions::action(ui, crate::i18n::tr("Sort ascending")) {
            *sort = Sort::new(col, false);
        }
        if crate::context_actions::action(ui, crate::i18n::tr("Sort descending")) {
            *sort = Sort::new(col, true);
        }
        crate::theme::divider(ui);
        crate::theme::TableLines::controls(ui);
    });
}

/// The look of every sortable header: the column sorted by is highlighted
/// with its direction, numeric columns are aligned right like their values.
/// `current` is `Some(descending)` for the sorted column. Returns a click.
pub fn cell_response(
    ui: &mut Ui,
    label: &str,
    numeric: bool,
    current: Option<bool>,
) -> egui::Response {
    let arrow = match current {
        Some(true) => " ↓",
        Some(false) => " ↑",
        None => "",
    };
    let text = RichText::new(format!("{label}{arrow}")).strong();
    let hint = crate::i18n::format("Sort by {0}", &[label.to_owned()]);
    if let Some((rect, background)) = ui.data(|d| {
        d.get_temp::<(egui::Rect, egui::layers::ShapeIdx)>(Id::unique("table-cell-context"))
    }) {
        let galley = egui::WidgetText::from(text).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            ui.available_width(),
            egui::TextStyle::Body,
        );
        let (_, content) = ui.allocate_space(egui::vec2(
            ui.available_width(),
            ui.spacing().interact_size.y,
        ));
        let response = ui.interact(
            rect,
            ui.scope_id().with("sort-header"),
            egui::Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                ui.is_enabled(),
                format!("{label}{arrow}"),
            )
        });
        let visuals = ui.style().interact_selectable(&response, current.is_some());
        if current.is_some() || response.hovered() {
            ui.painter().set(
                background,
                egui::Shape::rect_filled(rect.shrink(1.0), 0.0, visuals.bg_fill),
            );
        }
        let x = if numeric {
            content.right() - galley.size().x
        } else {
            content.left()
        };
        ui.painter().galley(
            egui::pos2(x, content.center().y - galley.size().y * 0.5),
            galley,
            visuals.fg_stroke.color,
        );
        return response.on_hover_text(hint);
    }
    let add = |ui: &mut Ui| {
        ui.scope(|ui| {
            // The table cell already provides horizontal padding. Do not
            // charge for it twice and truncate short headers such as CPU.
            ui.spacing_mut().button_padding.x = 0.0;
            ui.add(egui::Button::selectable(current.is_some(), text))
                .on_hover_text(hint)
        })
        .inner
    };
    if numeric {
        ui.with_layout(Layout::right_to_left(Align::Center), add)
            .inner
    } else {
        add(ui)
    }
}

/// A starting width for a table column: at least `width`, and wide enough
/// for its translated header `label` with the sort arrow.
pub fn width(ui: &Ui, label: &str, width: f32) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let text = ui.fonts_mut(|f| {
        f.layout_no_wrap(
            format!("{} ↓", crate::i18n::tr(label)),
            font,
            egui::Color32::WHITE,
        )
        .size()
        .x
    });
    width.max(text + 2.0 * ui.spacing().button_padding.x + 8.0)
}

/// Case-insensitive text order.
/// Text columns sort the way people read lists: "app2" before "app10", and
/// case and accents do not split names apart.
pub fn text(a: &str, b: &str) -> Ordering {
    wtm_sys::collate::natural(a, b)
}

pub fn num(a: f64, b: f64) -> Ordering {
    a.total_cmp(&b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sortable_header_uses_the_entire_cell() {
        use egui_kittest::kittest::Queryable;
        let mut h = egui_kittest::Harness::builder()
            .allow_missing_glyphs()
            .build_ui_state(
                |ui, sort: &mut Sort| {
                    ui.allocate_ui(egui::vec2(220.0, 32.0), |ui| {
                        crate::theme::table_cell(ui, |ui| header(ui, "Name", 0, false, sort));
                    });
                },
                Sort::new(0, false),
            );
        h.run();
        let rect = h.get_by_label("Name ↑").rect();
        assert!(
            rect.width() >= 220.0 && rect.height() >= 32.0,
            "whole cell is operable: {rect:?}"
        );
        let pos = rect.right_top() + egui::vec2(-3.0, 3.0);
        h.event(egui::Event::PointerMoved(pos));
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        h.run();
        assert_eq!(*h.state(), Sort::new(0, true));
    }

    #[test]
    fn sorts_rows_and_toggles_direction() {
        let mut rows = vec![("b", 2.0), ("A", 3.0), ("c", 1.0)];
        Sort::new(0, false).apply(&mut rows, |a, b, _| text(a.0, b.0));
        assert_eq!(rows[0].0, "A");
        Sort::new(1, true).apply(&mut rows, |a, b, _| num(a.1, b.1));
        assert_eq!(rows[0].1, 3.0);

        let mut sort = Sort::new(0, false);
        let mut h = egui_kittest::Harness::builder()
            .allow_missing_glyphs()
            .build_ui(|ui| {
                header(ui, "Name", 0, false, &mut sort);
                header(ui, "CPU", 1, true, &mut sort);
                sort.store(ui, "t");
                assert_eq!(Sort::load(ui, "t", Sort::new(9, false)), sort);
                assert_eq!(Sort::load(ui, "other", Sort::new(9, false)).col, 9);
            });
        h.run();
        use egui_kittest::kittest::Queryable;
        h.get_by_label("CPU").click();
        h.run();
        h.get_by_label("CPU ↓").click();
        h.run();
        h.get_by_label("Name").click();
        h.run();
        drop(h);
        assert_eq!(sort, Sort::new(0, false));
    }
}

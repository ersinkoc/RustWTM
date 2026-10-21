//! Click-to-sort table headers shared by every list.

use std::cmp::Ordering;

use eframe::egui::{self, Id, RichText, Ui};

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
    let label = crate::i18n::tr(label);
    let arrow = match (sort.col == col, sort.desc) {
        (true, true) => " ↓",
        (true, false) => " ↑",
        _ => "",
    };
    let button = egui::Button::new(RichText::new(format!("{label}{arrow}")).strong()).frame(false);
    if ui
        .add(button)
        .on_hover_text(crate::i18n::tr("Sort"))
        .clicked()
    {
        *sort = if sort.col == col {
            Sort::new(col, !sort.desc)
        } else {
            Sort::new(col, numeric)
        };
    }
}

/// Case-insensitive text order.
pub fn text(a: &str, b: &str) -> Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}

pub fn num(a: f64, b: f64) -> Ordering {
    a.total_cmp(&b)
}

#[cfg(test)]
mod tests {
    use super::*;

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

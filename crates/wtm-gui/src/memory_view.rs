//! The composition of physical memory on the Memory performance page: a bar
//! of in use, modified, standby and free pages, and the kernel pools and
//! hardware reservation beside it.

use eframe::egui::{self, Color32, Ui};
use wtm_core::format_bytes;
use wtm_sys::memory::Composition;

/// One part of the bar: label, bytes, colour and what it means.
struct Part {
    label: &'static str,
    bytes: u64,
    color: Color32,
    hint: &'static str,
}

/// The bar's parts, left to right, when the page lists are known. "In use"
/// leaves out the modified pages, which the bar shows on their own.
fn parts(c: &Composition, ui: &Ui) -> Option<[Part; 4]> {
    let (modified, standby, free) = (c.modified?, c.standby?, c.free?);
    let in_use = c
        .total
        .saturating_sub(modified)
        .saturating_sub(standby)
        .saturating_sub(free);
    let track = crate::theme::track_color(ui);
    Some([
        Part {
            label: crate::i18n::tr("In use"),
            bytes: in_use,
            color: crate::theme::MEMORY,
            hint: crate::i18n::tr("Used by programs, drivers and Windows."),
        },
        Part {
            label: crate::i18n::tr("Modified"),
            bytes: modified,
            color: crate::theme::MEMORY.gamma_multiply(0.65),
            hint: crate::i18n::tr(
                "Changed data that must be written to disk before the memory can be reused.",
            ),
        },
        Part {
            label: crate::i18n::tr("Standby"),
            bytes: standby,
            color: crate::theme::MEMORY.gamma_multiply(0.35),
            hint: crate::i18n::tr(
                "Cached data that is not in active use and is given to programs as soon as they need it.",
            ),
        },
        Part {
            label: crate::i18n::tr("Free"),
            bytes: free,
            color: track,
            hint: crate::i18n::tr("Memory holding nothing, ready at once."),
        },
    ])
}

/// Shows the composition, or nothing when it could not be measured.
pub fn show(ui: &mut Ui, composition: Option<&Composition>) {
    let Some(c) = composition.filter(|c| c.total > 0) else {
        return;
    };
    ui.add_space(0.0);
    if let Some(parts) = parts(c, ui) {
        bar(ui, &parts, c.total);
        ui.horizontal_wrapped(|ui| {
            for part in &parts {
                legend(ui, part);
            }
        });
    }
    ui.horizontal_wrapped(|ui| {
        let mut facts = vec![
            (crate::i18n::tr("Paged pool"), c.paged_pool, crate::i18n::tr("Kernel memory that Windows may move to disk.")),
            (crate::i18n::tr("Non-paged pool"), c.nonpaged_pool, crate::i18n::tr("Kernel memory that must stay in RAM. Steady growth points to a driver leak.")),
        ];
        if let Some(cached) = c.cached() {
            facts.push((crate::i18n::tr("Cached"), cached, crate::i18n::tr("Standby and modified pages together.")));
        }
        if let Some(reserved) = c.hardware_reserved() {
            facts.push((crate::i18n::tr("Hardware reserved"), reserved, crate::i18n::tr("Installed memory kept by firmware, integrated graphics or devices; Windows cannot use it.")));
        }
        for (i, (label, bytes, hint)) in facts.into_iter().enumerate() {
            if i > 0 {
                ui.weak("·");
            }
            ui.weak(format!("{label} {}", format_bytes(bytes as f64)))
                .on_hover_text(hint);
        }
    });
}

fn bar(ui: &mut Ui, parts: &[Part], total: u64) {
    let height = ui.spacing().interact_size.y * 0.6;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, crate::theme::track_color(ui));
    let mut x = rect.left();
    let mut hovered = None;
    for part in parts {
        let width = rect.width() * (part.bytes as f32 / total as f32).clamp(0.0, 1.0);
        let segment =
            egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(width, height));
        painter.rect_filled(segment, 0.0, part.color);
        if response.hover_pos().is_some_and(|p| segment.contains(p)) {
            hovered = Some(part);
        }
        x += width;
    }
    if let Some(part) = hovered {
        response.on_hover_text(format!(
            "{} {}\n{}",
            part.label,
            format_bytes(part.bytes as f64),
            part.hint
        ));
    }
}

fn legend(ui: &mut Ui, part: &Part) {
    let size = egui::vec2(10.0, 10.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, part.color);
    ui.label(format!(
        "{} {}",
        part.label,
        format_bytes(part.bytes as f64)
    ))
    .on_hover_text(part.hint);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    fn sample() -> Composition {
        Composition {
            total: 16 << 30,
            available: 8 << 30,
            installed: Some(17 << 30),
            paged_pool: 512 << 20,
            nonpaged_pool: 256 << 20,
            modified: Some(1 << 30),
            standby: Some(6 << 30),
            free: Some(2 << 30),
        }
    }

    #[test]
    fn in_use_leaves_out_the_page_lists_shown_beside_it() {
        let mut h = Harness::builder().build_ui_state(
            |ui, sizes: &mut Vec<u64>| {
                *sizes = parts(&sample(), ui).unwrap().map(|p| p.bytes).to_vec();
            },
            Vec::new(),
        );
        h.run();
        let sizes = h.state().clone();
        assert_eq!(sizes, [7 << 30, 1 << 30, 6 << 30, 2 << 30]);
        assert_eq!(sizes.iter().sum::<u64>(), 16 << 30);
    }

    #[test]
    fn the_breakdown_names_its_parts_and_hides_unknown_ones() {
        let mut c = sample();
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .build_ui(|ui| show(ui, Some(&c)));
        h.run();
        for text in [
            "In use",
            "Modified",
            "Standby",
            "Free",
            "Paged pool",
            "Hardware reserved",
        ] {
            assert!(h.query_by_label_contains(text).is_some(), "{text}");
        }
        drop(h);
        c.standby = None;
        c.installed = None;
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .build_ui(|ui| show(ui, Some(&c)));
        h.run();
        assert!(
            h.query_by_label_contains("Standby").is_none(),
            "no guessed lists"
        );
        assert!(h.query_by_label_contains("Hardware reserved").is_none());
        assert!(h.query_by_label_contains("Non-paged pool").is_some());
    }
}

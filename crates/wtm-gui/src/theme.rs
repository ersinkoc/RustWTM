//! The look of RustWTM: colours, the light and dark styles, and drawing
//! helpers shared by the tabs (cards, sparklines, ring gauges).

use std::collections::VecDeque;

use eframe::egui::{
    self, Color32, CornerRadius, Margin, Pos2, Rect, Shape, Stroke, Theme, Ui, Vec2, epaint,
};

/// The logo's orange.
pub const ACCENT: Color32 = Color32::from_rgb(222, 120, 60);
pub const CPU: Color32 = Color32::from_rgb(56, 139, 230);
pub const MEMORY: Color32 = Color32::from_rgb(160, 108, 213);
pub const GPU: Color32 = Color32::from_rgb(63, 185, 80);
pub const NET_RX: Color32 = Color32::from_rgb(20, 170, 170);
pub const NET_TX: Color32 = Color32::from_rgb(230, 150, 40);
pub const DISK_READ: Color32 = Color32::from_rgb(90, 160, 60);
pub const DISK_WRITE: Color32 = Color32::from_rgb(220, 90, 90);

pub const GOOD: Color32 = Color32::from_rgb(80, 180, 100);
pub const WARN: Color32 = Color32::from_rgb(230, 140, 40);
pub const BAD: Color32 = Color32::from_rgb(220, 70, 70);

/// The chart palette is bright for dark backgrounds. Use a darker tone for
/// small coloured text on light surfaces, while retaining the resource hue.
pub fn text_color(ui: &Ui, color: Color32) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(
            color.r().saturating_add(16),
            color.g().saturating_add(16),
            color.b().saturating_add(16),
        )
    } else {
        Color32::from_rgb(
            (u16::from(color.r()) * 3 / 5) as u8,
            (u16::from(color.g()) * 3 / 5) as u8,
            (u16::from(color.b()) * 3 / 5) as u8,
        )
    }
}

/// Green, orange from 60%, red from 90%.
pub fn load_color(percent: f32) -> Color32 {
    if percent >= 90.0 {
        BAD
    } else if percent >= 60.0 {
        WARN
    } else {
        GOOD
    }
}

/// Use the installed Windows UI font, with Symbol as a fallback for arrows,
/// status marks and controls. Fall back to egui's bundled fonts if unavailable.
/// Fonts stay on the machine; none are copied into the distribution.
pub fn install_fonts(ctx: &egui::Context) {
    let Some(windows) = std::env::var_os("WINDIR") else {
        return;
    };
    let dir = std::path::Path::new(&windows).join("Fonts");
    let mut fonts = egui::FontDefinitions::default();
    for (name, file, primary) in [
        ("Windows UI", "segoeui.ttf", true),
        ("Windows symbols", "seguisym.ttf", false),
    ] {
        if let Ok(bytes) = std::fs::read(dir.join(file)) {
            fonts.font_data.insert(
                name.into(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            for (family, names) in &mut fonts.families {
                if primary && *family == egui::FontFamily::Proportional {
                    names.insert(0, name.into());
                } else if !primary {
                    names.push(name.into());
                }
            }
        }
    }
    ctx.set_fonts(fonts);
}

/// Rounder, airier widgets with the accent colour, for both themes.
pub fn apply(ctx: &egui::Context) {
    for theme in [Theme::Light, Theme::Dark] {
        ctx.style_mut_of(theme, |style| {
            let dark = theme == Theme::Dark;
            style
                .text_styles
                .insert(egui::TextStyle::Heading, egui::FontId::proportional(23.0));
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
            style
                .text_styles
                .insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
            style
                .text_styles
                .insert(egui::TextStyle::Small, egui::FontId::proportional(11.0));
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(8.0, 4.0);
            style.spacing.interact_size.y = 28.0;
            let v = &mut style.visuals;
            let radius = CornerRadius::same(6);
            for w in [
                &mut v.widgets.noninteractive,
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.open,
            ] {
                w.corner_radius = radius;
            }
            v.window_corner_radius = CornerRadius::same(10);
            v.menu_corner_radius = CornerRadius::same(8);
            v.selection.bg_fill = if dark {
                Color32::from_rgb(76, 49, 34)
            } else {
                Color32::from_rgb(255, 232, 213)
            };
            // Text on selected items and inside progress bars.
            v.selection.stroke = Stroke::new(
                1.0,
                if dark {
                    Color32::WHITE
                } else {
                    Color32::from_rgb(35, 38, 45)
                },
            );
            v.hyperlink_color = if dark {
                ACCENT
            } else {
                Color32::from_rgb(157, 67, 20)
            };
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.7));
            if dark {
                v.panel_fill = Color32::from_rgb(19, 23, 29);
                v.window_fill = Color32::from_rgb(26, 31, 38);
                v.extreme_bg_color = Color32::from_rgb(14, 18, 23);
                v.faint_bg_color = Color32::from_rgb(25, 30, 37);
                v.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(198, 207, 218);
                v.widgets.noninteractive.bg_stroke.color = Color32::from_rgb(47, 56, 66);
                v.weak_text_color = Some(Color32::from_rgb(155, 166, 180));
            } else {
                v.panel_fill = Color32::from_rgb(246, 247, 249);
                v.window_fill = Color32::WHITE;
                v.extreme_bg_color = Color32::WHITE;
                v.faint_bg_color = Color32::from_rgb(238, 240, 244);
                v.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(55, 65, 78);
                v.widgets.noninteractive.bg_stroke.color = Color32::from_rgb(218, 224, 231);
                v.weak_text_color = Some(Color32::from_rgb(99, 112, 130));
            }
        });
    }
}

/// A rounded panel with a title, for one section of a tab.
pub fn card<R>(
    ui: &mut Ui,
    title: impl Into<egui::WidgetText>,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let dark = ui.visuals().dark_mode;
    egui::Frame::new()
        .fill(if dark {
            Color32::from_rgb(26, 31, 38)
        } else {
            Color32::WHITE
        })
        .stroke(Stroke::new(
            1.0,
            if dark {
                Color32::from_rgb(48, 52, 61)
            } else {
                Color32::from_rgb(222, 226, 232)
            },
        ))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(title.into());
            ui.add_space(4.0);
            add(ui)
        })
        .inner
}

/// A small history graph: area and line of `values` (0..=`max`, newest
/// last) in `rect`, oldest on the left.
pub fn sparkline(
    painter: &egui::Painter,
    rect: Rect,
    values: &VecDeque<f32>,
    max: f32,
    slots: usize,
    color: Color32,
) {
    if values.len() < 2 || max <= 0.0 {
        return;
    }
    let slots = slots.max(values.len()).max(2);
    let step = rect.width() / (slots - 1) as f32;
    let start = slots - values.len();
    let points: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = rect.left() + (start + i) as f32 * step;
            let y = rect.bottom() - rect.height() * (v / max).clamp(0.0, 1.0);
            egui::pos2(x, y)
        })
        .collect();
    // The area below the line, as one quad per segment.
    let mut mesh = epaint::Mesh::default();
    let fill = color.gamma_multiply(0.25);
    for w in points.windows(2) {
        let base = mesh.vertices.len() as u32;
        for p in [
            w[0],
            w[1],
            egui::pos2(w[1].x, rect.bottom()),
            egui::pos2(w[0].x, rect.bottom()),
        ] {
            mesh.colored_vertex(p, fill);
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(Shape::mesh(mesh));
    painter.add(Shape::line(points, Stroke::new(1.5, color)));
}

/// A ring filled to `percent`, with `center` text inside.
pub fn ring_gauge(
    ui: &mut Ui,
    diameter: f32,
    percent: f32,
    color: Color32,
    center: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let c = rect.center();
    let thickness = diameter * 0.11;
    let r = diameter / 2.0 - thickness / 2.0 - 1.0;
    let track = if ui.visuals().dark_mode {
        Color32::from_rgb(48, 52, 61)
    } else {
        Color32::from_rgb(226, 230, 236)
    };
    painter.circle_stroke(c, r, Stroke::new(thickness, track));
    let frac = (percent / 100.0).clamp(0.0, 1.0);
    if frac > 0.0 {
        // Clockwise from the top.
        let n = (64.0 * frac).ceil().max(2.0) as usize;
        let start = -std::f32::consts::FRAC_PI_2;
        let points: Vec<Pos2> = (0..=n)
            .map(|i| {
                let a = start + std::f32::consts::TAU * frac * i as f32 / n as f32;
                c + r * egui::vec2(a.cos(), a.sin())
            })
            .collect();
        painter.add(Shape::line(points, Stroke::new(thickness, color)));
    }
    painter.text(
        c,
        egui::Align2::CENTER_CENTER,
        center,
        egui::FontId::proportional(diameter * 0.22),
        ui.visuals().strong_text_color(),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn load_colours() {
        assert_eq!(load_color(95.0), BAD);
        assert_eq!(load_color(70.0), WARN);
        assert_eq!(load_color(10.0), GOOD);
    }

    #[test]
    fn resource_text_and_secondary_text_have_readable_contrast() {
        fn luminance(c: Color32) -> f32 {
            let channel = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            channel(c.r()) * 0.2126 + channel(c.g()) * 0.7152 + channel(c.b()) * 0.0722
        }
        let contrast = |a: Color32, b: Color32| {
            let (a, b) = (luminance(a), luminance(b));
            (a.max(b) + 0.05) / (a.min(b) + 0.05)
        };
        for theme in [Theme::Dark, Theme::Light] {
            let mut h = egui_kittest::Harness::builder()
                .with_theme(theme)
                .with_context_setup(apply)
                .build_ui(|ui| {
                    for color in [
                        CPU, MEMORY, GPU, NET_RX, NET_TX, DISK_READ, DISK_WRITE, GOOD, WARN, BAD,
                    ] {
                        assert!(contrast(text_color(ui, color), ui.visuals().panel_fill) >= 4.5);
                        let card = if ui.visuals().dark_mode {
                            Color32::from_rgb(26, 31, 38)
                        } else {
                            Color32::WHITE
                        };
                        assert!(contrast(text_color(ui, color), card) >= 4.5);
                    }
                    assert!(
                        contrast(ui.visuals().weak_text_color(), ui.visuals().panel_fill) >= 4.5
                    );
                    ui.label("Contrast checked");
                });
            h.run();
        }
    }

    #[test]
    fn styles_and_drawings_render_in_both_themes() {
        for theme in [Theme::Light, Theme::Dark] {
            let values: VecDeque<f32> = [0.0, 50.0, 100.0, 20.0].into_iter().collect();
            let mut h = egui_kittest::Harness::builder()
                .allow_missing_glyphs()
                .with_size([600.0, 400.0])
                .build_ui(move |ui| {
                    apply(ui.ctx());
                    ui.ctx().set_theme(theme);
                    card(ui, "Section", |ui| {
                        ui.label("inside");
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(100.0, 30.0), egui::Sense::hover());
                        sparkline(ui.painter(), rect, &values, 100.0, 60, CPU);
                        // Too little data or no scale: nothing drawn.
                        sparkline(
                            ui.painter(),
                            rect,
                            &values.iter().take(1).copied().collect(),
                            100.0,
                            60,
                            CPU,
                        );
                        sparkline(ui.painter(), rect, &values, 0.0, 60, CPU);
                        ring_gauge(ui, 100.0, 42.0, load_color(42.0), "42%");
                        ring_gauge(ui, 100.0, 0.0, GOOD, "0%");
                    });
                });
            h.run();
            assert!(h.query_by_label("Section").is_some());
            assert!(h.query_by_label("inside").is_some());
            let style = h.ctx.style_of(theme);
            assert_eq!(style.visuals.dark_mode, theme == Theme::Dark);
            assert!(style.visuals.hyperlink_color.is_opaque());
            if theme == Theme::Dark {
                assert_eq!(style.visuals.hyperlink_color, ACCENT);
            } else {
                assert!(
                    style.visuals.hyperlink_color.r() < ACCENT.r(),
                    "links use a darker orange on white"
                );
            }
        }
    }
}

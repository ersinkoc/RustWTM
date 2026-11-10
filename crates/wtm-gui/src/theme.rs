//! The look of RustWTM: colours, the light and dark styles, and drawing
//! helpers shared by the tabs (cards, sparklines, ring gauges).

use std::collections::VecDeque;

use eframe::egui::{
    self, Color32, CornerRadius, Margin, Pos2, Rect, RichText, Shape, Stroke, Theme, Ui, Vec2,
    epaint,
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

/// Square, adjoining surfaces with explicit separator lines, for both themes.
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
                .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
            style.spacing.item_spacing = CONTENT_SPACING;
            style.spacing.button_padding = CONTROL_PADDING;
            style.spacing.window_margin = CONTENT_PADDING;
            style.spacing.menu_margin = Margin::same(6);
            style.spacing.icon_spacing = 6.0;
            style.visuals.window_shadow = epaint::Shadow::NONE;
            style.visuals.popup_shadow = epaint::Shadow::NONE;
            style.visuals.widgets.inactive.bg_stroke =
                Stroke::new(1.0, style.visuals.widgets.noninteractive.bg_stroke.color);
            style.spacing.scroll.bar_inner_margin = 0.0;
            style.spacing.scroll.bar_outer_margin = 0.0;
            style.spacing.interact_size.y = 28.0;
            let v = &mut style.visuals;
            let radius = CornerRadius::ZERO;
            for w in [
                &mut v.widgets.noninteractive,
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.open,
            ] {
                w.corner_radius = radius;
            }
            v.window_corner_radius = CornerRadius::ZERO;
            v.menu_corner_radius = CornerRadius::ZERO;
            v.selection.bg_fill = if dark {
                Color32::from_rgb(47, 40, 35)
            } else {
                Color32::from_rgb(255, 241, 228)
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
                v.panel_fill = Color32::from_rgb(18, 22, 28);
                v.window_fill = Color32::from_rgb(25, 30, 37);
                v.extreme_bg_color = Color32::from_rgb(15, 19, 25);
                v.faint_bg_color = Color32::from_rgb(28, 33, 40);
                v.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(211, 220, 230);
                v.widgets.noninteractive.bg_stroke.color = Color32::from_rgb(47, 56, 66);
                v.weak_text_color = Some(Color32::from_rgb(155, 166, 180));
            } else {
                v.panel_fill = Color32::from_rgb(246, 247, 249);
                v.window_fill = Color32::WHITE;
                v.extreme_bg_color = Color32::WHITE;
                // Text fields stand out from the white cards they sit on.
                v.text_edit_bg_color = Some(FIELD_LIGHT);
                v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(206, 212, 221));
                v.faint_bg_color = Color32::from_rgb(238, 240, 244);
                v.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(55, 65, 78);
                v.widgets.noninteractive.bg_stroke.color = Color32::from_rgb(218, 224, 231);
                v.weak_text_color = Some(Color32::from_rgb(99, 112, 130));
            }
            // Explicit widget colours prevent egui's default grey blocks from
            // breaking the surface hierarchy. Hover/focus remain distinct.
            let ink = if dark {
                Color32::from_rgb(218, 224, 232)
            } else {
                Color32::from_rgb(43, 51, 63)
            };
            let border = if dark {
                Color32::from_rgb(52, 60, 71)
            } else {
                Color32::from_rgb(215, 221, 229)
            };
            let resting = if dark {
                Color32::from_rgb(29, 35, 43)
            } else {
                Color32::WHITE
            };
            let hover = if dark {
                Color32::from_rgb(39, 46, 56)
            } else {
                Color32::from_rgb(237, 240, 245)
            };
            let active = if dark {
                Color32::from_rgb(48, 55, 66)
            } else {
                Color32::from_rgb(227, 232, 240)
            };
            for (widget, fill) in [
                (&mut v.widgets.inactive, resting),
                (&mut v.widgets.hovered, hover),
                (&mut v.widgets.active, active),
                (&mut v.widgets.open, hover),
            ] {
                widget.bg_fill = fill;
                widget.weak_bg_fill = fill;
                widget.bg_stroke = Stroke::new(1.0, border);
                widget.fg_stroke = Stroke::new(1.0, ink);
                widget.expansion = 0.0;
            }
            // Rails and checkbox wells use the stronger fill; buttons use
            // weak_bg_fill, so they stay quiet without making sliders vanish.
            v.widgets.inactive.bg_fill = if dark {
                Color32::from_rgb(48, 56, 68)
            } else {
                Color32::from_rgb(218, 225, 235)
            };
            v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
            v.widgets.noninteractive.bg_fill = v.window_fill;
            v.widgets.noninteractive.weak_bg_fill = v.window_fill;
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border);
            v.text_edit_bg_color = Some(if dark {
                v.extreme_bg_color
            } else {
                FIELD_LIGHT
            });
        });
    }
    sharp_geometry(ctx);
}

/// Background of text fields on light surfaces.
const FIELD_LIGHT: Color32 = Color32::from_rgb(241, 243, 247);

/// Size of card titles and section labels.
const TITLE_SIZE: f32 = 16.0;

/// Outer surfaces join at their borders; contents have their own breathing room.
pub const CONTENT_PADDING: Margin = Margin::symmetric(10, 8);
/// Page gutters and section gaps are separate from control/cell padding.
pub const PAGE_PADDING: Margin = Margin::symmetric(12, 8);
pub const SECTION_SPACING: Vec2 = egui::vec2(8.0, 8.0);
pub const CONTROL_PADDING: Vec2 = egui::vec2(8.0, 4.0);
pub const CONTENT_SPACING: Vec2 = egui::vec2(6.0, 4.0);

pub fn surface(ui: &Ui) -> egui::Frame {
    egui::Frame::new()
        .fill(ui.visuals().panel_fill)
        .inner_margin(0.0)
        .outer_margin(0.0)
        .corner_radius(0.0)
}

pub fn solid_scroll() -> egui::style::ScrollStyle {
    egui::style::ScrollStyle {
        bar_inner_margin: 0.0,
        bar_outer_margin: 0.0,
        ..egui::style::ScrollStyle::solid()
    }
}

pub fn divider(ui: &mut Ui) -> egui::Response {
    ui.add(egui::Separator::default().spacing(1.0))
}

/// Shared table boundaries. No inset is added to the cell or its contents.
pub fn cell_lines(ui: &Ui) {
    // Place strokes inside the cell clip rect so shared edges stay visible.
    let rect = ui.max_rect().shrink(0.5);
    let stroke = Stroke::new(
        1.0,
        ui.visuals()
            .widgets
            .noninteractive
            .bg_stroke
            .color
            .gamma_multiply(0.45),
    );
    ui.painter()
        .line_segment([rect.right_top(), rect.right_bottom()], stroke);
    ui.painter()
        .line_segment([rect.left_bottom(), rect.right_bottom()], stroke);
}

/// Table cells touch at their boundaries, independently of form/control spacing.
pub fn table(ui: &mut Ui) -> egui_extras::TableBuilder<'_> {
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    egui_extras::TableBuilder::new(ui)
}

pub fn table_cell(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    cell_lines(ui);
    let header_background = ui.painter().add(egui::Shape::Noop);
    let key = egui::Id::unique("table-cell-context");
    let previous = ui.data_mut(|d| d.get_temp::<(egui::Rect, egui::layers::ShapeIdx)>(key));
    ui.data_mut(|d| d.insert_temp(key, (ui.max_rect(), header_background)));
    let size = ui.available_size();
    egui::Frame::new()
        .inner_margin(Margin::symmetric(6, 0))
        .show(ui, |ui| {
            ui.set_width((size.x - 12.0).max(0.0));
            ui.set_min_height(size.y);
            ui.spacing_mut().item_spacing = CONTENT_SPACING;
            add(ui);
        });
    ui.data_mut(|d| {
        d.remove::<(egui::Rect, egui::layers::ShapeIdx)>(key);
        if let Some(previous) = previous {
            d.insert_temp(key, previous);
        }
    });
}

/// Forms use their grid alignment; do not draw a box around every field row.
pub fn end_row(ui: &mut Ui) {
    ui.end_row();
}

pub fn workspace<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(PAGE_PADDING)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = SECTION_SPACING;
            add(ui)
        })
        .inner
}

pub fn choice<T: PartialEq>(
    ui: &mut Ui,
    current: &mut T,
    value: T,
    label: impl Into<egui::WidgetText>,
) -> egui::Response {
    let selected = *current == value;
    let mut response = ui.add(
        egui::Button::new(label)
            .selected(selected)
            .fill(if selected {
                ui.visuals().selection.bg_fill
            } else {
                Color32::TRANSPARENT
            })
            .stroke(Stroke::NONE),
    );
    if selected || response.hovered() || response.has_focus() {
        let color = if selected || response.has_focus() {
            ACCENT
        } else {
            ui.visuals().widgets.noninteractive.bg_stroke.color
        };
        let rect = response.rect;
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(2.0, color),
        );
    }
    if response.clicked() && !selected {
        *current = value;
        response.mark_changed();
    }
    response
}

/// Primary actions share a deliberate brand treatment; utility controls stay neutral.
pub fn primary_action(ui: &mut Ui, enabled: bool, label: &str) -> egui::Response {
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
        for (widget, fill) in [
            (
                &mut visuals.widgets.inactive,
                Color32::from_rgb(153, 71, 31),
            ),
            (&mut visuals.widgets.hovered, Color32::from_rgb(175, 83, 35)),
            (&mut visuals.widgets.active, Color32::from_rgb(132, 58, 25)),
        ] {
            widget.weak_bg_fill = fill;
            widget.bg_fill = fill;
            widget.bg_stroke = Stroke::new(1.0, fill);
            widget.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        }
        ui.add_enabled(
            enabled,
            egui::Button::new(RichText::new(label).strong()).min_size(egui::vec2(104.0, 28.0)),
        )
    })
    .inner
}

/// Apply the shared geometry even in embedded/headless application hosts.
pub fn sharp_geometry(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = CONTENT_SPACING;
        style.spacing.button_padding = CONTROL_PADDING;
        style.spacing.window_margin = CONTENT_PADDING;
        style.spacing.menu_margin = Margin::same(6);
        style.spacing.icon_spacing = 6.0;
        style.spacing.scroll.bar_inner_margin = 0.0;
        style.spacing.scroll.bar_outer_margin = 0.0;
        style.visuals.window_shadow = epaint::Shadow::NONE;
        style.visuals.popup_shadow = epaint::Shadow::NONE;
        style.visuals.widgets.inactive.bg_stroke =
            Stroke::new(1.0, style.visuals.widgets.noninteractive.bg_stroke.color);
        for widget in [
            &mut style.visuals.widgets.noninteractive,
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.open,
        ] {
            widget.corner_radius = CornerRadius::ZERO;
        }
        style.visuals.window_corner_radius = CornerRadius::ZERO;
        style.visuals.menu_corner_radius = CornerRadius::ZERO;
    });
}

/// A square panel with a title, for one section of a tab.
pub fn card<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    card_with(ui, title, &[], add)
}

/// A [`card`] whose title goes on with `parts` in their colours: current
/// values, or the legend of the graph inside. An empty `title` is left out.
pub fn card_with<R>(
    ui: &mut Ui,
    title: &str,
    parts: &[(&str, Color32)],
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    panel(ui, |ui| {
        let mut job = egui::text::LayoutJob::default();
        let format = |color| egui::TextFormat {
            font_id: egui::FontId::proportional(TITLE_SIZE),
            color,
            ..Default::default()
        };
        if !title.is_empty() {
            job.append(title, 0.0, format(ui.visuals().strong_text_color()));
        }
        for (text, color) in parts {
            if !job.text.is_empty() {
                job.append("   ·   ", 0.0, format(ui.visuals().weak_text_color()));
            }
            let mut legend = format(text_color(ui, *color));
            legend.font_id.size = 14.0;
            job.append(text, 0.0, legend);
        }
        job.wrap.max_width = ui.available_width();
        ui.label(job);
        ui.add_space(4.0);
        add(ui)
    })
}

/// A heading for a group of cards or a form within a page.
pub fn section(ui: &mut Ui, title: &str) {
    ui.label(RichText::new(title).size(TITLE_SIZE + 1.0).strong());
}

/// Text in a status colour, darkened on light surfaces (see [`text_color`]).
pub fn colored(ui: &mut Ui, color: Color32, text: impl Into<String>) -> egui::Response {
    ui.label(RichText::new(text.into()).color(text_color(ui, color)))
}

/// A spinner and what is being loaded, centred below the controls.
pub fn loading(ui: &mut Ui, text: &str) {
    ui.add_space(0.0);
    ui.vertical_centered(|ui| {
        ui.spinner();
        ui.weak(text);
    });
}

/// [`loading`] without the spinner, for data that may take long to come:
/// a spinner repaints the window all the while.
pub fn loading_still(ui: &mut Ui, text: &str) {
    ui.add_space(0.0);
    ui.vertical_centered(|ui| {
        ui.weak(text);
    });
}

/// Nothing to show: a title and a hint how to get something.
pub fn empty_state(ui: &mut Ui, title: &str, hint: &str) {
    ui.add_space(0.0);
    ui.label(RichText::new(title).size(TITLE_SIZE + 2.0).strong());
    ui.weak(hint);
}

/// A tinted full-width strip, for verdicts and warnings.
pub fn strip<R>(ui: &mut Ui, color: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.14))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.5)))
        .corner_radius(0.0)
        .inner_margin(CONTENT_PADDING)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A [`panel`] for a table whose columns are `min_width` wide together: it
/// scrolls sideways, with a visible bar, when the window is narrower.
pub fn table_panel<R>(ui: &mut Ui, id: &str, min_width: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    panel(ui, |ui| {
        let scroll = ui.spacing().scroll;
        ui.spacing_mut().scroll = solid_scroll();
        egui::ScrollArea::horizontal()
            .id_salt(("table_panel", id))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().scroll = scroll;
                ui.set_min_width(min_width);
                add(ui)
            })
            .inner
    })
}

/// A cell of a table or grid with its text on the right, for numbers.
pub fn right(ui: &mut Ui, text: impl Into<egui::WidgetText>) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(text);
    });
}

/// Ctrl+F moves the keyboard focus to the search field `field`.
pub fn focus_on_ctrl_f(ui: &Ui, field: &egui::Response) {
    if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
        field.request_focus();
    }
}

/// The search of a table toolbar: a label and a field that Ctrl+F focuses
/// and Escape or the ✖ button clears. Placed inline, so it wraps with the
/// rest of a `horizontal_wrapped` toolbar in a narrow window.
pub fn search_bar(ui: &mut Ui, id: egui::Id, text: &mut String, hint: &str, width: f32) {
    ui.label(crate::i18n::tr("Search:"));
    let field = ui
        .add(
            egui::TextEdit::singleline(text)
                .id(id)
                .hint_text(hint)
                .desired_width(width),
        )
        .on_hover_text("Ctrl+F");
    focus_on_ctrl_f(ui, &field);
    // TextEdit gives up focus on Escape in the same frame.
    if (field.has_focus() || field.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        text.clear();
    }
    if !text.is_empty()
        && ui
            .small_button("✖")
            .on_hover_text(crate::i18n::tr("Clear search"))
            .clicked()
    {
        text.clear();
    }
}

/// Ends the last cell of a striped grid at the right edge, so its stripes
/// span the whole card.
pub fn fill_row(ui: &mut Ui) {
    let width = ui.available_width();
    if width > 0.0 {
        ui.allocate_space(egui::vec2(width, 0.0));
    }
}

/// The empty part of a bar or meter.
pub fn track_color(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        ui.visuals().extreme_bg_color
    } else {
        Color32::from_rgb(226, 230, 236)
    }
}

/// A tile inside a card, such as a drive: set apart from the card in both
/// themes.
pub fn tile_fill(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(35, 41, 50)
    } else {
        FIELD_LIGHT
    }
}

/// A [`card`] without a title, for a page that is one section.
pub fn panel<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(card_fill(ui))
        .stroke(Stroke::new(
            1.0,
            ui.visuals()
                .widgets
                .noninteractive
                .bg_stroke
                .color
                .gamma_multiply(0.75),
        ))
        .corner_radius(0.0)
        .inner_margin(CONTENT_PADDING)
        .outer_margin(0)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = CONTENT_SPACING;
            ui.set_width(ui.available_width());
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
    let painter = painter.with_clip_rect(rect);
    // Reserve the stroke/endpoint footprint, including antialiasing, at 0/100%.
    let rect = rect.shrink(0.75);
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
    let fill = color.gamma_multiply(0.30);
    let clear = color.gamma_multiply(0.015);
    for w in points.windows(2) {
        let base = mesh.vertices.len() as u32;
        for (p, tint) in [
            (w[0], fill),
            (w[1], fill),
            (egui::pos2(w[1].x, rect.bottom()), clear),
            (egui::pos2(w[0].x, rect.bottom()), clear),
        ] {
            mesh.colored_vertex(p, tint);
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(Shape::mesh(mesh));
    let last = points.last().copied();
    painter.add(Shape::line(points, Stroke::new(1.75, color)));
    if rect.height() >= 24.0
        && let Some(last) = last
    {
        painter.circle_filled(last, 2.25, color);
    }
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
    let thickness = diameter * 0.085;
    // A centered path stroke keeps the entire ring inside its allocation.
    // CircleShape strokes are outside in epaint; treating those as centered
    // made the old track overflow and get cropped flat on all four sides.
    let r = diameter * 0.40;
    let track = if ui.visuals().dark_mode {
        Color32::from_rgb(48, 52, 61)
    } else {
        Color32::from_rgb(226, 230, 236)
    };
    let n = (diameter * ui.ctx().pixels_per_point() * 1.5)
        .ceil()
        .clamp(64.0, 256.0) as usize;
    let point = |t: f32, radius: f32| {
        let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * t;
        c + radius * egui::vec2(a.cos(), a.sin())
    };
    // Fine scale marks and a quiet inner rim add depth without obscuring data.
    for i in 0..20 {
        let t = i as f32 / 20.0;
        let outer = diameter / 2.0 - 2.5;
        painter.line_segment(
            [point(t, outer - 1.5), point(t, outer)],
            Stroke::new(1.0, track),
        );
    }
    painter.add(Shape::closed_line(
        (0..n).map(|i| point(i as f32 / n as f32, r)).collect(),
        Stroke::new(thickness, track),
    ));
    let inner = r - thickness * 0.90;
    painter.circle_filled(c, inner, color.gamma_multiply(0.035));
    painter.add(Shape::closed_line(
        (0..n).map(|i| point(i as f32 / n as f32, inner)).collect(),
        Stroke::new(0.7, color.gamma_multiply(0.14)),
    ));
    let frac = if percent.is_finite() {
        (percent / 100.0).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if frac > 0.0 {
        let steps = (n as f32 * frac).ceil().max(2.0) as usize;
        let points: Vec<Pos2> = (0..=steps)
            .map(|i| point(frac * i as f32 / steps as f32, r))
            .collect();
        // A spatial tint and round end caps, with no timers or idle animation.
        let tint = move |_: Rect, p: Pos2| {
            let t = ((p.y - c.y) / (2.0 * r) + 0.5).clamp(0.0, 1.0);
            color.gamma_multiply(1.0 - 0.25 * t)
        };
        if frac < 1.0 {
            painter.add(Shape::line(
                points,
                epaint::PathStroke::new_uv(thickness, tint),
            ));
            for end in [point(0.0, r), point(frac, r)] {
                painter.circle_filled(end, thickness / 2.0, tint(rect, end));
            }
        } else {
            // Close the loop explicitly: no double cap or seam at 100%.
            painter.add(Shape::closed_line(
                (0..n).map(|i| point(i as f32 / n as f32, r)).collect(),
                epaint::PathStroke::new_uv(thickness, tint),
            ));
        }
    }
    painter.text(
        c,
        egui::Align2::CENTER_CENTER,
        center,
        egui::FontId::proportional(diameter * 0.24),
        ui.visuals().strong_text_color(),
    );
    response
}

fn scroll_key(key: &str) -> egui::Id {
    egui::Id::unique(("more_below", key))
}

/// The offset a click on [`more_below`] asked for, for the next
/// `vertical_scroll_offset` of the scroll area with the same `key`.
pub fn take_scroll(ui: &Ui, key: &str) -> Option<f32> {
    ui.data_mut(|d| d.remove_temp::<f32>(scroll_key(key)))
}

/// While a scroll area's content goes on below its bottom edge: fades that
/// edge into `fill` and shows a *More below* button that scrolls most of a
/// view further (see [`take_scroll`]).
pub fn more_below<R>(
    ui: &Ui,
    key: &str,
    out: &egui::scroll_area::ScrollAreaOutput<R>,
    fill: Color32,
) {
    let view = out.inner_rect;
    let max_offset = out.content_size.y - view.height();
    let offset = out.state.clamped_offset().y;
    if max_offset - offset < 8.0 || view.height() < 80.0 {
        return;
    }
    let painter = ui.painter().with_clip_rect(view);

    let fade = Rect::from_min_max(egui::pos2(view.left(), view.bottom() - 36.0), view.max);
    let mut mesh = epaint::Mesh::default();
    let clear = fill.gamma_multiply(0.0);
    for (pos, color) in [
        (fade.left_top(), clear),
        (fade.right_top(), clear),
        (fade.right_bottom(), fill),
        (fade.left_bottom(), fill),
    ] {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));

    let label = crate::i18n::tr("More below");
    let galley = painter.layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(13.0),
        Color32::WHITE,
    );
    let size = galley.size() + egui::vec2(10.0, 0.0);
    let rect = Rect::from_center_size(
        egui::pos2(view.center().x, view.bottom() - size.y / 2.0),
        size,
    );
    let response = ui
        .interact(rect, scroll_key(key).with("button"), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let color = if response.hovered() {
        ACCENT.gamma_multiply(0.85)
    } else {
        ACCENT
    };
    painter.rect_filled(rect, 0.0, color);
    // A chevron pointing down, then the text.
    let c = egui::pos2(rect.left() + 5.0, rect.center().y);
    painter.add(Shape::line(
        vec![
            c + egui::vec2(-4.0, -2.0),
            c + egui::vec2(0.0, 2.0),
            c + egui::vec2(4.0, -2.0),
        ],
        Stroke::new(1.6, Color32::WHITE),
    ));
    painter.galley(
        egui::pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0),
        galley,
        Color32::WHITE,
    );
    if response.clicked() {
        let target = (offset + view.height() * 0.85).min(max_offset);
        ui.data_mut(|d| d.insert_temp(scroll_key(key), target));
        ui.ctx().request_repaint();
    }
}

/// Background of a [`card`].
pub fn card_fill(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(25, 30, 37)
    } else {
        Color32::WHITE
    }
}

/// A square, directly operable on/off control with an accessible state.
pub fn switch(ui: &mut Ui, value: &mut bool, label: &str) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(40.0, 22.0), egui::Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), *value, label)
    });
    let visuals = ui.style().interact_selectable(&response, *value);
    ui.painter().rect(
        rect.shrink(1.0),
        0.0,
        if *value { ACCENT } else { visuals.bg_fill },
        visuals.bg_stroke,
        egui::StrokeKind::Inside,
    );
    let x = if *value {
        rect.right() - 18.0
    } else {
        rect.left() + 3.0
    };
    ui.painter().rect_filled(
        egui::Rect::from_min_size(egui::pos2(x, rect.top() + 4.0), egui::vec2(15.0, 14.0)),
        0.0,
        if *value {
            Color32::WHITE
        } else {
            visuals.fg_stroke.color
        },
    );
    response.on_hover_text(label)
}

/// Keep long settings readable on wide displays without shrinking compact ones.
pub fn reading_column<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let available = ui.available_width();
    let width = available.min(980.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space((available - width) * 0.5);
        ui.vertical(|ui| {
            ui.set_width(width);
            add(ui)
        })
        .inner
    })
    .inner
}

/// A readable settings row: aligned controls on wide screens, stacked on narrow ones.
pub fn setting_row(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui, egui::Id)) {
    let width = ui.available_width();
    if width >= 620.0 {
        let controls = (width * 0.45).clamp(220.0, 360.0);
        ui.horizontal_top(|ui| {
            let label = ui
                .allocate_ui(egui::vec2((width - controls - 16.0).max(0.0), 28.0), |ui| {
                    ui.set_width((width - controls - 16.0).max(0.0));
                    ui.label(title)
                })
                .inner;
            ui.add_space(4.0);
            ui.allocate_ui(egui::vec2(controls, 28.0), |ui| add(ui, label.id));
        });
    } else {
        let label = ui.label(title);
        add(ui, label.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn settings_controls_align_and_stack_without_overflow() {
        for width in [960.0, 420.0] {
            let mut h = egui_kittest::Harness::builder()
                .with_size([width, 500.0])
                .build_ui(|ui| {
                    setting_row(ui, "Short title", |ui, _| {
                        let _ = ui.button("First control");
                    });
                    setting_row(ui, "A considerably longer setting title", |ui, _| {
                        let _ = ui.button("Second control");
                    });
                });
            h.run();
            let first = h.get_by_label("First control").rect();
            let second = h.get_by_label("Second control").rect();
            assert!(first.max.x <= width && second.max.x <= width);
            if width > 620.0 {
                assert!(
                    (first.min.x - second.min.x).abs() < 1.0,
                    "controls share a column: {first:?}, {second:?}"
                );
            } else {
                assert!(first.min.y >= h.get_by_label("Short title").rect().max.y);
                assert!(
                    second.min.y
                        >= h.get_by_label("A considerably longer setting title")
                            .rect()
                            .max
                            .y
                );
            }
        }
    }

    #[test]
    fn both_themes_keep_surfaces_square_and_adjoining() {
        let ctx = egui::Context::default();
        apply(&ctx);
        for theme in [Theme::Light, Theme::Dark] {
            let s = ctx.style_of(theme);
            assert_eq!(s.spacing.item_spacing, CONTENT_SPACING);
            assert_eq!(s.spacing.button_padding, CONTROL_PADDING);
            assert_eq!(s.spacing.window_margin, CONTENT_PADDING);
            assert_eq!(s.spacing.menu_margin, Margin::same(6));
            assert_eq!(s.spacing.scroll.bar_inner_margin, 0.0);
            assert_eq!(s.spacing.scroll.bar_outer_margin, 0.0);
            assert_eq!(s.visuals.window_corner_radius, CornerRadius::ZERO);
            assert_eq!(s.visuals.menu_corner_radius, CornerRadius::ZERO);
            assert_eq!(s.visuals.window_shadow, epaint::Shadow::NONE);
            assert_eq!(s.visuals.popup_shadow, epaint::Shadow::NONE);
            for w in [
                &s.visuals.widgets.noninteractive,
                &s.visuals.widgets.inactive,
                &s.visuals.widgets.hovered,
                &s.visuals.widgets.active,
                &s.visuals.widgets.open,
            ] {
                assert_eq!(w.corner_radius, CornerRadius::ZERO);
            }
            assert_eq!(s.visuals.widgets.inactive.bg_stroke.width, 1.0);
        }
    }

    #[test]
    fn gauges_keep_all_painted_geometry_inside_their_allocation() {
        for (diameter, dpi, theme, percent) in [48.0, 60.0, 72.0, 100.0]
            .into_iter()
            .flat_map(|d| [1.0, 1.25, 1.5, 2.0].into_iter().map(move |dpi| (d, dpi)))
            .flat_map(|(d, dpi)| {
                [Theme::Light, Theme::Dark]
                    .into_iter()
                    .map(move |theme| (d, dpi, theme))
            })
            .flat_map(|(d, dpi, theme)| {
                [0.0, 1.0, 42.0, 99.0, 100.0]
                    .into_iter()
                    .map(move |p| (d, dpi, theme, p))
            })
        {
            let mut h = egui_kittest::Harness::builder()
                .with_size([200.0, 180.0])
                .with_pixels_per_point(dpi)
                .with_theme(theme)
                .build_ui_state(
                    move |ui, rect: &mut Rect| {
                        *rect =
                            ring_gauge(ui, diameter, percent, CPU, &format!("{percent:.0}%")).rect;
                    },
                    Rect::NOTHING,
                );
            h.run();
            let rect = *h.state();
            let shapes = h
                .output()
                .shapes
                .iter()
                .filter(|s| s.clip_rect == rect)
                .cloned()
                .collect();
            let primitives = h.ctx.tessellate(shapes, h.ctx.pixels_per_point());
            assert!(!primitives.is_empty());
            for primitive in primitives {
                if let epaint::Primitive::Mesh(mesh) = primitive.primitive {
                    for vertex in mesh.vertices {
                        assert!(
                            rect.expand(0.05).contains(vertex.pos),
                            "{diameter}px/{dpi}x/{percent}% gauge is clipped: {:?} outside {rect:?}",
                            vertex.pos
                        );
                    }
                }
            }
        }
    }

    #[cfg(feature = "visual-tests")]
    #[test]
    fn render_gauge_review() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/visual-review/gauges");
        std::fs::create_dir_all(&root).unwrap();
        for (name, theme) in [("dark", Theme::Dark), ("light", Theme::Light)] {
            for dpi in [1.0, 1.25, 1.5, 2.0] {
                let mut h = egui_kittest::Harness::builder()
                    .with_size([880.0, 420.0])
                    .with_pixels_per_point(dpi)
                    .with_theme(theme)
                    .build_ui(move |ui| {
                        apply(ui.ctx());
                        ui.ctx().set_theme(theme);
                        ui.heading("Resource gauges");
                        ui.label("Native vector rendering · 0–100% · DPI scaling");
                        ui.add_space(0.0);
                        for diameter in [48.0, 72.0, 100.0] {
                            ui.horizontal(|ui| {
                                ui.set_height(diameter + 12.0);
                                ui.label(format!("{diameter:.0} pt"));
                                for (percent, accent) in [
                                    (0.0, CPU),
                                    (1.0, CPU),
                                    (42.0, CPU),
                                    (50.0, MEMORY),
                                    (70.0, WARN),
                                    (99.0, BAD),
                                    (100.0, GPU),
                                ] {
                                    ring_gauge(
                                        ui,
                                        diameter,
                                        percent,
                                        accent,
                                        &format!("{percent:.0}%"),
                                    );
                                    ui.add_space(0.0);
                                }
                            });
                        }
                    });
                h.run();
                h.render()
                    .expect("GPU render")
                    .save(root.join(format!("{name}-{dpi}x.png")))
                    .unwrap();
            }
        }
    }

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
    fn more_below_shows_until_the_end_and_scrolls() {
        let mut h = egui_kittest::Harness::builder()
            .allow_missing_glyphs()
            .with_size([400.0, 300.0])
            .build_ui(|ui| {
                let mut area = egui::ScrollArea::vertical()
                    .id_salt("list")
                    .auto_shrink([false, false]);
                if let Some(y) = take_scroll(ui, "list") {
                    area = area.vertical_scroll_offset(y);
                }
                let out = area.show(ui, |ui| {
                    for i in 0..60 {
                        ui.label(format!("row {i}"));
                    }
                });
                ui.data_mut(|d| {
                    d.insert_temp(egui::Id::unique("offset"), out.state.clamped_offset().y)
                });
                more_below(ui, "list", &out, ui.visuals().panel_fill);
            });
        h.run();
        let offset = |h: &egui_kittest::Harness<'_>| {
            h.ctx
                .data(|d| d.get_temp::<f32>(egui::Id::unique("offset")))
                .unwrap()
        };
        assert_eq!(offset(&h), 0.0);
        for _ in 0..10 {
            if h.query_by_label("More below").is_none() {
                break;
            }
            let before = offset(&h);
            h.get_by_label("More below").click();
            h.run();
            assert!(offset(&h) > before, "a click scrolls down");
        }
        assert!(h.query_by_label("More below").is_none(), "gone at the end");
        assert!(h.query_by_label("row 59").is_some());
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

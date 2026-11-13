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

/// The Windows font (file and face index) for Han and Hangul text in the
/// current language. Chinese, Japanese and Korean draw shared characters
/// differently, so the on-demand system fallback (which follows the Windows
/// locale) is not enough. The "UI" faces share Segoe UI's vertical metrics,
/// so mixed Latin and East Asian text stays on one baseline. Only this one
/// font is loaded; other scripts use the system fallback.
fn east_asian_font() -> Option<(&'static str, u32)> {
    use crate::i18n::Language;
    match Language::current() {
        Language::ChineseSimplified => Some(("msyh.ttc", 1)), // Microsoft YaHei UI
        Language::ChineseTraditional => Some(("msjh.ttc", 1)), // Microsoft JhengHei UI
        Language::Japanese => Some(("YuGothM.ttc", 1)),       // Yu Gothic UI
        Language::Korean => Some(("malgun.ttf", 0)),          // Malgun Gothic
        _ => None,
    }
}

/// Use the installed Windows UI font, with Symbol as a fallback for arrows,
/// status marks and controls. Fall back to egui's bundled fonts if unavailable.
/// Fonts stay on the machine; none are copied into the distribution.
/// Call again after the language changes.
pub fn install_fonts(ctx: &egui::Context) {
    let Some(windows) = std::env::var_os("WINDIR") else {
        return;
    };
    let dir = std::path::Path::new(&windows).join("Fonts");
    let mut fonts = egui::FontDefinitions::default();
    let east_asian =
        east_asian_font().map(|(file, index)| ("Windows East Asian", file, index, false));
    for (name, file, index, primary) in [
        Some(("Windows UI", "segoeui.ttf", 0, true)),
        east_asian,
        Some(("Windows symbols", "seguisym.ttf", 0, false)),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(bytes) = std::fs::read(dir.join(file)) {
            let mut data = egui::FontData::from_owned(bytes);
            data.index = index;
            fonts
                .font_data
                .insert(name.into(), std::sync::Arc::new(data));
            for (family, names) in &mut fonts.families {
                if primary && *family == egui::FontFamily::Proportional {
                    names.insert(0, name.into());
                } else if !primary {
                    names.push(name.into());
                }
            }
        }
    }
    // Windows' own icon font (Segoe Fluent Icons, or Segoe MDL2 Assets on
    // Windows 10) in a family of its own: its glyphs sit in the private-use
    // area and must never stand in for text.
    for file in ["SegoeIcons.ttf", "segmdl2.ttf"] {
        if let Ok(bytes) = std::fs::read(dir.join(file)) {
            fonts.font_data.insert(
                ICON_FONT.into(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts.families.insert(
                egui::FontFamily::Name(ICON_FONT.into()),
                vec![ICON_FONT.into()],
            );
            break;
        }
    }
    ctx.set_fonts(fonts);
}

const ICON_FONT: &str = "Windows icons";

/// Glyphs of the Windows icon font used by option rows.
pub mod icon {
    pub const GLOBE: char = '\u{E774}';
    pub const COLOR: char = '\u{E790}';
    pub const WARNING: char = '\u{E7BA}';
    pub const SYNC: char = '\u{E895}';
    pub const ADMIN: char = '\u{E7EF}';
    pub const PLAY: char = '\u{E768}';
    pub const HISTORY: char = '\u{E81C}';
    pub const CHART: char = '\u{E9D2}';
    pub const MONITOR: char = '\u{E7F4}';
    pub const SETTINGS: char = '\u{E713}';
    pub const LIST: char = '\u{EA37}';
    pub const VIEW: char = '\u{E890}';
    pub const FILTER: char = '\u{E71C}';
    pub const ZOOM: char = '\u{E71E}';
    pub const MINIMIZE: char = '\u{E921}';
    pub const PEOPLE: char = '\u{E716}';
    pub const PREVIOUS: char = '\u{E892}';
    pub const NEXT: char = '\u{E893}';
    pub const ZOOM_IN: char = '\u{E8A3}';
    pub const ZOOM_OUT: char = '\u{E71F}';
    pub const RESET: char = '\u{E72C}';
    pub const KEY: char = '\u{E8D7}';
    pub const CHAT: char = '\u{E8BD}';
    pub const BELL: char = '\u{EA8F}';
    pub const POWER: char = '\u{E7E8}';
    pub const CODE: char = '\u{E943}';
    pub const LINK: char = '\u{E71B}';
    pub const SPEED: char = '\u{EC4A}';
    pub const MEMORY: char = '\u{EEA0}';
    pub const GAME: char = '\u{E7FC}';
}

/// A small button showing `glyph` from the Windows icon font, named and
/// explained by `label` (its text where that font is missing).
pub fn icon_button(ui: &mut Ui, enabled: bool, glyph: char, label: &str) -> egui::Response {
    let family = egui::FontFamily::Name(ICON_FONT.into());
    let text = if ui.fonts(|f| f.families().contains(&family)) {
        RichText::new(glyph.to_string()).font(egui::FontId::new(14.0, family))
    } else {
        RichText::new(label)
    };
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(text).min_size(egui::vec2(30.0, ui.spacing().interact_size.y)),
    );
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, enabled, label));
    response.on_hover_text(label).on_disabled_hover_text(label)
}

/// Draws `glyph` from the Windows icon font, or leaves the same space empty
/// where that font is missing.
pub fn icon_glyph(ui: &mut Ui, glyph: char, size: f32) {
    let family = egui::FontFamily::Name(ICON_FONT.into());
    let available = ui.fonts(|f| f.families().contains(&family));
    if available {
        ui.add_sized(
            [size + 8.0, size + 4.0],
            egui::Label::new(
                RichText::new(glyph.to_string())
                    .font(egui::FontId::new(size, family))
                    .color(ui.visuals().text_color()),
            )
            .selectable(false),
        );
    } else {
        ui.add_space(size + 8.0);
    }
}

/// Quiet layered surfaces with rounded controls, for both themes: content
/// sits on soft cards instead of being fenced in by lines.
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
            style.visuals.window_shadow = flyout_shadow(dark);
            style.visuals.popup_shadow = flyout_shadow(dark);
            style.spacing.scroll.bar_inner_margin = 0.0;
            style.spacing.scroll.bar_outer_margin = 0.0;
            style.spacing.interact_size.y = 28.0;
            let v = &mut style.visuals;
            for w in [
                &mut v.widgets.noninteractive,
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.open,
            ] {
                w.corner_radius = CONTROL_RADIUS;
            }
            v.window_corner_radius = SURFACE_RADIUS;
            v.menu_corner_radius = SURFACE_RADIUS;
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
            if dark {
                v.panel_fill = Color32::from_rgb(32, 32, 32);
                v.window_fill = Color32::from_rgb(44, 44, 44);
                v.extreme_bg_color = Color32::from_rgb(28, 28, 28);
                v.faint_bg_color = Color32::from_rgb(50, 50, 50);
                v.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(222, 222, 222);
                v.widgets.noninteractive.bg_stroke.color = Color32::from_rgb(58, 58, 58);
                v.weak_text_color = Some(Color32::from_rgb(170, 170, 170));
            } else {
                v.panel_fill = Color32::from_rgb(243, 243, 243);
                v.window_fill = Color32::from_rgb(251, 251, 251);
                v.extreme_bg_color = Color32::WHITE;
                v.faint_bg_color = Color32::from_rgb(245, 245, 245);
                v.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(36, 36, 36);
                v.widgets.noninteractive.bg_stroke.color = Color32::from_rgb(229, 229, 229);
                v.weak_text_color = Some(Color32::from_rgb(96, 96, 96));
            }
            // Explicit widget colours prevent egui's default grey blocks from
            // breaking the surface hierarchy. Hover/focus remain distinct.
            let ink = if dark {
                Color32::from_rgb(232, 232, 232)
            } else {
                Color32::from_rgb(28, 28, 28)
            };
            let border = control_border(dark);
            let resting = if dark {
                Color32::from_rgb(55, 55, 55)
            } else {
                Color32::from_rgb(253, 253, 253)
            };
            let hover = if dark {
                Color32::from_rgb(62, 62, 62)
            } else {
                Color32::from_rgb(246, 246, 246)
            };
            let active = if dark {
                Color32::from_rgb(50, 50, 50)
            } else {
                Color32::from_rgb(240, 240, 240)
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
                Color32::from_rgb(72, 72, 72)
            } else {
                Color32::from_rgb(224, 224, 224)
            };
            v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
            v.widgets.noninteractive.bg_fill = v.window_fill;
            v.widgets.noninteractive.weak_bg_fill = v.window_fill;
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border);
            v.text_edit_bg_color = Some(if dark {
                Color32::from_rgb(40, 40, 40)
            } else {
                Color32::WHITE
            });
        });
    }
    geometry(ctx);
}

/// The hairline around controls and selected choices.
fn control_border(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(64, 64, 64)
    } else {
        Color32::from_rgb(222, 222, 222)
    }
}

/// A soft drop shadow lifts menus and dialogs off the page without a frame.
fn flyout_shadow(dark: bool) -> epaint::Shadow {
    epaint::Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 90 } else { 28 }),
    }
}

/// Background of text fields on light surfaces.
const FIELD_LIGHT: Color32 = Color32::from_rgb(241, 243, 247);

/// Size of card titles and section labels.
const TITLE_SIZE: f32 = 16.0;
/// Rounding of buttons, fields and other controls.
pub const CONTROL_RADIUS: CornerRadius = CornerRadius::same(4);
/// Rounding of cards, panels, menus and dialogs.
pub const SURFACE_RADIUS: CornerRadius = CornerRadius::same(8);

/// Outer surfaces join at their borders; contents have their own breathing room.
pub const CONTENT_PADDING: Margin = Margin::symmetric(12, 10);
/// Page gutters and section gaps are separate from control/cell padding.
pub const PAGE_PADDING: Margin = Margin::symmetric(16, 12);
pub const SECTION_SPACING: Vec2 = egui::vec2(10.0, 10.0);
pub const CONTROL_PADDING: Vec2 = egui::vec2(8.0, 4.0);
pub const CONTENT_SPACING: Vec2 = egui::vec2(8.0, 6.0);

/// Compact windows preserve room for controls and selected details.
pub fn page_padding(ui: &Ui) -> Margin {
    if ui.ctx().content_rect().height() < 640.0 {
        Margin::symmetric(12, 8)
    } else {
        PAGE_PADDING
    }
}

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
    let lines = TableLines::load(ui);
    if !lines.rows && !lines.columns {
        return;
    }
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
    if lines.columns {
        ui.painter()
            .line_segment([rect.right_top(), rect.right_bottom()], stroke);
    }
    if lines.rows {
        ui.painter()
            .line_segment([rect.left_bottom(), rect.right_bottom()], stroke);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TableLines {
    pub rows: bool,
    pub columns: bool,
}

/// Lists read cleanly without a grid: rows are told apart by hover and
/// selection. Separators stay available as a preference, and a choice saved
/// earlier is kept.
impl Default for TableLines {
    fn default() -> Self {
        Self {
            rows: false,
            columns: false,
        }
    }
}

impl TableLines {
    pub fn load(ui: &Ui) -> Self {
        Self::load_context(ui.ctx())
    }

    pub fn load_context(ctx: &egui::Context) -> Self {
        ctx.data_mut(|d| d.get_persisted(egui::Id::unique("workspace-table-lines")))
            .unwrap_or_default()
    }

    pub fn store(self, ui: &Ui) {
        self.store_context(ui.ctx());
    }

    pub fn store_context(self, ctx: &egui::Context) {
        ctx.data_mut(|d| d.insert_persisted(egui::Id::unique("workspace-table-lines"), self));
    }

    /// The two separator choices as option rows, for a settings page.
    pub fn options(ui: &mut Ui) {
        let mut lines = Self::load(ui);
        let rows = option_switch(
            ui,
            icon::LIST,
            crate::i18n::tr("Row separators"),
            "",
            &mut lines.rows,
        )
        .changed();
        let columns = option_switch(
            ui,
            icon::LIST,
            crate::i18n::tr("Column separators"),
            "",
            &mut lines.columns,
        )
        .changed();
        if rows || columns {
            lines.store(ui);
        }
    }

    pub fn controls(ui: &mut Ui) {
        let mut lines = Self::load(ui);
        let rows = ui
            .checkbox(&mut lines.rows, crate::i18n::tr("Row separators"))
            .changed();
        let columns = ui
            .checkbox(&mut lines.columns, crate::i18n::tr("Column separators"))
            .changed();
        if rows || columns {
            lines.store(ui);
        }
    }
}

/// Table cells touch at their boundaries, independently of form/control spacing.
pub fn table(ui: &mut Ui) -> egui_extras::TableBuilder<'_> {
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    if !TableLines::load(ui).columns {
        // Retain resize hit targets and the hover/drag indicator.
        ui.visuals_mut().widgets.noninteractive.bg_stroke.width = 0.0;
    }
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
        .inner_margin(Margin::symmetric(8, 0))
        .show(ui, |ui| {
            if !ui.is_sizing_pass() {
                ui.set_width((size.x - 16.0).max(0.0));
            }
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

/// A named row action with the table's density and full-cell hit area.
pub fn table_name(ui: &mut Ui, name: &str, selected: bool) -> egui::Response {
    let response = ui.add_sized(
        [ui.available_width(), ui.spacing().interact_size.y],
        egui::Button::new("")
            .left_text(name)
            .frame(false)
            .truncate(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, name)
    });
    response.on_hover_text(name)
}

/// Forms use their grid alignment; do not draw a box around every field row.
pub fn end_row(ui: &mut Ui) {
    ui.end_row();
}

pub fn workspace<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(page_padding(ui))
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
    let dark = ui.visuals().dark_mode;
    let mut response = ui.add(
        egui::Button::new(label)
            .selected(selected)
            .fill(if selected {
                chip_fill(dark)
            } else {
                Color32::TRANSPARENT
            })
            .stroke(if selected {
                Stroke::new(1.0, control_border(dark))
            } else {
                Stroke::NONE
            }),
    );
    // A short accent mark under the selected choice; focus shows the same
    // mark, so keyboard users see where they are.
    if selected || response.has_focus() {
        let rect = response.rect;
        let mark = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.bottom() - 2.0),
            egui::vec2((rect.width() * 0.35).clamp(10.0, 24.0), 3.0),
        );
        ui.painter()
            .rect_filled(mark, CornerRadius::same(2), ACCENT);
    }
    if response.clicked() && !selected {
        *current = value;
        response.mark_changed();
    }
    response
}

/// The raised chip of a selected segmented choice.
pub fn chip_fill(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(64, 64, 64)
    } else {
        Color32::WHITE
    }
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
pub fn geometry(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = CONTENT_SPACING;
        style.spacing.button_padding = CONTROL_PADDING;
        style.spacing.window_margin = CONTENT_PADDING;
        style.spacing.menu_margin = Margin::same(6);
        style.spacing.icon_spacing = 6.0;
        style.spacing.scroll.bar_inner_margin = 0.0;
        style.spacing.scroll.bar_outer_margin = 0.0;
        let dark = style.visuals.dark_mode;
        style.visuals.window_shadow = flyout_shadow(dark);
        style.visuals.popup_shadow = flyout_shadow(dark);
        for widget in [
            &mut style.visuals.widgets.noninteractive,
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.open,
        ] {
            widget.corner_radius = CONTROL_RADIUS;
        }
        style.visuals.window_corner_radius = SURFACE_RADIUS;
        style.visuals.menu_corner_radius = SURFACE_RADIUS;
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
        if !job.text.is_empty() {
            ui.label(job);
            ui.add_space(2.0);
        }
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
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(SURFACE_RADIUS)
        .inner_margin(if ui.ctx().content_rect().height() < 640.0 {
            Margin::symmetric(10, 8)
        } else {
            CONTENT_PADDING
        })
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
    if ui.is_sizing_pass() {
        ui.label(text);
        return;
    }
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
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(SURFACE_RADIUS)
        .inner_margin(CONTENT_PADDING)
        .outer_margin(0)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = CONTENT_SPACING;
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A large working surface (a table, a list): the raised fill of a panel
/// without its outline, so the content itself draws the only lines.
pub fn work_surface<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(card_fill(ui))
        .corner_radius(SURFACE_RADIUS)
        .inner_margin(CONTENT_PADDING)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = CONTENT_SPACING;
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// The frame and grid of a chart occupying `rect`, tinted with the colour
/// of its data. Charts draw no tick labels; [`chart_scale`] names the range.
pub fn chart_backdrop(ui: &Ui, rect: egui::Rect, color: Color32, grid: bool) {
    let painter = ui.painter_at(rect.expand(1.0));
    if grid {
        let stroke = Stroke::new(1.0, color.gamma_multiply(0.13));
        for i in 1..6 {
            let x = egui::lerp(rect.left()..=rect.right(), i as f32 / 6.0).round() + 0.5;
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                stroke,
            );
        }
        for i in 1..4 {
            let y = egui::lerp(rect.top()..=rect.bottom(), i as f32 / 4.0).round() + 0.5;
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                stroke,
            );
        }
    }
    painter.rect_stroke(
        rect,
        CornerRadius::same(4),
        Stroke::new(1.0, color.gamma_multiply(0.45)),
        egui::StrokeKind::Inside,
    );
}

/// A line of small text above or below a chart: `left` and `right` at its
/// two ends, e.g. a title and the top of the scale, or the time span.
pub fn chart_scale(ui: &mut Ui, left: &str, right: &str) {
    ui.horizontal(|ui| {
        ui.set_min_height(14.0);
        ui.label(RichText::new(left).small().weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(right).small().weak());
        });
    });
}

/// The top of a byte-rate scale: the smallest 1-2-5 step (in binary units,
/// from 1 KiB/s) that holds `peak`, so the scale moves in calm jumps.
pub fn rate_ceiling(peak: f64) -> f64 {
    let mut unit = 1024.0;
    loop {
        for step in [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0] {
            let ceiling = unit * step;
            if peak.is_nan() || peak <= ceiling {
                return ceiling;
            }
        }
        unit *= 1024.0;
        if unit > 1024f64.powi(6) {
            return peak;
        }
    }
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
        Color32::from_rgb(43, 43, 43)
    } else {
        Color32::from_rgb(251, 251, 251)
    }
}

/// A rounded on/off switch with an accessible state; the thumb slides
/// unless motion is reduced.
pub fn switch(ui: &mut Ui, value: &mut bool, label: &str) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(40.0, 20.0), egui::Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), *value, label)
    });
    let on = ui.ctx().animate_bool_responsive(response.id, *value);
    let dark = ui.visuals().dark_mode;
    let enabled = ui.is_enabled();
    let track = rect.shrink(0.5);
    let radius = CornerRadius::same((track.height() / 2.0) as u8);
    let off = if dark {
        Color32::from_rgb(160, 160, 160)
    } else {
        Color32::from_rgb(110, 110, 110)
    };
    if *value {
        let fill = if enabled {
            ACCENT
        } else {
            ACCENT.gamma_multiply(0.45)
        };
        ui.painter().rect_filled(track, radius, fill);
    } else {
        let fill = if response.hovered() {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect(
            track,
            radius,
            fill,
            Stroke::new(1.0, off),
            egui::StrokeKind::Inside,
        );
    }
    let thumb = if response.hovered() && enabled {
        6.0
    } else {
        5.0
    };
    let x = egui::lerp((track.left() + 10.0)..=(track.right() - 10.0), on);
    ui.painter().circle_filled(
        egui::pos2(x, track.center().y),
        thumb,
        if *value { Color32::WHITE } else { off },
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            CornerRadius::same((rect.height() / 2.0 + 2.0) as u8),
            Stroke::new(1.5, ui.visuals().strong_text_color()),
            egui::StrokeKind::Outside,
        );
    }
    response.on_hover_text(label)
}

/// A heading above a group of option rows.
pub fn option_group(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).size(15.0).strong());
}

/// One setting on its own card: an icon, the title and an optional muted
/// description on the left, the control on the right. Below 560 points the
/// control moves under the text.
pub fn option<R>(
    ui: &mut Ui,
    glyph: char,
    title: &str,
    description: &str,
    control: impl FnOnce(&mut Ui) -> R,
) -> R {
    option_row(ui, glyph, title, description, true, |ui, _| control(ui))
}

/// An [`option`] whose control is named by the row's title: `control`
/// receives the title's id for `labelled_by` (a slider, a number field).
pub fn option_labelled<R>(
    ui: &mut Ui,
    glyph: char,
    title: &str,
    description: &str,
    control: impl FnOnce(&mut Ui, egui::Id) -> R,
) -> R {
    option_row(ui, glyph, title, description, true, control)
}

/// An [`option`] whose control already carries `title` as its accessible
/// name (a switch, a labelled list): the title is only drawn, so assistive
/// technology does not announce it twice.
pub fn option_named_by_control<R>(
    ui: &mut Ui,
    glyph: char,
    title: &str,
    description: &str,
    control: impl FnOnce(&mut Ui) -> R,
) -> R {
    option_row(ui, glyph, title, description, false, |ui, _| control(ui))
}

fn option_row<R>(
    ui: &mut Ui,
    glyph: char,
    title: &str,
    description: &str,
    accessible_title: bool,
    add: impl FnOnce(&mut Ui, egui::Id) -> R,
) -> R {
    egui::Frame::new()
        .fill(card_fill(ui))
        .stroke(Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // The title's id (for `labelled_by`) and the text's height.
            let text = |ui: &mut Ui| {
                let shown = ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let id = if accessible_title {
                        ui.label(title).id
                    } else {
                        drawn_text(ui, title);
                        egui::Id::NULL
                    };
                    if !description.is_empty() {
                        ui.add(egui::Label::new(RichText::new(description).small().weak()).wrap());
                    }
                    id
                });
                (shown.inner, shown.response.rect.height())
            };
            if ui.available_width() < 560.0 {
                let (label, _) = ui
                    .horizontal_top(|ui| {
                        icon_glyph(ui, glyph, 16.0);
                        text(ui)
                    })
                    .inner;
                return ui.horizontal_wrapped(|ui| add(ui, label)).inner;
            }
            // The control's size from the previous frame keeps it flush
            // right with its pieces in reading order. egui does not centre
            // nested layouts in a row, so the icon, the text and the control
            // are each padded to the row's height, also from the last frame.
            let size_id = ui.make_persistent_id(("option-control", title));
            let text_id = size_id.with("text");
            let control = ui
                .data(|d| d.get_temp::<egui::Vec2>(size_id))
                .unwrap_or(egui::vec2(180.0, 0.0));
            let text_height = ui.data(|d| d.get_temp::<f32>(text_id)).unwrap_or(0.0);
            let row = control.y.max(text_height);
            let centre = |ui: &mut Ui, height: f32| ui.add_space(((row - height) / 2.0).max(0.0));
            ui.horizontal_top(|ui| {
                let text_width = (ui.available_width() - control.x - 16.0).max(120.0);
                let (label, measured_text) = ui
                    .allocate_ui_with_layout(
                        egui::vec2(text_width, row),
                        egui::Layout::left_to_right(egui::Align::Min),
                        |ui| {
                            ui.vertical(|ui| {
                                centre(ui, 20.0);
                                icon_glyph(ui, glyph, 16.0);
                            });
                            ui.vertical(|ui| {
                                centre(ui, text_height);
                                text(ui)
                            })
                            .inner
                        },
                    )
                    .inner;
                ui.add_space((ui.available_width() - control.x).max(0.0));
                let shown = ui.vertical(|ui| {
                    centre(ui, control.y);
                    ui.horizontal(|ui| add(ui, label))
                });
                let measured = shown.inner.response.rect.size();
                if (measured - control).length() > 0.5 || (measured_text - text_height).abs() > 0.5
                {
                    ui.data_mut(|d| {
                        d.insert_temp(size_id, measured);
                        d.insert_temp(text_id, measured_text);
                    });
                    ui.ctx().request_repaint();
                }
                shown.inner.inner
            })
            .inner
        })
        .inner
}

/// Body text painted without an accessibility node of its own.
fn drawn_text(ui: &mut Ui, text: &str) {
    let galley = ui.painter().layout(
        text.to_owned(),
        egui::TextStyle::Body.resolve(ui.style()),
        ui.visuals().text_color(),
        ui.available_width(),
    );
    let (rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    ui.painter()
        .galley(rect.min, galley, ui.visuals().text_color());
}

/// An option that is on or off: a switch with its state written beside it.
pub fn option_switch(
    ui: &mut Ui,
    glyph: char,
    title: &str,
    description: &str,
    value: &mut bool,
) -> egui::Response {
    option_named_by_control(ui, glyph, title, description, |ui| {
        ui.label(crate::i18n::tr(if *value { "On" } else { "Off" }));
        switch(ui, value, title)
    })
}

/// A readable settings row: aligned controls on wide screens, stacked on narrow ones.
pub fn setting_row(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui, egui::Id)) {
    ui.add_space(3.0);
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
    ui.add_space(6.0);
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
    fn option_titles_sit_level_with_their_controls() {
        let mut h = egui_kittest::Harness::builder()
            .allow_missing_glyphs()
            .with_size([900.0, 300.0])
            .build_ui(|ui| {
                option(ui, icon::GLOBE, "Single line", "", |ui| {
                    ui.add(egui::Button::new("Wide control").min_size(egui::vec2(160.0, 32.0)))
                });
            });
        h.run_steps(3);
        let title = h.get_by_label("Single line").rect().center().y;
        let control = h.get_by_label("Wide control").rect().center().y;
        assert!(
            (title - control).abs() <= 1.5,
            "title {title}, control {control}"
        );
    }

    #[test]
    fn rate_scales_move_in_calm_steps() {
        assert_eq!(rate_ceiling(0.0), 1024.0);
        assert_eq!(rate_ceiling(1024.0), 1024.0);
        assert_eq!(rate_ceiling(1500.0), 2048.0);
        assert_eq!(rate_ceiling(300.0 * 1024.0), 500.0 * 1024.0);
        assert_eq!(rate_ceiling(600.0 * 1024.0), 1024.0 * 1024.0);
        assert_eq!(rate_ceiling(f64::NAN), 1024.0);
    }

    #[test]
    fn both_themes_share_rounded_controls_and_soft_flyouts() {
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
            assert_eq!(s.visuals.window_corner_radius, SURFACE_RADIUS);
            assert_eq!(s.visuals.menu_corner_radius, SURFACE_RADIUS);
            assert_ne!(s.visuals.popup_shadow, epaint::Shadow::NONE);
            for w in [
                &s.visuals.widgets.noninteractive,
                &s.visuals.widgets.inactive,
                &s.visuals.widgets.hovered,
                &s.visuals.widgets.active,
                &s.visuals.widgets.open,
            ] {
                assert_eq!(w.corner_radius, CONTROL_RADIUS);
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

//! Local display preferences. These controls never send service commands.
use eframe::egui::{self, Ui};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Density {
    Compact,
    Comfortable,
    #[default]
    #[serde(other)]
    Standard,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Band {
    Compact,
    Full,
    #[default]
    #[serde(other)]
    Adaptive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Display {
    pub density: Density,
    pub scale: u16,
    pub reduced_motion: bool,
    pub band: Band,
}
impl Default for Display {
    fn default() -> Self {
        Self {
            density: Density::Standard,
            scale: 100,
            reduced_motion: false,
            band: Band::Adaptive,
        }
    }
}
impl Display {
    pub fn apply(&mut self, ctx: &egui::Context, applied: &mut Option<Self>) {
        if let Some(previous) = applied.as_ref()
            && self.scale == previous.scale
        {
            self.scale = (ctx.zoom_factor() * 100.0).round().clamp(75.0, 150.0) as u16;
        }
        self.scale = self.scale.clamp(75, 150);
        let zoom = f32::from(self.scale) / 100.0;
        if *applied != Some(*self) || (ctx.zoom_factor() - zoom).abs() > 0.001 {
            crate::theme::geometry(ctx);
            ctx.all_styles_mut(|style| {
                style.spacing.interact_size.y = match self.density {
                    Density::Compact => 22.0,
                    Density::Standard => 28.0,
                    Density::Comfortable => 34.0,
                };
                style.animation_time = if self.reduced_motion { 0.0 } else { 0.1 };
                style.scroll_animation = if self.reduced_motion {
                    egui::style::ScrollAnimation::none()
                } else {
                    egui::style::ScrollAnimation::default()
                };
            });
            ctx.set_zoom_factor(zoom);
            *applied = Some(*self);
        }
    }
}

pub fn appearance(ui: &mut Ui, display: &mut Display, nav: &mut Option<bool>) {
    use crate::theme::icon;
    crate::theme::option_group(ui, crate::i18n::tr("Display and accessibility"));
    ui.weak(crate::i18n::tr(
        "Changes apply immediately and are remembered for this window.",
    ));
    crate::theme::option(
        ui,
        icon::ZOOM,
        crate::i18n::tr("Interface scale"),
        "",
        |ui| {
            ui.add(egui::Slider::new(&mut display.scale, 75..=150).suffix("%"))
                .on_hover_text(crate::i18n::tr("Interface scale"));
        },
    );
    crate::theme::option(
        ui,
        icon::LIST,
        crate::i18n::tr("Control and row height"),
        "",
        |ui| {
            for (value, label) in [
                (Density::Compact, "Compact"),
                (Density::Standard, "Standard"),
                (Density::Comfortable, "Comfortable"),
            ] {
                crate::theme::choice(ui, &mut display.density, value, crate::i18n::tr(label));
            }
        },
    );
    crate::theme::option_switch(
        ui,
        icon::VIEW,
        crate::i18n::tr("Reduce interface animation"),
        "",
        &mut display.reduced_motion,
    );
    crate::theme::TableLines::options(ui);
    crate::theme::option_group(ui, crate::i18n::tr("Sidebar layout"));
    crate::theme::option(
        ui,
        icon::VIEW,
        crate::i18n::tr("Sidebar layout"),
        "",
        |ui| {
            for (value, label) in [
                (None, "Automatic"),
                (Some(false), "Expanded"),
                (Some(true), "Collapsed"),
            ] {
                crate::theme::choice(ui, nav, value, crate::i18n::tr(label));
            }
        },
    );
    if ui
        .button(crate::i18n::tr("Reset display preferences"))
        .clicked()
    {
        *display = Display::default();
        *nav = None;
        crate::theme::TableLines::default().store(ui);
    }
}

/// Returns true to navigate to the existing transactional column editor.
pub fn processes(
    ui: &mut Ui,
    display: &mut Display,
    strip: &mut bool,
    inspector: &mut crate::workspace_inspector::Mode,
    view: &mut crate::processes::ProcessView,
) -> bool {
    use crate::theme::{icon, option, option_group, option_switch};
    option_group(ui, crate::i18n::tr("Resource cards and inspector"));
    option_switch(
        ui,
        icon::MONITOR,
        crate::i18n::tr("Show resource cards above processes"),
        "",
        strip,
    );
    option(
        ui,
        icon::MONITOR,
        crate::i18n::tr("Resource card height"),
        "",
        |ui| {
            for (value, label) in [
                (Band::Adaptive, "Adaptive"),
                (Band::Compact, "Compact"),
                (Band::Full, "Full height"),
            ] {
                crate::theme::choice(ui, &mut display.band, value, crate::i18n::tr(label));
            }
        },
    );
    option(
        ui,
        icon::VIEW,
        crate::i18n::tr("Inspector"),
        crate::i18n::tr(
            "Adaptive mode shows system facts in wide windows and process details on selection.",
        ),
        |ui| {
            for (value, label) in [
                (crate::workspace_inspector::Mode::Auto, "Adaptive inspector"),
                (
                    crate::workspace_inspector::Mode::Always,
                    "Always show inspector",
                ),
                (crate::workspace_inspector::Mode::Hidden, "Hide inspector"),
            ] {
                crate::theme::choice(ui, inspector, value, crate::i18n::tr(label));
            }
        },
    );
    option_group(ui, crate::i18n::tr("Process table behavior"));
    option(ui, icon::LIST, crate::i18n::tr("Processes"), "", |ui| {
        for (value, label) in [
            (crate::processes::ViewMode::List, "List"),
            (crate::processes::ViewMode::Tree, "Tree"),
            (crate::processes::ViewMode::Apps, "Apps"),
        ] {
            crate::theme::choice(ui, &mut view.mode, value, crate::i18n::tr(label));
        }
    });
    option_switch(
        ui,
        icon::LIST,
        crate::i18n::tr("Show process descriptions"),
        "",
        &mut view.descriptive_names,
    );
    option_switch(
        ui,
        icon::CHART,
        crate::i18n::tr("Heat map"),
        "",
        &mut view.heat,
    );
    option_switch(
        ui,
        icon::LIST,
        crate::i18n::tr("Alternating row backgrounds"),
        "",
        &mut view.stripes,
    );
    crate::theme::TableLines::options(ui);
    option_switch(
        ui,
        icon::HISTORY,
        crate::i18n::tr("Hold live row order during inspection"),
        crate::i18n::tr("Values stay live. F6 remains a separate frozen snapshot."),
        &mut view.hold_sorting,
    );
    option_group(ui, crate::i18n::tr("Grouping and system processes"));
    option(
        ui,
        icon::PEOPLE,
        crate::i18n::tr("Group processes by user"),
        crate::i18n::tr(
            "Applies to the List view. Automatic groups only while more than one user runs programs.",
        ),
        |ui| {
            for (value, label) in [
                (crate::processes::UserGrouping::Auto, "Automatic"),
                (crate::processes::UserGrouping::Always, "Always group"),
                (crate::processes::UserGrouping::Never, "Never group"),
            ] {
                crate::theme::choice(ui, &mut view.user_grouping, value, crate::i18n::tr(label));
            }
        },
    );
    option_switch(
        ui,
        icon::FILTER,
        crate::i18n::tr("Hide system processes"),
        "",
        &mut view.hide_system,
    );
    option_switch(
        ui,
        icon::LIST,
        crate::i18n::tr("Start the process tree collapsed"),
        "",
        &mut view.tree_collapsed,
    );
    crate::theme::panel(ui, |ui| {
        name_list(
            ui,
            crate::i18n::tr("Always show these programs"),
            &mut view.system.always_show,
        );
        name_list(
            ui,
            crate::i18n::tr("Also hide these programs"),
            &mut view.system.also_hide,
        );
        ui.weak(crate::i18n::tr(
            "One program file name per line, for example notepad.exe.",
        ));
    });
    option_group(ui, crate::i18n::tr("Column layouts"));
    option(
        ui,
        icon::SETTINGS,
        crate::i18n::tr("Column profiles"),
        "",
        |ui| {
            for profile in crate::process_columns::Profile::ALL {
                if ui.button(profile.label()).clicked() {
                    view.apply_profile(profile);
                }
            }
        },
    );
    ui.button(crate::i18n::tr("Open process column editor"))
        .clicked()
}

/// Edits a list of image names, one per line. Blank lines are dropped.
fn name_list(ui: &mut Ui, label: &str, names: &mut Vec<String>) {
    let id = ui.make_persistent_id(("name-list", label));
    let mut text = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| names.join("\n"));
    let label = ui.label(label);
    let response = ui
        .add(
            egui::TextEdit::multiline(&mut text)
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        )
        .labelled_by(label.id);
    if response.changed() {
        *names = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect();
    }
    if response.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
}

pub fn charts(ui: &mut Ui, view: &mut crate::charts::State) {
    use crate::theme::icon;
    crate::theme::option_group(ui, crate::i18n::tr("Timeline preferences"));
    let mut seconds = view.range_seconds();
    crate::theme::option(
        ui,
        icon::HISTORY,
        crate::i18n::tr("Remembered time range"),
        crate::i18n::tr("Uses recorded samples only; selecting a range returns charts to live."),
        |ui| {
            for (value, label) in [(60, "1 min"), (120, "2 min"), (300, "5 min")] {
                if crate::theme::choice(ui, &mut seconds, value, crate::i18n::tr(label)).changed() {
                    view.set_range(seconds);
                }
            }
        },
    );
    for (value, label) in [
        (&mut view.options.grid, "Show chart grid"),
        (&mut view.options.fill, "Fill area below chart lines"),
        (&mut view.options.events, "Show recent event markers"),
        (&mut view.options.link_hover, "Synchronize hover cursors"),
    ] {
        crate::theme::option_switch(ui, icon::CHART, crate::i18n::tr(label), "", value);
    }
    if ui
        .button(crate::i18n::tr("Reset chart preferences"))
        .clicked()
    {
        view.options = crate::charts::Options::default();
        view.set_range(300);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn display_controls_apply_with_padding_and_without_rounding() {
        let mut h = Harness::builder().with_size([900.0, 700.0]).build_ui_state(
            |ui, state: &mut (Display, Option<bool>, Option<Display>)| {
                state.0.apply(ui.ctx(), &mut state.2);
                appearance(ui, &mut state.0, &mut state.1);
            },
            (Display::default(), None, None),
        );
        h.run();
        h.get_by_label("Comfortable").click();
        h.run();
        h.get_by_label("Reduce interface animation").click();
        h.run();
        h.get_by_label("Collapsed").click();
        h.run_steps(2);
        assert_eq!(h.state().0.density, Density::Comfortable);
        assert!(h.state().0.reduced_motion);
        assert_eq!(h.state().1, Some(true));
        let style = h.ctx.global_style();
        assert_eq!(style.spacing.interact_size.y, 34.0);
        assert_eq!(style.animation_time, 0.0);
        assert_eq!(style.spacing.item_spacing, crate::theme::CONTENT_SPACING);
        assert_eq!(style.spacing.button_padding, crate::theme::CONTROL_PADDING);
        assert_eq!(
            style.visuals.widgets.inactive.corner_radius,
            crate::theme::CONTROL_RADIUS
        );
        h.get_by_label("Reset display preferences").click();
        h.run_steps(2);
        assert_eq!(h.state().0, Display::default());
        assert_eq!(h.state().1, None);
        assert_eq!(h.ctx.global_style().spacing.interact_size.y, 28.0);
    }

    #[test]
    fn display_scale_is_bounded_and_tracks_keyboard_zoom() {
        let ctx = egui::Context::default();
        let mut display = Display {
            scale: 900,
            ..Display::default()
        };
        let mut applied = None;
        display.apply(&ctx, &mut applied);
        assert_eq!(display.scale, 150);
        ctx.run_ui(Default::default(), |_| {})
            .textures_delta
            .clear();
        assert_eq!(ctx.zoom_factor(), 1.5);
        ctx.set_zoom_factor(1.25);
        ctx.run_ui(Default::default(), |_| {})
            .textures_delta
            .clear();
        display.apply(&ctx, &mut applied);
        assert_eq!(display.scale, 125);
        display.scale = 1;
        display.apply(&ctx, &mut applied);
        assert_eq!(display.scale, 75);
        ctx.run_ui(Default::default(), |_| {})
            .textures_delta
            .clear();
        assert_eq!(ctx.zoom_factor(), 0.75);
        ctx.set_zoom_factor(0.5);
        ctx.run_ui(Default::default(), |_| {})
            .textures_delta
            .clear();
        display.apply(&ctx, &mut applied);
        ctx.run_ui(Default::default(), |_| {})
            .textures_delta
            .clear();
        assert_eq!(display.scale, 75);
        assert_eq!(ctx.zoom_factor(), 0.75);
        ctx.set_zoom_factor(2.0);
        ctx.run_ui(Default::default(), |_| {})
            .textures_delta
            .clear();
        display.apply(&ctx, &mut applied);
        ctx.run_ui(Default::default(), |_| {})
            .textures_delta
            .clear();
        assert_eq!(display.scale, 150);
        assert_eq!(ctx.zoom_factor(), 1.5);
    }

    #[test]
    fn chart_controls_change_effective_options_and_reset_the_timeline() {
        let mut h = Harness::builder().with_size([900.0, 700.0]).build_ui_state(
            |ui, state: &mut crate::charts::State| charts(ui, state),
            crate::charts::State::default(),
        );
        h.run();
        h.get_by_label("1 min").click();
        h.run();
        assert_eq!(h.state().range_seconds(), 60);
        for label in [
            "Show chart grid",
            "Fill area below chart lines",
            "Show recent event markers",
            "Synchronize hover cursors",
        ] {
            h.get_by_label(label).click();
            h.run();
        }
        assert_eq!(
            h.state().options,
            crate::charts::Options {
                grid: false,
                fill: false,
                events: false,
                link_hover: false,
                kernel_time: true,
            }
        );
        h.get_by_label("Reset chart preferences").click();
        h.run();
        assert_eq!(h.state().range_seconds(), 300);
        assert_eq!(h.state().options, crate::charts::Options::default());
    }
}

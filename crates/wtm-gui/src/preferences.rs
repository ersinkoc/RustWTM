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
            crate::theme::sharp_geometry(ctx);
            ctx.all_styles_mut(|style| {
                style.spacing.interact_size.y = match self.density {
                    Density::Compact => 22.0,
                    Density::Standard => 28.0,
                    Density::Comfortable => 34.0,
                };
                style.animation_time = if self.reduced_motion { 0.0 } else { 0.1 };
            });
            ctx.set_zoom_factor(zoom);
            *applied = Some(*self);
        }
    }
}

pub fn appearance(ui: &mut Ui, display: &mut Display, nav: &mut Option<bool>) {
    crate::theme::card(ui, crate::i18n::tr("Display and accessibility"), |ui| {
        ui.weak(crate::i18n::tr(
            "Changes apply immediately and are remembered for this window.",
        ));
        crate::theme::setting_row(ui, crate::i18n::tr("Interface scale"), |ui, label| {
            ui.add(egui::Slider::new(&mut display.scale, 75..=150).suffix("%"))
                .labelled_by(label);
        });
        crate::theme::divider(ui);
        crate::theme::setting_row(ui, crate::i18n::tr("Control and row height"), |ui, _| {
            ui.horizontal_wrapped(|ui| {
                for (value, label) in [
                    (Density::Compact, "Compact"),
                    (Density::Standard, "Standard"),
                    (Density::Comfortable, "Comfortable"),
                ] {
                    crate::theme::choice(ui, &mut display.density, value, crate::i18n::tr(label));
                }
            });
        });
        crate::theme::divider(ui);
        ui.checkbox(
            &mut display.reduced_motion,
            crate::i18n::tr("Reduce interface animation"),
        );
    });
    crate::theme::card(ui, crate::i18n::tr("Sidebar layout"), |ui| {
        ui.horizontal_wrapped(|ui| {
            for (value, label) in [
                (None, "Automatic"),
                (Some(false), "Expanded"),
                (Some(true), "Collapsed"),
            ] {
                crate::theme::choice(ui, nav, value, crate::i18n::tr(label));
            }
        });
    });
    if ui
        .button(crate::i18n::tr("Reset display preferences"))
        .clicked()
    {
        *display = Display::default();
        *nav = None;
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
    crate::theme::card(ui, crate::i18n::tr("Resource cards and inspector"), |ui| {
        ui.checkbox(
            strip,
            crate::i18n::tr("Show resource cards above processes"),
        );
        ui.label(crate::i18n::tr("Resource card height"));
        ui.horizontal_wrapped(|ui| {
            for (value, label) in [
                (Band::Adaptive, "Adaptive"),
                (Band::Compact, "Compact"),
                (Band::Full, "Full height"),
            ] {
                crate::theme::choice(ui, &mut display.band, value, crate::i18n::tr(label));
            }
        });
        crate::theme::divider(ui);
        ui.horizontal_wrapped(|ui| {
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
        });
    });
    crate::theme::card(ui, crate::i18n::tr("Process table behavior"), |ui| {
        ui.checkbox(&mut view.heat, crate::i18n::tr("Heat map"));
        ui.checkbox(
            &mut view.stripes,
            crate::i18n::tr("Alternating row backgrounds"),
        );
        ui.checkbox(
            &mut view.hold_sorting,
            crate::i18n::tr("Hold live row order during inspection"),
        );
        ui.weak(crate::i18n::tr(
            "Values stay live. F6 remains a separate frozen snapshot.",
        ));
        ui.horizontal_wrapped(|ui| {
            for (value, label) in [
                (crate::processes::ViewMode::List, "List"),
                (crate::processes::ViewMode::Tree, "Tree"),
                (crate::processes::ViewMode::Apps, "Apps"),
            ] {
                crate::theme::choice(ui, &mut view.mode, value, crate::i18n::tr(label));
            }
        });
    });
    crate::theme::card(ui, crate::i18n::tr("Column layouts"), |ui| {
        ui.horizontal_wrapped(|ui| {
            for profile in crate::process_columns::Profile::ALL {
                if ui.button(profile.label()).clicked() {
                    view.apply_profile(profile);
                }
            }
        });
    });
    ui.button(crate::i18n::tr("Open process column editor"))
        .clicked()
}

pub fn charts(ui: &mut Ui, view: &mut crate::charts::State) {
    crate::theme::card(ui, crate::i18n::tr("Timeline preferences"), |ui| {
        ui.label(crate::i18n::tr("Remembered time range"));
        let mut seconds = view.range_seconds();
        ui.horizontal_wrapped(|ui| {
            for (value, label) in [(60, "1 min"), (120, "2 min"), (300, "5 min")] {
                if crate::theme::choice(ui, &mut seconds, value, crate::i18n::tr(label)).changed() {
                    view.set_range(seconds);
                }
            }
        });
        ui.weak(crate::i18n::tr(
            "Uses recorded samples only; selecting a range returns charts to live.",
        ));
        crate::theme::divider(ui);
        ui.checkbox(&mut view.options.grid, crate::i18n::tr("Show chart grid"));
        ui.checkbox(
            &mut view.options.fill,
            crate::i18n::tr("Fill area below chart lines"),
        );
        ui.checkbox(
            &mut view.options.events,
            crate::i18n::tr("Show recent event markers"),
        );
        ui.checkbox(
            &mut view.options.link_hover,
            crate::i18n::tr("Synchronize hover cursors"),
        );
    });
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
            egui::CornerRadius::ZERO
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
                link_hover: false
            }
        );
        h.get_by_label("Reset chart preferences").click();
        h.run();
        assert_eq!(h.state().range_seconds(), 300);
        assert_eq!(h.state().options, crate::charts::Options::default());
    }
}

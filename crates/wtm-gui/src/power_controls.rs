//! Shared frequency controls remain distinct from per-process EcoQoS/affinity.
use crate::backend::{Data, Panel, UiCommand};
use eframe::egui::{self, Ui};
use wtm_core::power::{FrequencyRequest, PowerSource, PowerSupply};

#[derive(Clone)]
enum Confirmation {
    Limit(FrequencyRequest),
    Restore,
}
pub struct PowerView {
    class: u8,
    source: PowerSupply,
    mhz: u32,
    confirm: Option<Confirmation>,
}
impl Default for PowerView {
    fn default() -> Self {
        Self {
            class: 0,
            source: PowerSupply::Ac,
            mhz: 1800,
            confirm: None,
        }
    }
}
pub fn power_controls(ui: &mut Ui, data: &Data, view: &mut PowerView) -> Vec<UiCommand> {
    let mut commands = Vec::new();
    ui.add_space(12.0);
    crate::theme::card(ui, crate::i18n::tr("CPU frequency policy"), |ui| {
        ui.label(crate::i18n::tr("A shared Windows power-plan limit affects every app using that policy class. For one app, use Efficiency mode or CPU core type in Processes."));
        ui.weak(crate::i18n::tr(
            "Policy class 1 is not automatically E-cores. Firmware may ignore frequency requests.",
        ));
        if ui
            .small_button(crate::i18n::tr("Refresh power plan"))
            .clicked()
        {
            commands.push(UiCommand::Load(Panel::Sensors));
        }
        let report = match &data.frequency {
            Some(Ok(report)) => report,
            Some(Err(error)) => {
                ui.label(error);
                return;
            }
            None => {
                ui.weak(crate::i18n::tr(
                    "Power-plan settings have not been loaded yet.",
                ));
                return;
            }
        };
        ui.weak({
            let __wtm_i18n_0 = &(report.plan);
            crate::i18n::format("Active plan: {0}", std::slice::from_ref(__wtm_i18n_0))
        });
        egui::Grid::new("frequency_values")
            .spacing([18.0, 6.0])
            .show(ui, |ui| {
                ui.strong(crate::i18n::tr("Windows policy"));
                ui.strong(crate::i18n::tr("Plugged in"));
                ui.strong(crate::i18n::tr("Battery"));
                ui.end_row();
                for class in [0, 1] {
                    ui.label(crate::i18n::format("Class {0}", &[format!("{}", class)]));
                    for source in [PowerSource::Ac, PowerSource::Dc] {
                        let value = report
                            .values
                            .iter()
                            .find(|v| v.class == class && v.source == source)
                            .and_then(|v| v.mhz);
                        ui.label(value.map_or_else(
                            || crate::i18n::tr("Unavailable").into(),
                            |v| {
                                if v == 0 {
                                    crate::i18n::tr("Windows default").into()
                                } else {
                                    crate::i18n::format("{0} MHz", &[format!("{}", v)])
                                }
                            },
                        ));
                    }
                    ui.end_row();
                }
            });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::new("power_class", crate::i18n::tr("Policy class"))
                .selected_text({
                    let __wtm_i18n_0 = &(view.class);
                    crate::i18n::format("Class {0}", &[format!("{}", __wtm_i18n_0)])
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut view.class, 0, crate::i18n::tr("Class 0"));
                    ui.selectable_value(&mut view.class, 1, crate::i18n::tr("Class 1"));
                });
            egui::ComboBox::new("power_source", crate::i18n::tr("Power source"))
                .selected_text(crate::i18n::tr(view.source.label()))
                .show_ui(ui, |ui| {
                    for source in [PowerSupply::Ac, PowerSupply::Dc, PowerSupply::Both] {
                        ui.selectable_value(
                            &mut view.source,
                            source,
                            crate::i18n::tr(source.label()),
                        );
                    }
                });
            ui.add(
                egui::DragValue::new(&mut view.mhz)
                    .range(100..=64000)
                    .suffix(crate::i18n::tr(" MHz")),
            );
        });
        let request = FrequencyRequest {
            plan: report.plan.clone(),
            class: view.class,
            source: view.source,
            mhz: view.mhz,
        };
        let supported = report
            .values
            .iter()
            .filter(|v| {
                v.class == view.class
                    && match view.source {
                        PowerSupply::Ac => v.source == PowerSource::Ac,
                        PowerSupply::Dc => v.source == PowerSource::Dc,
                        PowerSupply::Both => true,
                    }
            })
            .filter(|v| v.mhz.is_some())
            .count()
            == if view.source == PowerSupply::Both {
                2
            } else {
                1
            };
        let available = data.elevated
            && supported
            && report.backup_plan.is_none()
            && report.backup_error.is_none()
            && request.validate().is_ok();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    available,
                    egui::Button::new(crate::i18n::tr("Set frequency limit…")),
                )
                .clicked()
            {
                view.confirm = Some(Confirmation::Limit(request));
            }
            if ui
                .add_enabled(
                    data.elevated && report.backup_plan.is_some(),
                    egui::Button::new(crate::i18n::tr("Restore saved frequency settings…")),
                )
                .clicked()
            {
                view.confirm = Some(Confirmation::Restore);
            }
        });
        if let Some(plan) = &report.backup_plan {
            ui.label({
                crate::i18n::format(
                    "Original settings saved for plan {0}. Restore before applying another limit.",
                    std::slice::from_ref(plan),
                )
            });
        }
        if let Some(error) = &report.backup_error {
            ui.label(crate::i18n::format(
                "Backup unavailable: {0}",
                std::slice::from_ref(error),
            ));
        }
        if !supported {
            ui.weak(crate::i18n::tr(
                "The selected setting is unavailable on this power plan.",
            ));
        }
        if !data.elevated {
            ui.weak(crate::i18n::tr(
                "Administrator rights are required to change these shared settings.",
            ));
            if ui
                .button(crate::i18n::tr("Elevate for power controls"))
                .clicked()
            {
                commands.push(UiCommand::RestartElevated);
            }
        }
    });
    if let Some(action) = view.confirm.clone() {
        let modal=egui::Modal::new(egui::Id::unique("confirm_frequency")).show(ui.ctx(),|ui| {
            let (heading,label)=match &action {Confirmation::Limit(_)=>(crate::i18n::tr("Change shared CPU frequency policy?"),crate::i18n::tr("Apply system frequency limit")),Confirmation::Restore=>(crate::i18n::tr("Restore original frequency settings?"),crate::i18n::tr("Restore power settings"))};
            ui.heading(heading);
            match &action {
                Confirmation::Limit(request)=> {
                    ui.label({ let __wtm_i18n_0 = &(request.class); let __wtm_i18n_1 = &(request.source.label()); let __wtm_i18n_2 = &(request.mhz); crate::i18n::format("Windows policy class {0} · {1} · {2} MHz", &[format!("{}", __wtm_i18n_0), __wtm_i18n_1.to_string(), format!("{}", __wtm_i18n_2)]) });
                    ui.label(crate::i18n::tr("This affects all apps using that policy class, not one PID. It persists after RustWTM closes. Original values are saved before writing; use Restore to undo it."));
                }
                Confirmation::Restore=>{ui.label(crate::i18n::tr("Restore the saved values on the original plan without switching plans. If another app changed these settings, RustWTM will refuse to overwrite them."));}
            }
            if ui.button(label).clicked() {
                commands.push(match action {Confirmation::Limit(request)=>UiCommand::SetFrequency(request),Confirmation::Restore=>UiCommand::RestoreFrequency});
                view.confirm=None;
            }
            if ui.button(crate::i18n::tr("Cancel")).clicked() {view.confirm=None;}
        });
        if modal.should_close() {
            view.confirm = None;
        }
    }
    commands
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };
    struct State {
        data: Data,
        view: PowerView,
        sent: Vec<UiCommand>,
    }

    #[cfg(feature = "visual-tests")]
    pub(crate) fn render_power_review(out: &std::path::Path) {
        std::fs::create_dir_all(out).unwrap();
        for (theme_name, theme) in [("dark", egui::Theme::Dark), ("light", egui::Theme::Light)] {
            for (size_name, size) in [("wide", [1100.0, 650.0]), ("compact", [820.0, 500.0])] {
                let mut data = crate::ui_tests::data();
                data.elevated = true;
                data.frequency = Some(Ok(wtm_core::power::FrequencyReport {
                    plan: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
                    values: [0, 1]
                        .into_iter()
                        .flat_map(|class| {
                            [PowerSource::Ac, PowerSource::Dc]
                                .into_iter()
                                .map(move |source| wtm_core::power::FrequencyValue {
                                    class,
                                    source,
                                    mhz: Some(if source == PowerSource::Ac { 0 } else { 2200 }),
                                })
                        })
                        .collect(),
                    backup_plan: None,
                    backup_error: None,
                }));
                let mut h = Harness::builder()
                    .allow_missing_glyphs()
                    .with_theme(theme)
                    .with_size(size)
                    .build_ui_state(
                        |ui, s: &mut State| {
                            s.sent.extend(power_controls(ui, &s.data, &mut s.view));
                        },
                        State {
                            data,
                            view: PowerView::default(),
                            sent: Vec::new(),
                        },
                    );
                crate::theme::install_fonts(&h.ctx);
                crate::theme::apply(&h.ctx);
                h.ctx.set_theme(theme);
                h.run_steps(3);
                h.render()
                    .unwrap()
                    .save(out.join(format!("{theme_name}-{size_name}-Power.png")))
                    .unwrap();
            }
        }
    }
    #[test]
    fn shared_power_changes_require_confirmation_and_privilege() {
        let mut data = crate::ui_tests::data();
        data.elevated = true;
        data.frequency = Some(Ok(wtm_core::power::FrequencyReport {
            plan: "test-plan".into(),
            values: vec![wtm_core::power::FrequencyValue {
                class: 0,
                source: PowerSource::Ac,
                mhz: Some(0),
            }],
            backup_plan: None,
            backup_error: None,
        }));
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size([900.0, 1000.0])
            .build_ui_state(
                |ui, s: &mut State| {
                    s.sent.extend(power_controls(ui, &s.data, &mut s.view));
                },
                State {
                    data,
                    view: PowerView::default(),
                    sent: Vec::new(),
                },
            );
        h.run();
        h.get_by_label("Set frequency limit…").click();
        h.run();
        assert!(h.state().sent.is_empty());
        h.get_by_label("Cancel").click();
        h.run();
        assert!(h.state().sent.is_empty());
        h.get_by_label("Set frequency limit…").click();
        h.run();
        h.get_by_label("Apply system frequency limit").click();
        h.run();
        assert!(matches!(
            h.state().sent[0],
            UiCommand::SetFrequency(FrequencyRequest {
                class: 0,
                mhz: 1800,
                source: PowerSupply::Ac,
                ..
            })
        ));
        h.state_mut().data.elevated = false;
        h.run();
        assert!(
            h.get_by_label("Set frequency limit…")
                .accesskit_node()
                .is_disabled()
        );
        h.state_mut().data.elevated = true;
        h.state_mut()
            .data
            .frequency
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap()
            .backup_plan = Some("old-plan".into());
        h.run();
        assert!(
            h.get_by_label("Set frequency limit…")
                .accesskit_node()
                .is_disabled()
        );
        h.get_by_label("Restore saved frequency settings…").click();
        h.run();
        h.get_by_label("Restore power settings").click();
        h.run();
        assert!(matches!(h.state().sent[1], UiCommand::RestoreFrequency));
    }
}

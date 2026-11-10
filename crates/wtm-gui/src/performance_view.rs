//! Resource navigation and presentation-only filters for Performance.
use eframe::egui::{self, Ui};

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Page {
    Cpu,
    Memory,
    Gpu,
    Network,
    Disk,
    #[default]
    #[serde(other)]
    All,
}

impl From<crate::overview::Resource> for Page {
    fn from(value: crate::overview::Resource) -> Self {
        use crate::overview::Resource;
        match value {
            Resource::Cpu => Self::Cpu,
            Resource::Memory => Self::Memory,
            Resource::Gpu => Self::Gpu,
            Resource::Network => Self::Network,
            Resource::Disk => Self::Disk,
        }
    }
}

#[derive(Default)]
pub struct State {
    pub page: Page,
    pub class: Option<u8>,
    pub engine_search: String,
}

impl State {
    /// Keep navigation outside the content scroll area, including on compact windows.
    pub fn navigation(&mut self, ui: &mut Ui) -> bool {
        let before = self.page;
        ui.horizontal_wrapped(|ui| {
            for (page, label) in [
                (Page::All, "All resources"),
                (Page::Cpu, "CPU"),
                (Page::Memory, "Memory"),
                (Page::Gpu, "GPU"),
                (Page::Network, "Network"),
                (Page::Disk, "Disk & I/O"),
            ] {
                let response =
                    crate::theme::choice(ui, &mut self.page, page, crate::i18n::tr(label));
                if page != Page::All {
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::accesskit::Role::Button,
                            ui.is_enabled(),
                            self.page == page,
                            crate::i18n::format(
                                "{0} performance view",
                                &[crate::i18n::tr(label).into()],
                            ),
                        )
                    });
                }
                if response.clicked() {
                    self.page = page;
                }
            }
        });
        ui.add_space(0.0);
        self.page != before
    }
}

/// Flat usage arrays have no processor-group identity. Only use topology when
/// each array index maps uniquely to a CPU in one group.
pub fn matched_topology(
    topology: Option<&wtm_core::cpu::Topology>,
    count: usize,
) -> Option<&wtm_core::cpu::Topology> {
    topology.filter(|t| {
        t.cpus.len() == count
            && t.groups().len() == 1
            && (0..count).all(|i| t.cpus.iter().filter(|c| usize::from(c.index) == i).count() == 1)
    })
}

pub fn cpu_filter(
    ui: &mut Ui,
    class: &mut Option<u8>,
    topology: Option<&wtm_core::cpu::Topology>,
    count: usize,
) {
    let Some(t) = matched_topology(topology, count).filter(|t| t.hybrid()) else {
        *class = None;
        ui.weak(crate::i18n::tr(
            "CPU tier filtering requires matching single-group hybrid topology.",
        ));
        return;
    };
    let classes = t.classes();
    if class.is_some_and(|c| !classes.contains(&c)) {
        *class = None;
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .selectable_label(class.is_none(), crate::i18n::tr("All CPU tiers"))
            .clicked()
        {
            *class = None;
        }
        for c in classes.into_iter().rev() {
            let count = t
                .cpus
                .iter()
                .filter(|cpu| cpu.efficiency_class == c)
                .count();
            let label = crate::i18n::format("{0} tier ({1})", &[t.tier(c), count.to_string()]);
            if ui.selectable_label(*class == Some(c), label).clicked() {
                *class = Some(c);
            }
        }
    });
    ui.weak(crate::i18n::tr(
        "The tier filter affects logical-processor tiles; the main chart remains total CPU usage.",
    ));
}

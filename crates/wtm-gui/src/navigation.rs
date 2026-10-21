//! Resolution-independent line icons and accessible navigation rows.

use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui};

#[derive(Clone, Copy)]
pub enum Icon {
    Overview,
    Processes,
    Performance,
    Disk,
    Sensors,
    History,
    Shield,
    Limits,
    Stop,
    Events,
    Settings,
    Startup,
    Services,
    Users,
    Network,
    Ports,
    About,
}

fn paint(ui: &Ui, rect: Rect, icon: Icon, color: Color32) {
    let p = ui.painter();
    let stroke = Stroke::new(1.6, color);
    let at = |x: f32, y: f32| rect.min + egui::vec2(x, y);
    let line = |points: &[(f32, f32)]| {
        p.add(egui::Shape::line(
            points.iter().map(|&(x, y)| at(x, y)).collect(),
            stroke,
        ));
    };
    let circle = |x, y, r| {
        p.circle_stroke(at(x, y), r, stroke);
    };
    let box_at = |x, y, w, h| {
        p.rect_stroke(
            Rect::from_min_size(at(x, y), egui::vec2(w, h)),
            2.0,
            stroke,
            egui::StrokeKind::Inside,
        );
    };
    match icon {
        Icon::Overview => {
            for (x, y) in [(2.0, 2.0), (11.0, 2.0), (2.0, 11.0), (11.0, 11.0)] {
                box_at(x, y, 6.0, 6.0);
            }
        }
        Icon::Processes => {
            box_at(2.0, 2.0, 15.0, 15.0);
            for y in [6.0, 10.0, 14.0] {
                line(&[(5.0, y), (14.0, y)]);
            }
        }
        Icon::Performance => line(&[
            (1.0, 10.0),
            (5.0, 10.0),
            (8.0, 3.0),
            (11.0, 16.0),
            (14.0, 8.0),
            (18.0, 8.0),
        ]),
        Icon::Disk => {
            // A drive: a box with a light and a platter line.
            box_at(2.0, 5.0, 15.0, 10.0);
            line(&[(5.0, 10.0), (11.0, 10.0)]);
            circle(14.0, 10.0, 0.8);
        }
        Icon::Sensors => {
            circle(9.5, 14.0, 4.0);
            line(&[
                (6.5, 11.0),
                (6.5, 3.0),
                (9.5, 1.5),
                (12.5, 3.0),
                (12.5, 11.0),
            ]);
            line(&[(9.5, 6.0), (9.5, 14.0)]);
        }
        Icon::History => {
            circle(9.5, 9.5, 7.0);
            line(&[(9.5, 5.0), (9.5, 10.0), (13.0, 12.0)]);
        }
        Icon::Shield => {
            line(&[
                (9.5, 1.5),
                (17.0, 4.0),
                (16.0, 11.0),
                (13.5, 15.0),
                (9.5, 18.0),
                (5.5, 15.0),
                (3.0, 11.0),
                (2.0, 4.0),
                (9.5, 1.5),
            ]);
            line(&[(6.0, 9.0), (8.5, 11.5), (13.0, 7.0)]);
        }
        Icon::Limits => {
            for (x, y) in [(4.0, 6.0), (9.5, 13.0), (15.0, 8.0)] {
                line(&[(x, 2.0), (x, y - 2.0)]);
                circle(x, y, 2.0);
                line(&[(x, y + 2.0), (x, 17.0)]);
            }
        }
        Icon::Settings => {
            // A gear: a hub, a rim and eight teeth.
            circle(9.5, 9.5, 2.5);
            circle(9.5, 9.5, 5.5);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let (s, c) = a.sin_cos();
                line(&[
                    (9.5 + 5.5 * c, 9.5 + 5.5 * s),
                    (9.5 + 8.0 * c, 9.5 + 8.0 * s),
                ]);
            }
        }
        Icon::Stop => {
            line(&[
                (6.0, 2.0),
                (13.0, 2.0),
                (17.0, 6.0),
                (17.0, 13.0),
                (13.0, 17.0),
                (6.0, 17.0),
                (2.0, 13.0),
                (2.0, 6.0),
                (6.0, 2.0),
            ]);
            line(&[(6.5, 6.5), (12.5, 12.5)]);
            line(&[(6.5, 12.5), (12.5, 6.5)]);
        }
        Icon::Events => {
            line(&[
                (3.0, 14.0),
                (5.0, 11.0),
                (5.0, 7.0),
                (7.0, 3.0),
                (12.0, 3.0),
                (14.0, 7.0),
                (14.0, 11.0),
                (16.0, 14.0),
                (3.0, 14.0),
            ]);
            line(&[(7.5, 17.0), (11.5, 17.0)]);
        }
        Icon::Startup => {
            line(&[(9.5, 17.0), (9.5, 2.0), (3.5, 8.0)]);
            line(&[(9.5, 2.0), (15.5, 8.0)]);
            line(&[(3.0, 17.0), (16.0, 17.0)]);
        }
        Icon::Services => {
            box_at(2.0, 2.0, 15.0, 6.0);
            box_at(2.0, 11.0, 15.0, 6.0);
            circle(5.5, 5.0, 0.5);
            circle(5.5, 14.0, 0.5);
        }
        Icon::Users => {
            circle(9.5, 5.5, 3.5);
            line(&[
                (2.5, 17.0),
                (3.5, 13.0),
                (6.5, 11.0),
                (12.5, 11.0),
                (15.5, 13.0),
                (16.5, 17.0),
            ]);
        }
        Icon::Network => {
            circle(9.5, 9.5, 7.5);
            line(&[(2.0, 9.5), (17.0, 9.5)]);
            p.add(egui::Shape::ellipse_stroke(
                at(9.5, 9.5),
                egui::vec2(3.5, 7.5),
                stroke,
            ));
        }
        Icon::Ports => {
            box_at(4.0, 6.0, 11.0, 7.0);
            for x in [7.0, 12.0] {
                line(&[(x, 2.0), (x, 6.0)]);
            }
            line(&[(9.5, 13.0), (9.5, 18.0)]);
        }
        Icon::About => {
            circle(9.5, 9.5, 7.5);
            circle(9.5, 5.5, 0.5);
            line(&[(9.5, 9.0), (9.5, 14.0)]);
        }
    }
}

/// Where a row's label starts, and the space kept free after it.
pub const LABEL_LEFT: f32 = 42.0;
pub const LABEL_RIGHT_GAP: f32 = 14.0;

pub fn row(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    selected: bool,
    collapsed: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::accesskit::Role::Button,
            ui.is_enabled(),
            selected,
            label,
        )
    });
    let v = ui.visuals();
    if selected || response.hovered() || response.has_focus() {
        let fill = if selected {
            v.selection.bg_fill
        } else {
            v.widgets.hovered.bg_fill
        };
        ui.painter().rect_filled(rect, 5.0, fill);
    }
    if selected {
        ui.painter().rect_filled(
            Rect::from_min_size(
                rect.left_top() + egui::vec2(0.0, 7.0),
                egui::vec2(3.0, 18.0),
            ),
            2.0,
            crate::theme::ACCENT,
        );
    }
    let color = if selected {
        v.strong_text_color()
    } else {
        v.text_color()
    };
    let left = if collapsed {
        rect.center().x - 9.5
    } else {
        rect.left() + 12.0
    };
    paint(
        ui,
        Rect::from_min_size(
            Pos2::new(left, rect.center().y - 9.5),
            egui::vec2(19.0, 19.0),
        ),
        icon,
        color,
    );
    if !collapsed {
        // Long translations end in "…" before the right edge; the hover
        // text has them in full.
        let left = rect.left() + LABEL_LEFT;
        let mut job = egui::text::LayoutJob::simple_singleline(
            label.into(),
            FontId::proportional(14.0),
            color,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(
            (rect.right() - LABEL_RIGHT_GAP - left).max(0.0),
        );
        let galley = ui.painter().layout_job(job);
        ui.painter().galley(
            Pos2::new(left, rect.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
    }
    response.on_hover_text(label)
}

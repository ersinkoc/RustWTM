//! Notification-area icon: show the window, toggle notifications, exit.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, ViewportCommand};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub struct Tray {
    icon: TrayIcon,
    tooltip: String,
    /// CPU load shown by the icon, in steps of 10%.
    load_step: Option<u8>,
}

const SIZE: u32 = 32;

/// The RustWTM logo at tray size.
fn icon() -> Option<Icon> {
    Icon::from_rgba(crate::logo::rgba(SIZE), SIZE, SIZE).ok()
}

/// Width of the load bar on the right edge of the icon.
const BAR: u32 = 7;

/// The logo with a CPU load bar (`step` tenths, 0..=10) on its right edge:
/// green, orange from 60%, red from 90%.
pub fn load_rgba(step: u8) -> Vec<u8> {
    // The logo, shrunk to the space left of the bar and centred vertically.
    let side = SIZE - BAR;
    let logo = crate::logo::rgba(side);
    let top = (SIZE - side) / 2;
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..side {
        let src = (y * side * 4) as usize;
        let dst = (((y + top) * SIZE) * 4) as usize;
        rgba[dst..dst + (side * 4) as usize].copy_from_slice(&logo[src..src + (side * 4) as usize]);
    }
    let filled = SIZE * u32::from(step.min(10)) / 10;
    let color = match step {
        9.. => [220, 70, 70],
        6..=8 => [230, 140, 40],
        _ => [80, 180, 100],
    };
    for y in 0..SIZE {
        for x in SIZE - BAR..SIZE {
            let i = ((y * SIZE + x) * 4) as usize;
            let [r, g, b] = if SIZE - y <= filled.max(2) && step > 0 {
                color
            } else {
                [40, 40, 40]
            };
            // A one-pixel transparent gap separates the bar from the logo.
            let a = if x == SIZE - BAR { 0 } else { 255 };
            rgba[i..i + 4].copy_from_slice(&[r, g, b, a]);
        }
    }
    rgba
}

fn show_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(ViewportCommand::Focus);
    ctx.request_repaint();
}

/// Handles a click on one of the menu items `[show, notify, exit]`.
fn on_menu(id: &MenuId, items: &[MenuId; 3], ctx: &egui::Context, notifications: &AtomicBool) {
    let [show_id, notify_id, exit_id] = items;
    if id == show_id {
        show_window(ctx);
    } else if id == notify_id {
        // The check item toggles itself; mirror it.
        notifications.fetch_xor(true, Ordering::Relaxed);
    } else if id == exit_id {
        ctx.send_viewport_cmd(ViewportCommand::Close);
        ctx.request_repaint();
    }
}

/// A left double-click on the icon brings the window back.
fn on_icon(event: &TrayIconEvent, ctx: &egui::Context) {
    if let TrayIconEvent::DoubleClick {
        button: MouseButton::Left,
        ..
    } = event
    {
        show_window(ctx);
    }
}

impl Tray {
    /// Returns `None` if the shell refuses the icon; the app works without it.
    pub fn new(ctx: &egui::Context, notifications: Arc<AtomicBool>) -> Option<Self> {
        let show = MenuItem::new("Show RustWTM", true, None);
        let notify = CheckMenuItem::new(
            "Notifications",
            true,
            notifications.load(Ordering::Relaxed),
            None,
        );
        let exit = MenuItem::new("Exit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&show, &notify, &PredefinedMenuItem::separator(), &exit])
            .ok()?;

        let items = [show.id().clone(), notify.id().clone(), exit.id().clone()];
        let menu_ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            on_menu(&event.id, &items, &menu_ctx, &notifications);
        }));

        let click_ctx = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            on_icon(&event, &click_ctx);
        }));

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("RustWTM")
            .with_icon(icon()?)
            .build()
            .ok()?;
        Some(Self {
            icon,
            tooltip: "RustWTM".into(),
            load_step: None,
        })
    }

    /// Shows the CPU load in the icon; redraws only when it changes by a step.
    pub fn set_load(&mut self, cpu_percent: f32) {
        let step = (cpu_percent / 10.0).round().clamp(0.0, 10.0) as u8;
        if self.load_step != Some(step)
            && let Ok(icon) = Icon::from_rgba(load_rgba(step), SIZE, SIZE)
        {
            let _ = self.icon.set_icon(Some(icon));
            self.load_step = Some(step);
        }
    }

    pub fn set_tooltip(&mut self, text: String) {
        if text != self.tooltip {
            let _ = self.icon.set_tooltip(Some(&text));
            self.tooltip = text;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tray_icon::dpi::PhysicalPosition;
    use tray_icon::{Rect, TrayIconId};

    /// Viewport commands sent to `ctx` since the last frame.
    fn commands(ctx: &egui::Context) -> Vec<ViewportCommand> {
        let mut output = ctx.run_pass(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.commands.clone())
            .unwrap_or_default()
    }

    /// A context past its first pass, which reports the initial viewport.
    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        commands(&ctx);
        ctx
    }

    #[test]
    fn the_load_bar_fills_from_the_bottom_with_the_load_colour() {
        let pixel = |rgba: &[u8], x: u32, y: u32| {
            let i = ((y * SIZE + x) * 4) as usize;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };
        let x = SIZE - 2;
        let idle = load_rgba(0);
        assert_eq!(pixel(&idle, x, SIZE - 1), [40, 40, 40, 255]);
        let half = load_rgba(5);
        assert_eq!(pixel(&half, x, SIZE - 1), [80, 180, 100, 255]);
        assert_eq!(pixel(&half, x, 0), [40, 40, 40, 255]);
        assert_eq!(pixel(&load_rgba(7), x, SIZE - 1), [230, 140, 40, 255]);
        let full = load_rgba(10);
        assert_eq!(pixel(&full, x, 0), [220, 70, 70, 255]);
        assert_eq!(pixel(&full, SIZE - BAR, 0)[3], 0, "gap");
        // The logo sits left of the bar, centred vertically.
        let side = SIZE - BAR;
        let top = (SIZE - side) / 2;
        let logo = crate::logo::rgba(side);
        let row = |rgba: &[u8], y: u32, w: u32| {
            rgba[(y * w * 4) as usize..][..(side * 4) as usize].to_vec()
        };
        assert_eq!(row(&full, top + side / 2, SIZE), row(&logo, side / 2, side));
        assert_eq!(pixel(&full, 0, 0)[3], 0, "transparent above the logo");
    }

    #[test]
    fn menu_items_show_toggle_and_exit() {
        let ctx = context();
        let notifications = AtomicBool::new(true);
        let items = [
            MenuId::new("show"),
            MenuId::new("notify"),
            MenuId::new("exit"),
        ];

        on_menu(&items[1], &items, &ctx, &notifications);
        assert!(!notifications.load(Ordering::Relaxed));
        on_menu(&MenuId::new("other"), &items, &ctx, &notifications);
        assert!(commands(&ctx).is_empty());

        on_menu(&items[0], &items, &ctx, &notifications);
        assert!(commands(&ctx).contains(&ViewportCommand::Visible(true)));
        on_menu(&items[2], &items, &ctx, &notifications);
        assert_eq!(commands(&ctx), [ViewportCommand::Close]);
    }

    #[test]
    fn left_double_click_shows_the_window() {
        let ctx = context();
        let click = |button| TrayIconEvent::DoubleClick {
            id: TrayIconId::new("tray"),
            position: PhysicalPosition::new(0.0, 0.0),
            rect: Rect::default(),
            button,
        };
        on_icon(&click(MouseButton::Right), &ctx);
        assert!(commands(&ctx).is_empty());
        on_icon(&click(MouseButton::Left), &ctx);
        assert!(commands(&ctx).contains(&ViewportCommand::Focus));
        assert!(icon().is_some());
    }
}

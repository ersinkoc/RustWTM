//! Notification-area icon: show the window, toggle notifications, exit.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, ViewportCommand};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub struct Tray {
    icon: TrayIcon,
    tooltip: String,
}

const SIZE: u32 = 32;

/// A simple generated icon: three ascending bars on a rounded blue tile.
fn icon() -> Option<Icon> {
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let corner = |cx: u32, cy: u32| {
                let (dx, dy) = (x.abs_diff(cx), y.abs_diff(cy));
                dx * dx + dy * dy > 36
            };
            let outside = (x < 6 && y < 6 && corner(6, 6))
                || (x > 25 && y < 6 && corner(25, 6))
                || (x < 6 && y > 25 && corner(6, 25))
                || (x > 25 && y > 25 && corner(25, 25));
            let bar = [(7, 18), (14, 12), (21, 6)]
                .iter()
                .any(|&(bx, top)| (bx..bx + 5).contains(&x) && (top..26).contains(&y));
            let px = match (outside, bar) {
                (true, _) => [0, 0, 0, 0],
                (false, true) => [255, 255, 255, 255],
                (false, false) => [30, 110, 210, 255],
            };
            rgba.extend(px);
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).ok()
}

fn show_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(ViewportCommand::Focus);
    ctx.request_repaint();
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

        let (show_id, notify_id, exit_id) =
            (show.id().clone(), notify.id().clone(), exit.id().clone());
        let menu_ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == show_id {
                show_window(&menu_ctx);
            } else if event.id == notify_id {
                // The check item toggles itself; mirror it.
                notifications.fetch_xor(true, Ordering::Relaxed);
            } else if event.id == exit_id {
                menu_ctx.send_viewport_cmd(ViewportCommand::Close);
                menu_ctx.request_repaint();
            }
        }));

        let click_ctx = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } = event
            {
                show_window(&click_ctx);
            }
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
        })
    }

    pub fn set_tooltip(&mut self, text: String) {
        if text != self.tooltip {
            let _ = self.icon.set_tooltip(Some(&text));
            self.tooltip = text;
        }
    }
}

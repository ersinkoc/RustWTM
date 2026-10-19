//! Notification-area icon: show the window, toggle notifications, exit.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, ViewportCommand};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub struct Tray {
    icon: TrayIcon,
    tooltip: String,
}

const SIZE: u32 = 32;

/// The RustWTM logo at tray size.
fn icon() -> Option<Icon> {
    Icon::from_rgba(crate::logo::rgba(SIZE), SIZE, SIZE).ok()
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
        })
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

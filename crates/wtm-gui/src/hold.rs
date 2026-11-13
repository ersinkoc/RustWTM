//! Holding Ctrl freezes the live views for as long as it is held: a quick
//! look at a moving list or chart without reaching for the pause button.

use eframe::egui::Context;

/// Ctrl alone is held down in the focused window, outside text fields,
/// menus and dialogs (where Ctrl belongs to copy, paste and shortcuts).
pub fn ctrl_held(ctx: &Context) -> bool {
    let held = ctx.input(|i| {
        let m = i.modifiers;
        i.raw.focused && m.ctrl && !m.alt && !m.shift
    });
    held && !ctx.text_edit_focused()
        && !eframe::egui::Popup::is_any_open(ctx)
        && ctx.memory(|m| m.top_modal_layer().is_none())
}

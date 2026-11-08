//! Keyboard shortcuts as the owner set them (Settings → Keyboard), over the defaults in
//! `tern_core::keys`. Key presses become accelerator names and are looked up there, so a
//! change takes effect at once.

use gtk::gdk;
use gtk::prelude::*;
use tern_core::keys::{self, Group, Overrides};

use crate::app::Ui;

const MODS: gdk::ModifierType = gdk::ModifierType::CONTROL_MASK
    .union(gdk::ModifierType::SHIFT_MASK)
    .union(gdk::ModifierType::ALT_MASK)
    .union(gdk::ModifierType::SUPER_MASK);

/// A key press as an accelerator name ("<Control><Shift>r"), or `None` for a lone modifier.
pub fn stroke(key: gdk::Key, mods: gdk::ModifierType) -> Option<String> {
    let name = key.name()?;
    if is_modifier(&name) {
        return None;
    }
    Some(keys::normalize(&gtk::accelerator_name(key.to_lower(), mods & MODS)))
}

fn is_modifier(name: &str) -> bool {
    ["Shift_", "Control_", "Alt_", "Super_", "Meta_", "Hyper_", "ISO_Level", "Caps_Lock", "Num_Lock"]
        .iter()
        .any(|p| name.starts_with(p))
}

/// The shortcut in `group` this single key press is bound to.
pub fn lookup(ui: &Ui, group: Group, stroke: &str) -> Option<&'static str> {
    let overrides = ui.settings.borrow().keys.clone();
    find(&overrides, group, stroke)
}

pub fn find(overrides: &Overrides, group: Group, stroke: &str) -> Option<&'static str> {
    keys::SHORTCUTS
        .iter()
        .filter(|s| s.group == group)
        .find(|s| keys::bindings(s, overrides).iter().any(|b| b == stroke))
        .map(|s| s.id)
}

/// Every key (or two-key sequence) bound in `group`, with its shortcut.
pub fn table(ui: &Ui, group: Group) -> Vec<(String, &'static str)> {
    let overrides = ui.settings.borrow().keys.clone();
    keys::SHORTCUTS
        .iter()
        .filter(|s| s.group == group)
        .flat_map(|s| keys::bindings(s, &overrides).into_iter().map(move |b| (b, s.id)))
        .collect()
}

/// (Re)register the app-wide shortcuts with GTK.
pub fn apply_app_accels(ui: &Ui) {
    let overrides = ui.settings.borrow().keys.clone();
    for s in keys::SHORTCUTS.iter().filter(|s| s.group == Group::App) {
        let accels = keys::bindings(s, &overrides);
        let accels: Vec<&str> = accels.iter().map(String::as_str).collect();
        ui.app.set_accels_for_action(s.id, &accels);
    }
}

/// How a binding reads in Settings: one shortcut label per key of a sequence.
pub fn label(binding: &str) -> gtk::Box {
    let b = gtk::Box::builder().spacing(4).valign(gtk::Align::Center).build();
    for stroke in binding.split_whitespace() {
        b.append(&gtk::ShortcutLabel::new(stroke));
    }
    b
}

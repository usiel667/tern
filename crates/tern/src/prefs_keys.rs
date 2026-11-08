//! Settings → Keyboard: Vim keys on/off and every shortcut, each changeable.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, glib};
use tern_core::keys::{self, Group, Shortcut};
use tern_sync::Command;

use crate::app::Ui;
use crate::shortcuts;

/// A Vim shortcut takes a second key pressed within this long.
const PAIR_WINDOW: Duration = Duration::from_millis(1000);

type Refreshers = Rc<RefCell<Vec<Box<dyn Fn()>>>>;

pub fn page(ui: &Rc<Ui>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("Keyboard").icon_name("input-keyboard-symbolic").build();

    let vim_group = adw::PreferencesGroup::new();
    let vim = adw::SwitchRow::builder()
        .title("Vim keys")
        .subtitle(
            "h / l move between Mailboxes, messages and the open message; j / k, gg / G and \
             Ctrl+D / Ctrl+U move inside it. Their keys are under Vim Keys below.",
        )
        .active(ui.settings.borrow().vim_keys)
        .build();
    vim_group.add(&vim);
    page.add(&vim_group);
    {
        let ui = ui.clone();
        vim.connect_active_notify(move |row| {
            ui.settings.borrow_mut().vim_keys = row.is_active();
            save(&ui);
        });
    }

    let refreshers: Refreshers = Rc::default();
    for group in Group::ALL {
        let g = adw::PreferencesGroup::builder().title(group.title()).description(description(group)).build();
        for sc in keys::SHORTCUTS.iter().filter(|s| s.group == group) {
            g.add(&row(ui, sc, &refreshers));
        }
        page.add(&g);
    }

    let reset_group = adw::PreferencesGroup::new();
    let reset_all = gtk::Button::builder().label("Reset All Shortcuts").halign(gtk::Align::Center).build();
    {
        let (ui, refreshers) = (ui.clone(), refreshers.clone());
        reset_all.connect_clicked(move |_| {
            ui.settings.borrow_mut().keys.clear();
            save(&ui);
            refresh_all(&refreshers);
        });
    }
    reset_group.add(&reset_all);
    page.add(&reset_group);
    page
}

fn description(group: Group) -> &'static str {
    match group {
        Group::App => "Work in every Tern window, also while typing, so they use Ctrl, Alt or Super.",
        Group::NewMessage => "In a New Message window.",
        Group::MessageList => "Single keys while the message list has the focus.",
        Group::Vim => "When Vim keys are on, anywhere in the main window except while typing. Two keys in a row (like g g) work too.",
    }
}

/// One shortcut: its keys (or "Off"), a reset button when changed; click to change.
fn row(ui: &Rc<Ui>, sc: &'static Shortcut, refreshers: &Refreshers) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(sc.title).activatable(true).build();
    let keys_box = gtk::Box::builder().spacing(12).valign(gtk::Align::Center).build();
    let reset = gtk::Button::builder()
        .icon_name("edit-undo-symbolic")
        .tooltip_text("Use the default")
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .build();
    row.add_suffix(&keys_box);
    row.add_suffix(&reset);

    let refresh = {
        let (ui, keys_box, reset) = (ui.clone(), keys_box.clone(), reset.clone());
        move || {
            while let Some(child) = keys_box.first_child() {
                keys_box.remove(&child);
            }
            let overrides = ui.settings.borrow().keys.clone();
            let bound = keys::bindings(sc, &overrides);
            if bound.is_empty() {
                keys_box.append(&gtk::Label::builder().label("Off").css_classes(["dim-label"]).build());
            }
            // "or" between alternatives, so L or O doesn't read like the pair g g.
            for (i, b) in bound.iter().enumerate() {
                if i > 0 {
                    keys_box.append(&gtk::Label::builder().label("or").css_classes(["dim-label"]).build());
                }
                keys_box.append(&shortcuts::label(b));
            }
            reset.set_visible(overrides.contains_key(sc.id));
        }
    };
    refresh();
    refreshers.borrow_mut().push(Box::new(refresh));
    {
        let (ui, refreshers) = (ui.clone(), refreshers.clone());
        reset.connect_clicked(move |_| {
            set(&ui, sc, None);
            refresh_all(&refreshers);
        });
    }
    {
        let (ui, refreshers) = (ui.clone(), refreshers.clone());
        row.connect_activated(move |row| capture(&ui, sc, row, &refreshers));
    }
    row
}

fn refresh_all(refreshers: &Refreshers) {
    for r in refreshers.borrow().iter() {
        r();
    }
}

/// Give a shortcut these keys (`None`: its defaults). Saved, and live at once.
fn set(ui: &Ui, sc: &Shortcut, to: Option<Vec<String>>) {
    {
        let mut s = ui.settings.borrow_mut();
        let defaults: Vec<String> = sc.defaults.iter().map(|d| keys::normalize(d)).collect();
        match to.map(|v| v.iter().map(|k| keys::normalize(k)).collect::<Vec<_>>()) {
            Some(v) if v != defaults => {
                s.keys.insert(sc.id.to_string(), v);
            }
            _ => {
                s.keys.remove(sc.id);
            }
        }
    }
    save(ui);
}

fn save(ui: &Ui) {
    shortcuts::apply_app_accels(ui);
    ui.send(Command::SaveSettings(ui.settings.borrow().clone()));
}

/// "Press the new shortcut": records the next key (or two, for Vim), says what's wrong with
/// it or what it's already used for, and sets it on Set (taking it from the others).
fn capture(ui: &Rc<Ui>, sc: &'static Shortcut, from: &adw::ActionRow, refreshers: &Refreshers) {
    let alert = adw::AlertDialog::new(Some(sc.title), Some("Press the new shortcut. Esc cancels."));
    alert.add_responses(&[("cancel", "Cancel"), ("off", "Turn Off"), ("set", "Set")]);
    alert.set_response_appearance("set", adw::ResponseAppearance::Suggested);
    alert.set_response_enabled("set", false);
    alert.set_close_response("cancel");
    alert.set_default_response(None::<&str>);

    let shown = gtk::Box::builder().spacing(8).halign(gtk::Align::Center).height_request(32).build();
    let note = gtk::Label::builder().wrap(true).justify(gtk::Justification::Center).css_classes(["dim-label"]).build();
    let extra = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
    extra.append(&shown);
    extra.append(&note);
    alert.set_extra_child(Some(&extra));

    // App shortcuts would fire before the dialog sees the key (Ctrl+Q would quit): off while open.
    for s in keys::SHORTCUTS.iter().filter(|s| s.group == Group::App) {
        ui.app.set_accels_for_action(s.id, &[]);
    }

    let captured: Rc<RefCell<String>> = Rc::default();
    let last: Rc<Cell<Option<Instant>>> = Rc::default();
    let key_ctl = gtk::EventControllerKey::new();
    key_ctl.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let (ui, alert, captured) = (ui.clone(), alert.clone(), captured.clone());
        key_ctl.connect_key_pressed(move |_, key, _, mods| {
            if key == gdk::Key::Escape && (mods & gtk::accelerator_get_default_mod_mask()).is_empty() {
                return glib::Propagation::Proceed;
            }
            let Some(stroke) = shortcuts::stroke(key, mods) else { return glib::Propagation::Proceed };
            let now = Instant::now();
            let pair = sc.group == Group::Vim
                && !captured.borrow().is_empty()
                && !captured.borrow().contains(' ')
                && last.get().is_some_and(|t| now.duration_since(t) < PAIR_WINDOW);
            let binding = if pair { format!("{} {stroke}", captured.borrow()) } else { stroke };
            last.set(Some(now));
            *captured.borrow_mut() = binding.clone();

            while let Some(child) = shown.first_child() {
                shown.remove(&child);
            }
            shown.append(&shortcuts::label(&binding));
            match keys::check(sc.group, &binding) {
                Err(why) => {
                    note.set_text(why);
                    alert.set_response_enabled("set", false);
                }
                Ok(()) => {
                    let clashes = keys::conflicts(sc.id, &binding, &ui.settings.borrow().keys);
                    let mut text = match clashes.as_slice() {
                        [] => String::new(),
                        list => format!(
                            "Used by {}. Set moves it here.",
                            list.iter().map(|c| format!("{} ({})", c.title, c.group.title())).collect::<Vec<_>>().join(", ")
                        ),
                    };
                    if sc.group == Group::Vim && !binding.contains(' ') {
                        if !text.is_empty() {
                            text.push(' ');
                        }
                        text.push_str("Press another key now for a two-key shortcut.");
                    }
                    note.set_text(&text);
                    alert.set_response_enabled("set", true);
                }
            }
            glib::Propagation::Stop
        });
    }
    alert.add_controller(key_ctl);

    let (ui, refreshers) = (ui.clone(), refreshers.clone());
    alert.connect_response(None, move |_, response| {
        match response {
            "off" => set(&ui, sc, Some(Vec::new())),
            "set" => {
                let binding = keys::normalize(&captured.borrow());
                let overrides = ui.settings.borrow().keys.clone();
                for other in keys::conflicts(sc.id, &binding, &overrides) {
                    // Take the key (or the clashing start of a pair) away from the other one.
                    let kept: Vec<String> = keys::bindings(other, &overrides)
                        .into_iter()
                        .filter(|b| !keys::conflicts(other.id, b, &[(sc.id.to_string(), vec![binding.clone()])].into()).contains(&sc))
                        .collect();
                    set(&ui, other, Some(kept));
                }
                set(&ui, sc, Some(vec![binding]));
            }
            _ => shortcuts::apply_app_accels(&ui),
        }
        refresh_all(&refreshers);
    });
    alert.present(Some(from));
}

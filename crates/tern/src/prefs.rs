//! Settings window (Ctrl+,). Pages so far: Appearance (prefs_theme.rs), Keyboard (prefs_keys.rs),
//! Accounts, Viewing, Signatures.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tern_core::account::{AccountConfig, Provider};
use tern_core::model::AccountId;
use tern_sync::Command;

use crate::app::Ui;

/// Typing in a signature saves it this long after the last key.
const SAVE_DELAY: Duration = Duration::from_millis(500);

pub fn open(ui: &Rc<Ui>) {
    // Wide enough for all five page tabs along the top.
    let dialog = adw::PreferencesDialog::builder().title("Settings").content_width(760).build();
    dialog.add(&crate::prefs_theme::page(ui));
    dialog.add(&crate::prefs_keys::page(ui));
    dialog.add(&accounts_page(ui, &dialog));
    dialog.add(&viewing_page(ui));
    dialog.add(&signatures_page(ui, &dialog));
    dialog.present(Some(&ui.window));
}

/// Apple Mail's layout, used by every per-account page: the accounts on the left (with
/// `below` under the list), `detail(account, its label in the list)` for the chosen one on
/// the right.
fn account_columns(
    ui: &Rc<Ui>,
    below: Option<&gtk::Widget>,
    mut detail: impl FnMut(&AccountConfig, &gtk::Label) -> gtk::Widget,
) -> gtk::Box {
    let list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).vexpand(true).build();
    let details = gtk::Stack::builder().hexpand(true).build();
    for (i, a) in ui.accounts.borrow().iter().enumerate() {
        let label = gtk::Label::builder().label(&a.description).xalign(0.0).margin_start(6).build();
        list.append(&label);
        details.add_named(&detail(a, &label), Some(&i.to_string()));
    }
    {
        let details = details.clone();
        list.connect_row_selected(move |_, row| {
            if let Some(r) = row {
                details.set_visible_child_name(&r.index().to_string());
            }
        });
    }
    list.select_row(list.row_at_index(0).as_ref());
    let left = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).width_request(170).build();
    left.append(&gtk::Frame::builder().child(&list).vexpand(true).build());
    if let Some(w) = below {
        left.append(w);
    }
    let row = gtk::Box::builder().spacing(12).height_request(260).build();
    row.append(&left);
    row.append(&details);
    row
}

fn provider_label(p: Provider) -> &'static str {
    match p {
        Provider::ICloud => "iCloud",
        Provider::Gmail => "Gmail",
        Provider::Fastmail => "Fastmail",
        Provider::Generic => "IMAP",
    }
}

/// Each account's names (editable), address and type, with Sign In Again and Remove.
fn accounts_page(ui: &Rc<Ui>, dialog: &adw::PreferencesDialog) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("Accounts").icon_name("avatar-default-symbolic").build();
    let group = adw::PreferencesGroup::new();
    let add = gtk::Button::builder().label("Add Account…").css_classes(["flat"]).build();
    {
        let (ui, dialog) = (ui.clone(), dialog.clone());
        add.connect_clicked(move |_| {
            dialog.close();
            let _ = WidgetExt::activate_action(&ui.window, "win.add-account", None);
        });
    }
    let columns = account_columns(ui, Some(add.upcast_ref()), |a, label| {
        let description = adw::EntryRow::builder().title("Description").text(&a.description).show_apply_button(true).build();
        let name = adw::EntryRow::builder()
            .title("Full Name")
            .text(a.identity.name.as_deref().unwrap_or(""))
            .show_apply_button(true)
            .build();
        let email = adw::ActionRow::builder().title("Email Address").subtitle(&a.identity.email).css_classes(["property"]).build();
        let kind = adw::ActionRow::builder().title("Type").subtitle(provider_label(a.provider)).css_classes(["property"]).build();
        let rows = gtk::ListBox::builder().css_classes(["boxed-list"]).selection_mode(gtk::SelectionMode::None).build();
        rows.append(&description);
        rows.append(&name);
        rows.append(&email);
        rows.append(&kind);

        let id = a.id;
        let save = {
            let (ui, description, name, label) = (ui.clone(), description.clone(), name.clone(), label.clone());
            Rc::new(move || {
                let d = description.text().trim().to_string();
                if d.is_empty() {
                    return;
                }
                let n = Some(name.text().trim().to_string()).filter(|n| !n.is_empty());
                label.set_label(&d);
                // The sidebar and New Message read `ui.accounts`; the engine saves accounts.toml.
                if let Some(acc) = ui.accounts.borrow_mut().iter_mut().find(|acc| acc.id == id) {
                    acc.description = d.clone();
                    acc.identity.name = n.clone();
                }
                ui.send(Command::SetAccountNames { account: id, description: d, name: n });
            })
        };
        for row in [&description, &name] {
            let save = save.clone();
            row.connect_apply(move |_| save());
        }

        let sign_in = gtk::Button::builder().label("Sign In Again…").build();
        {
            let (ui, dialog, id) = (ui.clone(), dialog.clone(), id.to_string());
            sign_in.connect_clicked(move |_| {
                dialog.close();
                crate::accounts::sign_in_again(&ui, &id);
            });
        }
        let remove = gtk::Button::builder().label("Remove Account…").css_classes(["destructive-action"]).build();
        {
            let (ui, dialog, label) = (ui.clone(), dialog.clone(), a.description.clone());
            remove.connect_clicked(move |b| confirm_remove(&ui, &dialog, b, id, &label));
        }
        let buttons = gtk::Box::builder().spacing(12).halign(gtk::Align::End).build();
        buttons.append(&sign_in);
        buttons.append(&remove);

        let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
        column.append(&rows);
        column.append(&buttons);
        column.upcast()
    });
    group.add(&columns);
    page.add(&group);
    page
}

/// App-wide switches for reading mail. Each takes effect at once on the open message.
fn viewing_page(ui: &Rc<Ui>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("Viewing").icon_name("view-reveal-symbolic").build();
    let s = ui.settings.borrow().clone();
    let messages = adw::PreferencesGroup::builder().title("Messages").build();
    let remote = adw::SwitchRow::builder()
        .title("Load remote content")
        .subtitle("Images and fonts from the internet, in every message. Senders can tell when you’ve opened their mail.")
        .active(s.load_remote_content)
        .build();
    let darken = adw::SwitchRow::builder()
        .title("Darken messages in dark mode")
        .subtitle("For mail without dark colors of its own. The sun button shows a message’s original colors.")
        .active(s.darken_html)
        .build();
    messages.add(&remote);
    messages.add(&darken);
    page.add(&messages);

    let save = {
        let ui = ui.clone();
        move |change: &dyn Fn(&mut tern_core::settings::Settings)| {
            change(&mut ui.settings.borrow_mut());
            let s = ui.settings.borrow().clone();
            for v in ui.viewers() {
            v.rerender(&ui);
        }
            ui.send(Command::SaveSettings(s));
        }
    };
    let save = Rc::new(save);
    {
        let save = save.clone();
        remote.connect_active_notify(move |r| save(&|s| s.load_remote_content = r.is_active()));
    }
    darken.connect_active_notify(move |r| save(&|s| s.darken_html = r.is_active()));
    page
}

/// Ask first: removing deletes Tern's copy (the mail stays on the server).
fn confirm_remove(ui: &Rc<Ui>, dialog: &adw::PreferencesDialog, from: &gtk::Button, id: AccountId, label: &str) {
    if ui.demo {
        ui.toast("Accounts can’t be changed in demo mode");
        return;
    }
    let alert = adw::AlertDialog::new(
        Some(&format!("Remove “{label}”?")),
        Some(
            "Your mail stays on the server. Tern deletes its downloaded copy, the saved password, \
             and any changes not yet sent to the server.",
        ),
    );
    alert.add_responses(&[("cancel", "Cancel"), ("remove", "Remove")]);
    alert.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    alert.set_default_response(Some("cancel"));
    alert.set_close_response("cancel");
    let (ui, dialog, label) = (ui.clone(), dialog.clone(), label.to_string());
    alert.connect_response(None, move |_, response| {
        if response != "remove" {
            return;
        }
        ui.accounts.borrow_mut().retain(|a| a.id != id);
        ui.send(Command::RemoveAccount(id));
        dialog.close();
        ui.toast(&format!("Removed “{label}”"));
    });
    alert.present(Some(from));
}

/// The chosen account's signature, with a switch to turn it off.
fn signatures_page(ui: &Rc<Ui>, dialog: &adw::PreferencesDialog) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("Signatures").icon_name("document-edit-symbolic").build();
    let group = adw::PreferencesGroup::builder()
        .description("Added to new messages, below what you write and above any quoted text.")
        .build();
    // Saves waiting for typing to pause; done at once when the window closes.
    type Saver = Rc<dyn Fn()>;
    let pending: Rc<RefCell<Vec<(AccountId, glib::SourceId, Saver)>>> = Rc::default();

    let columns = account_columns(ui, None, |a, _| {
        let switch = adw::SwitchRow::builder().title("Add a signature").active(a.signature.is_some()).build();
        let toggles = gtk::ListBox::builder().css_classes(["boxed-list"]).selection_mode(gtk::SelectionMode::None).build();
        toggles.append(&switch);
        let view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(10)
            .bottom_margin(10)
            .left_margin(12)
            .right_margin(12)
            .sensitive(a.signature.is_some())
            .vexpand(true)
            .build();
        view.buffer().set_text(a.signature.as_deref().unwrap_or(""));
        let frame = gtk::Frame::builder().child(&view).css_classes(["view"]).build();
        let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
        column.append(&toggles);
        column.append(&frame);

        let id = a.id;
        let save: Saver = {
            let ui = ui.clone();
            let (switch, buffer) = (switch.clone(), view.buffer());
            Rc::new(move || {
                let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                let text = text.trim_end();
                let signature = (switch.is_active() && !text.trim().is_empty()).then(|| text.to_string());
                // Compose reads `ui.accounts`; update it now (the engine saves accounts.toml).
                if let Some(acc) = ui.accounts.borrow_mut().iter_mut().find(|acc| acc.id == id) {
                    acc.signature = signature.clone();
                }
                ui.send(Command::SetSignature(id, signature));
            })
        };
        let save_later = {
            let (pending, save) = (pending.clone(), save.clone());
            move || {
                let mut p = pending.borrow_mut();
                if let Some(i) = p.iter().position(|(acc, ..)| *acc == id) {
                    p.remove(i).1.remove();
                }
                let (pending2, save2) = (pending.clone(), save.clone());
                let source = glib::timeout_add_local_once(SAVE_DELAY, move || {
                    pending2.borrow_mut().retain(|(acc, ..)| *acc != id);
                    save2();
                });
                p.push((id, source, save.clone()));
            }
        };
        {
            let save_later = save_later.clone();
            view.buffer().connect_changed(move |_| save_later());
        }
        switch.connect_active_notify(move |s| {
            view.set_sensitive(s.is_active());
            save_later();
        });
        column.upcast()
    });
    dialog.connect_closed(move |_| {
        let waiting: Vec<_> = pending.borrow_mut().drain(..).collect();
        for (_, source, save) in waiting {
            source.remove();
            save();
        }
    });

    group.add(&columns);
    page.add(&group);
    page
}

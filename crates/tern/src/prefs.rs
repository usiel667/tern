//! Settings window (Ctrl+,). Pages so far: Signatures.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tern_core::model::AccountId;
use tern_sync::Command;

use crate::app::Ui;

/// Typing in a signature saves it this long after the last key.
const SAVE_DELAY: Duration = Duration::from_millis(500);

pub fn open(ui: &Rc<Ui>) {
    let dialog = adw::PreferencesDialog::builder().title("Settings").build();
    dialog.add(&signatures_page(ui, &dialog));
    dialog.present(Some(&ui.window));
}

/// Apple Mail's layout: accounts on the left, the chosen account's signature on the right,
/// with a switch to turn it off.
fn signatures_page(ui: &Rc<Ui>, dialog: &adw::PreferencesDialog) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("Signatures").icon_name("document-edit-symbolic").build();
    let group = adw::PreferencesGroup::builder()
        .description("Added to new messages, below what you write and above any quoted text.")
        .build();
    let list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).width_request(170).build();
    let editors = gtk::Stack::builder().hexpand(true).build();
    // Saves waiting for typing to pause; done at once when the window closes.
    type Saver = Rc<dyn Fn()>;
    let pending: Rc<RefCell<Vec<(AccountId, glib::SourceId, Saver)>>> = Rc::default();

    for (i, a) in ui.accounts.borrow().iter().enumerate() {
        list.append(&gtk::Label::builder().label(&a.description).xalign(0.0).margin_start(6).build());

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
        editors.add_named(&column, Some(&i.to_string()));

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
    }

    {
        let editors = editors.clone();
        list.connect_row_selected(move |_, row| {
            if let Some(r) = row {
                editors.set_visible_child_name(&r.index().to_string());
            }
        });
    }
    list.select_row(list.row_at_index(0).as_ref());
    dialog.connect_closed(move |_| {
        let waiting: Vec<_> = pending.borrow_mut().drain(..).collect();
        for (_, source, save) in waiting {
            source.remove();
            save();
        }
    });

    let row = gtk::Box::builder().spacing(12).height_request(260).build();
    row.append(&gtk::Frame::builder().child(&list).build());
    row.append(&editors);
    group.add(&row);
    page.add(&group);
    page
}

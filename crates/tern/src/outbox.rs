//! Unsent Messages: messages a server refused for good (too large, recipient refused, …).
//! Each can be edited (reopened as a New Message), sent again, or deleted.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tern_core::mime::draft_from_raw;
use tern_store::OutboxItem;
use tern_sync::Command;

use crate::app::Ui;
use crate::format::format_addresses;

pub fn open(ui: &Rc<Ui>) {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .valign(gtk::Align::Start)
        .build();
    let clamp = adw::Clamp::builder()
        .maximum_size(640)
        .margin_top(12)
        .margin_bottom(18)
        .margin_start(12)
        .margin_end(12)
        .child(&list)
        .build();
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&gtk::ScrolledWindow::builder().child(&clamp).vexpand(true).build()));
    let dialog = adw::Dialog::builder()
        .title("Unsent Messages")
        .content_width(600)
        .content_height(440)
        .child(&tv)
        .build();
    *ui.unsent.borrow_mut() = Some((dialog.clone(), list));
    {
        let ui = Rc::downgrade(ui);
        dialog.connect_closed(move |_| {
            if let Some(ui) = ui.upgrade() {
                ui.unsent.borrow_mut().take();
            }
        });
    }
    refresh(ui);
    dialog.present(Some(&ui.window));
}

/// Rebuild the open dialog's rows (after a retry, delete or new refusal).
pub fn refresh(ui: &Rc<Ui>) {
    let Some((dialog, list)) = ui.unsent.borrow().clone() else { return };
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    let items = ui.store.borrow().outbox_rejected().unwrap_or_default();
    if items.is_empty() {
        list.append(
            &adw::ActionRow::builder()
                .title("No unsent messages")
                .subtitle("Messages a server refuses to send show up here.")
                .build(),
        );
        return;
    }
    for item in items {
        list.append(&row(ui, &dialog, item));
    }
}

/// How many refused messages are waiting for the user.
pub fn count(ui: &Ui) -> usize {
    ui.store.borrow().outbox_rejected().map(|v| v.len()).unwrap_or(0)
}

fn row(ui: &Rc<Ui>, dialog: &adw::Dialog, item: OutboxItem) -> adw::ActionRow {
    let rcpts = item.envelope.as_ref().map(|(_, to)| to.clone()).unwrap_or_default();
    let draft = draft_from_raw(&item.raw, &rcpts);
    let subject = draft.as_ref().map(|d| d.subject.clone()).filter(|s| !s.is_empty());
    let to = draft
        .as_ref()
        .map(|d| format_addresses(&d.to))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| rcpts.join(", "));
    let reason = item.last_error.clone().unwrap_or_else(|| "The server refused it".into());
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(subject.as_deref().unwrap_or("(No Subject)")))
        .subtitle(glib::markup_escape_text(&format!("To: {to}\n{reason}")))
        .subtitle_lines(3)
        .build();

    let button = |icon: &str, tip: &str| {
        gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(tip)
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build()
    };
    let edit = button("document-edit-symbolic", "Edit");
    let retry = button("view-refresh-symbolic", "Try Again");
    let delete = button("user-trash-symbolic", "Delete");
    edit.set_sensitive(draft.is_some());
    let id = item.id;
    {
        let (ui, dialog, account) = (ui.clone(), dialog.clone(), item.account);
        edit.connect_clicked(move |_| {
            let Some(d) = draft.clone() else { return };
            dialog.close();
            crate::compose::open_unsent(&ui, d, account);
            // Now it lives in the window; closing that unsent still asks before discarding.
            ui.send(Command::DiscardSend(id));
        });
    }
    {
        let ui = ui.clone();
        retry.connect_clicked(move |_| ui.send(Command::RetrySend(id)));
    }
    {
        let (ui, dialog) = (ui.clone(), dialog.clone());
        delete.connect_clicked(move |_| {
            let confirm = adw::AlertDialog::new(Some("Delete this message?"), Some("It hasn't been sent and can't be recovered."));
            confirm.add_response("cancel", "Cancel");
            confirm.add_response("delete", "Delete");
            confirm.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
            let ui = ui.clone();
            confirm.connect_response(Some("delete"), move |_, _| ui.send(Command::DiscardSend(id)));
            confirm.present(Some(&dialog));
        });
    }
    row.add_suffix(&edit);
    row.add_suffix(&retry);
    row.add_suffix(&delete);
    row
}

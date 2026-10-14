//! The New Message window.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use tern_core::compose::{ReplyKind, respond};
use tern_core::model::{AccountId, Address, Draft, MessageId, OutgoingAttachment};
use tern_sync::{Command, FlagChange};

use crate::app::Ui;
use crate::format::{format_addresses, mime_for, parse_addresses};

pub fn open_new(ui: &Rc<Ui>) {
    open(ui, Draft::default(), None, None);
}

pub fn open_mailto(ui: &Rc<Ui>, mailto: &str) {
    // mailto:addr?subject=...&body=...
    let (addr, query) = mailto.split_once('?').unwrap_or((mailto, ""));
    let mut draft = Draft {
        to: parse_addresses(&percent_decode(addr)).unwrap_or_default(),
        ..Default::default()
    };
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            match k.to_ascii_lowercase().as_str() {
                "subject" => draft.subject = percent_decode(v),
                "body" => draft.text = percent_decode(v),
                "cc" => draft.cc = parse_addresses(&percent_decode(v)).unwrap_or_default(),
                _ => {}
            }
        }
    }
    open(ui, draft, None, None);
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn open_response(ui: &Rc<Ui>, id: MessageId, kind: ReplyKind) {
    let Ok(m) = ui.store.borrow().message(id) else { return };
    let body = ui.store.borrow().body(id).ok().flatten();
    let Some(acct) = ui.account_for(m.account).or_else(|| ui.accounts.borrow().first().cloned()) else {
        ui.toast("Add an account first");
        return;
    };
    let me = vec![acct.identity.email.clone(), acct.username.clone()];
    let mut draft = respond(kind, &m.envelope, body.as_ref(), &acct.identity, &me);
    if kind == ReplyKind::Forward {
        // Carry the original's attachments along.
        if let Some(b) = &body {
            for (i, a) in b.attachments.iter().enumerate().filter(|(_, a)| !a.inline) {
                if let Some(data) = tern_core::mime::attachment_bytes(&b.raw, i as u32) {
                    draft.attachments.push(OutgoingAttachment {
                        filename: a.filename.clone().unwrap_or_else(|| format!("attachment-{}", i + 1)),
                        mime_type: a.mime_type.clone(),
                        data,
                    });
                }
            }
        }
    }
    let change = if kind == ReplyKind::Forward { FlagChange::Forwarded } else { FlagChange::Answered };
    open(ui, draft, Some(m.account), Some((id, change)));
}

struct Compose {
    window: adw::Window,
    from: gtk::DropDown,
    to: gtk::Entry,
    cc: gtk::Entry,
    bcc: gtk::Entry,
    subject: gtk::Entry,
    body: gtk::TextView,
    attachments: RefCell<Vec<OutgoingAttachment>>,
    attach_box: gtk::FlowBox,
    accounts: Vec<(AccountId, Address)>,
    in_reply_to: Option<String>,
    references: Vec<String>,
    responding: Option<(MessageId, FlagChange)>,
    toasts: adw::ToastOverlay,
}

fn field(label: &str, entry: &gtk::Entry) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    b.add_css_class("compose-field");
    let l = gtk::Label::builder().label(label).width_chars(8).xalign(1.0).css_classes(["dim-label"]).build();
    entry.set_hexpand(true);
    entry.set_has_frame(false);
    b.append(&l);
    b.append(entry);
    b
}

fn open(ui: &Rc<Ui>, draft: Draft, account: Option<AccountId>, responding: Option<(MessageId, FlagChange)>) {
    let accounts: Vec<(AccountId, Address)> =
        ui.accounts.borrow().iter().map(|a| (a.id, a.identity.clone())).collect();
    if accounts.is_empty() {
        ui.toast("Add an account first");
        return;
    }
    let labels: Vec<String> = accounts
        .iter()
        .map(|(_, a)| match &a.name {
            Some(n) => format!("{n} <{}>", a.email),
            None => a.email.clone(),
        })
        .collect();
    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let from = gtk::DropDown::from_strings(&label_refs);
    from.set_hexpand(true);
    if let Some(i) = account.and_then(|a| accounts.iter().position(|(id, _)| *id == a)) {
        from.set_selected(i as u32);
    }

    let to = gtk::Entry::new();
    let cc = gtk::Entry::new();
    let bcc = gtk::Entry::new();
    let subject = gtk::Entry::new();
    to.set_text(&format_addresses(&draft.to));
    cc.set_text(&format_addresses(&draft.cc));
    bcc.set_text(&format_addresses(&draft.bcc));
    subject.set_text(&draft.subject);

    let body = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .vexpand(true)
        .css_classes(["compose-body"])
        .build();
    let signature = account
        .or_else(|| accounts.first().map(|a| a.0))
        .and_then(|a| ui.account_for(a))
        .and_then(|a| a.signature);
    let mut text = draft.text.clone();
    if let Some(sig) = signature {
        // Signature above the quoted text, like Apple Mail.
        text = format!("\n\n-- \n{sig}{text}");
    }
    body.buffer().set_text(&text);
    body.buffer().place_cursor(&body.buffer().start_iter());

    let attach_box = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .max_children_per_line(4)
        .margin_start(12)
        .margin_end(12)
        .build();

    let from_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    from_row.add_css_class("compose-field");
    from_row.append(&gtk::Label::builder().label("From:").width_chars(8).xalign(1.0).css_classes(["dim-label"]).build());
    from_row.append(&from);

    let bcc_row = field("Bcc:", &bcc);
    bcc_row.set_visible(!draft.bcc.is_empty());

    let fields = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for w in [field("To:", &to), field("Cc:", &cc), bcc_row.clone(), field("Subject:", &subject), from_row] {
        // Separator lives inside the row so hiding Bcc hides its line too.
        w.set_orientation(gtk::Orientation::Vertical);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        while let Some(child) = w.first_child() {
            w.remove(&child);
            line.append(&child);
        }
        w.append(&line);
        w.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        w.remove_css_class("compose-field");
        line.add_css_class("compose-field");
        fields.append(&w);
    }
    fields.append(&attach_box);

    let scroller = gtk::ScrolledWindow::builder().child(&body).vexpand(true).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&fields);
    content.append(&scroller);

    let send = gtk::Button::builder()
        .icon_name("mail-send-symbolic")
        .tooltip_text("Send (Ctrl+Return)")
        .css_classes(["suggested-action"])
        .build();
    let attach = gtk::Button::builder().icon_name("mail-attachment-symbolic").tooltip_text("Attach Files…").build();
    let bcc_toggle = gtk::ToggleButton::builder().label("Bcc").active(!draft.bcc.is_empty()).build();
    let header = adw::HeaderBar::new();
    header.pack_start(&send);
    header.pack_end(&attach);
    header.pack_end(&bcc_toggle);
    let title = if draft.subject.is_empty() { "New Message".to_string() } else { draft.subject.clone() };

    let toasts = adw::ToastOverlay::new();
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&content));
    toasts.set_child(Some(&tv));

    let window = adw::Window::builder()
        .title(&title)
        .default_width(720)
        .default_height(620)
        .content(&toasts)
        .build();
    window.set_transient_for(None::<&gtk::Window>);
    window.set_application(Some(&ui.app));

    let c = Rc::new(Compose {
        window,
        from,
        to,
        cc,
        bcc,
        subject,
        body,
        attachments: RefCell::new(draft.attachments),
        attach_box,
        accounts,
        in_reply_to: draft.in_reply_to,
        references: draft.references,
        responding,
        toasts,
    });
    refresh_attachments(&c);

    {
        let row = bcc_row.clone();
        bcc_toggle.connect_toggled(move |b| row.set_visible(b.is_active()));
    }
    {
        let (c2, ui2) = (c.clone(), ui.clone());
        send.connect_clicked(move |_| do_send(&ui2, &c2));
    }
    {
        let c2 = c.clone();
        attach.connect_clicked(move |_| pick_files(&c2));
    }
    {
        let c2 = c.clone();
        c.subject.connect_changed(move |e| {
            let t = e.text();
            c2.window.set_title(Some(if t.is_empty() { "New Message" } else { t.as_str() }));
        });
    }
    // Ctrl+Return sends; Escape asks before discarding.
    let keys = gtk::EventControllerKey::new();
    {
        let (c2, ui2) = (c.clone(), ui.clone());
        keys.connect_key_pressed(move |_, key, _, mods| {
            if key == gdk::Key::Return && mods.contains(gdk::ModifierType::CONTROL_MASK) {
                do_send(&ui2, &c2);
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::Escape {
                c2.window.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    c.window.add_controller(keys);
    {
        let c2 = c.clone();
        c.window.connect_close_request(move |w| {
            let buf = c2.body.buffer();
            let text = buf.text(&buf.start_iter(), &buf.end_iter(), false);
            let typed = c2.to.text().len() + c2.subject.text().len() + text.trim().len();
            if typed == 0 || w.widget_name() == "sent" {
                return glib::Propagation::Proceed;
            }
            // TODO: "Save to Drafts" (APPEND to the Drafts mailbox).
            let d = adw::AlertDialog::new(Some("Discard this message?"), Some("Your changes will be lost."));
            d.add_response("cancel", "Keep Editing");
            d.add_response("discard", "Discard");
            d.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
            let w2 = w.clone();
            d.connect_response(Some("discard"), move |_, _| {
                w2.set_widget_name("sent");
                w2.close();
            });
            d.present(Some(w));
            glib::Propagation::Stop
        });
    }

    c.window.present();
    if c.to.text().is_empty() {
        c.to.grab_focus();
    } else {
        c.body.grab_focus();
    }
}

fn refresh_attachments(c: &Rc<Compose>) {
    while let Some(ch) = c.attach_box.first_child() {
        c.attach_box.remove(&ch);
    }
    for (i, a) in c.attachments.borrow().iter().enumerate() {
        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        chip.append(&gtk::Image::from_icon_name("mail-attachment-symbolic"));
        chip.append(&gtk::Label::new(Some(&a.filename)));
        let remove = gtk::Button::builder().icon_name("window-close-symbolic").css_classes(["flat", "circular"]).build();
        let c2 = c.clone();
        remove.connect_clicked(move |_| {
            c2.attachments.borrow_mut().remove(i);
            refresh_attachments(&c2);
        });
        chip.append(&remove);
        c.attach_box.append(&chip);
    }
    c.attach_box.set_visible(!c.attachments.borrow().is_empty());
}

fn pick_files(c: &Rc<Compose>) {
    let dialog = gtk::FileDialog::builder().title("Attach Files").modal(true).build();
    let c2 = c.clone();
    dialog.open_multiple(Some(&c.window), gio::Cancellable::NONE, move |res| {
        let Ok(files) = res else { return };
        for i in 0..files.n_items() {
            let Some(file) = files.item(i).and_downcast::<gio::File>() else { continue };
            let Some(path) = file.path() else { continue };
            match std::fs::read(&path) {
                Ok(data) => {
                    let filename = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    c2.attachments.borrow_mut().push(OutgoingAttachment {
                        mime_type: mime_for(&filename).into(),
                        filename,
                        data,
                    });
                }
                Err(e) => c2.toasts.add_toast(adw::Toast::new(&format!("Couldn’t read {}: {e}", path.display()))),
            }
        }
        refresh_attachments(&c2);
    });
}

fn do_send(ui: &Rc<Ui>, c: &Rc<Compose>) {
    let parse = |e: &gtk::Entry| parse_addresses(&e.text());
    let (to, cc, bcc) = match (parse(&c.to), parse(&c.cc), parse(&c.bcc)) {
        (Ok(a), Ok(b), Ok(d)) => (a, b, d),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
            c.toasts.add_toast(adw::Toast::new(&e));
            return;
        }
    };
    if to.is_empty() && cc.is_empty() && bcc.is_empty() {
        c.toasts.add_toast(adw::Toast::new("Add at least one recipient"));
        c.to.grab_focus();
        return;
    }
    let buf = c.body.buffer();
    let text = buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
    let (account, identity) = c.accounts[c.from.selected() as usize].clone();
    let draft = Draft {
        from: Some(identity),
        to,
        cc,
        bcc,
        subject: c.subject.text().to_string(),
        text,
        html: None,
        in_reply_to: c.in_reply_to.clone(),
        references: c.references.clone(),
        attachments: c.attachments.borrow().clone(),
    };
    // The signature is already in the body; don't let the engine add it again.
    ui.send(Command::Send { account, draft: Box::new(draft), responding_to: c.responding });
    c.window.set_widget_name("sent");
    c.window.close();
}

#[cfg(test)]
mod tests {
    use super::percent_decode;

    #[test]
    fn decodes_mailto_parts() {
        assert_eq!(percent_decode("Hello%20there+you%21"), "Hello there you!");
        assert_eq!(percent_decode("100%"), "100%");
    }
}

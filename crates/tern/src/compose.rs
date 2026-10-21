//! The New Message window.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use tern_core::compose::{ReplyKind, respond};
use tern_core::model::{AccountId, Address, Draft, MessageId, OutgoingAttachment};
use tern_sync::{Command, FlagChange};

use crate::app::Ui;
use crate::folder_zip::MAX_FOLDER_BYTES;
use crate::format::{
    crosses_size_limit, format_addresses, human_size, mime_for, parse_addresses, parse_uri_list, LARGE_ATTACHMENTS,
};

pub fn open_new(ui: &Rc<Ui>) {
    open(ui, Draft::default(), None, None);
}

pub fn open_mailto(ui: &Rc<Ui>, mailto: &str) {
    open(ui, mailto_draft(mailto), None, None);
}

/// A draft from a `mailto:` URI (RFC 6068), with or without the scheme. Only the fields a
/// person would fill in are read; everything else, `attach` included, is ignored so a link
/// can't attach local files or set arbitrary headers.
pub fn mailto_draft(uri: &str) -> Draft {
    let rest = match uri.get(..7) {
        Some(p) if p.eq_ignore_ascii_case("mailto:") => &uri[7..],
        _ => uri,
    };
    let (addr, query) = rest.split_once('?').unwrap_or((rest, ""));
    let addresses = |s: &str| parse_addresses(&single_line(&percent_decode(s))).unwrap_or_default();
    let mut draft = Draft { to: addresses(addr), ..Default::default() };
    for pair in query.split('&') {
        let Some((k, v)) = pair.split_once('=') else { continue };
        match percent_decode(k).to_ascii_lowercase().as_str() {
            "to" => draft.to.extend(addresses(v)),
            "cc" => draft.cc.extend(addresses(v)),
            "bcc" => draft.bcc.extend(addresses(v)),
            "subject" => draft.subject = single_line(&percent_decode(v)),
            "body" => draft.text = percent_decode(v).replace("\r\n", "\n").replace('\r', "\n"),
            "in-reply-to" => {
                let id = single_line(&percent_decode(v));
                let id = id.trim();
                if !id.is_empty() {
                    draft.in_reply_to =
                        Some(if id.starts_with('<') { id.to_string() } else { format!("<{id}>") });
                }
            }
            _ => {}
        }
    }
    draft
}

/// Puts the signature after anything already written (a mailto body) and above quoted or
/// forwarded text, like Apple Mail. Quoted text from `respond` starts with a blank line.
fn with_signature(text: &str, sig: &str) -> String {
    if text.is_empty() || text.starts_with('\n') {
        format!("\n\n-- \n{sig}{text}")
    } else {
        format!("{text}\n\n-- \n{sig}")
    }
}

/// Header values from a link must not carry line breaks.
fn single_line(s: &str) -> String {
    s.replace(['\r', '\n'], " ")
}

/// Percent-decoding per RFC 3986; `+` is a literal plus in mailto URIs (RFC 6068).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
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
    /// Folders being zipped in the background, by name; shown as busy chips.
    compressing: RefCell<Vec<String>>,
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
    let text = match signature {
        Some(sig) => with_signature(&draft.text, &sig),
        None => draft.text.clone(),
    };
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
    let attach = gtk::Button::builder()
        .icon_name("mail-attachment-symbolic")
        .tooltip_text("Attach Files… (Shift+Ctrl+A)")
        .build();
    let bcc_toggle = gtk::ToggleButton::builder().label("Bcc").active(!draft.bcc.is_empty()).build();
    let header = adw::HeaderBar::new();
    header.pack_start(&send);
    header.pack_end(&attach);
    header.pack_end(&bcc_toggle);
    let title = if draft.subject.is_empty() { "New Message".to_string() } else { draft.subject.clone() };

    // Shown while files are dragged over the window.
    let drop_hint = gtk::Label::builder()
        .label("Drop to attach")
        .css_classes(["drop-hint"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::End)
        .margin_bottom(24)
        .can_target(false)
        .visible(false)
        .build();
    let content_overlay = gtk::Overlay::builder().child(&content).build();
    content_overlay.add_overlay(&drop_hint);

    let toasts = adw::ToastOverlay::new();
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&content_overlay));
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
        compressing: RefCell::new(Vec::new()),
        attach_box,
        accounts,
        in_reply_to: draft.in_reply_to,
        references: draft.references,
        responding,
        toasts,
    });
    refresh_attachments(&c);
    {
        // Your own addresses are never suggested as recipients.
        let own: Vec<String> = ui
            .accounts
            .borrow()
            .iter()
            .flat_map(|a| [a.identity.email.to_lowercase(), a.username.to_lowercase()])
            .collect();
        for entry in [&c.to, &c.cc, &c.bcc] {
            let (ui2, own) = (ui.clone(), own.clone());
            let suggest = move |prefix: &str| {
                ui2.store
                    .borrow()
                    .suggest_addresses(prefix, 16)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|a| !own.contains(&a.email.to_lowercase()))
                    .collect()
            };
            crate::autocomplete::attach(entry, &c.window, Box::new(suggest));
        }
    }

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
        // DropTargetAsync, not DropTarget: file managers offer copy *or* move and Hyprland
        // picks move, which a copy-only DropTarget rejects outright, while a copy|move one
        // finishes the drop as a move. Here we accept both but always finish as a copy, so
        // no source ever treats the drop as "moved away".
        // Capture phase: the body TextView has its own drop target for text, and file
        // managers may offer text too, so the window has to see a file drop first.
        let files = gdk::FileList::static_type();
        let drop = gtk::DropTargetAsync::new(
            Some(gdk::ContentFormats::for_type(files)),
            gdk::DragAction::COPY | gdk::DragAction::MOVE,
        );
        drop.set_propagation_phase(gtk::PropagationPhase::Capture);
        let hover = Rc::new(move |on: bool| {
            if on {
                content_overlay.add_css_class("drop-hover");
            } else {
                content_overlay.remove_css_class("drop-hover");
            }
            drop_hint.set_visible(on);
        });
        {
            let hover = hover.clone();
            drop.connect_drag_enter(move |_, _, _, _| {
                hover(true);
                gdk::DragAction::COPY
            });
        }
        drop.connect_drag_motion(|_, _, _, _| gdk::DragAction::COPY);
        {
            let hover = hover.clone();
            drop.connect_drag_leave(move |_, _| hover(false));
        }
        let c2 = c.clone();
        drop.connect_drop(move |_, drop, _, _| {
            hover(false);
            let (c2, drop) = (c2.clone(), drop.clone());
            glib::spawn_future_local(async move {
                let Some(dropped) = dropped_files(&drop).await else {
                    drop.finish(gdk::DragAction::empty());
                    c2.toasts.add_toast(adw::Toast::new("Couldn’t read the dropped files"));
                    return;
                };
                drop.finish(gdk::DragAction::COPY);
                let (local, links): (Vec<gio::File>, Vec<gio::File>) =
                    dropped.into_iter().partition(|f| f.is_native());
                // A link dragged from a browser is text for the body, not something to download.
                for link in links {
                    c2.body.buffer().insert_at_cursor(&format!("{} ", link.uri()));
                }
                add_files(&c2, local);
            });
            true
        });
        c.window.add_controller(drop);
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
    // Shift+Ctrl+A attaches (Apple Mail's ⇧⌘A). Capture phase: text fields bind it to
    // "unselect all" and would swallow it before the window's bubble-phase handler.
    let attach_key = gtk::EventControllerKey::new();
    attach_key.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let c2 = c.clone();
        attach_key.connect_key_pressed(move |_, key, _, mods| {
            if matches!(key, gdk::Key::A | gdk::Key::a)
                && mods.contains(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK)
            {
                pick_files(&c2);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    c.window.add_controller(attach_key);
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
        chip.add_css_class("attachment-chip");
        chip.append(&gtk::Image::from_icon_name("mail-attachment-symbolic"));
        chip.append(&gtk::Label::new(Some(&a.filename)));
        chip.append(
            &gtk::Label::builder()
                .label(human_size(a.data.len() as u64))
                .css_classes(["dim-label", "caption"])
                .build(),
        );
        let remove = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Remove")
            .css_classes(["flat", "circular"])
            .build();
        let c2 = c.clone();
        remove.connect_clicked(move |_| {
            c2.attachments.borrow_mut().remove(i);
            refresh_attachments(&c2);
        });
        chip.append(&remove);
        c.attach_box.append(&chip);
    }
    for name in c.compressing.borrow().iter() {
        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        chip.add_css_class("attachment-chip");
        chip.append(&gtk::Spinner::builder().spinning(true).build());
        chip.append(&gtk::Label::new(Some(&format!("Compressing “{name}”…"))));
        c.attach_box.append(&chip);
    }
    c.attach_box.set_visible(!c.attachments.borrow().is_empty() || !c.compressing.borrow().is_empty());
}

fn pick_files(c: &Rc<Compose>) {
    let dialog = gtk::FileDialog::builder().title("Attach Files").modal(true).build();
    let c2 = c.clone();
    dialog.open_multiple(Some(&c.window), gio::Cancellable::NONE, move |res| {
        let Ok(files) = res else { return };
        let files = (0..files.n_items()).filter_map(|i| files.item(i).and_downcast::<gio::File>()).collect();
        add_files(&c2, files);
    });
}

/// The files in a drop. GTK prefers the document portal's transfer format when the source
/// offers it; if that fails (no document portal running, as on many Hyprland setups), fall
/// back to the plain `text/uri-list` every file manager also sends.
async fn dropped_files(drop: &gdk::Drop) -> Option<Vec<gio::File>> {
    if let Ok(v) = drop.read_value_future(gdk::FileList::static_type(), glib::Priority::DEFAULT).await
        && let Ok(list) = v.get::<gdk::FileList>()
    {
        return Some(list.files());
    }
    if !drop.formats().contain_mime_type("text/uri-list") {
        return None;
    }
    let (input, _) = drop.read_future(&["text/uri-list"], glib::Priority::DEFAULT).await.ok()?;
    let out = gio::MemoryOutputStream::new_resizable();
    out.splice_future(
        &input,
        gio::OutputStreamSpliceFlags::CLOSE_SOURCE | gio::OutputStreamSpliceFlags::CLOSE_TARGET,
        glib::Priority::DEFAULT,
    )
    .await
    .ok()?;
    let bytes = out.steal_as_bytes();
    let uris = parse_uri_list(&String::from_utf8_lossy(&bytes));
    (!uris.is_empty()).then(|| uris.iter().map(|u| gio::File::for_uri(u)).collect())
}

/// Read files in the background (no freeze on big ones) and attach them. Folders are
/// zipped (off the main thread) and attached as `Name.zip`; anything that can't be read
/// gets its own notice.
fn add_files(c: &Rc<Compose>, files: Vec<gio::File>) {
    if files.is_empty() {
        return;
    }
    let c = c.clone();
    glib::spawn_future_local(async move {
        let total = |c: &Compose| c.attachments.borrow().iter().map(|a| a.data.len() as u64).sum::<u64>();
        let before = total(&c);
        for file in files {
            let name = file
                .basename()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "attachment".into());
            if file.query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) == gio::FileType::Directory {
                // Email has no folders: attach it zipped, like Apple Mail.
                let Some(path) = file.path() else {
                    c.toasts.add_toast(adw::Toast::new(&format!("Couldn’t read the folder “{name}”")));
                    continue;
                };
                c.compressing.borrow_mut().push(name.clone());
                refresh_attachments(&c);
                let zipped = gio::spawn_blocking(move || crate::folder_zip::zip_folder(&path, MAX_FOLDER_BYTES)).await;
                c.compressing.borrow_mut().retain(|n| *n != name);
                match zipped {
                    Ok(Ok((filename, data))) => c.attachments.borrow_mut().push(OutgoingAttachment {
                        mime_type: "application/zip".into(),
                        filename,
                        data,
                    }),
                    Ok(Err(e)) => c.toasts.add_toast(adw::Toast::new(&format!("Couldn’t attach “{name}”: {e}"))),
                    Err(_) => c.toasts.add_toast(adw::Toast::new(&format!("Couldn’t attach “{name}”"))),
                }
                refresh_attachments(&c);
                continue;
            }
            match file.load_contents_future().await {
                Ok((data, _)) => {
                    c.attachments.borrow_mut().push(OutgoingAttachment {
                        mime_type: mime_for(&name).into(),
                        filename: name,
                        data: data.to_vec(),
                    });
                    refresh_attachments(&c);
                }
                Err(e) => c.toasts.add_toast(adw::Toast::new(&format!("Couldn’t read “{name}”: {e}"))),
            }
        }
        if crosses_size_limit(before, total(&c)) {
            c.toasts.add_toast(adw::Toast::new(&format!(
                "Attachments are over {} — many mail servers reject messages that large",
                human_size(LARGE_ATTACHMENTS)
            )));
        }
    });
}

fn do_send(ui: &Rc<Ui>, c: &Rc<Compose>) {
    if let Some(name) = c.compressing.borrow().first() {
        c.toasts.add_toast(adw::Toast::new(&format!("Still compressing “{name}” — send again in a moment")));
        return;
    }
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
    use super::{mailto_draft, percent_decode, with_signature};

    fn emails(a: &[tern_core::model::Address]) -> Vec<&str> {
        a.iter().map(|a| a.email.as_str()).collect()
    }

    #[test]
    fn decodes_percent_escapes_but_keeps_plus() {
        assert_eq!(percent_decode("Hello%20there+you%21"), "Hello there+you!");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("50%2"), "50%2");
        assert_eq!(percent_decode("caf%C3%A9"), "café");
    }

    #[test]
    fn plus_addressing_survives() {
        assert_eq!(emails(&mailto_draft("mailto:me+tag@example.com").to), ["me+tag@example.com"]);
    }

    #[test]
    fn reads_rfc6068_fields() {
        let d = mailto_draft("MAILTO:a@x.org?to=b@x.org&CC=c@x.org&bcc=d@x.org&Subject=Hi%20there&body=line%201%0D%0Aline%202&In-Reply-To=%3Cm1%40x.org%3E");
        assert_eq!(emails(&d.to), ["a@x.org", "b@x.org"]);
        assert_eq!(emails(&d.cc), ["c@x.org"]);
        assert_eq!(emails(&d.bcc), ["d@x.org"]);
        assert_eq!(d.subject, "Hi there");
        assert_eq!(d.text, "line 1\nline 2");
        assert_eq!(d.in_reply_to.as_deref(), Some("<m1@x.org>"));
    }

    #[test]
    fn works_without_scheme_or_address() {
        let d = mailto_draft("?to=a@x.org,b@x.org&subject=x");
        assert_eq!(emails(&d.to), ["a@x.org", "b@x.org"]);
        assert_eq!(mailto_draft("a@x.org").to.len(), 1);
    }

    #[test]
    fn ignores_attachments_and_other_headers() {
        let d = mailto_draft("mailto:x@y.org?attach=/home/me/.ssh/id_rsa&attachment=%2Fetc%2Fpasswd&X-Evil=1&from=boss@y.org");
        assert!(d.attachments.is_empty());
        assert!(d.from.is_none());
        assert_eq!(emails(&d.to), ["x@y.org"]);
    }

    #[test]
    fn header_fields_cannot_inject_lines() {
        let d = mailto_draft("mailto:x@y.org?subject=Hi%0D%0ABcc:%20spy@z.org");
        assert!(!d.subject.contains('\n') && !d.subject.contains('\r'));
    }

    #[test]
    fn signature_goes_between_own_text_and_quote() {
        assert_eq!(with_signature("", "Tom"), "\n\n-- \nTom");
        assert_eq!(with_signature("\n\nOn Monday Ann wrote:\n> hi", "Tom"), "\n\n-- \nTom\n\nOn Monday Ann wrote:\n> hi");
        assert_eq!(with_signature("hello\nworld", "Tom"), "hello\nworld\n\n-- \nTom");
    }
}

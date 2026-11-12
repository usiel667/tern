//! A message in a window of its own (double-click or Return in the list, or Ctrl+O), and
//! what any message view offers from its ⋯ menu: Print, Save As and View Source.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use tern_core::model::{FlagColor, MessageId};
use tern_sync::{Command, FlagChange, MoveTarget};
use webkit::prelude::*;

use crate::app::Ui;
use crate::viewer::Viewer;

/// Open message `id` in a new window.
pub fn open(ui: &Rc<Ui>, id: MessageId) {
    let Ok(m) = ui.store.borrow().message(id) else { return };
    let v = Viewer::new(true);
    let title = if m.envelope.subject.is_empty() { "(no subject)".to_string() } else { m.envelope.subject.clone() };
    let win = adw::ApplicationWindow::builder()
        .application(&ui.app)
        .title(&title)
        .default_width(900)
        .default_height(880)
        .content(&v.root)
        .build();
    crate::viewer::connect(ui, &v);
    install_view_actions(ui, &win, &v);

    // The header's buttons, for this window's message.
    let action = |name: &str, f: Box<dyn Fn(&Rc<Ui>, MessageId)>| {
        let a = gio::SimpleAction::new(name, None);
        let (weak, vw) = (Rc::downgrade(ui), Rc::downgrade(&v));
        a.connect_activate(move |_, _| {
            if let (Some(ui), Some(id)) = (weak.upgrade(), vw.upgrade().and_then(|v| v.current())) {
                f(&ui, id);
            }
        });
        win.add_action(&a);
    };
    use tern_core::compose::ReplyKind;
    action("reply", Box::new(|ui, id| crate::compose::open_response(ui, id, ReplyKind::Reply)));
    action("reply-all", Box::new(|ui, id| crate::compose::open_response(ui, id, ReplyKind::ReplyAll)));
    action("forward", Box::new(|ui, id| crate::compose::open_response(ui, id, ReplyKind::Forward)));
    action("toggle-flag", Box::new(|ui, id| {
        let flagged = ui.store.borrow().message(id).is_ok_and(|m| m.envelope.flags.flag.is_some());
        ui.send(Command::SetFlags(vec![id], FlagChange::Flag((!flagged).then_some(FlagColor::Red))));
    }));
    action("toggle-read", Box::new(|ui, id| {
        let seen = ui.store.borrow().message(id).is_ok_and(|m| m.envelope.flags.seen);
        ui.send(Command::SetFlags(vec![id], FlagChange::Seen(!seen)));
    }));
    // Moving the message away closes its window, as in Apple Mail.
    for (name, target) in [("archive", MoveTarget::Archive), ("delete", MoveTarget::Trash), ("junk", MoveTarget::Junk)] {
        let w = win.downgrade();
        action(name, Box::new(move |ui, id| {
            ui.send(Command::Move(vec![id], target));
            if let Some(w) = w.upgrade() {
                w.close();
            }
        }));
    }
    close_on_escape(&win);

    ui.message_windows.borrow_mut().push((win.clone(), v.clone()));
    let weak = Rc::downgrade(ui);
    win.connect_close_request(move |w| {
        if let Some(ui) = weak.upgrade() {
            ui.message_windows.borrow_mut().retain(|(x, _)| x != w);
        }
        glib::Propagation::Proceed
    });
    v.show(ui, id);
    win.present();
    // On the message, so the keyboard scrolls it (not on the selectable sender line).
    v.focus();
}

/// Ctrl+W and Escape close the window.
fn close_on_escape(win: &impl IsA<gtk::Widget>) {
    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("<Control>w|Escape"),
        Some(gtk::NamedAction::new("window.close")),
    ));
    win.add_controller(keys);
}

/// Print, Save As and View Source on `win`, for the message `v` shows.
pub fn install_view_actions(ui: &Rc<Ui>, win: &impl IsA<gio::ActionMap>, v: &Rc<Viewer>) {
    let add = |name: &str, f: fn(&Rc<Ui>, &Rc<Viewer>)| {
        let a = gio::SimpleAction::new(name, None);
        let (weak, vw) = (Rc::downgrade(ui), Rc::downgrade(v));
        a.connect_activate(move |_, _| {
            if let (Some(ui), Some(v)) = (weak.upgrade(), vw.upgrade()) {
                f(&ui, &v);
            }
        });
        win.add_action(&a);
    };
    add("print", print);
    add("save-message", save_message);
    add("view-source", view_source);
}

/// The open message's id and raw bytes, or a notice why there are none yet.
fn current_raw(ui: &Rc<Ui>, v: &Viewer) -> Option<(MessageId, tern_core::model::MessageBody)> {
    let Some(id) = v.current() else {
        ui.toast("Select a message first");
        return None;
    };
    match ui.store.borrow().body(id).ok().flatten() {
        Some(b) => Some((id, b)),
        None => {
            ui.toast("The message is still downloading");
            None
        }
    }
}

/// Print the message (light colours, its headers on top) with GTK's print dialog, which can
/// also save a PDF. A page of its own is laid out for it, so the view doesn't change.
pub fn print(ui: &Rc<Ui>, v: &Rc<Viewer>) {
    let Some((id, body)) = current_raw(ui, v) else { return };
    let Ok(m) = ui.store.borrow().message(id) else { return };
    let converted = v.heic_pictures(ui, id, &body);
    let html = crate::format::print_document(&body, &m.envelope, v.remote_allowed(ui, id), &converted);

    let page = webkit::WebView::new();
    if let Some(s) = WebViewExt::settings(&page) {
        s.set_enable_javascript(false);
    }
    // Same rule as the view: nothing navigates.
    page.connect_decide_policy(|_, decision, kind| {
        use webkit::{NavigationPolicyDecision, PolicyDecisionType};
        if !matches!(kind, PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction) {
            return false;
        }
        let uri = decision
            .downcast_ref::<NavigationPolicyDecision>()
            .and_then(|n| n.navigation_action())
            .and_then(|a| a.request())
            .and_then(|r| r.uri())
            .unwrap_or_default();
        if uri == "about:blank" {
            return false;
        }
        decision.ignore();
        true
    });
    let (window, vw) = (v.window(ui), Rc::downgrade(v));
    page.connect_load_changed(move |page, event| {
        if event != webkit::LoadEvent::Finished {
            return;
        }
        let op = webkit::PrintOperation::new(page);
        let done = vw.clone();
        op.connect_finished(move |_| {
            if let Some(v) = done.upgrade() {
                v.printing.take();
            }
        });
        let failed = vw.clone();
        op.connect_failed(move |_, e| {
            tracing::warn!("printing failed: {e}");
            if let Some(v) = failed.upgrade() {
                v.printing.take();
            }
        });
        if op.run_dialog(Some(&window)) == webkit::PrintOperationResponse::Cancel
            && let Some(v) = vw.upgrade()
        {
            v.printing.take();
        }
    });
    page.load_html(&html, Some("about:blank"));
    v.printing.replace(Some(page));
}

/// Save the whole message as an .eml file (what Apple Mail calls Raw Message Source).
pub fn save_message(ui: &Rc<Ui>, v: &Rc<Viewer>) {
    let Some((id, body)) = current_raw(ui, v) else { return };
    let subject = ui.store.borrow().message(id).map(|m| m.envelope.subject).unwrap_or_default();
    let name = format!("{}.eml", tern_core::mime::safe_file_name(&subject, "Message"));
    let dialog = gtk::FileDialog::builder().initial_name(&name).modal(true).build();
    let weak = Rc::downgrade(ui);
    dialog.save(Some(&v.window(ui)), gio::Cancellable::NONE, move |res| {
        let (Some(ui), Ok(file)) = (weak.upgrade(), res) else { return };
        let Some(path) = file.path() else { return };
        match std::fs::write(&path, &body.raw) {
            Ok(()) => ui.toast(&format!("Saved {}", path.display())),
            Err(e) => ui.toast(&format!("Couldn’t save: {e}")),
        }
    });
}

/// Most of a message's source shown at once; a bigger one (big attachments) is cut, with a
/// note, as a text view that size gets slow. Save As keeps all of it.
const SOURCE_SHOWN: usize = 2 * 1024 * 1024;

/// The message exactly as it came from the server: every header, then the MIME parts.
pub fn view_source(ui: &Rc<Ui>, v: &Rc<Viewer>) {
    let Some((id, body)) = current_raw(ui, v) else { return };
    let subject = ui.store.borrow().message(id).map(|m| m.envelope.subject).unwrap_or_default();
    let mut text = String::from_utf8_lossy(&body.raw[..body.raw.len().min(SOURCE_SHOWN)]).into_owned();
    if body.raw.len() > SOURCE_SHOWN {
        text.push_str(&format!(
            "\n\n[… {} more not shown. Save As keeps the whole message.]\n",
            crate::format::human_size((body.raw.len() - SOURCE_SHOWN) as u64)
        ));
    }
    let view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .build();
    view.buffer().set_text(&text);
    let scroll = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("Message Source", &subject))
        .build();
    let save = gtk::Button::builder().icon_name("document-save-symbolic").tooltip_text("Save As…").build();
    header.pack_start(&save);
    let content = adw::ToolbarView::new();
    content.add_top_bar(&header);
    content.set_content(Some(&scroll));
    let win = adw::Window::builder()
        .transient_for(&v.window(ui))
        .title("Message Source")
        .default_width(900)
        .default_height(720)
        .content(&content)
        .build();
    let (weak, vw) = (Rc::downgrade(ui), Rc::downgrade(v));
    save.connect_clicked(move |_| {
        if let (Some(ui), Some(v)) = (weak.upgrade(), vw.upgrade()) {
            save_message(&ui, &v);
        }
    });
    close_on_escape(&win);
    win.present();
    view.grab_focus();
}

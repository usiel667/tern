//! Right pane: the selected message (and its conversation).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib, pango};
use tern_core::model::{MessageBody, MessageId, MessageSummary};
use tern_sync::{Command, FlagChange};
use webkit::prelude::*;

use crate::app::Ui;
use crate::format::{full_date, human_size, render_body};

pub struct Viewer {
    pub root: adw::ToolbarView,
    stack: gtk::Stack,
    status: adw::StatusPage,
    thread_list: gtk::ListBox,
    thread_revealer: gtk::Revealer,
    avatar: adw::Avatar,
    from: gtk::Label,
    date: gtk::Label,
    to: gtk::Label,
    subject: gtk::Label,
    remote_banner: adw::Banner,
    unsub_banner: adw::Banner,
    attachments: gtk::FlowBox,
    webview: webkit::WebView,
    loading: gtk::Spinner,
    current: Cell<Option<MessageId>>,
    allow_remote: RefCell<HashSet<MessageId>>,
    unsubscribe: RefCell<Option<String>>,
}

fn header_button(icon: &str, tip: &str, action: &str) -> gtk::Button {
    gtk::Button::builder().icon_name(icon).tooltip_text(tip).action_name(action).build()
}

impl Viewer {
    pub fn new() -> Self {
        let header = adw::HeaderBar::new();
        header.set_show_title(false);
        header.pack_start(&header_button("tern-archive-symbolic", "Archive (Ctrl+Alt+A)", "win.archive"));
        header.pack_start(&header_button("user-trash-symbolic", "Delete (Delete)", "win.delete"));
        header.pack_start(&header_button("mail-mark-junk-symbolic", "Junk (Ctrl+Shift+J)", "win.junk"));
        header.pack_end(&header_button("mail-forward-symbolic", "Forward (Ctrl+Shift+F)", "win.forward"));
        header.pack_end(&header_button("mail-reply-all-symbolic", "Reply All (Ctrl+Shift+R)", "win.reply-all"));
        header.pack_end(&header_button("mail-reply-sender-symbolic", "Reply (Ctrl+R)", "win.reply"));
        header.pack_end(&header_button("starred-symbolic", "Flag (Ctrl+Shift+L)", "win.toggle-flag"));

        let avatar = adw::Avatar::new(44, None, true);
        avatar.set_valign(gtk::Align::Start);
        let label = |classes: &[&str]| {
            gtk::Label::builder()
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(pango::WrapMode::WordChar)
                .selectable(true)
                .css_classes(classes.to_vec())
                .build()
        };
        let from = label(&["viewer-from"]);
        from.set_hexpand(true);
        let date = label(&["dim-label"]);
        date.set_wrap(false);
        date.set_valign(gtk::Align::Start);
        let subject = label(&["viewer-subject"]);
        let to = label(&["dim-label"]);
        to.set_ellipsize(pango::EllipsizeMode::End);
        to.set_wrap(false);

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        top.append(&from);
        top.append(&date);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.append(&top);
        text.append(&subject);
        text.append(&to);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        head.add_css_class("viewer-header");
        head.append(&avatar);
        head.append(&text);

        let remote_banner = adw::Banner::new("This message contains remote content.");
        remote_banner.set_button_label(Some("Load Remote Content"));
        let unsub_banner = adw::Banner::new("This message is from a mailing list.");
        unsub_banner.set_button_label(Some("Unsubscribe"));

        let attachments = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(4)
            .row_spacing(6)
            .column_spacing(6)
            .margin_start(24)
            .margin_end(24)
            .margin_bottom(8)
            .build();

        let webview = webkit::WebView::new();
        if let Some(settings) = WebViewExt::settings(&webview) {
            // Mail never runs scripts. Remote loads are governed by CSP (format.rs).
            settings.set_enable_javascript(false);
            settings.set_enable_javascript_markup(false);
            settings.set_enable_developer_extras(cfg!(debug_assertions));
        }
        webview.set_vexpand(true);

        let loading = gtk::Spinner::builder().halign(gtk::Align::Center).margin_top(24).build();

        let thread_list = gtk::ListBox::new();
        thread_list.add_css_class("boxed-list");
        thread_list.set_margin_start(16);
        thread_list.set_margin_end(16);
        thread_list.set_margin_top(8);
        let thread_revealer = gtk::Revealer::builder().child(&thread_list).build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&thread_revealer);
        content.append(&head);
        content.append(&remote_banner);
        content.append(&unsub_banner);
        content.append(&attachments);
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        content.append(&loading);
        content.append(&webview);

        let status = adw::StatusPage::builder()
            .icon_name("mail-read-symbolic")
            .title("No Message Selected")
            .build();

        let stack = gtk::Stack::new();
        stack.add_named(&status, Some("empty"));
        stack.add_named(&content, Some("message"));

        let root = adw::ToolbarView::new();
        root.add_top_bar(&header);
        root.set_content(Some(&stack));

        Self {
            root,
            stack,
            status,
            thread_list,
            thread_revealer,
            avatar,
            from,
            date,
            to,
            subject,
            remote_banner,
            unsub_banner,
            attachments,
            webview,
            loading,
            current: Cell::new(None),
            allow_remote: RefCell::new(HashSet::new()),
            unsubscribe: RefCell::new(None),
        }
    }

    pub fn current(&self) -> Option<MessageId> {
        self.current.get()
    }

    pub fn clear(&self) {
        self.current.set(None);
        self.status.set_title("No Message Selected");
        self.stack.set_visible_child_name("empty");
    }

    pub fn show_multiple(&self, n: usize) {
        self.current.set(None);
        self.status.set_title(&format!("{n} Messages Selected"));
        self.stack.set_visible_child_name("empty");
    }

    pub fn show(&self, ui: &Rc<Ui>, id: MessageId) {
        let Ok(m) = ui.store.borrow().message(id) else { return self.clear() };
        let same = self.current.get() == Some(id);
        self.current.set(Some(id));
        self.stack.set_visible_child_name("message");

        let e = &m.envelope;
        let sender = e.from.first();
        let name = sender.map(|a| a.display().to_string()).unwrap_or_default();
        self.avatar.set_text(Some(&name));
        self.from.set_label(&match sender {
            Some(a) if a.name.is_some() => format!("{name}  <{}>", a.email),
            _ => name.clone(),
        });
        self.date.set_label(&full_date(e.date));
        self.subject.set_label(if e.subject.is_empty() { "(no subject)" } else { &e.subject });
        let mut to = format!("To: {}", e.to.iter().map(|a| a.display()).collect::<Vec<_>>().join(", "));
        if !e.cc.is_empty() {
            to.push_str(&format!("   Cc: {}", e.cc.iter().map(|a| a.display()).collect::<Vec<_>>().join(", ")));
        }
        self.to.set_label(&to);
        self.to.set_tooltip_text(Some(&to));

        self.fill_thread(ui, &m);

        if !same {
            self.remote_banner.set_revealed(false);
            self.unsub_banner.set_revealed(false);
            while let Some(c) = self.attachments.first_child() {
                self.attachments.remove(&c);
            }
        }

        // Body: from cache, or ask the engine and show a spinner.
        let body = ui.store.borrow().body(id).ok().flatten();
        match body {
            Some(b) => self.render(ui, id, &b),
            None => {
                self.loading.set_visible(true);
                self.loading.set_spinning(true);
                self.webview.set_visible(false);
                ui.send(Command::FetchBody(id));
            }
        }

        // Apple Mail marks a message read when it's displayed.
        if !e.flags.seen {
            ui.send(Command::SetFlags(vec![id], FlagChange::Seen(true)));
        }
    }

    pub fn body_ready(&self, ui: &Rc<Ui>, id: MessageId) {
        if self.current.get() != Some(id) {
            return;
        }
        if let Ok(Some(b)) = ui.store.borrow().body(id) {
            self.render(ui, id, &b);
        }
    }

    fn render(&self, ui: &Rc<Ui>, id: MessageId, body: &MessageBody) {
        self.loading.set_visible(false);
        self.loading.set_spinning(false);
        self.webview.set_visible(true);
        let allow = self.allow_remote.borrow().contains(&id);
        let rendered = render_body(body, allow, ui.is_dark());
        self.remote_banner.set_revealed(rendered.blocked_remote);
        let bg = if body.html.is_some() || !ui.is_dark() { "#ffffff" } else { "#1e1e1e" };
        if let Ok(rgba) = gdk::RGBA::parse(bg) {
            self.webview.set_background_color(&rgba);
        }
        self.webview.load_html(&rendered.html, Some("about:blank"));

        *self.unsubscribe.borrow_mut() = body.list_unsubscribe.clone();
        self.unsub_banner.set_revealed(body.list_unsubscribe.is_some());

        while let Some(c) = self.attachments.first_child() {
            self.attachments.remove(&c);
        }
        for (i, a) in body.attachments.iter().enumerate().filter(|(_, a)| !a.inline) {
            let name = a.filename.clone().unwrap_or_else(|| format!("attachment-{}", i + 1));
            let chip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            chip.append(&gtk::Image::from_icon_name("mail-attachment-symbolic"));
            chip.append(&gtk::Label::new(Some(&format!("{name}  ·  {}", human_size(a.size)))));
            let btn = gtk::Button::builder().child(&chip).css_classes(["attachment-chip"]).build();
            let (weak, raw) = (Rc::downgrade(ui), body.raw.clone());
            btn.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    save_attachment(&ui, raw.clone(), i as u32, name.clone());
                }
            });
            self.attachments.append(&btn);
        }
        self.attachments.set_visible(self.attachments.first_child().is_some());
    }

    fn fill_thread(&self, ui: &Rc<Ui>, m: &MessageSummary) {
        while let Some(c) = self.thread_list.first_child() {
            self.thread_list.remove(&c);
        }
        let msgs = match (ui.conversations.get(), m.thread_id) {
            (true, Some(tid)) => ui.store.borrow().thread(tid).unwrap_or_default(),
            _ => vec![],
        };
        if msgs.len() < 2 {
            self.thread_revealer.set_reveal_child(false);
            return;
        }
        // Newest first, like Apple Mail's default.
        for t in msgs.iter().rev() {
            let who = t.envelope.from.first().map(|a| a.display().to_string()).unwrap_or_default();
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&who).as_str())
                .subtitle(glib::markup_escape_text(t.envelope.preview.as_deref().unwrap_or("")).as_str())
                .subtitle_lines(1)
                .activatable(t.id != m.id)
                .build();
            row.add_suffix(&gtk::Label::builder()
                .label(crate::format::list_date(t.envelope.date, chrono::Local::now()))
                .css_classes(["dim-label"])
                .build());
            if t.id == m.id {
                row.add_prefix(&gtk::Image::from_icon_name("go-next-symbolic"));
            }
            let (weak, id) = (Rc::downgrade(ui), t.id);
            row.connect_activated(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.viewer.show(&ui, id);
                }
            });
            self.thread_list.append(&row);
        }
        self.thread_revealer.set_reveal_child(true);
    }
}

fn save_attachment(ui: &Rc<Ui>, raw: Vec<u8>, index: u32, name: String) {
    let dialog = gtk::FileDialog::builder().initial_name(&name).modal(true).build();
    let weak = Rc::downgrade(ui);
    dialog.save(Some(&ui.window), gio::Cancellable::NONE, move |res| {
        let (Some(ui), Ok(file)) = (weak.upgrade(), res) else { return };
        let Some(path) = file.path() else { return };
        match tern_core::mime::attachment_bytes(&raw, index) {
            Some(bytes) => match std::fs::write(&path, bytes) {
                Ok(()) => ui.toast(&format!("Saved {}", path.display())),
                Err(e) => ui.toast(&format!("Couldn’t save: {e}")),
            },
            None => ui.toast("Attachment not found in message"),
        }
    });
}

fn open_uri(window: &impl IsA<gtk::Window>, uri: &str) {
    gtk::UriLauncher::new(uri).launch(Some(window), gio::Cancellable::NONE, |_| {});
}

pub fn connect(ui: &Rc<Ui>) {
    let v = &ui.viewer;

    // Links open in the default browser; the view itself never navigates.
    {
        let weak = Rc::downgrade(ui);
        v.webview.connect_decide_policy(move |_, decision, kind| {
            use webkit::{NavigationPolicyDecision, NavigationType, PolicyDecisionType};
            if !matches!(kind, PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction) {
                return false;
            }
            let Some(nav) = decision.downcast_ref::<NavigationPolicyDecision>() else { return false };
            let Some(action) = nav.navigation_action() else { return false };
            let uri = action.request().and_then(|r| r.uri()).map(|u| u.to_string()).unwrap_or_default();
            if action.navigation_type() == NavigationType::Other && uri == "about:blank" {
                return false; // our own load_html
            }
            decision.ignore();
            if let Some(ui) = weak.upgrade()
                && (uri.starts_with("http") || uri.starts_with("mailto:")) {
                    if let Some(addr) = uri.strip_prefix("mailto:") {
                        crate::compose::open_mailto(&ui, addr);
                    } else {
                        open_uri(&ui.window, &uri);
                    }
                }
            true
        });
    }
    {
        let weak = Rc::downgrade(ui);
        v.remote_banner.connect_button_clicked(move |_| {
            let Some(ui) = weak.upgrade() else { return };
            if let Some(id) = ui.viewer.current() {
                ui.viewer.allow_remote.borrow_mut().insert(id);
                if let Ok(Some(b)) = ui.store.borrow().body(id) {
                    ui.viewer.render(&ui, id, &b);
                }
            }
        });
    }
    {
        let weak = Rc::downgrade(ui);
        v.unsub_banner.connect_button_clicked(move |b| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(target) = ui.viewer.unsubscribe.borrow().clone() else { return };
            let target = target.trim_matches(['<', '>']).to_string();
            let confirm = adw::AlertDialog::new(
                Some("Unsubscribe?"),
                Some(&format!("Tern will open:\n{target}")),
            );
            confirm.add_response("cancel", "Cancel");
            confirm.add_response("go", "Unsubscribe");
            confirm.set_response_appearance("go", adw::ResponseAppearance::Destructive);
            let (w, banner) = (Rc::downgrade(&ui), b.clone());
            confirm.connect_response(Some("go"), move |_, _| {
                let Some(ui) = w.upgrade() else { return };
                // TODO: RFC 8058 one-click POST when List-Unsubscribe-Post is present.
                if let Some(addr) = target.strip_prefix("mailto:") {
                    crate::compose::open_mailto(&ui, addr);
                } else {
                    open_uri(&ui.window, &target);
                }
                banner.set_revealed(false);
            });
            confirm.present(Some(&ui.window));
        });
    }
    // Re-render on light/dark switch.
    {
        let weak = Rc::downgrade(ui);
        adw::StyleManager::default().connect_dark_notify(move |_| {
            let Some(ui) = weak.upgrade() else { return };
            if let Some(id) = ui.viewer.current()
                && let Ok(Some(b)) = ui.store.borrow().body(id) {
                    ui.viewer.render(&ui, id, &b);
                }
        });
    }
}

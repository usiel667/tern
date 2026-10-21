//! Window assembly, shared UI state, actions/shortcuts, and the engine event loop.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use tern_core::account::AccountConfig;
use tern_core::model::*;
use tern_store::{ListScope, Store};
use tern_sync::{Command, EngineHandle, Event, FlagChange, MoveTarget, Paths};

use crate::message_list::MessageList;
use crate::sidebar::Sidebar;
use crate::viewer::Viewer;
use crate::{Args, accounts, compose};

pub struct Ui {
    pub app: adw::Application,
    pub window: adw::ApplicationWindow,
    pub engine: EngineHandle,
    /// Read-only connection; the engine owns the writer.
    pub store: RefCell<Store>,
    pub toasts: adw::ToastOverlay,
    pub outer: adw::OverlaySplitView,
    pub split: adw::NavigationSplitView,
    pub sidebar: Sidebar,
    pub list: MessageList,
    pub viewer: Viewer,
    pub scope: RefCell<ListScope>,
    pub scope_title: RefCell<String>,
    pub conversations: Cell<bool>,
    pub query: RefCell<String>,
    pub accounts: RefCell<Vec<AccountConfig>>,
    pub demo: bool,
    /// Each account's message size limit, as its server reported it.
    pub send_limits: RefCell<std::collections::HashMap<AccountId, u64>>,
    /// The Unsent Messages dialog and its list, while open.
    pub unsent: RefCell<Option<(adw::Dialog, gtk::ListBox)>>,
    reload_pending: Cell<bool>,
    sidebar_pending: Cell<bool>,
    busy: Cell<u32>,
}

pub fn load_css() {
    gio::resources_register_include!("tern.gresource").expect("bundled resources");
    let display = gdk::Display::default().expect("display");
    gtk::IconTheme::for_display(&display).add_resource_path("/dev/tern/Tern/icons");
    let provider = gtk::CssProvider::new();
    provider.load_from_resource("/dev/tern/Tern/style.css");
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Builds the main window; `None` when startup failed (an error dialog is shown instead).
pub fn build(app: &adw::Application, args: Args) -> Option<Rc<Ui>> {
    let (engine, demo) = match start_engine(&args) {
        Ok(e) => e,
        Err(e) => {
            let d = adw::AlertDialog::new(Some("Tern couldn’t start"), Some(&e));
            d.add_response("ok", "Quit");
            let win = adw::ApplicationWindow::new(app);
            let a = app.clone();
            d.connect_response(None, move |_, _| a.quit());
            win.present();
            d.present(Some(&win));
            return None;
        }
    };
    let store = match Store::open(&engine.paths.database) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot open mail cache: {e}");
            app.quit();
            return None;
        }
    };
    let accounts = if demo {
        vec![tern_sync::demo::demo_account()]
    } else {
        engine.paths.load_accounts().map(|f| f.accounts).unwrap_or_default()
    };

    let sidebar = Sidebar::new();
    let list = MessageList::new();
    let viewer = Viewer::new();

    let split = adw::NavigationSplitView::new();
    split.set_sidebar(Some(&adw::NavigationPage::new(&list.root, "Messages")));
    split.set_content(Some(&adw::NavigationPage::new(&viewer.root, "Message")));
    split.set_min_sidebar_width(320.0);
    split.set_max_sidebar_width(480.0);
    split.set_sidebar_width_fraction(0.38);

    let outer = adw::OverlaySplitView::new();
    outer.set_sidebar(Some(&sidebar.root));
    outer.set_content(Some(&split));
    outer.set_min_sidebar_width(200.0);
    outer.set_max_sidebar_width(260.0);

    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&outer));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Tern")
        .default_width(1280)
        .default_height(820)
        .width_request(360)
        .height_request(400)
        .content(&toasts)
        .build();

    // Collapse panes on narrow windows (tiling layouts), like Apple Mail on iPad.
    let bp = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        860.0,
        adw::LengthUnit::Sp,
    ));
    bp.add_setter(&outer, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(bp);
    let bp2 = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        620.0,
        adw::LengthUnit::Sp,
    ));
    bp2.add_setter(&outer, "collapsed", Some(&true.to_value()));
    bp2.add_setter(&split, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(bp2);

    let ui = Rc::new(Ui {
        app: app.clone(),
        window,
        engine,
        store: RefCell::new(store),
        toasts,
        outer,
        split,
        sidebar,
        list,
        viewer,
        scope: RefCell::new(ListScope::Role(MailboxRole::Inbox)),
        scope_title: RefCell::new("All Inboxes".into()),
        conversations: Cell::new(true),
        query: RefCell::new(String::new()),
        accounts: RefCell::new(accounts),
        demo,
        send_limits: Default::default(),
        unsent: RefCell::new(None),
        reload_pending: Cell::new(false),
        sidebar_pending: Cell::new(false),
        busy: Cell::new(0),
    });

    crate::sidebar::connect(&ui);
    crate::message_list::connect(&ui);
    crate::viewer::connect(&ui);
    install_actions(&ui);
    spawn_event_loop(&ui);

    ui.sidebar.rebuild(&ui);
    ui.reload_list();
    ui.window.present();

    if !demo && ui.accounts.borrow().is_empty() {
        ui.list.show_welcome(true);
    }
    // Messages refused in an earlier session are still waiting for a decision.
    match crate::outbox::count(&ui) {
        0 => {}
        n => ui.toast_review(&if n == 1 {
            "A message couldn’t be sent".to_string()
        } else {
            format!("{n} messages couldn’t be sent")
        }),
    }
    Some(ui)
}

fn start_engine(args: &Args) -> Result<(EngineHandle, bool), String> {
    if args.demo {
        let dir = std::env::temp_dir().join(format!("tern-demo-{}", std::process::id()));
        let paths = Paths::in_dir(dir);
        return tern_sync::demo::spawn_demo(paths).map(|h| (h, true)).map_err(|e| e.to_string());
    }
    let paths = match &args.profile {
        Some(dir) => Paths::in_dir(dir),
        None => Paths::xdg().map_err(|e| e.to_string())?,
    };
    // Make sure the database exists before the UI opens its reader.
    tern_store::Store::open(&paths.database).map_err(|e| e.to_string())?;
    tern_sync::spawn(paths).map(|h| (h, false)).map_err(|e| e.to_string())
}

impl Ui {
    pub fn send(&self, c: Command) {
        self.engine.send(c);
    }

    pub fn toast(&self, msg: &str) {
        let t = adw::Toast::new(msg);
        t.set_timeout(4);
        self.toasts.add_toast(t);
    }

    /// A notice with a Review button that opens Unsent Messages.
    pub fn toast_review(&self, msg: &str) {
        let t = adw::Toast::builder()
            .title(glib::markup_escape_text(msg))
            .button_label("Review")
            .action_name("win.unsent")
            .timeout(10)
            .build();
        self.toasts.add_toast(t);
    }

    pub fn is_dark(&self) -> bool {
        adw::StyleManager::default().is_dark()
    }

    pub fn set_scope(self: &Rc<Self>, scope: ListScope, title: &str) {
        if *self.scope.borrow() == scope {
            return;
        }
        *self.scope.borrow_mut() = scope.clone();
        *self.scope_title.borrow_mut() = title.to_string();
        self.list.selection.unselect_all();
        self.list.reset_paging();
        self.reload_list();
        self.viewer.clear();
        if let ListScope::Mailbox(id) = scope {
            self.send(Command::SyncMailbox(id));
        }
        if self.outer.is_collapsed() {
            self.outer.set_show_sidebar(false);
        }
    }

    /// Coalesce bursts of change events into one reload per main-loop turn.
    pub fn schedule_reload(self: &Rc<Self>) {
        if self.reload_pending.replace(true) {
            return;
        }
        let ui = self.clone();
        glib::idle_add_local_once(move || {
            ui.reload_pending.set(false);
            ui.reload_list();
        });
    }

    pub fn schedule_sidebar(self: &Rc<Self>) {
        if self.sidebar_pending.replace(true) {
            return;
        }
        let ui = self.clone();
        glib::idle_add_local_once(move || {
            ui.sidebar_pending.set(false);
            ui.sidebar.rebuild(&ui);
            ui.update_title();
        });
    }

    pub fn reload_list(self: &Rc<Self>) {
        crate::message_list::reload(self);
        self.update_title();
    }

    fn update_title(&self) {
        let unread: u32 = self
            .store
            .borrow()
            .mailboxes(None)
            .unwrap_or_default()
            .iter()
            .filter(|m| m.remote.role == MailboxRole::Inbox)
            .map(|m| m.unread)
            .sum();
        self.window.set_title(Some(&if unread > 0 {
            format!("Tern — {unread} unread")
        } else {
            "Tern".into()
        }));
    }

    /// Message ids the current selection acts on. A conversation row expands to
    /// its messages in the same mailbox, as in Apple Mail.
    pub fn selected_ids(&self) -> Vec<MessageId> {
        let rows = crate::message_list::selected(self);
        let store = self.store.borrow();
        let mut out = Vec::new();
        for m in rows {
            match (self.conversations.get(), m.thread_id) {
                (true, Some(tid)) if m.thread_count > 1 => out.extend(
                    store
                        .thread(tid)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|t| t.mailbox == m.mailbox)
                        .map(|t| t.id),
                ),
                _ => out.push(m.id),
            }
        }
        out.dedup();
        out
    }

    pub fn account_for(&self, id: AccountId) -> Option<AccountConfig> {
        self.accounts.borrow().iter().find(|a| a.id == id).cloned()
    }

    fn set_busy(&self, delta: i32) {
        let n = (self.busy.get() as i32 + delta).max(0) as u32;
        self.busy.set(n);
        self.list.set_syncing(n > 0);
    }
}

fn spawn_event_loop(ui: &Rc<Ui>) {
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        while let Ok(ev) = ui.engine.events.recv().await {
            handle_event(&ui, ev);
        }
    });
}

fn handle_event(ui: &Rc<Ui>, ev: Event) {
    match ev {
        Event::AccountsChanged => {
            if !ui.demo {
                let accts = ui.engine.paths.load_accounts().map(|f| f.accounts).unwrap_or_default();
                ui.list.show_welcome(accts.is_empty());
                *ui.accounts.borrow_mut() = accts;
            }
            ui.schedule_sidebar();
        }
        Event::MailboxesChanged => ui.schedule_sidebar(),
        Event::MessagesChanged(_) => ui.schedule_reload(),
        Event::BodyReady(id) => ui.viewer.body_ready(ui, id),
        Event::SyncStarted(_) => ui.set_busy(1),
        Event::SyncFinished(_) => ui.set_busy(-1),
        Event::NewMail { from, subject, id, .. } => {
            if !ui.window.is_active() {
                let n = gio::Notification::new(&from);
                n.set_body(Some(&subject));
                n.set_default_action_and_target_value("app.show-message", Some(&id.0.to_variant()));
                ui.app.send_notification(Some(&format!("msg-{}", id.0)), &n);
            }
        }
        Event::Sent => ui.toast("Message sent"),
        Event::SendLimit { account, bytes } => {
            let mut limits = ui.send_limits.borrow_mut();
            match bytes {
                Some(n) => limits.insert(account, n),
                None => limits.remove(&account),
            };
        }
        Event::SendRejected { subject, reason, .. } => {
            let subject = if subject.is_empty() { "your message".to_string() } else { format!("“{subject}”") };
            ui.toast_review(&format!("Couldn’t send {subject}: {reason}"));
        }
        Event::OutboxChanged => crate::outbox::refresh(ui),
        Event::Error { account, message } => {
            let prefix = account
                .and_then(|a| ui.account_for(a))
                .map(|a| format!("{}: ", a.description))
                .unwrap_or_default();
            ui.toast(&format!("{prefix}{message}"));
        }
    }
}

fn install_actions(ui: &Rc<Ui>) {
    let win = &ui.window;
    let app = &ui.app;

    let simple = |name: &str, f: Box<dyn Fn(&Rc<Ui>)>| {
        let a = gio::SimpleAction::new(name, None);
        let ui = ui.clone();
        a.connect_activate(move |_, _| f(&ui));
        win.add_action(&a);
    };

    simple("compose", Box::new(compose::open_new));
    simple("reply", Box::new(|ui| respond(ui, tern_core::compose::ReplyKind::Reply)));
    simple("reply-all", Box::new(|ui| respond(ui, tern_core::compose::ReplyKind::ReplyAll)));
    simple("forward", Box::new(|ui| respond(ui, tern_core::compose::ReplyKind::Forward)));
    simple("get-mail", Box::new(|ui| ui.send(Command::SyncAll)));
    simple("delete", Box::new(|ui| move_selected(ui, MoveTarget::Trash)));
    simple("archive", Box::new(|ui| move_selected(ui, MoveTarget::Archive)));
    simple("junk", Box::new(|ui| move_selected(ui, MoveTarget::Junk)));
    simple("not-junk", Box::new(|ui| move_selected(ui, MoveTarget::Inbox)));
    simple("toggle-read", Box::new(toggle_read));
    simple("toggle-flag", Box::new(toggle_flag));
    simple("search", Box::new(|ui| ui.list.focus_search()));
    simple("next-message", Box::new(|ui| crate::message_list::step(ui, 1)));
    simple("previous-message", Box::new(|ui| crate::message_list::step(ui, -1)));
    simple("add-account", Box::new(accounts::open));
    simple("unsent", Box::new(crate::outbox::open));
    simple("toggle-sidebar", Box::new(|ui| ui.outer.set_show_sidebar(!ui.outer.shows_sidebar())));
    simple("goto-inbox", Box::new(|ui| ui.sidebar.select_scope(ui, &ListScope::Role(MailboxRole::Inbox))));
    simple("about", Box::new(|ui| {
        let about = adw::AboutDialog::builder()
            .application_name("Tern")
            .application_icon("mail-unread-symbolic")
            .version(env!("CARGO_PKG_VERSION"))
            .comments("An Apple Mail-style email client for Linux")
            .license_type(gtk::License::Gpl30)
            .build();
        about.present(Some(&ui.window));
    }));

    // Flag colour: win.flag('red'|...|'none')
    let flag = gio::SimpleAction::new("flag", Some(glib::VariantTy::STRING));
    {
        let ui = ui.clone();
        flag.connect_activate(move |_, p| {
            let name: String = p.and_then(|v| v.get()).unwrap_or_default();
            let color = FlagColor::ALL.into_iter().find(|c| c.css_class() == format!("flag-{name}"));
            ui.send(Command::SetFlags(ui.selected_ids(), FlagChange::Flag(color)));
        });
    }
    win.add_action(&flag);

    // View ▸ Organize by Conversation (stateful).
    let conv = gio::SimpleAction::new_stateful("conversations", None, &true.to_variant());
    {
        let ui = ui.clone();
        conv.connect_activate(move |a, _| {
            let on = !a.state().and_then(|s| s.get::<bool>()).unwrap_or(true);
            a.set_state(&on.to_variant());
            ui.conversations.set(on);
            ui.reload_list();
        });
    }
    win.add_action(&conv);

    // Notification click: app.show-message(id)
    let show = gio::SimpleAction::new("show-message", Some(glib::VariantTy::INT64));
    {
        let ui = ui.clone();
        show.connect_activate(move |_, p| {
            if let Some(id) = p.and_then(|v| v.get::<i64>()) {
                ui.window.present();
                ui.viewer.show(&ui, MessageId(id));
            }
        });
    }
    app.add_action(&show);

    let quit = gio::SimpleAction::new("quit", None);
    {
        let ui = ui.clone();
        quit.connect_activate(move |_, _| {
            ui.send(Command::Shutdown);
            ui.app.quit();
        });
    }
    app.add_action(&quit);

    // Apple Mail shortcuts with ⌘ → Ctrl. Single keys (Delete, j/k) live on the
    // message list so they never fire while typing in a text field.
    let accels: &[(&str, &[&str])] = &[
        ("win.compose", &["<Control>n"]),
        ("win.reply", &["<Control>r"]),
        ("win.reply-all", &["<Control><Shift>r"]),
        ("win.forward", &["<Control><Shift>f"]),
        ("win.get-mail", &["<Control><Shift>n"]),
        ("win.toggle-read", &["<Control><Shift>u"]),
        ("win.toggle-flag", &["<Control><Shift>l"]),
        ("win.junk", &["<Control><Shift>j"]),
        ("win.archive", &["<Control><Alt>a"]),
        ("win.delete", &["<Control>BackSpace"]),
        ("win.search", &["<Control>f", "<Control><Alt>f"]),
        ("win.goto-inbox", &["<Control>1"]),
        ("win.toggle-sidebar", &["<Control><Shift>m"]),
        ("app.quit", &["<Control>q"]),
    ];
    for (action, keys) in accels {
        app.set_accels_for_action(action, keys);
    }
}

fn respond(ui: &Rc<Ui>, kind: tern_core::compose::ReplyKind) {
    let Some(id) = ui.viewer.current() else {
        ui.toast("Select a message first");
        return;
    };
    compose::open_response(ui, id, kind);
}

fn move_selected(ui: &Rc<Ui>, target: MoveTarget) {
    let ids = ui.selected_ids();
    if ids.is_empty() {
        return;
    }
    // Move selection to the next message first, like Apple Mail.
    crate::message_list::select_after_removal(ui);
    ui.send(Command::Move(ids, target));
}

fn toggle_read(ui: &Rc<Ui>) {
    let rows = crate::message_list::selected(ui);
    // If any selected is unread, mark all read; otherwise mark all unread.
    let any_unread = rows.iter().any(|m| !m.envelope.flags.seen);
    ui.send(Command::SetFlags(ui.selected_ids(), FlagChange::Seen(any_unread)));
}

fn toggle_flag(ui: &Rc<Ui>) {
    let rows = crate::message_list::selected(ui);
    let any_unflagged = rows.iter().any(|m| m.envelope.flags.flag.is_none());
    let color = any_unflagged.then_some(FlagColor::Red);
    ui.send(Command::SetFlags(ui.selected_ids(), FlagChange::Flag(color)));
}

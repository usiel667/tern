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

/// A main-thread step slower than this gets a log line (it holds up drawing and input).
const SLOW_STEP: std::time::Duration = std::time::Duration::from_millis(30);

pub(crate) fn timed<T>(step: &str, f: impl FnOnce() -> T) -> T {
    let started = std::time::Instant::now();
    let out = f();
    let took = started.elapsed();
    if took >= SLOW_STEP {
        tracing::info!(step, ?took, "slow ui step");
    }
    out
}

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
    /// App settings (Settings window); the engine saves them to settings.toml.
    pub settings: RefCell<tern_core::settings::Settings>,
    pub demo: bool,
    /// Each account's message size limit, as its server reported it.
    pub send_limits: RefCell<std::collections::HashMap<AccountId, u64>>,
    /// The Unsent Messages dialog and its list, while open.
    pub unsent: RefCell<Option<(adw::Dialog, gtk::ListBox)>>,
    /// The "couldn't sign in" toast while it's up, so retries don't stack more.
    signin_toast: Rc<RefCell<Option<adw::Toast>>>,
    /// Moves that can still be undone, oldest first, with their Undo toasts (Ctrl+Z takes
    /// back the newest).
    undo: RefCell<Vec<(u64, adw::Toast)>>,
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
    // The dark look (Tokyo Night) has to beat GTK themes that hard-code widget colours at USER
    // priority, so it sits just above them, and only while the app is dark.
    let dark = gtk::CssProvider::new();
    dark.load_from_resource("/dev/tern/Tern/tokyo-night.css");
    let styles = adw::StyleManager::default();
    let apply = move |sm: &adw::StyleManager| {
        if sm.is_dark() {
            gtk::style_context_add_provider_for_display(&display, &dark, gtk::STYLE_PROVIDER_PRIORITY_USER + 1);
        } else {
            gtk::style_context_remove_provider_for_display(&display, &dark);
        }
    };
    apply(&styles);
    styles.connect_dark_notify(apply);
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
    let settings = if demo { Default::default() } else { engine.paths.load_settings() };

    let sidebar = Sidebar::new();
    let list = MessageList::new();
    let viewer = Viewer::new();

    let split = adw::NavigationSplitView::new();
    let (list_column, list_handle) = with_resize_handle(&list.root);
    split.set_sidebar(Some(&adw::NavigationPage::new(&list_column, "Messages")));
    split.set_content(Some(&adw::NavigationPage::new(&viewer.root, "Message")));
    split.set_min_sidebar_width(320.0);
    split.set_max_sidebar_width(480.0);
    split.set_sidebar_width_fraction(0.38);

    let outer = adw::OverlaySplitView::new();
    let (mailboxes_column, mailboxes_handle) = with_resize_handle(&sidebar.root);
    outer.set_sidebar(Some(&mailboxes_column));
    outer.set_content(Some(&split));
    outer.set_min_sidebar_width(200.0);
    outer.set_max_sidebar_width(260.0);
    outer
        .bind_property("show-sidebar", &list.sidebar_btn, "tooltip-text")
        .transform_to(|_, shown: bool| {
            Some(if shown { "Hide Mailboxes (Ctrl+Shift+M)" } else { "Show Mailboxes (Ctrl+Shift+M)" })
        })
        .sync_create()
        .build();

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
    window.add_css_class("tern-main");

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
        settings: RefCell::new(settings),
        demo,
        send_limits: Default::default(),
        unsent: RefCell::new(None),
        signin_toast: Default::default(),
        undo: Default::default(),
        reload_pending: Cell::new(false),
        sidebar_pending: Cell::new(false),
        busy: Cell::new(0),
    });

    crate::sidebar::connect(&ui);
    crate::message_list::connect(&ui);
    apply_zoom(ui.settings.borrow().zoom_percent);
    let widths = (ui.settings.borrow().mailboxes_width, ui.settings.borrow().list_width);
    if let Some(w) = widths.0 {
        set_column_width(&ui, Column::Mailboxes, w as f64);
    }
    if let Some(w) = widths.1 {
        set_column_width(&ui, Column::List, w as f64);
    }
    connect_resize_handle(&ui, &mailboxes_handle, &mailboxes_column, Column::Mailboxes);
    connect_resize_handle(&ui, &list_handle, &list_column, Column::List);
    crate::viewer::connect(&ui);
    crate::vim::connect(&ui);
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

    /// Whether a message is in a Drafts mailbox (and so opens for editing).
    pub fn is_draft(&self, id: MessageId) -> bool {
        let store = self.store.borrow();
        store
            .message(id)
            .and_then(|m| store.mailbox(m.mailbox))
            .is_ok_and(|mb| mb.remote.role == MailboxRole::Drafts)
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

    /// A sign-in failure stays up until dismissed, with a button that fixes it.
    fn toast_sign_in(&self, account: AccountId, msg: &str) {
        if let Some(t) = self.signin_toast.borrow().as_ref() {
            t.set_title(&glib::markup_escape_text(msg));
            return;
        }
        let t = adw::Toast::builder()
            .title(glib::markup_escape_text(msg))
            .button_label("Sign In…")
            .action_name("win.sign-in")
            .action_target(&account.to_string().to_variant())
            .timeout(0)
            .build();
        let slot = self.signin_toast.clone();
        t.connect_dismissed(move |_| *slot.borrow_mut() = None);
        *self.signin_toast.borrow_mut() = Some(t.clone());
        self.toasts.add_toast(t);
    }

    /// New credentials work: take down the sign-in toast.
    pub fn signed_in(&self) {
        let toast = self.signin_toast.borrow_mut().take();
        if let Some(t) = toast {
            t.dismiss();
        }
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
        timed("mailbox switch", || self.reload_list());
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
            timed("list reload", || ui.reload_list());
        });
    }

    pub fn schedule_sidebar(self: &Rc<Self>) {
        if self.sidebar_pending.replace(true) {
            return;
        }
        let ui = self.clone();
        glib::idle_add_local_once(move || {
            ui.sidebar_pending.set(false);
            timed("sidebar rebuild", || {
                ui.sidebar.rebuild(&ui);
                ui.update_title();
            });
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
        Event::SendUndone(key) => compose::send_undone(&key),
        Event::Moved { key, count, to } => moved_toast(ui, key, count, to.as_deref()),
        Event::UndoExpired(key) => {
            forget_undo(ui, key);
        }
        Event::MoveUndone { key, ids } => {
            forget_undo(ui, key);
            // Show the messages again right away (reloads are otherwise batched) and select them.
            ui.reload_list();
            crate::message_list::select_ids(ui, &ids);
        }
        Event::SendReleased(key) => compose::send_released(&key),
        Event::Error { account, message } => {
            let prefix = account
                .and_then(|a| ui.account_for(a))
                .map(|a| format!("{}: ", a.description))
                .unwrap_or_default();
            ui.toast(&format!("{prefix}{message}"));
        }
        Event::SignInFailed { account, message } => {
            let name = ui.account_for(account).map(|a| a.identity.email).unwrap_or_default();
            ui.toast_sign_in(account, &format!("Couldn’t sign in to {name}: {message}"));
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
    simple("undo", Box::new(|ui| {
        // App shortcuts fire before the focused widget sees the key: in a text field Ctrl+Z
        // undoes typing, not the last move.
        if let Some(text) = editing_text(ui) {
            let _ = text.activate_action("text.undo", None);
            return;
        }
        let last = ui.undo.borrow().last().map(|(key, _)| *key);
        if let Some(key) = last {
            ui.send(Command::UndoMove(key));
        }
    }));
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
    simple("preferences", Box::new(crate::prefs::open));
    simple("zoom-in", Box::new(|ui| zoom(ui, 1)));
    simple("zoom-out", Box::new(|ui| zoom(ui, -1)));
    simple("zoom-reset", Box::new(|ui| zoom(ui, 0)));
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

    // Sign in again: win.sign-in(account id), or "" to choose the account.
    let sign_in = gio::SimpleAction::new("sign-in", Some(glib::VariantTy::STRING));
    {
        let ui = ui.clone();
        sign_in.connect_activate(move |_, p| {
            let id = p.and_then(|v| v.get::<String>()).unwrap_or_default();
            accounts::sign_in_again(&ui, &id);
        });
    }
    win.add_action(&sign_in);

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
        ("win.preferences", &["<Control>comma"]),
        ("win.zoom-in", &["<Control>equal", "<Control>plus", "<Control>KP_Add"]),
        ("win.zoom-out", &["<Control>minus", "<Control>KP_Subtract"]),
        ("win.zoom-reset", &["<Control>0", "<Control>KP_0"]),
        ("win.reply", &["<Control>r"]),
        ("win.reply-all", &["<Control><Shift>r"]),
        ("win.forward", &["<Control><Shift>f"]),
        ("win.get-mail", &["<Control><Shift>n"]),
        ("win.toggle-read", &["<Control><Shift>u"]),
        ("win.toggle-flag", &["<Control><Shift>l"]),
        ("win.junk", &["<Control><Shift>j"]),
        ("win.archive", &["<Control><Alt>a"]),
        ("win.delete", &["<Control>BackSpace"]),
        ("win.undo", &["<Control>z"]),
        ("win.search", &["<Control>f", "<Control><Alt>f"]),
        ("win.goto-inbox", &["<Control>1"]),
        ("win.toggle-sidebar", &["<Control><Shift>m"]),
        ("app.quit", &["<Control>q"]),
    ];
    for (action, keys) in accels {
        app.set_accels_for_action(action, keys);
    }
}

/// The main window's three toolbars (Mailboxes, message list, message) are kept the same
/// height, so the line under them (style.css) is one straight line at any zoom.
pub fn same_toolbar_height(header: &adw::HeaderBar) {
    thread_local! {
        static TOOLBARS: gtk::SizeGroup = gtk::SizeGroup::new(gtk::SizeGroupMode::Vertical);
    }
    TOOLBARS.with(|g| g.add_widget(header));
}

#[derive(Clone, Copy)]
enum Column {
    Mailboxes,
    List,
}

/// `column` with a thin strip on its right edge that the mouse drags to resize it (the split
/// views have no handle of their own). Narrow enough to stay clear of the scrollbar.
fn with_resize_handle(column: &impl IsA<gtk::Widget>) -> (gtk::Overlay, gtk::Box) {
    let handle = gtk::Box::builder()
        .width_request(4)
        .halign(gtk::Align::End)
        .cursor(&gtk::gdk::Cursor::from_name("col-resize", None).expect("cursor"))
        .css_classes(["column-resize"])
        .build();
    let overlay = gtk::Overlay::builder().child(column).build();
    overlay.add_overlay(&handle);
    (overlay, handle)
}

/// Sizes a column to `width` pixels (its pane's min and max both).
fn set_column_width(ui: &Ui, column: Column, width: f64) {
    match column {
        Column::Mailboxes => {
            let w = width.clamp(160.0, 480.0);
            ui.outer.set_sidebar_width_unit(adw::LengthUnit::Px);
            ui.outer.set_min_sidebar_width(w);
            ui.outer.set_max_sidebar_width(w);
        }
        Column::List => {
            let w = width.clamp(260.0, 900.0);
            ui.split.set_sidebar_width_unit(adw::LengthUnit::Px);
            ui.split.set_min_sidebar_width(w);
            ui.split.set_max_sidebar_width(w);
        }
    }
}

/// Dragging the handle resizes the column live; letting go saves the width in the settings.
/// Positions are taken in window coordinates: the handle moves with the edge it drags.
fn connect_resize_handle(ui: &Rc<Ui>, handle: &gtk::Box, column: &gtk::Overlay, which: Column) {
    let drag = gtk::GestureDrag::new();
    // (column width, pointer x in the window) when the drag began
    let start = Rc::new(std::cell::Cell::new((0.0, 0.0)));
    let window_x = {
        let (handle, ui) = (handle.clone(), Rc::downgrade(ui));
        move |g: &gtk::GestureDrag| -> Option<f64> {
            let (x, y) = g.point(None)?;
            let ui = ui.upgrade()?;
            handle.compute_point(&ui.window, &gtk::graphene::Point::new(x as f32, y as f32)).map(|p| p.x() as f64)
        }
    };
    {
        let (start, column, window_x) = (start.clone(), column.clone(), window_x.clone());
        drag.connect_drag_begin(move |g, _, _| {
            if let Some(x) = window_x(g) {
                start.set((column.width() as f64, x));
            }
        });
    }
    {
        let (ui, start) = (Rc::downgrade(ui), start.clone());
        drag.connect_drag_update(move |g, _, _| {
            if let (Some(ui), Some(x)) = (ui.upgrade(), window_x(g)) {
                let (w, x0) = start.get();
                set_column_width(&ui, which, w + x - x0);
            }
        });
    }
    {
        let (ui, column) = (Rc::downgrade(ui), column.clone());
        drag.connect_drag_end(move |_, _, _| {
            let Some(ui) = ui.upgrade() else { return };
            let w = Some(column.width().max(0) as u32);
            match which {
                Column::Mailboxes => ui.settings.borrow_mut().mailboxes_width = w,
                Column::List => ui.settings.borrow_mut().list_width = w,
            }
            ui.send(Command::SaveSettings(ui.settings.borrow().clone()));
        });
    }
    handle.add_controller(drag);
}

/// Sizes Ctrl+= and Ctrl+- step through, in percent.
const ZOOM_STEPS: &[u32] = &[70, 80, 90, 100, 110, 125, 150, 175, 200];

/// One step bigger (`1`), smaller (`-1`), or back to 100% (`0`); saved in the settings.
fn zoom(ui: &Rc<Ui>, step: i32) {
    let now = ui.settings.borrow().zoom_percent;
    let next = match step {
        0 => 100,
        s if s > 0 => ZOOM_STEPS.iter().copied().find(|&z| z > now).unwrap_or(now),
        _ => ZOOM_STEPS.iter().rev().copied().find(|&z| z < now).unwrap_or(now),
    };
    apply_zoom(next);
    ui.settings.borrow_mut().zoom_percent = next;
    ui.send(Command::SaveSettings(ui.settings.borrow().clone()));
}

/// Scales every window of the app (text and everything sized by it, messages included) by
/// changing this process's font DPI. The desktop's own value is read once, before any change.
pub fn apply_zoom(percent: u32) {
    thread_local! {
        static DESKTOP_DPI: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
    }
    let Some(settings) = gtk::Settings::default() else { return };
    let desktop = DESKTOP_DPI.with(|d| {
        d.get().unwrap_or_else(|| {
            let dpi = Some(settings.gtk_xft_dpi()).filter(|v| *v > 0).unwrap_or(96 * 1024);
            d.set(Some(dpi));
            dpi
        })
    });
    settings.set_gtk_xft_dpi((desktop as i64 * percent as i64 / 100) as i32);
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

/// The text field (entry or text view) being typed in, if any.
pub fn editing_text(ui: &Ui) -> Option<gtk::Widget> {
    gtk::prelude::GtkWindowExt::focus(&ui.window).filter(|w| w.is::<gtk::Text>() || w.is::<gtk::TextView>())
}

/// "Moved to Trash" with an Undo button, up for the engine's Undo window.
fn moved_toast(ui: &Rc<Ui>, key: u64, count: usize, to: Option<&str>) {
    let title = match (count, to) {
        (1, Some(to)) => format!("Moved to {to}"),
        (n, Some(to)) => format!("{n} messages moved to {to}"),
        (1, None) => "Message deleted".to_string(),
        (n, None) => format!("{n} messages deleted"),
    };
    let toast = adw::Toast::builder()
        .title(glib::markup_escape_text(&title))
        .button_label("Undo")
        .timeout(tern_sync::UNDO_WINDOW.as_secs() as u32)
        .build();
    {
        let ui = ui.clone();
        toast.connect_button_clicked(move |_| ui.send(Command::UndoMove(key)));
    }
    ui.toasts.add_toast(toast.clone());
    ui.undo.borrow_mut().push((key, toast));
}

/// The move can't be undone any more (or just was): drop it and its toast.
fn forget_undo(ui: &Ui, key: u64) {
    let gone = {
        let mut undo = ui.undo.borrow_mut();
        undo.iter().position(|(k, _)| *k == key).map(|i| undo.remove(i).1)
    };
    if let Some(toast) = gone {
        toast.dismiss();
    }
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

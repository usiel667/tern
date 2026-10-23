//! Mailbox sidebar: Favorites, Smart Mailboxes, then each account's folders.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib, pango};
use tern_core::model::{Mailbox, MailboxId, MailboxRole, MessageId};
use tern_store::ListScope;
use tern_sync::{Command, MoveTarget};

use crate::app::Ui;

pub struct Sidebar {
    pub root: adw::ToolbarView,
    listbox: gtk::ListBox,
    /// Row → what selecting it shows. `None` for headers and \Noselect folders.
    rows: RefCell<Vec<(gtk::ListBoxRow, Option<(ListScope, String)>)>>,
    rebuilding: std::cell::Cell<bool>,
}

impl Sidebar {
    pub fn new() -> Self {
        let listbox = gtk::ListBox::new();
        listbox.set_selection_mode(gtk::SelectionMode::Single);
        listbox.add_css_class("navigation-sidebar");

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&listbox)
            .build();

        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&adw::WindowTitle::new("Mailboxes", "")));
        let menu = gtk::gio::Menu::new();
        menu.append(Some("Add Account…"), Some("win.add-account"));
        menu.append(Some("Unsent Messages…"), Some("win.unsent"));
        let view = gtk::gio::Menu::new();
        view.append(Some("Organize by Conversation"), Some("win.conversations"));
        menu.append_section(None, &view);
        let about = gtk::gio::Menu::new();
        about.append(Some("About Tern"), Some("win.about"));
        about.append(Some("Quit"), Some("app.quit"));
        menu.append_section(None, &about);
        let menu_btn = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .tooltip_text("Main Menu")
            .build();
        header.pack_end(&menu_btn);

        let root = adw::ToolbarView::new();
        root.add_top_bar(&header);
        root.set_content(Some(&scroller));

        Self { root, listbox, rows: RefCell::new(vec![]), rebuilding: false.into() }
    }

    /// Recreate every row from the store, keeping the current selection.
    pub fn rebuild(&self, ui: &Rc<Ui>) {
        self.rebuilding.set(true);
        self.listbox.remove_all();
        self.rows.borrow_mut().clear();

        let mailboxes = ui.store.borrow().mailboxes(None).unwrap_or_default();
        let sum = |role: MailboxRole, unread: bool| -> u32 {
            mailboxes
                .iter()
                .filter(|m| m.remote.role == role)
                .map(|m| if unread { m.unread } else { m.total })
                .sum()
        };
        // Counted by the store so a message in several mailboxes (Gmail's All Mail) counts once.
        let flagged = ui.store.borrow().count(&ListScope::Flagged).unwrap_or(0);
        // The server's unread counts (most mail isn't downloaded). An account with All Mail
        // (Gmail) counts each message there once; others add up their mailboxes.
        let mut unread_total = 0;
        for account in mailboxes.iter().map(|m| m.account).collect::<std::collections::BTreeSet<_>>() {
            let own = || mailboxes.iter().filter(move |m| m.account == account);
            unread_total += match own().find(|m| m.remote.role == MailboxRole::All) {
                Some(all) => all.unread,
                None => own()
                    .filter(|m| !matches!(m.remote.role, MailboxRole::Trash | MailboxRole::Junk))
                    .map(|m| m.unread)
                    .sum(),
            };
        }

        self.section("Favorites");
        self.item(0, "tern-inbox-symbolic", "All Inboxes", sum(MailboxRole::Inbox, true), Some(ListScope::Role(MailboxRole::Inbox)), None);
        self.item(0, "starred-symbolic", "Flagged", flagged, Some(ListScope::Flagged), None);
        self.item(0, "document-edit-symbolic", "Drafts", sum(MailboxRole::Drafts, false), Some(ListScope::Role(MailboxRole::Drafts)), None);
        self.item(0, "mail-send-symbolic", "Sent", 0, Some(ListScope::Role(MailboxRole::Sent)), None);

        self.section("Smart Mailboxes");
        self.item(0, "mail-unread-symbolic", "Unread", unread_total, Some(ListScope::Unread), None);

        let accounts = ui.accounts.borrow().clone();
        let mut by_account: HashMap<_, Vec<&Mailbox>> = HashMap::new();
        for m in &mailboxes {
            by_account.entry(m.account).or_default().push(m);
        }
        for acct in &accounts {
            let Some(boxes) = by_account.get(&acct.id) else {
                self.section(&acct.description);
                continue;
            };
            self.section(&acct.description);
            for (m, depth) in tree_order(boxes) {
                let count = match m.remote.role {
                    MailboxRole::Drafts => m.total,
                    MailboxRole::Sent | MailboxRole::Trash | MailboxRole::All | MailboxRole::Archive => 0,
                    _ => m.unread,
                };
                let scope = m.remote.selectable.then_some(ListScope::Mailbox(m.id));
                self.item(depth, m.remote.role.icon_name(), &m.remote.name, count, scope, Some(m.id));
            }
        }

        let current = ui.scope.borrow().clone();
        let found = self
            .rows
            .borrow()
            .iter()
            .find(|(_, s)| s.as_ref().is_some_and(|(sc, _)| *sc == current))
            .map(|(r, _)| r.clone());
        if let Some(row) = found {
            self.listbox.select_row(Some(&row));
        }
        self.rebuilding.set(false);
    }

    pub fn select_scope(&self, ui: &Rc<Ui>, scope: &ListScope) {
        let found = self
            .rows
            .borrow()
            .iter()
            .find(|(_, s)| s.as_ref().is_some_and(|(sc, _)| sc == scope))
            .map(|(r, s)| (r.clone(), s.clone().unwrap().1));
        if let Some((row, title)) = found {
            self.listbox.select_row(Some(&row));
            ui.set_scope(scope.clone(), &title);
        }
    }

    fn section(&self, title: &str) {
        let label = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .css_classes(["sidebar-section"])
            .build();
        let row = gtk::ListBoxRow::builder().child(&label).selectable(false).activatable(false).build();
        self.listbox.append(&row);
        self.rows.borrow_mut().push((row, None));
    }

    fn item(&self, depth: usize, icon: &str, name: &str, count: u32, scope: Option<ListScope>, drop_target: Option<MailboxId>) {
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hbox.set_margin_start(4 + depth as i32 * 14);
        hbox.add_css_class("mailbox-row");
        hbox.append(&gtk::Image::from_icon_name(icon));
        hbox.append(
            &gtk::Label::builder()
                .label(name)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(pango::EllipsizeMode::End)
                .build(),
        );
        if count > 0 {
            hbox.append(&gtk::Label::builder().label(count.to_string()).css_classes(["badge"]).build());
        }
        let row = gtk::ListBoxRow::builder().child(&hbox).selectable(scope.is_some()).build();
        if scope.is_none() {
            row.set_sensitive(true);
            row.set_activatable(false);
            hbox.set_opacity(0.7);
        }
        if let Some(target) = drop_target {
            add_drop_target(&row, target);
        }
        self.listbox.append(&row);
        self.rows.borrow_mut().push((row, scope.map(|s| (s, name.to_string()))));
    }
}

/// Parent-before-child order with depth, siblings sorted by role then name.
fn tree_order<'a>(boxes: &[&'a Mailbox]) -> Vec<(&'a Mailbox, usize)> {
    let mut children: HashMap<Option<&str>, Vec<&'a Mailbox>> = HashMap::new();
    let ids: Vec<&str> = boxes.iter().map(|m| m.remote.remote_id.as_str()).collect();
    for m in boxes {
        // Orphans (parent not listed) go to the top level.
        let parent = m.remote.parent_remote_id.as_deref().filter(|p| ids.contains(p));
        children.entry(parent).or_default().push(m);
    }
    for v in children.values_mut() {
        v.sort_by_key(|m| (m.remote.role.sidebar_order(), m.remote.name.to_lowercase()));
    }
    let mut out = Vec::new();
    fn walk<'a>(
        key: Option<&str>,
        depth: usize,
        children: &HashMap<Option<&str>, Vec<&'a Mailbox>>,
        out: &mut Vec<(&'a Mailbox, usize)>,
    ) {
        if depth > 16 {
            return;
        }
        for m in children.get(&key).into_iter().flatten() {
            out.push((m, depth));
            walk(Some(m.remote.remote_id.as_str()), depth + 1, children, out);
        }
    }
    walk(None, 0, &children, &mut out);
    out
}

/// Accept messages dragged from the list (payload `tern-msgs:1,2,3`).
fn add_drop_target(row: &gtk::ListBoxRow, mailbox: MailboxId) {
    let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
    target.connect_drop(move |t, value, _, _| {
        let Ok(s) = value.get::<String>() else { return false };
        let Some(ids) = s.strip_prefix("tern-msgs:") else { return false };
        let ids: Vec<MessageId> = ids.split(',').filter_map(|i| i.parse().ok()).map(MessageId).collect();
        let Some(ui) = UI_FOR_DROP.with(|u| u.borrow().as_ref().and_then(|w| w.upgrade())) else {
            return false;
        };
        let _ = t;
        ui.send(Command::Move(ids, MoveTarget::Mailbox(mailbox)));
        true
    });
    row.add_controller(target);
}

thread_local! {
    /// Drop handlers are created during rebuilds; they reach the UI through this.
    static UI_FOR_DROP: RefCell<Option<std::rc::Weak<Ui>>> = const { RefCell::new(None) };
}

pub fn connect(ui: &Rc<Ui>) {
    UI_FOR_DROP.with(|u| *u.borrow_mut() = Some(Rc::downgrade(ui)));
    let weak = Rc::downgrade(ui);
    ui.sidebar.listbox.connect_row_selected(move |_, row| {
        let (Some(ui), Some(row)) = (weak.upgrade(), row) else { return };
        if ui.sidebar.rebuilding.get() {
            return;
        }
        let hit = ui
            .sidebar
            .rows
            .borrow()
            .iter()
            .find(|(r, _)| r == row)
            .and_then(|(_, s)| s.clone());
        if let Some((scope, title)) = hit {
            ui.set_scope(scope, &title);
        }
    });
}

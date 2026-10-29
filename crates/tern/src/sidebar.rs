//! Mailbox sidebar: Favorites, Smart Mailboxes, then each account's folders.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib, pango};
use tern_core::model::{AccountId, Mailbox, MailboxId, MailboxRole, MessageId};
use tern_store::ListScope;
use tern_sync::{Command, MoveTarget};

use crate::app::Ui;

pub struct Sidebar {
    pub root: adw::ToolbarView,
    listbox: gtk::ListBox,
    /// Row → what selecting it shows. `None` for headers and \Noselect folders.
    rows: RefCell<Vec<(gtk::ListBoxRow, Option<(ListScope, String)>)>>,
    rebuilding: std::cell::Cell<bool>,
    /// Accounts whose folders are hidden (until Tern restarts).
    collapsed: RefCell<std::collections::HashSet<String>>,
    /// Each account's header row, to light it up while another account is dragged over it.
    headers: RefCell<HashMap<String, gtk::ListBoxRow>>,
    /// Rows that belong to an account (header or folder), for drops that land between rows.
    account_rows: RefCell<Vec<(gtk::ListBoxRow, String, bool)>>,
    /// The account an account drag is over, and a counter that cancels a pending un-highlight.
    drag_over: RefCell<Option<String>>,
    drag_gen: std::cell::Cell<u32>,
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
        let sign_in = gtk::gio::MenuItem::new(Some("Sign In Again…"), None);
        sign_in.set_action_and_target_value(Some("win.sign-in"), Some(&"".to_variant()));
        menu.append_item(&sign_in);
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

        Self { root, listbox, rows: RefCell::new(vec![]), rebuilding: false.into(), collapsed: RefCell::default(), headers: RefCell::default(), account_rows: RefCell::default(), drag_over: RefCell::default(), drag_gen: Default::default() }
    }

    /// Recreate every row from the store, keeping the current selection.
    pub fn rebuild(&self, ui: &Rc<Ui>) {
        self.rebuilding.set(true);
        self.listbox.remove_all();
        self.rows.borrow_mut().clear();
        self.headers.borrow_mut().clear();
        self.account_rows.borrow_mut().clear();

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
            let key = acct.id.to_string();
            let collapsed = self.collapsed.borrow().contains(&key);
            self.account_section(&acct.description, key.clone(), collapsed);
            let Some(boxes) = by_account.get(&acct.id).filter(|_| !collapsed) else {
                continue;
            };
            for (m, depth) in tree_order(boxes) {
                let count = match m.remote.role {
                    MailboxRole::Drafts => m.total,
                    MailboxRole::Sent | MailboxRole::Trash | MailboxRole::All | MailboxRole::Archive => 0,
                    _ => m.unread,
                };
                let scope = m.remote.selectable.then_some(ListScope::Mailbox(m.id));
                self.item(depth, m.remote.role.icon_name(), &m.remote.name, count, scope, Some((m.id, key.clone())));
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

    /// An account's header, with a button that hides or shows its folders.
    fn account_section(&self, title: &str, key: String, collapsed: bool) {
        let label = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["sidebar-section", "account-section"])
            .build();
        // Thin chevrons, like Obsidian's folder tree.
        let (icon, tip) = if collapsed { ("go-next-symbolic", "Show") } else { ("go-down-symbolic", "Hide") };
        let toggle = gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(tip)
            .valign(gtk::Align::Center)
            .css_classes(["flat", "section-toggle"])
            .build();
        toggle.update_property(&[gtk::accessible::Property::Label(&format!("{tip} {title}"))]);
        let key_for_drag = key.clone();
        toggle.connect_clicked(move |_| {
            let Some(ui) = UI_FOR_DROP.with(|u| u.borrow().as_ref().and_then(|w| w.upgrade())) else { return };
            {
                let mut set = ui.sidebar.collapsed.borrow_mut();
                if !set.remove(&key) {
                    set.insert(key.clone());
                }
            }
            ui.schedule_sidebar();
        });
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        hbox.append(&label);
        hbox.append(&toggle);
        let row = gtk::ListBoxRow::builder()
            .child(&hbox)
            .selectable(false)
            .activatable(false)
            .css_classes(["account-header"])
            .build();
        row.set_cursor_from_name(Some("grab"));
        self.headers.borrow_mut().insert(key_for_drag.clone(), row.clone());
        self.account_rows.borrow_mut().push((row.clone(), key_for_drag.clone(), true));
        add_account_drag(&row, key_for_drag);
        self.listbox.append(&row);
        self.rows.borrow_mut().push((row, None));
    }

    /// `drop_target`: the mailbox messages can be dropped on, and its account (account drops).
    fn item(&self, depth: usize, icon: &str, name: &str, count: u32, scope: Option<ListScope>, drop_target: Option<(MailboxId, String)>) {
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
        if let Some((mailbox, account)) = drop_target {
            self.account_rows.borrow_mut().push((row.clone(), account.clone(), false));
            add_drop_target(&row, mailbox, account);
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
const ACCOUNT_DRAG: &str = "tern-account:";

/// Press and drag an account's header (its bubble moves with the pointer) to reorder accounts.
/// Drop it on another account's header (top half: above it, bottom half: below) or anywhere
/// on that account's folders (it moves past that account). That account's header lights up.
fn add_account_drag(row: &gtk::ListBoxRow, key: String) {
    let drag = gtk::DragSource::new();
    drag.set_actions(gdk::DragAction::MOVE);
    let payload = format!("{ACCOUNT_DRAG}{key}");
    drag.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&payload.to_value())));
    // Pressed: brighter bubble, before the drag even starts.
    let pressed = row.clone();
    drag.connect_begin(move |_, _| pressed.add_css_class("pressed"));
    let pressed = row.clone();
    drag.connect_end(move |_, _| pressed.remove_css_class("pressed"));
    let dragged = row.clone();
    drag.connect_drag_begin(move |src, _| {
        dragged.add_css_class("dragging");
        // A snapshot of the bubble, held where it was grabbed.
        let paintable = gtk::WidgetPaintable::new(Some(&dragged)).current_image();
        src.set_icon(Some(&paintable), 24, dragged.height() / 2);
        dragged.set_cursor_from_name(Some("grabbing"));
    });
    let dragged = row.clone();
    drag.connect_drag_end(move |_, _, _| {
        dragged.remove_css_class("dragging");
        dragged.remove_css_class("pressed");
        dragged.set_cursor_from_name(Some("grab"));
    });
    row.add_controller(drag);

    let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
    target.set_preload(true);
    let below = |t: &gtk::DropTarget, y: f64| t.widget().is_some_and(|w| y > f64::from(w.height()) / 2.0);
    let here = key.clone();
    target.connect_enter(move |t, _, _| over_account(t, &here));
    let here = key.clone();
    target.connect_motion(move |t, _, _| over_account(t, &here));
    target.connect_leave(|_| left_account());
    target.connect_drop(move |t, value, _, y| {
        highlight_account(None);
        let Some(moved) = value.get::<String>().ok().and_then(|s| s.strip_prefix(ACCOUNT_DRAG).map(str::to_string)) else {
            return false;
        };
        drop_account(&moved, &key, Some(below(t, y)))
    });
    row.add_controller(target);
}

fn is_account_drag(t: &gtk::DropTarget) -> bool {
    t.value().and_then(|v| v.get::<String>().ok()).is_some_and(|s| s.starts_with(ACCOUNT_DRAG))
}

/// An account drag is over one of account `key`'s rows: light up its header now.
fn over_account(t: &gtk::DropTarget, key: &str) -> gdk::DragAction {
    if is_account_drag(t) {
        highlight_account(Some(key));
    }
    gdk::DragAction::MOVE
}

/// The drag left a row. Moving to the next row enters it right away, so only clear the
/// highlight if nothing else is entered shortly: un-highlighting at once made it flicker.
fn left_account() {
    let Some(ui) = UI_FOR_DROP.with(|u| u.borrow().as_ref().and_then(|w| w.upgrade())) else { return };
    let generation = ui.sidebar.drag_gen.get().wrapping_add(1);
    ui.sidebar.drag_gen.set(generation);
    glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || {
        let Some(ui) = UI_FOR_DROP.with(|u| u.borrow().as_ref().and_then(|w| w.upgrade())) else { return };
        if ui.sidebar.drag_gen.get() == generation {
            highlight_account(None);
        }
    });
}

/// Light up `key`'s header (and only it); `None` clears. Cancels a pending clear.
fn highlight_account(key: Option<&str>) {
    let Some(ui) = UI_FOR_DROP.with(|u| u.borrow().as_ref().and_then(|w| w.upgrade())) else { return };
    let sidebar = &ui.sidebar;
    sidebar.drag_gen.set(sidebar.drag_gen.get().wrapping_add(1));
    if sidebar.drag_over.borrow().as_deref() == key {
        return;
    }
    let headers = sidebar.headers.borrow();
    if let Some(row) = sidebar.drag_over.borrow().as_ref().and_then(|k| headers.get(k)) {
        row.remove_css_class("drop-onto");
    }
    if let Some(row) = key.and_then(|k| headers.get(k)) {
        row.add_css_class("drop-onto");
    }
    *sidebar.drag_over.borrow_mut() = key.map(str::to_string);
}

/// The account row nearest to `y` (list coordinates): its account, whether it's the header,
/// and whether `y` is above that row's middle. For drops that land in the gaps between rows.
fn nearest_account_row(sidebar: &Sidebar, y: f64) -> Option<(String, bool, bool)> {
    let rows = sidebar.account_rows.borrow();
    let mut best: Option<(f64, String, bool, bool)> = None;
    for (row, key, header) in rows.iter() {
        let Some(b) = row.compute_bounds(&sidebar.listbox) else { continue };
        let (top, bottom) = (f64::from(b.y()), f64::from(b.y() + b.height()));
        let dist = if y < top { top - y } else if y > bottom { y - bottom } else { 0.0 };
        if best.as_ref().is_none_or(|(d, ..)| dist < *d) {
            best = Some((dist, key.clone(), *header, y < (top + bottom) / 2.0));
        }
    }
    best.map(|(_, key, header, above)| (key, header, above))
}

/// Move account `moved` next to account `target`: above or below it, or (`None`, dropped on
/// its folders) past it in the direction it was dragged. Saved through the engine.
fn drop_account(moved: &str, target: &str, below: Option<bool>) -> bool {
    let Some(ui) = UI_FOR_DROP.with(|u| u.borrow().as_ref().and_then(|w| w.upgrade())) else {
        return false;
    };
    let ids: Vec<String> = ui.accounts.borrow().iter().map(|a| a.id.to_string()).collect();
    let (Some(from), Some(to)) = (ids.iter().position(|k| k == moved), ids.iter().position(|k| k == target)) else {
        return false;
    };
    let after = below.unwrap_or(from < to);
    let order = crate::format::move_next_to(&ids, &moved.to_string(), &target.to_string(), after);
    if order == ids {
        return true;
    }
    let order: Vec<AccountId> = order
        .iter()
        .filter_map(|k| ui.accounts.borrow().iter().find(|a| a.id.to_string() == *k).map(|a| a.id))
        .collect();
    ui.accounts.borrow_mut().sort_by_key(|a| order.iter().position(|id| *id == a.id).unwrap_or(usize::MAX));
    ui.schedule_sidebar();
    ui.send(Command::ReorderAccounts(order));
    true
}

/// Messages dropped on a mailbox move there; an account dropped on one of another account's
/// folders moves past that account.
fn add_drop_target(row: &gtk::ListBoxRow, mailbox: MailboxId, account: String) {
    let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
    target.set_preload(true);
    let here = account.clone();
    target.connect_enter(move |t, _, _| over_account(t, &here));
    let here = account.clone();
    target.connect_motion(move |t, _, _| over_account(t, &here));
    target.connect_leave(|_| left_account());
    target.connect_drop(move |_, value, _, _| {
        highlight_account(None);
        let Ok(s) = value.get::<String>() else { return false };
        if let Some(moved) = s.strip_prefix(ACCOUNT_DRAG) {
            return drop_account(moved, &account, None);
        }
        let Some(ids) = s.strip_prefix("tern-msgs:") else { return false };
        let ids: Vec<MessageId> = ids.split(',').filter_map(|i| i.parse().ok()).map(MessageId).collect();
        let Some(ui) = UI_FOR_DROP.with(|u| u.borrow().as_ref().and_then(|w| w.upgrade())) else {
            return false;
        };
        ui.send(Command::Move(ids, MoveTarget::Mailbox(mailbox)));
        true
    });
    row.add_controller(target);
}

thread_local! {
    /// Drop handlers and the hide/show buttons are created during rebuilds; they reach the UI through this.
    static UI_FOR_DROP: RefCell<Option<std::rc::Weak<Ui>>> = const { RefCell::new(None) };
}

pub fn connect(ui: &Rc<Ui>) {
    UI_FOR_DROP.with(|u| *u.borrow_mut() = Some(Rc::downgrade(ui)));
    // Account drags that land between rows (the rows' own targets cover the rest).
    let gaps = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
    gaps.set_preload(true);
    let weak = Rc::downgrade(ui);
    let over = move |t: &gtk::DropTarget, y: f64| {
        if let Some(ui) = weak.upgrade()
            && is_account_drag(t)
            && let Some((key, _, _)) = nearest_account_row(&ui.sidebar, y)
        {
            highlight_account(Some(&key));
        }
        gdk::DragAction::MOVE
    };
    let enter = over.clone();
    gaps.connect_enter(move |t, _, y| enter(t, y));
    gaps.connect_motion(move |t, _, y| over(t, y));
    gaps.connect_leave(|_| left_account());
    let weak = Rc::downgrade(ui);
    gaps.connect_drop(move |_, value, _, y| {
        highlight_account(None);
        let Some(ui) = weak.upgrade() else { return false };
        let Some(moved) = value.get::<String>().ok().and_then(|s| s.strip_prefix(ACCOUNT_DRAG).map(str::to_string)) else {
            return false;
        };
        let Some((key, header, above)) = nearest_account_row(&ui.sidebar, y) else { return false };
        // Next to a header: above or below it; next to a folder: past that account.
        drop_account(&moved, &key, header.then_some(!above))
    });
    ui.sidebar.listbox.add_controller(gaps);
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

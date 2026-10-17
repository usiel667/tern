//! The middle pane: search field, message rows, empty/welcome states.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib, pango};
use tern_core::model::{MailboxId, MessageId, MessageSummary};
use tern_core::search::SearchQuery;
use tern_store::ListScope;
use tern_sync::Command;

use crate::app::Ui;
use crate::format::list_date;

pub struct MessageList {
    pub root: adw::ToolbarView,
    pub title: adw::WindowTitle,
    pub model: gio::ListStore,
    pub selection: gtk::MultiSelection,
    pub view: gtk::ListView,
    pub search: gtk::SearchEntry,
    search_bar: gtk::SearchBar,
    stack: gtk::Stack,
    empty: adw::StatusPage,
    unread_btn: gtk::ToggleButton,
    spinner: gtk::Spinner,
    sync_btn: gtk::Button,
    welcome_btn: gtk::Button,
    scroller: gtk::ScrolledWindow,
    /// How many rows to load from the cache; grows as you scroll down.
    limit: Cell<u32>,
    /// The last reload hit `limit`, so the cache has more rows to show.
    more_cached: Cell<bool>,
}

/// Rows loaded per page of the cached list.
const PAGE: u32 = 500;

/// Widgets of one row, stored on the row's root box.
struct RowWidgets {
    dot: gtk::Box,
    sender: gtk::Label,
    date: gtk::Label,
    subject: gtk::Label,
    preview: gtk::Label,
    count: gtk::Label,
    flag: gtk::Box,
    attachment: gtk::Image,
    replied: gtk::Image,
}

impl MessageList {
    pub fn new() -> Self {
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::MultiSelection::new(Some(model.clone()));
        let factory = gtk::SignalListItemFactory::new();
        let view = gtk::ListView::new(Some(selection.clone()), Some(factory));
        view.add_css_class("navigation-sidebar");
        view.set_vexpand(true);

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&view)
            .build();

        let empty = adw::StatusPage::builder()
            .icon_name("tern-inbox-symbolic")
            .title("No Messages")
            .build();
        empty.add_css_class("compact");

        let welcome_btn = gtk::Button::builder()
            .label("Add Account…")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let welcome = adw::StatusPage::builder()
            .icon_name("mail-send-receive-symbolic")
            .title("Welcome to Tern")
            .description("Add an iCloud, Google, Fastmail or other IMAP account to get started.")
            .child(&welcome_btn)
            .build();

        let stack = gtk::Stack::new();
        stack.add_named(&scroller, Some("list"));
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&welcome, Some("welcome"));

        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search  (from: subject: is:unread has:attachment)")
            .hexpand(true)
            .build();
        let search_bar = gtk::SearchBar::builder().child(&search).show_close_button(false).build();
        search_bar.connect_entry(&search);
        search_bar.set_search_mode(true);

        let title = adw::WindowTitle::new("All Inboxes", "");
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));

        let sidebar_btn = gtk::Button::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Show Mailboxes (Ctrl+Shift+M)")
            .action_name("win.toggle-sidebar")
            .build();
        header.pack_start(&sidebar_btn);
        let sync_btn = gtk::Button::builder()
            .icon_name("mail-send-receive-symbolic")
            .tooltip_text("Get Mail (Ctrl+Shift+N)")
            .action_name("win.get-mail")
            .build();
        let spinner = gtk::Spinner::new();
        header.pack_start(&sync_btn);
        header.pack_start(&spinner);

        let compose = gtk::Button::builder()
            .icon_name("mail-message-new-symbolic")
            .tooltip_text("New Message (Ctrl+N)")
            .action_name("win.compose")
            .build();
        let unread_btn = gtk::ToggleButton::builder()
            .icon_name("mail-unread-symbolic")
            .tooltip_text("Show Only Unread")
            .build();
        header.pack_end(&compose);
        header.pack_end(&unread_btn);

        let root = adw::ToolbarView::new();
        root.add_top_bar(&header);
        root.add_top_bar(&search_bar);
        root.set_content(Some(&stack));

        Self {
            root,
            title,
            model,
            selection,
            view,
            search,
            search_bar,
            stack,
            empty,
            unread_btn,
            spinner,
            sync_btn,
            welcome_btn,
            scroller,
            limit: Cell::new(PAGE),
            more_cached: Cell::new(false),
        }
    }

    /// Back to the first page (new view or search).
    pub fn reset_paging(&self) {
        self.limit.set(PAGE);
    }

    pub fn focus_search(&self) {
        self.search_bar.set_search_mode(true);
        self.search.grab_focus();
    }

    pub fn set_syncing(&self, on: bool) {
        self.spinner.set_spinning(on);
        self.spinner.set_visible(on);
        self.sync_btn.set_visible(!on);
    }

    pub fn show_welcome(&self, on: bool) {
        self.stack.set_visible_child_name(if on { "welcome" } else { "list" });
    }
}

fn item_summary(obj: &glib::Object) -> Option<MessageSummary> {
    obj.downcast_ref::<glib::BoxedAnyObject>()
        .map(|b| b.borrow::<MessageSummary>().clone())
}

/// Scrolled to the bottom: show more of the cache, or once it's all shown, ask
/// the engine for older mail in the mailboxes behind this view. Searches and
/// smart views that aren't mailboxes (Flagged, Unread) don't download.
pub fn load_more(ui: &Rc<Ui>) {
    if !ui.query.borrow().trim().is_empty() {
        return;
    }
    if ui.list.more_cached.get() {
        ui.list.limit.set(ui.list.limit.get() + PAGE);
        ui.reload_list();
        return;
    }
    let boxes: Vec<MailboxId> = match &*ui.scope.borrow() {
        ListScope::Mailbox(m) => vec![*m],
        ListScope::Role(role) => ui
            .store
            .borrow()
            .mailboxes(None)
            .unwrap_or_default()
            .into_iter()
            .filter(|m| m.remote.role == *role && m.remote.selectable)
            .map(|m| m.id)
            .collect(),
        _ => vec![],
    };
    if !boxes.is_empty() {
        // Make room so the downloaded batch shows up without another scroll.
        ui.list.limit.set(ui.list.limit.get() + PAGE);
        ui.send(Command::LoadOlder(boxes));
    }
}

/// Cached message and unread counts for a mailbox or unified view.
fn view_totals(ui: &Ui, scope: &ListScope) -> (usize, usize) {
    let boxes = ui.store.borrow().mailboxes(None).unwrap_or_default();
    boxes
        .iter()
        .filter(|m| match scope {
            ListScope::Mailbox(id) => m.id == *id,
            ListScope::Role(role) => m.remote.role == *role,
            _ => false,
        })
        .fold((0, 0), |(t, u), m| (t + m.total as usize, u + m.unread as usize))
}

/// Rows currently selected (in list order).
pub fn selected(ui: &Ui) -> Vec<MessageSummary> {
    let sel = &ui.list.selection;
    (0..ui.list.model.n_items())
        .filter(|&i| sel.is_selected(i))
        .filter_map(|i| ui.list.model.item(i).as_ref().and_then(item_summary))
        .collect()
}

/// Reload rows from the store for the current scope/search/filter, keeping the
/// selection on the same messages (or conversations) when they still exist.
pub fn reload(ui: &Rc<Ui>) {
    if ui.list.stack.visible_child_name().as_deref() == Some("welcome") {
        return;
    }
    let scope = ui.scope.borrow().clone();
    let query = ui.query.borrow().clone();
    let rows = {
        let store = ui.store.borrow();
        if query.trim().is_empty() {
            store.list(&scope, ui.conversations.get(), ui.list.limit.get())
        } else {
            store.search(&SearchQuery::parse(&query), &scope, 500)
        }
    };
    let mut rows = match rows {
        Ok(r) => r,
        Err(e) => {
            ui.toast(&e.to_string());
            vec![]
        }
    };
    ui.list.more_cached.set(query.trim().is_empty() && rows.len() as u32 >= ui.list.limit.get());
    if ui.list.unread_btn.is_active() {
        rows.retain(|m| !m.envelope.flags.seen);
    }

    let keep: Vec<(MessageId, Option<i64>)> = selected(ui).iter().map(|m| (m.id, m.thread_id)).collect();
    let objs: Vec<glib::BoxedAnyObject> = rows.iter().cloned().map(glib::BoxedAnyObject::new).collect();
    ui.list.model.splice(0, ui.list.model.n_items(), &objs);
    for (i, m) in rows.iter().enumerate() {
        let hit = keep.iter().any(|(id, tid)| *id == m.id || (ui.conversations.get() && tid.is_some() && *tid == m.thread_id));
        if hit {
            ui.list.selection.select_item(i as u32, false);
        }
    }

    // The real size of a mailbox view, not just the rows paged in so far.
    let filtered = !query.trim().is_empty() || ui.list.unread_btn.is_active();
    let (total, unread) = match &scope {
        ListScope::Mailbox(_) | ListScope::Role(_) if !filtered => view_totals(ui, &scope),
        _ => (rows.len(), rows.iter().filter(|m| !m.envelope.flags.seen).count()),
    };
    let subtitle = match (total, unread) {
        (0, _) => String::new(),
        (n, 0) => format!("{n} messages"),
        (n, u) => format!("{n} messages, {u} unread"),
    };
    ui.list.title.set_title(&ui.scope_title.borrow());
    ui.list.title.set_subtitle(&subtitle);

    if rows.is_empty() {
        ui.list.empty.set_title(if query.is_empty() { "No Messages" } else { "No Results" });
        ui.list.stack.set_visible_child_name("empty");
    } else {
        ui.list.stack.set_visible_child_name("list");
    }
}

/// Move the selection by `delta` rows (j/k, arrow keys on the viewer).
pub fn step(ui: &Rc<Ui>, delta: i32) {
    let n = ui.list.model.n_items() as i32;
    if n == 0 {
        return;
    }
    let cur = (0..n).find(|&i| ui.list.selection.is_selected(i as u32));
    let next = match cur {
        Some(c) => (c + delta).clamp(0, n - 1),
        None => 0,
    } as u32;
    ui.list.selection.select_item(next, true);
    ui.list.view.scroll_to(next, gtk::ListScrollFlags::FOCUS, None);
}

/// Before deleting/archiving, select the row after the last selected one so
/// the viewer moves on instead of going blank.
pub fn select_after_removal(ui: &Rc<Ui>) {
    let n = ui.list.model.n_items();
    let sel: Vec<u32> = (0..n).filter(|&i| ui.list.selection.is_selected(i)).collect();
    let (Some(&first), Some(&last)) = (sel.first(), sel.last()) else { return };
    let next = if last + 1 < n {
        Some(last + 1)
    } else if first > 0 {
        Some(first - 1)
    } else {
        None
    };
    match next {
        Some(i) => {
            ui.list.selection.select_item(i, true);
        }
        None => {
            ui.list.selection.unselect_all();
            ui.viewer.clear();
        }
    }
}

fn build_row() -> (gtk::Box, RowWidgets) {
    let dot = gtk::Box::builder().valign(gtk::Align::Start).css_classes(["unread-dot"]).build();
    let lbl = |classes: &[&str], ellipsize: bool| {
        let l = gtk::Label::builder().xalign(0.0).css_classes(classes.to_vec()).build();
        if ellipsize {
            l.set_ellipsize(pango::EllipsizeMode::End);
        }
        l
    };
    let sender = lbl(&["sender"], true);
    sender.set_hexpand(true);
    let date = lbl(&["date"], false);
    let count = lbl(&["thread-count"], false);
    count.set_valign(gtk::Align::Center);
    let replied = gtk::Image::from_icon_name("mail-replied-symbolic");
    replied.add_css_class("dim-label");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    top.append(&replied);
    top.append(&sender);
    top.append(&count);
    top.append(&date);

    let subject = lbl(&["subject"], true);
    subject.set_hexpand(true);
    let attachment = gtk::Image::from_icon_name("mail-attachment-symbolic");
    attachment.add_css_class("dim-label");
    let flag = gtk::Box::builder().valign(gtk::Align::Center).css_classes(["flag-dot"]).build();
    let mid = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    mid.append(&subject);
    mid.append(&attachment);
    mid.append(&flag);

    let preview = lbl(&["preview"], true);
    preview.set_lines(2);
    preview.set_wrap(true);
    preview.set_wrap_mode(pango::WrapMode::WordChar);
    preview.set_ellipsize(pango::EllipsizeMode::End);
    preview.set_yalign(0.0);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.append(&top);
    text.append(&mid);
    text.append(&preview);

    let root = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    root.add_css_class("message-row");
    root.append(&dot);
    root.append(&text);
    (root, RowWidgets { dot, sender, date, subject, preview, count, flag, attachment, replied })
}

fn bind_row(w: &RowWidgets, m: &MessageSummary, conversations: bool) {
    let e = &m.envelope;
    if e.flags.seen {
        w.dot.add_css_class("read");
    } else {
        w.dot.remove_css_class("read");
    }
    let sender = e.from.first().map(|a| a.display().to_string()).unwrap_or_else(|| "(no sender)".into());
    w.sender.set_label(&sender);
    w.date.set_label(&list_date(e.date, chrono::Local::now()));
    w.subject.set_label(if e.subject.is_empty() { "(no subject)" } else { &e.subject });
    w.preview.set_label(e.preview.as_deref().unwrap_or(""));
    w.preview.set_visible(e.preview.as_deref().is_some_and(|p| !p.is_empty()));
    w.count.set_visible(conversations && m.thread_count > 1);
    w.count.set_label(&m.thread_count.to_string());
    w.attachment.set_visible(e.has_attachments);
    w.replied.set_visible(e.flags.answered || e.flags.forwarded);
    if e.flags.forwarded && !e.flags.answered {
        w.replied.set_icon_name(Some("mail-forward-symbolic"));
    } else {
        w.replied.set_icon_name(Some("mail-replied-symbolic"));
    }
    for c in tern_core::model::FlagColor::ALL {
        w.flag.remove_css_class(c.css_class());
    }
    match e.flags.flag {
        Some(c) => {
            w.flag.add_css_class(c.css_class());
            w.flag.set_visible(true);
        }
        None => w.flag.set_visible(false),
    }
}

fn context_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let respond = gio::Menu::new();
    respond.append(Some("Reply"), Some("win.reply"));
    respond.append(Some("Reply All"), Some("win.reply-all"));
    respond.append(Some("Forward"), Some("win.forward"));
    menu.append_section(None, &respond);
    let mark = gio::Menu::new();
    mark.append(Some("Mark as Read / Unread"), Some("win.toggle-read"));
    let flags = gio::Menu::new();
    for name in ["red", "orange", "yellow", "green", "blue", "purple", "gray"] {
        let mut label = name.to_string();
        label[..1].make_ascii_uppercase();
        let item = gio::MenuItem::new(Some(&label), None);
        item.set_action_and_target_value(Some("win.flag"), Some(&name.to_variant()));
        flags.append_item(&item);
    }
    let clear = gio::MenuItem::new(Some("Clear Flag"), None);
    clear.set_action_and_target_value(Some("win.flag"), Some(&"none".to_variant()));
    flags.append_item(&clear);
    mark.append_submenu(Some("Flag"), &flags);
    menu.append_section(None, &mark);
    let mv = gio::Menu::new();
    mv.append(Some("Archive"), Some("win.archive"));
    mv.append(Some("Move to Junk"), Some("win.junk"));
    mv.append(Some("Move to Trash"), Some("win.delete"));
    menu.append_section(None, &mv);
    menu
}

pub fn connect(ui: &Rc<Ui>) {
    let list = &ui.list;
    let factory = list
        .view
        .factory()
        .and_downcast::<gtk::SignalListItemFactory>()
        .expect("signal factory");
    let menu = context_menu();

    {
        let weak = Rc::downgrade(ui);
        factory.connect_setup(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            let (root, widgets) = build_row();
            // SAFETY: the key is only read back as `RowWidgets` in `connect_bind`.
            unsafe { root.set_data("row", widgets) };

            // Right-click: select this row if needed, then show the context menu.
            let popover = gtk::PopoverMenu::from_model(Some(&menu));
            popover.set_parent(&root);
            popover.set_has_arrow(false);
            popover.set_halign(gtk::Align::Start);
            let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
            let (w, it) = (weak.clone(), item.downgrade());
            click.connect_pressed(move |_, _, x, y| {
                if let (Some(ui), Some(item)) = (w.upgrade(), it.upgrade()) {
                    let pos = item.position();
                    if !ui.list.selection.is_selected(pos) {
                        ui.list.selection.select_item(pos, true);
                    }
                }
                popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                popover.popup();
            });
            root.add_controller(click);

            // Drag rows onto a sidebar mailbox to move them.
            let drag = gtk::DragSource::new();
            drag.set_actions(gdk::DragAction::MOVE);
            let (w, it) = (weak.clone(), item.downgrade());
            drag.connect_prepare(move |_, _, _| {
                let (ui, item) = (w.upgrade()?, it.upgrade()?);
                let pos = item.position();
                if !ui.list.selection.is_selected(pos) {
                    ui.list.selection.select_item(pos, true);
                }
                let ids: Vec<String> = ui.selected_ids().iter().map(|i| i.0.to_string()).collect();
                Some(gdk::ContentProvider::for_value(&format!("tern-msgs:{}", ids.join(",")).to_value()))
            });
            drag.connect_drag_begin(|src, _| {
                let icon = gtk::Image::from_icon_name("mail-message-new-symbolic");
                icon.set_pixel_size(32);
                src.set_icon(Some(&gtk::WidgetPaintable::new(Some(&icon))), 16, 16);
            });
            root.add_controller(drag);

            item.set_child(Some(&root));
        });
    }
    {
        let weak = Rc::downgrade(ui);
        factory.connect_bind(move |_, obj| {
            let Some(ui) = weak.upgrade() else { return };
            let item = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            let (Some(root), Some(data)) = (item.child(), item.item()) else { return };
            let Some(m) = item_summary(&data) else { return };
            // SAFETY: set in `connect_setup` with this exact type.
            if let Some(w) = unsafe { root.data::<RowWidgets>("row") } {
                bind_row(unsafe { w.as_ref() }, &m, ui.conversations.get());
            }
        });
    }

    // Selection drives the viewer.
    {
        let weak = Rc::downgrade(ui);
        list.selection.connect_selection_changed(move |_, _, _| {
            let Some(ui) = weak.upgrade() else { return };
            let rows = selected(&ui);
            match rows.as_slice() {
                [one] => {
                    ui.viewer.show(&ui, one.id);
                    if ui.split.is_collapsed() {
                        ui.split.set_show_content(true);
                    }
                }
                [] => {}
                many => ui.viewer.show_multiple(many.len()),
            }
        });
    }

    {
        let weak = Rc::downgrade(ui);
        list.scroller.connect_edge_reached(move |_, pos| {
            if pos == gtk::PositionType::Bottom
                && let Some(ui) = weak.upgrade()
            {
                load_more(&ui);
            }
        });
    }

    // Search as you type (debounced by the entry's search-changed delay).
    {
        let weak = Rc::downgrade(ui);
        list.search.connect_search_changed(move |e| {
            let Some(ui) = weak.upgrade() else { return };
            *ui.query.borrow_mut() = e.text().to_string();
            ui.list.reset_paging();
            ui.reload_list();
        });
    }
    {
        let weak = Rc::downgrade(ui);
        list.search.connect_stop_search(move |e| {
            e.set_text("");
            if let Some(ui) = weak.upgrade() {
                ui.list.view.grab_focus();
            }
        });
    }
    {
        let weak = Rc::downgrade(ui);
        list.unread_btn.connect_toggled(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.reload_list();
            }
        });
    }
    {
        let weak = Rc::downgrade(ui);
        list.welcome_btn.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                crate::accounts::open(&ui);
            }
        });
    }

    // Single-key shortcuts, only while the list has focus.
    let keys = gtk::EventControllerKey::new();
    {
        let weak = Rc::downgrade(ui);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(ui) = weak.upgrade() else { return glib::Propagation::Proceed };
            if mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            let action = match key {
                gdk::Key::Delete | gdk::Key::BackSpace => "delete",
                gdk::Key::j => "next-message",
                gdk::Key::k => "previous-message",
                gdk::Key::slash => "search",
                gdk::Key::r => "reply",
                gdk::Key::a => "reply-all",
                gdk::Key::f => "forward",
                gdk::Key::e => "archive",
                gdk::Key::u => "toggle-read",
                gdk::Key::s => "toggle-flag",
                _ => return glib::Propagation::Proceed,
            };
            let _ = WidgetExt::activate_action(&ui.window, &format!("win.{action}"), None);
            glib::Propagation::Stop
        });
    }
    list.view.add_controller(keys);
}

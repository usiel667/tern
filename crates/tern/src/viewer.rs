//! Right pane: the selected message (and its conversation).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, gio, glib, pango};
use tern_core::model::{MessageBody, MessageId, MessageSummary};
use tern_sync::{Command, FlagChange};
use webkit::prelude::*;

use crate::app::Ui;
use crate::format::{full_date, human_size};

/// How far `Viewer::scroll` moves the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    Lines(i32),
    HalfPages(i32),
    Top,
    Bottom,
}

pub struct Viewer {
    /// Itself, for callbacks that outlive a call (the main window's viewer, or one in a
    /// message window of its own).
    me: std::rc::Weak<Viewer>,
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
    draft_banner: adw::Banner,
    attachments: gtk::FlowBox,
    webview: webkit::WebView,
    loading: gtk::Spinner,
    current: Cell<Option<MessageId>>,
    allow_remote: RefCell<HashSet<MessageId>>,
    /// Switches the open message between darkened and its own colours (dark mode only).
    colors_btn: gtk::Button,
    /// Messages switched from the default (darkened, unless Settings turns that off) with the
    /// sun/moon button, this session.
    original_colors: RefCell<HashSet<MessageId>>,
    /// The open message's unsubscribe links (its `List-Unsubscribe` header).
    unsubscribe: RefCell<Option<tern_core::mime::Unsubscribe>>,
    /// Messages unsubscribed from this session (their banner stays hidden).
    unsubscribed: RefCell<HashSet<MessageId>>,
    opening: Cell<Option<Opening>>,
    /// HEIC pictures converted for showing (`None`: couldn't be read), and those being converted.
    heic: RefCell<std::collections::HashMap<(MessageId, usize), Option<Vec<u8>>>>,
    heic_pending: RefCell<HashSet<(MessageId, usize)>>,
    /// The page being printed (`message_window::print`), kept alive until it's done.
    pub(crate) printing: RefCell<Option<webkit::WebView>>,
}

/// Timing of the message being opened, logged when it's slow.
#[derive(Clone, Copy)]
struct Opening {
    id: MessageId,
    started: Instant,
    /// When the body was in hand (from the cache, or once the engine fetched it).
    body_at: Option<Instant>,
    cached: bool,
}

/// Opening a message slower than this gets a log line with where the time went.
const SLOW_OPEN: Duration = Duration::from_millis(300);

fn header_button(icon: &str, tip: &str, action: &str) -> gtk::Button {
    gtk::Button::builder().icon_name(icon).tooltip_text(tip).action_name(action).build()
}

impl Viewer {
    /// `standalone`: for a message window of its own (no "Open in New Window").
    pub fn new(standalone: bool) -> Rc<Self> {
        let header = adw::HeaderBar::new();
        crate::app::same_toolbar_height(&header);
        header.set_show_title(false);
        header.pack_start(&header_button("tern-archive-symbolic", "Archive (Ctrl+Alt+A)", "win.archive"));
        header.pack_start(&header_button("user-trash-symbolic", "Delete (Delete)", "win.delete"));
        header.pack_start(&header_button("mail-mark-junk-symbolic", "Junk (Ctrl+Shift+J)", "win.junk"));
        header.pack_end(&header_button("mail-forward-symbolic", "Forward (Ctrl+Shift+F)", "win.forward"));
        header.pack_end(&header_button("mail-reply-all-symbolic", "Reply All (Ctrl+Shift+R)", "win.reply-all"));
        header.pack_end(&header_button("mail-reply-sender-symbolic", "Reply (Ctrl+R)", "win.reply"));
        header.pack_end(&header_button("starred-symbolic", "Flag (Ctrl+Shift+L)", "win.toggle-flag"));
        let colors_btn = gtk::Button::builder().icon_name("tern-sun-symbolic").visible(false).build();
        header.pack_end(&colors_btn);
        let menu = gio::Menu::new();
        if !standalone {
            menu.append(Some("Open in New Window"), Some("win.open-window"));
        }
        menu.append(Some("Print…"), Some("win.print"));
        menu.append(Some("Save As…"), Some("win.save-message"));
        menu.append(Some("View Source"), Some("win.view-source"));
        let more = gtk::MenuButton::builder().icon_name("view-more-symbolic").tooltip_text("More").menu_model(&menu).build();
        header.pack_end(&more);

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
        let draft_banner = adw::Banner::new("This message is a draft.");
        draft_banner.set_button_label(Some("Edit Draft"));

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
            // Mail never runs scripts: markup off strips <script>, on* attributes and the
            // like while parsing, and the CSP (format.rs) allows no script at all. Scripting
            // itself stays on only so Tern can scroll the view (`Viewer::scroll`). Checked
            // 2026-11-07 with inline/onerror/svg/iframe/meta-refresh/javascript:-link/form probes.
            // Remote loads are governed by the CSP too.
            settings.set_enable_javascript(true);
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
        content.append(&draft_banner);
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

        Rc::new_cyclic(|me| Self {
            me: me.clone(),
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
            draft_banner,
            attachments,
            webview,
            loading,
            current: Cell::new(None),
            opening: Cell::new(None),
            allow_remote: RefCell::new(HashSet::new()),
            colors_btn,
            original_colors: RefCell::new(HashSet::new()),
            heic: Default::default(),
            heic_pending: Default::default(),
            unsubscribe: RefCell::new(None),
            unsubscribed: Default::default(),
            printing: Default::default(),
        })
    }

    /// Whether the message shows remote content (Settings, or Load Remote Content).
    pub fn remote_allowed(&self, ui: &Ui, id: MessageId) -> bool {
        ui.settings.borrow().load_remote_content || self.allow_remote.borrow().contains(&id)
    }

    /// The window this viewer is in.
    pub fn window(&self, ui: &Ui) -> gtk::Window {
        self.root.root().and_downcast::<gtk::Window>().unwrap_or_else(|| ui.window.clone().upcast())
    }

    /// Show the open message again (a setting that changes how it looks changed).
    /// The message's inline HEIC pictures already converted to JPEG; the others start
    /// converting in the background and the message is drawn again when they're ready.
    pub(crate) fn heic_pictures(&self, ui: &Rc<Ui>, id: MessageId, body: &MessageBody) -> std::collections::HashMap<usize, Vec<u8>> {
        let mut ready = std::collections::HashMap::new();
        for (i, a) in body.attachments.iter().enumerate() {
            if !a.inline || !crate::heic::is_heic(&a.mime_type, a.filename.as_deref()) {
                continue;
            }
            if let Some(done) = self.heic.borrow().get(&(id, i)) {
                if let Some(jpeg) = done {
                    ready.insert(i, jpeg.clone());
                }
                continue;
            }
            if !self.heic_pending.borrow_mut().insert((id, i)) {
                continue;
            }
            let Some(bytes) = tern_core::mime::attachment_bytes(&body.raw, i as u32) else { continue };
            let (weak, me) = (Rc::downgrade(ui), self.me.clone());
            glib::spawn_future_local(async move {
                let jpeg = gtk::gio::spawn_blocking(move || crate::heic::to_jpeg(&bytes)).await.ok().flatten();
                let (Some(ui), Some(v)) = (weak.upgrade(), me.upgrade()) else { return };
                v.heic_pending.borrow_mut().remove(&(id, i));
                v.heic.borrow_mut().insert((id, i), jpeg);
                if v.current() == Some(id) {
                    v.rerender(&ui);
                }
            });
        }
        ready
    }

    pub fn rerender(&self, ui: &Rc<Ui>) {
        if let Some(id) = self.current()
            && let Ok(Some(b)) = ui.store.borrow().body(id)
        {
            self.render(ui, id, &b);
        }
    }

    /// Scroll the open message (Vim keys). Runs a line of Tern's own script in the view:
    /// scripting is on for that alone; the message's scripts are stripped
    /// (`enable_javascript_markup` off) and its CSP allows none.
    pub fn scroll(&self, by: Scroll) {
        let js = match by {
            Scroll::Lines(n) => format!("window.scrollBy(0, {})", n * 60),
            Scroll::HalfPages(n) => format!("window.scrollBy(0, {n} * window.innerHeight / 2)"),
            Scroll::Top => "window.scrollTo(0, 0)".to_string(),
            Scroll::Bottom => "window.scrollTo(0, document.scrollingElement.scrollHeight)".to_string(),
        };
        self.webview.evaluate_javascript(&js, None, None, None::<&gtk::gio::Cancellable>, |_| {});
    }

    /// Put the keyboard focus on the open message.
    pub fn focus(&self) {
        self.webview.grab_focus();
    }

    /// Whether `w` is (in) the message view.
    pub fn contains(&self, w: &gtk::Widget) -> bool {
        w == self.webview.upcast_ref::<gtk::Widget>() || w.is_ancestor(&self.webview)
    }

    pub fn current(&self) -> Option<MessageId> {
        self.current.get()
    }

    pub fn clear(&self) {
        self.current.set(None);
        self.colors_btn.set_visible(false);
        self.status.set_title("No Message Selected");
        self.stack.set_visible_child_name("empty");
    }

    pub fn show_multiple(&self, n: usize) {
        self.current.set(None);
        self.colors_btn.set_visible(false);
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
        self.draft_banner.set_revealed(ui.is_draft(id));

        if !same {
            self.remote_banner.set_revealed(false);
            self.colors_btn.set_visible(false);
            self.unsub_banner.set_revealed(false);
            while let Some(c) = self.attachments.first_child() {
                self.attachments.remove(&c);
            }
        }

        // Body: from cache, or ask the engine and show a spinner.
        let body = ui.store.borrow().body(id).ok().flatten();
        if !same {
            let started = Instant::now();
            self.opening.set(Some(Opening { id, started, body_at: None, cached: body.is_some() }));
        }
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
        if let Some(o) = self.opening.get().filter(|o| o.id == id && o.body_at.is_none()) {
            self.opening.set(Some(Opening { body_at: Some(Instant::now()), ..o }));
        }
        self.loading.set_visible(false);
        self.loading.set_spinning(false);
        self.webview.set_visible(true);
        let settings = ui.settings.borrow().clone();
        let allow = settings.load_remote_content || self.allow_remote.borrow().contains(&id);
        let original = self.original_colors.borrow().contains(&id) == settings.darken_html;
        let look = match crate::app::palette() {
            Some(p) => crate::format::Look::of(p),
            None if ui.is_dark() => crate::format::Look::ADWAITA_DARK,
            None => crate::format::Look::LIGHT,
        };
        let converted = self.heic_pictures(ui, id, body);
        let rendered = crate::format::render_body_with(body, allow, &look, original, &converted);
        self.remote_banner.set_revealed(rendered.blocked_remote);
        self.colors_btn.set_visible(rendered.darkened.is_some());
        // A sun while darkened (back to the original light page), a moon while original.
        let (icon, tip) = if rendered.darkened == Some(true) {
            ("tern-sun-symbolic", "Show Original Colors")
        } else {
            ("tern-moon-symbolic", "Show in Dark Colors")
        };
        self.colors_btn.set_icon_name(icon);
        self.colors_btn.set_tooltip_text(Some(tip));
        if let Ok(rgba) = gdk::RGBA::parse(rendered.background) {
            self.webview.set_background_color(&rgba);
        }
        self.webview.load_html(&rendered.html, Some("about:blank"));

        let unsub = tern_core::mime::unsubscribe(&body.raw);
        self.unsub_banner.set_revealed(unsub.is_some() && !self.unsubscribed.borrow().contains(&id));
        *self.unsubscribe.borrow_mut() = unsub;

        while let Some(c) = self.attachments.first_child() {
            self.attachments.remove(&c);
        }
        // A chip for every attachment the message's own HTML doesn't show (pictures shown
        // below the text keep one, for saving).
        for (i, a) in body.attachments.iter().enumerate().filter(|(i, _)| !rendered.placed.contains(i)) {
            let name = a.filename.clone().unwrap_or_else(|| format!("attachment-{}", i + 1));
            let chip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            chip.append(&gtk::Image::from_icon_name("mail-attachment-symbolic"));
            chip.append(&gtk::Label::new(Some(&format!("{name}  ·  {}", human_size(a.size)))));
            let btn = gtk::Button::builder().child(&chip).css_classes(["attachment-chip"]).build();
            let (weak, me, raw) = (Rc::downgrade(ui), self.me.clone(), body.raw.clone());
            btn.connect_clicked(move |b| {
                if let (Some(ui), Some(v)) = (weak.upgrade(), me.upgrade()) {
                    attachment_menu(&ui, &v, b, raw.clone(), i as u32, name.clone());
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
            let (weak, me, id) = (Rc::downgrade(ui), self.me.clone(), t.id);
            row.connect_activated(move |_| {
                if let (Some(ui), Some(v)) = (weak.upgrade(), me.upgrade()) {
                    v.show(&ui, id);
                }
            });
            self.thread_list.append(&row);
        }
        self.thread_revealer.set_reveal_child(true);
    }
}

/// Open / Save… for an attachment chip.
fn attachment_menu(ui: &Rc<Ui>, v: &Rc<Viewer>, chip: &gtk::Button, raw: Vec<u8>, index: u32, name: String) {
    let pop = gtk::Popover::new();
    let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let open = gtk::Button::builder().label("Open").css_classes(["flat"]).build();
    let save = gtk::Button::builder().label("Save…").css_classes(["flat"]).build();
    for b in [&open, &save] {
        b.child().and_downcast::<gtk::Label>().inspect(|l| l.set_xalign(0.0));
        col.append(b);
    }
    pop.set_child(Some(&col));
    pop.set_parent(chip);
    pop.connect_closed(|p| {
        let p = p.clone();
        // After the clicked button's handler has run.
        glib::idle_add_local_once(move || p.unparent());
    });
    let raw = Rc::new(raw);
    {
        let (ui, v, raw, name, pop) = (ui.clone(), v.clone(), raw.clone(), name.clone(), pop.clone());
        open.connect_clicked(move |_| {
            pop.popdown();
            open_attachment(&ui, &v.window(&ui), &raw, index, &name);
        });
    }
    {
        let (ui, v, pop) = (ui.clone(), v.clone(), pop.clone());
        save.connect_clicked(move |_| {
            pop.popdown();
            save_attachment(&ui, &v.window(&ui), (*raw).clone(), index, name.clone());
        });
    }
    pop.popup();
}

fn save_attachment(ui: &Rc<Ui>, window: &gtk::Window, raw: Vec<u8>, index: u32, name: String) {
    let initial = tern_core::mime::safe_file_name(&name, "attachment");
    let dialog = gtk::FileDialog::builder().initial_name(&initial).modal(true).build();
    let weak = Rc::downgrade(ui);
    dialog.save(Some(window), gio::Cancellable::NONE, move |res| {
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

/// Where opened attachments are put for other apps to read: a folder per open, in Tern's
/// cache, cleared of copies older than a day each time.
fn opened_dir() -> std::io::Result<std::path::PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let base = glib::user_cache_dir().join("tern").join("opened");
    // Only this user can read what's been opened.
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&base)?;
    let day = Duration::from_secs(24 * 60 * 60);
    for entry in std::fs::read_dir(&base)?.flatten() {
        let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().unwrap_or_default() > day);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let dir = base.join(stamp.to_string());
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    Ok(dir)
}

/// Open an attachment in the app the desktop picks for it. Programs and scripts aren't
/// opened, only saved.
fn open_attachment(ui: &Rc<Ui>, window: &gtk::Window, raw: &[u8], index: u32, name: &str) {
    let mime = tern_core::mime::body_from_raw(raw.to_vec())
        .attachments
        .get(index as usize)
        .map(|a| a.mime_type.clone())
        .unwrap_or_default();
    if tern_core::mime::risky_to_open(name, &mime) {
        let alert = adw::AlertDialog::new(
            Some("Save This File First?"),
            Some(&format!("“{name}” could be a program. Tern doesn’t open programs or scripts from messages; save it, and open it yourself if you trust it.")),
        );
        alert.add_responses(&[("cancel", "Cancel"), ("save", "Save…")]);
        let (ui2, w2, raw2, name2) = (ui.clone(), window.clone(), raw.to_vec(), name.to_string());
        alert.connect_response(Some("save"), move |_, _| save_attachment(&ui2, &w2, raw2.clone(), index, name2.clone()));
        alert.present(Some(window));
        return;
    }
    let Some(bytes) = tern_core::mime::attachment_bytes(raw, index) else {
        return ui.toast("Attachment not found in message");
    };
    let path = opened_dir().map(|d| d.join(tern_core::mime::safe_file_name(name, "attachment")));
    let path = match path.and_then(|p| std::fs::write(&p, bytes).map(|()| p)) {
        Ok(p) => p,
        Err(e) => return ui.toast(&format!("Couldn’t open: {e}")),
    };
    let weak = Rc::downgrade(ui);
    gtk::FileLauncher::new(Some(&gio::File::for_path(&path))).launch(Some(window), gio::Cancellable::NONE, move |res| {
        if let (Err(e), Some(ui)) = (res, weak.upgrade())
            && !e.matches(gtk::DialogError::Dismissed)
        {
            ui.toast(&format!("Couldn’t open: {e}"));
        }
    });
}

fn open_uri(window: &impl IsA<gtk::Window>, uri: &str) {
    gtk::UriLauncher::new(uri).launch(Some(window), gio::Cancellable::NONE, |_| {});
}

/// Wire up a viewer (the main window's, or a message window's).
pub fn connect(ui: &Rc<Ui>, v: &Rc<Viewer>) {

    {
        let vw = Rc::downgrade(v);
        v.webview.connect_load_changed(move |_, event| {
            if event != webkit::LoadEvent::Finished {
                return;
            }
            let Some(v) = vw.upgrade() else { return };
            let Some(o) = v.opening.take() else { return };
            let total = o.started.elapsed();
            if total >= SLOW_OPEN {
                let body = o.body_at.map(|t| t - o.started).unwrap_or_default();
                tracing::info!(
                    id = o.id.0, cached = o.cached, ?total, waiting_for_body = ?body, rendering = ?(total - body),
                    "slow message open"
                );
            }
        });
    }

    // Links open in the default browser; the view itself never navigates.
    {
        let (weak, vw) = (Rc::downgrade(ui), Rc::downgrade(v));
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
                && let Some(v) = vw.upgrade()
                && (uri.starts_with("http") || uri.starts_with("mailto:")) {
                    if let Some(addr) = uri.strip_prefix("mailto:") {
                        crate::compose::open_mailto(&ui, addr);
                    } else {
                        open_uri(&v.window(&ui), &uri);
                    }
                }
            true
        });
    }
    {
        let (weak, vw) = (Rc::downgrade(ui), Rc::downgrade(v));
        v.draft_banner.connect_button_clicked(move |_| {
            let (Some(ui), Some(v)) = (weak.upgrade(), vw.upgrade()) else { return };
            if let Some(id) = v.current() {
                crate::compose::open_draft(&ui, id);
            }
        });
    }
    {
        let (weak, vw) = (Rc::downgrade(ui), Rc::downgrade(v));
        v.colors_btn.connect_clicked(move |_| {
            let (Some(ui), Some(v)) = (weak.upgrade(), vw.upgrade()) else { return };
            if let Some(id) = v.current() {
                let mut set = v.original_colors.borrow_mut();
                if !set.remove(&id) {
                    set.insert(id);
                }
                drop(set);
                if let Ok(Some(b)) = ui.store.borrow().body(id) {
                    v.render(&ui, id, &b);
                }
            }
        });
    }
    {
        let (weak, vw) = (Rc::downgrade(ui), Rc::downgrade(v));
        v.remote_banner.connect_button_clicked(move |_| {
            let (Some(ui), Some(v)) = (weak.upgrade(), vw.upgrade()) else { return };
            if let Some(id) = v.current() {
                v.allow_remote.borrow_mut().insert(id);
                if let Ok(Some(b)) = ui.store.borrow().body(id) {
                    v.render(&ui, id, &b);
                }
            }
        });
    }
    {
        let (weak, vw) = (Rc::downgrade(ui), Rc::downgrade(v));
        v.unsub_banner.connect_button_clicked(move |b| {
            let (Some(ui), Some(v)) = (weak.upgrade(), vw.upgrade()) else { return };
            let (Some(u), Some(id)) = (v.unsubscribe.borrow().clone(), v.current()) else { return };
            let sender = ui
                .store
                .borrow()
                .message(id)
                .ok()
                .and_then(|m| m.envelope.from.first().map(|a| a.display().to_string()))
                .unwrap_or_else(|| "this list".into());
            // One-click (RFC 8058) when the sender takes it; otherwise their page, or an email.
            let (body, button) = match (&u.web, &u.mail) {
                (Some(_), _) if u.one_click => ("Tern will ask the list to stop sending you mail. No web page opens.".to_string(), "Unsubscribe"),
                (Some(web), _) => (format!("This list unsubscribes on its website:\n{web}"), "Open Website"),
                (None, Some(mail)) => (
                    format!("This list unsubscribes by email. A message to {} opens for you to send.", mail.trim_start_matches("mailto:").split('?').next().unwrap_or("")),
                    "Write Email",
                ),
                (None, None) => return,
            };
            let confirm = adw::AlertDialog::new(Some(&format!("Unsubscribe from {sender}?")), Some(&body));
            confirm.add_response("cancel", "Cancel");
            confirm.add_response("go", button);
            confirm.set_response_appearance("go", adw::ResponseAppearance::Destructive);
            let (w, vw, banner) = (Rc::downgrade(&ui), Rc::downgrade(&v), b.clone());
            confirm.connect_response(Some("go"), move |_, _| {
                let (Some(ui), Some(v)) = (w.upgrade(), vw.upgrade()) else { return };
                match (&u.web, &u.mail) {
                    (Some(web), _) if u.one_click => ui.send(Command::Unsubscribe(web.clone())),
                    (Some(web), _) => open_uri(&v.window(&ui), web),
                    (None, Some(mail)) => crate::compose::open_mailto(&ui, mail.trim_start_matches("mailto:")),
                    (None, None) => {}
                }
                v.unsubscribed.borrow_mut().insert(id);
                banner.set_revealed(false);
            });
            confirm.present(Some(&v.window(&ui)));
        });
    }
    // Re-render on light/dark switch.
    {
        let (weak, vw) = (Rc::downgrade(ui), Rc::downgrade(v));
        adw::StyleManager::default().connect_dark_notify(move |_| {
            let (Some(ui), Some(v)) = (weak.upgrade(), vw.upgrade()) else { return };
            if let Some(id) = v.current()
                && let Ok(Some(b)) = ui.store.borrow().body(id) {
                    v.render(&ui, id, &b);
                }
        });
    }
}

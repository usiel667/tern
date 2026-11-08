//! Vim keys (Settings → General): move around the main window without the mouse.
//!
//! h / l move between Mailboxes, the message list and the open message; j / k, gg / G and
//! Ctrl+D / Ctrl+U move inside the focused one (mailboxes, messages, or scrolling the
//! message). Plus / search, u undo, U read/unread, dd delete, and the message list's single
//! keys (r a f e s) from any pane. Never while typing in a text field or in a dialog.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::glib;

use crate::app::Ui;
use crate::viewer::Scroll;

/// How long `g` / `d` wait for their second key.
const SEQUENCE_TIMEOUT: Duration = Duration::from_millis(1000);
/// Ctrl+D / Ctrl+U in the message list move this many messages.
const HALF_PAGE_ROWS: i32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Left,
    Right,
    Down,
    Up,
    Top,
    Bottom,
    HalfDown,
    HalfUp,
    /// A window action (`win.<name>`).
    Action(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Run(Cmd),
    /// First key of a pair (g, d): swallowed, waiting for the second.
    Wait,
    /// Not a Vim key: let it through.
    Pass,
}

/// Turns key presses into commands. A key that starts a two-key binding (g of gg, d of dd)
/// waits for the next one.
#[derive(Debug, Default)]
pub struct Keys {
    pending: Option<(String, Instant)>,
}

impl Keys {
    /// `table`: every Vim binding (normalized accelerator names, "g g" for a pair) and its
    /// shortcut id, from `shortcuts::table`.
    pub fn feed(&mut self, stroke: &str, table: &[(String, &'static str)], now: Instant) -> Outcome {
        let pending = self.pending.take().filter(|(_, at)| now.duration_since(*at) < SEQUENCE_TIMEOUT).map(|(p, _)| p);
        if let Some(first) = pending {
            let pair = format!("{first} {stroke}");
            if let Some((_, id)) = table.iter().find(|(b, _)| *b == pair) {
                return command(id).map_or(Outcome::Pass, Outcome::Run);
            }
        }
        let starts_pair = format!("{stroke} ");
        if table.iter().any(|(b, _)| b.starts_with(&starts_pair)) {
            self.pending = Some((stroke.to_string(), now));
            return Outcome::Wait;
        }
        match table.iter().find(|(b, _)| b == stroke) {
            Some((_, id)) => command(id).map_or(Outcome::Pass, Outcome::Run),
            None => Outcome::Pass,
        }
    }
}

/// What a Vim shortcut (`tern_core::keys`, id `vim.*`) does.
fn command(id: &str) -> Option<Cmd> {
    Some(match id {
        "vim.left" => Cmd::Left,
        "vim.right" => Cmd::Right,
        "vim.down" => Cmd::Down,
        "vim.up" => Cmd::Up,
        "vim.top" => Cmd::Top,
        "vim.bottom" => Cmd::Bottom,
        "vim.half-down" => Cmd::HalfDown,
        "vim.half-up" => Cmd::HalfUp,
        "vim.search" => Cmd::Action("search"),
        "vim.undo" => Cmd::Action("undo"),
        "vim.toggle-read" => Cmd::Action("toggle-read"),
        "vim.delete" => Cmd::Action("delete"),
        "vim.reply" => Cmd::Action("reply"),
        "vim.reply-all" => Cmd::Action("reply-all"),
        "vim.forward" => Cmd::Action("forward"),
        "vim.archive" => Cmd::Action("archive"),
        "vim.toggle-flag" => Cmd::Action("toggle-flag"),
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    Mailboxes,
    Messages,
    Message,
}

fn pane(ui: &Ui, focus: Option<&gtk::Widget>) -> Pane {
    match focus {
        Some(w) if ui.sidebar.contains(w) => Pane::Mailboxes,
        Some(w) if ui.viewer.contains(w) => Pane::Message,
        _ => Pane::Messages,
    }
}

/// Whether keys should be left alone: typing in a field, or a dialog / menu has the focus.
fn busy(ui: &Ui, focus: Option<&gtk::Widget>) -> bool {
    crate::app::editing_text(ui).is_some()
        || focus.is_some_and(|w| {
            w.ancestor(adw::Dialog::static_type()).is_some() || w.ancestor(gtk::Popover::static_type()).is_some()
        })
}

fn focus_messages(ui: &Rc<Ui>) {
    // Take the focus from the other pane first: scrolling with FOCUS only moves it within
    // the list. Then `step(0)` selects the first message when none is and focuses the row.
    ui.list.view.grab_focus();
    crate::message_list::step(ui, 0);
}

fn run(ui: &Rc<Ui>, cmd: Cmd, pane: Pane) {
    match (cmd, pane) {
        (Cmd::Action(name), _) => {
            let _ = WidgetExt::activate_action(&ui.window, &format!("win.{name}"), None);
        }
        (Cmd::Left, Pane::Messages) => {
            ui.outer.set_show_sidebar(true);
            ui.sidebar.focus();
        }
        (Cmd::Left, Pane::Message) => focus_messages(ui),
        (Cmd::Right, Pane::Mailboxes) => focus_messages(ui),
        (Cmd::Right, Pane::Messages) => {
            if ui.viewer.current().is_some() {
                ui.viewer.focus();
            }
        }
        (Cmd::Left, Pane::Mailboxes) | (Cmd::Right, Pane::Message) => {}
        (Cmd::Down, Pane::Mailboxes) => ui.sidebar.step(1),
        (Cmd::Up, Pane::Mailboxes) => ui.sidebar.step(-1),
        (Cmd::Top, Pane::Mailboxes) => ui.sidebar.step(i32::MIN),
        (Cmd::Bottom, Pane::Mailboxes) => ui.sidebar.step(i32::MAX),
        (Cmd::HalfDown, Pane::Mailboxes) => ui.sidebar.step(HALF_PAGE_ROWS),
        (Cmd::HalfUp, Pane::Mailboxes) => ui.sidebar.step(-HALF_PAGE_ROWS),
        (Cmd::Down, Pane::Messages) => crate::message_list::step(ui, 1),
        (Cmd::Up, Pane::Messages) => crate::message_list::step(ui, -1),
        (Cmd::Top, Pane::Messages) => crate::message_list::step(ui, i32::MIN / 2),
        (Cmd::Bottom, Pane::Messages) => crate::message_list::step(ui, i32::MAX / 2),
        (Cmd::HalfDown, Pane::Messages) => crate::message_list::step(ui, HALF_PAGE_ROWS),
        (Cmd::HalfUp, Pane::Messages) => crate::message_list::step(ui, -HALF_PAGE_ROWS),
        (Cmd::Down, Pane::Message) => ui.viewer.scroll(Scroll::Lines(1)),
        (Cmd::Up, Pane::Message) => ui.viewer.scroll(Scroll::Lines(-1)),
        (Cmd::Top, Pane::Message) => ui.viewer.scroll(Scroll::Top),
        (Cmd::Bottom, Pane::Message) => ui.viewer.scroll(Scroll::Bottom),
        (Cmd::HalfDown, Pane::Message) => ui.viewer.scroll(Scroll::HalfPages(1)),
        (Cmd::HalfUp, Pane::Message) => ui.viewer.scroll(Scroll::HalfPages(-1)),
    }
}

/// Listen on the main window, ahead of the focused widget (capture phase), so keys also work
/// while the message view (WebKit) has the focus.
pub fn connect(ui: &Rc<Ui>) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let state = Rc::new(RefCell::new(Keys::default()));
    let weak = Rc::downgrade(ui);
    keys.connect_key_pressed(move |_, key, _, mods| {
        let Some(ui) = weak.upgrade() else { return glib::Propagation::Proceed };
        if !ui.settings.borrow().vim_keys {
            return glib::Propagation::Proceed;
        }
        let focus = gtk::prelude::GtkWindowExt::focus(&ui.window);
        if busy(&ui, focus.as_ref()) {
            return glib::Propagation::Proceed;
        }
        let Some(stroke) = crate::shortcuts::stroke(key, mods) else { return glib::Propagation::Proceed };
        let table = crate::shortcuts::table(&ui, tern_core::keys::Group::Vim);
        match state.borrow_mut().feed(&stroke, &table, Instant::now()) {
            Outcome::Pass => glib::Propagation::Proceed,
            Outcome::Wait => glib::Propagation::Stop,
            Outcome::Run(cmd) => {
                run(&ui, cmd, pane(&ui, focus.as_ref()));
                glib::Propagation::Stop
            }
        }
    });
    ui.window.add_controller(keys);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_core::keys::{self, Group, Overrides};

    fn table(o: &Overrides) -> Vec<(String, &'static str)> {
        keys::SHORTCUTS
            .iter()
            .filter(|s| s.group == Group::Vim)
            .flat_map(|s| keys::bindings(s, o).into_iter().map(move |b| (b, s.id)))
            .collect()
    }

    fn feed(k: &mut Keys, strokes: &[&str], t: &[(String, &'static str)], at: Instant) -> Vec<Outcome> {
        strokes.iter().map(|s| k.feed(s, t, at)).collect()
    }

    #[test]
    fn default_keys() {
        let (t, now) = (table(&Overrides::new()), Instant::now());
        let mut k = Keys::default();
        assert_eq!(feed(&mut k, &["g", "g"], &t, now), [Outcome::Wait, Outcome::Run(Cmd::Top)]);
        assert_eq!(feed(&mut k, &["d", "d"], &t, now), [Outcome::Wait, Outcome::Run(Cmd::Action("delete"))]);
        assert_eq!(feed(&mut k, &["<Shift>g"], &t, now), [Outcome::Run(Cmd::Bottom)]);
        assert_eq!(feed(&mut k, &["u"], &t, now), [Outcome::Run(Cmd::Action("undo"))]);
        assert_eq!(feed(&mut k, &["<Shift>u"], &t, now), [Outcome::Run(Cmd::Action("toggle-read"))]);
        assert_eq!(feed(&mut k, &["x"], &t, now), [Outcome::Pass]);
        assert_eq!(feed(&mut k, &["<Control>d", "<Control>u"], &t, now), [Outcome::Run(Cmd::HalfDown), Outcome::Run(Cmd::HalfUp)]);
        assert_eq!(feed(&mut k, &["<Control>r"], &t, now), [Outcome::Pass], "Ctrl+R stays the app's reply");
    }

    #[test]
    fn a_pair_needs_both_keys_in_time_and_in_order() {
        let (t, now) = (table(&Overrides::new()), Instant::now());
        let mut k = Keys::default();
        assert_eq!(k.feed("g", &t, now), Outcome::Wait);
        assert_eq!(k.feed("g", &t, now + SEQUENCE_TIMEOUT), Outcome::Wait, "too late: starts over");
        assert_eq!(k.feed("j", &t, now + SEQUENCE_TIMEOUT), Outcome::Run(Cmd::Down), "a different key cancels");
        assert_eq!(k.feed("g", &t, now), Outcome::Wait);
        assert_eq!(k.feed("d", &t, now), Outcome::Wait, "gd isn't bound; d now waits");
        assert_eq!(k.feed("d", &t, now), Outcome::Run(Cmd::Action("delete")));
    }

    #[test]
    fn changed_keys_take_over() {
        let mut o = Overrides::new();
        o.insert("vim.down".into(), vec!["n".into()]);
        o.insert("vim.top".into(), vec!["<Shift>k".into()]);
        o.insert("vim.delete".into(), vec!["x".into()]);
        let (t, now) = (table(&o), Instant::now());
        let mut k = Keys::default();
        assert_eq!(k.feed("n", &t, now), Outcome::Run(Cmd::Down));
        assert_eq!(k.feed("j", &t, now), Outcome::Pass, "old key freed");
        assert_eq!(k.feed("g", &t, now), Outcome::Pass, "gg gone, so g doesn't wait");
        assert_eq!(k.feed("<Shift>k", &t, now), Outcome::Run(Cmd::Top));
        assert_eq!(k.feed("x", &t, now), Outcome::Run(Cmd::Action("delete")));
    }
}

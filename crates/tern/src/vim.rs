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
use gtk::{gdk, glib};

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

/// Turns key presses into commands; `g` and `d` wait for a second key (gg, dd).
#[derive(Debug, Default)]
pub struct Keys {
    pending: Option<(char, Instant)>,
}

impl Keys {
    pub fn feed(&mut self, c: char, ctrl: bool, now: Instant) -> Outcome {
        let pending = self.pending.take().filter(|(_, at)| now.duration_since(*at) < SEQUENCE_TIMEOUT).map(|(p, _)| p);
        if ctrl {
            return match c {
                'd' => Outcome::Run(Cmd::HalfDown),
                'u' => Outcome::Run(Cmd::HalfUp),
                _ => Outcome::Pass,
            };
        }
        let cmd = match (pending, c) {
            (Some('g'), 'g') => Cmd::Top,
            (Some('d'), 'd') => Cmd::Action("delete"),
            (_, 'g' | 'd') => {
                self.pending = Some((c, now));
                return Outcome::Wait;
            }
            (_, 'h') => Cmd::Left,
            (_, 'l' | 'o') => Cmd::Right,
            (_, 'j') => Cmd::Down,
            (_, 'k') => Cmd::Up,
            (_, 'G') => Cmd::Bottom,
            (_, '/') => Cmd::Action("search"),
            (_, 'u') => Cmd::Action("undo"),
            (_, 'U') => Cmd::Action("toggle-read"),
            (_, 'r') => Cmd::Action("reply"),
            (_, 'a') => Cmd::Action("reply-all"),
            (_, 'f') => Cmd::Action("forward"),
            (_, 'e') => Cmd::Action("archive"),
            (_, 's') => Cmd::Action("toggle-flag"),
            _ => return Outcome::Pass,
        };
        Outcome::Run(cmd)
    }
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
        if !ui.settings.borrow().vim_keys || mods.intersects(gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK) {
            return glib::Propagation::Proceed;
        }
        let focus = gtk::prelude::GtkWindowExt::focus(&ui.window);
        if busy(&ui, focus.as_ref()) {
            return glib::Propagation::Proceed;
        }
        let Some(c) = key.to_unicode() else { return glib::Propagation::Proceed };
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        match state.borrow_mut().feed(c, ctrl, Instant::now()) {
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

    fn feed(keys: &mut Keys, text: &str, start: Instant) -> Vec<Outcome> {
        text.chars().map(|c| keys.feed(c, false, start)).collect()
    }

    #[test]
    fn pairs_and_singles() {
        let t = Instant::now();
        let mut k = Keys::default();
        assert_eq!(feed(&mut k, "gg", t), [Outcome::Wait, Outcome::Run(Cmd::Top)]);
        assert_eq!(feed(&mut k, "dd", t), [Outcome::Wait, Outcome::Run(Cmd::Action("delete"))]);
        assert_eq!(feed(&mut k, "G", t), [Outcome::Run(Cmd::Bottom)]);
        assert_eq!(feed(&mut k, "u", t), [Outcome::Run(Cmd::Action("undo"))]);
        assert_eq!(feed(&mut k, "U", t), [Outcome::Run(Cmd::Action("toggle-read"))]);
        assert_eq!(feed(&mut k, "x", t), [Outcome::Pass]);
        assert_eq!(k.feed('d', true, t), Outcome::Run(Cmd::HalfDown));
        assert_eq!(k.feed('u', true, t), Outcome::Run(Cmd::HalfUp));
        assert_eq!(k.feed('r', true, t), Outcome::Pass, "Ctrl+R stays the app's reply");
    }

    #[test]
    fn a_pair_needs_both_keys_in_time_and_in_order() {
        let t = Instant::now();
        let mut k = Keys::default();
        assert_eq!(k.feed('g', false, t), Outcome::Wait);
        assert_eq!(k.feed('g', false, t + SEQUENCE_TIMEOUT), Outcome::Wait, "too late: starts over");
        assert_eq!(k.feed('j', false, t + SEQUENCE_TIMEOUT), Outcome::Run(Cmd::Down), "a different key cancels");
        assert_eq!(k.feed('g', false, t), Outcome::Wait);
        assert_eq!(k.feed('d', false, t), Outcome::Wait, "gd isn't a command; d now waits");
        assert_eq!(k.feed('d', false, t), Outcome::Run(Cmd::Action("delete")));
    }
}

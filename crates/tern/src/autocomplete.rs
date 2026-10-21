//! Recipient suggestions under the To/Cc/Bcc entries.
//!
//! A non-focusable popover below the entry lists matches for the address being typed (the
//! text after the last separator). Up/Down/Return/Tab/Escape act on the list only while it
//! shows, so the compose window's own shortcuts keep working otherwise.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};
use tern_core::model::Address;

use crate::format::{format_address, parse_addresses};

const SHOWN: usize = 8;

/// Gives the suggestions for a typed prefix; already-present emails are excluded by the caller
/// of `attach`, so this only needs to query and rank.
pub type Suggest = dyn Fn(&str) -> Vec<Address>;

struct State {
    entry: gtk::Entry,
    popover: gtk::Popover,
    list: gtk::ListBox,
    shown: std::cell::RefCell<Vec<Address>>,
    /// Set while we rewrite the entry ourselves, so `changed` doesn't re-query.
    filling: Cell<bool>,
    suggest: Box<Suggest>,
}

/// Attach suggestions to `entry`. The popover is removed when `window` is destroyed.
pub fn attach(entry: &gtk::Entry, window: &impl IsA<gtk::Widget>, suggest: Box<Suggest>) {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .activate_on_single_click(true)
        .can_focus(false)
        .css_classes(["autocomplete"])
        .build();
    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(false)
        .can_focus(false)
        .position(gtk::PositionType::Bottom)
        .halign(gtk::Align::Start)
        .child(&list)
        .build();
    popover.set_parent(entry);
    let st = Rc::new(State {
        entry: entry.clone(),
        popover: popover.clone(),
        list: list.clone(),
        shown: Default::default(),
        filling: Cell::new(false),
        suggest,
    });

    {
        let st = st.clone();
        entry.connect_changed(move |_| {
            if !st.filling.get() {
                refresh(&st);
            }
        });
    }
    {
        let st = st.clone();
        list.connect_row_activated(move |_, row| pick(&st, row.index() as usize));
    }
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let st = st.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            if !st.popover.is_visible() || mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            let n = st.shown.borrow().len() as i32;
            let cur = st.list.selected_row().map_or(-1, |r| r.index());
            match key {
                gdk::Key::Down => select(&st, (cur + 1).min(n - 1)),
                gdk::Key::Up => select(&st, (cur - 1).max(0)),
                gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::Tab => pick(&st, cur.max(0) as usize),
                gdk::Key::Escape => st.popover.popdown(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
    }
    entry.add_controller(keys);
    let focus = gtk::EventControllerFocus::new();
    {
        // Delay so a click on a suggestion lands before the list goes away.
        let st = st.clone();
        focus.connect_leave(move |_| {
            let st = st.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                if !st.entry.has_focus() && !st.entry.first_child().is_some_and(|c| c.has_focus()) {
                    st.popover.popdown();
                }
            });
        });
    }
    entry.add_controller(focus);
    window.connect_destroy(move |_| popover.unparent());
}

fn refresh(st: &State) {
    let text = st.entry.text();
    let (start, token) = current_token(&text);
    let mut found = Vec::new();
    if !token.is_empty() {
        // Don't offer what's already in this field.
        let present: Vec<String> = parse_addresses(&text[..start])
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.email.to_lowercase())
            .collect();
        found = (st.suggest)(token)
            .into_iter()
            .filter(|a| !present.contains(&a.email.to_lowercase()))
            .take(SHOWN)
            .collect();
    }
    while let Some(row) = st.list.row_at_index(0) {
        st.list.remove(&row);
    }
    if found.is_empty() {
        st.shown.borrow_mut().clear();
        st.popover.popdown();
        return;
    }
    for a in &found {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let (title, sub) = match &a.name {
            Some(n) => (n.as_str(), Some(a.email.as_str())),
            None => (a.email.as_str(), None),
        };
        b.append(&gtk::Label::builder().label(title).xalign(0.0).build());
        if let Some(sub) = sub {
            b.append(&gtk::Label::builder().label(sub).xalign(0.0).css_classes(["dim-label", "caption"]).build());
        }
        let row = gtk::ListBoxRow::builder().child(&b).can_focus(false).build();
        st.list.append(&row);
    }
    *st.shown.borrow_mut() = found;
    select(st, 0);
    // Point at the left edge of the entry so the list sits under the text, not centred.
    let h = st.entry.height();
    st.popover.set_pointing_to(Some(&gdk::Rectangle::new(0, 0, 1, h)));
    st.popover.popup();
    // GtkEntry doesn't size popovers it doesn't own (only parents that call
    // gtk_popover_present() in size_allocate do), so position it ourselves.
    st.popover.present();
}

fn select(st: &State, i: i32) {
    if let Some(row) = st.list.row_at_index(i) {
        st.list.select_row(Some(&row));
    }
}

fn pick(st: &State, i: usize) {
    let Some(a) = st.shown.borrow().get(i).cloned() else { return };
    let text = st.entry.text();
    let (start, _) = current_token(&text);
    st.filling.set(true);
    st.entry.set_text(&with_choice(&text[..start], &a));
    st.entry.set_position(-1);
    st.filling.set(false);
    st.popover.popdown();
}

/// Byte offset where the address being typed starts, and its text (trimmed). Separators
/// inside quotes or angle brackets don't count, matching `parse_addresses`.
fn current_token(text: &str) -> (usize, &str) {
    let (mut quoted, mut angle, mut start) = (false, false, 0);
    for (i, c) in text.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '<' => angle = true,
            '>' => angle = false,
            ',' | ';' if !quoted && !angle => start = i + 1,
            _ => {}
        }
    }
    (start, text[start..].trim())
}

/// The field's earlier addresses, then the chosen one, ready for the next.
fn with_choice(before: &str, a: &Address) -> String {
    let before = before.trim_end();
    let sep = if before.is_empty() { "" } else { " " };
    format!("{before}{sep}{}, ", format_address(a))
}

#[cfg(test)]
mod tests {
    use super::{current_token, with_choice};
    use tern_core::model::Address;

    #[test]
    fn token_is_text_after_last_separator() {
        assert_eq!(current_token("ma"), (0, "ma"));
        assert_eq!(current_token("Ann <a@x.org>, ma"), (14, "ma"));
        assert_eq!(current_token("a@x.org; "), (8, ""));
        assert_eq!(current_token(r#""Lee, Cat" <c@x.org>, Bo"#).1, "Bo");
        assert_eq!(current_token(r#""Lee, Ca"#).1, r#""Lee, Ca"#, "comma inside quotes");
    }

    #[test]
    fn choice_replaces_the_token() {
        let maya = Address::new(Some("Maya Chen"), "maya@x.org");
        assert_eq!(with_choice("", &maya), "Maya Chen <maya@x.org>, ");
        assert_eq!(with_choice("a@x.org,", &maya), "a@x.org, Maya Chen <maya@x.org>, ");
        let lee = Address::new(Some("Lee, Cat"), "c@x.org");
        assert_eq!(with_choice("", &lee), r#""Lee, Cat" <c@x.org>, "#);
    }
}

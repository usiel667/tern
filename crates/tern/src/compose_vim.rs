//! Vim editing in the New Message body (when Vim keys are on): runs `tern_core::vim_text` on
//! the text box. Starts in Insert mode; Esc → Normal (block cursor); Esc again is the
//! window's (asks to close). Keys set in Settings → Keyboard (like kj) also work as Esc. Each change is one undo step, so `u` / Ctrl+R use the box's undo.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use std::time::Duration;

use tern_core::vim_text::{Action, Escape, Mode, Vim, escape_step};

use crate::app::Ui;

/// How long the keys set to work as Esc (Settings → Keyboard) may take, like Vim's
/// `timeoutlen`; a held key is typed after this.
const ESCAPE_WAIT: Duration = Duration::from_millis(1000);

/// Hook Vim up to `body`; `status` shows the mode. `run` does save / close for `:w` / `:q`.
pub fn attach(ui: &Rc<Ui>, body: &gtk::TextView, status: &gtk::Label, run: Rc<dyn Fn(Action)>) {
    let vim = Rc::new(RefCell::new(Vim::new()));
    // The Vim cursor while a visual selection is shown (the box's own cursor sits at its edge).
    let visual_cursor = Rc::new(Cell::new(0usize));
    let on = ui.settings.borrow().vim_keys;
    status.set_visible(on);
    status.set_label(&vim.borrow().status());

    // Keys held back while they may be the start of the Esc keys, and the timer that types them.
    let held: Rc<RefCell<String>> = Rc::default();
    let held_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    {
        // Leaving the body (a click on Send) types anything held back.
        let focus = gtk::EventControllerFocus::new();
        let (held, held_timer, body2) = (held.clone(), held_timer.clone(), body.downgrade());
        focus.connect_leave(move |_| {
            if let Some(id) = held_timer.take() {
                id.remove();
            }
            if let Some(body) = body2.upgrade() {
                type_held(&body, &held.take());
            }
        });
        body.add_controller(focus);
    }

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(ui);
    let (body2, status) = (body.clone(), status.clone());
    keys.connect_key_pressed(move |_, key, _, mods| {
        let Some(ui) = weak.upgrade() else { return glib::Propagation::Proceed };
        if !ui.settings.borrow().vim_keys {
            status.set_visible(false);
            body2.set_overwrite(false);
            return glib::Propagation::Proceed;
        }
        status.set_visible(true);
        let name = key_name(key, mods);
        let mut v = vim.borrow_mut();
        let name = match name {
            Some(n) if v.mode == Mode::Insert && n != "<Esc>" => {
                // Keys set to work as Esc (kj): held back until they finish or don't.
                let seq = ui.settings.borrow().vim_escape.clone();
                let before = held.take();
                let (flush, then) = escape_step(&seq, &before, &n);
                if let Some(id) = held_timer.take() {
                    id.remove();
                }
                type_held(&body2, &flush);
                match then {
                    Escape::Esc => "<Esc>".to_string(),
                    Escape::Hold if !body2.buffer().has_selection() => {
                        held.replace(format!("{}{n}", if flush.is_empty() { before } else { String::new() }));
                        status.set_label(&format!("{}   {}", v.status(), held.borrow()));
                        let (held, timer, body, status, vim) = (held.clone(), held_timer.clone(), body2.downgrade(), status.clone(), vim.clone());
                        held_timer.replace(Some(glib::timeout_add_local_once(ESCAPE_WAIT, move || {
                            timer.take();
                            if let Some(body) = body.upgrade() {
                                type_held(&body, &held.take());
                            }
                            if let Ok(v) = vim.try_borrow() {
                                status.set_label(&v.status());
                            }
                        })));
                        return glib::Propagation::Stop;
                    }
                    // Insert mode: only Esc is ours; don't copy the text for every typed letter.
                    _ => {
                        status.set_label(&v.status());
                        return glib::Propagation::Proceed;
                    }
                }
            }
            Some(n) => n,
            None => {
                // A shortcut or arrow: type anything held back first.
                if let Some(id) = held_timer.take() {
                    id.remove();
                }
                type_held(&body2, &held.take());
                return glib::Propagation::Proceed;
            }
        };
        if v.mode == Mode::Insert && name == "<Esc>" {
            // Esc itself (or Ctrl+[) while keys are held: they were typing, then Esc.
            if let Some(id) = held_timer.take() {
                id.remove();
            }
            type_held(&body2, &held.take());
        }
        let buffer = body2.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).to_string();
        let visual_before = matches!(v.mode, Mode::Visual | Mode::VisualLine);
        let cursor = if visual_before { visual_cursor.get() } else { buffer.iter_at_mark(&buffer.get_insert()).offset() as usize };
        let out = v.key(&text, cursor, &name);
        if !out.handled {
            return glib::Propagation::Proceed;
        }
        if let Some(e) = &out.edit {
            buffer.begin_user_action();
            let mut a = buffer.iter_at_offset(e.start as i32);
            let mut b = buffer.iter_at_offset(e.end as i32);
            buffer.delete(&mut a, &mut b);
            let mut at = buffer.iter_at_offset(e.start as i32);
            buffer.insert(&mut at, &e.text);
            buffer.end_user_action();
        }
        let new_cursor = out.cursor.unwrap_or(cursor);
        let new_text = if out.edit.is_some() { buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).to_string() } else { text };
        match v.selection(&new_text, new_cursor) {
            Some((a, b)) => {
                visual_cursor.set(new_cursor);
                buffer.select_range(&buffer.iter_at_offset(b as i32), &buffer.iter_at_offset(a as i32));
            }
            None => buffer.place_cursor(&buffer.iter_at_offset(new_cursor as i32)),
        }
        body2.scroll_mark_onscreen(&buffer.get_insert());
        body2.set_overwrite(v.mode != Mode::Insert);
        status.set_label(&v.status());
        let action = out.action;
        drop(v);
        match action {
            Some(Action::Undo) => {
                if buffer.can_undo() {
                    buffer.undo();
                }
            }
            Some(Action::Redo) => {
                if buffer.can_redo() {
                    buffer.redo();
                }
            }
            Some(other) => run(other),
            None => {}
        }
        glib::Propagation::Stop
    });
    body.add_controller(keys);
}

/// Type keys that were held back, as if typed then.
fn type_held(body: &gtk::TextView, text: &str) {
    if text.is_empty() {
        return;
    }
    let buffer = body.buffer();
    buffer.begin_user_action();
    buffer.insert_interactive_at_cursor(text, body.is_editable());
    buffer.end_user_action();
    body.scroll_mark_onscreen(&buffer.get_insert());
}

/// A key press as Vim names it, or `None` for keys left to the text box and window (arrows,
/// Tab, shortcuts with Ctrl/Alt other than Ctrl+R and Ctrl+[).
fn key_name(key: gdk::Key, mods: gdk::ModifierType) -> Option<String> {
    let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
    if mods.intersects(gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK) {
        return None;
    }
    if ctrl {
        return match key {
            gdk::Key::r | gdk::Key::R => Some("<C-r>".into()),
            gdk::Key::bracketleft => Some("<Esc>".into()),
            _ => None,
        };
    }
    match key {
        gdk::Key::Escape => Some("<Esc>".into()),
        gdk::Key::Return | gdk::Key::KP_Enter => Some("<CR>".into()),
        gdk::Key::BackSpace => Some("<BS>".into()),
        _ => key.to_unicode().filter(|c| !c.is_control()).map(String::from),
    }
}

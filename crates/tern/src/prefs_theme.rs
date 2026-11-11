//! Settings → Appearance: the color theme, applied at once to every window and the open
//! message.

use std::rc::Rc;

use adw::prelude::*;
use tern_core::themes;
use tern_sync::Command;

use crate::app::Ui;

pub fn page(ui: &Rc<Ui>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("Appearance").icon_name("preferences-desktop-appearance-symbolic").build();
    let group = adw::PreferencesGroup::builder()
        .title("Theme")
        .description("Colors for all of Tern: the window, messages and New Message. System follows your desktop's light or dark mode.")
        .build();

    let choices = themes::choices();
    let names: Vec<&str> = choices.iter().map(|c| c.1).collect();
    let model = gtk::StringList::new(&names);
    let row = adw::ComboRow::builder().title("Theme").model(&model).build();

    // Each choice with a few dots of its colors.
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, obj| {
        let item = obj.downcast_ref::<gtk::ListItem>().expect("list item");
        let b = gtk::Box::builder().spacing(10).build();
        // Same width for every choice, so the names line up.
        b.append(&gtk::DrawingArea::builder().content_width(SLOT * 6).content_height(SLOT).valign(gtk::Align::Center).build());
        b.append(&gtk::Label::builder().xalign(0.0).build());
        item.set_child(Some(&b));
    });
    factory.connect_bind(|_, obj| {
        let item = obj.downcast_ref::<gtk::ListItem>().expect("list item");
        let (Some(b), Some(s)) = (item.child(), item.item().and_downcast::<gtk::StringObject>()) else { return };
        let name = s.string();
        let id = themes::choices().into_iter().find(|c| c.1 == name.as_str()).map_or(themes::SYSTEM, |c| c.0);
        let (Some(dots), Some(label)) = (b.first_child().and_downcast::<gtk::DrawingArea>(), b.last_child().and_downcast::<gtk::Label>()) else {
            return;
        };
        let colors: Vec<gtk::gdk::RGBA> = swatches(id).iter().filter_map(|c| gtk::gdk::RGBA::parse(*c).ok()).collect();
        dots.set_draw_func(move |area, cr, _, h| {
            // An outline in the text colour keeps dark dots visible on a dark list.
            let edge = area.color();
            let r = SLOT as f64 / 2.0 - 2.0;
            for (i, c) in colors.iter().enumerate() {
                let x = SLOT as f64 * (i as f64 + 0.5);
                cr.arc(x, h as f64 / 2.0, r, 0.0, std::f64::consts::TAU);
                cr.set_source_rgba(c.red().into(), c.green().into(), c.blue().into(), 1.0);
                let _ = cr.fill_preserve();
                cr.set_source_rgba(edge.red().into(), edge.green().into(), edge.blue().into(), 0.35);
                cr.set_line_width(1.0);
                let _ = cr.stroke();
            }
        });
        dots.queue_draw();
        label.set_label(&name);
    });
    row.set_factory(Some(&factory));

    let current = ui.settings.borrow().theme.clone();
    row.set_selected(choices.iter().position(|c| c.0 == current).unwrap_or(0) as u32);
    let ui = ui.clone();
    row.connect_selected_notify(move |row| {
        let Some(&(id, _)) = themes::choices().get(row.selected() as usize) else { return };
        ui.settings.borrow_mut().theme = id.to_string();
        ui.send(Command::SaveSettings(ui.settings.borrow().clone()));
        crate::app::apply_theme(id);
        ui.viewer.rerender(&ui);
    });

    group.add(&row);
    page.add(&group);
    page
}

/// Width of one swatch dot.
const SLOT: i32 = 14;

/// A choice's colors for its dots: background, mailboxes, then the accent colors.
fn swatches(id: &str) -> Vec<&'static str> {
    match themes::find(id) {
        Some(p) => vec![p.bg, p.sidebar, p.purple, p.red, p.amber, p.accent],
        None if id == themes::ADWAITA_LIGHT => vec!["#ffffff", "#ebebed", "#3584e4"],
        None if id == themes::ADWAITA_DARK => vec!["#1e1e1e", "#303030", "#3584e4"],
        // System: Adwaita light, then Tokyo Night.
        None => vec!["#ffffff", "#3584e4", "#1a1b26", "#bb9af7"],
    }
}

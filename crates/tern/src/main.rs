//! Tern — an Apple Mail-style email client for Linux.
//!
//! Usage:
//!   tern              normal mode (accounts from ~/.config/tern/accounts.toml)
//!   tern --demo       sample account, no network, nothing saved
//!   tern --profile D  keep config and cache under directory D
//!   tern mailto:…     open a New Message from a mailto: link (in the running Tern if there is one)

mod accounts;
mod app;
mod autocomplete;
mod compose;
mod folder_zip;
mod format;
mod message_list;
mod sidebar;
mod viewer;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

pub const APP_ID: &str = "dev.tern.Tern";

#[derive(Debug, Clone, Default)]
pub struct Args {
    pub demo: bool,
    pub profile: Option<std::path::PathBuf>,
}

fn parse_args() -> (Args, Vec<String>) {
    let mut args = Args::default();
    let mut gtk_args = vec![std::env::args().next().unwrap_or_else(|| "tern".into())];
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--demo" => args.demo = true,
            "--profile" => args.profile = it.next().map(Into::into),
            "-h" | "--help" => {
                println!("tern [--demo] [--profile DIR] [mailto:URI…]");
                std::process::exit(0);
            }
            _ => gtk_args.push(a),
        }
    }
    (args, gtk_args)
}

fn main() -> gtk::glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tern=info,tern_sync=info,tern_imap=info".into()),
        )
        .init();

    let (args, gtk_args) = parse_args();
    let app = adw::Application::builder()
        .application_id(if args.demo { "dev.tern.Tern.Demo" } else { APP_ID })
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_startup(|_| app::load_css());
    // Both handlers run in the primary instance; a second `tern` forwards to it over D-Bus.
    let ui: Rc<RefCell<Option<Rc<app::Ui>>>> = Rc::default();
    let ensure_ui = move |app: &adw::Application| -> Option<Rc<app::Ui>> {
        if let Some(ui) = ui.borrow().as_ref() {
            ui.window.present();
            return Some(ui.clone());
        }
        let built = app::build(app, args.clone());
        *ui.borrow_mut() = built.clone();
        built
    };
    let ensure = ensure_ui.clone();
    app.connect_activate(move |app| {
        ensure(app);
    });
    app.connect_open(move |app, files, _| {
        let Some(ui) = ensure_ui(app) else { return };
        for file in files {
            let uri = file.uri();
            if uri.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("mailto:")) {
                compose::open_mailto(&ui, &uri);
            } else {
                ui.toast("Tern can only open mailto: links");
            }
        }
    });
    app.run_with_args(&gtk_args)
}

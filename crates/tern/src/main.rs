//! Tern — an Apple Mail-style email client for Linux.
//!
//! Usage:
//!   tern              normal mode (accounts from ~/.config/tern/accounts.toml)
//!   tern --demo       sample account, no network, nothing saved
//!   tern --profile D  keep config and cache under directory D

mod accounts;
mod app;
mod compose;
mod format;
mod message_list;
mod sidebar;
mod viewer;

use adw::prelude::*;

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
                println!("tern [--demo] [--profile DIR]");
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
        .build();
    app.connect_startup(|_| app::load_css());
    app.connect_activate(move |app| {
        if let Some(win) = app.active_window() {
            win.present();
            return;
        }
        app::build(app, args.clone());
    });
    app.run_with_args(&gtk_args)
}

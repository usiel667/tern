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
mod compose_vim;
mod folder_zip;
mod format;
mod heic;
mod message_list;
mod message_window;
mod outbox;
mod prefs;
mod prefs_keys;
mod prefs_theme;
mod report;
mod shortcuts;
mod sidebar;
mod viewer;
mod vim;

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

/// On NVIDIA's driver, GTK's default Vulkan renderer draws the message view slowly enough
/// to make scrolling jitter (measured at 4K/120 Hz: ~45 changed frames a second, vs ~63
/// with OpenGL). Use OpenGL there unless the user picked a renderer.
fn prefer_gl_on_nvidia() -> bool {
    if std::env::var_os("GSK_RENDERER").is_some() || !std::path::Path::new("/proc/driver/nvidia/version").exists() {
        return false;
    }
    // SAFETY: first thing in `main`, before any other thread exists.
    // GTK's OpenGL renderer; it used to be called "ngl", and GTK now warns that the old name
    // was renamed to "gl".
    unsafe { std::env::set_var("GSK_RENDERER", "gl") };
    true
}

/// Pick how notifications are sent, so clicking one brings Tern forward with that message.
/// GLib 2.90 otherwise sends an unsandboxed app's notifications through the desktop portal,
/// which can't tell which app it is, and the click never reached Tern (seen on Plasma,
/// 2026-11-17). On GNOME, GNOME Shell's own interface (GLib's "gtk" backend) is the one that
/// also raises the window out of the overview; elsewhere the freedesktop notification server.
/// Inside a Flatpak the portal knows the app, so it's left alone there.
fn choose_notification_backend() {
    if std::env::var_os("GNOTIFICATION_BACKEND").is_some() || std::path::Path::new("/.flatpak-info").exists() {
        return;
    }
    let gnome = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|d| d.eq_ignore_ascii_case("GNOME")));
    // SAFETY: first thing in `main`, before any other thread exists.
    unsafe { std::env::set_var("GNOTIFICATION_BACKEND", if gnome { "gtk" } else { "freedesktop" }) };
}

fn main() -> gtk::glib::ExitCode {
    choose_notification_backend();
    let gl = prefer_gl_on_nvidia();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tern=info,tern_sync=info,tern_imap=info".into()),
        )
        .init();

    if gl {
        tracing::info!("NVIDIA driver: using GTK's OpenGL renderer (set GSK_RENDERER to override)");
    }
    tracing::debug!(backend = ?std::env::var_os("GNOTIFICATION_BACKEND"), "notifications");
    let (args, gtk_args) = parse_args();
    // The name notifications and the desktop show (otherwise the binary's, "tern").
    gtk::glib::set_application_name("Tern");
    let app = adw::Application::builder()
        .application_id(if args.demo { "dev.tern.Tern.Demo" } else { APP_ID })
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_startup(|_| {
        app::load_css();
        gtk::Window::set_default_icon_name(APP_ID);
    });
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

#[cfg(test)]
mod icon_tests {
    /// Every icon the code names is one of Tern's bundled `tern-*` icons (so the desktop's
    /// icon theme can't swap it, see data/icons/README.md), and is listed in the resources.
    #[test]
    fn every_icon_is_bundled() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let resources = std::fs::read_to_string(dir.join("data/resources.gresource.xml")).unwrap();
        let mut seen = 0;
        // The app, plus tern-core's mailbox-role icons.
        let files = std::fs::read_dir(dir.join("src")).unwrap().map(|e| e.unwrap().path());
        for path in files.chain([dir.join("../tern-core/src/model.rs")]) {
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let code = std::fs::read_to_string(&path).unwrap();
            for name in code.split('"').filter(|s| {
                s.len() > "-symbolic".len()
                    && s.ends_with("-symbolic")
                    && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            }) {
                let file = format!("{name}.svg");
                assert!(name.starts_with("tern-"), "{}: \"{name}\" comes from the desktop's icon theme", path.display());
                assert!(dir.join("data/icons").join(&file).exists(), "{name}: no data/icons/{file}");
                assert!(resources.contains(&format!("alias=\"{file}\"")), "{name}: not in resources.gresource.xml");
                seen += 1;
            }
        }
        assert!(seen > 30, "found the icon names ({seen})");
    }
}

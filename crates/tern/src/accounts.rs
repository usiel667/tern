//! "Add Account" sheet, modelled on Apple Mail's provider picker.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tern_core::account::{AccountConfig, Provider, Security, ServerConfig, Transport};
use tern_sync::Command;

use crate::app::Ui;

pub fn open(ui: &Rc<Ui>) {
    if ui.demo {
        ui.toast("Accounts can’t be added in demo mode");
        return;
    }
    let nav = adw::NavigationView::new();
    let dialog = adw::Dialog::builder()
        .title("Add Account")
        .content_width(460)
        .content_height(560)
        .child(&nav)
        .build();

    let group = adw::PreferencesGroup::builder()
        .title("Choose a mail account provider")
        .build();
    for (provider, title, subtitle, icon) in [
        (Provider::ICloud, "iCloud", "Uses an app-specific password", "weather-overcast-symbolic"),
        (Provider::Gmail, "Google", "Sign in with your browser", "web-browser-symbolic"),
        (Provider::Fastmail, "Fastmail", "Uses a JMAP API token", "mail-send-receive-symbolic"),
        (Provider::Generic, "Other Mail Account…", "IMAP and SMTP", "mail-unread-symbolic"),
    ] {
        let row = adw::ActionRow::builder().title(title).subtitle(subtitle).activatable(true).build();
        row.add_prefix(&gtk::Image::from_icon_name(icon));
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let (nav2, ui2, dialog2) = (nav.clone(), ui.clone(), dialog.clone());
        row.connect_activated(move |_| nav2.push(&form_page(&ui2, &dialog2, provider, title)));
        group.add(&row);
    }
    let page = adw::PreferencesPage::new();
    page.add(&group);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&page));
    nav.add(&adw::NavigationPage::new(&tv, "Add Account"));

    dialog.present(Some(&ui.window));
}

fn form_page(ui: &Rc<Ui>, dialog: &adw::Dialog, provider: Provider, title: &str) -> adw::NavigationPage {
    let name = adw::EntryRow::builder().title("Name").build();
    let email = adw::EntryRow::builder().title("Email Address").build();
    email.set_input_purpose(gtk::InputPurpose::Email);
    let secret = adw::PasswordEntryRow::builder()
        .title(match provider {
            Provider::ICloud => "App-Specific Password",
            Provider::Fastmail => "API Token",
            _ => "Password",
        })
        .build();
    let identity = adw::PreferencesGroup::new();
    identity.add(&name);
    identity.add(&email);
    if provider != Provider::Gmail {
        identity.add(&secret);
    }
    identity.set_description(Some(match provider {
        Provider::ICloud => "Create one at account.apple.com → Sign-In and Security → App-Specific Passwords.",
        Provider::Fastmail => "Create one in Fastmail → Settings → Privacy & Security → API tokens, with Mail and Submission access.",
        Provider::Gmail => "Your browser will open so you can sign in to Google. Requires TERN_GOOGLE_CLIENT_ID (see README).",
        Provider::Generic => "Use an app password if your provider has two-factor authentication.",
    }));

    let imap_host = adw::EntryRow::builder().title("Incoming (IMAP) Server").build();
    let imap_port = adw::EntryRow::builder().title("IMAP Port").text("993").build();
    let smtp_host = adw::EntryRow::builder().title("Outgoing (SMTP) Server").build();
    let smtp_port = adw::EntryRow::builder().title("SMTP Port").text("465").build();
    let servers = adw::PreferencesGroup::builder()
        .title("Servers")
        .description("Ports 993/465 use TLS; 143/587 use STARTTLS.")
        .build();
    for r in [&imap_host, &imap_port, &smtp_host, &smtp_port] {
        servers.add(r);
    }
    // Pre-fill server names from the address for "Other".
    {
        let (ih, sh) = (imap_host.clone(), smtp_host.clone());
        email.connect_changed(move |e| {
            if let Some((_, domain)) = e.text().split_once('@')
                && !domain.is_empty() {
                    ih.set_text(&format!("imap.{domain}"));
                    sh.set_text(&format!("smtp.{domain}"));
                }
        });
    }

    let status = gtk::Label::builder().wrap(true).css_classes(["error"]).visible(false).build();
    let spinner = gtk::Spinner::new();
    let button = gtk::Button::builder()
        .label(if provider == Provider::Gmail { "Sign In with Google" } else { "Sign In" })
        .css_classes(["pill", "suggested-action"])
        .halign(gtk::Align::Center)
        .build();
    let actions = gtk::Box::new(gtk::Orientation::Vertical, 12);
    actions.set_margin_top(12);
    actions.append(&status);
    actions.append(&spinner);
    actions.append(&button);

    let page = adw::PreferencesPage::new();
    page.add(&identity);
    if provider == Provider::Generic {
        page.add(&servers);
    }
    let tail = adw::PreferencesGroup::new();
    tail.add(&actions);
    page.add(&tail);

    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&page));

    let (ui2, dialog2) = (ui.clone(), dialog.clone());
    button.connect_clicked(move |btn| {
        let name_s = name.text().trim().to_string();
        let email_s = email.text().trim().to_string();
        let secret_s = secret.text().to_string();
        if name_s.is_empty() || !email_s.contains('@') {
            show_error(&status, "Enter your name and email address.");
            return;
        }
        if provider != Provider::Gmail && secret_s.is_empty() {
            show_error(&status, "Enter your password or token.");
            return;
        }
        let mut cfg = AccountConfig::preset(provider, &name_s, &email_s);
        if provider == Provider::Generic {
            let port = |e: &adw::EntryRow, d: u16| e.text().trim().parse::<u16>().unwrap_or(d);
            let sec = |p: u16| if matches!(p, 143 | 587 | 25) { Security::StartTls } else { Security::Tls };
            let (ip, sp) = (port(&imap_port, 993), port(&smtp_port, 465));
            cfg.transport = Transport::Imap {
                imap: ServerConfig { host: imap_host.text().trim().into(), port: ip, security: sec(ip) },
                smtp: ServerConfig { host: smtp_host.text().trim().into(), port: sp, security: sec(sp) },
            };
            if let Some((_, d)) = email_s.split_once('@') {
                cfg.description = d.to_string();
            }
        }

        status.set_visible(false);
        spinner.set_spinning(true);
        btn.set_sensitive(false);
        let rx = verify_in_background(cfg.clone(), secret_s);
        let (ui3, dialog3, status2, spinner2, btn2) =
            (ui2.clone(), dialog2.clone(), status.clone(), spinner.clone(), btn.clone());
        glib::spawn_future_local(async move {
            let result = rx.recv().await.unwrap_or_else(|_| Err("verification thread stopped".into()));
            spinner2.set_spinning(false);
            btn2.set_sensitive(true);
            match result {
                Ok(()) => {
                    ui3.send(Command::AddAccount(Box::new(cfg)));
                    ui3.toast("Account added — syncing…");
                    dialog3.close();
                }
                Err(e) => show_error(&status2, &e),
            }
        });
    });

    adw::NavigationPage::new(&tv, title)
}

fn show_error(label: &gtk::Label, msg: &str) {
    label.set_label(msg);
    label.set_visible(true);
}

/// Save credentials, then log in once to prove they work. Runs on its own
/// thread (keyring D-Bus calls and network) and reports back over a channel.
fn verify_in_background(cfg: AccountConfig, secret: String) -> async_channel::Receiver<Result<(), String>> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build();
        let result = match rt {
            Ok(rt) => rt.block_on(verify(&cfg, secret)),
            Err(e) => Err(e.to_string()),
        };
        if result.is_err() {
            let _ = tern_auth::delete_secret(cfg.id);
        }
        let _ = tx.send_blocking(result);
    });
    rx
}

async fn verify(cfg: &AccountConfig, secret: String) -> Result<(), String> {
    if cfg.provider == Provider::Gmail {
        let tokens = tern_auth::google::sign_in(&cfg.identity.email, |url| {
            let _ = std::process::Command::new("xdg-open").arg(url).spawn();
        })
        .await
        .map_err(|e| e.to_string())?;
        tern_auth::store_tokens(cfg.id, &tokens).map_err(|e| e.to_string())?;
    } else {
        tern_auth::store_secret(cfg.id, &secret).map_err(|e| {
            format!("{e}\n\nIs a Secret Service provider running (gnome-keyring or KeePassXC)?")
        })?;
    }
    let cred = tern_auth::credential_for(cfg).await.map_err(|e| e.to_string())?;
    match &cfg.transport {
        Transport::Imap { imap, .. } => tern_imap::ImapBackend::new(imap.clone(), cred)
            .verify()
            .await
            .map_err(|e| format!("Couldn’t sign in to {}: {e}", imap.host)),
        Transport::Jmap { session_url } => {
            let tern_auth::Credential::Bearer { token } = cred else {
                return Err("JMAP needs an API token".into());
            };
            tern_jmap_verify(session_url, token).await
        }
    }
}

async fn tern_jmap_verify(session_url: &str, token: String) -> Result<(), String> {
    let b = tern_sync::jmap_backend(session_url.to_string(), token);
    b.list_mailboxes().await.map(|_| ()).map_err(|e| format!("Couldn’t sign in to Fastmail: {e}"))
}

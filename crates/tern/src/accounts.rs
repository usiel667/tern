//! "Add Account" sheet, modelled on Apple Mail's provider picker.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tern_core::account::{AccountConfig, AuthMethod, Provider, Security, ServerConfig, Transport};
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
    let google = if tern_auth::google::configured() {
        "App password or browser sign-in"
    } else {
        "Uses an app password"
    };
    for (provider, title, subtitle, icon) in [
        (Provider::ICloud, "iCloud", "Uses an app-specific password", "weather-overcast-symbolic"),
        (Provider::Gmail, "Google", google, "web-browser-symbolic"),
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
    let secret = adw::PasswordEntryRow::builder().title(secret_help(provider).0).build();
    // Google: app passwords always work; browser sign-in only with an OAuth client configured
    // (until Tern's own client is verified by Google).
    let method = (provider == Provider::Gmail && tern_auth::google::configured()).then(|| {
        adw::ComboRow::builder()
            .title("Sign-In Method")
            .model(&gtk::StringList::new(&["App Password", "Browser"]))
            .build()
    });
    let identity = adw::PreferencesGroup::new();
    identity.add(&name);
    identity.add(&email);
    if let Some(m) = &method {
        identity.add(m);
    }
    identity.add(&secret);
    identity.set_description(Some(secret_help(provider).1));
    let uses_browser = {
        let method = method.clone();
        move || method.as_ref().is_some_and(|m| m.selected() == 1)
    };

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
        .label("Sign In")
        .css_classes(["pill", "suggested-action"])
        .halign(gtk::Align::Center)
        .build();
    if let Some(m) = &method {
        let (secret, identity, button) = (secret.clone(), identity.clone(), button.clone());
        m.connect_selected_notify(move |m| {
            let browser = m.selected() == 1;
            secret.set_visible(!browser);
            identity.set_description(Some(if browser { GMAIL_BROWSER } else { GMAIL_APP_PASSWORD }));
            button.set_label(if browser { "Sign In with Google" } else { "Sign In" });
        });
    }
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
        let browser = uses_browser();
        if !browser && secret_s.trim().is_empty() {
            show_error(&status, "Enter your password or token.");
            return;
        }
        let mut cfg = AccountConfig::preset(provider, &name_s, &email_s);
        if provider == Provider::Gmail && !browser {
            cfg.auth = AuthMethod::Password;
        }
        let secret_s = cfg.clean_secret(&secret_s);
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
        let rx = verify_in_background(cfg.clone(), secret_s, true);
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

const GMAIL_APP_PASSWORD: &str = "Create one at myaccount.google.com/apppasswords (needs 2-Step Verification) \
     and enter it here — not your regular Google password.";
const GMAIL_BROWSER: &str = "Your browser will open so you can sign in to Google.";

/// What a provider's secret is called, and where to get one.
fn secret_help(provider: Provider) -> (&'static str, &'static str) {
    match provider {
        Provider::ICloud => (
            "App-Specific Password",
            "Create one at account.apple.com → Sign-In and Security → App-Specific Passwords.",
        ),
        Provider::Gmail => ("App Password", GMAIL_APP_PASSWORD),
        Provider::Fastmail => (
            "API Token",
            "Create one in Fastmail → Settings → Privacy & Security → API tokens, with Mail and Submission access.",
        ),
        Provider::Generic => ("Password", "Use an app password if your provider has two-factor authentication."),
    }
}

/// "Sign In Again" sheet: a new password (or browser sign-in) for an account that has one
/// already. `id` picks the account; empty lets you choose. The new secret is saved only once
/// it works, so a typo can't replace a good password.
pub fn sign_in_again(ui: &Rc<Ui>, id: &str) {
    if ui.demo {
        ui.toast("Accounts can’t be changed in demo mode");
        return;
    }
    let accounts = ui.accounts.borrow().clone();
    if accounts.is_empty() {
        ui.toast("No accounts yet — add one first");
        return;
    }
    let start = accounts.iter().position(|a| a.id.to_string() == id).unwrap_or(0);

    let group = adw::PreferencesGroup::new();
    let picker = (accounts.len() > 1).then(|| {
        let names: Vec<&str> = accounts.iter().map(|a| a.identity.email.as_str()).collect();
        let row = adw::ComboRow::builder()
            .title("Account")
            .model(&gtk::StringList::new(&names))
            .selected(start as u32)
            .build();
        group.add(&row);
        row
    });
    let secret = adw::PasswordEntryRow::new();
    group.add(&secret);

    let status = gtk::Label::builder().wrap(true).css_classes(["error"]).visible(false).build();
    let spinner = gtk::Spinner::new();
    let button = gtk::Button::builder()
        .label("Sign In")
        .css_classes(["pill", "suggested-action"])
        .halign(gtk::Align::Center)
        .build();
    let actions = gtk::Box::new(gtk::Orientation::Vertical, 12);
    actions.set_margin_top(12);
    actions.append(&status);
    actions.append(&spinner);
    actions.append(&button);
    let tail = adw::PreferencesGroup::new();
    tail.add(&actions);

    let current: Rc<dyn Fn() -> AccountConfig> = {
        let picker = picker.clone();
        Rc::new(move || accounts[picker.as_ref().map_or(start, |p| p.selected() as usize)].clone())
    };
    // Fit the form to the chosen account: app password, token, or browser sign-in.
    let refresh = {
        let (current, group, secret, button) = (current.clone(), group.clone(), secret.clone(), button.clone());
        move || {
            let cfg = current();
            let browser = cfg.auth == AuthMethod::OAuth2Google;
            let (title, help) = secret_help(cfg.provider);
            group.set_title(&glib::markup_escape_text(&cfg.identity.email));
            group.set_description(Some(if browser { GMAIL_BROWSER } else { help }));
            secret.set_title(title);
            secret.set_visible(!browser);
            button.set_label(if browser { "Sign In with Google" } else { "Sign In" });
        }
    };
    refresh();
    if let Some(p) = &picker {
        p.connect_selected_notify(move |_| refresh());
    }

    let page = adw::PreferencesPage::new();
    page.add(&group);
    page.add(&tail);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&page));
    let dialog = adw::Dialog::builder().title("Sign In Again").content_width(460).child(&tv).build();

    let (ui2, dialog2) = (ui.clone(), dialog.clone());
    button.connect_clicked(move |btn| {
        let cfg = current();
        let secret_s = cfg.clean_secret(&secret.text());
        if cfg.auth != AuthMethod::OAuth2Google && secret_s.is_empty() {
            show_error(&status, "Enter your password or token.");
            return;
        }
        status.set_visible(false);
        spinner.set_spinning(true);
        btn.set_sensitive(false);
        let rx = verify_in_background(cfg.clone(), secret_s, false);
        let (ui3, dialog3, status2, spinner2, btn2) =
            (ui2.clone(), dialog2.clone(), status.clone(), spinner.clone(), btn.clone());
        glib::spawn_future_local(async move {
            let result = rx.recv().await.unwrap_or_else(|_| Err("verification thread stopped".into()));
            spinner2.set_spinning(false);
            btn2.set_sensitive(true);
            match result {
                Ok(()) => {
                    ui3.send(Command::SignIn(Box::new(cfg)));
                    ui3.signed_in();
                    ui3.toast("Signed in — syncing…");
                    dialog3.close();
                }
                Err(e) => show_error(&status2, &e),
            }
        });
    });
    dialog.present(Some(&ui.window));
}

fn show_error(label: &gtk::Label, msg: &str) {
    label.set_label(msg);
    label.set_visible(true);
}

/// Log in once to prove the credentials work, then save them. Runs on its own thread (keyring
/// D-Bus calls and network) and reports back over a channel. `adding`: a failed new account
/// leaves nothing in the keyring.
fn verify_in_background(
    cfg: AccountConfig,
    secret: String,
    adding: bool,
) -> async_channel::Receiver<Result<(), String>> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build();
        let result = match rt {
            Ok(rt) => rt.block_on(verify(&cfg, secret)),
            Err(e) => Err(e.to_string()),
        };
        if result.is_err() && adding {
            let _ = tern_auth::delete_secret(cfg.id);
        }
        let _ = tx.send_blocking(result);
    });
    rx
}

async fn verify(cfg: &AccountConfig, secret: String) -> Result<(), String> {
    let oauth = cfg.auth == AuthMethod::OAuth2Google;
    let cred = if oauth {
        let tokens = tern_auth::google::sign_in(&cfg.identity.email, |url| {
            let _ = std::process::Command::new("xdg-open").arg(url).spawn();
        })
        .await
        .map_err(|e| e.to_string())?;
        tern_auth::store_tokens(cfg.id, &tokens).map_err(|e| e.to_string())?;
        tern_auth::credential_for(cfg).await.map_err(|e| e.to_string())?
    } else {
        tern_auth::credential_from_secret(cfg, &secret)
    };
    log_in(cfg, cred).await?;
    if !oauth {
        tern_auth::store_secret(cfg.id, &secret).map_err(|e| {
            format!("{e}\n\nIs a Secret Service provider running (gnome-keyring or KeePassXC)?")
        })?;
    }
    Ok(())
}

async fn log_in(cfg: &AccountConfig, cred: tern_auth::Credential) -> Result<(), String> {
    match &cfg.transport {
        Transport::Imap { imap, .. } => tern_imap::ImapBackend::new(imap.clone(), cred)
            .verify()
            .await
            .map_err(|e| signin_error(cfg, &imap.host, &e)),
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

/// The message shown when signing in fails. Google refuses regular passwords over IMAP, which
/// is the most likely mistake with a Gmail account, so say what's needed instead.
fn signin_error(cfg: &AccountConfig, host: &str, e: &tern_core::Error) -> String {
    if cfg.provider == Provider::Gmail && cfg.auth == AuthMethod::Password && matches!(e, tern_core::Error::Auth(_)) {
        return "Google didn’t accept that password. Use an app password, not your regular Google password: \
                create one at myaccount.google.com/apppasswords (needs 2-Step Verification)."
            .into();
    }
    format!("Couldn’t sign in to {host}: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gmail_password_errors_explain_app_passwords() {
        let mut gmail = AccountConfig::preset(Provider::Gmail, "T", "t@gmail.com");
        gmail.auth = AuthMethod::Password;
        let refused = tern_core::Error::Auth("[ALERT] Application-specific password required".into());
        assert!(signin_error(&gmail, "imap.gmail.com", &refused).contains("app password"));
        let offline = tern_core::Error::Network("connection refused".into());
        assert!(signin_error(&gmail, "imap.gmail.com", &offline).starts_with("Couldn’t sign in to imap.gmail.com"));
        let icloud = AccountConfig::preset(Provider::ICloud, "T", "t@icloud.com");
        assert!(signin_error(&icloud, "imap.mail.me.com", &refused).starts_with("Couldn’t sign in"));
    }
}

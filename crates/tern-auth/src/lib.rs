//! Credentials.
//!
//! * Passwords / app passwords / API tokens live in the freedesktop Secret
//!   Service (gnome-keyring, KeePassXC, kwallet's bridge) via `keyring`.
//! * Google accounts use OAuth2 with PKCE and a loopback redirect, the flow
//!   Google requires for desktop apps. Tokens are stored as JSON in the keyring.
//!
//! On a bare Hyprland setup you need a Secret Service provider running, e.g.
//! `gnome-keyring-daemon` started from `exec-once`, or KeePassXC with Secret
//! Service integration enabled.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tern_core::account::{AccountConfig, AuthMethod};
use tern_core::model::AccountId;
use tern_core::{Error, Result};

pub mod google;

const SERVICE: &str = "dev.tern.Tern";

/// What a backend needs to log in.
#[derive(Clone)]
pub enum Credential {
    Password { username: String, password: String },
    /// SASL XOAUTH2 / HTTP bearer.
    OAuth2 { username: String, access_token: String },
    Bearer { token: String },
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print secrets.
        match self {
            Self::Password { username, .. } => write!(f, "Password({username}, ***)"),
            Self::OAuth2 { username, .. } => write!(f, "OAuth2({username}, ***)"),
            Self::Bearer { .. } => write!(f, "Bearer(***)"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl OAuthTokens {
    /// Refresh a minute early so a token never expires mid-command.
    pub fn needs_refresh(&self) -> bool {
        self.expires_at
            .is_some_and(|t| t - Duration::seconds(60) <= Utc::now())
    }
}

fn entry(account: AccountId) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, &account.to_string()).map_err(keyring_error)
}

/// A keyring failure in words a person can act on (still `Error::Auth`, so the engine treats
/// it as before). The details stay in the log.
fn keyring_error(e: keyring::Error) -> Error {
    let detail = e.to_string();
    tracing::warn!("keyring: {detail}");
    Error::Auth(explain_keyring(&detail, matches!(e, keyring::Error::NoStorageAccess(_))))
}

/// The two problems people actually hit: a keyring that's locked and can't ask to be
/// unlocked (Fedora's oo7 after automatic login, 2026-11-16), and no keyring at all (a bare
/// window manager).
pub fn explain_keyring(detail: &str, no_access: bool) -> String {
    let d = detail.to_ascii_lowercase();
    if d.contains("prompt") || d.contains("locked") || d.contains("dismissed") {
        "Your keyring is locked, so Tern can’t save or read the password. Unlock it (open \
         Passwords and Keys, or log in to your desktop with your password instead of \
         automatic login), then try again."
            .into()
    } else if no_access || d.contains("org.freedesktop.secrets") || d.contains("serviceunknown") {
        "Tern keeps passwords in your desktop’s keyring, and none is running. Install and start \
         one (gnome-keyring, KWallet, or KeePassXC with Secret Service turned on), then try again."
            .into()
    } else {
        format!("Tern couldn’t use your keyring: {detail}")
    }
}

/// Save a password, app password, or API token for an account.
pub fn store_secret(account: AccountId, secret: &str) -> Result<()> {
    entry(account)?.set_password(secret).map_err(keyring_error)
}

pub fn store_tokens(account: AccountId, tokens: &OAuthTokens) -> Result<()> {
    let json = serde_json::to_string(tokens).map_err(|e| Error::Auth(e.to_string()))?;
    store_secret(account, &json)
}

pub fn delete_secret(account: AccountId) -> Result<()> {
    entry(account)?.delete_credential().map_err(keyring_error)
}

fn load_secret(account: AccountId) -> Result<String> {
    entry(account)?.get_password().map_err(|e| match e {
        keyring::Error::NoEntry => Error::Auth("no saved credentials; sign in again".into()),
        e => keyring_error(e),
    })
}

/// Load (refreshing OAuth tokens when needed) the credential for an account.
pub async fn credential_for(cfg: &AccountConfig) -> Result<Credential> {
    let secret = load_secret(cfg.id)?;
    Ok(match cfg.auth {
        AuthMethod::Password | AuthMethod::BearerToken => credential_from_secret(cfg, &secret),
        AuthMethod::OAuth2Google => {
            let mut tokens: OAuthTokens =
                serde_json::from_str(&secret).map_err(|e| Error::Auth(e.to_string()))?;
            if tokens.needs_refresh() {
                tokens = google::refresh(&tokens).await?;
                store_tokens(cfg.id, &tokens)?;
            }
            Credential::OAuth2 {
                username: cfg.username.clone(),
                access_token: tokens.access_token,
            }
        }
    })
}

/// The credential for a password or token account, cleaned the way the sign-in form cleans
/// what you type: a secret saved outside Tern (e.g. with `secret-tool`) can carry a newline,
/// which IMAP refuses to send.
pub fn credential_from_secret(cfg: &AccountConfig, secret: &str) -> Credential {
    let secret = cfg.clean_secret(secret);
    match cfg.auth {
        AuthMethod::BearerToken => Credential::Bearer { token: secret },
        _ => Credential::Password { username: cfg.username.clone(), password: secret },
    }
}

/// The SASL XOAUTH2 initial response (before base64), shared by IMAP and SMTP.
pub fn xoauth2_payload(user: &str, access_token: &str) -> String {
    format!("user={user}\x01auth=Bearer {access_token}\x01\x01")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_secrets() {
        let c = Credential::Password { username: "u".into(), password: "hunter2".into() };
        assert!(!format!("{c:?}").contains("hunter2"));
    }

    #[test]
    fn saved_secrets_are_cleaned() {
        use tern_core::account::Provider;
        let password = |c: Credential| match c {
            Credential::Password { password, .. } => password,
            other => panic!("{other:?}"),
        };
        // A Gmail app password pasted with its spaces and a trailing newline.
        let mut gmail = AccountConfig::preset(Provider::Gmail, "T", "t@gmail.com");
        gmail.auth = AuthMethod::Password;
        assert_eq!(password(credential_from_secret(&gmail, "abcd efgh ijkl mnop\n")), "abcdefghijklmnop");
        let other = AccountConfig::preset(Provider::Generic, "T", "t@example.com");
        assert_eq!(password(credential_from_secret(&other, "pa ss\r\n")), "pa ss");
        let fastmail = AccountConfig::preset(Provider::Fastmail, "T", "t@fastmail.com");
        assert!(matches!(credential_from_secret(&fastmail, "tok\n"), Credential::Bearer { token } if token == "tok"));
    }

    #[test]
    fn keyring_errors_say_what_to_do() {
        // Fedora 45's oo7 after automatic login (seen on the test VM, 2026-11-16).
        let oo7 = "Platform failure: zbus error: org.freedesktop.zbus.Error: org.freedesktop.DBus.Error.Failed: \
                   CLI prompter failed: org.freedesktop.DBus.Error.NameHasNoOwner: Destination does not exist";
        assert!(explain_keyring(oo7, false).starts_with("Your keyring is locked"));
        assert!(explain_keyring("Platform failure: prompt dismissed", false).contains("locked"));
        // No Secret Service at all.
        let none = "Platform failure: zbus error: org.freedesktop.DBus.Error.ServiceUnknown: \
                    The name org.freedesktop.secrets was not provided by any .service files";
        assert!(explain_keyring(none, false).contains("none is running"));
        assert!(explain_keyring("anything", true).contains("none is running"));
        // Anything else keeps its detail.
        assert_eq!(explain_keyring("odd", false), "Tern couldn’t use your keyring: odd");
    }

    #[test]
    fn xoauth2_format() {
        assert_eq!(xoauth2_payload("a@b", "t"), "user=a@b\x01auth=Bearer t\x01\x01");
    }

    #[test]
    fn refresh_window() {
        let t = OAuthTokens {
            access_token: "x".into(),
            refresh_token: None,
            expires_at: Some(Utc::now() + Duration::seconds(30)),
        };
        assert!(t.needs_refresh());
    }
}

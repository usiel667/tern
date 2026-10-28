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
    keyring::Entry::new(SERVICE, &account.to_string()).map_err(|e| Error::Auth(e.to_string()))
}

/// Save a password, app password, or API token for an account.
pub fn store_secret(account: AccountId, secret: &str) -> Result<()> {
    entry(account)?
        .set_password(secret)
        .map_err(|e| Error::Auth(e.to_string()))
}

pub fn store_tokens(account: AccountId, tokens: &OAuthTokens) -> Result<()> {
    let json = serde_json::to_string(tokens).map_err(|e| Error::Auth(e.to_string()))?;
    store_secret(account, &json)
}

pub fn delete_secret(account: AccountId) -> Result<()> {
    entry(account)?
        .delete_credential()
        .map_err(|e| Error::Auth(e.to_string()))
}

fn load_secret(account: AccountId) -> Result<String> {
    entry(account)?.get_password().map_err(|e| match e {
        keyring::Error::NoEntry => Error::Auth("no saved credentials; sign in again".into()),
        e => Error::Auth(e.to_string()),
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

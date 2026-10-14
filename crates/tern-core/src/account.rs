//! Account configuration. Secrets never live here — only a reference to the
//! system keyring entry (see `tern-auth`).

use serde::{Deserialize, Serialize};

use crate::model::{AccountId, Address};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Security {
    /// Implicit TLS (IMAPS 993 / SMTPS 465).
    Tls,
    /// Plain connection upgraded with STARTTLS (IMAP 143 / submission 587).
    StartTls,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub security: Security,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMethod {
    /// Password or app-specific password, stored in the keyring.
    Password,
    /// OAuth2 access/refresh tokens in the keyring, sent as SASL XOAUTH2.
    OAuth2Google,
    /// JMAP bearer API token (Fastmail).
    BearerToken,
}

/// Which protocol family an account speaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Transport {
    Imap {
        imap: ServerConfig,
        smtp: ServerConfig,
    },
    Jmap {
        /// Session URL, e.g. `https://api.fastmail.com/jmap/session`.
        session_url: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Generic,
    Gmail,
    ICloud,
    Fastmail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountConfig {
    pub id: AccountId,
    /// Sidebar label ("iCloud", "Work").
    pub description: String,
    pub provider: Provider,
    pub identity: Address,
    /// Login name; usually the email address.
    pub username: String,
    pub auth: AuthMethod,
    pub transport: Transport,
    /// Plain-text signature appended in compose. Apple Mail supports per-account
    /// signatures; rich signatures come later.
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl AccountConfig {
    /// Fill in server settings for the well-known providers, the way Apple Mail's
    /// "Add Account" sheet does.
    pub fn preset(provider: Provider, name: &str, email: &str) -> Self {
        let identity = Address::new(Some(name), email);
        let (description, auth, transport) = match provider {
            Provider::Gmail => (
                "Google",
                AuthMethod::OAuth2Google,
                Transport::Imap {
                    imap: server("imap.gmail.com", 993, Security::Tls),
                    smtp: server("smtp.gmail.com", 465, Security::Tls),
                },
            ),
            Provider::ICloud => (
                "iCloud",
                AuthMethod::Password,
                Transport::Imap {
                    imap: server("imap.mail.me.com", 993, Security::Tls),
                    smtp: server("smtp.mail.me.com", 587, Security::StartTls),
                },
            ),
            Provider::Fastmail => (
                "Fastmail",
                AuthMethod::BearerToken,
                Transport::Jmap {
                    session_url: "https://api.fastmail.com/jmap/session".into(),
                },
            ),
            Provider::Generic => {
                let domain = email.rsplit('@').next().unwrap_or("example.com");
                (
                    "Mail",
                    AuthMethod::Password,
                    Transport::Imap {
                        imap: server(&format!("imap.{domain}"), 993, Security::Tls),
                        smtp: server(&format!("smtp.{domain}"), 465, Security::Tls),
                    },
                )
            }
        };
        // iCloud logs in with the part before @ for @icloud.com/@me.com addresses,
        // but the full address also works; keep the full address everywhere.
        Self {
            id: AccountId::new(),
            description: description.into(),
            provider,
            identity,
            username: email.into(),
            auth,
            transport,
            signature: None,
            enabled: true,
        }
    }
}

fn server(host: &str, port: u16, security: Security) -> ServerConfig {
    ServerConfig {
        host: host.into(),
        port,
        security,
    }
}

/// Contents of `~/.config/tern/accounts.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountsFile {
    #[serde(default, rename = "account")]
    pub accounts: Vec<AccountConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_pick_correct_transport() {
        let g = AccountConfig::preset(Provider::Gmail, "T", "t@gmail.com");
        assert_eq!(g.auth, AuthMethod::OAuth2Google);
        let f = AccountConfig::preset(Provider::Fastmail, "T", "t@fastmail.com");
        assert!(matches!(f.transport, Transport::Jmap { .. }));
        let x = AccountConfig::preset(Provider::Generic, "T", "t@example.org");
        match x.transport {
            Transport::Imap { imap, .. } => assert_eq!(imap.host, "imap.example.org"),
            _ => panic!(),
        }
    }
}

//! Where Tern keeps things, following the XDG base directory spec.

use std::path::PathBuf;

use tern_core::account::AccountsFile;
use tern_core::{Error, Result};

#[derive(Debug, Clone)]
pub struct Paths {
    /// `~/.config/tern/accounts.toml`
    pub accounts: PathBuf,
    /// `~/.local/share/tern/mail.db`
    pub database: PathBuf,
}

impl Paths {
    pub fn xdg() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("dev", "tern", "tern")
            .ok_or_else(|| Error::Config("no home directory".into()))?;
        Ok(Self {
            accounts: dirs.config_dir().join("accounts.toml"),
            database: dirs.data_dir().join("mail.db"),
        })
    }

    /// Everything under one directory (tests, `--profile`).
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        Self {
            accounts: dir.join("accounts.toml"),
            database: dir.join("mail.db"),
        }
    }

    pub fn load_accounts(&self) -> Result<AccountsFile> {
        match std::fs::read_to_string(&self.accounts) {
            Ok(s) => toml::from_str(&s).map_err(|e| Error::Config(format!("{}: {e}", self.accounts.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AccountsFile::default()),
            Err(e) => Err(Error::Config(e.to_string())),
        }
    }

    pub fn save_accounts(&self, file: &AccountsFile) -> Result<()> {
        if let Some(dir) = self.accounts.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::Config(e.to_string()))?;
        }
        let s = toml::to_string_pretty(file).map_err(|e| Error::Config(e.to_string()))?;
        // Write-then-rename so a crash never leaves a truncated config.
        let tmp = self.accounts.with_extension("toml.tmp");
        std::fs::write(&tmp, s).map_err(|e| Error::Config(e.to_string()))?;
        std::fs::rename(&tmp, &self.accounts).map_err(|e| Error::Config(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_core::account::{AccountConfig, Provider};

    #[test]
    fn accounts_round_trip() {
        let dir = std::env::temp_dir().join(format!("tern-test-{}", std::process::id()));
        let p = Paths::in_dir(&dir);
        assert!(p.load_accounts().unwrap().accounts.is_empty());
        let file = AccountsFile {
            accounts: vec![
                AccountConfig::preset(Provider::ICloud, "Tom", "tom@icloud.com"),
                AccountConfig::preset(Provider::Fastmail, "Tom", "tom@fastmail.com"),
            ],
        };
        p.save_accounts(&file).unwrap();
        assert_eq!(p.load_accounts().unwrap(), file);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

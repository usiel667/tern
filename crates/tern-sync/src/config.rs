//! Where Tern keeps things, following the XDG base directory spec.

use std::path::PathBuf;

use tern_core::account::AccountsFile;
use tern_core::settings::Settings;
use tern_core::{Error, Result};

#[derive(Debug, Clone)]
pub struct Paths {
    /// `~/.config/tern/accounts.toml`
    pub accounts: PathBuf,
    /// `~/.config/tern/settings.toml`
    pub settings: PathBuf,
    /// `~/.local/share/tern/mail.db`
    pub database: PathBuf,
}

impl Paths {
    pub fn xdg() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("dev", "tern", "tern")
            .ok_or_else(|| Error::Config("no home directory".into()))?;
        Ok(Self {
            accounts: dirs.config_dir().join("accounts.toml"),
            settings: dirs.config_dir().join("settings.toml"),
            database: dirs.data_dir().join("mail.db"),
        })
    }

    /// Everything under one directory (tests, `--profile`).
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        Self {
            accounts: dir.join("accounts.toml"),
            settings: dir.join("settings.toml"),
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
        write_toml(&self.accounts, file)
    }

    /// The app settings; defaults if the file is missing (or unreadable: they're only preferences).
    pub fn load_settings(&self) -> Settings {
        std::fs::read_to_string(&self.settings).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        write_toml(&self.settings, settings)
    }
}

fn write_toml(path: &std::path::Path, value: &impl serde::Serialize) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::Config(e.to_string()))?;
    }
    let s = toml::to_string_pretty(value).map_err(|e| Error::Config(e.to_string()))?;
    // Write-then-rename so a crash never leaves a truncated config.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, s).map_err(|e| Error::Config(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| Error::Config(e.to_string()))
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

        assert_eq!(p.load_settings(), Settings::default(), "no file yet");
        let mut s = Settings { load_remote_content: true, darken_html: false, zoom_percent: 125, mailboxes_width: Some(240), list_width: None, vim_keys: true, ..Settings::default() };
        s.keys.insert("win.reply".into(), vec!["<Control>e".into()]);
        s.keys.insert("vim.top".into(), vec![]);
        p.save_settings(&s).unwrap();
        assert_eq!(p.load_settings(), s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

//! App-wide preferences, kept in settings.toml (accounts live in accounts.toml).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Load remote images and fonts in every message without asking (tracking pixels too).
    pub load_remote_content: bool,
    /// In dark mode, darken HTML mail that has no dark styles of its own.
    pub darken_html: bool,
    /// Size of the whole app (Ctrl+= / Ctrl+- / Ctrl+0), in percent of the desktop's.
    pub zoom_percent: u32,
    /// Widths the Mailboxes and message-list columns were dragged to, in pixels (unset: automatic).
    pub mailboxes_width: Option<u32>,
    pub list_width: Option<u32>,
    /// Vim keys across the main window: h/l between panes, j/k, gg/G, Ctrl+D/U, dd, u undo.
    pub vim_keys: bool,
    /// Shortcuts changed in Settings → Keyboard, by id (`keys::SHORTCUTS`); the rest keep
    /// their defaults.
    pub keys: crate::keys::Overrides,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            load_remote_content: false,
            darken_html: true,
            zoom_percent: 100,
            mailboxes_width: None,
            list_width: None,
            vim_keys: false,
            keys: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn missing_keys_take_the_defaults() {
        let s: Settings = toml::from_str("load_remote_content = true").unwrap();
        assert_eq!(s, Settings { load_remote_content: true, ..Settings::default() });
        assert_eq!(toml::from_str::<Settings>("").unwrap(), Settings::default());
        // A key from an older Tern is ignored.
        assert_eq!(toml::from_str::<Settings>("smooth_scrolling = false").unwrap(), Settings::default());
    }
}

//! Keyboard shortcuts: every one Tern has, its defaults, and the owner's changes
//! (`Settings::keys`). Bindings are GTK accelerator names ("<Control>r", "<Shift>g", "slash");
//! a Vim sequence is two of them with a space ("g g"). The app turns key presses into the same
//! form (`gtk::accelerator_name`), and `normalize` makes the two comparable.

use std::collections::BTreeMap;

/// Where a shortcut works. Keys only clash within a group, or with an app-wide shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    /// Anywhere in Tern (also while typing), so they need Ctrl, Alt or Super.
    App,
    /// In a New Message window, also while typing.
    NewMessage,
    /// Single keys while the message list has the focus.
    MessageList,
    /// Vim keys (when turned on), anywhere in the main window except while typing.
    Vim,
}

impl Group {
    pub const ALL: [Group; 4] = [Group::App, Group::NewMessage, Group::MessageList, Group::Vim];

    pub fn title(self) -> &'static str {
        match self {
            Group::App => "Everywhere",
            Group::NewMessage => "New Message",
            Group::MessageList => "Message List",
            Group::Vim => "Vim Keys",
        }
    }

    fn clashes_with(self, other: Group) -> bool {
        self == other || self == Group::App || other == Group::App
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    /// Stable name, saved in settings.toml. App shortcuts use their GTK action name.
    pub id: &'static str,
    pub title: &'static str,
    pub group: Group,
    pub defaults: &'static [&'static str],
}

const fn s(id: &'static str, title: &'static str, group: Group, defaults: &'static [&'static str]) -> Shortcut {
    Shortcut { id, title, group, defaults }
}

/// Every shortcut, in the order Settings lists them.
pub const SHORTCUTS: &[Shortcut] = &[
    s("win.compose", "New Message", Group::App, &["<Control>n"]),
    s("win.reply", "Reply", Group::App, &["<Control>r"]),
    s("win.reply-all", "Reply All", Group::App, &["<Shift><Control>r"]),
    s("win.forward", "Forward", Group::App, &["<Shift><Control>f"]),
    s("win.get-mail", "Get Mail", Group::App, &["<Shift><Control>n"]),
    s("win.toggle-read", "Mark as Read / Unread", Group::App, &["<Shift><Control>u"]),
    s("win.toggle-flag", "Flag", Group::App, &["<Shift><Control>l"]),
    s("win.archive", "Archive", Group::App, &["<Control><Alt>a"]),
    s("win.junk", "Move to Junk", Group::App, &["<Shift><Control>j"]),
    s("win.delete", "Move to Trash", Group::App, &["<Control>BackSpace"]),
    s("win.undo", "Undo", Group::App, &["<Control>z"]),
    s("win.search", "Search", Group::App, &["<Control>f", "<Control><Alt>f"]),
    s("win.goto-inbox", "Go to Inboxes", Group::App, &["<Control>1"]),
    s("win.toggle-sidebar", "Show / Hide Mailboxes", Group::App, &["<Shift><Control>m"]),
    s("win.zoom-in", "Zoom In", Group::App, &["<Control>equal", "<Control>plus", "<Control>KP_Add"]),
    s("win.zoom-out", "Zoom Out", Group::App, &["<Control>minus", "<Control>KP_Subtract"]),
    s("win.zoom-reset", "Actual Size", Group::App, &["<Control>0", "<Control>KP_0"]),
    s("win.preferences", "Settings", Group::App, &["<Control>comma"]),
    s("app.quit", "Quit", Group::App, &["<Control>q"]),
    s("compose.send", "Send", Group::NewMessage, &["<Control>Return", "<Control>KP_Enter"]),
    s("compose.save", "Save to Drafts", Group::NewMessage, &["<Control>s"]),
    s("compose.attach", "Attach Files", Group::NewMessage, &["<Shift><Control>a"]),
    s("list.delete", "Move to Trash", Group::MessageList, &["Delete", "BackSpace"]),
    s("list.next", "Next Message", Group::MessageList, &["j"]),
    s("list.previous", "Previous Message", Group::MessageList, &["k"]),
    s("list.search", "Search", Group::MessageList, &["slash"]),
    s("list.reply", "Reply", Group::MessageList, &["r"]),
    s("list.reply-all", "Reply All", Group::MessageList, &["a"]),
    s("list.forward", "Forward", Group::MessageList, &["f"]),
    s("list.archive", "Archive", Group::MessageList, &["e"]),
    s("list.toggle-read", "Mark as Read / Unread", Group::MessageList, &["u"]),
    s("list.toggle-flag", "Flag", Group::MessageList, &["s"]),
    s("vim.left", "Pane to the left", Group::Vim, &["h"]),
    s("vim.right", "Pane to the right", Group::Vim, &["l", "o"]),
    s("vim.down", "Down", Group::Vim, &["j"]),
    s("vim.up", "Up", Group::Vim, &["k"]),
    s("vim.top", "Top", Group::Vim, &["g g"]),
    s("vim.bottom", "Bottom", Group::Vim, &["<Shift>g"]),
    s("vim.half-down", "Half a page down", Group::Vim, &["<Control>d"]),
    s("vim.half-up", "Half a page up", Group::Vim, &["<Control>u"]),
    s("vim.search", "Search", Group::Vim, &["slash"]),
    s("vim.undo", "Undo", Group::Vim, &["u"]),
    s("vim.toggle-read", "Mark as Read / Unread", Group::Vim, &["<Shift>u"]),
    s("vim.delete", "Move to Trash", Group::Vim, &["d d"]),
    s("vim.reply", "Reply", Group::Vim, &["r"]),
    s("vim.reply-all", "Reply All", Group::Vim, &["a"]),
    s("vim.forward", "Forward", Group::Vim, &["f"]),
    s("vim.archive", "Archive", Group::Vim, &["e"]),
    s("vim.toggle-flag", "Flag", Group::Vim, &["s"]),
];

pub fn shortcut(id: &str) -> Option<&'static Shortcut> {
    SHORTCUTS.iter().find(|s| s.id == id)
}

/// The owner's changes, by shortcut id (an empty list turns a shortcut off).
pub type Overrides = BTreeMap<String, Vec<String>>;

/// The keys a shortcut has now, normalized.
pub fn bindings(sc: &Shortcut, overrides: &Overrides) -> Vec<String> {
    match overrides.get(sc.id) {
        Some(keys) => keys.iter().map(|k| normalize(k)).collect(),
        None => sc.defaults.iter().map(|k| normalize(k)).collect(),
    }
}

/// One spelling per key: modifiers in a fixed order (`<Primary>`/`<Ctrl>` = `<Control>`),
/// single letters lower case. Sequences keep their order.
pub fn normalize(binding: &str) -> String {
    binding.split_whitespace().map(normalize_stroke).collect::<Vec<_>>().join(" ")
}

const MODIFIERS: [&str; 4] = ["Control", "Shift", "Alt", "Super"];

fn normalize_stroke(stroke: &str) -> String {
    let mut mods = [false; 4];
    let mut rest = stroke;
    while let Some(inner) = rest.strip_prefix('<') {
        let Some(end) = inner.find('>') else { break };
        let name = match inner[..end].to_ascii_lowercase().as_str() {
            "primary" | "ctrl" | "control" => "Control",
            "mod1" | "alt" => "Alt",
            "meta" | "super" => "Super",
            "shift" => "Shift",
            _ => "",
        };
        if let Some(i) = MODIFIERS.iter().position(|m| m.eq_ignore_ascii_case(name)) {
            mods[i] = true;
        }
        rest = &inner[end + 1..];
    }
    let key = if rest.chars().count() == 1 { rest.to_ascii_lowercase() } else { rest.to_string() };
    let mut out = String::new();
    for (m, on) in MODIFIERS.iter().zip(mods) {
        if on {
            out.push_str(&format!("<{m}>"));
        }
    }
    out + &key
}

/// Why a key can't be used for a shortcut in this group, if it can't.
pub fn check(group: Group, binding: &str) -> Result<(), &'static str> {
    let strokes: Vec<&str> = binding.split_whitespace().collect();
    match (group, strokes.len()) {
        (_, 0) => Err("No key pressed."),
        (Group::Vim, 1 | 2) => Ok(()),
        (Group::Vim, _) => Err("Vim keys are one key or two in a row."),
        (_, n) if n > 1 => Err("Use a single key here."),
        (Group::App | Group::NewMessage, _) => {
            let s = normalize(strokes[0]);
            let key = s.rsplit('>').next().unwrap_or("");
            let function_key = key.len() > 1 && key.starts_with('F') && key[1..].chars().all(|c| c.is_ascii_digit());
            if s.contains("<Control>") || s.contains("<Alt>") || s.contains("<Super>") || function_key {
                Ok(())
            } else {
                Err("These work while you type, so they need Ctrl, Alt or Super.")
            }
        }
        (Group::MessageList, _) => Ok(()),
    }
}

/// Shortcuts that would clash with giving `id` the key `binding`: the same key where both
/// work, or (Vim) one key starting the other's sequence.
pub fn conflicts(id: &str, binding: &str, overrides: &Overrides) -> Vec<&'static Shortcut> {
    let Some(me) = shortcut(id) else { return Vec::new() };
    let new = normalize(binding);
    SHORTCUTS
        .iter()
        .filter(|other| other.id != id && me.group.clashes_with(other.group))
        .filter(|other| {
            bindings(other, overrides).iter().any(|b| {
                *b == new
                    || (me.group == Group::Vim && other.group == Group::Vim && (b.starts_with(&format!("{new} ")) || new.starts_with(&format!("{b} "))))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spellings_meet() {
        assert_eq!(normalize("<Primary><Shift>R"), "<Control><Shift>r");
        assert_eq!(normalize("<Shift><Control>r"), "<Control><Shift>r");
        assert_eq!(normalize("<ctrl>BackSpace"), "<Control>BackSpace");
        assert_eq!(normalize("g  g"), "g g");
        assert_eq!(normalize("slash"), "slash");
    }

    #[test]
    fn ids_are_unique_and_defaults_are_valid() {
        let mut ids: Vec<_> = SHORTCUTS.iter().map(|s| s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), SHORTCUTS.len());
        for sc in SHORTCUTS {
            for d in sc.defaults {
                assert_eq!(check(sc.group, d), Ok(()), "{} {d}", sc.id);
                assert!(conflicts(sc.id, d, &Overrides::new()).is_empty(), "{} {d} clashes by default", sc.id);
            }
        }
    }

    #[test]
    fn app_shortcuts_need_a_modifier() {
        assert!(check(Group::App, "r").is_err());
        assert!(check(Group::NewMessage, "<Shift>s").is_err());
        assert_eq!(check(Group::App, "<Alt>r"), Ok(()));
        assert_eq!(check(Group::App, "F5"), Ok(()));
        assert!(check(Group::App, "<Control>a <Control>b").is_err());
        assert_eq!(check(Group::MessageList, "x"), Ok(()));
        assert_eq!(check(Group::Vim, "z z"), Ok(()));
        assert!(check(Group::Vim, "a b c").is_err());
    }

    #[test]
    fn overrides_replace_defaults_and_clashes_are_found() {
        let mut o = Overrides::new();
        o.insert("win.reply".into(), vec!["<Primary>e".into()]);
        let reply = shortcut("win.reply").unwrap();
        assert_eq!(bindings(reply, &o), ["<Control>e"]);
        // Ctrl+E is now Reply's: taking it for Compose clashes, Ctrl+R is free.
        let ids = |v: Vec<&Shortcut>| v.iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(ids(conflicts("win.compose", "<Control>e", &o)), ["win.reply"]);
        assert!(conflicts("win.compose", "<Control>r", &o).is_empty());
        // App keys clash with every group; the list and Vim don't clash with each other.
        assert_eq!(ids(conflicts("list.reply", "<Control>z", &o)), ["win.undo"]);
        assert!(conflicts("vim.down", "r", &o).iter().all(|s| s.group == Group::Vim));
        assert_eq!(ids(conflicts("vim.down", "u", &o)), ["vim.undo"]);
        // Vim: a key that starts another's sequence clashes, and the other way round.
        assert_eq!(ids(conflicts("vim.down", "g", &o)), ["vim.top"]);
        assert_eq!(ids(conflicts("vim.top", "h x", &o)), ["vim.left"]);
        o.insert("win.undo".into(), vec![]);
        assert!(bindings(shortcut("win.undo").unwrap(), &o).is_empty(), "turned off");
    }
}

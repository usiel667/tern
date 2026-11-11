//! Color themes (Settings → Appearance). Each is ten colors that the app's theme CSS and
//! the message view are filled in from. The palettes are the themes' own published colors
//! (MIT licensed: Tokyo Night, Catppuccin, Rosé Pine, Gruvbox, Nord, Dracula, Everforest,
//! Kanagawa).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Saved in settings.toml.
    pub id: &'static str,
    pub name: &'static str,
    /// Dark themes switch the app to dark, light ones to light.
    pub dark: bool,
    /// Window and message background.
    pub bg: &'static str,
    /// The Mailboxes column.
    pub sidebar: &'static str,
    /// Hover and selection.
    pub highlight: &'static str,
    pub text: &'static str,
    /// Icons and secondary text.
    pub dim: &'static str,
    /// Previews, counts, section titles.
    pub faint: &'static str,
    /// Senders, the selected mailbox, the Send button.
    pub purple: &'static str,
    /// The selected sender, destructive buttons.
    pub red: &'static str,
    /// Dates.
    pub amber: &'static str,
    /// Accent: unread dots, links.
    pub accent: &'static str,
}

#[allow(clippy::too_many_arguments)]
const fn p(
    id: &'static str,
    name: &'static str,
    dark: bool,
    [bg, sidebar, highlight, text, dim, faint, purple, red, amber, accent]: [&'static str; 10],
) -> Palette {
    Palette { id, name, dark, bg, sidebar, highlight, text, dim, faint, purple, red, amber, accent }
}

/// Every theme, in the order Settings lists them.
pub const THEMES: &[Palette] = &[
    // Tokyo Night "Night" as the owner's Obsidian has it (red and accent from that theme).
    p("tokyo-night", "Tokyo Night", true, ["#1a1b26", "#16161e", "#292e42", "#c0caf5", "#a9b1d6", "#565f89", "#bb9af7", "#ff757f", "#e0af68", "#80d0ff"]),
    p("tokyo-night-storm", "Tokyo Night Storm", true, ["#24283b", "#1f2335", "#292e42", "#c0caf5", "#a9b1d6", "#565f89", "#bb9af7", "#f7768e", "#e0af68", "#7dcfff"]),
    p("tokyo-night-moon", "Tokyo Night Moon", true, ["#222436", "#1e2030", "#2f334d", "#c8d3f5", "#828bb8", "#636da6", "#c099ff", "#ff757f", "#ffc777", "#86e1fc"]),
    p("tokyo-night-day", "Tokyo Night Day", false, ["#e1e2e7", "#d0d5e3", "#c4c8da", "#3760bf", "#6172b0", "#848cb5", "#9854f1", "#f52a65", "#8c6c3e", "#2e7de9"]),
    p("catppuccin-mocha", "Catppuccin Mocha", true, ["#1e1e2e", "#181825", "#313244", "#cdd6f4", "#bac2de", "#6c7086", "#cba6f7", "#f38ba8", "#f9e2af", "#89b4fa"]),
    p("catppuccin-macchiato", "Catppuccin Macchiato", true, ["#24273a", "#1e2030", "#363a4f", "#cad3f5", "#b8c0e0", "#6e738d", "#c6a0f6", "#ed8796", "#eed49f", "#8aadf4"]),
    p("catppuccin-frappe", "Catppuccin Frappé", true, ["#303446", "#292c3c", "#414559", "#c6d0f5", "#b5bfe2", "#737994", "#ca9ee6", "#e78284", "#e5c890", "#8caaee"]),
    p("catppuccin-latte", "Catppuccin Latte", false, ["#eff1f5", "#e6e9ef", "#ccd0da", "#4c4f69", "#5c5f77", "#9ca0b0", "#8839ef", "#d20f39", "#df8e1d", "#1e66f5"]),
    p("rose-pine", "Rosé Pine", true, ["#191724", "#1f1d2e", "#403d52", "#e0def4", "#908caa", "#6e6a86", "#c4a7e7", "#eb6f92", "#f6c177", "#9ccfd8"]),
    p("rose-pine-moon", "Rosé Pine Moon", true, ["#232136", "#2a273f", "#44415a", "#e0def4", "#908caa", "#6e6a86", "#c4a7e7", "#eb6f92", "#f6c177", "#9ccfd8"]),
    p("rose-pine-dawn", "Rosé Pine Dawn", false, ["#faf4ed", "#fffaf3", "#dfdad9", "#575279", "#797593", "#9893a5", "#907aa9", "#b4637a", "#ea9d34", "#56949f"]),
    p("gruvbox-dark", "Gruvbox Dark", true, ["#282828", "#1d2021", "#3c3836", "#ebdbb2", "#d5c4a1", "#928374", "#d3869b", "#fb4934", "#fabd2f", "#83a598"]),
    p("gruvbox-light", "Gruvbox Light", false, ["#fbf1c7", "#f2e5bc", "#ebdbb2", "#3c3836", "#504945", "#928374", "#8f3f71", "#9d0006", "#b57614", "#076678"]),
    p("nord", "Nord", true, ["#2e3440", "#292e39", "#434c5e", "#eceff4", "#d8dee9", "#616e88", "#b48ead", "#bf616a", "#ebcb8b", "#88c0d0"]),
    p("dracula", "Dracula", true, ["#282a36", "#21222c", "#44475a", "#f8f8f2", "#e2e2dc", "#6272a4", "#bd93f9", "#ff5555", "#ffb86c", "#8be9fd"]),
    p("everforest-dark", "Everforest Dark", true, ["#2d353b", "#232a2e", "#3d484d", "#d3c6aa", "#9da9a0", "#7a8478", "#d699b6", "#e67e80", "#dbbc7f", "#7fbbb3"]),
    p("kanagawa-wave", "Kanagawa Wave", true, ["#1f1f28", "#16161d", "#363646", "#dcd7ba", "#c8c093", "#727169", "#957fb8", "#e46876", "#e6c384", "#7fb4ca"]),
];

/// Follow the desktop: Tokyo Night when it's dark, plain Adwaita when it's light.
pub const SYSTEM: &str = "system";
pub const ADWAITA_LIGHT: &str = "adwaita-light";
pub const ADWAITA_DARK: &str = "adwaita-dark";

/// Light or dark, as the app asks libadwaita for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    FollowDesktop,
    Light,
    Dark,
}

pub fn find(id: &str) -> Option<&'static Palette> {
    THEMES.iter().find(|p| p.id == id)
}

/// Every choice Settings offers: (id, name), System and plain Adwaita first.
pub fn choices() -> Vec<(&'static str, &'static str)> {
    let mut v = vec![(SYSTEM, "System"), (ADWAITA_LIGHT, "Adwaita Light"), (ADWAITA_DARK, "Adwaita Dark")];
    v.extend(THEMES.iter().map(|p| (p.id, p.name)));
    v
}

/// What a theme choice means: the scheme to ask for, and the palette in use when the app's
/// look is `dark` (from libadwaita, after the scheme took effect). Unknown ids act as System.
pub fn resolve(id: &str, dark: bool) -> (Scheme, Option<&'static Palette>) {
    match id {
        ADWAITA_LIGHT => (Scheme::Light, None),
        ADWAITA_DARK => (Scheme::Dark, None),
        _ => match find(id) {
            Some(p) => (if p.dark { Scheme::Dark } else { Scheme::Light }, Some(p)),
            None => (Scheme::FollowDesktop, dark.then(|| find("tokyo-night")).flatten()),
        },
    }
}

/// Fill a CSS template: `@bg@`, `@text@`… become the palette's colors.
pub fn fill(template: &str, p: &Palette) -> String {
    [
        ("@bg@", p.bg),
        ("@sidebar@", p.sidebar),
        ("@highlight@", p.highlight),
        ("@text@", p.text),
        ("@dim@", p.dim),
        ("@faint@", p.faint),
        ("@purple@", p.purple),
        ("@red@", p.red),
        ("@amber@", p.amber),
        ("@accent@", p.accent),
    ]
    .iter()
    .fold(template.to_string(), |css, (k, v)| css.replace(k, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_are_well_formed() {
        let mut ids: Vec<_> = choices().iter().map(|c| c.0).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), THEMES.len() + 3, "unique ids");
        for p in THEMES {
            for c in [p.bg, p.sidebar, p.highlight, p.text, p.dim, p.faint, p.purple, p.red, p.amber, p.accent] {
                assert!(c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|h| h.is_ascii_hexdigit()), "{} {c}", p.id);
            }
            // Text must stand out from the background: dark themes light text, light dark.
            let lum = |hex: &str| {
                let v = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap() as f32;
                0.2126 * v(1) + 0.7152 * v(3) + 0.0722 * v(5)
            };
            assert_eq!(lum(p.text) > lum(p.bg), p.dark, "{}", p.id);
            assert!((lum(p.text) - lum(p.bg)).abs() > 100.0, "{} contrast", p.id);
        }
    }

    #[test]
    fn choices_resolve() {
        assert_eq!(resolve(SYSTEM, false), (Scheme::FollowDesktop, None));
        assert_eq!(resolve(SYSTEM, true).1.map(|p| p.id), Some("tokyo-night"));
        assert_eq!(resolve("nonsense", true).0, Scheme::FollowDesktop);
        assert_eq!(resolve(ADWAITA_DARK, true), (Scheme::Dark, None));
        assert_eq!(resolve("catppuccin-latte", true).0, Scheme::Light);
        assert_eq!(resolve("nord", false).1.map(|p| p.id), Some("nord"));
    }

    #[test]
    fn templates_fill() {
        let p = find("dracula").unwrap();
        assert_eq!(fill("a{color:@text@;background:@bg@}", p), "a{color:#f8f8f2;background:#282a36}");
    }
}

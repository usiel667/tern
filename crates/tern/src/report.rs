//! Report a Bug: a new GitHub issue (the bug form in .github/ISSUE_TEMPLATE) with what a bug
//! report needs to know about this machine filled in. Nothing personal: no accounts,
//! addresses, host or user name, and no mail. The person sees it all before sending.

/// Where issues are filed.
const NEW_ISSUE: &str = "https://github.com/usiel667/tern/issues/new";

/// The facts filled into the form's Environment field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    pub version: String,
    pub flatpak: bool,
    pub distro: String,
    pub desktop: String,
    pub session: String,
    pub gtk: String,
    pub adwaita: String,
    pub webkit: String,
}

impl Environment {
    /// This machine (GTK must be up: the library versions come from it).
    pub fn here() -> Self {
        Environment {
            gtk: format!("{}.{}.{}", gtk::major_version(), gtk::minor_version(), gtk::micro_version()),
            adwaita: format!("{}.{}.{}", adw::major_version(), adw::minor_version(), adw::micro_version()),
            webkit: format!(
                "{}.{}.{}",
                webkit::functions::major_version(),
                webkit::functions::minor_version(),
                webkit::functions::micro_version()
            ),
            ..Self::system()
        }
    }

    /// Version, packaging, distribution and desktop; no library versions.
    fn system() -> Self {
        let flatpak = std::path::Path::new("/.flatpak-info").exists();
        // In a Flatpak /etc/os-release is the runtime's; the host's is under /run/host.
        let files: &[&str] = if flatpak { &["/run/host/os-release"] } else { &["/etc/os-release", "/usr/lib/os-release"] };
        let distro = files
            .iter()
            .find_map(|f| std::fs::read_to_string(f).ok().and_then(|t| pretty_name(&t)))
            .unwrap_or_else(|| "unknown".into());
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "unknown".into());
        Environment {
            version: env!("CARGO_PKG_VERSION").into(),
            flatpak,
            distro,
            desktop: var("XDG_CURRENT_DESKTOP"),
            session: var("XDG_SESSION_TYPE"),
            gtk: String::new(),
            adwaita: String::new(),
            webkit: String::new(),
        }
    }

    /// As the form's Environment field shows it.
    pub fn text(&self) -> String {
        format!(
            "Tern: {}{}\nDistribution: {}\nDesktop: {} ({})\nGTK {}, libadwaita {}, WebKitGTK {}",
            self.version,
            if self.flatpak { " (Flatpak)" } else { "" },
            self.distro,
            self.desktop,
            self.session,
            self.gtk,
            self.adwaita,
            self.webkit
        )
    }
}

/// The new-issue page with the bug form and `env` filled in.
pub fn issue_url(env: &Environment) -> String {
    format!("{NEW_ISSUE}?template=bug_report.yml&environment={}", encode(&env.text()))
}

/// `PRETTY_NAME` from an os-release file ("Fedora Linux 45 (KDE Plasma Desktop Edition)").
fn pretty_name(os_release: &str) -> Option<String> {
    os_release.lines().find_map(|l| {
        let v = l.strip_prefix("PRETTY_NAME=")?.trim();
        let v = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(v);
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// Percent-encode everything but unreserved characters (RFC 3986).
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Environment {
        Environment {
            version: "0.1.0".into(),
            flatpak: true,
            distro: "Fedora Linux 45 (KDE Plasma Desktop Edition)".into(),
            desktop: "KDE".into(),
            session: "wayland".into(),
            gtk: "4.24.1".into(),
            adwaita: "1.10.0".into(),
            webkit: "2.54.2".into(),
        }
    }

    #[test]
    fn the_form_gets_the_environment() {
        let env = sample();
        assert_eq!(
            env.text(),
            "Tern: 0.1.0 (Flatpak)\nDistribution: Fedora Linux 45 (KDE Plasma Desktop Edition)\n\
             Desktop: KDE (wayland)\nGTK 4.24.1, libadwaita 1.10.0, WebKitGTK 2.54.2"
        );
        let url = issue_url(&env);
        assert!(url.starts_with("https://github.com/usiel667/tern/issues/new?template=bug_report.yml&environment=Tern%3A%200.1.0%20%28Flatpak%29%0ADistribution"));
        assert!(!url.contains(' ') && !url.contains('\n'));
    }

    #[test]
    fn distro_from_os_release() {
        assert_eq!(pretty_name("NAME=Arch\nPRETTY_NAME=\"Arch Linux\"\nID=arch\n").as_deref(), Some("Arch Linux"));
        assert_eq!(pretty_name("PRETTY_NAME=Debian\n").as_deref(), Some("Debian"));
        assert_eq!(pretty_name("NAME=x\n"), None);
    }

    #[test]
    fn this_machine_says_nothing_personal() {
        let text = Environment::system().text();
        for secret in [std::env::var("USER").unwrap_or_default(), std::env::var("HOME").unwrap_or_default()] {
            if secret.len() > 2 {
                assert!(!text.contains(&secret), "{text}");
            }
        }
        assert_eq!(text.lines().count(), 4);
    }
}

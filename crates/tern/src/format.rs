//! Pure helpers: dates, address parsing, and the HTML document the viewer loads.
//! Kept free of GTK so they're unit-testable.

use base64::Engine as _;
use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use tern_core::model::{Address, MessageBody};

/// Message-list date, Apple Mail style: time today, "Yesterday", weekday this
/// week, otherwise a short date.
pub fn list_date(dt: DateTime<Utc>, now: DateTime<Local>) -> String {
    let local = dt.with_timezone(&Local);
    let (d, today) = (local.date_naive(), now.date_naive());
    let days = (today - d).num_days();
    if days <= 0 {
        local.format("%-I:%M %p").to_string()
    } else if days == 1 {
        "Yesterday".into()
    } else if days < 7 {
        local.format("%A").to_string()
    } else if d.year() == today.year() {
        local.format("%b %-d").to_string()
    } else {
        local.format("%-m/%-d/%y").to_string()
    }
}

/// Viewer header date: "October 14, 2026 at 3:42 PM".
pub fn full_date(dt: DateTime<Utc>) -> String {
    dt.with_timezone(&Local).format("%B %-d, %Y at %-I:%M %p").to_string()
}

/// Parse a comma/semicolon separated recipient field:
/// `Ann <ann@x.com>, bob@x.com; "Lee, Cat" <cat@x.com>`.
pub fn parse_addresses(input: &str) -> Result<Vec<Address>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut angle = false;
    for c in input.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            '<' => {
                angle = true;
                cur.push(c);
            }
            '>' => {
                angle = false;
                cur.push(c);
            }
            ',' | ';' if !quoted && !angle => {
                push_addr(&mut out, &cur)?;
                cur.clear();
            }
            c => cur.push(c),
        }
    }
    push_addr(&mut out, &cur)?;
    Ok(out)
}

fn push_addr(out: &mut Vec<Address>, raw: &str) -> Result<(), String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(());
    }
    let (name, email) = match (raw.rfind('<'), raw.rfind('>')) {
        (Some(l), Some(r)) if l < r => {
            let name = raw[..l].trim().trim_matches('"').trim();
            (Some(name).filter(|n| !n.is_empty()), raw[l + 1..r].trim())
        }
        _ => (None, raw),
    };
    let valid = email.split_once('@').is_some_and(|(u, d)| !u.is_empty() && d.contains('.') && !d.ends_with('.'))
        && !email.contains(char::is_whitespace);
    if !valid {
        return Err(format!("“{raw}” isn’t a valid email address"));
    }
    out.push(Address::new(name, email));
    Ok(())
}

pub fn format_addresses(list: &[Address]) -> String {
    list.iter().map(format_address).collect::<Vec<_>>().join(", ")
}

/// `Name <email>`, quoting names with separators so `parse_addresses` reads them back
/// ("Lee, Cat" would otherwise split into two invalid addresses).
pub fn format_address(a: &Address) -> String {
    match &a.name {
        Some(n) if n.contains([',', ';', '<', '>', '"']) => format!("\"{}\" <{}>", n.replace('"', ""), a.email),
        Some(n) => format!("{n} <{}>", a.email),
        None => a.email.clone(),
    }
}

/// The document the viewer's WebView loads, plus whether remote content was blocked.
pub struct RenderedBody {
    pub html: String,
    pub blocked_remote: bool,
    /// The WebView colour behind the page, so loading never flashes a different one.
    pub background: &'static str,
    /// `Some` when the message can be shown darkened (dark mode, HTML without dark styles of
    /// its own): whether it is. The viewer's Original Colors button follows it.
    pub darkened: Option<bool>,
    /// Attachments the message's own HTML shows (`cid:` pictures): no chip for those.
    pub placed: Vec<usize>,
}

/// The colours a message is shown in: the theme's (Settings → Appearance) or plain Adwaita's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub dark: bool,
    /// Behind the message (the window colour, so it never flashes another one).
    pub bg: &'static str,
    pub fg: &'static str,
    pub quote: &'static str,
    pub link: &'static str,
}

impl Look {
    pub const LIGHT: Look = Look { dark: false, bg: "#ffffff", fg: "#1c1c1c", quote: "#6b6b6b", link: "#1c71d8" };
    pub const ADWAITA_DARK: Look = Look { dark: true, bg: "#1e1e1e", fg: "#ffffff", quote: "#9a9a9a", link: "#78aeed" };

    pub fn of(p: &'static tern_core::themes::Palette) -> Look {
        Look { dark: p.dark, bg: p.bg, fg: p.text, quote: p.dim, link: p.accent }
    }
}

/// Dark mode, mail with its own dark styles (or shown in its original colours): the page on a
/// rounded sheet inside the dark window.
fn card_css(look: &Look) -> String {
    format!(
        "html{{background:{bg}}}body{{background:#fff;color:#1c1c1c;margin:12px 16px!important;\
         padding:16px 20px;border-radius:12px}}",
        bg = look.bg
    )
}

/// Dark mode, any other HTML mail: three layers fixed over the visible area turn the page
/// dark. The first inverts what's behind it and turns hues back round (so colours keep their
/// hue), the second multiplies by the theme's text colour and the third lifts the darkest
/// colours to its window colour. Pictures are inverted first, so they come out with
/// their own colours (with a slight blue cast). Filtering only the visible area, not the whole
/// page, matters: a filter over a tall email made a layer too big to draw and the page went
/// blank at 4K. Pure CSS: scripts stay off and nothing is fetched.
fn forced_dark_css(look: &Look) -> String {
    format!(
        "html{{background:#fff;color:#1c1c1c}}\
         img,video,picture,svg,[background],[style*=background-image]{{filter:invert(1) hue-rotate(180deg)}}\
         #tern-dark-1,#tern-dark-2,#tern-dark-3{{position:fixed!important;inset:0!important;margin:0!important;\
         padding:0!important;border:0!important;pointer-events:none!important;z-index:2147483647!important}}\
         #tern-dark-1{{backdrop-filter:invert(1) hue-rotate(180deg)}}\
         #tern-dark-2{{background:{fg}!important;mix-blend-mode:multiply}}\
         #tern-dark-3{{background:{bg}!important;mix-blend-mode:lighten}}",
        fg = look.fg,
        bg = look.bg
    )
}
const FORCED_DARK_LAYERS: &str = "<div id=tern-dark-1></div><div id=tern-dark-2></div><div id=tern-dark-3></div>";

/// Whether HTML mail ships its own dark-mode styles (WebKit then applies them itself): a
/// `prefers-color-scheme: dark` rule that reaches something in the message. Marketing mail
/// often fakes it and would stay light:
/// * a `<meta name="color-scheme" content="light dark">` with no dark rules (TikTok);
/// * dark rules only for classes no element uses, template leftovers (Sur La Table).
fn has_own_dark_styles(html: &str) -> bool {
    let l = html.to_ascii_lowercase();
    let (mut markup, mut styles) = (String::new(), String::new());
    let mut rest = l.as_str();
    while let Some(i) = rest.find("<style") {
        markup.push_str(&rest[..i]);
        let style = &rest[i..];
        let css = style.find('>').map_or("", |g| &style[g + 1..]);
        let end = css.find("</style>").unwrap_or(css.len());
        styles.push_str(&css[..end]);
        styles.push('\n');
        rest = css.get(end + 8..).unwrap_or("");
    }
    markup.push_str(rest);
    let classes = attribute_words(&markup, "class=");
    let ids = attribute_words(&markup, "id=");

    let mut selectors = Vec::new();
    for (i, _) in styles.match_indices("prefers-color-scheme") {
        let after = styles[i + "prefers-color-scheme".len()..].trim_start();
        let Some(value) = after.strip_prefix(':') else { continue };
        if !value.trim_start().starts_with("dark") {
            continue;
        }
        if let Some(open) = value.find('{') {
            css_selectors(block_body(&value[open + 1..]), &mut selectors);
        }
    }
    selectors.iter().flat_map(|s| s.split(',')).any(|s| {
        let names = |mark: char| -> Vec<&str> {
            s.split(mark)
                .skip(1)
                .map(|t| t.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).next().unwrap_or(""))
                .filter(|t| !t.is_empty())
                .collect()
        };
        // A selector without classes or ids (body, td, :root) reaches the page; one with
        // them only if the markup uses every one.
        names('.').iter().all(|c| classes.contains(c)) && names('#').iter().all(|d| ids.contains(d))
    })
}

/// The text inside a `{` already opened, up to its matching `}`.
fn block_body(css: &str) -> &str {
    let mut depth = 1;
    for (i, c) in css.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &css[..i];
                }
            }
            _ => {}
        }
    }
    css
}

/// The selectors of the rules in a block of CSS, looking inside nested at-rules.
fn css_selectors<'a>(mut css: &'a str, out: &mut Vec<&'a str>) {
    while let Some(open) = css.find('{') {
        let head = css[..open].rsplit("*/").next().unwrap_or("").trim();
        let body = block_body(&css[open + 1..]);
        if head.starts_with('@') {
            css_selectors(body, out);
        } else if !head.is_empty() {
            out.push(head);
        }
        css = css.get(open + 1 + body.len() + 1..).unwrap_or("");
    }
}

/// Every word in the markup's `name` attributes (`class=` → each class used).
fn attribute_words<'a>(markup: &'a str, name: &str) -> std::collections::HashSet<&'a str> {
    let mut words = std::collections::HashSet::new();
    for (i, _) in markup.match_indices(name) {
        if markup[..i].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '-') {
            continue; // data-class=, subclass=
        }
        let v = &markup[i + name.len()..];
        let value = match v.chars().next() {
            Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or(""),
            _ => v.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or(""),
        };
        words.extend(value.split_whitespace());
    }
    words
}

/// Build a locked-down HTML document for a message body.
///
/// * A Content-Security-Policy blocks every network load unless `allow_remote`
///   (tracking pixels, remote fonts) — Apple Mail's "Load Remote Content".
/// * `cid:` inline images are rewritten to `data:` URIs from the raw message.
/// * Plain text is escaped, link-ified, and quote levels are styled.
/// * Plain text takes the theme's colours (`look`). With a dark look, HTML mail is darkened
///   to them unless it has dark styles of its own or `original_colors` (then it sits on a
///   white sheet); with a light one it shows as sent.
#[cfg(test)]
pub fn render_body(body: &MessageBody, allow_remote: bool, look: &Look, original_colors: bool) -> RenderedBody {
    render_body_with(body, allow_remote, look, original_colors, &std::collections::HashMap::new())
}

/// `render_body`, with pictures the view can't draw given as JPEGs to show instead
/// (attachment index → bytes): HEIC photos, converted by `heic::to_jpeg`.
pub fn render_body_with(
    body: &MessageBody,
    allow_remote: bool,
    look: &Look,
    original_colors: bool,
    converted: &std::collections::HashMap<usize, Vec<u8>>,
) -> RenderedBody {
    let csp = if allow_remote {
        "default-src 'none'; img-src * data:; style-src * 'unsafe-inline'; font-src * data:; media-src *;"
    } else {
        "default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:;"
    };
    let (fg, bg, quote, link, dark) = (look.fg, look.bg, look.quote, look.link, look.dark);
    let base_css = format!(
        "html{{color-scheme:{scheme}}}body{{font:15px/1.5 -apple-system,'Inter','Cantarell',system-ui,sans-serif;\
         margin:16px 24px;word-wrap:break-word;font-feature-settings:'calt' 0;}}img{{max-width:100%;height:auto}}\
         .tern-picture{{margin:12px 0;white-space:normal}}.tern-picture img{{border-radius:6px}}",
        scheme = if dark { "dark" } else { "light" }
    );

    match &body.html {
        Some(html) => {
            let (html, placed) = inline_cid_images(html, body);
            let pictures = loose_pictures(body, &placed, converted);
            let blocked_remote = !allow_remote && has_remote_resources(&html);
            let (page, layers, background, darkened) = if !dark {
                ("body{background:#fff;color:#1c1c1c}".to_string(), "", "#ffffff", None)
            } else if has_own_dark_styles(&html) {
                (card_css(look), "", bg, None)
            } else if original_colors {
                (card_css(look), "", bg, Some(false))
            } else {
                (forced_dark_css(look), FORCED_DARK_LAYERS, bg, Some(true))
            };
            let doc = format!(
                "<!doctype html><html><head><meta charset=utf-8>\
                 <meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">\
                 <style>{base_css} {page}</style></head><body>{layers}{html}{pictures}</body></html>"
            );
            RenderedBody { html: doc, blocked_remote, background, darkened, placed }
        }
        None => {
            let text = body.text.as_deref().unwrap_or("");
            let mut out = String::new();
            for line in text.lines() {
                let depth = line.chars().take_while(|c| *c == '>' || *c == ' ').filter(|c| *c == '>').count();
                let content = if depth > 0 {
                    line.trim_start_matches(['>', ' '])
                } else {
                    line
                };
                let escaped = linkify(&escape(content));
                if depth > 0 {
                    out.push_str(&format!(
                        "<div class=q style=\"border-left:2px solid {quote};padding-left:{}px;color:{quote}\">{escaped}&nbsp;</div>",
                        8 * depth
                    ));
                } else {
                    out.push_str(&format!("<div>{escaped}&nbsp;</div>"));
                }
            }
            let doc = format!(
                "<!doctype html><html><head><meta charset=utf-8>\
                 <meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">\
                 <style>{base_css} body{{background:{bg};color:{fg};white-space:pre-wrap}} a{{color:{link}}}</style>\
                 </head><body>{out}{pictures}</body></html>",
                pictures = loose_pictures(body, &[], converted)
            );
            RenderedBody { html: doc, blocked_remote: false, background: bg, darkened: None, placed: Vec::new() }
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn linkify(escaped: &str) -> String {
    let mut out = String::with_capacity(escaped.len());
    for (i, word) in escaped.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let trimmed = word.trim_end_matches(['.', ',', ')', ';', ':']);
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            let rest = &word[trimmed.len()..];
            out.push_str(&format!("<a href=\"{trimmed}\">{trimmed}</a>{rest}"));
        } else {
            out.push_str(word);
        }
    }
    out
}

fn has_remote_resources(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    ["src=\"http", "src='http", "src=http", "url(http", "url('http", "url(\"http", "background=\"http", "srcset=\"http"]
        .iter()
        .any(|p| lower.contains(p))
}

/// Put `cid:` pictures into the HTML as `data:` URIs; also returns which attachments it placed.
fn inline_cid_images(html: &str, body: &MessageBody) -> (String, Vec<usize>) {
    let mut html = html.to_string();
    let mut placed = Vec::new();
    for (i, att) in body.attachments.iter().enumerate() {
        let Some(cid) = &att.content_id else { continue };
        let cid = cid.trim_matches(['<', '>']);
        let needle = format!("cid:{cid}");
        if !html.contains(&needle) {
            continue;
        }
        if let Some(bytes) = tern_core::mime::attachment_bytes(&body.raw, i as u32) {
            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            html = html.replace(&needle, &format!("data:{};base64,{data}", att.mime_type));
            placed.push(i);
        }
    }
    (html, placed)
}

/// Pictures sent to be shown in the message that the message doesn't place itself: iPhone
/// Mail sends photos as inline parts next to plain text, with no HTML to put them anywhere.
/// Shown below the text, as Apple Mail does (they keep their attachment chips for saving).
/// Types WebKit draws, plus any given in `converted` (HEIC as JPEG); others stay chips.
fn loose_pictures(body: &MessageBody, placed: &[usize], converted: &std::collections::HashMap<usize, Vec<u8>>) -> String {
    const SHOWN: [&str; 5] = ["image/jpeg", "image/png", "image/gif", "image/webp", "image/bmp"];
    let mut out = String::new();
    for (i, att) in body.attachments.iter().enumerate() {
        if !att.inline || placed.contains(&i) {
            continue;
        }
        let (mime, bytes) = match converted.get(&i) {
            Some(jpeg) => ("image/jpeg".to_string(), Some(jpeg.clone())),
            None => {
                let mime = att.mime_type.to_ascii_lowercase();
                if !SHOWN.contains(&mime.as_str()) {
                    continue;
                }
                let bytes = tern_core::mime::attachment_bytes(&body.raw, i as u32);
                (mime, bytes)
            }
        };
        if let Some(bytes) = bytes {
            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            let alt = escape(att.filename.as_deref().unwrap_or("picture"));
            out.push_str(&format!("<p class=tern-picture><img src=\"data:{mime};base64,{data}\" alt=\"{alt}\"></p>"));
        }
    }
    out
}

/// Guess a MIME type for an outgoing attachment from its file name.
pub fn mime_for(filename: &str) -> &'static str {
    let ext = filename.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "txt" | "md" | "log" | "rs" | "toml" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "zip" => "application/zip",
        "json" => "application/json",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "ics" => "text/calendar",
        _ => "application/octet-stream",
    }
}

/// URIs from a `text/uri-list` (RFC 2483): one per line, `#` lines are comments, CRLF or LF.
pub fn parse_uri_list(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

/// Many servers (Gmail included) reject messages larger than this.
pub const LARGE_ATTACHMENTS: u64 = 25 * 1024 * 1024;

/// True when adding attachments takes a size from at or under `limit` to over it, so the
/// warning shows once rather than on every file.
pub fn crosses_limit(before: u64, after: u64, limit: u64) -> bool {
    before <= limit && after > limit
}

/// Roughly how big a message with this much text and attachment data is once encoded:
/// base64 makes attachments 4/3 larger plus a line break every 76 characters, and headers
/// and MIME boundaries add a little. Only for warnings; the Send check measures exactly.
pub fn estimated_message_size(text_bytes: u64, attachment_bytes: u64) -> u64 {
    text_bytes + attachment_bytes * 4 / 3 * 78 / 76 + 4096
}

pub fn human_size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} bytes"),
        b if b < 1024 * 1024 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}

#[allow(dead_code)]
pub fn date_from(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// `items` with `moved` taken out and put just before `target` (or just after it).
/// Unchanged when either is missing or they're the same item.
pub fn move_next_to<T: PartialEq + Clone>(items: &[T], moved: &T, target: &T, after: bool) -> Vec<T> {
    if moved == target || !items.contains(moved) || !items.contains(target) {
        return items.to_vec();
    }
    let mut out: Vec<T> = items.iter().filter(|i| *i != moved).cloned().collect();
    let at = out.iter().position(|i| i == target).unwrap_or(out.len());
    out.insert(if after { at + 1 } else { at }, moved.clone());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokyo() -> Look {
        Look::of(tern_core::themes::find("tokyo-night").unwrap())
    }

    #[test]
    fn messages_take_the_themes_colors() {
        let nord = Look::of(tern_core::themes::find("nord").unwrap());
        let html = body(None, Some("<p>Sale</p>"));
        let r = render_body(&html, false, &nord, false);
        assert_eq!(r.background, "#2e3440");
        assert!(r.html.contains("background:#eceff4!important") && r.html.contains("background:#2e3440!important"), "darkened to Nord");
        assert!(!r.html.contains("#1a1b26") && !r.html.contains("#c0caf5"), "no Tokyo Night left");
        let plain = MessageBody { text: Some("hi".into()), ..body(None, None) };
        let r = render_body(&plain, false, &nord, false);
        assert!(r.html.contains("background:#2e3440;color:#eceff4"));
        let latte = Look::of(tern_core::themes::find("catppuccin-latte").unwrap());
        let r = render_body(&html, false, &latte, false);
        assert_eq!((r.darkened, r.background), (None, "#ffffff"), "light themes show HTML as sent");
        assert!(render_body(&plain, false, &latte, false).html.contains("background:#eff1f5;color:#4c4f69"));
    }
    use chrono::{Duration, TimeZone};

    #[test]
    fn list_dates() {
        let now = Local.with_ymd_and_hms(2026, 10, 14, 15, 0, 0).unwrap();
        let at = |d: Duration| (now - d).with_timezone(&Utc);
        assert!(list_date(at(Duration::hours(2)), now).ends_with("PM"));
        assert_eq!(list_date(at(Duration::days(1)), now), "Yesterday");
        assert_eq!(list_date(at(Duration::days(3)), now), "Sunday");
        assert_eq!(list_date(at(Duration::days(30)), now), "Sep 14");
        assert_eq!(list_date(at(Duration::days(400)), now), "9/9/25");
    }

    #[test]
    fn uri_lists() {
        assert_eq!(
            parse_uri_list("# from dolphin\r\nfile:///a/b.txt\r\nfile:///c%20d.pdf\r\n\r\n"),
            ["file:///a/b.txt", "file:///c%20d.pdf"]
        );
        assert!(parse_uri_list("").is_empty());
    }

    #[test]
    fn size_warning_crosses_once() {
        let mb = 1024 * 1024;
        let l = LARGE_ATTACHMENTS;
        assert!(!crosses_limit(0, 10 * mb, l));
        assert!(crosses_limit(20 * mb, 26 * mb, l));
        assert!(!crosses_limit(26 * mb, 30 * mb, l), "already over: don't warn again");
        assert!(!crosses_limit(0, l, l), "exactly at the limit is fine");
    }

    #[test]
    fn encoded_size_estimate_is_close() {
        // 20 MB of attachments: iCloud (28,319,744) takes it, Gmail's 25 MB rule of thumb holds.
        let est = estimated_message_size(1000, 20 * 1024 * 1024);
        assert!(est > 28_000_000 && est < 29_000_000, "{est}");
        assert!(estimated_message_size(1000, 25 * 1024 * 1024) < 35_882_577, "Gmail's advertised SIZE");
    }

    #[test]
    fn address_lists() {
        let a = parse_addresses(r#"Ann <ann@x.com>, bob@x.com; "Lee, Cat" <cat@x.com>,"#).unwrap();
        assert_eq!(a.len(), 3);
        assert_eq!(a[2].name.as_deref(), Some("Lee, Cat"));
        assert_eq!(a[1].email, "bob@x.com");
        assert!(parse_addresses("not an address").is_err());
        assert!(parse_addresses("  ").unwrap().is_empty());
        assert_eq!(format_addresses(&a[..2]), "Ann <ann@x.com>, bob@x.com");
        // Names with separators round-trip.
        assert_eq!(parse_addresses(&format_addresses(&a)).unwrap(), a);
        assert_eq!(format_addresses(&a[2..]), r#""Lee, Cat" <cat@x.com>"#);
    }

    fn body(text: Option<&str>, html: Option<&str>) -> MessageBody {
        MessageBody {
            text: text.map(Into::into),
            html: html.map(Into::into),
            attachments: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: false,
            raw: vec![],
        }
    }

    #[test]
    fn iphone_inline_photos_show_below_the_text() {
        // iPhone Mail (2026-11-10): photos as inline parts with no Content-ID, then plain text.
        let raw = "From: t@me.com\r\nSubject: Pics\r\nMime-Version: 1.0\r\n\
            Content-Type: multipart/mixed; boundary=B\r\n\r\n\
            --B\r\nContent-Type: image/jpeg; name=image0.jpeg\r\nContent-Disposition: inline; filename=image0.jpeg\r\n\
            Content-Transfer-Encoding: base64\r\n\r\nSlBFRzA=\r\n\
            --B\r\nContent-Type: image/jpeg; name=image1.jpeg\r\nContent-Disposition: inline; filename=image1.jpeg\r\n\
            Content-Transfer-Encoding: base64\r\n\r\nSlBFRzE=\r\n\
            --B\r\nContent-Type: text/plain; charset=us-ascii\r\n\r\nLook at these\r\n--B--\r\n";
        let b = tern_core::mime::body_from_raw(raw.as_bytes().to_vec());
        assert_eq!(b.attachments.len(), 2);
        let r = render_body(&b, false, &Look::LIGHT, false);
        assert!(r.html.contains("Look at these"));
        assert_eq!(r.html.matches("<p class=tern-picture><img src=\"data:image/jpeg;base64,").count(), 2, "both photos shown");
        assert!(r.html.contains("SlBFRzA=") && r.html.contains("SlBFRzE="), "their own bytes");
        assert!(r.html.find("Look at these") < r.html.find("<p class=tern-picture"), "below the text");
        assert!(r.placed.is_empty(), "and still offered as chips");

        // HTML that places one picture itself: that one isn't repeated, the loose one is shown.
        let raw = "From: t@me.com\r\nMime-Version: 1.0\r\nContent-Type: multipart/related; boundary=R\r\n\r\n\
            --R\r\nContent-Type: text/html\r\n\r\n<p>Hi <img src=\"cid:logo@x\"></p>\r\n\
            --R\r\nContent-Type: image/png\r\nContent-ID: <logo@x>\r\nContent-Disposition: inline\r\n\
            Content-Transfer-Encoding: base64\r\n\r\nTE9HTw==\r\n\
            --R\r\nContent-Type: image/png\r\nContent-Disposition: inline; filename=extra.png\r\n\
            Content-Transfer-Encoding: base64\r\n\r\nRVhUUkE=\r\n--R--\r\n";
        let b = tern_core::mime::body_from_raw(raw.as_bytes().to_vec());
        let r = render_body(&b, false, &Look::LIGHT, false);
        assert_eq!(r.html.matches("TE9HTw==").count(), 1, "placed logo appears once");
        assert_eq!(r.html.matches("tern-picture><img").count(), 1, "only the loose one is added");
        assert!(r.html.contains("RVhUUkE="));
        assert_eq!(r.placed.len(), 1, "no chip for the logo the HTML shows");
    }

    #[test]
    fn heic_photos_show_once_converted() {
        let raw = "From: t@me.com\r\nMime-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=B\r\n\r\n\
            --B\r\nContent-Type: image/heic; name=IMG_1.HEIC\r\nContent-Disposition: inline; filename=IMG_1.HEIC\r\n\
            Content-Transfer-Encoding: base64\r\n\r\nSEVJQw==\r\n\
            --B\r\nContent-Type: text/plain\r\n\r\nFrom my phone\r\n--B--\r\n";
        let b = tern_core::mime::body_from_raw(raw.as_bytes().to_vec());
        let r = render_body(&b, false, &Look::LIGHT, false);
        assert!(!r.html.contains("<p class=tern-picture"), "can't be drawn yet: chip only");
        assert!(r.placed.is_empty());
        let converted = std::collections::HashMap::from([(0usize, b"JPEGBYTES".to_vec())]);
        let r = render_body_with(&b, false, &Look::LIGHT, false, &converted);
        assert!(r.html.contains("<img src=\"data:image/jpeg;base64,SlBFR0JZVEVT\""), "shown as the JPEG");
        assert!(r.placed.is_empty(), "the chip still saves the original HEIC");
    }

    #[test]
    fn remote_content_blocked_by_default() {
        let b = body(None, Some(r#"<p>Hi</p><img src="https://t.example/p.gif">"#));
        let r = render_body(&b, false, &Look::LIGHT, false);
        assert!(r.blocked_remote);
        assert!(r.html.contains("img-src data:;"));
        let r = render_body(&b, true, &Look::LIGHT, false);
        assert!(!r.blocked_remote);
        assert!(r.html.contains("img-src * data:"));
    }

    #[test]
    fn plain_text_is_escaped_linked_and_quoted() {
        let b = body(Some("See https://x.com/a.\n<script>\n> quoted\n>> deeper"), None);
        let r = render_body(&b, false, &tokyo(), false);
        assert!(r.html.contains("<a href=\"https://x.com/a\">https://x.com/a</a>."));
        assert!(r.html.contains("&lt;script&gt;"));
        assert!(!r.html.contains("<script>"));
        assert!(r.html.contains("padding-left:8px") && r.html.contains("padding-left:16px"));
    }

    #[test]
    fn dark_mode_darkens_html_unless_it_has_its_own_dark_styles() {
        let plain = body(None, Some("<p style=\"color:#333\">Sale</p>"));
        let r = render_body(&plain, false, &tokyo(), false);
        assert_eq!(r.darkened, Some(true));
        assert!(r.html.contains("tern-dark-1") && r.background == "#1a1b26");
        let r = render_body(&plain, false, &tokyo(), true);
        assert_eq!(r.darkened, Some(false), "original colours");
        assert!(!r.html.contains("tern-dark") && r.html.contains("border-radius:12px"));
        let r = render_body(&plain, false, &Look::LIGHT, false);
        assert_eq!(r.darkened, None, "light mode");
        assert!(!r.html.contains("tern-dark") && r.background == "#ffffff");

        for own in [
            "<style>@media (prefers-color-scheme: dark){body{background:#000}}</style>",
            "<meta name=\"color-scheme\" content=\"light dark\"><style>@media (prefers-color-scheme:dark){p{color:#fff}}</style>",
            "<style>@media (prefers-color-scheme: dark) { /* dm */ .dm-bg { background:#111 !important } }</style>\
             <table class=\"wrap dm-bg\"><tr><td>Hi</td></tr></table>",
            "<style>@media screen and (prefers-color-scheme: dark){@supports (color:red){#main{color:#fff}}}</style>\
             <div id='main'>Hi</div>",
        ] {
            let r = render_body(&body(None, Some(own)), false, &tokyo(), false);
            assert_eq!(r.darkened, None, "{own}");
            assert!(!r.html.contains("tern-dark"), "{own}");
        }
        // Claims dark support but has no dark rules (TikTok, 2026-11-07): darken it ourselves.
        for not_own in [
            "<meta content=\"light dark\" name=\"color-scheme\"><meta content=\"light dark\" name=\"supported-color-schemes\">\
             <body style=\"background-color:#F8F8F8\"><p>Hi</p>",
            "<meta name=\"color-scheme\" content=\"light\"><p>dark chocolate</p>",
            "<style>@media (prefers-color-scheme: light){body{background:#fff}}</style>",
            // Dark rules for classes nothing uses (Sur La Table, 2026-11-07).
            "<style>@media (prefers-color-scheme: dark) { .dm-bg { background-color: #1a1a1a !important; }\
             .dm-text { color: #f0f0f0 !important; } }</style>\
             <body class=\"body\" style=\"background-color:#fff\"><p data-class=\"dm-text\">Sale</p>",
            "<picture><source media=\"(prefers-color-scheme: dark)\" srcset=\"x.png\"></picture>\
             <style>body{background:#fff}</style>",
        ] {
            assert_eq!(render_body(&body(None, Some(not_own)), false, &tokyo(), false).darkened, Some(true), "{not_own}");
        }
    }

    #[test]
    fn move_next_to_reorders_around_the_target() {
        let v = ["gmail", "demo", "icloud"];
        assert_eq!(move_next_to(&v, &"icloud", &"gmail", false), ["icloud", "gmail", "demo"], "above");
        assert_eq!(move_next_to(&v, &"gmail", &"icloud", true), ["demo", "icloud", "gmail"], "below");
        assert_eq!(move_next_to(&v, &"gmail", &"demo", false), ["gmail", "demo", "icloud"], "already there");
        assert_eq!(move_next_to(&v, &"gmail", &"gmail", true), v, "onto itself");
        assert_eq!(move_next_to(&v, &"work", &"gmail", false), v, "unknown item");
    }
}

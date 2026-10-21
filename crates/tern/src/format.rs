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
}

/// Build a locked-down HTML document for a message body.
///
/// * A Content-Security-Policy blocks every network load unless `allow_remote`
///   (tracking pixels, remote fonts) — Apple Mail's "Load Remote Content".
/// * `cid:` inline images are rewritten to `data:` URIs from the raw message.
/// * Plain text is escaped, link-ified, and quote levels are styled.
pub fn render_body(body: &MessageBody, allow_remote: bool, dark: bool) -> RenderedBody {
    let csp = if allow_remote {
        "default-src 'none'; img-src * data:; style-src * 'unsafe-inline'; font-src * data:; media-src *;"
    } else {
        "default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:;"
    };
    let (fg, bg, quote, link) = if dark {
        ("#e8e8e8", "#1e1e1e", "#9a9a9a", "#78aeed")
    } else {
        ("#1c1c1c", "#ffffff", "#6b6b6b", "#1c71d8")
    };
    let base_css = format!(
        "html{{color-scheme:{scheme}}}body{{font:15px/1.5 -apple-system,'Inter','Cantarell',system-ui,sans-serif;\
         margin:16px 24px;word-wrap:break-word;font-feature-settings:'calt' 0;}}img{{max-width:100%;height:auto}}",
        scheme = if dark { "dark" } else { "light" }
    );

    match &body.html {
        Some(html) => {
            let html = inline_cid_images(html, body);
            let blocked_remote = !allow_remote && has_remote_resources(&html);
            // HTML mail is designed for a white page; keep it white even in dark mode
            // (Apple Mail does the same unless the message opts into dark styles).
            let doc = format!(
                "<!doctype html><html><head><meta charset=utf-8>\
                 <meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">\
                 <style>{base_css} body{{background:#fff;color:#1c1c1c}}</style></head><body>{html}</body></html>"
            );
            RenderedBody { html: doc, blocked_remote }
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
                 </head><body>{out}</body></html>"
            );
            RenderedBody { html: doc, blocked_remote: false }
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

fn inline_cid_images(html: &str, body: &MessageBody) -> String {
    let mut html = html.to_string();
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
        }
    }
    html
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

/// Many servers (Gmail included) reject messages larger than this.
pub const LARGE_ATTACHMENTS: u64 = 25 * 1024 * 1024;

/// True when adding attachments takes the total from at or under the limit to over it,
/// so the warning shows once rather than on every file.
pub fn crosses_size_limit(before: u64, after: u64) -> bool {
    before <= LARGE_ATTACHMENTS && after > LARGE_ATTACHMENTS
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

#[cfg(test)]
mod tests {
    use super::*;
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
    fn size_warning_crosses_once() {
        let mb = 1024 * 1024;
        assert!(!crosses_size_limit(0, 10 * mb));
        assert!(crosses_size_limit(20 * mb, 26 * mb));
        assert!(!crosses_size_limit(26 * mb, 30 * mb), "already over: don't warn again");
        assert!(!crosses_size_limit(0, LARGE_ATTACHMENTS), "exactly at the limit is fine");
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
    fn remote_content_blocked_by_default() {
        let b = body(None, Some(r#"<p>Hi</p><img src="https://t.example/p.gif">"#));
        let r = render_body(&b, false, false);
        assert!(r.blocked_remote);
        assert!(r.html.contains("img-src data:;"));
        let r = render_body(&b, true, false);
        assert!(!r.blocked_remote);
        assert!(r.html.contains("img-src * data:"));
    }

    #[test]
    fn plain_text_is_escaped_linked_and_quoted() {
        let b = body(Some("See https://x.com/a.\n<script>\n> quoted\n>> deeper"), None);
        let r = render_body(&b, false, true);
        assert!(r.html.contains("<a href=\"https://x.com/a\">https://x.com/a</a>."));
        assert!(r.html.contains("&lt;script&gt;"));
        assert!(!r.html.contains("<script>"));
        assert!(r.html.contains("padding-left:8px") && r.html.contains("padding-left:16px"));
    }
}

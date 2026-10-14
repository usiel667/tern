//! Search-field query parsing.
//!
//! Apple Mail turns typed text into "tokens" (From: Ann, Subject: invoice,
//! Flagged). We accept the same ideas as text so the search bar and the
//! (future) token UI share one representation:
//!
//! `from:ann subject:"q3 invoice" is:unread is:flagged has:attachment after:2026-01-01 budget`

use chrono::NaiveDate;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// Free text, matched against everything.
    pub text: Vec<String>,
    pub from: Vec<String>,
    pub to: Vec<String>,
    pub subject: Vec<String>,
    pub unread: Option<bool>,
    pub flagged: Option<bool>,
    pub has_attachment: Option<bool>,
    pub after: Option<NaiveDate>,
    pub before: Option<NaiveDate>,
}

impl SearchQuery {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn parse(input: &str) -> Self {
        let mut q = SearchQuery::default();
        for tok in tokenize(input) {
            let (key, val) = match tok.split_once(':') {
                Some((k, v)) if !v.is_empty() && !k.contains(' ') => (k.to_ascii_lowercase(), v.to_string()),
                _ => {
                    q.text.push(tok);
                    continue;
                }
            };
            match key.as_str() {
                "from" => q.from.push(val),
                "to" | "cc" => q.to.push(val),
                "subject" => q.subject.push(val),
                "is" => match val.to_ascii_lowercase().as_str() {
                    "unread" => q.unread = Some(true),
                    "read" => q.unread = Some(false),
                    "flagged" => q.flagged = Some(true),
                    "unflagged" => q.flagged = Some(false),
                    _ => q.text.push(tok),
                },
                "has" if val.eq_ignore_ascii_case("attachment") => q.has_attachment = Some(true),
                "after" => match NaiveDate::parse_from_str(&val, "%Y-%m-%d") {
                    Ok(d) => q.after = Some(d),
                    Err(_) => q.text.push(tok),
                },
                "before" => match NaiveDate::parse_from_str(&val, "%Y-%m-%d") {
                    Ok(d) => q.before = Some(d),
                    Err(_) => q.text.push(tok),
                },
                _ => q.text.push(tok),
            }
        }
        q
    }
}

/// Split on whitespace, keeping `"quoted phrases"` (also after `key:`) together.
fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in input.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tokens() {
        let q = SearchQuery::parse(r#"from:ann subject:"q3 invoice" is:unread has:attachment after:2026-01-02 budget"#);
        assert_eq!(q.from, vec!["ann"]);
        assert_eq!(q.subject, vec!["q3 invoice"]);
        assert_eq!(q.unread, Some(true));
        assert_eq!(q.has_attachment, Some(true));
        assert_eq!(q.after, NaiveDate::from_ymd_opt(2026, 1, 2));
        assert_eq!(q.text, vec!["budget"]);
    }

    #[test]
    fn unknown_keys_are_text() {
        let q = SearchQuery::parse("http://x 10:30");
        assert_eq!(q.text, vec!["http://x", "10:30"]);
    }
}

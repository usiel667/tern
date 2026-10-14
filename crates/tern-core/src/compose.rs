//! Reply / Reply All / Forward draft construction, matching Apple Mail's output.

use chrono::Local;

use crate::model::{Address, Draft, Envelope, MessageBody};
use crate::threading::base_subject;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyKind {
    Reply,
    ReplyAll,
    Forward,
}

/// Build a draft responding to `orig`. `me` is every address of the sending
/// account (so Reply All never includes yourself).
pub fn respond(
    kind: ReplyKind,
    orig: &Envelope,
    body: Option<&MessageBody>,
    from: &Address,
    me: &[String],
) -> Draft {
    let is_me = |a: &Address| me.iter().any(|m| m.eq_ignore_ascii_case(&a.email));
    let mut draft = Draft {
        from: Some(from.clone()),
        ..Default::default()
    };

    let (base, _) = base_subject(&orig.subject);
    let orig_trimmed = strip_prefixes(&orig.subject);
    match kind {
        ReplyKind::Forward => {
            draft.subject = format!("Fwd: {orig_trimmed}");
        }
        ReplyKind::Reply | ReplyKind::ReplyAll => {
            draft.subject = if base.is_empty() {
                "Re:".into()
            } else {
                format!("Re: {orig_trimmed}")
            };
            // If I sent the original, replying goes to its recipients (Apple Mail behaviour).
            let sent_by_me = orig.from.iter().any(is_me);
            draft.to = if sent_by_me { orig.to.clone() } else { orig.from.clone() };
            if kind == ReplyKind::ReplyAll {
                let mut seen: Vec<String> = draft.to.iter().map(|a| a.email.to_lowercase()).collect();
                for a in orig.to.iter().chain(&orig.cc) {
                    let e = a.email.to_lowercase();
                    if !is_me(a) && !seen.contains(&e) {
                        seen.push(e);
                        draft.cc.push(a.clone());
                    }
                }
            }
            draft.in_reply_to = orig.message_id.clone();
            draft.references = orig.references.clone();
            if let Some(id) = &orig.message_id
                && !draft.references.contains(id) {
                    draft.references.push(id.clone());
                }
        }
    }

    let orig_text = body
        .and_then(|b| b.text.clone())
        .or_else(|| orig.preview.clone())
        .unwrap_or_default();
    let sender = orig.from.first().map(|a| a.to_string()).unwrap_or_default();
    let date = orig.date.with_timezone(&Local).format("%b %-d, %Y at %-I:%M %p");

    draft.text = match kind {
        ReplyKind::Forward => {
            let to = orig.to.iter().map(Address::to_string).collect::<Vec<_>>().join(", ");
            format!(
                "\n\nBegin forwarded message:\n\nFrom: {sender}\nSubject: {}\nDate: {date}\nTo: {to}\n\n{orig_text}",
                orig.subject
            )
        }
        _ => {
            let quoted = orig_text
                .lines()
                .map(|l| if l.starts_with('>') { format!(">{l}") } else { format!("> {l}") })
                .collect::<Vec<_>>()
                .join("\n");
            format!("\n\n> On {date}, {sender} wrote:\n> \n{quoted}\n")
        }
    };
    draft
}

fn strip_prefixes(subject: &str) -> String {
    // Keep the original casing; just drop leading Re:/Fwd: chains.
    let mut s = subject.trim();
    loop {
        let lower = s.to_ascii_lowercase();
        let cut = ["re:", "fwd:", "fw:"].iter().find(|p| lower.starts_with(*p));
        match cut {
            Some(p) => s = s[p.len()..].trim_start(),
            None => break,
        }
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Flags;
    use chrono::Utc;

    fn orig() -> Envelope {
        Envelope {
            remote_id: "1".into(),
            message_id: Some("<b@x>".into()),
            in_reply_to: Some("<a@x>".into()),
            references: vec!["<a@x>".into()],
            subject: "Re: Lunch".into(),
            from: vec![Address::new(Some("Ann"), "ann@x.com")],
            to: vec![Address::new(Some("Tom"), "tom@x.com"), Address::new(None, "bob@x.com")],
            cc: vec![Address::new(None, "ANN@x.com"), Address::new(None, "cat@x.com")],
            date: Utc::now(),
            flags: Flags::default(),
            size: 0,
            has_attachments: false,
            preview: Some("Noon?\n> older".into()),
        }
    }

    fn me() -> (Address, Vec<String>) {
        (Address::new(Some("Tom"), "tom@x.com"), vec!["tom@x.com".into()])
    }

    #[test]
    fn reply_all_excludes_me_and_dupes() {
        let (from, mine) = me();
        let d = respond(ReplyKind::ReplyAll, &orig(), None, &from, &mine);
        assert_eq!(d.subject, "Re: Lunch");
        assert_eq!(d.to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["ann@x.com"]);
        assert_eq!(d.cc.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["bob@x.com", "cat@x.com"]);
        assert_eq!(d.in_reply_to.as_deref(), Some("<b@x>"));
        assert_eq!(d.references, vec!["<a@x>", "<b@x>"]);
        assert!(d.text.contains("> Noon?") && d.text.contains(">> older"));
    }

    #[test]
    fn reply_to_own_message_goes_to_recipients() {
        let (from, mine) = me();
        let mut o = orig();
        o.from = vec![from.clone()];
        let d = respond(ReplyKind::Reply, &o, None, &from, &mine);
        assert_eq!(d.to.len(), 2);
    }

    #[test]
    fn forward_has_no_threading_headers() {
        let (from, mine) = me();
        let d = respond(ReplyKind::Forward, &orig(), None, &from, &mine);
        assert_eq!(d.subject, "Fwd: Lunch");
        assert!(d.to.is_empty() && d.in_reply_to.is_none());
        assert!(d.text.contains("Begin forwarded message:"));
    }
}

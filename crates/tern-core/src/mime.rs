//! RFC 5322 / MIME parsing into the Tern model, shared by every backend.

use chrono::{TimeZone, Utc};
use mail_parser::{HeaderValue, MessageParser, MimeHeaders};

use crate::model::{Address, Attachment, Draft, Envelope, Flags, MessageBody, OutgoingAttachment};

const PREVIEW_CHARS: usize = 200;

/// Build an [`Envelope`] from raw header bytes plus (optionally) the first few KB
/// of the body, which is enough for the list preview.
pub fn envelope_from_parts(
    remote_id: String,
    header: &[u8],
    body_prefix: Option<&[u8]>,
    flags: Flags,
    size: u32,
    fallback_date: Option<i64>,
) -> Envelope {
    let mut raw = header.to_vec();
    if let Some(b) = body_prefix {
        if !raw.ends_with(b"\r\n\r\n") && !raw.ends_with(b"\n\n") {
            raw.extend_from_slice(b"\r\n");
        }
        raw.extend_from_slice(b);
    }
    let parser = MessageParser::default();
    let Some(msg) = parser.parse(&raw) else {
        return Envelope {
            remote_id,
            message_id: None,
            in_reply_to: None,
            references: vec![],
            subject: String::new(),
            from: vec![],
            to: vec![],
            cc: vec![],
            date: ts(fallback_date.unwrap_or(0)),
            flags,
            size,
            has_attachments: false,
            preview: None,
        };
    };

    let content_type = msg.header_raw("Content-Type").unwrap_or_default().to_ascii_lowercase();
    Envelope {
        remote_id,
        message_id: msg.message_id().map(|s| format!("<{s}>")),
        in_reply_to: id_list(msg.in_reply_to()).into_iter().next(),
        references: id_list(msg.references()),
        subject: msg.subject().unwrap_or_default().trim().to_owned(),
        from: addrs(msg.from()),
        to: addrs(msg.to()),
        cc: addrs(msg.cc()),
        date: msg
            .date()
            .map(|d| ts(d.to_timestamp()))
            .or(fallback_date.map(ts))
            .unwrap_or_default(),
        flags,
        size,
        has_attachments: content_type.contains("multipart/mixed"),
        preview: body_prefix.and_then(|_| preview_of(&msg)),
    }
}

/// Fully parse a downloaded message.
pub fn body_from_raw(raw: Vec<u8>) -> MessageBody {
    let parser = MessageParser::default();
    let Some(msg) = parser.parse(&raw) else {
        return MessageBody {
            text: Some(String::from_utf8_lossy(&raw).into_owned()),
            html: None,
            attachments: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: false,
            raw,
        };
    };
    let text = msg.body_text(0).map(|c| c.into_owned());
    // Only treat as HTML if there's a real text/html part (mail-parser synthesises
    // HTML from text otherwise).
    let html = msg
        .html_bodies()
        .next()
        .filter(|p| p.is_content_type("text", "html"))
        .and_then(|_| msg.body_html(0))
        .map(|c| c.into_owned());

    let attachments = msg
        .attachments()
        .enumerate()
        .map(|(i, part)| {
            let ct = part
                .content_type()
                .map(|c| match c.subtype() {
                    Some(s) => format!("{}/{}", c.ctype(), s),
                    None => c.ctype().to_string(),
                })
                .unwrap_or_else(|| "application/octet-stream".into());
            let inline = part
                .content_disposition()
                .is_some_and(|d| d.ctype().eq_ignore_ascii_case("inline"))
                || (part.content_id().is_some() && part.attachment_name().is_none());
            Attachment {
                filename: part.attachment_name().map(str::to_owned),
                mime_type: ct,
                size: part.len() as u64,
                content_id: part.content_id().map(str::to_owned),
                inline,
                part_index: i as u32,
            }
        })
        .collect();

    let list_unsubscribe = match msg.list_unsubscribe() {
        HeaderValue::Address(a) => a.first().and_then(|x| x.address.as_deref()).map(|s| s.to_owned()),
        HeaderValue::Text(t) => Some(t.to_string()),
        HeaderValue::TextList(l) => l.first().map(|s| s.to_string()),
        _ => msg.header_raw("List-Unsubscribe").map(|s| s.trim().to_owned()),
    };
    let list_unsubscribe_post = msg
        .header_raw("List-Unsubscribe-Post")
        .is_some_and(|v| v.to_ascii_lowercase().contains("one-click"));

    MessageBody {
        text,
        html,
        attachments,
        list_unsubscribe,
        list_unsubscribe_post,
        raw: raw.clone(),
    }
}

/// Bytes of attachment `index` (as numbered in [`MessageBody::attachments`]).
/// Rebuild an editable [`Draft`] from a message (a refused send or a saved draft).
/// `envelope_rcpts` is the SMTP envelope's recipient list, if any: whoever is in it but not in
/// To/Cc was Bcc, which sent messages don't carry in their headers (drafts do).
pub fn draft_from_raw(raw: &[u8], envelope_rcpts: &[String]) -> Option<Draft> {
    let msg = MessageParser::default().parse(raw)?;
    let to = addrs(msg.to());
    let cc = addrs(msg.cc());
    // Drafts keep a Bcc header; sent messages only have Bcc in the envelope.
    let mut bcc = addrs(msg.bcc());
    let mut known: Vec<String> = to.iter().chain(&cc).chain(&bcc).map(|a| a.email.to_ascii_lowercase()).collect();
    for r in envelope_rcpts {
        if !known.contains(&r.to_ascii_lowercase()) {
            known.push(r.to_ascii_lowercase());
            bcc.push(Address::new(None, r));
        }
    }
    let attachments = msg
        .attachments()
        .filter(|a| a.content_disposition().is_none_or(|d| !d.is_inline()) || a.attachment_name().is_some())
        .map(|a| OutgoingAttachment {
            filename: a.attachment_name().unwrap_or("attachment").to_owned(),
            mime_type: a
                .content_type()
                .map(|ct| match ct.subtype() {
                    Some(sub) => format!("{}/{sub}", ct.ctype()),
                    None => ct.ctype().to_owned(),
                })
                .unwrap_or_else(|| "application/octet-stream".into()),
            data: a.contents().to_vec(),
        })
        .collect();
    Some(Draft {
        from: addrs(msg.from()).into_iter().next(),
        to,
        cc,
        bcc,
        subject: msg.subject().unwrap_or_default().to_owned(),
        text: msg.body_text(0).map(|t| t.replace("\r\n", "\n").trim_end().to_owned()).unwrap_or_default(),
        html: None,
        in_reply_to: id_list(msg.in_reply_to()).into_iter().next(),
        references: id_list(msg.references()),
        attachments,
    })
}

pub fn attachment_bytes(raw: &[u8], index: u32) -> Option<Vec<u8>> {
    let parser = MessageParser::default();
    let msg = parser.parse(raw)?;
    msg.attachments().nth(index as usize).map(|p| p.contents().to_vec())
}

fn preview_of(msg: &mail_parser::Message<'_>) -> Option<String> {
    let text = msg.body_text(0)?;
    let mut out = String::with_capacity(PREVIEW_CHARS);
    let mut last_space = true;
    for line in text.lines() {
        let l = line.trim();
        // Skip quoted text and the "On ..., X wrote:" attribution line, like Apple Mail.
        if l.starts_with('>') || (l.starts_with("On ") && l.ends_with("wrote:")) {
            continue;
        }
        for c in l.chars().chain(std::iter::once(' ')) {
            let space = c.is_whitespace();
            if space && last_space {
                continue;
            }
            out.push(if space { ' ' } else { c });
            last_space = space;
            if out.chars().count() >= PREVIEW_CHARS {
                return Some(out.trim().to_owned());
            }
        }
    }
    let out = out.trim().to_owned();
    (!out.is_empty()).then_some(out)
}

fn addrs(a: Option<&mail_parser::Address<'_>>) -> Vec<Address> {
    a.map(|a| {
        a.iter()
            .filter_map(|x| {
                x.address
                    .as_deref()
                    .map(|email| Address::new(x.name.as_deref(), email))
            })
            .collect()
    })
    .unwrap_or_default()
}

fn id_list(v: &HeaderValue<'_>) -> Vec<String> {
    match v {
        HeaderValue::Text(t) => vec![format!("<{t}>")],
        HeaderValue::TextList(l) => l.iter().map(|t| format!("<{t}>")).collect(),
        _ => vec![],
    }
}

fn ts(secs: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &[u8] = b"From: =?UTF-8?Q?Ren=C3=A9e?= <renee@example.com>\r\n\
To: Tom <tom@example.com>, bob@example.com\r\n\
Subject: =?UTF-8?B?UmU6IENhZsOp?=\r\n\
Date: Wed, 14 Oct 2026 10:00:00 -0600\r\n\
Message-ID: <c@x>\r\n\
In-Reply-To: <b@x>\r\n\
References: <a@x> <b@x>\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\n";

    #[test]
    fn envelope_decodes_headers_and_preview() {
        let body = b"Sounds   good,\r\nsee you there.\r\n\r\nOn Tue, Bob wrote:\r\n> earlier\r\n";
        let e = envelope_from_parts("42".into(), HEADER, Some(body), Flags::default(), 10, None);
        assert_eq!(e.subject, "Re: Café");
        assert_eq!(e.from[0].display(), "Renée");
        assert_eq!(e.to.len(), 2);
        assert_eq!(e.message_id.as_deref(), Some("<c@x>"));
        assert_eq!(e.in_reply_to.as_deref(), Some("<b@x>"));
        assert_eq!(e.references, vec!["<a@x>", "<b@x>"]);
        assert_eq!(e.date.timestamp(), 1791993600);
        assert_eq!(e.preview.as_deref(), Some("Sounds good, see you there."));
    }

    #[test]
    fn body_with_html_and_attachment() {
        let raw = b"From: a@b.c\r\nSubject: x\r\nList-Unsubscribe: <https://u.example/x>\r\n\
List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n\
Content-Type: multipart/mixed; boundary=\"B\"\r\n\r\n\
--B\r\nContent-Type: multipart/alternative; boundary=\"A\"\r\n\r\n\
--A\r\nContent-Type: text/plain\r\n\r\nhello\r\n\
--A\r\nContent-Type: text/html\r\n\r\n<p>hello</p>\r\n--A--\r\n\
--B\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"q3.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\nSGVsbG8=\r\n--B--\r\n"
            .to_vec();
        let b = body_from_raw(raw.clone());
        assert_eq!(b.text.as_deref().map(str::trim), Some("hello"));
        assert!(b.html.as_deref().unwrap().contains("<p>hello</p>"));
        assert_eq!(b.attachments.len(), 1);
        assert_eq!(b.attachments[0].filename.as_deref(), Some("q3.pdf"));
        assert_eq!(b.attachments[0].mime_type, "application/pdf");
        assert!(!b.attachments[0].inline);
        assert!(b.list_unsubscribe.unwrap().contains("u.example"));
        assert!(b.list_unsubscribe_post);
        assert_eq!(attachment_bytes(&raw, 0).unwrap(), b"Hello");
    }

    #[test]
    fn plain_text_has_no_html() {
        let b = body_from_raw(b"Subject: x\r\n\r\njust text\r\n".to_vec());
        assert!(b.html.is_none());
    }
}

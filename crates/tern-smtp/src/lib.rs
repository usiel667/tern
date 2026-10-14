//! Turn a [`Draft`] into RFC 5322 bytes and submit it over SMTP.
//!
//! Building and sending are separate so the sync engine can queue raw bytes in
//! the outbox while offline and APPEND the same bytes to Sent afterwards.

use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::{Address as LAddress, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use tern_auth::Credential;
use tern_core::account::{Security, ServerConfig};
use tern_core::model::{Address, Draft};
use tern_core::{Error, Result};

/// A message ready to send: raw bytes plus the SMTP envelope (which includes Bcc).
pub struct Outgoing {
    pub raw: Vec<u8>,
    pub envelope: lettre::address::Envelope,
}

fn mailbox(a: &Address) -> Result<Mailbox> {
    let addr: LAddress = a
        .email
        .parse()
        .map_err(|e| Error::Config(format!("invalid address {}: {e}", a.email)))?;
    Ok(Mailbox::new(a.name.clone(), addr))
}

pub use lettre::Address as EmailAddress;

/// Build an SMTP envelope directly (used when resuming a queued send).
pub fn lettre_envelope(from: Option<EmailAddress>, to: Vec<EmailAddress>) -> Result<lettre::address::Envelope> {
    lettre::address::Envelope::new(from, to).map_err(|e| Error::Config(e.to_string()))
}

pub fn build(draft: &Draft) -> Result<Outgoing> {
    let from = draft
        .from
        .as_ref()
        .ok_or_else(|| Error::Config("draft has no From".into()))?;
    let mut b = Message::builder()
        .from(mailbox(from)?)
        .subject(draft.subject.clone())
        .date_now()
        .message_id(None)
        .user_agent("Tern".into());
    for a in &draft.to {
        b = b.to(mailbox(a)?);
    }
    for a in &draft.cc {
        b = b.cc(mailbox(a)?);
    }
    for a in &draft.bcc {
        b = b.bcc(mailbox(a)?);
    }
    if let Some(irt) = &draft.in_reply_to {
        b = b.in_reply_to(irt.clone());
    }
    if !draft.references.is_empty() {
        b = b.references(draft.references.join(" "));
    }

    let body = match &draft.html {
        Some(html) => MultiPart::alternative_plain_html(draft.text.clone(), html.clone()),
        None => MultiPart::mixed().singlepart(SinglePart::plain(draft.text.clone())),
    };
    let msg = if draft.attachments.is_empty() {
        match &draft.html {
            Some(_) => b.multipart(body),
            None => b.singlepart(SinglePart::plain(draft.text.clone())),
        }
    } else {
        let mut mixed = MultiPart::mixed().multipart(body);
        for a in &draft.attachments {
            let ct = ContentType::parse(&a.mime_type)
                .unwrap_or_else(|_| ContentType::parse("application/octet-stream").unwrap());
            mixed = mixed.singlepart(Attachment::new(a.filename.clone()).body(a.data.clone(), ct));
        }
        b.multipart(mixed)
    }
    .map_err(|e| Error::Config(e.to_string()))?;

    Ok(Outgoing {
        envelope: msg.envelope().clone(),
        raw: msg.formatted(),
    })
}

pub async fn send(server: &ServerConfig, cred: &Credential, out: &Outgoing) -> Result<()> {
    let builder = match server.security {
        Security::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&server.host),
        Security::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&server.host),
    }
    .map_err(|e| Error::Network(e.to_string()))?
    .port(server.port);

    let builder = match cred {
        Credential::Password { username, password } => {
            builder.credentials(Credentials::new(username.clone(), password.clone()))
        }
        Credential::OAuth2 { username, access_token } => builder
            .credentials(Credentials::new(username.clone(), access_token.clone()))
            .authentication(vec![Mechanism::Xoauth2]),
        Credential::Bearer { .. } => return Err(Error::Unsupported("bearer tokens over SMTP")),
    };
    builder
        .build()
        .send_raw(&out.envelope, &out.raw)
        .await
        .map_err(|e| {
            if e.is_permanent() {
                Error::Protocol(e.to_string())
            } else {
                Error::Network(e.to_string())
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_core::model::OutgoingAttachment;

    #[test]
    fn builds_reply_with_attachment_and_hidden_bcc() {
        let d = Draft {
            from: Some(Address::new(Some("Tom"), "tom@x.com")),
            to: vec![Address::new(Some("Ann"), "ann@x.com")],
            bcc: vec![Address::new(None, "secret@x.com")],
            subject: "Re: Café".into(),
            text: "Sounds good".into(),
            in_reply_to: Some("<b@x>".into()),
            references: vec!["<a@x>".into(), "<b@x>".into()],
            attachments: vec![OutgoingAttachment {
                filename: "notes.txt".into(),
                mime_type: "text/plain".into(),
                data: b"hello".to_vec(),
            }],
            ..Default::default()
        };
        let out = build(&d).unwrap();
        let text = String::from_utf8_lossy(&out.raw);
        assert!(!text.contains("secret@x.com"), "Bcc must not be in headers");
        assert_eq!(out.envelope.to().len(), 2, "Bcc is in the SMTP envelope");

        let parsed = mail_parser::MessageParser::default().parse(&out.raw).unwrap();
        assert_eq!(parsed.subject(), Some("Re: Café"));
        assert_eq!(parsed.in_reply_to().as_text(), Some("b@x"));
        assert!(parsed.message_id().is_some());
        assert_eq!(parsed.attachments().count(), 1);
        assert_eq!(parsed.body_text(0).unwrap().trim(), "Sounds good");
    }
}

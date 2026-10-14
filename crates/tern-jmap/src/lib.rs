//! JMAP backend (RFC 8620 core, RFC 8621 mail). Tested target: Fastmail with an
//! API token (Settings → Privacy & Security → API tokens, scopes Mail +
//! Submission).

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tern_core::backend::{MailBackend, MailboxDelta, PushEvent, SyncState};
use tern_core::mime::body_from_raw;
use tern_core::model::{Address, Envelope, Flags, MailboxRole, MessageBody, RemoteMailbox};
use tern_core::{Error, Result};
use tokio::sync::OnceCell;

const USING: &[&str] = &[
    "urn:ietf:params:jmap:core",
    "urn:ietf:params:jmap:mail",
    "urn:ietf:params:jmap:submission",
];

const EMAIL_PROPS: &[&str] = &[
    "id", "blobId", "messageId", "inReplyTo", "references", "subject", "from", "to", "cc",
    "receivedAt", "keywords", "size", "hasAttachment", "preview", "mailboxIds",
];

/// Keywords we own, mapped to the IMAP spelling `Flags` understands.
const KEYWORDS: &[(&str, &str)] = &[
    ("$seen", "\\Seen"),
    ("$answered", "\\Answered"),
    ("$flagged", "\\Flagged"),
    ("$draft", "\\Draft"),
    ("$forwarded", "$Forwarded"),
    ("$junk", "$Junk"),
    ("$mailflagbit0", "$MailFlagBit0"),
    ("$mailflagbit1", "$MailFlagBit1"),
    ("$mailflagbit2", "$MailFlagBit2"),
];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    api_url: String,
    download_url: String,
    upload_url: String,
    primary_accounts: Map<String, Value>,
}

pub struct JmapBackend {
    http: reqwest::Client,
    session_url: String,
    token: String,
    session: OnceCell<(Session, String)>,
}

fn net(e: impl std::fmt::Display) -> Error {
    Error::Network(e.to_string())
}

impl JmapBackend {
    pub fn new(session_url: String, token: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            session_url,
            token,
            session: OnceCell::new(),
        }
    }

    async fn session(&self) -> Result<&(Session, String)> {
        self.session
            .get_or_try_init(|| async {
                let resp = self
                    .http
                    .get(&self.session_url)
                    .bearer_auth(&self.token)
                    .send()
                    .await
                    .map_err(net)?;
                if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
                    return Err(Error::Auth("JMAP token rejected".into()));
                }
                let s: Session = resp.error_for_status().map_err(net)?.json().await.map_err(net)?;
                let account = s
                    .primary_accounts
                    .get("urn:ietf:params:jmap:mail")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::Protocol("no mail account in JMAP session".into()))?
                    .to_string();
                Ok((s, account))
            })
            .await
    }

    /// Run method calls; returns each call's arguments in order.
    async fn call(&self, calls: Vec<(&str, Value)>) -> Result<Vec<Value>> {
        let (s, _) = self.session().await?;
        let method_calls: Vec<Value> = calls
            .iter()
            .enumerate()
            .map(|(i, (name, args))| json!([name, args, format!("c{i}")]))
            .collect();
        let resp: Value = self
            .http
            .post(&s.api_url)
            .bearer_auth(&self.token)
            .json(&json!({ "using": USING, "methodCalls": method_calls }))
            .send()
            .await
            .map_err(net)?
            .error_for_status()
            .map_err(net)?
            .json()
            .await
            .map_err(net)?;
        let responses = resp["methodResponses"]
            .as_array()
            .ok_or_else(|| Error::Protocol("missing methodResponses".into()))?;
        responses
            .iter()
            .map(|r| {
                if r[0] == "error" {
                    Err(Error::Protocol(format!("JMAP error: {}", r[1])))
                } else {
                    Ok(r[1].clone())
                }
            })
            .collect()
    }

    async fn account(&self) -> Result<String> {
        Ok(self.session().await?.1.clone())
    }

    async fn upload(&self, raw: &[u8]) -> Result<String> {
        let (s, account) = self.session().await?;
        let url = s.upload_url.replace("{accountId}", account);
        let v: Value = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .header("Content-Type", "message/rfc822")
            .body(raw.to_vec())
            .send()
            .await
            .map_err(net)?
            .error_for_status()
            .map_err(net)?
            .json()
            .await
            .map_err(net)?;
        v["blobId"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| Error::Protocol("upload returned no blobId".into()))
    }

    async fn mailbox_id_by_role(&self, role: &str) -> Result<Option<String>> {
        let account = self.account().await?;
        let r = self
            .call(vec![("Mailbox/query", json!({ "accountId": account, "filter": { "role": role } }))])
            .await?;
        Ok(r[0]["ids"][0].as_str().map(str::to_owned))
    }
}

fn flags_from_keywords(k: &Value) -> Flags {
    let obj = k.as_object();
    let imap: Vec<&str> = KEYWORDS
        .iter()
        .filter(|(kw, _)| {
            obj.is_some_and(|o| o.iter().any(|(key, v)| key.eq_ignore_ascii_case(kw) && v == &Value::Bool(true)))
        })
        .map(|(_, i)| *i)
        .collect();
    Flags::from_imap(imap)
}

fn addresses(v: &Value) -> Vec<Address> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| Some(Address::new(x["name"].as_str(), x["email"].as_str()?)))
                .collect()
        })
        .unwrap_or_default()
}

fn ids(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(|s| s.as_str().map(|s| format!("<{s}>"))).collect())
        .unwrap_or_default()
}

fn envelope_of(e: &Value) -> Option<Envelope> {
    Some(Envelope {
        remote_id: e["id"].as_str()?.to_string(),
        message_id: ids(&e["messageId"]).into_iter().next(),
        in_reply_to: ids(&e["inReplyTo"]).into_iter().next(),
        references: ids(&e["references"]),
        subject: e["subject"].as_str().unwrap_or_default().to_string(),
        from: addresses(&e["from"]),
        to: addresses(&e["to"]),
        cc: addresses(&e["cc"]),
        date: e["receivedAt"]
            .as_str()
            .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_default(),
        flags: flags_from_keywords(&e["keywords"]),
        size: e["size"].as_u64().unwrap_or(0) as u32,
        has_attachments: e["hasAttachment"].as_bool().unwrap_or(false),
        preview: e["preview"].as_str().map(str::to_owned),
    })
}

fn role_of(v: &Value) -> MailboxRole {
    v.as_str().map(MailboxRole::parse).unwrap_or(MailboxRole::Custom)
}

#[async_trait]
impl MailBackend for JmapBackend {
    fn kind(&self) -> &'static str {
        "jmap"
    }

    async fn list_mailboxes(&self) -> Result<Vec<RemoteMailbox>> {
        let account = self.account().await?;
        let r = self
            .call(vec![("Mailbox/get", json!({ "accountId": account, "ids": null }))])
            .await?;
        Ok(r[0]["list"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| {
                Some(RemoteMailbox {
                    remote_id: m["id"].as_str()?.to_string(),
                    name: m["name"].as_str().unwrap_or_default().to_string(),
                    parent_remote_id: m["parentId"].as_str().map(str::to_owned),
                    role: role_of(&m["role"]),
                    selectable: true,
                })
            })
            .collect())
    }

    async fn sync_mailbox(
        &self,
        mailbox: &str,
        since: Option<&SyncState>,
        limit: Option<u32>,
    ) -> Result<MailboxDelta> {
        let account = self.account().await?;
        let mut delta = MailboxDelta::default();

        if let Some(state) = since.filter(|s| !s.0.is_empty()) {
            let r = self
                .call(vec![
                    ("Email/changes", json!({ "accountId": account, "sinceState": state.0, "maxChanges": 1000 })),
                    ("Email/get", json!({
                        "accountId": account,
                        "#ids": { "resultOf": "c0", "name": "Email/changes", "path": "/updated" },
                        "properties": EMAIL_PROPS,
                    })),
                    ("Email/get", json!({
                        "accountId": account,
                        "#ids": { "resultOf": "c0", "name": "Email/changes", "path": "/created" },
                        "properties": EMAIL_PROPS,
                    })),
                ])
                .await;
            match r {
                Ok(r) => {
                    let changes = &r[0];
                    for e in r[1]["list"].as_array().into_iter().chain(r[2]["list"].as_array()).flatten() {
                        let here = e["mailboxIds"].get(mailbox) == Some(&Value::Bool(true));
                        match (here, envelope_of(e)) {
                            (true, Some(env)) => delta.added.push(env), // upsert updates flags
                            (false, Some(env)) => delta.removed.push(env.remote_id),
                            _ => {}
                        }
                    }
                    for id in changes["destroyed"].as_array().into_iter().flatten() {
                        if let Some(id) = id.as_str() {
                            delta.removed.push(id.to_string());
                        }
                    }
                    // If hasMoreChanges, the next sync picks up from newState.
                    delta.new_state = SyncState(changes["newState"].as_str().unwrap_or_default().into());
                    return Ok(delta);
                }
                // cannotCalculateChanges: fall through to a full resync.
                Err(Error::Protocol(msg)) if msg.contains("cannotCalculateChanges") => {}
                Err(e) => return Err(e),
            }
        }

        delta.full_resync = true;
        let r = self
            .call(vec![
                ("Email/query", json!({
                    "accountId": account,
                    "filter": { "inMailbox": mailbox },
                    "sort": [{ "property": "receivedAt", "isAscending": false }],
                    "limit": limit.unwrap_or(5000),
                })),
                ("Email/get", json!({
                    "accountId": account,
                    "#ids": { "resultOf": "c0", "name": "Email/query", "path": "/ids" },
                    "properties": EMAIL_PROPS,
                })),
            ])
            .await?;
        delta.added = r[1]["list"].as_array().into_iter().flatten().filter_map(envelope_of).collect();
        delta.new_state = SyncState(r[1]["state"].as_str().unwrap_or_default().into());
        Ok(delta)
    }

    async fn fetch_body(&self, _mailbox: &str, remote_id: &str) -> Result<MessageBody> {
        let account = self.account().await?;
        let r = self
            .call(vec![("Email/get", json!({
                "accountId": account, "ids": [remote_id], "properties": ["blobId"],
            }))])
            .await?;
        let blob = r[0]["list"][0]["blobId"]
            .as_str()
            .ok_or_else(|| Error::NotFound(format!("email {remote_id}")))?;
        let (s, account) = self.session().await?;
        let url = s
            .download_url
            .replace("{accountId}", account)
            .replace("{blobId}", blob)
            .replace("{type}", "message/rfc822")
            .replace("{name}", "message.eml");
        let raw = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(net)?
            .error_for_status()
            .map_err(net)?
            .bytes()
            .await
            .map_err(net)?;
        Ok(body_from_raw(raw.to_vec()))
    }

    async fn set_flags(&self, _mailbox: &str, remote_ids: &[String], flags: &Flags) -> Result<()> {
        let set = flags.to_imap();
        let mut patch = Map::new();
        for (kw, imap) in KEYWORDS {
            let v = if set.contains(imap) { Value::Bool(true) } else { Value::Null };
            patch.insert(format!("keywords/{kw}"), v);
        }
        let update: Map<String, Value> =
            remote_ids.iter().map(|id| (id.clone(), Value::Object(patch.clone()))).collect();
        let account = self.account().await?;
        self.call(vec![("Email/set", json!({ "accountId": account, "update": update }))])
            .await?;
        Ok(())
    }

    async fn move_messages(&self, from: &str, remote_ids: &[String], to: &str) -> Result<Vec<String>> {
        let patch = json!({ format!("mailboxIds/{from}"): null, format!("mailboxIds/{to}"): true });
        let update: Map<String, Value> =
            remote_ids.iter().map(|id| (id.clone(), patch.clone())).collect();
        let account = self.account().await?;
        self.call(vec![("Email/set", json!({ "accountId": account, "update": update }))])
            .await?;
        // JMAP ids are stable across mailboxes.
        Ok(remote_ids.to_vec())
    }

    async fn append(&self, mailbox: &str, raw: &[u8], flags: &Flags) -> Result<Option<String>> {
        let blob = self.upload(raw).await?;
        let set = flags.to_imap();
        let keywords: Map<String, Value> = KEYWORDS
            .iter()
            .filter(|(_, i)| set.contains(i))
            .map(|(k, _)| (k.to_string(), Value::Bool(true)))
            .collect();
        let account = self.account().await?;
        let r = self
            .call(vec![("Email/import", json!({
                "accountId": account,
                "emails": { "e": { "blobId": blob, "mailboxIds": { mailbox: true }, "keywords": keywords } },
            }))])
            .await?;
        Ok(r[0]["created"]["e"]["id"].as_str().map(str::to_owned))
    }

    async fn expunge(&self, _mailbox: &str) -> Result<()> {
        Ok(())
    }

    async fn wait_for_push(&self, _mailbox: &str, timeout: Duration) -> Result<Option<PushEvent>> {
        // TODO: JMAP EventSource (session.eventSourceUrl) for real push. Polling
        // every couple of minutes is what Apple Mail does for many servers anyway.
        tokio::time::sleep(timeout.min(Duration::from_secs(120))).await;
        Ok(None)
    }

    fn handles_submission(&self) -> bool {
        true
    }

    async fn submit(&self, raw: &[u8], from: &str, recipients: &[String]) -> Result<()> {
        let account = self.account().await?;
        let sent = self
            .mailbox_id_by_role("sent")
            .await?
            .ok_or_else(|| Error::NotFound("Sent mailbox".into()))?;
        let identities = self
            .call(vec![("Identity/get", json!({ "accountId": account, "ids": null }))])
            .await?;
        let identity = identities[0]["list"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|i| i["email"].as_str().is_some_and(|e| e.eq_ignore_ascii_case(from)))
            .and_then(|i| i["id"].as_str())
            .ok_or_else(|| Error::Config(format!("no JMAP identity for {from}")))?
            .to_string();
        let blob = self.upload(raw).await?;
        let rcpt: Vec<Value> = recipients.iter().map(|r| json!({ "email": r })).collect();
        self.call(vec![
            ("Email/import", json!({
                "accountId": account,
                "emails": { "e": { "blobId": blob, "mailboxIds": { sent: true }, "keywords": { "$seen": true } } },
            })),
            ("EmailSubmission/set", json!({
                "accountId": account,
                "create": { "s": {
                    "identityId": identity,
                    "emailId": "#e",
                    "envelope": { "mailFrom": { "email": from }, "rcptTo": rcpt },
                } },
            })),
        ])
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_core::model::FlagColor;

    #[test]
    fn parses_email_object() {
        let e = json!({
            "id": "M1", "messageId": ["c@x"], "inReplyTo": ["b@x"], "references": ["a@x", "b@x"],
            "subject": "Hi", "from": [{"name": "Ann", "email": "ann@x.com"}],
            "to": [{"name": null, "email": "tom@x.com"}], "cc": null,
            "receivedAt": "2026-10-14T16:00:00Z",
            "keywords": {"$seen": true, "$flagged": true, "$MailFlagBit2": true},
            "size": 1234, "hasAttachment": true, "preview": "hello",
        });
        let env = envelope_of(&e).unwrap();
        assert_eq!(env.message_id.as_deref(), Some("<c@x>"));
        assert_eq!(env.references, vec!["<a@x>", "<b@x>"]);
        assert!(env.flags.seen);
        assert_eq!(env.flags.flag, Some(FlagColor::Blue));
        assert_eq!(env.date.timestamp(), 1791993600);
    }
}

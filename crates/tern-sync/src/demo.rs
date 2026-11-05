//! `tern --demo`: a fake account with realistic mail, so the UI can be developed
//! and screenshotted without real credentials. Nothing touches the network.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{Duration as CDuration, Utc};
use tern_core::Result;
use tern_core::account::{AccountConfig, Provider};
use tern_core::backend::{MailBackend, MailboxDelta, OlderBatch, OlderFrom, PushEvent, SyncState};
use tern_core::model::*;

use crate::{Engine, EngineHandle, Paths};

pub struct DemoBackend {
    boxes: Mutex<HashMap<String, Vec<(Envelope, Vec<u8>)>>>,
    /// Messages moved or sent after startup. A real server gives these new UIDs
    /// above the cached window, so they're always inside it whatever their date.
    arrived: Mutex<std::collections::HashSet<String>>,
    /// Counter for ids of appended messages (drafts).
    appended: Mutex<u32>,
}

fn addr(name: &str, email: &str) -> Address {
    Address::new(Some(name), email)
}

struct Spec {
    mailbox: &'static str,
    from: (&'static str, &'static str),
    subject: &'static str,
    hours_ago: i64,
    seen: bool,
    flag: Option<FlagColor>,
    id: &'static str,
    reply_to: Option<&'static str>,
    body: &'static str,
    html: bool,
}

const ME: (&str, &str) = ("Tom", "tom@example.com");

/// Generated older inbox mail, so scrolling back through history can be tried.
const HISTORY: usize = 600;
/// The demo "server" only returns this many newest messages per mailbox on first
/// sync (a real server's limit is 1,000), so loading older mail is visible.
const DEMO_WINDOW: usize = 50;

const HISTORY_SENDERS: &[(&str, &str, &str)] = &[
    ("Rust Weekly", "news@rustweekly.example", "This Week in Rust #"),
    ("GitHub", "notifications@github.example", "[tern] Pull request merged #"),
    ("Jordan Alvarez", "jordan@ops.example", "Daily ops report #"),
    ("Arch Linux", "news@archlinux.example", "Package update digest #"),
];

const SPECS: &[Spec] = &[
    Spec { mailbox: "INBOX", from: ("Maya Chen", "maya@studio.example"), subject: "Hyprland config for the new monitors", hours_ago: 1, seen: false, flag: None, id: "m1", reply_to: None,
        body: "Hey Tom,\n\nI finally got the 5K panels running at 2x scaling. The trick was setting\n`monitor=DP-1,5120x2880@60,0x0,2` and forcing GDK_SCALE in the env.\n\nWant me to send over the whole config?\n\nMaya", html: false },
    Spec { mailbox: "Sent", from: ME, subject: "Re: Hyprland config for the new monitors", hours_ago: 0, seen: true, flag: None, id: "m2", reply_to: Some("m1"),
        body: "Yes please! Especially the workspace rules.\n\n> On Oct 14, Maya Chen wrote:\n> Want me to send over the whole config?", html: false },
    Spec { mailbox: "INBOX", from: ("Rust Weekly", "news@rustweekly.example"), subject: "This Week in Rust #620", hours_ago: 5, seen: false, flag: None, id: "m3", reply_to: None,
        body: "<html><body style=\"font-family:-apple-system,system-ui;max-width:600px;margin:auto\"><h1 style=\"color:#b7410e\">This Week in Rust</h1><p>Hello and welcome to another issue!</p><h2>Updates from the Rust project</h2><ul><li>Async closures are now used across the ecosystem</li><li>New edition guide chapters</li></ul><img src=\"https://tracker.example/pixel.gif\" width=1 height=1><p><a href=\"https://example.com/unsubscribe\">Unsubscribe</a></p></body></html>", html: true },
    Spec { mailbox: "INBOX", from: ("Jordan Alvarez", "jordan@ops.example"), subject: "Order fulfillment numbers for Q3", hours_ago: 26, seen: true, flag: Some(FlagColor::Orange), id: "m4", reply_to: None,
        body: "Tom,\n\nAttached are the Q3 fulfillment numbers. Shipping latency is down 18%\nsince the new pick-list view went live. Nice work.\n\nCan we review Thursday?\n\nJordan", html: false },
    Spec { mailbox: "INBOX", from: ("GitHub", "notifications@github.example"), subject: "[tern] Build passed on main", hours_ago: 30, seen: true, flag: None, id: "m5", reply_to: None,
        body: "All checks have passed on main.\n\n  ✓ cargo test (stable)\n  ✓ cargo clippy\n  ✓ build (arch-linux)", html: false },
    Spec { mailbox: "INBOX", from: ("Sam Okafor", "sam@example.org"), subject: "Weekend climbing?", hours_ago: 50, seen: false, flag: Some(FlagColor::Blue), id: "m6", reply_to: None,
        body: "Thinking of heading up to Eldorado Canyon Saturday morning. You in?\n\n— Sam", html: false },
    Spec { mailbox: "INBOX", from: ("Priya Natarajan", "priya@example.net"), subject: "Re: OBS audio routing with PipeWire", hours_ago: 75, seen: true, flag: None, id: "m7", reply_to: None,
        body: "The virtual sink approach worked — I made a null-audio-sink for desktop audio and\nlinked it with qpwgraph. No more echo on stream.\n\nOn Sat, Tom wrote:\n> Did you try the pw-loopback thing?", html: false },
    Spec { mailbox: "INBOX", from: ("Linear", "updates@linear.example"), subject: "3 issues assigned to you", hours_ago: 120, seen: true, flag: None, id: "m8", reply_to: None,
        body: "<html><body style=\"font-family:system-ui\"><h3>Your assigned issues</h3><ol><li>ORD-412 Bulk-ship selected orders</li><li>ORD-415 Fix timezone on packing slips</li><li>ORD-420 Add carrier rate cache</li></ol></body></html>", html: true },
    Spec { mailbox: "Archive", from: ("Arch Linux", "news@archlinux.example"), subject: "Manual intervention required for linux-firmware", hours_ago: 400, seen: true, flag: None, id: "m9", reply_to: None,
        body: "The linux-firmware package has been split. Run pacman -Syu --overwrite as described in the news post.", html: false },
    Spec { mailbox: "Drafts", from: ME, subject: "Proposal: keyboard-first mail client", hours_ago: 10, seen: true, flag: None, id: "m10", reply_to: None,
        body: "Notes:\n- j/k navigation\n- Ctrl+R to reply\n- tmux-friendly", html: false },
    // A typical white newsletter and one with its own dark styles, for trying dark mode.
    Spec { mailbox: "INBOX", from: ("Trailhead Outfitters", "deals@trailhead.example"), subject: "Fall gear sale: 30% off packs", hours_ago: 2, seen: false, flag: None, id: "m11", reply_to: None,
        body: r##"<html><body style="margin:0;background:#f2f2f2"><table width="100%" cellpadding="0" cellspacing="0" bgcolor="#f2f2f2"><tr><td align="center" style="padding:24px 0"><table width="600" cellpadding="0" cellspacing="0" bgcolor="#ffffff" style="font-family:Helvetica,Arial,sans-serif;color:#333333"><tr><td bgcolor="#2d5a27" style="padding:20px 32px"><img alt="Trailhead" width="140" height="32" src="data:image/svg+xml;utf8,%3Csvg xmlns='http://www.w3.org/2000/svg' width='140' height='32'%3E%3Cpath d='M4 28 L16 6 L28 28 Z' fill='%23f5c542'/%3E%3Ctext x='36' y='23' font-family='Arial' font-weight='bold' font-size='17' fill='white'%3ETRAILHEAD%3C/text%3E%3C/svg%3E"></td></tr><tr><td style="padding:32px"><h1 style="margin:0 0 12px;font-size:28px;color:#1b3618">Fall gear sale</h1><p style="font-size:16px;line-height:24px;margin:0 0 20px">Take <b style="color:#c0392b">30% off</b> every pack and tent through Sunday. Members get free shipping on orders over $50.</p><table cellpadding="0" cellspacing="0"><tr><td bgcolor="#e8772e" style="border-radius:6px;padding:12px 24px"><a href="https://trailhead.example/sale" style="color:#ffffff;font-weight:bold;text-decoration:none">Shop the sale</a></td></tr></table></td></tr><tr><td style="padding:0 32px 32px"><table width="100%" cellpadding="0" cellspacing="0"><tr><td width="50%" style="padding-right:8px;vertical-align:top"><div style="background:#fff7e6;border:1px solid #f0d9a8;padding:16px;border-radius:6px"><b>Ridgeline 40L</b><br><span style="color:#888888;text-decoration:line-through">$180</span> <span style="color:#2d5a27;font-weight:bold">$126</span></div></td><td width="50%" style="padding-left:8px;vertical-align:top"><div style="background:#eef6ee;border:1px solid #c8e0c5;padding:16px;border-radius:6px"><b>Basecamp 2P tent</b><br><span style="color:#888888;text-decoration:line-through">$320</span> <span style="color:#2d5a27;font-weight:bold">$224</span></div></td></tr></table></td></tr><tr><td bgcolor="#fafafa" style="padding:20px 32px;font-size:12px;color:#999999;border-top:1px solid #eeeeee">You're getting this because you signed up at trailhead.example. <a href="https://example.com/unsubscribe" style="color:#999999">Unsubscribe</a></td></tr></table></td></tr></table></body></html>"##, html: true },
    Spec { mailbox: "INBOX", from: ("Lumen Notes", "hello@lumen.example"), subject: "What's new in Lumen 3.2", hours_ago: 3, seen: false, flag: None, id: "m12", reply_to: None,
        body: r##"<html><head><meta name="color-scheme" content="light dark"><style>body{background:#ffffff;color:#222222;font-family:Arial,sans-serif}.card{background:#f4f4f8;border-radius:8px;padding:16px}.accent{color:#5b4bdb}@media (prefers-color-scheme: dark){body{background:#121212!important;color:#e6e6e6!important}.card{background:#1f1f24!important}.accent{color:#a99cff!important}}</style></head><body><div style="max-width:560px;margin:24px auto;padding:0 16px"><h2 class="accent">Lumen 3.2 is here</h2><p>Backlinks now update live, and the graph view got a lot faster on big vaults.</p><div class="card"><b>Tip:</b> press <code>Ctrl+K</code> to jump to any note.</div><p style="font-size:12px;opacity:.7">This message supports dark mode.</p></div></body></html>"##, html: true },
];

impl DemoBackend {
    pub fn new() -> Self {
        let mut boxes: HashMap<String, Vec<(Envelope, Vec<u8>)>> = HashMap::new();
        for name in ["INBOX", "Drafts", "Sent", "Junk", "Trash", "Archive", "Receipts", "Projects/Tern"] {
            boxes.insert(name.into(), vec![]);
        }
        for (uid, s) in SPECS.iter().enumerate() {
            let date = Utc::now() - CDuration::hours(s.hours_ago) - CDuration::minutes(uid as i64 * 7);
            let ctype = if s.html { "text/html" } else { "text/plain" };
            let mut raw = format!(
                "From: {} <{}>\r\nTo: Tom <tom@example.com>\r\nSubject: {}\r\nDate: {}\r\nMessage-ID: <{}@demo>\r\n",
                s.from.0, s.from.1, s.subject, date.to_rfc2822(), s.id
            );
            if let Some(r) = s.reply_to {
                raw.push_str(&format!("In-Reply-To: <{r}@demo>\r\nReferences: <{r}@demo>\r\n"));
            }
            if s.html {
                raw.push_str("List-Unsubscribe: <https://example.com/unsubscribe>\r\n");
            }
            raw.push_str(&format!("MIME-Version: 1.0\r\nContent-Type: {ctype}; charset=utf-8\r\n\r\n{}\r\n", s.body));
            let preview = if s.html {
                Some(strip_tags(s.body))
            } else {
                Some(s.body.lines().filter(|l| !l.starts_with('>')).collect::<Vec<_>>().join(" "))
            };
            let env = Envelope {
                remote_id: (uid + 1).to_string(),
                message_id: Some(format!("<{}@demo>", s.id)),
                in_reply_to: s.reply_to.map(|r| format!("<{r}@demo>")),
                references: s.reply_to.map(|r| vec![format!("<{r}@demo>")]).unwrap_or_default(),
                subject: s.subject.into(),
                from: vec![addr(s.from.0, s.from.1)],
                to: vec![addr(ME.0, ME.1)],
                cc: vec![],
                date,
                flags: Flags { seen: s.seen, flag: s.flag, ..Default::default() },
                size: raw.len() as u32,
                has_attachments: false,
                preview: preview.map(|p| p.chars().take(200).collect()),
            };
            boxes.get_mut(s.mailbox).unwrap().push((env, raw.into_bytes()));
        }
        let inbox = boxes.get_mut("INBOX").unwrap();
        for i in 0..HISTORY {
            let (name, email, subject) = HISTORY_SENDERS[i % HISTORY_SENDERS.len()];
            let n = HISTORY - i;
            let subject = format!("{subject}{n}");
            let date = Utc::now() - CDuration::days(6) - CDuration::hours(i as i64 * 8);
            let body = format!("Older demo message {n} of {HISTORY}, from {name}.");
            let raw = format!(
                "From: {name} <{email}>\r\nTo: Tom <tom@example.com>\r\nSubject: {subject}\r\nDate: {}\r\n\
                 Message-ID: <h{n}@demo>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n",
                date.to_rfc2822()
            );
            let env = Envelope {
                remote_id: (1000 + i).to_string(),
                message_id: Some(format!("<h{n}@demo>")),
                in_reply_to: None,
                references: vec![],
                subject,
                from: vec![addr(name, email)],
                to: vec![addr(ME.0, ME.1)],
                cc: vec![],
                date,
                flags: Flags { seen: true, ..Default::default() },
                size: raw.len() as u32,
                has_attachments: false,
                preview: Some(body),
            };
            inbox.push((env, raw.into_bytes()));
        }
        Self { boxes: Mutex::new(boxes), arrived: Default::default(), appended: Default::default() }
    }
}

impl DemoBackend {
    fn envelopes(&self, mailbox: &str) -> Vec<Envelope> {
        let boxes = self.boxes.lock().unwrap();
        boxes.get(mailbox).map(|v| v.iter().map(|(e, _)| e.clone()).collect()).unwrap_or_default()
    }
}

impl Default for DemoBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// The demo cursor: `demo:<unix seconds>`, the date of the oldest cached message.
/// Everything at or after it is "cached", like an IMAP UID window.
fn cutoff_of(since: Option<&SyncState>) -> Option<i64> {
    since?.0.strip_prefix("demo:")?.parse().ok()
}

/// Messages newest first.
fn by_date_desc(mut all: Vec<Envelope>) -> Vec<Envelope> {
    all.sort_by_key(|e| std::cmp::Reverse(e.date));
    all
}

fn strip_tags(html: &str) -> String {
    let html = html.split_once("</head>").map_or(html, |(_, body)| body);
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn role(name: &str) -> MailboxRole {
    match name {
        "INBOX" => MailboxRole::Inbox,
        "Drafts" => MailboxRole::Drafts,
        "Sent" => MailboxRole::Sent,
        "Junk" => MailboxRole::Junk,
        "Trash" => MailboxRole::Trash,
        "Archive" => MailboxRole::Archive,
        _ => MailboxRole::Custom,
    }
}

#[async_trait]
impl MailBackend for DemoBackend {
    fn kind(&self) -> &'static str {
        "demo"
    }

    async fn list_mailboxes(&self) -> Result<Vec<RemoteMailbox>> {
        let boxes = self.boxes.lock().unwrap();
        let mut out: Vec<RemoteMailbox> = boxes
            .keys()
            .map(|k| RemoteMailbox {
                remote_id: k.clone(),
                name: if k == "INBOX" { "Inbox".into() } else { k.rsplit('/').next().unwrap().into() },
                parent_remote_id: k.rsplit_once('/').map(|(p, _)| p.to_string()),
                role: role(k),
                selectable: true,
            })
            .collect();
        out.push(RemoteMailbox {
            remote_id: "Projects".into(),
            name: "Projects".into(),
            parent_remote_id: None,
            role: MailboxRole::Custom,
            selectable: false,
        });
        Ok(out)
    }

    async fn sync_mailbox(&self, mailbox: &str, since: Option<&SyncState>, _: Option<u32>) -> Result<MailboxDelta> {
        let all = by_date_desc(self.envelopes(mailbox));
        let cutoff = cutoff_of(since).unwrap_or_else(|| {
            all.get(DEMO_WINDOW.min(all.len()).saturating_sub(1)).map_or(0, |e| e.date.timestamp())
        });
        let arrived = self.arrived.lock().unwrap().clone();
        let visible: Vec<Envelope> = all
            .into_iter()
            .filter(|e| e.date.timestamp() >= cutoff || arrived.contains(&e.remote_id))
            .collect();
        Ok(MailboxDelta {
            full_resync: since.is_none(),
            present: Some(visible.iter().map(|e| e.remote_id.clone()).collect()),
            added: visible,
            new_state: SyncState(format!("demo:{cutoff}")),
            ..Default::default()
        })
    }

    async fn fetch_older(&self, mailbox: &str, since: &SyncState, _: &OlderFrom, count: u32) -> Result<OlderBatch> {
        let Some(cutoff) = cutoff_of(Some(since)) else {
            return Ok(OlderBatch { delta: MailboxDelta { new_state: since.clone(), ..Default::default() }, more: true });
        };
        let arrived = self.arrived.lock().unwrap().clone();
        let older: Vec<Envelope> = by_date_desc(self.envelopes(mailbox))
            .into_iter()
            .filter(|e| e.date.timestamp() < cutoff && !arrived.contains(&e.remote_id))
            .collect();
        let more = older.len() > count as usize;
        let added: Vec<Envelope> = older.into_iter().take(count as usize).collect();
        let new_cutoff = added.last().map_or(cutoff, |e| e.date.timestamp());
        Ok(OlderBatch {
            delta: MailboxDelta { added, new_state: SyncState(format!("demo:{new_cutoff}")), ..Default::default() },
            more,
        })
    }

    async fn fetch_body(&self, mailbox: &str, remote_id: &str) -> Result<MessageBody> {
        tokio::time::sleep(Duration::from_millis(150)).await; // feel like a network
        self.prefetch_body(mailbox, remote_id).await
    }

    /// No pretend delay: hundreds of these run in tests.
    async fn prefetch_body(&self, mailbox: &str, remote_id: &str) -> Result<MessageBody> {
        let boxes = self.boxes.lock().unwrap();
        let raw = boxes
            .values()
            .flatten()
            .find(|(e, _)| e.remote_id == remote_id)
            .map(|(_, r)| r.clone())
            .ok_or_else(|| tern_core::Error::NotFound(format!("{mailbox}/{remote_id}")))?;
        Ok(tern_core::mime::body_from_raw(raw))
    }

    async fn set_flags(&self, mailbox: &str, rids: &[String], flags: &Flags) -> Result<()> {
        let mut boxes = self.boxes.lock().unwrap();
        for (e, _) in boxes.get_mut(mailbox).into_iter().flatten() {
            if rids.contains(&e.remote_id) {
                e.flags = flags.clone();
            }
        }
        Ok(())
    }

    async fn move_messages(&self, from: &str, rids: &[String], to: &str) -> Result<Vec<String>> {
        let mut boxes = self.boxes.lock().unwrap();
        let src = boxes.entry(from.into()).or_default();
        let (moved, keep): (Vec<_>, Vec<_>) = src.drain(..).partition(|(e, _)| rids.contains(&e.remote_id));
        *src = keep;
        boxes.entry(to.into()).or_default().extend(moved);
        self.arrived.lock().unwrap().extend(rids.iter().cloned());
        Ok(rids.to_vec())
    }

    async fn append(&self, mailbox: &str, raw: &[u8], flags: &Flags) -> Result<Option<String>> {
        // Keep it, like a server would, so drafts can be tried in the demo.
        let id = {
            let mut n = self.appended.lock().unwrap();
            *n += 1;
            format!("a{}", *n)
        };
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
        let env = tern_core::mime::envelope_from_parts(id.clone(), &raw[..split], Some(&raw[split..]), flags.clone(), raw.len() as u32, None);
        self.arrived.lock().unwrap().insert(id.clone());
        self.boxes.lock().unwrap().entry(mailbox.into()).or_default().push((env, raw.to_vec()));
        Ok(Some(id))
    }

    async fn expunge(&self, mailbox: &str) -> Result<()> {
        let mut boxes = self.boxes.lock().unwrap();
        if let Some(v) = boxes.get_mut(mailbox) {
            v.retain(|(e, _)| !e.flags.deleted);
        }
        Ok(())
    }

    async fn wait_for_push(&self, _: &str, timeout: Duration) -> Result<Option<PushEvent>> {
        tokio::time::sleep(timeout).await;
        Ok(None)
    }

    fn handles_submission(&self) -> bool {
        true
    }

    /// Small enough to try the size check with a couple of attachments.
    async fn send_limit(&self) -> Result<Option<u64>> {
        Ok(Some(DEMO_SEND_LIMIT))
    }

    async fn submit(&self, raw: &[u8], _: &str, _: &[String]) -> Result<()> {
        // A way to see a refused send in the demo: put "[reject]" in the subject.
        if String::from_utf8_lossy(&raw[..raw.len().min(4096)]).contains("[reject]") {
            return Err(tern_core::Error::Rejected("550 5.7.1 Message rejected by the demo server".into()));
        }
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
        let env = tern_core::mime::envelope_from_parts(
            format!("s{}", raw.len()),
            &raw[..split],
            Some(&raw[split..]),
            Flags { seen: true, ..Default::default() },
            raw.len() as u32,
            None,
        );
        self.arrived.lock().unwrap().insert(env.remote_id.clone());
        self.boxes.lock().unwrap().entry("Sent".into()).or_default().push((env, raw.to_vec()));
        Ok(())
    }
}

/// The demo server's message size limit (10 MB).
pub const DEMO_SEND_LIMIT: u64 = 10 * 1024 * 1024;

/// The demo account's config (fixed id so the UI can refer to it).
pub fn demo_account() -> AccountConfig {
    let mut cfg = AccountConfig::preset(Provider::Fastmail, ME.0, ME.1);
    cfg.id = AccountId(uuid::Uuid::from_u128(0x7e2a));
    cfg.description = "Demo".into();
    cfg.signature = Some("Sent from Tern".into());
    cfg
}

/// Start an engine with one demo account and an in-memory-like store at `paths`.
pub fn spawn_demo(paths: Paths) -> Result<EngineHandle> {
    let _ = std::fs::remove_file(&paths.database);
    let (cmd_tx, cmd_rx) = async_channel::unbounded();
    let (ev_tx, ev_rx) = async_channel::unbounded();
    let db = paths.database.clone();
    // Open (and create) the database before returning so the UI can read it.
    let store = tern_store::Store::open(&db)?;
    std::thread::Builder::new()
        .name("tern-sync-demo".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                let mut engine = Engine::new(store, ev_tx);
                engine.add(demo_account(), Arc::new(DemoBackend::new()));
                engine.run(cmd_rx).await;
            });
        })
        .map_err(|e| tern_core::Error::Config(e.to_string()))?;
    Ok(EngineHandle { commands: cmd_tx, events: ev_rx, paths })
}

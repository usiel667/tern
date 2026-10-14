use super::*;
use chrono::TimeZone;

fn env(rid: &str, msgid: &str, irt: Option<&str>, subject: &str, from: &str, hour: u32) -> Envelope {
    Envelope {
        remote_id: rid.into(),
        message_id: Some(msgid.into()),
        in_reply_to: irt.map(Into::into),
        references: irt.map(|s| vec![s.to_string()]).unwrap_or_default(),
        subject: subject.into(),
        from: vec![Address::new(Some(from), &format!("{}@example.com", from.to_lowercase()))],
        to: vec![Address::new(None, "tom@example.com")],
        cc: vec![],
        date: Utc.with_ymd_and_hms(2026, 10, 1, hour, 0, 0).unwrap(),
        flags: Flags::default(),
        size: 1000,
        has_attachments: false,
        preview: Some(format!("preview of {subject}")),
    }
}

fn setup() -> (Store, AccountId, MailboxId, MailboxId) {
    let mut s = Store::open_in_memory().unwrap();
    let acct = AccountId::new();
    let mb = |rid: &str, role| RemoteMailbox {
        remote_id: rid.into(),
        name: rid.into(),
        parent_remote_id: None,
        role,
        selectable: true,
    };
    s.sync_mailboxes(acct, &[mb("INBOX", MailboxRole::Inbox), mb("Sent", MailboxRole::Sent)])
        .unwrap();
    let boxes = s.mailboxes(Some(acct)).unwrap();
    (s, acct, boxes[0].id, boxes[1].id)
}

#[test]
fn sidebar_order_and_counts() {
    let (mut s, acct, inbox, _) = setup();
    s.apply_delta(
        inbox,
        &MailboxDelta {
            added: vec![env("1", "<a@x>", None, "Hi", "Ann", 1)],
            new_state: SyncState("s1".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let boxes = s.mailboxes(Some(acct)).unwrap();
    assert_eq!(boxes[0].remote.role, MailboxRole::Inbox);
    assert_eq!((boxes[0].unread, boxes[0].total), (1, 1));
    assert_eq!(s.sync_state(inbox).unwrap(), Some(SyncState("s1".into())));
}

#[test]
fn conversations_span_mailboxes() {
    let (mut s, _, inbox, sent) = setup();
    s.apply_delta(
        inbox,
        &MailboxDelta {
            added: vec![
                env("1", "<a@x>", None, "Lunch", "Ann", 1),
                env("2", "<c@x>", None, "Other", "Bob", 2),
                env("3", "<d@x>", Some("<b@x>"), "Re: Lunch", "Ann", 4),
            ],
            ..Default::default()
        },
    )
    .unwrap();
    s.apply_delta(
        sent,
        &MailboxDelta {
            added: vec![env("9", "<b@x>", Some("<a@x>"), "Re: Lunch", "Tom", 3)],
            ..Default::default()
        },
    )
    .unwrap();

    let rows = s.list(&ListScope::Mailbox(inbox), true, 50).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].envelope.subject, "Re: Lunch");
    assert_eq!(rows[0].thread_count, 3, "includes the Sent reply");

    let flat = s.list(&ListScope::Mailbox(inbox), false, 50).unwrap();
    assert_eq!(flat.len(), 3);

    let thread = s.thread(rows[0].thread_id.unwrap()).unwrap();
    let froms: Vec<_> = thread.iter().map(|m| m.envelope.from[0].display().to_string()).collect();
    assert_eq!(froms, ["Ann", "Tom", "Ann"]);
}

#[test]
fn flags_removal_and_resync() {
    let (mut s, _, inbox, _) = setup();
    s.apply_delta(
        inbox,
        &MailboxDelta {
            added: vec![env("1", "<a@x>", None, "A", "Ann", 1), env("2", "<b@x>", None, "B", "Bob", 2)],
            ..Default::default()
        },
    )
    .unwrap();
    let seen = Flags { seen: true, flag: Some(FlagColor::Blue), ..Default::default() };
    s.apply_delta(
        inbox,
        &MailboxDelta {
            flags_changed: vec![("1".into(), seen.clone())],
            removed: vec!["2".into()],
            ..Default::default()
        },
    )
    .unwrap();
    let rows = s.list(&ListScope::Mailbox(inbox), false, 50).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].envelope.flags, seen);
    assert_eq!(s.list(&ListScope::Flagged, false, 50).unwrap().len(), 1);
    assert_eq!(s.list(&ListScope::Unread, false, 50).unwrap().len(), 0);

    s.apply_delta(
        inbox,
        &MailboxDelta {
            added: vec![env("5", "<e@x>", None, "E", "Eve", 5)],
            ..Default::default()
        },
    )
    .unwrap();
    // Server says only uid 5 remains: uid 1 was expunged elsewhere.
    s.apply_delta(inbox, &MailboxDelta { present: Some(vec!["5".into()]), ..Default::default() })
        .unwrap();
    let left: Vec<_> = s
        .list(&ListScope::Mailbox(inbox), false, 50)
        .unwrap()
        .into_iter()
        .map(|m| m.envelope.remote_id)
        .collect();
    assert_eq!(left, vec!["5"]);

    s.apply_delta(inbox, &MailboxDelta { full_resync: true, ..Default::default() }).unwrap();
    assert!(s.list(&ListScope::Mailbox(inbox), false, 50).unwrap().is_empty());
}

#[test]
fn full_text_search_with_tokens() {
    let (mut s, _, inbox, _) = setup();
    s.apply_delta(
        inbox,
        &MailboxDelta {
            added: vec![
                env("1", "<a@x>", None, "Quarterly invoice", "Ann", 1),
                env("2", "<b@x>", None, "Invoice reminder", "Bob", 2),
                env("3", "<c@x>", None, "Café plans", "Ann", 3),
            ],
            ..Default::default()
        },
    )
    .unwrap();
    let id = s.list(&ListScope::Mailbox(inbox), false, 50).unwrap()[2].id;
    s.store_body(
        id,
        &MessageBody {
            text: Some("Please find the attached PDF for Q3".into()),
            html: None,
            attachments: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: false,
            raw: b"raw".to_vec(),
        },
    )
    .unwrap();

    let q = |s: &Store, text: &str| s.search(&SearchQuery::parse(text), &ListScope::Everything, 50).unwrap().len();
    assert_eq!(q(&s, "invoice"), 2);
    assert_eq!(q(&s, "from:ann invoice"), 1);
    assert_eq!(q(&s, "invo"), 2, "prefix match as you type");
    assert_eq!(q(&s, "cafe"), 1, "diacritics folded");
    assert_eq!(q(&s, "attached"), 1, "body indexed after download");
    assert_eq!(q(&s, "is:unread"), 3);

    let body = s.body(id).unwrap().unwrap();
    assert_eq!(body.raw, b"raw");
}

#[test]
fn outbox_round_trip() {
    let (mut s, acct, _, _) = setup();
    let id = s.outbox_push(acct, b"Subject: hi\r\n\r\nhi").unwrap();
    assert_eq!(s.outbox().unwrap().len(), 1);
    s.outbox_failed(id, "offline").unwrap();
    s.outbox_done(id).unwrap();
    assert!(s.outbox().unwrap().is_empty());
}

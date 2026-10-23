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
    let to = vec!["ann@x.com".to_string(), "secret-bcc@x.com".to_string()];
    let id = s.outbox_push(acct, b"Subject: hi\r\n\r\nhi", "tom@x.com", &to).unwrap();
    let items = s.outbox().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].envelope, Some(("tom@x.com".to_string(), to)));
    s.outbox_failed(id, "offline").unwrap();
    let item = &s.outbox().unwrap()[0];
    assert_eq!((item.attempts, item.last_error.as_deref()), (1, Some("offline")));
    s.outbox_done(id).unwrap();
    assert!(s.outbox().unwrap().is_empty());
}

#[test]
fn migrates_v1_outbox() {
    // A v1 database with a queued send, as shipped in milestone 1.
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE outbox (id INTEGER PRIMARY KEY, account TEXT NOT NULL, raw BLOB NOT NULL,
             created INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, last_error TEXT);
         INSERT INTO outbox (account, raw, created) VALUES ('00000000-0000-0000-0000-000000000000', x'41', 1);
         PRAGMA user_version = 1;",
    )
    .unwrap();
    let mut s = Store::init(conn).unwrap();
    let old = &s.outbox().unwrap()[0];
    assert_eq!((old.raw.as_slice(), &old.envelope), (b"A".as_slice(), &None));
    // New rows can use the added column.
    s.outbox_push(old.account, b"B", "tom@x.com", &["ann@x.com".into()]).unwrap();
    assert!(s.outbox().unwrap()[1].envelope.is_some());
    let v: i64 = s.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, SCHEMA_VERSION);
}

#[test]
fn pending_ops_queue_in_order_per_account() {
    let (mut s, acct, _, _) = setup();
    let other = AccountId::new();
    let a = s.pending_push(acct, "first").unwrap();
    s.pending_push(other, "elsewhere").unwrap();
    s.pending_push(acct, "second").unwrap();
    let ops: Vec<String> = s.pending_ops(acct).unwrap().into_iter().map(|(_, op)| op).collect();
    assert_eq!(ops, ["first", "second"]);
    s.pending_done(a).unwrap();
    assert_eq!(s.pending_ops(acct).unwrap()[0].1, "second");
    s.pending_clear(acct).unwrap();
    assert!(s.pending_ops(acct).unwrap().is_empty());
    assert_eq!(s.pending_ops(other).unwrap().len(), 1, "other accounts untouched");
}

#[test]
fn edit_rows_by_remote_id() {
    let (mut s, _, inbox, _) = setup();
    let delta = MailboxDelta {
        added: vec![env("1", "<a@x>", None, "A", "Ann", 1), env("2", "<b@x>", None, "B", "Bob", 2)],
        new_state: SyncState("s".into()),
        ..Default::default()
    };
    s.apply_delta(inbox, &delta).unwrap();
    let flagged = Flags { seen: true, flag: Some(FlagColor::Red), ..Default::default() };
    s.set_flags_remote(inbox, "1", &flagged).unwrap();
    s.set_flags_remote(inbox, "missing", &flagged).unwrap(); // not cached: no-op
    s.delete_remote(inbox, &["2".to_string()]).unwrap();
    let rows = s.list(&ListScope::Mailbox(inbox), false, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].envelope.flags, flagged);
}

#[test]
fn oldest_cached_reports_where_history_ends() {
    let (mut s, _, inbox, _) = setup();
    assert_eq!(s.oldest_cached(inbox).unwrap(), (None, 0));
    let delta = MailboxDelta {
        added: vec![env("7", "<a@x>", None, "A", "Ann", 5), env("3", "<b@x>", None, "B", "Bob", 2)],
        new_state: SyncState("s".into()),
        ..Default::default()
    };
    s.apply_delta(inbox, &delta).unwrap();
    assert_eq!(s.oldest_cached(inbox).unwrap(), (Some("3".to_string()), 2));
}

fn mail(rid: &str, from: Address, to: Vec<Address>, hour: u32) -> Envelope {
    Envelope { from: vec![from], to, ..env(rid, &format!("<{rid}@x>"), None, "s", "x", hour) }
}

fn emails(a: &[Address]) -> Vec<&str> {
    a.iter().map(|a| a.email.as_str()).collect()
}

fn add(s: &mut Store, mb: MailboxId, added: Vec<Envelope>) {
    s.apply_delta(mb, &MailboxDelta { added, new_state: SyncState("s".into()), ..Default::default() })
        .unwrap();
}

#[test]
fn learns_contacts_by_mailbox_role() {
    let (mut s, acct, inbox, sent) = setup();
    let mb = |rid: &str, role| RemoteMailbox {
        remote_id: rid.into(),
        name: rid.into(),
        parent_remote_id: None,
        role,
        selectable: true,
    };
    s.sync_mailboxes(acct, &[mb("INBOX", MailboxRole::Inbox), mb("Sent", MailboxRole::Sent), mb("Junk", MailboxRole::Junk)])
        .unwrap();
    let junk = s.mailboxes(Some(acct)).unwrap().into_iter().find(|m| m.remote.role == MailboxRole::Junk).unwrap().id;
    let me = Address::new(Some("Tom"), "tom@example.com");
    add(&mut s, inbox, vec![
        mail("1", Address::new(Some("Maya Chen"), "maya@example.com"), vec![me.clone()], 1),
        mail("2", Address::new(None, "noreply@github.com"), vec![me.clone()], 2),
        mail("3", Address::new(Some("Bounces"), "bounce-123@list.org"), vec![me.clone()], 3),
    ]);
    add(&mut s, sent, vec![Envelope {
        cc: vec![Address::new(Some("Carl Cc"), "carl@example.com")],
        ..mail("9", me.clone(), vec![Address::new(Some("Nora Work"), "nora@work.com")], 4)
    }]);
    add(&mut s, junk, vec![mail("5", Address::new(Some("Deals"), "deals@spam.example"), vec![me.clone()], 5)]);

    assert_eq!(emails(&s.suggest_addresses("maya", 8).unwrap()), ["maya@example.com"]);
    assert_eq!(emails(&s.suggest_addresses("chen", 8).unwrap()), ["maya@example.com"], "name word");
    assert_eq!(emails(&s.suggest_addresses("Nor", 8).unwrap()), ["nora@work.com"], "Sent To learned");
    assert_eq!(emails(&s.suggest_addresses("carl", 8).unwrap()), ["carl@example.com"], "Sent Cc learned");
    assert!(s.suggest_addresses("deals", 8).unwrap().is_empty(), "Junk ignored");
    assert!(s.suggest_addresses("noreply", 8).unwrap().is_empty(), "machine sender ignored");
    assert!(s.suggest_addresses("bounce", 8).unwrap().is_empty());
    assert!(s.suggest_addresses("tom", 8).unwrap().is_empty(), "recipients of received mail aren't learned");
    assert!(s.suggest_addresses("", 8).unwrap().is_empty());
}

#[test]
fn suggestions_rank_people_you_wrote_to_first() {
    let (mut s, _, inbox, sent) = setup();
    let me = Address::new(None, "tom@example.com");
    let news: Vec<Envelope> = (0..10)
        .map(|i| mail(&format!("n{i}"), Address::new(Some("Shop News"), "news@shop.com"), vec![me.clone()], 10 + i))
        .collect();
    add(&mut s, inbox, news.clone());
    add(&mut s, sent, vec![mail("s1", me.clone(), vec![Address::new(None, "nora@work.com")], 1)]);
    assert_eq!(emails(&s.suggest_addresses("n", 8).unwrap()), ["nora@work.com", "news@shop.com"]);
    // Re-applying the same rows (a flag re-sync) doesn't count them again.
    add(&mut s, inbox, news);
    let received: i64 = s.conn.query_row("SELECT received FROM contacts WHERE email = 'news@shop.com'", [], |r| r.get(0)).unwrap();
    assert_eq!(received, 10);
}

#[test]
fn suggestions_take_latest_name_and_literal_wildcards() {
    let (mut s, _, inbox, _) = setup();
    let me = Address::new(None, "tom@example.com");
    add(&mut s, inbox, vec![
        mail("1", Address::new(Some("Old Name"), "ann@x.org"), vec![me.clone()], 1),
        mail("2", Address::new(Some("Ann Lee"), "ANN@x.org"), vec![me.clone()], 3),
        mail("3", Address::new(None, "ann@x.org"), vec![me.clone()], 5),
        mail("4", Address::new(None, "a_b@x.org"), vec![me.clone()], 2),
        mail("5", Address::new(None, "axb@x.org"), vec![me.clone()], 2),
    ]);
    let ann = s.suggest_addresses("lee", 8).unwrap();
    assert_eq!(ann.len(), 1, "one row per address, case-insensitive");
    assert_eq!(ann[0].name.as_deref(), Some("Ann Lee"), "newest name kept; a nameless newer message doesn't erase it");
    assert_eq!(emails(&s.suggest_addresses("a_", 8).unwrap()), ["a_b@x.org"], "_ is literal");
    assert!(s.suggest_addresses("%", 8).unwrap().is_empty(), "% is literal");
}

#[test]
fn migration_to_v3_learns_cached_mail() {
    let (mut s, _, inbox, sent) = setup();
    let me = Address::new(None, "tom@example.com");
    add(&mut s, inbox, vec![mail("1", Address::new(Some("Maya Chen"), "maya@example.com"), vec![me.clone()], 1)]);
    add(&mut s, sent, vec![mail("2", me, vec![Address::new(None, "nora@work.com")], 2)]);
    // Turn it back into a v2 database: no contacts table.
    s.conn.execute_batch("DROP TABLE contacts; PRAGMA user_version = 2;").unwrap();
    let s = Store::init(s.conn).unwrap();
    assert_eq!(emails(&s.suggest_addresses("ch", 8).unwrap()), ["maya@example.com"]);
    assert_eq!(emails(&s.suggest_addresses("nora", 8).unwrap()), ["nora@work.com"]);
}

#[test]
fn rejected_sends_leave_the_queue_but_are_kept() {
    let (mut s, acct, _, _) = setup();
    let a = s.outbox_push(acct, b"A", "tom@x.com", &["ann@x.com".into()]).unwrap();
    let b = s.outbox_push(acct, b"B", "tom@x.com", &["bob@x.com".into()]).unwrap();
    s.outbox_reject(a, "552 message too large").unwrap();
    let queued: Vec<i64> = s.outbox().unwrap().iter().map(|i| i.id).collect();
    assert_eq!(queued, [b], "refused messages aren't retried");
    let refused = s.outbox_rejected().unwrap();
    assert_eq!((refused[0].id, refused[0].last_error.as_deref()), (a, Some("552 message too large")));
    assert_eq!(s.outbox_item(a).unwrap().unwrap().raw, b"A");
    s.outbox_retry(a).unwrap();
    assert_eq!(s.outbox().unwrap().len(), 2);
    assert!(s.outbox_rejected().unwrap().is_empty());
    assert!(s.outbox_item(999).unwrap().is_none());
}

#[test]
fn migrates_v3_outbox_to_v4() {
    let (s, acct, _, _) = setup();
    // A v3 outbox: no `rejected` column.
    s.conn
        .execute_batch(
            "DROP TABLE outbox;
             CREATE TABLE outbox (id INTEGER PRIMARY KEY, account TEXT NOT NULL, raw BLOB NOT NULL,
                 envelope TEXT, created INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, last_error TEXT);
             PRAGMA user_version = 3;",
        )
        .unwrap();
    s.conn
        .execute("INSERT INTO outbox (account, raw, created) VALUES (?1, x'41', 1)", [acct.to_string()])
        .unwrap();
    let s = Store::init(s.conn).unwrap();
    assert_eq!(s.outbox().unwrap().len(), 1, "queued v3 rows stay queued");
    assert!(s.outbox_rejected().unwrap().is_empty());
}

#[test]
fn drafts_keep_versions_and_their_server_copy() {
    let (mut s, acct, inbox, _) = setup();
    s.draft_put("k1", acct, b"v1", Some("<v1@tern>")).unwrap();
    let d = s.draft("k1").unwrap().unwrap();
    assert_eq!((d.version, d.uploaded, d.remote.clone()), (1, false, None));
    assert_eq!(s.drafts_pending().unwrap().len(), 1);

    // Uploaded as version 1, but version 2 was saved meanwhile: still pending.
    s.draft_put("k1", acct, b"v2", Some("<v2@tern>")).unwrap();
    let copy = |rid: &str, mid: &str| ServerCopy { mailbox: inbox, remote_id: Some(rid.into()), message_id: Some(mid.into()) };
    s.draft_uploaded("k1", &copy("41", "<v1@tern>"), 1).unwrap();
    let d = s.draft("k1").unwrap().unwrap();
    assert_eq!((d.version, d.uploaded), (2, false));
    assert_eq!(d.remote, Some(copy("41", "<v1@tern>")));
    assert_eq!(d.raw, b"v2");

    s.draft_uploaded("k1", &copy("42", "<v2@tern>"), 2).unwrap();
    assert!(s.draft("k1").unwrap().unwrap().uploaded);
    assert!(s.drafts_pending().unwrap().is_empty());

    s.draft_delete("k1").unwrap();
    assert!(s.draft("k1").unwrap().is_none());
}

#[test]
fn finds_cached_messages_by_message_id() {
    let (mut s, _, inbox, _) = setup();
    add(&mut s, inbox, vec![env("7", "<draft-v3@tern>", None, "Plan", "Tom", 1)]);
    assert_eq!(s.find_by_message_id(inbox, "<draft-v3@tern>").unwrap().map(|(_, r)| r), Some("7".into()));
    assert!(s.find_by_message_id(inbox, "<nope@tern>").unwrap().is_none());
}

#[test]
fn gmail_copies_count_once() {
    // Gmail: every message is also in All Mail, starred ones in Starred too.
    let mut s = Store::open_in_memory().unwrap();
    let acct = AccountId::new();
    let mb = |rid: &str, role| RemoteMailbox { remote_id: rid.into(), name: rid.into(), parent_remote_id: None, role, selectable: true };
    s.sync_mailboxes(
        acct,
        &[
            mb("INBOX", MailboxRole::Inbox),
            mb("[Gmail]/All Mail", MailboxRole::All),
            mb("[Gmail]/Starred", MailboxRole::Flagged),
        ],
    )
    .unwrap();
    let id = |role| s.mailboxes(Some(acct)).unwrap().into_iter().find(|m| m.remote.role == role).unwrap().id;
    let (inbox, all, starred) = (id(MailboxRole::Inbox), id(MailboxRole::All), id(MailboxRole::Flagged));
    let flagged = |mut e: Envelope| {
        e.flags.flag = Some(FlagColor::Red);
        e
    };
    let add = |s: &mut Store, mb, added| s.apply_delta(mb, &MailboxDelta { added, ..Default::default() }).unwrap();
    add(&mut s, all, vec![
        flagged(env("10", "<a@x>", None, "Welcome", "Google", 1)),
        env("11", "<b@x>", Some("<a@x>"), "Re: Welcome", "Ann", 2),
    ]);
    add(&mut s, inbox, vec![
        flagged(env("1", "<a@x>", None, "Welcome", "Google", 1)),
        env("2", "<b@x>", Some("<a@x>"), "Re: Welcome", "Ann", 2),
    ]);
    add(&mut s, starred, vec![flagged(env("7", "<a@x>", None, "Welcome", "Google", 1))]);

    let rows = s.list(&ListScope::Mailbox(inbox), true, 50).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].thread_count, 2, "two messages, not their four copies");

    let thread = s.thread(rows[0].thread_id.unwrap()).unwrap();
    assert_eq!(thread.len(), 2);
    assert!(thread.iter().all(|m| m.mailbox == inbox), "the Inbox copy, not All Mail's");

    let flagged_rows = s.list(&ListScope::Flagged, false, 50).unwrap();
    assert_eq!(flagged_rows.len(), 1, "flagged once, not per mailbox");
    assert_eq!(flagged_rows[0].mailbox, inbox);

    let everything = s.list(&ListScope::Everything, false, 50).unwrap();
    assert_eq!(everything.len(), 2);
    let found = s.search(&SearchQuery::parse("welcome"), &ListScope::Everything, 50).unwrap();
    assert_eq!(found.len(), 2, "{found:?}");

    assert_eq!(s.count(&ListScope::Flagged).unwrap(), 1);
    assert_eq!(s.count(&ListScope::Unread).unwrap(), 2, "two unread messages, six unread copies");

    // All Mail on its own still lists its messages.
    assert_eq!(s.list(&ListScope::Mailbox(all), false, 50).unwrap().len(), 2);
}

#[test]
fn mailbox_counts_follow_the_server() {
    let (mut s, acct, inbox, _) = setup();
    let counts = |s: &Store| {
        let b = s.mailboxes(Some(acct)).unwrap().into_iter().find(|m| m.id == inbox).unwrap();
        (b.unread, b.total)
    };
    let unread = |rid: &str, hour| env(rid, &format!("<{rid}@x>"), None, "Hi", "Ann", hour);
    // The newest 2 of 5,000 messages are cached; the server says 4,832 are unread.
    let server = |total, unread| Some(MailboxCounts { total, unread });
    s.apply_delta(
        inbox,
        &MailboxDelta { added: vec![unread("9", 9), unread("10", 10)], full_resync: true, counts: server(5000, 4832), ..Default::default() },
    )
    .unwrap();
    assert_eq!(counts(&s), (4832, 5000));

    // Reading one in Tern lowers the badge before the server hears of it.
    let id = s.list(&ListScope::Mailbox(inbox), false, 10).unwrap()[0].id;
    s.set_flags(id, &Flags { seen: true, ..Default::default() }).unwrap();
    assert_eq!(counts(&s), (4831, 5000));

    // Older mail the server already counted doesn't change anything.
    s.apply_delta(inbox, &MailboxDelta { added: vec![unread("3", 3), unread("4", 4)], ..Default::default() })
        .unwrap();
    assert_eq!(counts(&s), (4831, 5000));

    // Deleting locally lowers both; the next sync's counts take over again.
    s.delete_local(&[id]).unwrap();
    assert_eq!(counts(&s), (4831, 4999));
    s.apply_delta(inbox, &MailboxDelta { counts: server(4999, 4830), ..Default::default() }).unwrap();
    assert_eq!(counts(&s), (4830, 4999));
}

#[test]
fn mailbox_counts_fall_back_to_the_cache() {
    let (mut s, acct, inbox, _) = setup();
    s.apply_delta(inbox, &MailboxDelta { added: vec![env("1", "<a@x>", None, "Hi", "Ann", 1)], ..Default::default() })
        .unwrap();
    let b = s.mailboxes(Some(acct)).unwrap().into_iter().find(|m| m.id == inbox).unwrap();
    assert_eq!((b.unread, b.total), (1, 1), "no server counts yet: the cache's");
}

#[test]
fn migrates_v5_mailboxes_to_v6() {
    let (s, acct, _, _) = setup();
    s.conn
        .execute_batch(
            "ALTER TABLE mailboxes DROP COLUMN server_total; ALTER TABLE mailboxes DROP COLUMN server_unread;
             ALTER TABLE mailboxes DROP COLUMN base_total; ALTER TABLE mailboxes DROP COLUMN base_unread;
             PRAGMA user_version = 5;",
        )
        .unwrap();
    let s = Store::init(s.conn).unwrap();
    assert_eq!(s.mailboxes(Some(acct)).unwrap().len(), 2);
}

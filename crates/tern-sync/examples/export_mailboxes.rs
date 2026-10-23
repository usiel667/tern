//! Copy whole mailboxes of a configured IMAP account to `.eml` files, read-only.
//!
//! ```sh
//! cargo run --release -p tern-sync --example export_mailboxes -- OUT_DIR "Mailbox" ["Mailbox"…]
//! ```
//! Uses the first IMAP account in accounts.toml (`TERN_EXPORT_ACCOUNT=address` picks
//! another) and its password from the keyring. Each mailbox becomes a folder of
//! `NNNNN-uid.eml` files; `index.csv` lists date, from, to and subject of everything.
//! The server is opened with EXAMINE and BODY.PEEK, so nothing there changes.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use tern_core::account::Transport;
use tern_core::mime::envelope_from_parts;
use tern_core::model::{Address, Flags};
use tern_imap::ImapBackend;
use tern_sync::Paths;

fn private_dir(p: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(p)
}

fn private_file(p: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(p)
}

/// A folder name from a mailbox path: `[Gmail]/Sent Mail` → `[Gmail] Sent Mail`.
fn folder_name(mailbox: &str) -> String {
    mailbox.chars().map(|c| if c == '/' || c == '\0' { ' ' } else { c }).collect()
}

fn csv(field: &str) -> String {
    format!("\"{}\"", field.replace('"', "\"\""))
}

fn addresses(a: &[Address]) -> String {
    a.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", ")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().ok_or("usage: export_mailboxes OUT_DIR MAILBOX…")?);
    let mailboxes: Vec<String> = args.collect();
    if mailboxes.is_empty() {
        return Err("name at least one mailbox".into());
    }
    let wanted = std::env::var("TERN_EXPORT_ACCOUNT").ok();
    let cfg = Paths::xdg()?
        .load_accounts()?
        .accounts
        .into_iter()
        .filter(|a| matches!(a.transport, Transport::Imap { .. }))
        .find(|a| wanted.as_deref().is_none_or(|w| a.identity.email.eq_ignore_ascii_case(w)))
        .ok_or("no matching IMAP account in accounts.toml")?;
    let Transport::Imap { imap, .. } = &cfg.transport else { unreachable!() };
    let backend = ImapBackend::new(imap.clone(), tern_auth::credential_for(&cfg).await?);

    private_dir(&out)?;
    let mut index = private_file(&out.join("index.csv"))?;
    writeln!(index, "mailbox,file,date,from,to,cc,subject")?;
    for mailbox in &mailboxes {
        let dir = out.join(folder_name(mailbox));
        private_dir(&dir)?;
        let mut n = 0usize;
        let mut bytes = 0usize;
        let count = backend
            .export_raw(mailbox, 50, |uid, raw| {
                n += 1;
                bytes += raw.len();
                let file = format!("{n:05}-{uid}.eml");
                let write = |f: &mut fs::File| f.write_all(raw);
                private_file(&dir.join(&file)).and_then(|mut f| write(&mut f)).map_err(|e| tern_core::Error::Storage(e.to_string()))?;
                let e = envelope_from_parts(uid.to_string(), raw, None, Flags::default(), raw.len() as u32, None);
                writeln!(
                    index,
                    "{},{},{},{},{},{},{}",
                    csv(mailbox),
                    csv(&format!("{}/{file}", folder_name(mailbox))),
                    e.date.format("%Y-%m-%d %H:%M"),
                    csv(&addresses(&e.from)),
                    csv(&addresses(&e.to)),
                    csv(&addresses(&e.cc)),
                    csv(&e.subject)
                )
                .map_err(|e| tern_core::Error::Storage(e.to_string()))
            })
            .await?;
        println!("{mailbox}: {n} of {count} messages, {:.1} MB", bytes as f64 / 1e6);
    }
    println!("done: {}", out.display());
    Ok(())
}

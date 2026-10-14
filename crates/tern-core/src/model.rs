use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable local id for an account (never sent to a server).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AccountId(pub Uuid);

impl AccountId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for AccountId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AccountId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Local row id of a mailbox in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MailboxId(pub i64);

/// Local row id of a message in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MessageId(pub i64);

/// What a mailbox is *for*. Mirrors IMAP SPECIAL-USE (RFC 6154) and JMAP roles,
/// which is also how Apple Mail decides what goes in the sidebar's top section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MailboxRole {
    Inbox,
    Drafts,
    Sent,
    Junk,
    Trash,
    Archive,
    /// Gmail "All Mail".
    All,
    /// Server-side flagged view (Gmail "Starred").
    Flagged,
    Important,
    /// Ordinary user folder.
    Custom,
}

impl MailboxRole {
    /// Sort key so the sidebar reads Inbox, Drafts, Sent, Junk, Trash, Archive, then folders.
    pub fn sidebar_order(self) -> u8 {
        match self {
            Self::Inbox => 0,
            Self::Flagged => 1,
            Self::Drafts => 2,
            Self::Sent => 3,
            Self::Junk => 4,
            Self::Trash => 5,
            Self::Archive => 6,
            Self::All => 7,
            Self::Important => 8,
            Self::Custom => 9,
        }
    }

    pub fn icon_name(self) -> &'static str {
        match self {
            Self::Inbox => "tern-inbox-symbolic",
            Self::Drafts => "document-edit-symbolic",
            Self::Sent => "mail-send-symbolic",
            Self::Junk => "mail-mark-junk-symbolic",
            Self::Trash => "user-trash-symbolic",
            Self::Archive => "tern-archive-symbolic",
            Self::All => "tern-inbox-symbolic",
            Self::Flagged => "starred-symbolic",
            Self::Important => "emblem-important-symbolic",
            Self::Custom => "folder-symbolic",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Drafts => "drafts",
            Self::Sent => "sent",
            Self::Junk => "junk",
            Self::Trash => "trash",
            Self::Archive => "archive",
            Self::All => "all",
            Self::Flagged => "flagged",
            Self::Important => "important",
            Self::Custom => "custom",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "inbox" => Self::Inbox,
            "drafts" => Self::Drafts,
            "sent" => Self::Sent,
            "junk" | "spam" => Self::Junk,
            "trash" => Self::Trash,
            "archive" => Self::Archive,
            "all" => Self::All,
            "flagged" => Self::Flagged,
            "important" => Self::Important,
            _ => Self::Custom,
        }
    }
}

/// A mailbox as reported by the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteMailbox {
    /// Server-side identifier: full IMAP path or JMAP mailbox id.
    pub remote_id: String,
    /// Human name (last path component for IMAP).
    pub name: String,
    pub parent_remote_id: Option<String>,
    pub role: MailboxRole,
    /// Some IMAP folders exist only as hierarchy nodes (\Noselect).
    pub selectable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mailbox {
    pub id: MailboxId,
    pub account: AccountId,
    pub remote: RemoteMailbox,
    pub unread: u32,
    pub total: u32,
}

/// Apple Mail's seven flag colours. IMAP stores them as the de-facto
/// `$MailFlagBit0..2` keywords alongside `\Flagged`, which we round-trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FlagColor {
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Gray,
}

impl FlagColor {
    pub const ALL: [FlagColor; 7] = [
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Blue,
        Self::Purple,
        Self::Gray,
    ];

    /// Bit pattern used by Apple Mail in `$MailFlagBit0`, `$MailFlagBit1`, `$MailFlagBit2`.
    pub fn bits(self) -> u8 {
        match self {
            Self::Red => 0,
            Self::Orange => 1,
            Self::Yellow => 2,
            Self::Green => 3,
            Self::Blue => 4,
            Self::Purple => 5,
            Self::Gray => 6,
        }
    }

    pub fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Orange,
            2 => Self::Yellow,
            3 => Self::Green,
            4 => Self::Blue,
            5 => Self::Purple,
            6 => Self::Gray,
            _ => Self::Red,
        }
    }

    pub fn css_class(self) -> &'static str {
        match self {
            Self::Red => "flag-red",
            Self::Orange => "flag-orange",
            Self::Yellow => "flag-yellow",
            Self::Green => "flag-green",
            Self::Blue => "flag-blue",
            Self::Purple => "flag-purple",
            Self::Gray => "flag-gray",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flags {
    pub seen: bool,
    pub answered: bool,
    pub forwarded: bool,
    pub draft: bool,
    pub deleted: bool,
    pub junk: bool,
    /// `Some` means flagged; the colour defaults to red like Apple Mail.
    pub flag: Option<FlagColor>,
}

impl Flags {
    /// Build from IMAP flag/keyword strings (`\Seen`, `$Forwarded`, `$MailFlagBit1`, ...).
    pub fn from_imap<'a>(items: impl IntoIterator<Item = &'a str>) -> Self {
        let mut f = Flags::default();
        let mut flagged = false;
        let mut bits = 0u8;
        for item in items {
            match item.to_ascii_lowercase().as_str() {
                "\\seen" => f.seen = true,
                "\\answered" => f.answered = true,
                "\\draft" => f.draft = true,
                "\\deleted" => f.deleted = true,
                "\\flagged" => flagged = true,
                "$forwarded" => f.forwarded = true,
                "$junk" | "junk" => f.junk = true,
                "$mailflagbit0" => bits |= 1,
                "$mailflagbit1" => bits |= 2,
                "$mailflagbit2" => bits |= 4,
                _ => {}
            }
        }
        if flagged {
            f.flag = Some(FlagColor::from_bits(bits));
        }
        f
    }

    /// The full IMAP flag set to `STORE FLAGS.SILENT` (system flags + keywords we own).
    pub fn to_imap(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.seen {
            out.push("\\Seen");
        }
        if self.answered {
            out.push("\\Answered");
        }
        if self.draft {
            out.push("\\Draft");
        }
        if self.deleted {
            out.push("\\Deleted");
        }
        if self.forwarded {
            out.push("$Forwarded");
        }
        if self.junk {
            out.push("$Junk");
        }
        if let Some(color) = self.flag {
            out.push("\\Flagged");
            let b = color.bits();
            if b & 1 != 0 {
                out.push("$MailFlagBit0");
            }
            if b & 2 != 0 {
                out.push("$MailFlagBit1");
            }
            if b & 4 != 0 {
                out.push("$MailFlagBit2");
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Address {
    pub name: Option<String>,
    pub email: String,
}

impl Address {
    pub fn new(name: Option<&str>, email: &str) -> Self {
        Self {
            name: name.map(str::to_owned).filter(|n| !n.trim().is_empty()),
            email: email.to_owned(),
        }
    }

    /// What the message list shows: the display name, falling back to the address.
    pub fn display(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.email)
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.name {
            Some(n) => write!(f, "\"{}\" <{}>", n.replace('"', "\\\""), self.email),
            None => write!(f, "{}", self.email),
        }
    }
}

/// Everything needed to draw a row in the message list and to thread it,
/// without downloading the body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// IMAP UID (as string) or JMAP email id.
    pub remote_id: String,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub subject: String,
    pub from: Vec<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub date: DateTime<Utc>,
    pub flags: Flags,
    pub size: u32,
    pub has_attachments: bool,
    /// First ~200 chars of text, for the two-line preview.
    pub preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageSummary {
    pub id: MessageId,
    pub account: AccountId,
    pub mailbox: MailboxId,
    pub thread_id: Option<i64>,
    pub envelope: Envelope,
    /// Number of messages in the conversation (1 when unthreaded).
    pub thread_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub filename: Option<String>,
    pub mime_type: String,
    pub size: u64,
    /// `cid:` for inline images.
    pub content_id: Option<String>,
    pub inline: bool,
    /// Index into the parsed message's parts, for lazily extracting bytes.
    pub part_index: u32,
}

/// The fully downloaded message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageBody {
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachments: Vec<Attachment>,
    pub list_unsubscribe: Option<String>,
    pub list_unsubscribe_post: bool,
    /// Raw RFC 5322 bytes, kept so we can re-parse, forward-as-attachment, or "View Source".
    #[serde(skip)]
    pub raw: Vec<u8>,
}

/// An outgoing message as edited in the compose window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub from: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub subject: String,
    pub text: String,
    pub html: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub attachments: Vec<OutgoingAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutgoingAttachment {
    pub filename: String,
    pub mime_type: String,
    #[serde(skip)]
    pub data: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_flag_colors_round_trip_through_imap_keywords() {
        for color in FlagColor::ALL {
            let flags = Flags {
                flag: Some(color),
                seen: true,
                ..Default::default()
            };
            let parsed = Flags::from_imap(flags.to_imap());
            assert_eq!(parsed, flags, "{color:?}");
        }
    }

    #[test]
    fn plain_flagged_is_red() {
        let f = Flags::from_imap(["\\Flagged"]);
        assert_eq!(f.flag, Some(FlagColor::Red));
    }

    #[test]
    fn address_display_falls_back_to_email() {
        assert_eq!(Address::new(Some("  "), "a@b.c").display(), "a@b.c");
        assert_eq!(Address::new(Some("Ann"), "a@b.c").display(), "Ann");
    }
}

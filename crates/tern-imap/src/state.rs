//! The IMAP sync cursor stored in `SyncState`.

/// `v2:<uidvalidity>:<low>:<high>:<modseq>` — the UID window we cache plus the
/// mailbox's HIGHESTMODSEQ when it was last synced (CONDSTORE, RFC 7162). `low`
/// lets us check flags only for messages we actually have, not a 100k-message
/// archive. `modseq` 0 means unknown: v1 cursors, or servers without CONDSTORE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImapState {
    pub uid_validity: u32,
    pub low_uid: u32,
    pub high_uid: u32,
    pub modseq: u64,
}

impl ImapState {
    pub fn encode(&self) -> String {
        format!("v2:{}:{}:{}:{}", self.uid_validity, self.low_uid, self.high_uid, self.modseq)
    }

    /// Also reads milestone-1 `v1:<uidvalidity>:<low>:<high>` cursors (modseq unknown).
    pub fn decode(s: &str) -> Option<Self> {
        let mut it = s.split(':');
        let version = it.next()?;
        if version != "v1" && version != "v2" {
            return None;
        }
        let state = Self {
            uid_validity: it.next()?.parse().ok()?,
            low_uid: it.next()?.parse().ok()?,
            high_uid: it.next()?.parse().ok()?,
            modseq: if version == "v2" { it.next()?.parse().ok()? } else { 0 },
        };
        it.next().is_none().then_some(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = ImapState { uid_validity: 7, low_uid: 100, high_uid: 250, modseq: 90060115205545359 };
        assert_eq!(ImapState::decode(&s.encode()), Some(s));
    }

    #[test]
    fn reads_v1_cursors_with_unknown_modseq() {
        let s = ImapState::decode("v1:7:100:250").unwrap();
        assert_eq!((s.uid_validity, s.low_uid, s.high_uid, s.modseq), (7, 100, 250, 0));
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["garbage", "", "v3:1:2:3:4", "v2:1:2:3", "v1:1:2:x", "v2:1:2:3:4:5", "demo:123"] {
            assert_eq!(ImapState::decode(bad), None, "{bad}");
        }
    }
}

//! The IMAP sync cursor stored in `SyncState`.

/// `v3:<uidvalidity>:<low>:<high>:<modseq>:<uidnext>:<exists>` — the UID window we
/// cache plus the mailbox's HIGHESTMODSEQ, UIDNEXT and message count when it was last
/// synced (CONDSTORE, RFC 7162). `low` lets us check flags only for messages we
/// actually have, not a 100k-message archive. `modseq` 0 means unknown: v1 cursors, or
/// servers without CONDSTORE. `uid_next` 0 means `uid_next` and `exists` are unknown
/// (v1/v2 cursors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImapState {
    pub uid_validity: u32,
    pub low_uid: u32,
    pub high_uid: u32,
    pub modseq: u64,
    pub uid_next: u32,
    pub exists: u32,
}

impl ImapState {
    pub fn encode(&self) -> String {
        format!(
            "v3:{}:{}:{}:{}:{}:{}",
            self.uid_validity, self.low_uid, self.high_uid, self.modseq, self.uid_next, self.exists
        )
    }

    /// Also reads older cursors: `v1:<uidvalidity>:<low>:<high>` (modseq unknown) and
    /// `v2:…:<modseq>` (UIDNEXT and count unknown).
    pub fn decode(s: &str) -> Option<Self> {
        let mut it = s.split(':');
        let version = match it.next()? {
            "v1" => 1,
            "v2" => 2,
            "v3" => 3,
            _ => return None,
        };
        let state = Self {
            uid_validity: it.next()?.parse().ok()?,
            low_uid: it.next()?.parse().ok()?,
            high_uid: it.next()?.parse().ok()?,
            modseq: if version >= 2 { it.next()?.parse().ok()? } else { 0 },
            uid_next: if version >= 3 { it.next()?.parse().ok()? } else { 0 },
            exists: if version >= 3 { it.next()?.parse().ok()? } else { 0 },
        };
        it.next().is_none().then_some(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = ImapState {
            uid_validity: 7,
            low_uid: 100,
            high_uid: 250,
            modseq: 90060115205545359,
            uid_next: 251,
            exists: 4000,
        };
        assert_eq!(ImapState::decode(&s.encode()), Some(s));
    }

    #[test]
    fn reads_v1_cursors_with_unknown_modseq() {
        let s = ImapState::decode("v1:7:100:250").unwrap();
        assert_eq!((s.uid_validity, s.low_uid, s.high_uid, s.modseq, s.uid_next), (7, 100, 250, 0, 0));
    }

    #[test]
    fn reads_v2_cursors_with_unknown_uid_next() {
        let s = ImapState::decode("v2:7:100:250:90").unwrap();
        assert_eq!((s.modseq, s.uid_next, s.exists), (90, 0, 0));
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["garbage", "", "v4:1:2:3:4", "v2:1:2:3", "v1:1:2:x", "v2:1:2:3:4:5", "v3:1:2:3:4:5", "demo:123"] {
            assert_eq!(ImapState::decode(bad), None, "{bad}");
        }
    }
}

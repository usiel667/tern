//! The IMAP sync cursor stored in `SyncState`.

/// `v1:<uidvalidity>:<low>:<high>` — the UID window we cache. `low` lets us
/// check flags only for messages we actually have, not a 100k-message archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImapState {
    pub uid_validity: u32,
    pub low_uid: u32,
    pub high_uid: u32,
}

impl ImapState {
    pub fn encode(&self) -> String {
        format!("v1:{}:{}:{}", self.uid_validity, self.low_uid, self.high_uid)
    }

    pub fn decode(s: &str) -> Option<Self> {
        let mut it = s.split(':');
        if it.next()? != "v1" {
            return None;
        }
        Some(Self {
            uid_validity: it.next()?.parse().ok()?,
            low_uid: it.next()?.parse().ok()?,
            high_uid: it.next()?.parse().ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = ImapState { uid_validity: 7, low_uid: 100, high_uid: 250 };
        assert_eq!(ImapState::decode(&s.encode()), Some(s));
        assert_eq!(ImapState::decode("garbage"), None);
    }
}

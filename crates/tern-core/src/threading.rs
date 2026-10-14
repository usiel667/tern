//! Conversation grouping, as in Apple Mail's "Organize by Conversation".
//!
//! Apple Mail shows conversations flat (sorted by date), not as a reply tree, so
//! we only need *which* messages belong together. That is a connected-components
//! problem over Message-ID / In-Reply-To / References, solved here with
//! union-find, followed by the JWZ subject-merge heuristic for clients that drop
//! the reference headers.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

/// Minimal view of a message for threading.
#[derive(Debug, Clone)]
pub struct ThreadInput<'a> {
    pub message_id: Option<&'a str>,
    pub in_reply_to: Option<&'a str>,
    pub references: &'a [String],
    pub subject: &'a str,
    pub date: DateTime<Utc>,
}

/// Returns one group per conversation: indices into `msgs`, sorted oldest
/// first. Groups are sorted newest-activity first (what the list shows).
pub fn group_conversations(msgs: &[ThreadInput<'_>]) -> Vec<Vec<usize>> {
    let mut uf = UnionFind::default();
    // Node per message index, plus nodes for every referenced id.
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut msg_node = Vec::with_capacity(msgs.len());

    for m in msgs {
        let node = match m.message_id.map(normalize_id) {
            Some(id) if !id.is_empty() => *by_id.entry(id).or_insert_with(|| uf.add()),
            _ => uf.add(),
        };
        msg_node.push(node);
    }

    for (i, m) in msgs.iter().enumerate() {
        let me = msg_node[i];
        let refs = m
            .references
            .iter()
            .map(String::as_str)
            .chain(m.in_reply_to);
        for r in refs {
            let id = normalize_id(r);
            if id.is_empty() {
                continue;
            }
            let other = *by_id.entry(id).or_insert_with(|| uf.add());
            uf.union(me, other);
        }
    }

    // Collect components.
    let mut comps: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, &node) in msg_node.iter().enumerate() {
        comps.entry(uf.find(node)).or_default().push(i);
    }
    let mut groups: Vec<Vec<usize>> = comps.into_values().collect();
    for g in &mut groups {
        g.sort_by_key(|&i| (msgs[i].date, i));
    }

    // Subject merge: a group whose first message is a reply ("Re: Lunch") joins
    // the group that started the subject ("Lunch") if one exists.
    // The reply attaches to the most recent starter that predates it.
    let mut starters: HashMap<String, Vec<usize>> = HashMap::new();
    for (gi, g) in groups.iter().enumerate() {
        let (base, is_reply) = base_subject(msgs[g[0]].subject);
        if !is_reply && !base.is_empty() {
            starters.entry(base).or_default().push(gi);
        }
    }
    let mut merge_into: Vec<Option<usize>> = vec![None; groups.len()];
    for (gi, g) in groups.iter().enumerate() {
        let first = &msgs[g[0]];
        let (base, is_reply) = base_subject(first.subject);
        if !is_reply {
            continue;
        }
        merge_into[gi] = starters.get(&base).and_then(|cands| {
            cands
                .iter()
                .copied()
                .filter(|&c| c != gi && msgs[groups[c][0]].date <= first.date)
                .max_by_key(|&c| msgs[groups[c][0]].date)
        });
    }
    let mut merged: Vec<Vec<usize>> = Vec::new();
    let mut slot: Vec<Option<usize>> = vec![None; groups.len()];
    for gi in 0..groups.len() {
        if merge_into[gi].is_none() {
            slot[gi] = Some(merged.len());
            merged.push(groups[gi].clone());
        }
    }
    for gi in 0..groups.len() {
        if let Some(t) = merge_into[gi] {
            let s = slot[t].expect("target is never itself merged");
            merged[s].extend_from_slice(&groups[gi]);
        }
    }
    for g in &mut merged {
        g.sort_by_key(|&i| (msgs[i].date, i));
    }
    merged.sort_by(|a, b| {
        let la = msgs[*a.last().unwrap()].date;
        let lb = msgs[*b.last().unwrap()].date;
        lb.cmp(&la)
    });
    merged
}

/// Strip `Re:`, `Fwd:`, `AW:`, `[list]` tags, etc. Returns the lowercase base
/// subject and whether any reply/forward prefix was removed.
pub fn base_subject(subject: &str) -> (String, bool) {
    const PREFIXES: &[&str] = &[
        "re", "fw", "fwd", "aw", "wg", "sv", "vs", "antw", "rif", "r", "tr", "odp",
    ];
    let mut s = subject.trim();
    let mut stripped = false;
    loop {
        let before = s;
        // "[list-name] " tags.
        if s.starts_with('[')
            && let Some(end) = s.find(']') {
                s = s[end + 1..].trim_start();
            }
        // "Re:", "RE[2]:", "Fwd :".
        if let Some(colon) = s.find(':') {
            let head = s[..colon].trim();
            let word = head.split('[').next().unwrap_or(head).trim();
            if PREFIXES.iter().any(|p| word.eq_ignore_ascii_case(p)) {
                s = s[colon + 1..].trim_start();
                stripped = true;
            }
        }
        if s == before {
            break;
        }
    }
    (s.to_lowercase(), stripped)
}

fn normalize_id(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_ascii_lowercase()
}

#[derive(Default)]
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl UnionFind {
    fn add(&mut self) -> usize {
        self.parent.push(self.parent.len());
        self.rank.push(0);
        self.parent.len() - 1
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        match self.rank[ra].cmp(&self.rank[rb]) {
            std::cmp::Ordering::Less => self.parent[ra] = rb,
            std::cmp::Ordering::Greater => self.parent[rb] = ra,
            std::cmp::Ordering::Equal => {
                self.parent[rb] = ra;
                self.rank[ra] += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 1, h, 0, 0).unwrap()
    }

    struct M {
        id: Option<&'static str>,
        irt: Option<&'static str>,
        refs: Vec<String>,
        subj: &'static str,
        h: u32,
    }

    fn m(id: Option<&'static str>, irt: Option<&'static str>, refs: &[&str], subj: &'static str, h: u32) -> M {
        M { id, irt, refs: refs.iter().map(|s| s.to_string()).collect(), subj, h }
    }

    fn run(ms: &[M]) -> Vec<Vec<usize>> {
        let inputs: Vec<_> = ms
            .iter()
            .map(|m| ThreadInput {
                message_id: m.id,
                in_reply_to: m.irt,
                references: &m.refs,
                subject: m.subj,
                date: t(m.h),
            })
            .collect();
        group_conversations(&inputs)
    }

    #[test]
    fn groups_by_references() {
        let g = run(&[
            m(Some("<a@x>"), None, &[], "Lunch", 1),
            m(Some("<b@x>"), Some("<a@x>"), &["<a@x>"], "Re: Lunch", 2),
            m(Some("<c@x>"), None, &[], "Other", 3),
            m(Some("<d@x>"), Some("<b@x>"), &["<a@x>", "<b@x>"], "Re: Lunch", 4),
        ]);
        assert_eq!(g, vec![vec![0, 1, 3], vec![2]]);
    }

    #[test]
    fn joins_via_missing_parent() {
        // Two replies to a message we don't have still belong together.
        let g = run(&[
            m(Some("<b@x>"), Some("<a@x>"), &[], "Re: Plan", 1),
            m(Some("<c@x>"), Some("<a@x>"), &[], "Re: Plan", 2),
        ]);
        assert_eq!(g, vec![vec![0, 1]]);
    }

    #[test]
    fn subject_merge_for_broken_clients() {
        let g = run(&[
            m(Some("<a@x>"), None, &[], "Budget", 1),
            m(Some("<z@y>"), None, &[], "RE: [team] Budget", 2),
            m(Some("<q@y>"), None, &[], "Budget", 0), // older unrelated starter loses
        ]);
        assert_eq!(g.len(), 2);
        assert!(g.contains(&vec![0, 1]));
    }

    #[test]
    fn non_reply_same_subject_stays_separate() {
        let g = run(&[
            m(Some("<a@x>"), None, &[], "Hello", 1),
            m(Some("<b@x>"), None, &[], "Hello", 2),
        ]);
        assert_eq!(g.len(), 2);
    }

    #[test]
    fn base_subject_strips_prefixes() {
        assert_eq!(base_subject("Re: Fwd: AW: [list] Hi"), ("hi".into(), true));
        assert_eq!(base_subject("RE[3]: Hi"), ("hi".into(), true));
        assert_eq!(base_subject("Meeting: notes"), ("meeting: notes".into(), false));
    }
}

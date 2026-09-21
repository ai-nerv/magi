//! What fits in one frame: a frame past the wire's limit is refused whole, closing the connection.

use magi_proto::Entry;

/// What the entries in one snapshot may weigh.
pub(super) const SNAPSHOT_BUDGET: usize = 8 * 1024 * 1024;

/// Roughly what an entry costs on the wire.
pub(super) fn weight(entry: &Entry) -> usize {
    serde_json::to_vec(entry).map_or(0, |bytes| bytes.len())
}

/// The newest entries that fit, oldest first; at least one always comes back.
pub(super) fn newest_within<'a>(
    entries: impl Iterator<Item = &'a Entry>,
    budget: usize,
) -> Vec<Entry> {
    let seen: Vec<&Entry> = entries.collect();
    let mut kept: Vec<Entry> = Vec::new();
    let mut held = 0;
    for entry in seen.iter().rev() {
        held += weight(entry);
        if held > budget && !kept.is_empty() {
            break;
        }
        kept.push((*entry).clone());
    }
    kept.reverse();
    kept
}

#[cfg(test)]
mod tests {
    use super::{SNAPSHOT_BUDGET, newest_within, weight};
    use magi_proto::{Entry, MessageId};

    fn user(text: &str) -> Entry {
        Entry::User {
            id: MessageId::new(text),
            text: text.to_owned(),
            aside: String::new(),
        }
    }

    #[test]
    fn a_snapshot_stays_inside_the_frame_the_wire_allows() {
        let big = "x".repeat(200_000);
        let entries: Vec<Entry> = (0..60).map(|_| user(&big)).collect();
        let kept = newest_within(entries.iter(), SNAPSHOT_BUDGET);
        let held: usize = kept.iter().map(weight).sum();
        assert!(held <= SNAPSHOT_BUDGET, "{held} bytes");
        assert!(!kept.is_empty(), "nothing at all came back");
    }

    #[test]
    fn one_entry_past_the_budget_still_comes_back() {
        let huge = user(&"x".repeat(32));
        let kept = newest_within(std::iter::once(&huge), 1);
        assert_eq!(kept.len(), 1);
    }
}

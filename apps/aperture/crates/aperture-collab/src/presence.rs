//! presence — where everyone else's cursor is.
//!
//! ## The trick RGA hands you
//!
//! A caret stored as a char index has to be shifted by every remote edit
//! that lands before it. That is a second correctness problem the size
//! of the first one, with the same concurrency hazards and none of the
//! CRDT's guarantees — and it fails in a way users notice immediately,
//! because a collaborator's cursor drifts into the wrong word.
//!
//! A caret stored as a character ID needs no transformation at all. The
//! character it points at keeps its identity no matter what lands around
//! it, so resolving a caret to a screen position is a lookup, done at
//! render time against whatever the document currently is.
//!
//! So presence here is `Option<CharId>` twice — anchor and head — and
//! the whole update path for a remote edit is: nothing.
//!
//! ## Deleted anchors
//!
//! If the character a caret points at is deleted, the id is still valid
//! (tombstones are kept) and [`Rga::index_after`] gives the position its
//! surviving neighbour now occupies. That is the behaviour a person
//! expects: your collaborator's cursor stays where the text used to be,
//! rather than jumping to the start.
//!
//! ## What is deliberately not here
//!
//! Liveness. A peer that stops sending is still listed, because "has
//! this person left" is a transport question — studio-collab's session
//! layer already answers it — and a CRDT module inventing its own
//! timeout would give two different answers to one question.

use crate::rga::{CharId, Rga, SiteId};
use std::collections::BTreeMap;

/// One participant's selection.
///
/// `None` means the start of the document: a caret at index 0 has no
/// character before it to anchor to, and anchoring to the character
/// AFTER would move the caret whenever someone types in front of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Caret {
    /// The character the selection starts after.
    pub anchor: Option<CharId>,
    /// The character the caret sits after. Equal to `anchor` for a bare
    /// caret with no selection.
    pub head: Option<CharId>,
}

impl Caret {
    pub fn at(id: Option<CharId>) -> Self {
        Caret { anchor: id, head: id }
    }
    pub fn is_caret(&self) -> bool {
        self.anchor == self.head
    }
}

/// A caret resolved against the current document, in visible char
/// indices — what a renderer draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub site: SiteId,
    pub anchor: usize,
    pub head: usize,
}

impl Resolved {
    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }
    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }
    pub fn is_caret(&self) -> bool {
        self.anchor == self.head
    }
}

/// Everyone's carets, including our own.
#[derive(Debug, Clone, Default)]
pub struct Presence {
    carets: BTreeMap<SiteId, Caret>,
}

impl Presence {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record where a site's caret is. Last write wins, which is right
    /// here for the reason it is wrong for text: a caret has exactly one
    /// current position and no merge of two positions is meaningful.
    pub fn set(&mut self, site: SiteId, caret: Caret) {
        self.carets.insert(site, caret);
    }

    /// Forget a participant. The transport decides when.
    pub fn remove(&mut self, site: SiteId) -> bool {
        self.carets.remove(&site).is_some()
    }

    pub fn get(&self, site: SiteId) -> Option<Caret> {
        self.carets.get(&site).copied()
    }

    pub fn len(&self) -> usize {
        self.carets.len()
    }
    pub fn is_empty(&self) -> bool {
        self.carets.is_empty()
    }

    /// Resolve every caret against the document, skipping `exclude`
    /// (normally the local site, which the editor draws itself).
    ///
    /// Ordered by site id so a renderer assigning colours by position
    /// gets a stable answer frame to frame.
    pub fn resolve_all(&self, doc: &Rga, exclude: Option<SiteId>) -> Vec<Resolved> {
        self.carets
            .iter()
            .filter(|(s, _)| Some(**s) != exclude)
            .filter_map(|(s, c)| {
                Some(Resolved {
                    site: *s,
                    anchor: resolve_one(doc, c.anchor)?,
                    head: resolve_one(doc, c.head)?,
                })
            })
            .collect()
    }
}

/// A caret id → a visible index.
///
/// `None` is the document start. A live character resolves to just after
/// itself; a tombstoned one resolves to where its surviving neighbour
/// now sits, so a collaborator's cursor stays put when the text under it
/// is deleted instead of snapping to the top of the file.
///
/// An id this replica has never seen resolves to nothing rather than to
/// zero: drawing a cursor at the wrong place is worse than not drawing
/// it for the moment it takes the op to arrive.
fn resolve_one(doc: &Rga, id: Option<CharId>) -> Option<usize> {
    match id {
        None => Some(0),
        Some(i) => doc.index_after(i),
    }
}

/// Build a caret for a visible index in `doc` — the conversion the local
/// editor does before broadcasting, since its own cursor is an index.
pub fn caret_at_index(doc: &Rga, at: usize) -> Caret {
    Caret::at(if at == 0 { None } else { doc.id_at(at - 1) })
}

/// Build a selection from two visible indices.
pub fn selection(doc: &Rga, anchor: usize, head: usize) -> Caret {
    Caret {
        anchor: if anchor == 0 { None } else { doc.id_at(anchor - 1) },
        head: if head == 0 { None } else { doc.id_at(head - 1) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SiteId = SiteId(1);
    const B: SiteId = SiteId(2);

    #[test]
    fn a_caret_at_the_start_anchors_to_nothing() {
        let doc = Rga::from_text("abc", A);
        let c = caret_at_index(&doc, 0);
        assert_eq!(c.anchor, None);
        // and anchoring to the character AFTER would be wrong: it would
        // move when someone types in front
        assert_eq!(resolve_one(&doc, c.anchor), Some(0));
    }

    #[test]
    fn a_remote_edit_before_the_caret_needs_no_transformation() {
        let mut doc = Rga::from_text("hello", A);
        let mut p = Presence::new();
        p.set(B, caret_at_index(&doc, 5)); // B is at the end
        assert_eq!(p.resolve_all(&doc, None)[0].head, 5);
        // A types at the front; nothing touches the caret
        doc.local_insert(0, ">>> ", A);
        assert_eq!(doc.text(), ">>> hello");
        assert_eq!(p.resolve_all(&doc, None)[0].head, 9, "the caret followed its character");
    }

    #[test]
    fn a_remote_edit_after_the_caret_leaves_it_alone() {
        let mut doc = Rga::from_text("hello", A);
        let mut p = Presence::new();
        p.set(B, caret_at_index(&doc, 2));
        doc.local_insert(5, "!!!", A);
        assert_eq!(p.resolve_all(&doc, None)[0].head, 2);
    }

    #[test]
    fn a_caret_on_deleted_text_stays_where_the_text_was() {
        let mut doc = Rga::from_text("abcdef", A);
        let mut p = Presence::new();
        p.set(B, caret_at_index(&doc, 4)); // after 'd'
        doc.local_delete(3, 1); // remove 'd'
        assert_eq!(doc.text(), "abcef");
        // not 0, and not the end: where 'd' used to be
        assert_eq!(p.resolve_all(&doc, None)[0].head, 3);
    }

    #[test]
    fn a_selection_survives_an_insert_inside_it() {
        let mut doc = Rga::from_text("abcdef", A);
        let mut p = Presence::new();
        p.set(B, selection(&doc, 1, 4)); // covers "bcd"
        doc.local_insert(2, "XX", A);
        assert_eq!(doc.text(), "abXXcdef");
        let r = p.resolve_all(&doc, None)[0];
        assert_eq!((r.start(), r.end()), (1, 6), "the selection grew with its contents");
        assert!(!r.is_caret());
    }

    #[test]
    fn an_unknown_id_is_not_drawn_at_zero() {
        let doc = Rga::from_text("abc", A);
        let mut p = Presence::new();
        p.set(B, Caret::at(Some(CharId::new(999, 9))));
        assert!(
            p.resolve_all(&doc, None).is_empty(),
            "better no cursor for one frame than a cursor in the wrong place"
        );
    }

    #[test]
    fn the_local_site_can_be_excluded() {
        let doc = Rga::from_text("abc", A);
        let mut p = Presence::new();
        p.set(A, caret_at_index(&doc, 1));
        p.set(B, caret_at_index(&doc, 2));
        assert_eq!(p.resolve_all(&doc, Some(A)).len(), 1);
        assert_eq!(p.resolve_all(&doc, Some(A))[0].site, B);
        assert_eq!(p.resolve_all(&doc, None).len(), 2);
    }

    #[test]
    fn resolution_order_is_stable() {
        let doc = Rga::from_text("abcdef", A);
        let mut p = Presence::new();
        for s in [5u64, 1, 3] {
            p.set(SiteId(s), caret_at_index(&doc, 1));
        }
        let sites: Vec<u64> = p.resolve_all(&doc, None).iter().map(|r| r.site.0).collect();
        assert_eq!(sites, vec![1, 3, 5], "a renderer colouring by position needs this");
    }

    #[test]
    fn a_caret_is_last_write_wins_per_site() {
        let doc = Rga::from_text("abc", A);
        let mut p = Presence::new();
        p.set(B, caret_at_index(&doc, 1));
        p.set(B, caret_at_index(&doc, 3));
        assert_eq!(p.len(), 1);
        assert_eq!(p.resolve_all(&doc, None)[0].head, 3);
        assert!(p.remove(B));
        assert!(!p.remove(B));
        assert!(p.is_empty());
    }
}

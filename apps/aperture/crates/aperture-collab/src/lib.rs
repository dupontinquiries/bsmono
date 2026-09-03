//! aperture-collab — several people editing one aperture buffer, with
//! no server deciding who wins.
//!
//! See [`rga`] for why this is a CRDT and not last-writer-wins or OT.
//! See [`presence`] for why carets anchor to character ids.
//!
//! ## The bridge to aperture-core, and why it is a feature
//!
//! [`TextEdit`] is structurally identical to `aperture_core::Op`. The
//! duplication is deliberate: the CRDT is dependency-free and testable
//! on its own — `cargo test -p aperture-collab` needs no rope, no
//! editor, no egui — and the one-line conversion lives behind the
//! `aperture-core` feature. Same shape as `scene-kit`'s `glow` feature:
//! a pure data core with the integration bolted on.
//!
//! ## The rule that matters most
//!
//! **A remote edit must not enter your undo history.**
//!
//! aperture's undo is "apply the inverse op", and the inverse of a
//! collaborator's insert is a delete of their text. If remote edits
//! landed in the local history, one Ctrl+Z would silently remove
//! somebody else's sentence — and it would be *your* undo stack that did
//! it, so the blame lands nowhere useful.
//!
//! So [`Session::remote`] returns [`TextEdit`]s the caller applies to
//! the buffer DIRECTLY, bypassing `Editor`'s history-recording path,
//! while [`Session::local`] takes edits that already went through
//! history normally. That asymmetry is the whole integration contract,
//! and getting it wrong is not a crash — it is a slow leak of other
//! people's work.
//!
//! ## What is not here
//!
//! Transport. `studio-collab` already owns the carrier, session
//! negotiation and DHT rendezvous; a second answer to "how do bytes
//! reach the peer" would be one too many. This crate produces and
//! consumes [`rga::CrdtOp`] values and can render them as bytes
//! ([`wire`]); moving those bytes is somebody else's job.

pub mod presence;
pub mod rga;
pub mod wire;

pub use presence::{Caret, Presence, Resolved};
pub use rga::{CharId, CrdtOp, Effect, Pending, Rga, SiteId};

/// A local text change, in visible char indices.
///
/// Structurally `aperture_core::Op`. Kept separate so the CRDT core has
/// no dependencies; see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextEdit {
    Insert { at: usize, text: String },
    Delete { at: usize, text: String },
}

impl TextEdit {
    pub fn at(&self) -> usize {
        match self {
            TextEdit::Insert { at, .. } | TextEdit::Delete { at, .. } => *at,
        }
    }
    pub fn text(&self) -> &str {
        match self {
            TextEdit::Insert { text, .. } | TextEdit::Delete { text, .. } => text,
        }
    }
}

#[cfg(feature = "aperture-core")]
mod core_bridge {
    use super::TextEdit;
    use aperture_core::op::Op;

    impl From<&Op> for TextEdit {
        fn from(op: &Op) -> TextEdit {
            match op {
                Op::Insert { at, text } => TextEdit::Insert { at: *at, text: text.clone() },
                Op::Delete { at, text } => TextEdit::Delete { at: *at, text: text.clone() },
            }
        }
    }

    impl From<&TextEdit> for Op {
        fn from(e: &TextEdit) -> Op {
            match e {
                TextEdit::Insert { at, text } => Op::Insert { at: *at, text: text.clone() },
                TextEdit::Delete { at, text } => Op::Delete { at: *at, text: text.clone() },
            }
        }
    }
}

/// One participant's view of a shared document.
#[derive(Debug, Clone)]
pub struct Session {
    site: SiteId,
    doc: Rga,
    peers: Presence,
}

impl Session {
    /// Open a document nobody else has yet. Seeds the CRDT from the
    /// file's text as though this site typed all of it.
    pub fn host(site: SiteId, text: &str) -> Self {
        Session { site, doc: Rga::from_text(text, site), peers: Presence::new() }
    }

    /// Join a document someone else opened: start EMPTY and replay their
    /// [`Session::history`].
    ///
    /// Seeding both sides from the same file independently is the one
    /// way to break this. Two disjoint id spaces describing the same
    /// characters merge into a duplicated document, and it looks like a
    /// CRDT bug rather than a setup error. Two constructors exist so the
    /// distinction has to be made at the call site.
    pub fn join(site: SiteId) -> Self {
        Session { site, doc: Rga::new(), peers: Presence::new() }
    }

    pub fn site(&self) -> SiteId {
        self.site
    }
    pub fn text(&self) -> String {
        self.doc.text()
    }
    pub fn doc(&self) -> &Rga {
        &self.doc
    }
    pub fn peers(&self) -> &Presence {
        &self.peers
    }

    /// Everything a joining peer needs.
    pub fn history(&self) -> Vec<CrdtOp> {
        self.doc.history()
    }

    /// Record a local edit that has ALREADY been applied to the buffer
    /// and entered the undo history. Returns the ops to broadcast.
    pub fn local(&mut self, edit: &TextEdit) -> Vec<CrdtOp> {
        match edit {
            TextEdit::Insert { at, text } => self.doc.local_insert(*at, text, self.site),
            TextEdit::Delete { at, text } => self.doc.local_delete(*at, text.chars().count()),
        }
    }

    pub fn local_all(&mut self, edits: &[TextEdit]) -> Vec<CrdtOp> {
        edits.iter().flat_map(|e| self.local(e)).collect()
    }

    /// Apply remote ops, returning the edits to make to the local buffer
    /// **without recording them in undo history** — see the module docs.
    ///
    /// Edits come back in application order, each indexed against the
    /// buffer as it stands after the previous one, so a caller applies
    /// them in sequence and adjusts nothing.
    pub fn remote(&mut self, ops: &[CrdtOp]) -> Vec<TextEdit> {
        self.doc
            .apply_all(ops)
            .into_iter()
            .filter_map(|e| match e {
                Effect::Inserted { at, ch } => Some(TextEdit::Insert { at, text: ch.to_string() }),
                Effect::Removed { at, ch } => Some(TextEdit::Delete { at, text: ch.to_string() }),
                Effect::None => None,
            })
            .collect()
    }

    /// Ops still waiting on a causal predecessor. Persistently nonzero
    /// means the transport is losing messages, not that the CRDT is
    /// stuck — worth surfacing in the loop pane rather than hiding.
    pub fn deferred(&self) -> usize {
        self.doc.deferred_count()
    }

    // ── presence ────────────────────────────────────────────────────

    /// Broadcast where our cursor is. Takes visible indices, because
    /// that is what `Editor`'s selection holds; returns the anchored
    /// form to send.
    pub fn set_local_caret(&mut self, anchor: usize, head: usize) -> Caret {
        let c = presence::selection(&self.doc, anchor, head);
        self.peers.set(self.site, c);
        c
    }

    pub fn observe_caret(&mut self, site: SiteId, caret: Caret) {
        self.peers.set(site, caret);
    }

    pub fn forget(&mut self, site: SiteId) -> bool {
        self.peers.remove(site)
    }

    /// Other people's carets, resolved to indices for the renderer.
    pub fn remote_carets(&self) -> Vec<Resolved> {
        self.peers.resolve_all(&self.doc, Some(self.site))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SiteId = SiteId(1);
    const B: SiteId = SiteId(2);

    /// Apply a session's remote edits to a plain String, the way a
    /// caller applies them to a buffer.
    fn apply_to(s: &mut String, edits: &[TextEdit]) {
        for e in edits {
            let mut v: Vec<char> = s.chars().collect();
            match e {
                TextEdit::Insert { at, text } => {
                    for (k, ch) in text.chars().enumerate() {
                        v.insert(at + k, ch);
                    }
                }
                TextEdit::Delete { at, text } => {
                    let n = text.chars().count();
                    v.drain(*at..*at + n);
                }
            }
            *s = v.into_iter().collect();
        }
    }

    #[test]
    fn host_and_join_converge_through_the_facade() {
        let mut a = Session::host(A, "fn main() {}");
        let mut b = Session::join(B);
        b.remote(&a.history());
        assert_eq!(b.text(), a.text());

        let from_a = a.local(&TextEdit::Insert { at: 11, text: "\n".into() });
        let from_b = b.local(&TextEdit::Insert { at: 0, text: "// hi\n".into() });
        b.remote(&from_a);
        a.remote(&from_b);
        assert_eq!(a.text(), b.text());
        assert_eq!(a.text(), "// hi\nfn main() {\n}");
    }

    #[test]
    fn remote_edits_apply_in_sequence_to_a_plain_buffer() {
        // the contract a caller relies on: each edit is indexed against
        // the buffer as it stands after the previous one
        let mut a = Session::host(A, "abc");
        let mut b = Session::join(B);
        let mut buf = String::new();
        apply_to(&mut buf, &b.remote(&a.history()));
        assert_eq!(buf, "abc");

        let ops = a.local_all(&[
            TextEdit::Insert { at: 3, text: "def".into() },
            TextEdit::Delete { at: 0, text: "a".into() },
        ]);
        apply_to(&mut buf, &b.remote(&ops));
        assert_eq!(buf, b.text());
        assert_eq!(buf, "bcdef");
    }

    #[test]
    fn a_delete_is_sized_by_chars_not_bytes() {
        let mut a = Session::host(A, "héllo");
        let ops = a.local(&TextEdit::Delete { at: 1, text: "é".into() });
        assert_eq!(ops.len(), 1, "one character, not two bytes");
        assert_eq!(a.text(), "hllo");
    }

    #[test]
    fn concurrent_edits_through_two_sessions_converge() {
        let mut a = Session::host(A, "one two");
        let mut b = Session::join(B);
        b.remote(&a.history());
        let oa = a.local(&TextEdit::Insert { at: 3, text: " AND".into() });
        let ob = b.local(&TextEdit::Delete { at: 0, text: "one".into() });
        a.remote(&ob);
        b.remote(&oa);
        assert_eq!(a.text(), b.text());
    }

    #[test]
    fn duplicate_delivery_produces_no_edits() {
        let mut a = Session::host(A, "x");
        let mut b = Session::join(B);
        b.remote(&a.history());
        let ops = a.local(&TextEdit::Insert { at: 1, text: "y".into() });
        assert_eq!(b.remote(&ops).len(), 1);
        assert!(b.remote(&ops).is_empty(), "a redelivered op must not edit the buffer twice");
        assert_eq!(b.text(), "xy");
    }

    #[test]
    fn out_of_order_ops_are_reported_as_deferred() {
        let mut a = Session::host(A, "");
        let ops = a.local(&TextEdit::Insert { at: 0, text: "abc".into() });
        let mut b = Session::join(B);
        assert!(b.remote(&ops[2..]).is_empty());
        assert_eq!(b.deferred(), 1, "a caller can see the transport dropped something");
        b.remote(&ops[..2]);
        assert_eq!(b.text(), "abc");
        assert_eq!(b.deferred(), 0);
    }

    #[test]
    fn a_caret_broadcast_resolves_on_the_other_side() {
        let mut a = Session::host(A, "hello world");
        let mut b = Session::join(B);
        b.remote(&a.history());
        let c = a.set_local_caret(5, 5);
        b.observe_caret(A, c);
        let seen = b.remote_carets();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].head, 5);
        // B types at the front; A's caret follows its character with no
        // transformation anywhere
        let ob = b.local(&TextEdit::Insert { at: 0, text: ">> ".into() });
        a.remote(&ob);
        assert_eq!(b.remote_carets()[0].head, 8);
    }

    #[test]
    fn our_own_caret_is_not_in_the_remote_list() {
        let mut a = Session::host(A, "abc");
        a.set_local_caret(1, 1);
        assert!(a.remote_carets().is_empty());
        a.observe_caret(B, Caret::default());
        assert_eq!(a.remote_carets().len(), 1);
        assert!(a.forget(B));
        assert!(a.remote_carets().is_empty());
    }

    #[test]
    fn a_selection_broadcast_keeps_its_extent() {
        let mut a = Session::host(A, "abcdef");
        let c = a.set_local_caret(1, 4);
        let mut b = Session::join(B);
        b.remote(&a.history());
        b.observe_caret(A, c);
        let r = b.remote_carets()[0];
        assert_eq!((r.start(), r.end()), (1, 4));
        assert!(!r.is_caret());
    }

    #[test]
    fn three_sessions_converge_under_different_delivery_orders() {
        let mut a = Session::host(A, "shared");
        let hist = a.history();
        let mut b = Session::join(SiteId(2));
        let mut c = Session::join(SiteId(3));
        b.remote(&hist);
        c.remote(&hist);

        let oa = a.local(&TextEdit::Insert { at: 6, text: "!".into() });
        let ob = b.local(&TextEdit::Insert { at: 0, text: "un".into() });
        let oc = c.local(&TextEdit::Delete { at: 0, text: "s".into() });

        a.remote(&ob);
        a.remote(&oc);
        b.remote(&oc);
        b.remote(&oa);
        c.remote(&oa);
        c.remote(&ob);

        assert_eq!(a.text(), b.text());
        assert_eq!(b.text(), c.text());
    }
}

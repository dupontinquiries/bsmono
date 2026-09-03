//! rga — the sequence CRDT under aperture's text.
//!
//! ## Why not studio's answer
//!
//! `apps/studio/crates/studio-collab` solves the same problem with a
//! Lamport clock and per-field last-writer-wins, and its own docs say
//! why that does not transfer:
//!
//! > LWW is a poor fit for text (it loses characters) but a good fit for
//! > a timeline
//!
//! Studio gets away with it because clips have stable ids and absolute
//! positions — there is no ordering array to reindex. Aperture's
//! `Op::Insert { at, .. }` is a char index valid only at the instant it
//! was made; a concurrent insert three characters earlier invalidates
//! it. That is exactly the case LWW loses.
//!
//! ## Why not OT
//!
//! Transforming two char-indexed insert/delete ops against each other is
//! about forty lines, and aperture's `Op` is already the OT primitive —
//! it is by far the smaller change. But OT's correctness for more than
//! two peers depends on a server serializing the history, and
//! studio-collab is explicitly serverless ("without a server deciding
//! who wins") with a DHT and a peer carrier already built. Adding the
//! one component in the suite that needs infrastructure is a bigger cost
//! than the bigger diff.
//!
//! ## What RGA is
//!
//! Every character gets an immutable id and remembers which character it
//! was inserted after. Insertion is "find my origin, skip the
//! concurrent stuff that sorts above me, land there". Deletion is a
//! tombstone. Two replicas that have seen the same set of ops hold the
//! same sequence regardless of arrival order, with no transformation and
//! no coordinator.
//!
//! The skip rule is one line and it is the whole algorithm:
//!
//! ```text
//! pos = origin + 1;  while entries[pos].id > new.id { pos += 1 }
//! ```
//!
//! It works because ids are Lamport-ordered, so a character can never
//! have an id below the character it was inserted after. Anything with a
//! higher id sitting between us and our origin is therefore concurrent
//! (or descended from something concurrent), and both replicas compute
//! the same answer because `Ord` on the id is total and identical
//! everywhere.
//!
//! ## The bonus that makes presence nearly free
//!
//! A caret anchored to a character ID rather than a char index needs no
//! transformation when a remote edit lands. Index-based presence has to
//! be shifted by every incoming op, which is a second correctness
//! problem the size of the first. See [`crate::presence`].
//!
//! Pure: std only, no I/O, no time.

use std::collections::{BTreeMap, HashMap};

/// Which replica an op came from. Must be unique per participant and
/// stable for the session — the same contract as
/// `studio_collab::SiteId`, deliberately, so a future shared transport
/// can carry both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SiteId(pub u64);

/// A character's identity: a Lamport counter plus the site that minted
/// it.
///
/// Ordering is (lamport, site) and it is TOTAL, which is what lets every
/// replica resolve concurrent insertions identically without asking
/// anyone. The site tiebreak is arbitrary but consistent, which is all
/// convergence needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CharId {
    pub lamport: u64,
    pub site: SiteId,
}

impl CharId {
    pub fn new(lamport: u64, site: u64) -> Self {
        CharId { lamport, site: SiteId(site) }
    }
}

/// One character in the sequence.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    id: CharId,
    ch: char,
    /// The character this was inserted after. `None` = the start.
    origin: Option<CharId>,
    /// Tombstoned. Kept because a concurrent insert may still name it as
    /// an origin, and because deleting the entry outright would make
    /// delete-then-insert order-dependent.
    deleted: bool,
}

/// What goes over the wire.
#[derive(Debug, Clone, PartialEq)]
pub enum CrdtOp {
    Insert { id: CharId, ch: char, after: Option<CharId> },
    /// Tombstone `target`. Idempotent, so duplicate delivery is free.
    Delete { target: CharId },
}

impl CrdtOp {
    pub fn lamport(&self) -> u64 {
        match self {
            CrdtOp::Insert { id, .. } => id.lamport,
            // A delete carries no id of its own; it does not extend the
            // causal frontier, it only references it.
            CrdtOp::Delete { .. } => 0,
        }
    }
}

/// Why an op could not be applied yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    /// Its origin has not arrived. Buffer and retry.
    MissingOrigin,
    /// Its target has not arrived. Buffer and retry.
    MissingTarget,
}

/// The effect an applied remote op had on the visible text, in char
/// indices — what the local buffer and every caret need to know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Inserted { at: usize, ch: char },
    Removed { at: usize, ch: char },
    /// Applied but nothing visible changed: a duplicate, or a delete of
    /// something already tombstoned.
    None,
}

/// A replicated character sequence.
#[derive(Debug, Clone, Default)]
pub struct Rga {
    entries: Vec<Entry>,
    /// id → index, rebuilt on insert. A linear scan per op is fine at
    /// human typing rates but not at document-load rates, and loading is
    /// exactly when a session starts.
    index: HashMap<CharId, usize>,
    /// Highest lamport seen from anyone, including ourselves.
    frontier: u64,
    /// Ops that arrived before their causal predecessor.
    deferred: Vec<CrdtOp>,
}

impl Rga {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed from existing text as though `site` typed all of it. Use for
    /// the peer that opens the document first; everyone else starts
    /// empty and receives.
    ///
    /// Both replicas MUST NOT seed the same text independently — that
    /// produces two disjoint id spaces and the merge is a duplicated
    /// document. Whoever joins second gets the sequence, not the string.
    pub fn from_text(text: &str, site: SiteId) -> Self {
        let mut r = Rga::new();
        let mut after: Option<CharId> = None;
        for ch in text.chars() {
            r.frontier += 1;
            let id = CharId { lamport: r.frontier, site };
            // A locally minted op names an origin we just placed, so it
            // cannot be deferred. Asserted rather than ignored: if it
            // ever can, that is a bug in id minting, not a transport
            // hiccup to swallow.
            debug_assert!(r.apply(&CrdtOp::Insert { id, ch, after }).is_ok());
            after = Some(id);
        }
        r
    }

    /// The visible text.
    pub fn text(&self) -> String {
        self.entries.iter().filter(|e| !e.deleted).map(|e| e.ch).collect()
    }

    /// Number of visible characters.
    pub fn len(&self) -> usize {
        self.entries.iter().filter(|e| !e.deleted).count()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Id of the visible character at `at`, or None past the end.
    pub fn id_at(&self, at: usize) -> Option<CharId> {
        self.visible_ids().into_iter().nth(at)
    }

    /// Visible index of `id`, or None if it is unknown or tombstoned.
    pub fn index_of(&self, id: CharId) -> Option<usize> {
        let mut vis = 0usize;
        for e in &self.entries {
            if e.id == id {
                return (!e.deleted).then_some(vis);
            }
            if !e.deleted {
                vis += 1;
            }
        }
        None
    }

    /// Nearest visible index for a possibly-tombstoned id — where a
    /// caret anchored to a since-deleted character should sit. Counts
    /// the visible characters before it, which is the position its
    /// neighbour now occupies.
    pub fn index_after(&self, id: CharId) -> Option<usize> {
        let mut vis = 0usize;
        for e in &self.entries {
            if e.id == id {
                return Some(if e.deleted { vis } else { vis + 1 });
            }
            if !e.deleted {
                vis += 1;
            }
        }
        None
    }

    fn visible_ids(&self) -> Vec<CharId> {
        self.entries.iter().filter(|e| !e.deleted).map(|e| e.id).collect()
    }

    // ── local edits ─────────────────────────────────────────────────

    /// Insert `text` at visible index `at`, minting new ids for `site`.
    /// Returns the ops to broadcast.
    pub fn local_insert(&mut self, at: usize, text: &str, site: SiteId) -> Vec<CrdtOp> {
        let mut after = if at == 0 { None } else { self.id_at(at - 1) };
        let mut out = Vec::new();
        for ch in text.chars() {
            self.frontier += 1;
            let id = CharId { lamport: self.frontier, site };
            let op = CrdtOp::Insert { id, ch, after };
            debug_assert!(self.apply(&op).is_ok(), "a local insert named a missing origin");
            out.push(op);
            after = Some(id);
        }
        out
    }

    /// Delete `count` visible characters starting at `at`.
    pub fn local_delete(&mut self, at: usize, count: usize) -> Vec<CrdtOp> {
        let ids: Vec<CharId> = self.visible_ids().into_iter().skip(at).take(count).collect();
        ids.into_iter()
            .map(|target| {
                let op = CrdtOp::Delete { target };
                debug_assert!(self.apply(&op).is_ok(), "a local delete named a missing target");
                op
            })
            .collect()
    }

    // ── remote ops ──────────────────────────────────────────────────

    /// Apply one op. Idempotent and order-independent: an op whose
    /// causal predecessor is missing is buffered and retried, and a
    /// duplicate is a no-op.
    pub fn apply(&mut self, op: &CrdtOp) -> Result<Effect, Pending> {
        let eff = self.apply_one(op)?;
        // Arrival may have unblocked something buffered earlier.
        self.drain_deferred();
        Ok(eff)
    }

    fn apply_one(&mut self, op: &CrdtOp) -> Result<Effect, Pending> {
        match op {
            CrdtOp::Insert { id, ch, after } => {
                self.frontier = self.frontier.max(id.lamport);
                if self.index.contains_key(id) {
                    return Ok(Effect::None); // duplicate delivery
                }
                let origin_idx = match after {
                    None => None,
                    Some(a) => match self.index.get(a) {
                        Some(&i) => Some(i),
                        None => {
                            self.defer(op.clone());
                            return Err(Pending::MissingOrigin);
                        }
                    },
                };
                // The whole algorithm: start just past the origin and
                // skip everything that sorts above us. Anything there is
                // concurrent, or descended from something concurrent,
                // because a Lamport id is always greater than its
                // origin's.
                let mut pos = match origin_idx {
                    Some(i) => i + 1,
                    None => 0,
                };
                while pos < self.entries.len() && self.entries[pos].id > *id {
                    pos += 1;
                }
                let vis_at = self.entries[..pos].iter().filter(|e| !e.deleted).count();
                self.entries
                    .insert(pos, Entry { id: *id, ch: *ch, origin: *after, deleted: false });
                self.reindex_from(pos);
                Ok(Effect::Inserted { at: vis_at, ch: *ch })
            }
            CrdtOp::Delete { target } => {
                let Some(&i) = self.index.get(target) else {
                    self.defer(op.clone());
                    return Err(Pending::MissingTarget);
                };
                if self.entries[i].deleted {
                    return Ok(Effect::None); // idempotent
                }
                let vis_at = self.entries[..i].iter().filter(|e| !e.deleted).count();
                let ch = self.entries[i].ch;
                self.entries[i].deleted = true;
                Ok(Effect::Removed { at: vis_at, ch })
            }
        }
    }

    fn defer(&mut self, op: CrdtOp) {
        if !self.deferred.contains(&op) {
            self.deferred.push(op);
        }
    }

    /// Retry buffered ops until a pass applies nothing new. Bounded by
    /// the buffer emptying or stalling; a genuinely unresolvable op
    /// (its origin was never sent) stays buffered forever rather than
    /// being dropped, because dropping it silently loses a character.
    fn drain_deferred(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        loop {
            let pending = std::mem::take(&mut self.deferred);
            let before = pending.len();
            let mut still = Vec::new();
            for op in pending {
                match self.apply_one(&op) {
                    Ok(e) => effects.push(e),
                    Err(_) => {
                        // apply_one re-deferred it; take it back so the
                        // list does not grow while we iterate
                        self.deferred.retain(|o| o != &op);
                        still.push(op);
                    }
                }
            }
            let progressed = still.len() < before;
            self.deferred = still;
            if !progressed || self.deferred.is_empty() {
                break;
            }
        }
        effects
    }

    /// Apply a batch, returning the visible effects in order. Ops whose
    /// predecessors are missing are buffered, not dropped.
    pub fn apply_all(&mut self, ops: &[CrdtOp]) -> Vec<Effect> {
        let mut out = Vec::new();
        for op in ops {
            match self.apply_one(op) {
                Ok(e) => out.push(e),
                Err(_) => {}
            }
        }
        out.extend(self.drain_deferred());
        out.retain(|e| e != &Effect::None);
        out
    }

    /// How many ops are waiting on a predecessor. Nonzero for long is a
    /// transport problem, not a CRDT one, and worth surfacing.
    pub fn deferred_count(&self) -> usize {
        self.deferred.len()
    }

    fn reindex_from(&mut self, from: usize) {
        for (i, e) in self.entries.iter().enumerate().skip(from) {
            self.index.insert(e.id, i);
        }
    }

    /// Every op needed to bring an empty replica to this state — what a
    /// joining peer receives instead of the string.
    pub fn history(&self) -> Vec<CrdtOp> {
        let mut out = Vec::new();
        for e in &self.entries {
            out.push(CrdtOp::Insert { id: e.id, ch: e.ch, after: e.origin });
        }
        for e in &self.entries {
            if e.deleted {
                out.push(CrdtOp::Delete { target: e.id });
            }
        }
        out
    }

    /// Highest lamport per site — a version vector, for asking a peer
    /// only for what is missing.
    pub fn frontier_by_site(&self) -> BTreeMap<SiteId, u64> {
        let mut m = BTreeMap::new();
        for e in &self.entries {
            let slot = m.entry(e.id.site).or_insert(0);
            if e.id.lamport > *slot {
                *slot = e.id.lamport;
            }
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SiteId = SiteId(1);
    const B: SiteId = SiteId(2);
    const C: SiteId = SiteId(3);

    #[test]
    fn local_editing_reads_like_a_string() {
        let mut r = Rga::new();
        r.local_insert(0, "hello", A);
        assert_eq!(r.text(), "hello");
        r.local_insert(5, " world", A);
        assert_eq!(r.text(), "hello world");
        r.local_delete(0, 6);
        assert_eq!(r.text(), "world");
        assert_eq!(r.len(), 5);
    }

    #[test]
    fn from_text_seeds_a_document() {
        let r = Rga::from_text("fn main() {}", A);
        assert_eq!(r.text(), "fn main() {}");
        assert_eq!(r.len(), 12);
    }

    #[test]
    fn a_joining_peer_replays_history() {
        let mut a = Rga::from_text("hello", A);
        a.local_delete(0, 1);
        a.local_insert(0, "H", A);
        let mut b = Rga::new();
        b.apply_all(&a.history());
        assert_eq!(b.text(), a.text());
        assert_eq!(b.text(), "Hello");
    }

    /// The property the whole module exists for.
    fn converges(ops: &[CrdtOp], expect_same_as: &str) {
        // every rotation of the op list, plus a duplicate of each op
        for start in 0..ops.len() {
            let mut r = Rga::new();
            let rotated: Vec<CrdtOp> =
                ops[start..].iter().chain(ops[..start].iter()).cloned().collect();
            r.apply_all(&rotated);
            // deliver everything a second time
            r.apply_all(&rotated);
            assert_eq!(r.deferred_count(), 0, "rotation {start} left ops stranded");
            assert_eq!(r.text(), expect_same_as, "rotation {start} diverged");
        }
    }

    #[test]
    fn concurrent_inserts_at_the_same_spot_converge() {
        let mut a = Rga::from_text("ac", A);
        let base = a.history();
        let mut b = Rga::new();
        b.apply_all(&base);

        // both type at index 1, neither has seen the other
        let from_a = a.local_insert(1, "X", A);
        let from_b = b.local_insert(1, "Y", B);
        a.apply_all(&from_b);
        b.apply_all(&from_a);
        assert_eq!(a.text(), b.text(), "replicas disagree");
        // and the answer is one of the two orderings, not a mangle
        assert!(a.text() == "aXYc" || a.text() == "aYXc", "{}", a.text());

        let mut all = base;
        all.extend(from_a);
        all.extend(from_b);
        let expect = a.text();
        converges(&all, &expect);
    }

    #[test]
    fn three_way_concurrency_converges() {
        let seed = Rga::from_text("hello world", A).history();
        let (mut a, mut b, mut c) = (Rga::new(), Rga::new(), Rga::new());
        for r in [&mut a, &mut b, &mut c] {
            r.apply_all(&seed);
        }
        let oa = a.local_insert(5, ",", A);
        let ob = b.local_insert(11, "!", B);
        let oc = c.local_delete(0, 1);

        let mut all = seed;
        all.extend(oa.clone());
        all.extend(ob.clone());
        all.extend(oc.clone());

        for r in [&mut a, &mut b, &mut c] {
            r.apply_all(&oa);
            r.apply_all(&ob);
            r.apply_all(&oc);
        }
        assert_eq!(a.text(), b.text());
        assert_eq!(b.text(), c.text());
        assert_eq!(a.text(), "ello, world!");
        converges(&all, "ello, world!");
    }

    #[test]
    fn out_of_order_delivery_is_buffered_not_dropped() {
        let mut a = Rga::new();
        let ops = a.local_insert(0, "abc", A);
        let mut b = Rga::new();
        // deliver the LAST op first: its origin has not arrived
        assert_eq!(b.apply(&ops[2]), Err(Pending::MissingOrigin));
        assert_eq!(b.deferred_count(), 1);
        assert_eq!(b.text(), "");
        b.apply(&ops[0]).unwrap();
        b.apply(&ops[1]).unwrap();
        assert_eq!(b.text(), "abc", "the buffered op landed once it could");
        assert_eq!(b.deferred_count(), 0);
    }

    #[test]
    fn a_delete_before_its_target_is_buffered() {
        let mut a = Rga::new();
        let ins = a.local_insert(0, "x", A);
        let del = a.local_delete(0, 1);
        let mut b = Rga::new();
        assert_eq!(b.apply(&del[0]), Err(Pending::MissingTarget));
        b.apply(&ins[0]).unwrap();
        assert_eq!(b.text(), "", "the delete applied as soon as its target existed");
        assert_eq!(b.deferred_count(), 0);
    }

    #[test]
    fn duplicate_delivery_changes_nothing() {
        let mut a = Rga::new();
        let ops = a.local_insert(0, "hi", A);
        let mut b = Rga::new();
        b.apply_all(&ops);
        b.apply_all(&ops);
        b.apply_all(&ops);
        assert_eq!(b.text(), "hi");
    }

    #[test]
    fn deleting_twice_is_idempotent() {
        let mut a = Rga::from_text("ab", A);
        let d = a.local_delete(0, 1);
        assert_eq!(a.apply(&d[0]), Ok(Effect::None));
        assert_eq!(a.text(), "b");
    }

    #[test]
    fn an_insert_next_to_a_deleted_character_still_lands() {
        // the reason tombstones are kept: a concurrent insert may name a
        // character the other side has already removed
        let mut a = Rga::from_text("abc", A);
        let base = a.history();
        let mut b = Rga::new();
        b.apply_all(&base);
        let del = a.local_delete(1, 1); // remove 'b'
        let ins = b.local_insert(2, "X", B); // after 'b'
        a.apply_all(&ins);
        b.apply_all(&del);
        assert_eq!(a.text(), b.text());
        assert_eq!(a.text(), "aXc");
    }

    #[test]
    fn effects_report_visible_indices() {
        let mut a = Rga::from_text("ac", A);
        let mut b = Rga::new();
        b.apply_all(&a.history());
        let ops = a.local_insert(1, "b", A);
        let eff = b.apply_all(&ops);
        assert_eq!(eff, vec![Effect::Inserted { at: 1, ch: 'b' }]);
        let d = a.local_delete(0, 1);
        assert_eq!(b.apply_all(&d), vec![Effect::Removed { at: 0, ch: 'a' }]);
        assert_eq!(b.text(), "bc");
    }

    #[test]
    fn index_of_and_index_after_track_tombstones() {
        let mut a = Rga::from_text("abc", A);
        let b_id = a.id_at(1).unwrap();
        assert_eq!(a.index_of(b_id), Some(1));
        a.local_delete(1, 1);
        assert_eq!(a.index_of(b_id), None, "a tombstone has no visible index");
        // a caret anchored to it lands where its neighbour now is
        assert_eq!(a.index_after(b_id), Some(1));
        assert_eq!(a.text(), "ac");
    }

    #[test]
    fn interleaved_typing_does_not_shuffle_a_run() {
        // the classic RGA failure people worry about: two peers typing
        // whole words at the same spot must not comb together
        let mut a = Rga::from_text("|", A);
        let base = a.history();
        let mut b = Rga::new();
        b.apply_all(&base);
        let oa = a.local_insert(0, "alpha", A);
        let ob = b.local_insert(0, "bravo", B);
        a.apply_all(&ob);
        b.apply_all(&oa);
        assert_eq!(a.text(), b.text());
        let t = a.text();
        assert!(
            t == "alphabravo|" || t == "bravoalpha|",
            "runs stayed contiguous? got {t}"
        );
    }

    #[test]
    fn multibyte_text_survives() {
        let mut a = Rga::from_text("héllo 日本", A);
        let mut b = Rga::new();
        b.apply_all(&a.history());
        let ops = a.local_insert(6, "→", A);
        b.apply_all(&ops);
        assert_eq!(a.text(), b.text());
        assert_eq!(a.text(), "héllo →日本");
        // char indices, not byte indices, throughout
        assert_eq!(a.len(), "héllo →日本".chars().count());
    }

    // ── randomized convergence ──────────────────────────────────────
    //
    // The hand-written cases above cover schedules I thought of. A CRDT
    // is exactly where that is not enough: the failure mode is a
    // specific interleaving of concurrent inserts at one position, and
    // the space of interleavings is combinatorial. Deterministic seed so
    // a failure reproduces — a convergence bug that only shows up on one
    // machine is a bug nobody fixes.

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 11
        }
        fn upto(&mut self, n: usize) -> usize {
            if n == 0 {
                0
            } else {
                (self.next() % n as u64) as usize
            }
        }
    }

    /// Random concurrent editing across `n_sites` replicas with a lossy,
    /// reordering, duplicating delivery schedule. Every replica must end
    /// with identical text and nothing stranded.
    fn fuzz_round(seed: u64, n_sites: usize, rounds: usize) {
        let words = ["a", "bc", "def", "\n", "  ", "é", "日", "xyz"];
        let mut rng = Rng(seed);
        let mut reps: Vec<Rga> = (0..n_sites).map(|_| Rga::new()).collect();

        // One replica seeds; everyone else replays. (Seeding
        // independently is the documented way to break this.)
        let seed_ops = Rga::from_text("start", SiteId(0)).history();
        for r in reps.iter_mut() {
            r.apply_all(&seed_ops);
        }

        // ops each replica has produced but not yet given to peer j
        let mut outbox: Vec<Vec<CrdtOp>> = vec![Vec::new(); n_sites];
        let mut all_ops: Vec<CrdtOp> = seed_ops.clone();

        for _ in 0..rounds {
            // a random replica makes a random local edit
            let i = rng.upto(n_sites);
            let len = reps[i].len();
            let produced = if len > 2 && rng.next() % 3 == 0 {
                let at = rng.upto(len);
                let count = 1 + rng.upto((len - at).min(3));
                reps[i].local_delete(at, count)
            } else {
                let at = rng.upto(len + 1);
                let w = words[rng.upto(words.len())];
                reps[i].local_insert(at, w, SiteId(i as u64))
            };
            all_ops.extend(produced.iter().cloned());
            outbox[i].extend(produced);

            // deliver a random slice of a random outbox to a random peer,
            // sometimes reversed, sometimes twice
            let from = rng.upto(n_sites);
            let to = rng.upto(n_sites);
            if from != to && !outbox[from].is_empty() {
                let take = 1 + rng.upto(outbox[from].len());
                let mut batch: Vec<CrdtOp> = outbox[from][..take].to_vec();
                if rng.next() % 2 == 0 {
                    batch.reverse(); // out-of-order delivery
                }
                reps[to].apply_all(&batch);
                if rng.next() % 4 == 0 {
                    reps[to].apply_all(&batch); // duplicate delivery
                }
            }
        }

        // settle: everyone eventually sees everything
        for r in reps.iter_mut() {
            r.apply_all(&all_ops);
            r.apply_all(&all_ops);
        }

        let expect = reps[0].text();
        for (i, r) in reps.iter().enumerate() {
            assert_eq!(
                r.deferred_count(),
                0,
                "seed {seed}: replica {i} stranded {} ops",
                r.deferred_count()
            );
            assert_eq!(r.text(), expect, "seed {seed}: replica {i} diverged");
        }
    }

    #[test]
    fn random_schedules_converge_two_sites() {
        for seed in 0..40 {
            fuzz_round(seed, 2, 60);
        }
    }

    #[test]
    fn random_schedules_converge_five_sites() {
        for seed in 100..130 {
            fuzz_round(seed, 5, 80);
        }
    }

    #[test]
    fn a_partition_that_heals_converges() {
        // two halves edit in isolation for a while, then reconnect —
        // the case a lossy-delivery fuzz reaches only by accident
        let seed_ops = Rga::from_text("shared line", SiteId(0)).history();
        let mut rng = Rng(7);
        let mut left: Vec<Rga> = (0..2).map(|_| Rga::new()).collect();
        let mut right: Vec<Rga> = (0..2).map(|_| Rga::new()).collect();
        for r in left.iter_mut().chain(right.iter_mut()) {
            r.apply_all(&seed_ops);
        }
        let mut lops = Vec::new();
        let mut rops = Vec::new();
        for k in 0..30 {
            let i = k % 2;
            let at = rng.upto(left[i].len() + 1);
            let o = left[i].local_insert(at, "L", SiteId(i as u64));
            left[1 - i].apply_all(&o);
            lops.extend(o);

            let at = rng.upto(right[i].len() + 1);
            let o = right[i].local_insert(at, "R", SiteId((i + 10) as u64));
            right[1 - i].apply_all(&o);
            rops.extend(o);
        }
        // heal
        for r in left.iter_mut() {
            r.apply_all(&rops);
        }
        for r in right.iter_mut() {
            r.apply_all(&lops);
        }
        let expect = left[0].text();
        for r in left.iter().chain(right.iter()) {
            assert_eq!(r.text(), expect);
            assert_eq!(r.deferred_count(), 0);
        }
        assert_eq!(expect.matches('L').count(), 30);
        assert_eq!(expect.matches('R').count(), 30);
    }

    #[test]
    fn frontier_by_site_reports_a_version_vector() {
        let mut a = Rga::new();
        a.local_insert(0, "aa", A);
        let ops = a.local_insert(2, "b", B);
        let v = a.frontier_by_site();
        assert_eq!(v.get(&A).copied(), Some(2));
        assert!(v.get(&B).copied().unwrap() >= 3);
        assert_eq!(ops.len(), 1);
        assert!(v.get(&C).is_none());
    }

    #[test]
    fn lamport_rises_past_anything_seen() {
        let mut a = Rga::new();
        let far = CrdtOp::Insert { id: CharId::new(500, 9), ch: 'z', after: None };
        a.apply(&far).unwrap();
        let ops = a.local_insert(0, "x", A);
        match &ops[0] {
            CrdtOp::Insert { id, .. } => {
                assert!(id.lamport > 500, "a local id must not collide with what we have seen")
            }
            _ => panic!(),
        }
    }
}

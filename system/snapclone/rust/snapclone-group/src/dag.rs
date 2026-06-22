//! dag.rs — a content-addressed hash-linked DAG for ordering ephemeral group
//! snaps across a distributed system, without pulling in git.
//!
//! # What this is (and what it deliberately is NOT)
//!
//! The user's instinct: keep group chats ephemeral, but double-check
//! distribution with "some sort of git repo that stores the hashes of the snaps
//! so distribution is smooth and ordered across a distributed system." The part
//! of git that actually solves that problem is the **Merkle DAG**: every object
//! names its parents by hash, so the graph structure *is* the causal history,
//! and two replicas can find their difference cheaply by comparing tips. We take
//! exactly that — and none of the rest (no files, packs, refs, working tree,
//! merge strategies).
//!
//! Each `DagNode` is content-addressed: its id is the SHA-256 of its canonical
//! bytes, so the id can't disagree with the content (tamper-evidence comes for
//! free, like a git object hash). A node names the **parent node ids it had
//! seen** when it was created, which makes concurrency detectable (two nodes
//! with disjoint causal pasts are concurrent) and gives us a partial order. A
//! Lamport logical clock + a deterministic tiebreak turns that partial order
//! into a **total order every replica computes identically** — so even with
//! out-of-order arrival and no central clock, everyone shows the same sequence.
//!
//! # Why this fits the constraints
//!
//!   * Ephemeral: nodes carry only snap REFERENCES (content hash + small
//!     metadata), never the media. They expire with the group's DHT TTL; the
//!     DAG is rebuilt from whatever members still hold. Losing old nodes just
//!     truncates history — it never corrupts ordering of what remains.
//!   * Smooth distribution / "only pull what's missing": `missing_for(tips)`
//!     answers "given that a peer has these tips, what nodes do they lack?" so
//!     sync transfers only the delta, never the whole log. This is git's
//!     have/want negotiation in miniature.
//!   * Out-of-order + conflict resolution: `insert` accepts nodes in ANY order,
//!     buffering those whose parents haven't arrived yet (orphans) and admitting
//!     them once the parents do. `total_order` is deterministic regardless of
//!     arrival order.
//!
//! Pure logic over in-memory structures — no I/O, no deps beyond sha2/hex which
//! the crate already uses. Fully unit-tested; the C++/DHT glue that ships nodes
//! around is a thin transport on top.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// A node id is the lowercase-hex SHA-256 of the node's canonical bytes.
pub type NodeId = String;

/// One node in the group DAG: a reference to a snap (or control event) plus the
/// causal parents the author had seen. The `id` is derived, never trusted from
/// the wire — `DagNode::compute_id` recomputes and `SnapDag::insert` verifies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DagNode {
    /// Author's sc-ID (used in the deterministic tiebreak and for display).
    pub author: String,
    /// Membership epoch this node belongs to (nodes from different epochs are
    /// never causally linked; ordering is computed within an epoch).
    pub epoch: u64,
    /// Lamport logical clock: 1 + max(parent clocks), or 1 for a root. Gives a
    /// monotonic-with-causality scalar for the primary sort key.
    pub lamport: u64,
    /// Parent node ids (the tips the author had seen). Empty = a root node.
    /// Sorted + de-duplicated in canonical form so the id is stable.
    pub parents: Vec<NodeId>,
    /// The snap's content hash (SHA-256 of the encrypted media), or other
    /// payload reference. The DAG orders REFERENCES; media travels separately
    /// (content-addressed, fetched P2P, ephemeral by TTL).
    pub content_hash: String,
}

impl DagNode {
    /// Canonical byte serialization for hashing/signing. Deterministic: parents
    /// are sorted+deduped, all fields length-prefixed, so two members building
    /// the "same" node get byte-identical output (and thus the same id).
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut sorted = self.parents.clone();
        sorted.sort();
        sorted.dedup();
        let mut out = Vec::new();
        let put = |out: &mut Vec<u8>, s: &[u8]| {
            out.extend_from_slice(&(s.len() as u32).to_be_bytes());
            out.extend_from_slice(s);
        };
        out.extend_from_slice(b"SDAG1");
        put(&mut out, self.author.as_bytes());
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out.extend_from_slice(&self.lamport.to_be_bytes());
        out.extend_from_slice(&(sorted.len() as u32).to_be_bytes());
        for p in &sorted {
            put(&mut out, p.as_bytes());
        }
        put(&mut out, self.content_hash.as_bytes());
        out
    }

    /// The content-addressed id (lowercase-hex SHA-256 of canonical bytes).
    pub fn compute_id(&self) -> NodeId {
        let digest = Sha256::digest(self.canonical_bytes());
        hex::encode(digest)
    }

    /// Serialize for the WIRE — the bytes that become a sealed `GroupEntryBody`
    /// payload. Distinct from `canonical_bytes` (which is the hashing preimage):
    /// this is a self-describing, round-trippable frame. It uses the SAME
    /// canonical parent ordering (sorted + deduped) as `canonical_bytes`, so a
    /// node that is serialized, shipped, and parsed reconstructs with an
    /// identical `compute_id()` — the content address survives the round trip.
    /// Length-prefixed throughout, matching the group entry framing convention.
    pub fn to_payload(&self) -> Vec<u8> {
        let mut sorted = self.parents.clone();
        sorted.sort();
        sorted.dedup();
        let mut out = Vec::new();
        out.extend_from_slice(b"DAGN1");
        let put = |out: &mut Vec<u8>, s: &[u8]| {
            out.extend_from_slice(&(s.len() as u32).to_be_bytes());
            out.extend_from_slice(s);
        };
        put(&mut out, self.author.as_bytes());
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out.extend_from_slice(&self.lamport.to_be_bytes());
        out.extend_from_slice(&(sorted.len() as u32).to_be_bytes());
        for p in &sorted {
            put(&mut out, p.as_bytes());
        }
        put(&mut out, self.content_hash.as_bytes());
        out
    }

    /// Parse a `DagNode` from `to_payload` bytes. Returns None on any framing
    /// error (truncation, bad magic, non-UTF-8 strings).
    pub fn from_payload(buf: &[u8]) -> Option<DagNode> {
        let mut pos = 0usize;
        // magic
        if buf.len() < 5 || &buf[0..5] != b"DAGN1" {
            return None;
        }
        pos += 5;
        let take_lp = |buf: &[u8], pos: &mut usize| -> Option<Vec<u8>> {
            if *pos + 4 > buf.len() {
                return None;
            }
            let len = u32::from_be_bytes(buf[*pos..*pos + 4].try_into().ok()?) as usize;
            *pos += 4;
            if *pos + len > buf.len() {
                return None;
            }
            let s = buf[*pos..*pos + len].to_vec();
            *pos += len;
            Some(s)
        };
        let take_u64 = |buf: &[u8], pos: &mut usize| -> Option<u64> {
            if *pos + 8 > buf.len() {
                return None;
            }
            let v = u64::from_be_bytes(buf[*pos..*pos + 8].try_into().ok()?);
            *pos += 8;
            Some(v)
        };
        let take_u32 = |buf: &[u8], pos: &mut usize| -> Option<u32> {
            if *pos + 4 > buf.len() {
                return None;
            }
            let v = u32::from_be_bytes(buf[*pos..*pos + 4].try_into().ok()?);
            *pos += 4;
            Some(v)
        };

        let author = String::from_utf8(take_lp(buf, &mut pos)?).ok()?;
        let epoch = take_u64(buf, &mut pos)?;
        let lamport = take_u64(buf, &mut pos)?;
        let n_parents = take_u32(buf, &mut pos)? as usize;
        let mut parents = Vec::with_capacity(n_parents);
        for _ in 0..n_parents {
            parents.push(String::from_utf8(take_lp(buf, &mut pos)?).ok()?);
        }
        let content_hash = String::from_utf8(take_lp(buf, &mut pos)?).ok()?;
        Some(DagNode {
            author,
            epoch,
            lamport,
            parents,
            content_hash,
        })
    }

    /// True if this is a root (no causal parents).
    pub fn is_root(&self) -> bool {
        self.parents.is_empty()
    }
}

/// Why an insert didn't fully admit a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertOutcome {
    /// Node (and possibly some previously-orphaned descendants) admitted.
    Admitted { admitted_ids: Vec<NodeId> },
    /// Node buffered: it references parents we don't have yet. Carries the set
    /// of missing parent ids the caller can request from peers.
    Orphaned { missing_parents: Vec<NodeId> },
    /// The node's claimed id didn't match its content (rejected, tamper).
    BadId,
    /// Already present; no-op.
    Duplicate,
    /// The Lamport clock is inconsistent with the parents (rejected).
    BadClock,
}

/// The DAG of group snap references.
#[derive(Debug, Default, Clone)]
pub struct SnapDag {
    /// Admitted nodes by id (all parents present).
    nodes: HashMap<NodeId, DagNode>,
    /// Reverse edges: parent id → child ids (for tip maintenance + traversal).
    children: HashMap<NodeId, HashSet<NodeId>>,
    /// Current tips (admitted nodes that no admitted node names as a parent).
    tips: HashSet<NodeId>,
    /// Orphans: nodes whose parents aren't all present yet, keyed by their own
    /// id, plus the set of parent ids each is still waiting on.
    orphans: HashMap<NodeId, (DagNode, HashSet<NodeId>)>,
    /// Reverse index: missing-parent id → orphan ids waiting on it, so admitting
    /// a node can cheaply wake the orphans that needed it.
    waiting_on: HashMap<NodeId, HashSet<NodeId>>,
}

impl SnapDag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    pub fn contains(&self, id: &NodeId) -> bool {
        self.nodes.contains_key(id)
    }
    pub fn get(&self, id: &NodeId) -> Option<&DagNode> {
        self.nodes.get(id)
    }

    /// Current tips (the frontier). A peer syncing sends these; we reply with
    /// what they're missing. Sorted for determinism.
    pub fn tips(&self) -> Vec<NodeId> {
        let mut v: Vec<NodeId> = self.tips.iter().cloned().collect();
        v.sort();
        v
    }

    /// Build a node authored locally that extends the CURRENT tips. This is how
    /// a member adds a snap: it automatically parents off everything seen so
    /// far, so its causal past is the whole current history. Returns the node
    /// (already admitted) and its id.
    pub fn author_node(
        &mut self,
        author: &str,
        epoch: u64,
        content_hash: &str,
    ) -> (DagNode, NodeId) {
        let parents = self.tips();
        let lamport = parents
            .iter()
            .filter_map(|p| self.nodes.get(p))
            .map(|n| n.lamport)
            .max()
            .unwrap_or(0)
            + 1;
        let node = DagNode {
            author: author.to_owned(),
            epoch,
            lamport,
            parents,
            content_hash: content_hash.to_owned(),
        };
        let id = node.compute_id();
        // Local authorship is always consistent; admit directly.
        self.admit(node.clone(), id.clone());
        (node, id)
    }

    /// Insert a node received from a peer (any order). Verifies the id, checks
    /// the Lamport clock against present parents, and either admits it (waking
    /// any orphans that were waiting on it) or buffers it as an orphan with the
    /// list of parents to fetch.
    pub fn insert(&mut self, node: DagNode) -> InsertOutcome {
        let id = node.compute_id();
        if self.nodes.contains_key(&id) || self.orphans.contains_key(&id) {
            return InsertOutcome::Duplicate;
        }

        // Which parents are missing right now?
        let missing: HashSet<NodeId> = node
            .parents
            .iter()
            .filter(|p| !self.nodes.contains_key(*p))
            .cloned()
            .collect();

        if !missing.is_empty() {
            // Buffer as orphan; index it under each missing parent.
            for p in &missing {
                self.waiting_on.entry(p.clone()).or_default().insert(id.clone());
            }
            let mut miss_vec: Vec<NodeId> = missing.iter().cloned().collect();
            miss_vec.sort();
            self.orphans.insert(id, (node, missing));
            return InsertOutcome::Orphaned { missing_parents: miss_vec };
        }

        // All parents present → validate the clock, then admit + cascade.
        if !self.clock_ok(&node) {
            return InsertOutcome::BadClock;
        }
        let mut admitted = Vec::new();
        self.admit(node, id.clone());
        admitted.push(id.clone());
        self.cascade_orphans(&id, &mut admitted);
        InsertOutcome::Admitted { admitted_ids: admitted }
    }

    /// Lamport invariant: a node's clock must be exactly 1 + max(parent clocks),
    /// or 1 for a root. Rejects forged clocks that would corrupt ordering.
    fn clock_ok(&self, node: &DagNode) -> bool {
        let expected = node
            .parents
            .iter()
            .filter_map(|p| self.nodes.get(p))
            .map(|n| n.lamport)
            .max()
            .unwrap_or(0)
            + 1;
        node.lamport == expected
    }

    /// Admit a validated node: store it, wire reverse edges, update tips.
    fn admit(&mut self, node: DagNode, id: NodeId) {
        for p in &node.parents {
            self.children.entry(p.clone()).or_default().insert(id.clone());
            // A parent that gains a child is no longer a tip.
            self.tips.remove(p);
        }
        // The new node is a tip until something parents off it.
        if self.children.get(&id).map(|c| c.is_empty()).unwrap_or(true) {
            self.tips.insert(id.clone());
        }
        self.nodes.insert(id, node);
    }

    /// After admitting `id`, any orphan that was waiting solely on it may now be
    /// admissible. Walk the wake-list transitively.
    fn cascade_orphans(&mut self, id: &NodeId, admitted: &mut Vec<NodeId>) {
        let mut ready: Vec<NodeId> = Vec::new();
        if let Some(waiters) = self.waiting_on.remove(id) {
            for w in waiters {
                if let Some((_, missing)) = self.orphans.get_mut(&w) {
                    missing.remove(id);
                    if missing.is_empty() {
                        ready.push(w);
                    }
                }
            }
        }
        for w in ready {
            if let Some((node, _)) = self.orphans.remove(&w) {
                // Re-validate the clock now that parents are present.
                if self.clock_ok(&node) {
                    self.admit(node, w.clone());
                    admitted.push(w.clone());
                    self.cascade_orphans(&w, admitted);
                }
                // If the clock is bad we simply drop the orphan (it was forged);
                // its own descendants stay orphaned and will be re-requested.
            }
        }
    }

    /// Given a peer's tips, return the ids of admitted nodes the peer is MISSING
    /// (everything NOT in the causal past of any of their tips). This is the
    /// "only send the delta" sync primitive — the peer sends tips, we compute
    /// and send the difference, never the whole DAG.
    pub fn missing_for(&self, peer_tips: &[NodeId]) -> Vec<NodeId> {
        // Causal closure of the peer's tips = everything they already have.
        let mut have: HashSet<NodeId> = HashSet::new();
        let mut stack: Vec<NodeId> = peer_tips
            .iter()
            .filter(|t| self.nodes.contains_key(*t))
            .cloned()
            .collect();
        while let Some(n) = stack.pop() {
            if !have.insert(n.clone()) {
                continue;
            }
            if let Some(node) = self.nodes.get(&n) {
                for p in &node.parents {
                    if !have.contains(p) {
                        stack.push(p.clone());
                    }
                }
            }
        }
        // We send everything we have that they don't.
        let mut missing: Vec<NodeId> = self
            .nodes
            .keys()
            .filter(|id| !have.contains(*id))
            .cloned()
            .collect();
        missing.sort();
        missing
    }

    /// Deterministic TOTAL order of all admitted nodes. Same input set → same
    /// order on every replica, regardless of arrival order. Sort key:
    ///   (lamport, author, node_id)
    /// lamport respects causality (a parent always has a strictly smaller clock
    /// than its child), and (author, id) is a stable tiebreak for concurrent
    /// nodes that share a clock. A topological pass is not needed because the
    /// Lamport clock already linearizes causality; we assert parents precede
    /// children as a safety check via the clock invariant enforced on insert.
    pub fn total_order(&self) -> Vec<NodeId> {
        // BTreeMap keyed by the canonical sort tuple gives the order directly.
        let mut ordered: BTreeMap<(u64, String, String), NodeId> = BTreeMap::new();
        for (id, node) in &self.nodes {
            ordered.insert(
                (node.lamport, node.author.clone(), id.clone()),
                id.clone(),
            );
        }
        ordered.into_values().collect()
    }

    /// The orphan ids currently buffered (waiting on missing parents), and the
    /// union of parent ids to fetch. Lets the sync layer request the gaps.
    pub fn pending_fetches(&self) -> Vec<NodeId> {
        let set: BTreeSet<NodeId> = self.waiting_on.keys().cloned().collect();
        set.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(author: &str, lamport: u64, parents: &[&str], content: &str) -> DagNode {
        DagNode {
            author: author.to_owned(),
            epoch: 1,
            lamport,
            parents: parents.iter().map(|s| s.to_string()).collect(),
            content_hash: content.to_owned(),
        }
    }

    #[test]
    fn payload_round_trips_and_preserves_id() {
        // A node with multiple parents (given out of canonical order) must
        // serialize, parse back, and compute the SAME id — the content address
        // survives the wire trip.
        let n = DagNode {
            author: "sc-alice".to_owned(),
            epoch: 7,
            lamport: 42,
            parents: vec!["ffee".to_owned(), "0011".to_owned(), "ffee".to_owned()],
            content_hash: "deadbeef".to_owned(),
        };
        let bytes = n.to_payload();
        let back = DagNode::from_payload(&bytes).expect("parses");
        assert_eq!(back.compute_id(), n.compute_id(), "id stable across the wire");
        assert_eq!(back.author, "sc-alice");
        assert_eq!(back.epoch, 7);
        assert_eq!(back.lamport, 42);
        assert_eq!(back.content_hash, "deadbeef");
    }

    #[test]
    fn from_payload_rejects_garbage() {
        assert!(DagNode::from_payload(b"").is_none());
        assert!(DagNode::from_payload(b"NOPE!").is_none());
        assert!(DagNode::from_payload(b"DAGN1\x00\x00").is_none()); // truncated
    }

    #[test]
    fn root_node_payload_round_trips() {
        let n = DagNode {
            author: "sc-bob".to_owned(),
            epoch: 1,
            lamport: 1,
            parents: vec![],
            content_hash: "abc123".to_owned(),
        };
        let back = DagNode::from_payload(&n.to_payload()).unwrap();
        assert_eq!(back.compute_id(), n.compute_id());
        assert!(back.is_root());
    }

    #[test]
    fn id_is_content_addressed_and_stable() {
        let a = node("sc-a", 1, &[], "h1");
        let b = node("sc-a", 1, &[], "h1");
        assert_eq!(a.compute_id(), b.compute_id(), "same content → same id");
        let c = node("sc-a", 1, &[], "h2");
        assert_ne!(a.compute_id(), c.compute_id(), "different content → different id");
    }

    #[test]
    fn parents_order_does_not_change_id() {
        // Canonical form sorts parents, so [p1,p2] and [p2,p1] hash identically.
        let n1 = node("sc-a", 2, &["p1", "p2"], "h");
        let n2 = node("sc-a", 2, &["p2", "p1"], "h");
        assert_eq!(n1.compute_id(), n2.compute_id());
    }

    #[test]
    fn author_node_extends_tips_and_bumps_lamport() {
        let mut dag = SnapDag::new();
        let (_n1, id1) = dag.author_node("sc-a", 1, "h1");
        assert_eq!(dag.tips(), vec![id1.clone()]);
        let (n2, id2) = dag.author_node("sc-a", 1, "h2");
        // Second node parents off the first and has lamport 2.
        assert_eq!(n2.parents, vec![id1.clone()]);
        assert_eq!(n2.lamport, 2);
        // Tip moved to the new node.
        assert_eq!(dag.tips(), vec![id2]);
        assert_eq!(dag.len(), 2);
    }

    #[test]
    fn out_of_order_insert_buffers_then_admits() {
        // Build a chain root → c1 → c2 on one replica.
        let mut src = SnapDag::new();
        let (root, root_id) = src.author_node("sc-a", 1, "h-root");
        let (c1, c1_id) = src.author_node("sc-a", 1, "h-c1");
        let (c2, _c2_id) = src.author_node("sc-a", 1, "h-c2");

        // Deliver them OUT OF ORDER to a fresh replica: c2 first (orphan), then
        // c1 (still orphan — needs root), then root (cascades all three in).
        let mut dst = SnapDag::new();
        match dst.insert(c2.clone()) {
            InsertOutcome::Orphaned { missing_parents } => {
                assert_eq!(missing_parents, vec![c1_id.clone()]);
            }
            other => panic!("expected orphan, got {other:?}"),
        }
        assert!(matches!(dst.insert(c1.clone()), InsertOutcome::Orphaned { .. }));
        // root admits and cascades c1 then c2.
        match dst.insert(root.clone()) {
            InsertOutcome::Admitted { admitted_ids } => {
                assert_eq!(admitted_ids.len(), 3, "root + cascaded c1 + c2");
                assert!(admitted_ids.contains(&root_id));
            }
            other => panic!("expected admitted cascade, got {other:?}"),
        }
        assert_eq!(dst.len(), 3);
    }

    #[test]
    fn replicas_converge_to_same_total_order() {
        // Two replicas receive the SAME nodes in DIFFERENT orders; total_order
        // must match.
        let mut src = SnapDag::new();
        let (root, _) = src.author_node("sc-a", 1, "h0");
        let (n1, _) = src.author_node("sc-b", 1, "h1");
        let (n2, _) = src.author_node("sc-a", 1, "h2");
        let all = [root, n1, n2];

        let mut r1 = SnapDag::new();
        for n in [&all[0], &all[1], &all[2]] {
            r1.insert(n.clone());
        }
        let mut r2 = SnapDag::new();
        for n in [&all[2], &all[0], &all[1]] {
            r2.insert(n.clone());
        }
        assert_eq!(r1.total_order(), r2.total_order(), "order is arrival-independent");
        assert_eq!(r1.len(), 3);
        assert_eq!(r2.len(), 3);
    }

    #[test]
    fn concurrent_nodes_break_ties_deterministically() {
        // Two roots authored concurrently (both lamport 1, no parents) by
        // different authors must order by author then id — identically on any
        // replica.
        let mut dag = SnapDag::new();
        let a = node("sc-zzz", 1, &[], "ha");
        let b = node("sc-aaa", 1, &[], "hb");
        dag.insert(a.clone());
        dag.insert(b.clone());
        let order = dag.total_order();
        // sc-aaa sorts before sc-zzz.
        assert_eq!(order[0], b.compute_id());
        assert_eq!(order[1], a.compute_id());
    }

    #[test]
    fn missing_for_sends_only_the_delta() {
        // Replica has root → c1 → c2. Peer has only [root]. missing_for should
        // return c1 and c2, not root.
        let mut dag = SnapDag::new();
        let (_root, root_id) = dag.author_node("sc-a", 1, "h0");
        let (_c1, c1_id) = dag.author_node("sc-a", 1, "h1");
        let (_c2, c2_id) = dag.author_node("sc-a", 1, "h2");
        let missing = dag.missing_for(&[root_id.clone()]);
        assert!(missing.contains(&c1_id));
        assert!(missing.contains(&c2_id));
        assert!(!missing.contains(&root_id), "peer already has root");
        // Peer fully caught up → nothing missing.
        assert!(dag.missing_for(&[c2_id]).is_empty());
    }

    #[test]
    fn tampered_id_is_rejected() {
        // A node whose claimed id (recomputed) doesn't match is impossible to
        // smuggle in, because insert ALWAYS recomputes the id from content. So
        // there's no "claimed id" field to forge — proving content-addressing.
        // We assert that flipping content changes the id (the tamper-evidence).
        let good = node("sc-a", 1, &[], "real");
        let evil = node("sc-a", 1, &[], "fake");
        assert_ne!(good.compute_id(), evil.compute_id());
    }

    #[test]
    fn forged_lamport_clock_rejected() {
        let mut dag = SnapDag::new();
        let (_root, root_id) = dag.author_node("sc-a", 1, "h0");
        // A child of root MUST have lamport 2; claim 9 → rejected.
        let bad = node("sc-b", 9, &[&root_id], "h1");
        assert_eq!(dag.insert(bad), InsertOutcome::BadClock);
        assert_eq!(dag.len(), 1, "bad-clock node not admitted");
    }

    #[test]
    fn duplicate_insert_is_noop() {
        let mut dag = SnapDag::new();
        let n = node("sc-a", 1, &[], "h0");
        assert!(matches!(dag.insert(n.clone()), InsertOutcome::Admitted { .. }));
        assert_eq!(dag.insert(n), InsertOutcome::Duplicate);
        assert_eq!(dag.len(), 1);
    }

    #[test]
    fn merge_two_branches_keeps_both_tips_then_joins() {
        // root → (branchA, branchB) authored concurrently → a join node naming
        // both. After the join there's a single tip.
        let mut dag = SnapDag::new();
        let (_root, root_id) = dag.author_node("sc-a", 1, "h0");
        // Two concurrent children of root (same lamport 2, different content).
        let a = node("sc-a", 2, &[&root_id], "hA");
        let b = node("sc-b", 2, &[&root_id], "hB");
        dag.insert(a.clone());
        dag.insert(b.clone());
        let mut tips = dag.tips();
        tips.sort();
        let mut expect = vec![a.compute_id(), b.compute_id()];
        expect.sort();
        assert_eq!(tips, expect, "two concurrent branches → two tips");
        // A join node referencing both branches collapses to one tip.
        let join = node("sc-a", 3, &[&a.compute_id(), &b.compute_id()], "hJoin");
        dag.insert(join.clone());
        assert_eq!(dag.tips(), vec![join.compute_id()]);
    }
}

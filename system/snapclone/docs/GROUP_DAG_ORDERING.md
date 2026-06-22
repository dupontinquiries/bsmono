# Ephemeral Group Ordering — a content-addressed DAG (git's good part, not git)

Status: **design + a tested Rust core** (`snapclone-group::dag`). Answers the
question "keep groups ephemeral, double-check distribution with something
git-like that stores snap hashes for smooth, ordered distribution across a
distributed system — or is there a better, more integrated system?"

## TL;DR

The part of git that solves this problem is the **Merkle DAG**: every object
names its parents by hash, so the graph *is* the causal history and two replicas
find their difference cheaply by comparing tips. We take exactly that and none of
the rest — no files, packs, refs, working tree, or merge strategies. The result
is a few hundred lines of pure, tested logic that handles out-of-order delivery
and gives every member the same message order without a central clock.

Embedding a real git repo would be the wrong tool: it's heavyweight, file- and
disk-oriented, assumes a checkout, and brings a merge model we don't want. We
need the data structure, not the program.

## What a node is

```
DagNode {
  author:       sc-ID            // who made it (tiebreak + display)
  epoch:        u64              // membership epoch (ordering is per-epoch)
  lamport:      u64              // logical clock: 1 + max(parent clocks)
  parents:      [NodeId]         // the tips the author had SEEN (causal past)
  content_hash: sha256(snap ct)  // a REFERENCE to the snap, never the media
}
NodeId = sha256(canonical_bytes(node))   // content-addressed, like a git object
```

The id is derived from the content, so it can't disagree with it — tamper-
evidence is free, exactly like a git object hash. `parents` are the current tips
the author saw, so the graph encodes "happens-before": two nodes with disjoint
causal pasts are concurrent.

## The three properties this buys

1. **Out-of-order delivery.** `insert` accepts nodes in any order. A node whose
   parents haven't arrived is buffered as an *orphan* with the list of missing
   parent ids to fetch; when the parents arrive it's admitted, cascading to any
   descendants that were waiting on it. Delivery order never affects the result.

2. **Deterministic convergence (conflict resolution).** `total_order` sorts by
   `(lamport, author, node_id)`. The Lamport clock respects causality (a parent
   always has a strictly smaller clock than its child); `(author, id)` is a
   stable tiebreak for concurrent nodes. Every replica with the same node set
   computes the same order — no central sequencer, no clocks to agree on.

3. **Smooth distribution — only pull the delta.** `missing_for(peer_tips)`
   computes the causal closure of the peer's tips (what they already have) and
   returns only the nodes they lack. This is git's have/want negotiation in
   miniature: a member announces its tips, the peer replies with the difference,
   and the whole history is never re-sent. Tips live in the group's DHT feed
   bucket; sync is tip-exchange + delta-fetch.

## How it fits the ephemeral model

Nodes carry only snap *references* (content hash + small metadata), never media.
They expire with the group's DHT TTL. Losing old nodes just truncates history —
it never corrupts the ordering of what remains, because each surviving node still
carries its own causal parents and clock. A member rejoining rebuilds whatever
the group still collectively holds. The media itself is content-addressed and
fetched P2P (any member who has it can re-seed), also ephemeral by TTL.

## How it composes with the existing group crypto

The DAG is the *ordering* layer; it sits on top of the *confidentiality* layer
already built and tested:

  * A `DagNode` is serialized into a sealed `GroupEntryBody` (kind `MediaRef` or
    `Control`) — so the DAG metadata (parents, clock, content hash) is itself
    encrypted under the per-message sender key and Ed25519-signed via
    `entry::seal_and_sign`. The DHT and the thin LAN server see only the routing
    bucket + opaque signed bytes (S-19 stays closed).
  * Members fetch entries from `feed_bucket(group, epoch)`, `verify_and_open`
    them, deserialize the `DagNode`, and `insert` it into their local `SnapDag`.
  * `verify_and_open` authenticates BEFORE decrypt; the DAG then re-derives the
    node id from content and checks the Lamport invariant, so a forged ordering
    can't be smuggled in even by a member.

So the layering is: **DHT value layer (micro-DHT / OpenDHT) → sealed+signed
entry (confidentiality + authenticity) → DAG node (ordering + causality)**. Each
layer is independently tested.

## Why not a CRDT / vector clocks / a real DB?

  * **Vector clocks** grow O(members) per message and need membership-stable
    indices; the epoch + Lamport + parent-hashes approach is O(parents) (usually
    1–2) and survives membership churn because epochs re-key and re-root.
  * **A full CRDT (e.g. RGA/LSEQ text CRDT)** solves *character-level* merge,
    which a snap feed doesn't need — snaps are atomic, append-only references.
    The DAG's total order over atomic nodes is the right granularity and far
    simpler.
  * **An embedded database** (sqlite, sled) is for durable local state, not for
    the distributed *agreement* problem. The app already has storage for what it
    wants to keep; the DAG is the wire/agreement model, orthogonal to local
    persistence.

The DAG is the smallest thing that delivers ordered, gap-tolerant, convergent
distribution for an ephemeral feed.

## What ships now vs deferred

**Now (tested):** `snapclone-group::dag` — `DagNode`, `SnapDag`
(author_node / insert / missing_for / total_order / tips / pending_fetches),
content-addressed ids, orphan buffering + cascade, Lamport validation, branch
merge. 11 unit tests including replica convergence under reordering,
out-of-order admit, delta sync, forged-clock rejection.

**Deferred (untestable here / needs the C++ + transport):**
  * Serializing `DagNode` into `GroupEntryBody` + an `sc_group_dag_*` FFI seam
    (the FFI pattern is established; this is the next Rust-verifiable rung).
  * The sync loop: announce tips to the feed bucket, fetch `missing_for` deltas,
    request `pending_fetches` parents. Pure transport on top of the tested core.
  * C++ group session that drives all of the above; the snapclone-server binary
    participating as a passive, integrity-checking replica for public groups.

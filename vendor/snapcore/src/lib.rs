//! snapcore — the pure-Rust DHT node that backs the monorepo's *lean* builds
//! directly, and the *integrated* builds behind a daemon-as-proxy.
//!
//! ## Layering (built bottom-up; this PR is layer 1)
//!
//! 1. **routing** (here): the Kademlia map — k-buckets + the XOR metric +
//!    find-closest — hardened with the two refinements OpenDHT proves are
//!    necessary in the real world: a per-bucket *replacement cache* and node
//!    *liveness* states (good / questionable / bad). This is the INVARIANT core;
//!    every topology and policy decision sits on top of it unchanged.
//!
//! 2. **cluster overlay** (next): a densely-tracked peer set for a shared
//!    mailbox / group / LAN-affinity group, kept regardless of XOR distance.
//!    This is the "smaller connected graphs + id-mapping" resilience model —
//!    clusters are *explicit* (a cluster IS a shared mailbox you join, not an
//!    emergent thing an algorithm discovers). Global Kademlia stays sparse (find
//!    which cluster holds a key); inside a cluster you keep dense peers so a node
//!    that's flaky globally is still a reliable member of its small graphs. Any
//!    node in two mailboxes is a live bridge — that's the id-mapping across them.
//!
//! 3. **scheduling** (next): maintenance (bucket refresh / re-announce) batched
//!    into ID-derived time *windows* and tuned by the data-type / Lifetime flag,
//!    handed to `snapclone-coresched`. The radio has tail energy, so batching
//!    amortizes wake-ups: on battery/cellular minimize radio (wake once per
//!    window); on wifi/plugged the regime flips to minimizing CPU via `uclamp`
//!    (the node's on anyway). Ephemeral data never wakes the radio; durable data
//!    earns a window. The flag trickles app -> share-kit -> snapcore -> scheduler.
//!
//! 4. **transport** (wired; field-validated on real devices): STUN, UPnP/NAT-PMP,
//!    and the relay all already exist in pure Rust; the hole-punch FSM is the one
//!    remaining port. Integrated = daemon-as-proxy (OpenDHT's proxy-client/server
//!    model, the battle-tested answer for CGNAT/asleep peers); lean = relay-first.
//!    On mainline-Linux phones a node can serve for real when able rather than
//!    being relegated to a proxy client — participation is a runtime policy.
//!
//! Storage semantics (TTL, re-announce-dedup, multi-value-per-key, storage cap)
//! are proven in `snapclone-microdht` and lifted in a following step; this crate
//! starts with the routing the micro-DHT lacks.
//!
//! Time is a logical `u64` tick supplied by the caller everywhere, so the whole
//! crate is deterministic and unit-testable with no wall clock and no network —
//! the multi-node story is exercised on loopback, which is what microdht enabled.

pub mod cluster;
pub mod lookup;
pub mod node;
pub mod proximity;
pub mod routing;
pub mod sim;
pub mod store;

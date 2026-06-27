//! share-kit — the opinionated shared MAILBOX, the layer above snapcore + crypto-kit.
//!
//! snapcore is the NODE (a DHT with a keyspace and a value store); crypto-kit is
//! the PRIMITIVES (sealing + the group `SenderChain`). share-kit ties them into
//! the mailbox abstraction the apps actually want — a place a group of members
//! posts to and reads from, where the DHT itself IS the host (no rack):
//!
//!   * a per-epoch FEED bucket holds content-hash POINTERS (multi-value: each
//!     post appends one), and
//!   * a content-addressed BLOB bucket holds the sealed POSTINGS.
//!
//! The mailbox is generalized by an app-supplied `namespace` + `id`, so the same
//! machinery serves snapclone groups (`namespace = "group-feed"`, byte-compatible
//! with the existing scheme) and unendlich sessions alike. A [`Lifetime`] tags
//! each posting Durable or Ephemeral (TTL + whether to replicate onto the cluster
//! roster). DURABILITY IS POPULATION: a mailbox shared by many members survives
//! because many nodes hold its buckets — the mobile-network-switch superpower
//! (your data survives because the group does).
//!
//! Layering: [`keys`] does the "one layer up" hashing snapcore's store expects
//! (logical name → keyspace point); [`mailbox`] is the post/read flow over a
//! storage [`Backend`](mailbox::Backend) — snapcore's local `Store` in tests, the
//! networked DHT in production (iterative lookup + store-to-closest over a
//! `Transport`, plus cluster replication for Durable — the one network seam not
//! exercised by the unit tests). Sealing and the per-epoch sender keys come from
//! crypto-kit's group layer.

pub mod keys;
pub mod mailbox;

pub use mailbox::{Backend, Lifetime, Mailbox, Posting};

/// The keyspace point type (a 160-bit `snapcore::routing::NodeId`), re-exported so
/// callers don't need a direct snapcore dependency just to name a bucket key.
pub use snapcore::store::Key;

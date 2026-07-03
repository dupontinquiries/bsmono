//! snapclone-piggyback — multi-hop offline-gap-fill relay for snaps.
//!
//! # Problem
//!
//! Snaps are end-to-end encrypted. When sender A wants to send to
//! recipient B and they're both online, delivery is direct over the
//! existing transport layer. But when:
//!
//!   - A is online, B is offline → snap sits in A's outbox waiting
//!   - A goes offline before B comes back → undelivered
//!
//! This crate provides infrastructure for a third option: hand the
//! ciphertext to a mutual contact C who IS reachable to both, with the
//! agreement that C stores-and-forwards when B comes online.
//!
//! # Approach
//!
//! Up to N hops deep (default 5). Each hop appends a signed entry to
//! a chain. Recipient verifies the full chain before accepting:
//!   - Every hop's signature is valid
//!   - No node appears twice in the chain (loop prevention)
//!   - Chain length is within the hop limit
//!
//! # What this crate exposes
//!
//!   - `RelayChain` — the append-only signed chain of hops
//!   - `ChainEntry` — one hop's signed forwarding declaration
//!   - `Storage` trait — pluggable backing store (in-memory, sled, sqlite…)
//!     for the encrypted blobs as they wait to be forwarded
//!   - Verification entry points
//!   - Constants: hop limit, storage budget, eviction policy
//!
//! # What this crate does NOT do
//!
//!   - Actually encrypt/decrypt snaps (snapclone-crypto)
//!   - Discover candidate relay peers (snapclone-peer / contact graph)
//!   - Run the network transport (snapclone-session)
//!   - Decide routing policy beyond "is this hop allowed" (left to
//!     the orchestrator, which sees the full network state)
//!
//! # Threat model summary (full design in docs/TODO.md piggyback entry)
//!
//! Each hop sees:
//!   - The ciphertext blob (opaque — can't decrypt)
//!   - The previous hop's id and the next hop's id (immediate neighbours
//!     only — not the full chain)
//!   - Timing of forward decisions (can infer some connectivity patterns)
//!
//! Recipient sees:
//!   - The full chain (necessary for verification)
//!
//! Strangers (non-contacts) cannot enter the chain — every hop must
//! be a contact-of-contact reachable through the friend graph.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod chain;
pub mod storage;
pub mod limits;

pub use chain::{ChainEntry, RelayChain, ChainError};
pub use storage::{Storage, StorageEntry, StorageError, InMemoryStorage};
pub use limits::{
    MAX_HOP_DEPTH, DEFAULT_STORAGE_BUDGET_BYTES, DEFAULT_BLOB_TTL_HOURS,
};

/// Crate-level error type. Variants are distinct from chain/storage
/// errors so callers can pattern-match on the layer where the failure
/// occurred.
#[derive(Debug, thiserror::Error)]
pub enum PiggybackError {
    /// Problem with the chain (signature, loop, depth, etc.).
    #[error("chain error: {0}")]
    Chain(#[from] ChainError),

    /// Problem with the storage backend.
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    /// Caller-supplied parameter outside accepted bounds.
    #[error("invalid argument: {0}")]
    InvalidArgument(&'static str),
}

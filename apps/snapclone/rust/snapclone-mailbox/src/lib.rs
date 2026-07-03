//! snapclone-mailbox — DHT mailbox rendezvous and deferred transfer.
//!
//! # Deferred Transfer Model (NETWORK_ARCHITECTURE.md §6)
//!
//! The key insight: separate "intent to communicate" from "actual data transfer".
//!
//! ```text
//! Phase 1 — Intent (tiny, always succeeds):
//!   Sender publishes a MailboxEntry to the DHT (< 100 bytes).
//!   Entry contains: snap_id, content_hash, sender_sc_id, size_bytes, ttl.
//!   Cost: one DHT PUT, ~200ms, works even on CGNAT.
//!
//! Phase 2 — Rendezvous (when both peers are reachable):
//!   Recipient polls the mailbox for their sc_id.
//!   Finds the MailboxEntry, initiates a direct connection to fetch the snap.
//!   Only THEN does the bulk transfer happen (on a warm path).
//!
//! Phase 3 — Bulk transfer (only on good paths):
//!   Snap is transferred P2P when both peers have a live direct session.
//!   If no direct path: relay is used as last resort.
//! ```
//!
//! This approach means:
//! - Snaps are never lost even if the recipient is offline at send time
//! - Large media only travels once (no repeated relay retries)
//! - The DHT only carries tiny metadata (< 200 bytes per snap)

pub mod entry;
pub mod store;
pub mod envelope;
pub mod requests;
pub mod requests_ffi;

pub use entry::{MailboxEntry, MailboxId, DeliveryHint, SignedMailboxEntry};
pub use store::MailboxStore;
pub use envelope::{EncryptedEnvelope, fixed_size_envelope};
pub use requests::{
    RequestMailbox, InboundRequest, RequestState, RequestAction, ApplyOutcome,
};

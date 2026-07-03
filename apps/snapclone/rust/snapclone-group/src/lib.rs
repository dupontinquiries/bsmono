//! snapclone-group — group messaging crypto for SnapClone.
//!
//! Implements the sender-key scheme from `docs/GROUP_AND_SERVER_DESIGN.md`:
//! one symmetric content key per message (so media is encrypted ONCE
//! regardless of group size), distributed to each member via a small
//! per-recipient sealed key message, with membership epochs that re-key on
//! every add/remove.
//!
//! # Layout
//!
//! | Module    | Responsibility                                              |
//! |-----------|-------------------------------------------------------------|
//! | `labels`  | centralized, versioned HKDF domain-separation labels (§3.2) |
//! | `ratchet` | the one-way sender-key chain (forward secrecy within epoch) |
//! | `seal`    | per-recipient sender-key seal + AEAD content enc/dec        |
//! | `group`   | group id, membership epochs, add/remove re-key flow         |
//!
//! # What's proven here
//!
//! These are the §3 "careful consideration" safety properties, each a test:
//!   * forward secrecy within an epoch (ratchet can't go backward),
//!   * a removed member cannot read new-epoch content,
//!   * a newly-added member cannot read prior-epoch content,
//!   * tampered ciphertext / wrong key / wrong recipient all fail closed.
//!
//! Built ONLY from vetted primitives already in the workspace lockfile (HKDF,
//! AES-256-GCM, Ed25519, X25519, SHA-256) — no homegrown crypto, no new dep
//! surface. The entry/wire framing and FFI live in later steps (see the design
//! doc §5); this crate is the math, proven correct first.

pub mod dag;
pub mod delivery;
pub mod entry;
pub mod feed;
pub mod ffi;
pub mod floor;
pub mod group;
pub mod sync;
pub mod labels;
pub mod ratchet;
pub mod seal;
pub mod mailbox;

pub use dag::{DagNode, InsertOutcome, NodeId, SnapDag};
pub use delivery::{DeliverySummary, DeliveryTracker, MemberDelivery};
pub use floor::{Floor, FloorConfig, FloorControl};
pub use sync::{SyncConfig, SyncEngine, SyncMessage};
pub use entry::{GroupEntryBody, MessageKind, OpenedEntry, seal_and_sign, verify_and_open};
pub use group::{feed_bucket, GroupId, GroupState, Member};
pub use ratchet::SenderChain;
pub use seal::{
    decrypt_content, encrypt_content, open_sender_key, seal_sender_key, SealError,
};

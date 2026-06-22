//! AppEvent — things that happened in the system.
//!
//! These flow from core → UI. The UI observes events and updates its display.
//! Using an enum instead of dozens of callbacks makes the boundary explicit:
//! every possible event is visible in one place, and UI code can exhaustively
//! match on them.
//!
//! Compare to Qt signals: this is the Rust equivalent but without the
//! per-signal connect() boilerplate.

use crate::{Contact, Peer, Snap, SnapId};

#[derive(Debug, Clone)]
pub enum AppEvent {
    // ── Identity ─────────────────────────────────────────────────────────
    /// App initialised with this stable sc-ID.
    Initialized { sc_id: String },

    // ── Contacts ─────────────────────────────────────────────────────────
    /// Initial contact list loaded from DB at startup.
    ContactsLoaded { contacts: Vec<Contact> },
    /// A contact was added, updated (online status, name, etc.), or saved.
    ContactUpdated { contact: Contact },
    /// A contact was deleted.
    ContactRemoved { id: String },

    // ── Peers (network layer) ─────────────────────────────────────────────
    /// A peer completed the SHAK handshake — now online.
    PeerConnected { peer: Peer },
    /// A peer disconnected (transport closed or timed out).
    PeerDisconnected { id: String },
    /// Network phase update (Connecting / Punching / Relaying / etc.)
    PeerStatusChanged { id: String, phase: String, detail: String },
    /// An inbound stranger requests approval before we exchange keys.
    InboundStrangerRequest { peer: Peer },

    // ── Snaps ─────────────────────────────────────────────────────────────
    /// A new snap arrived from a peer (already decrypted, saved to disk).
    SnapReceived { snap: Snap },
    /// Our outbound snap was delivered to the recipient.
    SnapDelivered { id: SnapId },
    /// An outbound snap is queued (recipient offline).
    SnapQueued { id: SnapId, recipient_id: String },

    // ── Network / NAT ─────────────────────────────────────────────────────
    /// STUN discovered our reflexive address.
    ReflexiveAddressDiscovered { ip: String, port: u16 },
    /// We're behind CGNAT — relay is mandatory.
    CgnatDetected { ip: String },
    /// Network connectivity changed (went online/offline).
    NetworkChanged { online: bool },

    // ── Errors ────────────────────────────────────────────────────────────
    Error { context: String, message: String },
}

//! AppCommand — things the user (or UI) wants to do.
//!
//! Commands flow from UI → core. The core processes them and emits AppEvents
//! in response. This is the entire API surface the UI needs; it doesn't call
//! into subsystems directly.
//!
//! This is the Rust equivalent of AppController's public slots/methods.

use crate::{Contact, SnapId};

#[derive(Debug, Clone)]
pub enum AppCommand {
    // ── Startup / lifecycle ───────────────────────────────────────────────
    Initialize,
    FactoryReset,

    // ── Peer connectivity ─────────────────────────────────────────────────
    /// Connect to a peer by their sc-ID (triggers DHT lookup if needed)
    ConnectPeer { sc_id: String },
    /// Approve an inbound stranger (send them our public key)
    ApprovePeer { id: String },
    /// Deny an inbound stranger (add to blocked list)
    DenyPeer { id: String },

    // ── Snaps ─────────────────────────────────────────────────────────────
    /// Send a snap to a contact. The snap's local_path must exist on disk.
    SendSnap {
        recipient_id: String,
        local_path:   String,
        duration_secs: u32,
    },
    /// Mark a snap as viewed (starts the burn timer)
    MarkSnapViewed { id: SnapId },
    /// Delete a snap and its media file from disk
    DeleteSnap { id: SnapId },

    // ── Contacts ─────────────────────────────────────────────────────────
    /// Persist a contact edit (nickname, notes, avatar)
    SaveContact { contact: Contact },
    /// Remove a contact and revoke their key
    DeleteContact { id: String },

    // ── Settings ─────────────────────────────────────────────────────────
    SetRelayUrl { url: String },
    SetDisplayName { name: String },
}

//! unendlich_id (r197) — the persistent player identity and the default
//! session policy for playtesters. Two jobs:
//!   1. a stable per-install NodeId (the "unendlich-id") with a human
//!      handle, persisted to the settings dir so a player keeps the same
//!      identity across launches (DHT routing + mailbox addressing key);
//!   2. the default session policy: ROLLBACK netcode by default, not
//!      client-server (operator: "rollback mode by default instead
//!      client server default").
//!
//! Pure + deterministic given inputs; the only side effects are the
//! explicit load/save fns. No live networking here — this is the policy
//! and identity layer the transport (dht_join / mp_dht) consumes.

use crate::sim::dht_join::NodeId;

/// How peers stay in sync. Rollback is the default: each peer simulates
/// forward optimistically and rolls back when a remote input arrives for
/// an earlier tick (the engine already has rollback points + the r196
/// ramp reconciliation). Client-server is opt-in for large sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetMode {
    /// Default: peer-to-peer rollback (deterministic lockstep with
    /// optimistic prediction + rollback on late inputs).
    Rollback,
    /// Authoritative host; clients send inputs, receive state. For
    /// sessions too large for all-pairs rollback.
    ClientServer,
}

impl Default for NetMode {
    fn default() -> Self {
        NetMode::Rollback
    }
}

/// A player's stable identity. `handle` is the human name; `node_id` is
/// the DHT/mailbox key derived from it + a per-install salt so two
/// players who pick the same handle still get distinct ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub handle: String,
    pub node_id: NodeId,
    /// Per-install random salt (hex) — persisted so the id is stable.
    pub salt: u64,
}

impl Identity {
    /// Derive a NodeId from a handle + salt. Deterministic: same inputs
    /// always yield the same id (so a player's id is stable across
    /// launches as long as handle+salt persist).
    pub fn derive(handle: &str, salt: u64) -> NodeId {
        let mut h = crate::util::hash_util::mix64(salt ^ 0x7E57_C0DE_1234_5678);
        for b in handle.as_bytes() {
            h = crate::util::hash_util::mix64(h ^ (*b as u64));
        }
        h
    }

    /// Fresh identity for `handle` with a salt drawn from `entropy`
    /// (caller passes a clock/random seed; kept pure here for testing).
    pub fn new(handle: &str, entropy: u64) -> Identity {
        let salt = crate::util::hash_util::mix64(entropy);
        Identity {
            handle: handle.to_string(),
            node_id: Self::derive(handle, salt),
            salt,
        }
    }

    /// Serialize to a tiny JSON-ish line for the settings file.
    pub fn to_line(&self) -> String {
        format!("{}\t{:016x}\t{:016x}", self.handle, self.salt, self.node_id)
    }

    /// Parse a line written by `to_line`. Re-derives node_id and checks
    /// it matches (rejects a tampered/old-format line).
    pub fn from_line(line: &str) -> Option<Identity> {
        let mut it = line.splitn(3, '\t');
        let handle = it.next()?.to_string();
        let salt = u64::from_str_radix(it.next()?.trim(), 16).ok()?;
        let stored = u64::from_str_radix(it.next()?.trim(), 16).ok()?;
        let node_id = Self::derive(&handle, salt);
        if node_id == stored {
            Some(Identity { handle, node_id, salt })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_is_default() {
        assert_eq!(NetMode::default(), NetMode::Rollback);
    }

    #[test]
    fn id_is_stable_for_same_handle_and_salt() {
        let a = Identity::derive("alice", 42);
        let b = Identity::derive("alice", 42);
        assert_eq!(a, b);
        // different salt -> different id even with same handle
        assert_ne!(Identity::derive("alice", 42), Identity::derive("alice", 43));
        // different handle -> different id
        assert_ne!(Identity::derive("alice", 42), Identity::derive("bob", 42));
    }

    #[test]
    fn roundtrip_line() {
        let id = Identity::new("Pathfinder", 0xDEADBEEF);
        let line = id.to_line();
        let back = Identity::from_line(&line).expect("parse");
        assert_eq!(id, back);
    }

    #[test]
    fn tampered_line_rejected() {
        let id = Identity::new("eve", 7);
        let line = id.to_line();
        let parts: Vec<&str> = line.split('\t').collect();
        // corrupt the stored node_id
        let bad = format!("{}\t{}\t{}", parts[0], parts[1], "ffffffffffffffff");
        assert!(Identity::from_line(&bad).is_none());

    }
}

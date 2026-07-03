//! SessionPool — manages all peer sessions.
//!
//! Rust equivalent of NetworkManager's m_sessions map.
//! Provides O(1) lookup by sc-ID and bulk operations.

use crate::session::{PeerSession, SessionEvent, SessionState};
use std::collections::HashMap;
use tracing::debug;

pub struct SessionPool {
    sessions: HashMap<String, PeerSession>,
}

impl SessionPool {
    pub fn new() -> Self { Self { sessions: HashMap::new() } }

    /// Apply an event to a specific peer's session, creating it if needed.
    pub fn apply(&mut self, sc_id: &str, event: SessionEvent) -> SessionState {
        let session = self.sessions
            .entry(sc_id.to_owned())
            .or_insert_with(|| PeerSession::new(sc_id));
        session.apply(event)
    }

    /// True if the peer has a live (connecting/handshaking/connected) session.
    pub fn is_live(&self, sc_id: &str) -> bool {
        self.sessions.get(sc_id).map(|s| s.is_live()).unwrap_or(false)
    }

    /// True if the peer has a usable (fully connected) session.
    pub fn is_connected(&self, sc_id: &str) -> bool {
        self.sessions.get(sc_id).map(|s| s.is_connected()).unwrap_or(false)
    }

    /// Get a specific session.
    pub fn get(&self, sc_id: &str) -> Option<&PeerSession> {
        self.sessions.get(sc_id)
    }

    /// Remove a session (called on explicit disconnect or factory reset).
    pub fn remove(&mut self, sc_id: &str) -> Option<PeerSession> {
        self.sessions.remove(sc_id)
    }

    /// All currently connected peer sc-IDs.
    pub fn connected_ids(&self) -> Vec<&str> {
        self.sessions.values()
            .filter(|s| s.is_connected())
            .map(|s| s.sc_id.as_str())
            .collect()
    }

    /// Discard all sessions (factory reset).
    pub fn clear(&mut self) {
        debug!("SessionPool: cleared {} sessions", self.sessions.len());
        self.sessions.clear();
    }

    pub fn len(&self) -> usize { self.sessions.len() }
    pub fn is_empty(&self) -> bool { self.sessions.is_empty() }
}

impl Default for SessionPool { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionEvent;

    #[test]
    fn pool_tracks_connections() {
        let mut pool = SessionPool::new();
        assert!(!pool.is_live("sc-aabbccdd"));

        pool.apply("sc-aabbccdd", SessionEvent::ConnectInitiated { address: "1.2.3.4:9876".to_owned() });
        assert!(pool.is_live("sc-aabbccdd"));
        assert!(!pool.is_connected("sc-aabbccdd")); // not yet SHAK'd

        pool.apply("sc-aabbccdd", SessionEvent::ShakCompleted { display_name: "Alice".to_owned() });
        assert!(pool.is_connected("sc-aabbccdd"));

        let ids = pool.connected_ids();
        assert!(ids.contains(&"sc-aabbccdd"));
    }

    #[test]
    fn pool_clear_removes_all() {
        let mut pool = SessionPool::new();
        pool.apply("sc-aaaaaaaa", SessionEvent::ConnectInitiated { address: "1.1.1.1:9876".to_owned() });
        pool.apply("sc-bbbbbbbb", SessionEvent::ConnectInitiated { address: "2.2.2.2:9876".to_owned() });
        pool.clear();
        assert!(pool.is_empty());
    }
}

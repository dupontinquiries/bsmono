//! PeerSession — single peer's connection state.
//!
//! Ported from NetworkManager::PeerSession (the C++ inner struct) and
//! the session lifecycle methods: connectToPeer, promoteInboundSession,
//! onTransportConnected, onTransportDisconnected.

use crate::epoch::Epoch;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// How the peer is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TransportKind {
    #[default]
    Unknown,
    DirectTcp,   // WebSocket over direct TCP
    UdpChannel,  // UDP chunked transfer (LAN hole-punch)
    Relay,       // Magic Wormhole relay (CGNAT fallback)
    Inbound,     // Not-yet-promoted inbound connection
}

/// Lifecycle state of a peer session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SessionState {
    #[default]
    Idle,
    Connecting,  // transport dialling
    Handshaking, // SHAK in progress
    Connected,   // SHAK complete, session active
    Relayed,     // connected but via relay
    Disconnected,// graceful close (waiting to reconnect)
    Failed,      // irrecoverable (max retries exceeded)
}

impl SessionState {
    pub fn is_live(self) -> bool {
        matches!(self, Self::Connected | Self::Relayed | Self::Handshaking)
    }
    pub fn is_usable(self) -> bool {
        matches!(self, Self::Connected | Self::Relayed)
    }
}

/// Events that drive the session state machine.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// We initiated a connection to this address.
    ConnectInitiated { address: String },
    /// An inbound connection arrived from this address.
    InboundArrived { address: String },
    /// SHAK handshake succeeded — peer is now identified.
    ShakCompleted { display_name: String },
    /// Connection dropped (transport closed).
    Disconnected { reason: String },
    /// A relay transport was adopted for this peer.
    RelayActivated { relay_url: String },
    /// The peer's address changed (reconnected from different IP).
    AddressChanged { new_address: String },
    /// Error (e.g. SHAK parse failure, encryption error).
    Error { message: String },
}

/// A single peer's session.
#[derive(Debug)]
pub struct PeerSession {
    pub sc_id:        String,
    pub state:        SessionState,
    pub transport:    TransportKind,
    pub address:      Option<String>,
    pub display_name: Option<String>,
    pub relay_url:    Option<String>,
    pub we_initiated: bool,
    pub connected_at: Option<DateTime<Utc>>,
    pub reconnect_count: u32,
    pub epoch:        Epoch,
}

impl PeerSession {
    pub fn new(sc_id: &str) -> Self {
        Self {
            sc_id:           sc_id.to_owned(),
            state:           SessionState::Idle,
            transport:       TransportKind::Unknown,
            address:         None,
            display_name:    None,
            relay_url:       None,
            we_initiated:    false,
            connected_at:    None,
            reconnect_count: 0,
            epoch:           Epoch::default(),
        }
    }

    /// Apply an event and transition the state machine.
    /// Returns the new state.
    pub fn apply(&mut self, event: SessionEvent) -> SessionState {
        debug!("[{}] {:?} + {:?}", self.sc_id, self.state, event);
        let prev = self.state;
        match event {
            SessionEvent::ConnectInitiated { address } => {
                // If a transfer was in-flight, bump epoch to invalidate stale callbacks
                if self.state.is_live() { self.epoch.bump(); }
                self.address      = Some(address);
                self.we_initiated = true;
                self.state        = SessionState::Connecting;
                self.transport    = TransportKind::DirectTcp;
            }
            SessionEvent::InboundArrived { address } => {
                self.address      = Some(address);
                self.we_initiated = false;
                self.state        = SessionState::Connecting;
                self.transport    = TransportKind::Inbound;
            }
            SessionEvent::ShakCompleted { display_name } => {
                self.display_name = Some(display_name.clone());
                self.connected_at = Some(Utc::now());
                self.state        = SessionState::Connected;
                if self.relay_url.is_some() {
                    self.state    = SessionState::Relayed;
                    self.transport = TransportKind::Relay;
                }
                info!("[{}] connected as {:?} via {:?}", self.sc_id, display_name, self.transport);
            }
            SessionEvent::Disconnected { reason } => {
                self.epoch.bump(); // invalidate in-flight callbacks
                self.reconnect_count += 1;
                self.state     = SessionState::Disconnected;
                self.connected_at = None;
                debug!("[{}] disconnected: {}", self.sc_id, reason);
            }
            SessionEvent::RelayActivated { relay_url } => {
                self.relay_url  = Some(relay_url);
                self.transport  = TransportKind::Relay;
                if self.state == SessionState::Connected {
                    self.state  = SessionState::Relayed;
                }
            }
            SessionEvent::AddressChanged { new_address } => {
                self.epoch.bump(); // stale-transfer guard
                self.address = Some(new_address);
                self.state   = SessionState::Connecting;
            }
            SessionEvent::Error { message } => {
                debug!("[{}] error: {}", self.sc_id, message);
                self.state = SessionState::Failed;
            }
        }
        if self.state != prev {
            debug!("[{}] {:?} → {:?}", self.sc_id, prev, self.state);
        }
        self.state
    }

    pub fn is_connected(&self) -> bool { self.state.is_usable() }
    pub fn is_live(&self) -> bool      { self.state.is_live() }
    pub fn current_epoch(&self) -> u32 { self.epoch.current() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_shak_disconnect() {
        let mut s = PeerSession::new("sc-aabbccdd");
        s.apply(SessionEvent::ConnectInitiated { address: "1.2.3.4:9876".to_owned() });
        assert_eq!(s.state, SessionState::Connecting);
        assert!(s.we_initiated);

        s.apply(SessionEvent::ShakCompleted { display_name: "Alice".to_owned() });
        assert_eq!(s.state, SessionState::Connected);
        assert!(s.is_connected());

        let epoch_before = s.current_epoch();
        s.apply(SessionEvent::Disconnected { reason: "timeout".to_owned() });
        assert_eq!(s.state, SessionState::Disconnected);
        assert!(!s.is_connected());
        assert_eq!(s.reconnect_count, 1);
        // Epoch bumped on disconnect
        assert_ne!(s.current_epoch(), epoch_before);
    }

    #[test]
    fn relay_activation() {
        let mut s = PeerSession::new("sc-11223344");
        s.apply(SessionEvent::ConnectInitiated { address: "5.6.7.8:9876".to_owned() });
        s.apply(SessionEvent::ShakCompleted { display_name: "Bob".to_owned() });
        assert_eq!(s.state, SessionState::Connected);

        s.apply(SessionEvent::RelayActivated { relay_url: "wss://relay.example.com".to_owned() });
        assert_eq!(s.state, SessionState::Relayed);
        assert_eq!(s.transport, TransportKind::Relay);
    }

    #[test]
    fn inbound_promoted() {
        let mut s = PeerSession::new("sc-aabbccdd");
        s.apply(SessionEvent::InboundArrived { address: "10.0.0.5:44444".to_owned() });
        assert!(!s.we_initiated);
        assert_eq!(s.transport, TransportKind::Inbound);
    }

    #[test]
    fn address_change_bumps_epoch() {
        let mut s = PeerSession::new("sc-aabbccdd");
        s.apply(SessionEvent::ConnectInitiated { address: "1.1.1.1:9876".to_owned() });
        s.apply(SessionEvent::ShakCompleted { display_name: "C".to_owned() });
        let e0 = s.current_epoch();
        s.apply(SessionEvent::AddressChanged { new_address: "2.2.2.2:9876".to_owned() });
        assert_ne!(s.current_epoch(), e0, "Epoch must bump on address change");
    }
}

//! SessionCoordinator — top-level session manager combining all subsystems.
//!
//! Wraps snapclone-peer SessionPool, RetryBackoff, and TransportSelector
//! into a unified interface for AppController to use.

use crate::{
    backoff::RetryBackoff,
    selector::{TransportChoice, TransportSelector},
};
use snapclone_net_intel::{ReachabilityDb, bins::NetworkBin, scoring::Protocol};
use snapclone_peer::{SessionPool, SessionEvent, SessionState};
use std::collections::HashMap;
use tracing::{debug, info, warn};

pub struct SessionCoordinator {
    pub pool:    SessionPool,
    pub db:      ReachabilityDb,
    backoffs:    HashMap<String, RetryBackoff>,
    relay_url:   Option<String>,
    ws_port:     u16,
}

impl SessionCoordinator {
    pub fn new(relay_url: Option<String>, ws_port: u16) -> Self {
        Self {
            pool:      SessionPool::new(),
            db:        ReachabilityDb::new(),
            backoffs:  HashMap::new(),
            relay_url,
            ws_port,
        }
    }

    /// Called when a direct connection attempt succeeds.
    pub fn on_connected(&mut self, peer_id: &str, addr: &str, rtt_ms: Option<f32>) {
        self.pool.apply(peer_id, SessionEvent::ConnectInitiated { address: addr.to_owned() });
        self.pool.apply(peer_id, SessionEvent::ShakCompleted { display_name: peer_id.to_owned() });
        // Record success in reachability db
        if let Ok(ip) = addr.split(':').next().unwrap_or("").parse() {
            let bin = NetworkBin::of(ip);
            self.db.record_success(peer_id, bin, Protocol::TcpDirect, rtt_ms);
        }
        // Reset backoff on success
        self.backoffs.entry(peer_id.to_owned()).or_insert_with(RetryBackoff::default_session).reset();
        info!("SessionCoordinator: {} connected from {}", peer_id, addr);
    }

    /// Called when a connection attempt fails.
    pub fn on_failed(&mut self, peer_id: &str, addr: &str, protocol: Protocol) {
        if let Ok(ip) = addr.split(':').next().unwrap_or("").parse() {
            let bin = NetworkBin::of(ip);
            self.db.record_failure(peer_id, bin, protocol);
        }
        self.pool.apply(peer_id, SessionEvent::Disconnected { reason: "connect_failed".to_owned() });
        let backoff = self.backoffs.entry(peer_id.to_owned()).or_insert_with(RetryBackoff::default_session);
        let delay = backoff.next();
        debug!("SessionCoordinator: {} failed, retry in {:?}", peer_id, delay);
    }

    /// Called when a peer disconnects.
    pub fn on_disconnected(&mut self, peer_id: &str, reason: &str) {
        self.pool.apply(peer_id, SessionEvent::Disconnected { reason: reason.to_owned() });
    }

    /// Get ordered transport choices for connecting to a peer.
    pub fn transport_choices(
        &self, peer_id: &str, peer_ip: Option<std::net::IpAddr>, is_cgnat: bool
    ) -> Vec<TransportChoice> {
        TransportSelector::ordered_choices(
            &self.db, peer_id, peer_ip, is_cgnat,
            self.relay_url.as_deref(), self.ws_port
        )
    }

    /// Prune stale reachability records (call periodically).
    pub fn gc(&mut self) { self.db.gc(); }
}

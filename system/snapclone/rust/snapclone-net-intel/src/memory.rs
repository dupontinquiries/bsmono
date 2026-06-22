//! ReachabilityDb — persists path scores, keyed by (peer_id, bin, protocol).

use crate::scoring::{PathScore, Protocol};
use crate::bins::NetworkBin;
use std::collections::HashMap;
use tracing::debug;

type Key = (String, NetworkBin, Protocol);

#[derive(Debug, Default)]
pub struct ReachabilityDb {
    records: HashMap<Key, PathRecord>,
}

#[derive(Debug, Clone)]
pub struct PathRecord {
    pub score: PathScore,
}

impl ReachabilityDb {
    pub fn new() -> Self { Self::default() }

    pub fn record_success(&mut self, peer: &str, bin: NetworkBin, proto: Protocol, rtt_ms: Option<f32>) {
        let entry = self.records.entry((peer.to_owned(), bin.clone(), proto)).or_default();
        entry.score.record_success(rtt_ms);
        debug!("ReachabilityDb: success {} {:?} {:?} conf={:.2}",
               peer, bin, proto, entry.score.confidence());
    }

    pub fn record_failure(&mut self, peer: &str, bin: NetworkBin, proto: Protocol) {
        let entry = self.records.entry((peer.to_owned(), bin.clone(), proto)).or_default();
        entry.score.record_failure();
        debug!("ReachabilityDb: failure {} {:?} {:?} conf={:.2}",
               peer, bin, proto, entry.score.confidence());
    }

    /// Best protocol to try first for a given peer+bin (highest warm confidence).
    pub fn best_protocol(&self, peer: &str, bin: &NetworkBin) -> Option<Protocol> {
        [Protocol::Udp, Protocol::TcpDirect, Protocol::Ipv6, Protocol::TcpRelay]
            .iter()
            .filter_map(|&proto| {
                let key = (peer.to_owned(), bin.clone(), proto);
                self.records.get(&key).map(|r| (proto, r.score.confidence()))
            })
            .filter(|(_, c)| *c > 0.4)
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .map(|(proto, _)| proto)
    }

    /// True if this path should be skipped (negative cache still fresh).
    pub fn is_blocked(&self, peer: &str, bin: &NetworkBin, proto: Protocol) -> bool {
        self.records.get(&(peer.to_owned(), bin.clone(), proto))
            .map(|r| r.score.is_negcached())
            .unwrap_or(false)
    }

    /// Prune records with no data and decayed entries.
    pub fn gc(&mut self) {
        self.records.retain(|_, r| {
            // Keep if any data in last 7 days
            let any_recent = r.score.last_success
                .or(r.score.last_failure)
                .map(|t| chrono::Utc::now().signed_duration_since(t).num_days() < 7)
                .unwrap_or(false);
            any_recent
        });
    }
}

impl std::default::Default for PathRecord {
    fn default() -> Self { Self { score: PathScore::new() } }
}

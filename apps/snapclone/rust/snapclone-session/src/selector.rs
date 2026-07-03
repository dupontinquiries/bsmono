//! Transport selector — picks the best transport for a peer connection attempt.
//!
//! Uses ReachabilityDb to order transports by historical success rate.
//! Warm paths (recently successful) are tried first; negatively cached paths
//! are skipped entirely until the negative cache expires.

use snapclone_net_intel::{ReachabilityDb, bins::NetworkBin, scoring::Protocol, hints::hints_for};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// The transport to use for a specific attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransportChoice {
    /// UDP hole punch then TCP WebSocket on same port.
    UdpPunchThenTcp { peer_addr: String, punch_port: u16 },
    /// Direct TCP WebSocket without prior UDP punch.
    DirectTcp       { peer_addr: String, ws_port: u16 },
    /// Request via relay server.
    Relay           { relay_url: String },
    /// Leave a DHT mailbox entry and wait for peer to come online.
    DhtMailbox,
}

pub struct TransportSelector;

impl TransportSelector {
    /// Pick an ordered list of transports to try for a peer.
    /// The list is ordered: best-first based on history + current NAT state.
    pub fn ordered_choices(
        db:        &ReachabilityDb,
        peer_id:   &str,
        peer_ip:   Option<IpAddr>,
        is_cgnat:  bool,
        relay_url: Option<&str>,
        ws_port:   u16,
    ) -> Vec<TransportChoice> {
        let mut choices = Vec::new();

        if is_cgnat {
            // CGNAT: skip all direct attempts, go straight to mailbox+relay
            choices.push(TransportChoice::DhtMailbox);
            if let Some(url) = relay_url {
                choices.push(TransportChoice::Relay { relay_url: url.to_owned() });
            }
            return choices;
        }

        let Some(ip) = peer_ip else {
            // No IP known: mailbox only
            choices.push(TransportChoice::DhtMailbox);
            return choices;
        };

        let bin  = NetworkBin::of(ip);
        let addr = ip.to_string();
        let hints = hints_for(db, peer_id, ip, is_cgnat);

        // Translate network-intel hints to transport choices
        for strategy in hints.strategies {
            use snapclone_net_intel::hints::TraversalStrategy;
            match strategy {
                TraversalStrategy::UdpPunch { .. } => {
                    choices.push(TransportChoice::UdpPunchThenTcp {
                        peer_addr: addr.clone(),
                        punch_port: ws_port,
                    });
                }
                TraversalStrategy::TcpDirect { .. } => {
                    choices.push(TransportChoice::DirectTcp {
                        peer_addr: addr.clone(),
                        ws_port,
                    });
                }
                TraversalStrategy::MailboxRendezvous => {
                    choices.push(TransportChoice::DhtMailbox);
                }
                TraversalStrategy::Relay { relay_url } => {
                    let url = if relay_url.is_empty() {
                        relay_url.as_str().to_owned()
                    } else if let Some(u) = relay_url {
                        u.to_owned()
                    } else { continue };
                    choices.push(TransportChoice::Relay { relay_url: url });
                }
                _ => {}
            }
        }

        if choices.is_empty() {
            choices.push(TransportChoice::DirectTcp { peer_addr: addr, ws_port });
            choices.push(TransportChoice::DhtMailbox);
        }
        choices
    }
}

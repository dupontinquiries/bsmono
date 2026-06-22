//! High-level NAT traversal coordinator.
//!
//! This is the Rust equivalent of NatTraversal (the Qt class). It owns the
//! UDP socket, manages the STUN state machine, and emits events for the
//! caller (Qt or otherwise) to react to.
//!
//! # Usage (from Rust / tokio)
//!
//! ```rust,no_run
//! use snapclone_nat::{NatTraversal, NatConfig, NatEvent};
//!
//! let (mut nat, mut events) = NatTraversal::new(NatConfig::default());
//! nat.start().await.unwrap();
//!
//! while let Some(evt) = events.recv().await {
//!     match evt {
//!         NatEvent::ReflexiveAddress { ip, port } => {
//!             println!("Public address: {}:{}", ip, port);
//!         }
//!         NatEvent::CgnatDetected { ip } => {
//!             println!("Behind CGNAT at {} — relay required", ip);
//!         }
//!         NatEvent::PunchSuccess { peer } => {
//!             println!("Hole punch succeeded to {}", peer);
//!         }
//!         _ => {}
//!     }
//! }
//! ```

use crate::cgnat::{AddrClass};
use crate::stun::{self, ReflexiveAddr, TransactionId};
use anyhow::Result;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{info, warn};

/// Configuration matching NatTraversal constants in Timings.hpp
pub struct NatConfig {
    /// Local UDP port to bind for punching (9876 in C++).
    pub local_port:     u16,
    /// STUN servers to try in order.
    pub stun_servers:   Vec<String>,
    /// Timeout per STUN attempt.
    pub stun_timeout:   Duration,
    /// UDP hole-punch burst configuration.
    pub punch_burst:    u32,
    pub punch_interval: Duration,
    pub punch_timeout:  Duration,
}

impl Default for NatConfig {
    fn default() -> Self {
        Self {
            local_port:     9876,
            stun_servers:   vec![
                "stun.l.google.com:19302".to_owned(),
                "stun1.l.google.com:19302".to_owned(),
                "stun.cloudflare.com:3478".to_owned(),
            ],
            stun_timeout:   Duration::from_secs(4),
            punch_burst:    8,
            punch_interval: Duration::from_millis(250),
            punch_timeout:  Duration::from_secs(10),
        }
    }
}

/// Events emitted by NatTraversal — equivalent to Qt signals.
#[derive(Debug, Clone)]
pub enum NatEvent {
    /// STUN succeeded — our public reflexive address.
    ReflexiveAddress { ip: IpAddr, port: u16 },
    /// Reflexive address is in CGNAT range — relay is mandatory.
    CgnatDetected { ip: IpAddr },
    /// UDP hole punch to a specific peer succeeded.
    PunchSuccess { peer: SocketAddr },
    /// STUN failed after all servers exhausted.
    StunFailed { reason: String },
}

pub struct NatTraversal {
    config: NatConfig,
    tx:     mpsc::Sender<NatEvent>,
}

impl NatTraversal {
    pub fn new(config: NatConfig) -> (Self, mpsc::Receiver<NatEvent>) {
        let (tx, rx) = mpsc::channel(32);
        (Self { config, tx }, rx)
    }

    /// Run STUN discovery on the local punch socket.
    /// Equivalent to NatTraversal::start() + the STUN state machine.
    pub async fn discover(&self) -> Result<ReflexiveAddr> {
        // Bind the punch socket (ShareAddress equivalent: SO_REUSEADDR)
        let socket = UdpSocket::bind(
            format!("0.0.0.0:{}", self.config.local_port)
        ).await?;
        socket.set_broadcast(true)?;

        let tx_id = TransactionId::random();
        let request = stun::build_request(&tx_id);

        for server in &self.config.stun_servers {
            // Resolve STUN server
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host(server.as_str())
                .await?
                .filter(|a| a.is_ipv4())
                .collect();
            let Some(stun_addr) = addrs.first().copied() else {
                warn!("STUN: could not resolve {}", server);
                continue;
            };

            socket.send_to(&request, stun_addr).await?;

            let mut buf = vec![0u8; 512];
            match timeout(self.config.stun_timeout, socket.recv_from(&mut buf)).await {
                Ok(Ok((n, _from))) => {
                    match stun::parse_response(&buf[..n], &tx_id) {
                        Ok(addr) => {
                            let cls = AddrClass::of(addr.ip);
                            info!("STUN reflexive: {}:{} ({:?})", addr.ip, addr.port, cls);
                            let _ = self.tx.send(NatEvent::ReflexiveAddress {
                                ip: addr.ip, port: addr.port
                            }).await;
                            if cls.needs_relay() {
                                let _ = self.tx.send(NatEvent::CgnatDetected {
                                    ip: addr.ip
                                }).await;
                            }
                            return Ok(addr);
                        }
                        Err(e) => warn!("STUN parse error from {}: {}", server, e),
                    }
                }
                Ok(Err(e)) => warn!("STUN recv error from {}: {}", server, e),
                Err(_) => warn!("STUN timeout from {}", server),
            }
        }

        let _ = self.tx.send(NatEvent::StunFailed {
            reason: "All STUN servers exhausted".to_owned()
        }).await;
        anyhow::bail!("STUN failed: all servers exhausted");
    }

    /// Attempt UDP hole punching to a peer.
    pub async fn punch(&self, peer: SocketAddr) -> Result<bool> {
        let socket = UdpSocket::bind(
            format!("0.0.0.0:{}", self.config.local_port)
        ).await?;
        let cfg = crate::punch::PunchConfig {
            burst_count:    self.config.punch_burst,
            burst_interval: self.config.punch_interval,
            total_timeout:  self.config.punch_timeout,
        };
        match crate::punch::run(&socket, peer, &cfg).await? {
            crate::punch::PunchResult::Success { from } => {
                let _ = self.tx.send(NatEvent::PunchSuccess { peer: from }).await;
                Ok(true)
            }
            crate::punch::PunchResult::Timeout => Ok(false),
        }
    }
}

//! UDP hole-punching state machine.
//!
//! Ported from NatTraversal.cpp :: startPunch() / onPunchTimer() / onPunchReadyRead().
//!
//! Strategy (simultaneous open):
//!   Both peers send kPunchCount UDP datagrams to each other's reflexive address
//!   every kPunchIntervalMs. Most NATs create a pinhole after seeing the first
//!   outbound packet and then allow the inbound reply. After kPunchCount bursts,
//!   if no reply has been received, the punch is considered failed.
//!
//! The C++ implementation uses QTimer + QUdpSocket. This Rust version uses
//! tokio::time and tokio::net::UdpSocket for async operation, making it
//! suitable for use without a Qt event loop.

use anyhow::Result;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::{interval, Instant};
use tracing::{debug, info, warn};

/// Parameters tunable via NatTraversal.hpp constants.
pub struct PunchConfig {
    /// How many UDP packets to send per burst.
    pub burst_count:    u32,
    /// Interval between individual packets in a burst.
    pub burst_interval: Duration,
    /// Total timeout before giving up.
    pub total_timeout:  Duration,
}

impl Default for PunchConfig {
    fn default() -> Self {
        Self {
            burst_count:    8,
            burst_interval: Duration::from_millis(250),
            total_timeout:  Duration::from_secs(10),
        }
    }
}

/// Result of a hole-punch attempt.
#[derive(Debug, Clone)]
pub enum PunchResult {
    /// Punch succeeded — reply received from the peer.
    Success { from: SocketAddr },
    /// Punch timed out without receiving any reply.
    Timeout,
}

/// Run a UDP hole-punch attempt against `peer_addr` using `socket`.
///
/// Sends `config.burst_count` keepalive datagrams (single zero byte, same
/// as the C++ implementation) then listens for any datagram from `peer_addr`.
/// The calling code is responsible for binding `socket` to the local punch port
/// before calling this function.
pub async fn run(
    socket: &UdpSocket,
    peer_addr: SocketAddr,
    config: &PunchConfig,
) -> Result<PunchResult> {
    let deadline = Instant::now() + config.total_timeout;
    let mut ticker = interval(config.burst_interval);
    let mut sent = 0u32;
    let mut recv_buf = [0u8; 512];

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if sent < config.burst_count {
                    match socket.send_to(&[0u8], peer_addr).await {
                        Ok(_) => {
                            sent += 1;
                            debug!("punch: sent {}/{} to {}", sent, config.burst_count, peer_addr);
                        }
                        Err(e) => warn!("punch: send error: {}", e),
                    }
                }
                if Instant::now() >= deadline {
                    info!("punch: timeout after {} packets to {}", sent, peer_addr);
                    return Ok(PunchResult::Timeout);
                }
            }
            result = socket.recv_from(&mut recv_buf) => {
                match result {
                    Ok((_, from)) if from == peer_addr => {
                        info!("punch: success — reply from {}", from);
                        return Ok(PunchResult::Success { from });
                    }
                    Ok((_, from)) => {
                        debug!("punch: ignoring packet from unexpected {}", from);
                    }
                    Err(e) => warn!("punch: recv error: {}", e),
                }
            }
        }
    }
}

/// CGNAT keepalive: sends a zero-byte UDP packet to keep the NAT mapping alive.
/// The C++ fires this every kCgnatKeepaliveMs (30 seconds by default).
/// Call from a tokio::spawn'd task with a select! on a shutdown channel.
pub async fn keepalive_loop(
    socket: &UdpSocket,
    target: SocketAddr,
    interval_ms: u64,
) {
    let mut ticker = interval(Duration::from_millis(interval_ms));
    loop {
        ticker.tick().await;
        if let Err(e) = socket.send_to(&[0u8], target).await {
            warn!("cgnat keepalive: send failed: {}", e);
        } else {
            debug!("cgnat keepalive: sent to {}", target);
        }
    }
}

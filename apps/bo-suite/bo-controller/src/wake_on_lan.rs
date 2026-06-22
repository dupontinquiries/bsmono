// src/wake_on_lan.rs
#![allow(dead_code)]
//
// Wake-on-LAN packet generator + per-MAC throttling.
//
// When the anticipator's confidence on `SwitchDevice` toward a
// specific device is high AND that device has been unreachable
// recently, we send a WoL magic packet. WoL-capable B&O devices
// (Stage, Balance, Emerge, Level, A5/A9) wake in ~3-5 s. By pre-
// waking on prediction, the user-visible "tap → device responds"
// latency disappears when our guess is correct.
//
// The magic packet format is the standard ratified by AMD in the 90s:
//   6 bytes of 0xFF, followed by 16 repetitions of the target MAC.
//   Sent as a UDP broadcast to port 9 (the discard service — the
//   port doesn't matter; routers + NICs sniff for the magic pattern
//   in any UDP payload).
//
// We rate-limit per-MAC because some routers de-prioritize the
// broadcast if it floods, and waking a device every few seconds when
// it's intentionally off is rude. One packet every 30s per MAC is
// plenty — the device either woke from the first one or it isn't
// reachable anyway.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

pub const WAKE_THROTTLE: Duration = Duration::from_secs(30);
pub const WOL_PORT: u16 = 9;

/// Construct the 102-byte magic packet for a given MAC. Validates the
/// MAC parses to 6 octets; returns None for malformed input.
pub fn magic_packet(mac: &str) -> Option<Vec<u8>> {
    let bytes = parse_mac(mac)?;
    let mut packet = Vec::with_capacity(6 + 16 * 6);
    packet.extend_from_slice(&[0xFF; 6]);
    for _ in 0..16 {
        packet.extend_from_slice(&bytes);
    }
    Some(packet)
}

/// Parse "AA:BB:CC:DD:EE:FF" (or with hyphens / no separator). Returns
/// 6 octets on success.
pub fn parse_mac(input: &str) -> Option<[u8; 6]> {
    let cleaned: String = input.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    if cleaned.len() != 12 { return None; }
    let mut out = [0u8; 6];
    for i in 0..6 {
        out[i] = u8::from_str_radix(&cleaned[i*2..i*2+2], 16).ok()?;
    }
    Some(out)
}

/// Send a magic packet to the broadcast address on `port` (default 9).
/// Uses a UDP socket bound to 0.0.0.0:0 with broadcast enabled.
/// Returns Ok(()) on send success, Err on socket-level failures.
pub fn send_magic(mac: &str, port: u16) -> Result<(), String> {
    let packet = magic_packet(mac).ok_or_else(|| format!("invalid MAC: {}", mac))?;
    let sock = UdpSocket::bind("0.0.0.0:0")
        .map_err(|e| format!("bind: {}", e))?;
    sock.set_broadcast(true)
        .map_err(|e| format!("set_broadcast: {}", e))?;
    let dest = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)), port);
    sock.send_to(&packet, dest)
        .map_err(|e| format!("send: {}", e))?;
    Ok(())
}

/// Per-MAC throttler. Tracks the last wake time for each MAC; checks
/// reject sends within WAKE_THROTTLE.
#[derive(Debug, Default)]
pub struct WakeThrottler {
    last_wake: HashMap<String, Instant>,
}

#[derive(Debug, PartialEq)]
pub enum WakeDecision {
    Send,
    Throttled { last_at: Instant },
}

impl WakeThrottler {
    pub fn new() -> Self { Self::default() }

    /// Should we wake this MAC right now?
    pub fn check(&self, mac: &str, now: Instant) -> WakeDecision {
        match self.last_wake.get(mac) {
            Some(t) if now.duration_since(*t) < WAKE_THROTTLE => {
                WakeDecision::Throttled { last_at: *t }
            }
            _ => WakeDecision::Send,
        }
    }

    /// Record that we just sent a wake packet for this MAC.
    pub fn note_sent(&mut self, mac: &str, now: Instant) {
        self.last_wake.insert(mac.to_string(), now);
    }

    /// How many MACs have been pinged in the last WAKE_THROTTLE? Used
    /// for diagnostic + sanity bounds.
    pub fn recent_count(&self, now: Instant) -> usize {
        self.last_wake.values()
            .filter(|t| now.duration_since(**t) < WAKE_THROTTLE)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mac_accepts_colon_form() {
        let m = parse_mac("AA:BB:CC:DD:EE:FF").unwrap();
        assert_eq!(m, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn parse_mac_accepts_hyphen_form() {
        let m = parse_mac("aa-bb-cc-dd-ee-ff").unwrap();
        assert_eq!(m, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn parse_mac_accepts_no_separator() {
        let m = parse_mac("AABBCCDDEEFF").unwrap();
        assert_eq!(m, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn parse_mac_rejects_short() {
        assert!(parse_mac("AA:BB:CC").is_none());
    }

    #[test]
    fn parse_mac_rejects_nonhex() {
        assert!(parse_mac("ZZ:YY:XX:WW:VV:UU").is_none());
    }

    #[test]
    fn magic_packet_length_is_102() {
        let p = magic_packet("AA:BB:CC:DD:EE:FF").unwrap();
        assert_eq!(p.len(), 102);
    }

    #[test]
    fn magic_packet_starts_with_six_ff() {
        let p = magic_packet("AA:BB:CC:DD:EE:FF").unwrap();
        assert_eq!(&p[..6], &[0xFF; 6]);
    }

    #[test]
    fn magic_packet_repeats_mac_16x() {
        let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let p = magic_packet("AA:BB:CC:DD:EE:FF").unwrap();
        for i in 0..16 {
            let start = 6 + i * 6;
            assert_eq!(&p[start..start+6], &mac, "rep {} mismatch", i);
        }
    }

    #[test]
    fn throttler_allows_first_wake() {
        let t = WakeThrottler::new();
        assert_eq!(t.check("AA:BB:CC:DD:EE:FF", Instant::now()), WakeDecision::Send);
    }

    #[test]
    fn throttler_blocks_immediate_resend() {
        let mut t = WakeThrottler::new();
        let now = Instant::now();
        t.note_sent("AA:BB", now);
        match t.check("AA:BB", now + Duration::from_secs(1)) {
            WakeDecision::Throttled { .. } => {},
            _ => panic!("expected throttle"),
        }
    }

    #[test]
    fn throttler_allows_after_window() {
        let mut t = WakeThrottler::new();
        let now = Instant::now();
        t.note_sent("AA:BB", now);
        let later = now + WAKE_THROTTLE + Duration::from_secs(1);
        assert_eq!(t.check("AA:BB", later), WakeDecision::Send);
    }

    #[test]
    fn throttler_independent_per_mac() {
        let mut t = WakeThrottler::new();
        let now = Instant::now();
        t.note_sent("AA:BB", now);
        assert_eq!(t.check("CC:DD", now + Duration::from_secs(1)), WakeDecision::Send);
    }

    #[test]
    fn recent_count_reflects_recent_sends() {
        let mut t = WakeThrottler::new();
        let now = Instant::now();
        t.note_sent("AA", now);
        t.note_sent("BB", now);
        assert_eq!(t.recent_count(now), 2);
        let later = now + WAKE_THROTTLE * 2;
        assert_eq!(t.recent_count(later), 0);
    }
}

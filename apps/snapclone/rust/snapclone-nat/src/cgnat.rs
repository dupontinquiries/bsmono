//! CGNAT detection — RFC 6598 shared address space (100.64.0.0/10).
//!
//! Ported from NatTraversal.cpp :: STUN reflexive address check.
//! Carrier-grade NAT is used by AT&T mobile hotspots, Comcast, and many
//! others. When detected, direct peer connections are impossible and relay
//! is mandatory.

use std::net::IpAddr;

/// Returns true if the address is in the RFC 6598 CGNAT range 100.64.0.0/10.
///
/// This covers 100.64.0.0 – 100.127.255.255 (4,194,304 addresses).
/// Peers on CGNAT cannot be reached directly via UPnP or hole-punching.
pub fn is_cgnat(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            a == 100 && b >= 64 && b <= 127
        }
        IpAddr::V6(_) => false, // CGNAT is IPv4 only
    }
}

/// Returns true if the address is in any RFC 1918 private range.
/// Private addresses cannot be announced as public endpoints.
pub fn is_private(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            a == 10
                || (a == 172 && b >= 16 && b <= 31)
                || (a == 192 && b == 168)
                || a == 127  // loopback
        }
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// Classify an IP address for NAT traversal purposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddrClass {
    /// Public routable address — announce to DHT and try direct connection.
    Public,
    /// RFC 6598 CGNAT — relay is mandatory.
    Cgnat,
    /// RFC 1918 private — LAN only, do not announce publicly.
    Private,
    /// Loopback — testing only.
    Loopback,
}

impl AddrClass {
    pub fn of(addr: IpAddr) -> Self {
        if is_cgnat(addr)    { return AddrClass::Cgnat;    }
        if is_private(addr)  { return AddrClass::Private;  }
        if addr.is_loopback(){ return AddrClass::Loopback; }
        AddrClass::Public
    }

    pub fn needs_relay(&self) -> bool {
        matches!(self, AddrClass::Cgnat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn cgnat_range() {
        assert!(is_cgnat(Ipv4Addr::new(100, 64,  0, 1).into()));
        assert!(is_cgnat(Ipv4Addr::new(100, 127, 255, 255).into()));
        assert!(!is_cgnat(Ipv4Addr::new(100, 128, 0, 0).into()));
        assert!(!is_cgnat(Ipv4Addr::new(100, 63,  0, 0).into()));
    }

    #[test]
    fn private_ranges() {
        assert!(is_private(Ipv4Addr::new(10, 0, 0, 1).into()));
        assert!(is_private(Ipv4Addr::new(172, 16, 0, 1).into()));
        assert!(is_private(Ipv4Addr::new(192, 168, 1, 1).into()));
        assert!(!is_private(Ipv4Addr::new(8, 8, 8, 8).into()));
    }

    #[test]
    fn addr_class_needs_relay() {
        assert!(AddrClass::of(Ipv4Addr::new(100, 72, 0, 1).into()).needs_relay());
        assert!(!AddrClass::of(Ipv4Addr::new(1, 2, 3, 4).into()).needs_relay());
    }
}

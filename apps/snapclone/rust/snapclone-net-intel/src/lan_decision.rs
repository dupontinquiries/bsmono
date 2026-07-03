//! LAN connection-attempt decision — mirrors `NetworkManager::connectToPeer`'s
//! address-class + failure-count guard, as pure tested logic.
//!
//! `connectToPeer` decides, per outbound attempt, whether to actually try a
//! LAN-direct WebSocket to a peer's address or to skip straight to the
//! relay/rendezvous path. That decision governs whether a queued snap ever
//! connects, so it's worth locking down with tests rather than leaving it
//! tangled in Qt members. The rules (from the C++):
//!
//! - **Loopback / our own address** → never connect (would loop).
//! - **Link-local (169.254/16), Multicast, Broadcast, Invalid** → REFUSE: these
//!   are upstream source bugs / unroutable; the C++ rejects them outright.
//! - **CGNAT (100.64/10)** → STILL attempt (info-only). Two CGNAT peers can
//!   reach each other via a hairpinning ISP or fall back to relay downstream;
//!   `connectToPeer` does NOT skip here, it just logs. (The relay preference is
//!   a downstream diagnostics decision, not this guard's.)
//! - **RFC1918 LAN (10/8, 172.16/12, 192.168/16)** → attempt LAN, UNLESS we've
//!   already failed this peer's LAN address `threshold` times this session
//!   (AP client-isolation almost certainly on) → skip to relay. The threshold
//!   is context-aware: on cellular it's 1 (LAN literally cannot work over a
//!   cellular interface, so don't waste a second attempt); on wifi/ethernet/
//!   unknown it's the permissive default (transient drops shouldn't lock us out).
//! - **Public** → attempt direct (it's routable).
//!
//! The C++ keeps the per-peer failure counts and connection type as members;
//! this module is the stateless decision over those inputs.

/// Address class the decision branches on (mirrors AddressClass::Class).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrClass {
    Loopback,
    LinkLocal,
    Multicast,
    Broadcast,
    Invalid,
    Rfc1918,
    Cgnat,
    Public,
}

/// The network interface we're currently on — affects the LAN threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnType {
    Wifi,
    Ethernet,
    Cellular,
    Unknown,
}

/// What to do with this outbound connection attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectDecision {
    /// Attempt a direct connection to the address.
    AttemptDirect,
    /// Don't attempt LAN/direct; fall back to relay/rendezvous. (Carries why,
    /// for logging.)
    SkipToRelay(SkipReason),
    /// Never connect to this address (loopback/self/link-local).
    Refuse(RefuseReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Too many prior LAN failures this session → AP client isolation likely.
    LanIsolationSuspected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefuseReason {
    LoopbackOrSelf,
    /// Link-local / multicast / broadcast / invalid — unroutable, upstream bug.
    Unroutable,
}

/// Default LAN failure threshold before we treat the AP as client-isolated.
pub const DEFAULT_LAN_THRESHOLD: u32 = 2;
/// On cellular, a single LAN failure is enough — LAN can't work over cellular.
pub const CELLULAR_LAN_THRESHOLD: u32 = 1;

/// The effective LAN failure threshold for a connection type.
pub fn lan_threshold(conn: ConnType) -> u32 {
    match conn {
        ConnType::Cellular => CELLULAR_LAN_THRESHOLD,
        _ => DEFAULT_LAN_THRESHOLD,
    }
}

/// The core decision. `is_self` is true if the address resolves to us (hairpin /
/// our own reflexive address). `lan_failures` is how many times this peer's LAN
/// address has failed to connect THIS session.
pub fn decide(
    class: AddrClass,
    conn: ConnType,
    is_self: bool,
    lan_failures: u32,
) -> ConnectDecision {
    // Self / loopback first (C++ checks loopback class, then self-id).
    if is_self || class == AddrClass::Loopback {
        return ConnectDecision::Refuse(RefuseReason::LoopbackOrSelf);
    }
    match class {
        AddrClass::LinkLocal | AddrClass::Multicast
        | AddrClass::Broadcast | AddrClass::Invalid =>
            ConnectDecision::Refuse(RefuseReason::Unroutable),
        // CGNAT: the C++ does NOT skip here — it logs and still attempts.
        // The relay preference is a downstream decision, not this guard's.
        AddrClass::Cgnat => ConnectDecision::AttemptDirect,
        AddrClass::Rfc1918 => {
            if lan_failures >= lan_threshold(conn) {
                ConnectDecision::SkipToRelay(SkipReason::LanIsolationSuspected)
            } else {
                ConnectDecision::AttemptDirect
            }
        }
        AddrClass::Public => ConnectDecision::AttemptDirect,
        AddrClass::Loopback => unreachable!("handled above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_and_loopback_refused() {
        assert_eq!(decide(AddrClass::Public, ConnType::Wifi, true, 0),
                   ConnectDecision::Refuse(RefuseReason::LoopbackOrSelf));
        assert_eq!(decide(AddrClass::Loopback, ConnType::Wifi, false, 0),
                   ConnectDecision::Refuse(RefuseReason::LoopbackOrSelf));
    }

    #[test]
    fn link_local_and_friends_refused() {
        for c in [AddrClass::LinkLocal, AddrClass::Multicast,
                  AddrClass::Broadcast, AddrClass::Invalid] {
            assert_eq!(decide(c, ConnType::Wifi, false, 0),
                       ConnectDecision::Refuse(RefuseReason::Unroutable),
                       "class {:?} should be refused as unroutable", c);
        }
    }

    #[test]
    fn cgnat_still_attempts_matches_cpp() {
        // The C++ logs a hint but does NOT skip — it still tries the address.
        assert_eq!(decide(AddrClass::Cgnat, ConnType::Wifi, false, 0),
                   ConnectDecision::AttemptDirect);
    }

    #[test]
    fn public_attempts_direct() {
        assert_eq!(decide(AddrClass::Public, ConnType::Wifi, false, 0),
                   ConnectDecision::AttemptDirect);
    }

    #[test]
    fn lan_first_attempt_tries_direct() {
        assert_eq!(decide(AddrClass::Rfc1918, ConnType::Wifi, false, 0),
                   ConnectDecision::AttemptDirect);
        assert_eq!(decide(AddrClass::Rfc1918, ConnType::Wifi, false, 1),
                   ConnectDecision::AttemptDirect); // still under threshold 2
    }

    #[test]
    fn lan_skips_after_threshold_on_wifi() {
        // wifi threshold is 2 → the 2nd prior failure trips the skip.
        assert_eq!(decide(AddrClass::Rfc1918, ConnType::Wifi, false, 2),
                   ConnectDecision::SkipToRelay(SkipReason::LanIsolationSuspected));
    }

    #[test]
    fn lan_skips_after_single_failure_on_cellular() {
        // cellular threshold is 1 → one prior failure is enough.
        assert_eq!(decide(AddrClass::Rfc1918, ConnType::Cellular, false, 1),
                   ConnectDecision::SkipToRelay(SkipReason::LanIsolationSuspected));
        // but the very first cellular LAN attempt still tries (count 0).
        assert_eq!(decide(AddrClass::Rfc1918, ConnType::Cellular, false, 0),
                   ConnectDecision::AttemptDirect);
    }

    #[test]
    fn threshold_by_conn_type() {
        assert_eq!(lan_threshold(ConnType::Cellular), 1);
        assert_eq!(lan_threshold(ConnType::Wifi), 2);
        assert_eq!(lan_threshold(ConnType::Ethernet), 2);
        assert_eq!(lan_threshold(ConnType::Unknown), 2);
    }

    #[test]
    fn self_check_beats_class() {
        // even a LAN address that is actually us → refuse, not attempt.
        assert_eq!(decide(AddrClass::Rfc1918, ConnType::Wifi, true, 0),
                   ConnectDecision::Refuse(RefuseReason::LoopbackOrSelf));
    }
}

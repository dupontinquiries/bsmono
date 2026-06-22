//! `sim::net::rendezvous_join` — turn a DHT-mailbox Accept into a hole-punch.
//!
//! The join handshake in [`dht_mailbox`](crate::sim::net::dht_mailbox) ends with
//! [`JoinOutcome::Accepted`] carrying the host's `addr` — the one place the host's
//! address ever travels, revealed only after acceptance (the IP-protection
//! invariant). That address is exactly the input the
//! [`HolePunch`](crate::sim::net_rendezvous::HolePunch) FSM needs: remote
//! candidates in a [`MailboxBlob`]. This module composes the two ends so a join
//! flows `mailbox Accept → UDP hole-punch → direct connection`.
//!
//! Pure: address parsing + FSM setup only. The DHT put/get and the UDP sockets
//! stay bin glue; here we just bridge the accepted address into the punch.

use crate::sim::net::dht_mailbox::JoinOutcome;
use crate::sim::net_rendezvous::{
    candidate_priority, CandKind, Candidate, HolePunch, MailboxBlob, PunchConfig, BLOB_VERSION,
};

/// RFC1918 / loopback / link-local — addresses that are reachable on the same
/// network rather than across the internet.
fn is_local(ip: [u8; 4]) -> bool {
    matches!(ip, [10, ..] | [127, ..] | [169, 254, ..] | [192, 168, ..])
        || (ip[0] == 172 && (16..=31).contains(&ip[1]))
}

/// Parse one `"a.b.c.d:port"` into a remote candidate. A local address becomes a
/// `Host` candidate (same-network, ICE-preferred); anything else becomes `Srflx`
/// (a reflexive/public address). `None` on a malformed address.
pub fn parse_candidate(s: &str) -> Option<Candidate> {
    let s = s.trim();
    let (ip_s, port_s) = s.rsplit_once(':')?;
    let port: u16 = port_s.parse().ok()?;
    let mut oct = ip_s.split('.');
    let ip = [
        oct.next()?.parse().ok()?,
        oct.next()?.parse().ok()?,
        oct.next()?.parse().ok()?,
        oct.next()?.parse().ok()?,
    ];
    if oct.next().is_some() {
        return None;
    }
    let kind = if is_local(ip) { CandKind::Host } else { CandKind::Srflx };
    Some(Candidate { ip, port, kind, priority: candidate_priority(kind, 0) })
}

/// Parse the accepted `addr` field — one address, or several comma-separated
/// (so a host can advertise both its LAN and public addresses) — into remote
/// candidates, dropping any that don't parse.
pub fn parse_accepted_addrs(addr: &str) -> Vec<Candidate> {
    addr.split(',').filter_map(parse_candidate).collect()
}

/// Begin a hole-punch toward a host that just accepted our join. `local` are our
/// own candidates; `peer_id`/`session`/`nonce` tag the rendezvous blob;
/// `txid_seed` seeds the deterministic ping txids. Returns a `HolePunch` already
/// advanced to `Probing`, or `None` if the outcome was a denial or carried no
/// usable address (e.g. the host's reply key rather than a real endpoint).
pub fn begin_punch(
    accepted: &JoinOutcome,
    local: Vec<Candidate>,
    cfg: PunchConfig,
    txid_seed: u64,
    peer_id: u64,
    session: u64,
    nonce: u64,
) -> Option<HolePunch> {
    let addr = match accepted {
        JoinOutcome::Accepted { addr, .. } => addr,
        JoinOutcome::Denied => return None,
    };
    let candidates = parse_accepted_addrs(addr);
    if candidates.is_empty() {
        return None;
    }
    let blob = MailboxBlob { version: BLOB_VERSION, peer_id, session, nonce, candidates };
    let mut hp = HolePunch::new_seeded(local, cfg, txid_seed);
    hp.on_remote_blob(&blob);
    Some(hp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::net_rendezvous::{Action, PunchState};

    fn local_cands() -> Vec<Candidate> {
        vec![Candidate {
            ip: [192, 168, 1, 50],
            port: 7777,
            kind: CandKind::Host,
            priority: candidate_priority(CandKind::Host, 0),
        }]
    }

    #[test]
    fn classifies_local_vs_public() {
        assert_eq!(parse_candidate("192.168.2.178:9876").unwrap().kind, CandKind::Host);
        assert_eq!(parse_candidate("10.0.0.5:1").unwrap().kind, CandKind::Host);
        assert_eq!(parse_candidate("172.16.3.4:2").unwrap().kind, CandKind::Host);
        assert_eq!(parse_candidate("203.0.113.7:40123").unwrap().kind, CandKind::Srflx);
        // 172.15 and 172.32 are NOT in the private block
        assert_eq!(parse_candidate("172.15.0.1:5").unwrap().kind, CandKind::Srflx);
        assert_eq!(parse_candidate("172.32.0.1:5").unwrap().kind, CandKind::Srflx);
    }

    #[test]
    fn rejects_malformed_addresses() {
        assert!(parse_candidate("192.168.2.178").is_none()); // no port
        assert!(parse_candidate("1.2.3:9").is_none()); // 3 octets
        assert!(parse_candidate("1.2.3.4.5:9").is_none()); // 5 octets
        assert!(parse_candidate("a.b.c.d:9").is_none()); // non-numeric
        assert!(parse_candidate("1.2.3.4:notaport").is_none());
        assert!(parse_candidate("dht:somekey").is_none());
    }

    #[test]
    fn parses_multi_address_list_dropping_junk() {
        let cs = parse_accepted_addrs("192.168.2.178:9876,203.0.113.7:40123,garbage");
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].kind, CandKind::Host);
        assert_eq!(cs[1].kind, CandKind::Srflx);
    }

    #[test]
    fn accept_starts_a_probing_punch() {
        let outcome = JoinOutcome::Accepted {
            assigned_name: "guest".into(),
            addr: "192.168.2.178:9876".into(),
        };
        let mut hp = begin_punch(&outcome, local_cands(), PunchConfig::default(), 42, 1, 2, 3)
            .expect("punch begins");
        assert_eq!(hp.state(), PunchState::Probing);
        // Driving it actually emits a ping → the punch is live, not inert.
        let acts = hp.poll(0);
        assert!(
            acts.iter().any(|a| matches!(a, Action::SendPing { .. })),
            "a started punch pings the host"
        );
    }

    #[test]
    fn multiple_host_addrs_make_multiple_pairs() {
        // Two remote candidates × one local = two pairs; the punch still probes.
        let outcome = JoinOutcome::Accepted {
            assigned_name: "g".into(),
            addr: "192.168.2.178:9876,203.0.113.7:40123".into(),
        };
        let mut hp =
            begin_punch(&outcome, local_cands(), PunchConfig::default(), 7, 9, 9, 9).expect("begins");
        assert_eq!(hp.state(), PunchState::Probing);
        assert!(hp.poll(0).iter().any(|a| matches!(a, Action::SendPing { .. })));
    }

    #[test]
    fn denial_or_addressless_accept_yields_none() {
        assert!(begin_punch(&JoinOutcome::Denied, local_cands(), PunchConfig::default(), 1, 0, 0, 0).is_none());
        let no_addr = JoinOutcome::Accepted { assigned_name: "g".into(), addr: "dht:onlyakey".into() };
        assert!(begin_punch(&no_addr, local_cands(), PunchConfig::default(), 1, 0, 0, 0).is_none());
    }
}

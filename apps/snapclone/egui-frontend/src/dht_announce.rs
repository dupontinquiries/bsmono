//! dht_announce — what to publish so a peer behind CGNAT can actually
//! reach you.
//!
//! Ported from `JamiAdapter_dht_impl.cpp`, which is the reference for this:
//! that build works over CGNAT, and the reasons are all in the shape of
//! what it announces rather than in the DHT library.
//!
//! ## The bug this exists to fix
//!
//! I wired `NodeHost::publish` with an EMPTY host and a comment claiming
//! "the resolver observes the real address, the same rule gossip already
//! follows". That reasoning is right for gossip and wrong here, and the
//! difference is the whole point of a DHT: a gossip datagram arrives from a
//! source address the receiver can see, while a DHT record is stored DATA
//! that a stranger fetches later from a third machine. Nobody observes
//! anything. An empty host publishes a record that says "I exist" and
//! cannot be dialled — precisely useless, and it looked reasonable.
//!
//! ## Announce every address, not the best one
//!
//! The C++ announces all local IPv4 addresses AND the public one, and the
//! header explains why: *"so any peer on any shared subnet can reach us"*.
//! Picking one is a guess about which network the OTHER person is on, made
//! by the machine with the least information about it. Two laptops on the
//! same VPN, a phone on the same wifi, and a peer across the internet each
//! need a different one of your addresses, and you cannot know which of
//! them is asking.
//!
//! Under CGNAT this is what makes the difference: the carrier NAT means
//! your public address is shared and unreachable inbound, so the LAN
//! addresses are what actually work whenever the peer turns out to be
//! nearby — and the public one is what a hole punch has to aim at when they
//! are not.
//!
//! ## The freshness stamp is not decoration
//!
//! `#ts:<epoch>` is the last line. DHT values live about an hour, so a
//! record from the network you were on this morning is still there this
//! afternoon and points at an address that is now somebody else's printer.
//! [`newest`] keeps only the addresses from the freshest announcement.
//! Trailing '#' and last, so an older parser reads it as an address line,
//! fails on the port, and skips it — the compatibility trick is the reason
//! for the placement and is worth preserving rather than tidying.
//!
//! That trick has a condition the C++ does not spell out: it works because
//! an epoch in seconds does not fit in a `u16`, so the old parser's port
//! field rejects it. A small stamp would parse, and the peer would try to
//! dial a host called `#ts` on port 42. Nothing in this format prevents
//! that, so [`parse`] here skips '#' lines by CONTENT rather than relying
//! on the accident — new parsers should not inherit an old one's luck.
//!
//! ## Interop status, precisely
//!
//! The PLAINTEXT here matches the C++ v2 format byte for byte, and v1 is
//! parsed too so an older peer is still understood. What is NOT implemented
//! is the C++ `deriveEndpointKey` envelope: it encrypts the payload with a
//! key derived from the user id, which is obfuscation rather than secrecy
//! (anybody who knows the id can derive the key), but a Rust peer will not
//! read a Qt record or vice versa until it exists. So: same format, not yet
//! the same bytes. Said plainly because "ported the format" would otherwise
//! be heard as "interoperates".

use std::net::{Ipv4Addr, UdpSocket};

/// The Qt build's re-announce interval (`Timings::kDhtReannounceMs`), kept for
/// reference -- NOT this build's republish cadence.
///
/// This comment used to say the C++ uses 55 minutes "against a DHT value lifetime
/// of about an hour". An OpenDHT value lives 10 minutes (`value.h`,
/// `DEFAULT_VALUE_EXPIRATION`); the Qt presence put is `permanent=true`, which the
/// local OpenDHT node refreshes on its own, and this timer only re-announces on
/// top of that. The Rust node has no permanent puts, so it republishes every 8
/// minutes -- see `dht_presence::PUBLISH_EVERY_MS`.
pub const REANNOUNCE_MS: u64 = 55 * 60 * 1000;

/// One reachable address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addr {
    pub host: String,
    /// The port a snap connection is made to.
    pub ws_port: u16,
    /// The port a hole punch aims at. Equal to `ws_port` when there is no
    /// separate reflexive mapping — the C++ writes it either way rather
    /// than omitting it, so the field count per line never varies.
    pub punch_port: u16,
}

impl Addr {
    pub fn new(host: &str, ws_port: u16, punch_port: u16) -> Addr {
        Addr {
            host: host.to_string(),
            ws_port,
            punch_port: if punch_port == 0 { ws_port } else { punch_port },
        }
    }
}

/// One announcement: who, where, and when it was made.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Announcement {
    pub user_id: String,
    pub addrs: Vec<Addr>,
    /// Epoch seconds. 0 means an unstamped legacy record.
    pub stamp: i64,
    /// The X25519 mailbox key, base64 as the FOURTH field of the first address
    /// line -- exactly where the Qt build puts it (`makeMultiAddrPayload`), so a
    /// Qt peer can seal a connection request to us. See `mailbox.rs`.
    pub x_public: Option<[u8; 32]>,
    /// `#ed:` -- the Ed25519 identity key, so a contact never seen on the LAN
    /// can be pinned (its fingerprint must equal the sc-ID; checked by the
    /// caller). Ignored by the Qt parser, which skips unknown `#` lines.
    pub ed_public: Option<[u8; 32]>,
    /// `#xsig:` -- the identity key's signature over the mailbox key
    /// (`mailbox::xsig_message`). What turns the Qt build's trust-on-first-use
    /// into a check, for a peer whose identity key is pinned.
    pub xsig: Option<[u8; 64]>,
}

/// Build the v2 plaintext: `snapclone\nuserId\nhost:ws:punch\n…\n#ts:epoch`.
///
/// Addresses with no host or a zero port are dropped rather than written as
/// empty fields — a line the parser will reject is worse than an absent
/// one, because it costs the reader a parse and teaches nothing.
pub fn render(a: &Announcement) -> String {
    let mut out = String::from("snapclone\n");
    out.push_str(&a.user_id);
    let mut first = true;
    for ad in &a.addrs {
        if ad.host.trim().is_empty() || ad.ws_port == 0 {
            continue;
        }
        out.push('\n');
        out.push_str(&format!("{}:{}:{}", ad.host, ad.ws_port,
                              if ad.punch_port == 0 { ad.ws_port } else { ad.punch_port }));
        if first {
            if let Some(k) = &a.x_public {
                out.push(':');
                out.push_str(&crate::mailbox::b64_encode(k));
            }
            first = false;
        }
    }
    if let Some(k) = &a.ed_public {
        out.push_str(&format!("\n#ed:{}", crate::mailbox::b64_encode(k)));
    }
    if let Some(sig) = &a.xsig {
        out.push_str(&format!("\n#xsig:{}", crate::mailbox::b64_encode(sig)));
    }
    // LAST, and '#'-prefixed. See the module docs: an older parser treats
    // this as an address line, fails to parse the port, and skips it.
    out.push_str(&format!("\n#ts:{}", a.stamp));
    out
}

/// Parse either format.
///
/// v2 is `snapclone`-led and multi-address. v1 is
/// `userId\nhost\nwsPort\npunchPort[\npubkey]`, still parsed because a peer
/// running the older build is not an error — refusing it would split the
/// network on an upgrade, which is the same argument `wire_v3::negotiate`
/// makes about protocol versions.
pub fn parse(text: &str) -> Option<Announcement> {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.is_empty() {
        return None;
    }
    if lines[0].trim() == "snapclone" {
        if lines.len() < 3 {
            return None;
        }
        let user_id = lines[1].trim().to_string();
        if user_id.is_empty() {
            return None;
        }
        let mut ann = Announcement { user_id, ..Default::default() };
        for line in &lines[2..] {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Metadata is recognised by content, not by index, so an
            // unknown '#' line never displaces a real address.
            if let Some(rest) = line.strip_prefix('#') {
                if let Some(ts) = rest.strip_prefix("ts:") {
                    ann.stamp = ts.trim().parse().unwrap_or(0);
                } else if let Some(k) = rest.strip_prefix("ed:") {
                    ann.ed_public = crate::mailbox::b64_key(k);
                } else if let Some(sig) = rest.strip_prefix("xsig:") {
                    ann.xsig = crate::mailbox::b64_decode(sig).and_then(|v| v.try_into().ok());
                }
                continue;
            }
            // The mailbox key rides on the FIRST address line only, as in
            // the Qt build -- by content, so a metadata line cannot take the
            // slot.
            if let Some((a, key)) = parse_addr_line(line) {
                if ann.addrs.is_empty() && ann.x_public.is_none() {
                    ann.x_public = key;
                }
                ann.addrs.push(a);
            }
        }
        return Some(ann);
    }

    // v1.
    if lines.len() < 3 {
        return None;
    }
    let user_id = lines[0].trim().to_string();
    let host = lines[1].trim().to_string();
    let ws: u16 = lines[2].trim().parse().ok()?;
    if user_id.is_empty() || host.is_empty() || ws == 0 {
        return None;
    }
    let punch = lines.get(3).and_then(|p| p.trim().parse().ok()).unwrap_or(ws);
    Some(Announcement {
        user_id,
        addrs: vec![Addr::new(&host, ws, punch)],
        stamp: 0,
        // v1 carried the key as its fifth line.
        x_public: lines.get(4).and_then(|k| crate::mailbox::b64_key(k)),
        ..Default::default()
    })
}

/// `host:ws:punch[:pubkey]`, and the X25519 key when the fourth field is one.
/// The C++ puts the key on the first address line; a parser that choked on the
/// extra field would reject every record that carries a key.
fn parse_addr_line(line: &str) -> Option<(Addr, Option<[u8; 32]>)> {
    let mut it = line.split(':');
    let host = it.next()?.trim();
    let ws: u16 = it.next()?.trim().parse().ok()?;
    if host.is_empty() || ws == 0 {
        return None;
    }
    let punch = it.next().and_then(|p| p.trim().parse().ok()).unwrap_or(ws);
    let key = it.next().and_then(crate::mailbox::b64_key);
    Some((Addr::new(host, ws, punch), key))
}

/// Keep only the addresses from the freshest announcement.
///
/// A DHT holds every record that has not expired, so after moving networks
/// your own old announcement is still there for the best part of an hour,
/// pointing at an address that now belongs to somebody else's printer.
/// Dialling it is a connection to a stranger, and the person seeing the
/// failure has no way to know why.
///
/// Unstamped (v1, `stamp == 0`) records lose to any stamped one and are
/// used only when nothing better exists — an old peer should still be
/// reachable, just not in preference to current information.
pub fn newest(records: &[Announcement]) -> Vec<Addr> {
    let Some(best) = records.iter().map(|r| r.stamp).max() else { return Vec::new() };
    let mut out: Vec<Addr> = Vec::new();
    for r in records.iter().filter(|r| r.stamp == best) {
        for a in &r.addrs {
            if !out.contains(a) {
                out.push(a.clone());
            }
        }
    }
    out
}

/// Pick the one address to try first, given our own.
///
/// `punch_addrs` holds ONE address per peer, so a multi-address
/// announcement has to be reduced somewhere, and the reduction is the
/// interesting part on CGNAT.
///
/// Order, and the reason for each step:
///
/// 1. **Same /24 as one of ours.** If we are on the same subnet the
///    connection is direct — no NAT, no punch, no carrier in the path. This
///    is the case CGNAT makes most valuable, because it is the one that
///    still works when the public address does not.
/// 2. **Any other private address.** A different private range often means
///    a VPN both machines are on, which is also direct.
/// 3. **Anything else**, which in practice is the public address and the
///    only option when the peer really is elsewhere.
///
/// A /24 is a guess: the announcement carries no netmask, and the C++ does
/// not carry one either. It is right for essentially every home and phone
/// network and wrong for a /16 campus, where this picks a same-subnet
/// address that is merely nearby — which still routes, so the cost of being
/// wrong is a hop rather than a failure.
pub fn best_for<'a>(ours: &[Ipv4Addr], candidates: &'a [Addr]) -> Option<&'a Addr> {
    let parsed: Vec<(Option<Ipv4Addr>, &Addr)> = candidates.iter()
        .map(|a| (a.host.parse::<Ipv4Addr>().ok(), a))
        .collect();
    let same_24 = |ip: &Ipv4Addr| {
        ours.iter().any(|o| o.octets()[..3] == ip.octets()[..3])
    };
    parsed.iter().find(|(ip, _)| ip.is_some_and(|i| same_24(&i)))
        .or_else(|| parsed.iter().find(|(ip, _)| ip.is_some_and(|i| i.is_private())))
        .or_else(|| parsed.first())
        .map(|(_, a)| *a)
}

// ───────────────────────────────────────────────── finding our own addresses ──

/// The address this machine would use to reach the internet.
///
/// A connected UDP socket sends NOTHING — `connect` on UDP only sets the
/// default peer — and the kernel picks a source address by consulting the
/// routing table. That makes this a route lookup with no packets, no
/// dependency and no DNS, which is why it is preferred over asking an
/// external service for anything.
///
/// It answers "which interface would be used", not "what does the world
/// see": behind CGNAT this is a private address. That is not a shortcoming
/// here, since the private ones are exactly what a peer on the same network
/// needs.
pub fn primary_ipv4() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    // A documentation-range address: routable enough for the kernel to pick
    // an interface, and never actually contacted.
    s.connect("192.0.2.1:9").ok()?;
    match s.local_addr().ok()? {
        std::net::SocketAddr::V4(v4) => Some(*v4.ip()),
        _ => None,
    }
}

/// Parse `ip -o -4 addr show` output into addresses.
///
/// Pure, so the interesting cases can be tested without a network stack.
/// Loopback is dropped: announcing 127.0.0.1 tells a peer to connect to
/// itself, which fails in the most confusing possible way — something
/// answers.
pub fn parse_ip_addr_output(text: &str) -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    for line in text.lines() {
        // "3: wlan0    inet 192.168.1.42/24 brd … scope global wlan0"
        let mut it = line.split_whitespace();
        let mut iface = None;
        while let Some(tok) = it.next() {
            if tok == "inet" {
                let Some(cidr) = it.next() else { break };
                let addr = cidr.split('/').next().unwrap_or(cidr);
                if let Ok(ip) = addr.parse::<Ipv4Addr>() {
                    if !ip.is_loopback() && !out.contains(&ip) {
                        out.push(ip);
                    }
                }
                break;
            }
            iface = Some(tok);
        }
        let _ = iface;
    }
    out
}

/// Every local IPv4 address worth announcing.
///
/// `ip -o -4 addr` when it exists — that is the one that finds the VPN and
/// the second interface, which is the whole reason the C++ enumerates
/// rather than picking. When the tool is absent (a minimal container, a
/// phone image without iproute2) it falls back to the route lookup, which
/// always works and finds one. Falling back to one address is a degraded
/// announcement; falling back to none would be a silent failure to be
/// reachable at all.
/// Is this an address other people could reach us at from outside?
///
/// Loopback, link-local, RFC 1918 / ULA and unspecified are not. What a
/// peer reports as our address is one of these when the peer is on our
/// LAN, and the record must not carry it as if it were public: a remote
/// reader would try 192.168.x.x and conclude we are gone.
pub fn is_routable(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => !(v4.is_private() || v4.is_loopback() || v4.is_link_local()
            || v4.is_unspecified() || v4.is_broadcast() || v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64),
        std::net::IpAddr::V6(v6) => !(v6.is_loopback() || v6.is_unspecified()
            || (v6.octets()[0] & 0xFE) == 0xFC          // fc00::/7 unique local
            || (v6.octets()[0] == 0xFE && (v6.octets()[1] & 0xC0) == 0x80)), // fe80::/10
    }
}

/// The addresses to announce: every local one, plus what peers say we
/// reach them from if that is routable and not already listed.
///
/// This is what the Qt build asked ipify for. The DHT reports it for free
/// in every reply (`NodeHost::public_ip`), by majority of the peers that
/// answered, which is also the only source that reflects the network the
/// DHT traffic actually crossed.
///
/// `reflexive` is the punch socket's STUN-mapped address — the one that
/// crosses a carrier NAT — and wins over the DHT-reported ip when both
/// are known: it carries the mapped PORT, which `sa` never does, and a
/// record with the wrong punch port sends every peer's volley to a
/// mapping that does not exist.
pub fn with_public(mut addrs: Vec<Addr>, public_ip: Option<std::net::IpAddr>, port: u16,
                   reflexive: Option<std::net::SocketAddr>) -> Vec<Addr> {
    let (ip, punch) = match (reflexive, public_ip) {
        (Some(r), _) => (Some(r.ip()), r.port()),
        (None, ip) => (ip, 0),
    };
    if let Some(ip) = ip {
        let host = ip.to_string();
        if is_routable(ip) && !addrs.iter().any(|a| a.host == host) {
            addrs.push(Addr::new(&host, port, punch));
        }
    }
    addrs
}

pub fn local_ipv4s() -> Vec<Ipv4Addr> {
    let mut out = std::process::Command::new("ip")
        .args(["-o", "-4", "addr", "show"])
        .output()
        .ok()
        .map(|o| parse_ip_addr_output(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();
    if let Some(p) = primary_ipv4() {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_announcement_survives_the_round_trip_with_every_address() {
        // Picking one address is a guess about which network the OTHER
        // person is on, made by the machine with the least information.
        let a = Announcement {
            user_id: "sc-aabbccdd".into(),
            addrs: vec![
                Addr::new("192.168.1.42", 9876, 9876),
                Addr::new("10.8.0.3", 9876, 41234),
                Addr::new("203.0.113.9", 9876, 33445),
            ],
            stamp: 1_700_000_000, ..Default::default()
        };
        let got = parse(&render(&a)).expect("did not parse");
        assert_eq!(got, a);
    }

    #[test]
    fn the_stamp_is_the_last_line_and_prefixed_so_old_parsers_skip_it() {
        // The placement is the compatibility trick: an older parser reads
        // it as an address line, fails on the port, and skips it. Tidying
        // it into a header would break every peer running the old build.
        let text = render(&Announcement {
            user_id: "sc-aabbccdd".into(),
            addrs: vec![Addr::new("192.168.1.42", 9876, 9876)],
            stamp: 1_700_000_000, ..Default::default()
        });
        assert!(text.ends_with("\n#ts:1700000000"), "{text:?}");
        // A real epoch does not fit in a u16, so an old parser's port field
        // rejects it and the line is skipped. That is the trick, and it is
        // luck rather than design.
        assert!(parse_addr_line("#ts:1700000000").is_none(),
                "an old parser would dial a host called #ts");
        // Which is exactly why THIS parser does not depend on it: a small
        // stamp would sail through the port check.
        assert!(parse_addr_line("#ts:42").is_some(),
                "the assumption under the old trick has changed — check the module note");
        assert_eq!(parse(&text).unwrap().stamp, 1_700_000_000);
        let small = parse("snapclone\nu\n10.0.0.1:9876\n#ts:42").unwrap();
        assert_eq!(small.stamp, 42);
        assert_eq!(small.addrs.len(), 1, "the stamp was parsed as an address: {small:?}");
    }

    #[test]
    fn a_stale_record_from_a_previous_network_loses_to_a_fresh_one() {
        // DHT values live about an hour, so this morning's announcement is
        // still there this afternoon pointing at an address that now
        // belongs to somebody else's printer.
        let old = Announcement { user_id: "u".into(),
            addrs: vec![Addr::new("192.168.1.42", 9876, 9876)], stamp: 1_000, ..Default::default() };
        let new = Announcement { user_id: "u".into(),
            addrs: vec![Addr::new("10.0.0.5", 9876, 9876)], stamp: 2_000, ..Default::default() };
        let picked = newest(&[old, new]);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].host, "10.0.0.5");
    }

    #[test]
    fn two_records_from_the_same_moment_are_merged_not_chosen_between() {
        // The C++ publishes one value per address, so a single announcement
        // arrives back as several records sharing a stamp. Taking only the
        // first would throw away the address the asking peer can reach.
        let a = Announcement { user_id: "u".into(),
            addrs: vec![Addr::new("192.168.1.42", 9876, 9876)], stamp: 5, ..Default::default() };
        let b = Announcement { user_id: "u".into(),
            addrs: vec![Addr::new("10.8.0.3", 9876, 9876)], stamp: 5, ..Default::default() };
        let picked = newest(&[a.clone(), b, a]);
        assert_eq!(picked.len(), 2, "duplicates survived or an address was lost: {picked:?}");
    }

    #[test]
    fn an_unstamped_legacy_record_is_used_only_when_nothing_better_exists() {
        // An old peer should still be reachable — just not in preference to
        // current information.
        let legacy = parse("sc-aabbccdd\n192.168.1.42\n9876\n9876").expect("v1 did not parse");
        assert_eq!(legacy.stamp, 0);
        assert_eq!(legacy.addrs, vec![Addr::new("192.168.1.42", 9876, 9876)]);
        assert_eq!(newest(&[legacy.clone()]).len(), 1, "an old peer became unreachable");

        let fresh = Announcement { user_id: "sc-aabbccdd".into(),
            addrs: vec![Addr::new("10.0.0.5", 9876, 9876)], stamp: 1, ..Default::default() };
        assert_eq!(newest(&[legacy, fresh])[0].host, "10.0.0.5");
    }

    #[test]
    fn a_pubkey_on_the_first_address_line_does_not_break_the_parse() {
        // The C++ appends a base64 key to the first address. A parser that
        // choked on the extra field would reject every record carrying one.
        let a = parse("snapclone\nsc-aabbccdd\n192.168.1.42:9876:9876:AAAABBBBCCCC=\n#ts:9")
            .expect("did not parse");
        assert_eq!(a.addrs, vec![Addr::new("192.168.1.42", 9876, 9876)]);
        assert_eq!(a.stamp, 9);
    }

    /// The mailbox key and its binding survive the round trip, the key sits
    /// where the Qt parser reads it (fourth field of the FIRST address line),
    /// and a record with every field still fits the 512-byte presence cap once
    /// sealed.
    #[test]
    fn the_mailbox_key_rides_where_the_qt_build_reads_it() {
        let a = Announcement {
            user_id: "sc-aabbccdd".into(),
            addrs: vec![Addr::new("192.168.1.42", 9876, 9876), Addr::new("10.8.0.3", 9876, 41234),
                        Addr::new("100.64.12.200", 9876, 50000), Addr::new("203.0.113.9", 9876, 33445)],
            stamp: 1_790_000_000,
            x_public: Some([0x5a; 32]),
            ed_public: Some([0x11; 32]),
            xsig: Some([0x77; 64]),
        };
        let text = render(&a);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[2], format!("192.168.1.42:9876:9876:{}", crate::mailbox::b64_encode(&[0x5a; 32])));
        assert!(!lines[3].contains(&crate::mailbox::b64_encode(&[0x5a; 32])), "the key went on a second line too");
        assert!(text.ends_with("\n#ts:1790000000"), "the stamp must stay last: {text:?}");
        assert_eq!(parse(&text).unwrap(), a);
        let sealed = crate::dht_envelope::seal_with_nonce("sc-aabbccdd", crate::dht_envelope::V_MULTI,
                                                         [3u8; crate::dht_envelope::NONCE_BYTES], text.as_bytes());
        assert!(sealed.len() <= 512, "{} bytes sealed: over the Qt reader's 512-byte cap", sealed.len());
        // A key-less record is the same as before.
        assert_eq!(parse(&render(&Announcement { x_public: None, ed_public: None, xsig: None, ..a.clone() })).unwrap().x_public, None);
    }

    #[test]
    fn an_address_with_no_host_or_port_is_dropped_rather_than_written_empty() {
        let text = render(&Announcement {
            user_id: "u".into(),
            addrs: vec![Addr::new("", 9876, 0), Addr::new("10.0.0.1", 0, 0),
                        Addr::new("10.0.0.2", 9876, 0)],
            stamp: 1, ..Default::default()
        });
        let got = parse(&text).unwrap();
        assert_eq!(got.addrs, vec![Addr::new("10.0.0.2", 9876, 9876)]);
    }

    #[test]
    fn the_reported_address_is_announced_only_when_routable_and_new() {
        let lan = vec![Addr::new("192.168.1.5", 9876, 0)];
        let pub_ip: std::net::IpAddr = "203.0.113.7".parse().unwrap();
        let out = with_public(lan.clone(), Some(pub_ip), 9876, None);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].host, "203.0.113.7");
        assert_eq!(out[1].ws_port, 9876);
        // A LAN peer reports our LAN address back: not public, not added.
        assert_eq!(with_public(lan.clone(), Some("192.168.1.5".parse().unwrap()), 9876, None).len(), 1);
        assert_eq!(with_public(lan.clone(), Some("10.4.0.9".parse().unwrap()), 9876, None).len(), 1);
        assert_eq!(with_public(lan.clone(), Some("127.0.0.1".parse().unwrap()), 9876, None).len(), 1);
        // CGNAT space (100.64/10) is not reachable from outside either.
        assert_eq!(with_public(lan.clone(), Some("100.72.1.1".parse().unwrap()), 9876, None).len(), 1);
        // Already listed: not doubled.
        let already = vec![Addr::new("203.0.113.7", 9876, 0)];
        assert_eq!(with_public(already, Some(pub_ip), 9876, None).len(), 1);
        assert_eq!(with_public(lan.clone(), None, 9876, None).len(), 1);
        // The STUN-mapped address wins and carries the mapped port: that
        // port is what a peer's volley must hit.
        let refl: std::net::SocketAddr = "203.0.113.7:51234".parse().unwrap();
        let out = with_public(lan.clone(), Some("203.0.113.9".parse().unwrap()), 9876, Some(refl));
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].host, "203.0.113.7");
        assert_eq!(out[1].punch_port, 51234);
        assert_eq!(out[1].ws_port, 9876);
        assert_eq!(with_public(lan, None, 9876, Some("192.168.1.5:5".parse().unwrap())).len(), 1,
                   "a LAN STUN answer (a hairpin, or a local server) is not public either");
        assert!(is_routable("2001:db8::1".parse().unwrap()));
        assert!(!is_routable("fd12::1".parse().unwrap()));
        assert!(!is_routable("fe80::1".parse().unwrap()));
    }

    #[test]
    fn a_missing_punch_port_defaults_to_the_connection_port() {
        // The C++ writes `punch ? punch : ws`, so the field count never
        // varies. A record from something that omitted it must not come
        // back as port 0, which is unroutable.
        assert_eq!(Addr::new("10.0.0.1", 9876, 0).punch_port, 9876);
        let a = parse("snapclone\nu\n10.0.0.1:9876\n#ts:1").unwrap();
        assert_eq!(a.addrs[0].punch_port, 9876);
    }

    #[test]
    fn the_address_on_our_own_subnet_is_tried_first() {
        // The case CGNAT makes most valuable: same subnet means a direct
        // connection, no NAT and no carrier in the path — and it still
        // works when the public address does not.
        let ours = vec!["192.168.1.7".parse::<Ipv4Addr>().unwrap()];
        let cands = vec![
            Addr::new("203.0.113.9", 9876, 9876),
            Addr::new("10.8.0.3", 9876, 9876),
            Addr::new("192.168.1.42", 9876, 9876),
        ];
        assert_eq!(best_for(&ours, &cands).unwrap().host, "192.168.1.42");
    }

    #[test]
    fn a_vpn_address_beats_a_public_one_when_no_subnet_matches() {
        // A different private range usually means a VPN both machines are
        // on, which is also direct.
        let ours = vec!["172.20.0.4".parse::<Ipv4Addr>().unwrap()];
        let cands = vec![
            Addr::new("203.0.113.9", 9876, 9876),
            Addr::new("10.8.0.3", 9876, 9876),
        ];
        assert_eq!(best_for(&ours, &cands).unwrap().host, "10.8.0.3");
    }

    #[test]
    fn the_public_address_is_used_when_it_is_the_only_way_out() {
        let ours = vec!["192.168.1.7".parse::<Ipv4Addr>().unwrap()];
        let cands = vec![Addr::new("203.0.113.9", 9876, 33445)];
        let best = best_for(&ours, &cands).unwrap();
        assert_eq!(best.host, "203.0.113.9");
        assert_eq!(best.punch_port, 33445, "the punch port was lost on the way through");
        assert!(best_for(&ours, &[]).is_none());
    }

    #[test]
    fn a_hostname_candidate_does_not_displace_a_usable_address() {
        // Unparseable hosts sort last rather than being dropped: resolving
        // one needs DNS on the caller's thread, and it is still better than
        // nothing if it is all there is.
        let ours = vec!["192.168.1.7".parse::<Ipv4Addr>().unwrap()];
        let cands = vec![
            Addr::new("peer.example.invalid", 9876, 9876),
            Addr::new("10.8.0.3", 9876, 9876),
        ];
        assert_eq!(best_for(&ours, &cands).unwrap().host, "10.8.0.3");
        let only_name = vec![Addr::new("peer.example.invalid", 9876, 9876)];
        assert_eq!(best_for(&ours, &only_name).unwrap().host, "peer.example.invalid");
    }

    #[test]
    fn the_interface_list_finds_every_address_and_drops_loopback() {
        // Announcing 127.0.0.1 tells a peer to connect to itself, which
        // fails in the most confusing way available: something answers.
        let out = "\
1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever
2: eth0    inet 192.168.1.42/24 brd 192.168.1.255 scope global eth0\\       valid_lft forever
3: wlan0    inet 10.0.0.5/24 brd 10.0.0.255 scope global dynamic wlan0
4: tun0    inet 10.8.0.3/24 scope global tun0
2: eth0    inet 192.168.1.42/24 brd 192.168.1.255 scope global secondary eth0
";
        let ips = parse_ip_addr_output(out);
        assert_eq!(ips, vec![
            "192.168.1.42".parse::<Ipv4Addr>().unwrap(),
            "10.0.0.5".parse().unwrap(),
            "10.8.0.3".parse().unwrap(),
        ], "the VPN or the second interface went missing");
    }

    #[test]
    fn a_missing_iproute2_still_yields_an_address() {
        // Falling back to one address is a degraded announcement; falling
        // back to none is a silent failure to be reachable at all.
        assert!(parse_ip_addr_output("").is_empty());
        assert!(primary_ipv4().is_some(),
                "the route lookup found nothing — no interface at all?");
    }
}

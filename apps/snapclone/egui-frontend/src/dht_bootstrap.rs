//! dht_bootstrap — how the DHT gets its first contact, from
//! `JamiAdapter_dht_impl`.
//!
//! A distributed hash table needs one working peer to join. Everything after
//! that is peer-to-peer; **this** step is the one that fails on a hostile
//! network, so the table is built to survive three separate failures.
//!
//! ## Hardcoded IPs alongside hostnames, because DNS is not reliable
//!
//! *"The hardcoded IPs let the DHT start even when system DNS is broken (e.g.
//! ISP hijacking, split-horizon DNS, corporate filters)."*
//!
//! Each entry carries **both**, and the hostname is tried first — the IP is the
//! fallback, not the primary, because a hardcoded address goes stale and a
//! hostname does not. The source records when they were checked: *"IPs verified
//! 2026-04: dig bootstrap.jami.net @8.8.8.8"*. That date is part of the data:
//! an IP with no verification date is a number nobody can decide whether to
//! trust.
//!
//! ## Three ports, and the third is for phone hotspots
//!
//! *"4222 (standard), 443 (firewall bypass), 80 (hotspot)."*
//!
//! 4222 is blocked on most corporate and public networks. 443 gets through
//! nearly everything because blocking it breaks HTTPS. 80 is there for captive
//! portals and tethered connections that allow only web ports — which is
//! exactly the network a phone is on when it most needs to find peers.
//!
//! ## Callbacks arrive on a foreign thread
//!
//! *"All DHT callbacks fire on opendht's internal thread... `value_cb` always
//! completes before `done_cb`."* That contract is what makes the lifetime safe:
//! the context can be freed in `done_cb` precisely because no value callback can
//! still be running. Porting the ordering assumption matters more than porting
//! the code around it.

/// One way to reach a bootstrap node.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BootstrapServer {
    /// Tried first: survives an address change.
    pub host: &'static str,
    /// Used when DNS fails: survives a broken resolver.
    pub ip: &'static str,
    pub port: u16,
}

/// Standard DHT port.
pub const PORT_STANDARD: u16 = 4222;
/// Gets through most firewalls, because blocking it breaks HTTPS.
pub const PORT_FIREWALL: u16 = 443;
/// For captive portals and tethered connections that allow only web ports.
pub const PORT_HOTSPOT: u16 = 80;

/// When the hardcoded addresses were last checked against DNS.
///
/// Carried as data because an IP with no verification date is a number nobody
/// can decide whether to trust.
pub const IPS_VERIFIED: &str = "2026-04";

/// Bootstrap candidates, in the order they should be tried.
pub const BOOTSTRAP_SERVERS: &[BootstrapServer] = &[
    BootstrapServer { host: "bootstrap.jami.net", ip: "158.69.23.138", port: PORT_STANDARD },
    BootstrapServer { host: "bootstrap.jami.net", ip: "144.217.10.53", port: PORT_STANDARD },
    BootstrapServer { host: "bootstrap.ring.cx",  ip: "54.39.28.13",   port: PORT_STANDARD },
    BootstrapServer { host: "bootstrap.jami.net", ip: "158.69.23.138", port: PORT_FIREWALL },
    BootstrapServer { host: "bootstrap.jami.net", ip: "144.217.10.53", port: PORT_FIREWALL },
    BootstrapServer { host: "bootstrap.ring.cx",  ip: "54.39.28.13",   port: PORT_FIREWALL },
    BootstrapServer { host: "bootstrap.jami.net", ip: "158.69.23.138", port: PORT_HOTSPOT },
    BootstrapServer { host: "bootstrap.jami.net", ip: "144.217.10.53", port: PORT_HOTSPOT },
];

/// Which address to dial for an attempt.
///
/// Hostname first, IP second. The IP exists for a broken resolver, not as the
/// preferred route — a hardcoded address goes stale between releases and a
/// hostname does not.
pub fn address_for(s: &BootstrapServer, dns_working: bool) -> String {
    let hostpart = if dns_working { s.host } else { s.ip };
    format!("{hostpart}:{}", s.port)
}

/// Candidates ordered for a network with the given constraints.
///
/// When only web ports are open, the standard-port entries are still included
/// but last: a wrong guess about the network should cost latency, not
/// reachability.
pub fn ordered_for(web_ports_only: bool) -> Vec<&'static BootstrapServer> {
    let mut v: Vec<&BootstrapServer> = BOOTSTRAP_SERVERS.iter().collect();
    if web_ports_only {
        v.sort_by_key(|s| if s.port == PORT_STANDARD { 1 } else { 0 });
    }
    v
}

/// A lookup in flight.
///
/// Models the opendht callback contract: values arrive, then exactly one
/// completion. The context may only be released on completion, and only because
/// *"value_cb always completes before done_cb"*.
#[derive(Clone, Debug, Default)]
pub struct Lookup {
    values: Vec<Vec<u8>>,
    done: bool,
    /// Set if a value arrived after completion — a contract violation worth
    /// noticing rather than crashing on.
    pub late_values: u32,
}

impl Lookup {
    pub fn values(&self) -> &[Vec<u8>] { &self.values }
    pub fn is_done(&self) -> bool { self.done }

    /// A value callback fired.
    pub fn on_value(&mut self, v: Vec<u8>) {
        if self.done {
            // Freeing on done_cb is only safe because this cannot happen. If it
            // does, count it rather than touching freed state — a use-after-free
            // on a foreign thread is the worst failure available here.
            self.late_values += 1;
            return;
        }
        self.values.push(v);
    }

    /// The completion callback fired.
    ///
    /// Returns whether it is now safe to release the context. False on a
    /// duplicate completion, which would otherwise free twice.
    pub fn on_done(&mut self) -> bool {
        if self.done { return false; }
        self.done = true;
        true
    }

    /// Did the lookup find anything?
    ///
    /// Distinct from "did it finish": a completed lookup with no values is a
    /// peer that is genuinely not published, which is a different answer from a
    /// lookup still running.
    pub fn outcome(&self) -> Outcome {
        match (self.done, self.values.is_empty()) {
            (false, _) => Outcome::Pending,
            (true, true) => Outcome::NotFound,
            (true, false) => Outcome::Found,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome { Pending, Found, NotFound }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_server_carries_both_a_hostname_and_an_ip() {
        // "The hardcoded IPs let the DHT start even when system DNS is broken
        // (ISP hijacking, split-horizon DNS, corporate filters)." An entry with
        // only one of the two loses one of the failure modes.
        for s in BOOTSTRAP_SERVERS {
            assert!(!s.host.is_empty(), "{s:?}");
            assert!(!s.ip.is_empty(), "{s:?}");
            assert!(s.ip.parse::<std::net::Ipv4Addr>().is_ok(), "{} is not an IP", s.ip);
        }
    }

    #[test]
    fn the_hostname_is_preferred_and_the_ip_is_the_fallback() {
        // A hardcoded address goes stale between releases; a hostname does not.
        let s = &BOOTSTRAP_SERVERS[0];
        assert!(address_for(s, true).starts_with("bootstrap.jami.net"));
        assert!(address_for(s, false).starts_with("158.69.23.138"));
    }

    #[test]
    fn all_three_ports_are_covered() {
        // 4222 is blocked on most corporate and public networks; 443 gets
        // through nearly everything because blocking it breaks HTTPS; 80 is for
        // captive portals and tethering — exactly the network a phone is on
        // when it most needs to find peers.
        for p in [PORT_STANDARD, PORT_FIREWALL, PORT_HOTSPOT] {
            assert!(BOOTSTRAP_SERVERS.iter().any(|s| s.port == p), "no entry on port {p}");
        }
    }

    #[test]
    fn more_than_one_host_is_listed_per_port_family() {
        // "Multiple fallback servers so we're resilient if one is down." A
        // single host per port means one outage takes the whole bootstrap out.
        for p in [PORT_STANDARD, PORT_FIREWALL] {
            let hosts: std::collections::HashSet<_> =
                BOOTSTRAP_SERVERS.iter().filter(|s| s.port == p).map(|s| s.ip).collect();
            assert!(hosts.len() >= 2, "only one address on port {p}");
        }
    }

    #[test]
    fn the_standard_port_is_tried_first_on_an_ordinary_network() {
        // It is the one the DHT is actually designed around; 443 and 80 are
        // workarounds that share a port with other traffic.
        assert_eq!(BOOTSTRAP_SERVERS[0].port, PORT_STANDARD);
        assert_eq!(ordered_for(false)[0].port, PORT_STANDARD);
    }

    #[test]
    fn a_web_only_network_reaches_the_web_ports_first_but_still_keeps_the_rest() {
        // A wrong guess about the network should cost LATENCY, not
        // reachability — dropping 4222 entirely would strand a device we
        // misdiagnosed.
        let v = ordered_for(true);
        assert_ne!(v[0].port, PORT_STANDARD);
        assert!(v.iter().any(|s| s.port == PORT_STANDARD), "the standard port was dropped");
        assert_eq!(v.len(), BOOTSTRAP_SERVERS.len());
    }

    #[test]
    fn the_ip_verification_date_travels_with_the_ips() {
        // An address with no verification date is a number nobody can decide
        // whether to trust, and these go stale silently.
        assert_eq!(IPS_VERIFIED, "2026-04");
    }

    #[test]
    fn values_arrive_before_completion() {
        // The opendht contract the lifetime depends on.
        let mut l = Lookup::default();
        l.on_value(vec![1]);
        l.on_value(vec![2]);
        assert_eq!(l.outcome(), Outcome::Pending);
        assert!(l.on_done());
        assert_eq!(l.outcome(), Outcome::Found);
        assert_eq!(l.values().len(), 2);
    }

    #[test]
    fn a_value_after_completion_is_counted_rather_than_crashing() {
        // Freeing the context on done_cb is only safe because "value_cb always
        // completes before done_cb". If that contract ever breaks, counting it
        // is survivable; touching freed state on a foreign thread is not.
        let mut l = Lookup::default();
        l.on_done();
        l.on_value(vec![9]);
        assert_eq!(l.late_values, 1);
        assert!(l.values().is_empty());
    }

    #[test]
    fn a_duplicate_completion_does_not_free_twice() {
        // on_done gates the release, so a second completion returning true
        // would be a double free on opendht's thread.
        let mut l = Lookup::default();
        assert!(l.on_done());
        assert!(!l.on_done());
    }

    #[test]
    fn a_finished_lookup_with_no_values_is_not_found_rather_than_pending() {
        // "The peer is not published" and "we are still looking" are different
        // answers, and conflating them leaves the UI spinning forever on a
        // contact who simply has not registered.
        let mut l = Lookup::default();
        l.on_done();
        assert_eq!(l.outcome(), Outcome::NotFound);
        assert!(l.is_done());
    }

    #[test]
    fn a_lookup_that_never_completes_stays_pending() {
        let mut l = Lookup::default();
        l.on_value(vec![1]);
        assert_eq!(l.outcome(), Outcome::Pending, "values alone are not completion");
    }
}

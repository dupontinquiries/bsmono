//! clusternode.rs — the one object that owns the cluster pieces and is polled once per frame.
//!
//! Identity, registry, socket and discovery table all existed and nothing held them together, so
//! nothing could be called from an app. This is the composition: construct once, `poll` every frame,
//! ask it where a peer is.
//!
//! ## Why the composition is its own type
//!
//! A UI that wires four objects together will wire them slightly differently in the next place it
//! needs them — and the pieces have ordering constraints that are not obvious from their signatures
//! (age before accept, or a peer heard this frame is immediately one frame stale; accept before
//! observe, or an unverified announcement lands in the table). Putting the order here means it is
//! written once and tested.
//!
//! ## What it does not do
//!
//! It does not connect, authenticate, or send work. It answers "which of my machines are reachable
//! right now", which is the question everything downstream starts from.

pub struct ClusterNode {
    pub identity: crate::machineid::Identity,
    pub registry: crate::trustkit::Registry,
    pub discovery: crate::discovery::Discovery,
    wire: Option<crate::lanwire::LanWire>,
    /// Why the socket is unavailable, if it is. Shown rather than retried silently.
    pub wire_error: Option<String>,
    /// This machine's own announced address.
    pub self_addr: String,
}

impl ClusterNode {
    /// Build from an identity path and a registry, binding the discovery socket.
    ///
    /// A socket failure is RECORDED, not fatal: a phone with multicast blocked should still show its
    /// own id and its trust list, because those are what an operator needs to fix the situation.
    /// Build from a state directory, loading the identity and the trust list from it.
    ///
    /// The two files live together because they are one setting: an identity without its registry is
    /// a machine nobody trusts, and a registry without its identity is a list of machines that cannot
    /// recognise you. Backing up the directory keeps the pair.
    pub fn from_dir(dir: &std::path::Path, self_addr: String)
        -> Result<(ClusterNode, Vec<String>), crate::machineid::IdentityError> {
        let identity = crate::machineid::Identity::load_or_create(&dir.join("identity.key"))?;
        let (registry, warnings) = crate::trustkit::Registry::load(&dir.join("trusted.txt"));
        Ok((ClusterNode::new(identity, registry, self_addr), warnings))
    }

    /// Persist the trust list.
    ///
    /// Called after any change, because a machine added and not saved is one that vanishes on
    /// restart — and it vanishes SILENTLY, reading as a discovery failure rather than as forgotten
    /// state. That is the failure trustkit's persistence was written to prevent and which nothing was
    /// calling.
    pub fn save_registry(&self, dir: &std::path::Path) -> std::io::Result<()> {
        self.registry.save(&dir.join("trusted.txt"))
    }

    pub fn new(identity: crate::machineid::Identity, registry: crate::trustkit::Registry,
               self_addr: String) -> ClusterNode {
        let (wire, wire_error) = match crate::lanwire::LanWire::bind() {
            Ok(w) => (Some(w), None),
            Err(e) => (None, Some(format!(
                "discovery socket unavailable: {e}. This machine can still be reached if a peer has \
its address, and can still register others — only LAN auto-discovery is off"))),
        };
        ClusterNode { identity, registry, discovery: crate::discovery::Discovery::new(),
                      wire, wire_error, self_addr }
    }

    /// Advance one frame: age the table, drain what arrived, announce if due.
    ///
    /// ORDER MATTERS and is the reason this exists. Age FIRST, so a peer heard this frame is not
    /// immediately one frame stale. Drain before announcing, so a reply to our previous announcement
    /// is processed before we send another.
    pub fn poll(&mut self, dt_s: f32, now_s: u64) {
        self.discovery.age(dt_s);

        if let Some(w) = &self.wire {
            for (body, sig, _from) in w.drain() {
                let a = match crate::announce::Announce::parse(&body) { Ok(a) => a, Err(_) => continue };
                // Verification happens BEFORE the table is touched — an unverified announcement must
                // never become an address anything connects to.
                match crate::announce::accept(&a, now_s, &self.registry,
                        |key, msg| verify_ed25519(key, msg, &sig)) {
                    Ok(c) => self.discovery.observed(&a.id, c),
                    Err(crate::announce::Reject::Unregistered) => self.discovery.saw_unregistered(&a.id),
                    Err(_) => {}
                }
            }
        }

        if self.discovery.tick(dt_s) {
            if let Some(w) = &self.wire {
                let a = crate::announce::Announce {
                    id: self.identity.id(), addr: self.self_addr.clone(), sent_at_s: now_s,
                };
                let sig = self.identity.sign(&a.payload());
                let _ = w.announce(&a, &sig);
            }
        }
    }

    /// Where is this peer right now, if anywhere?
    pub fn locate(&self, id: &str) -> Option<&crate::rendezvous::Candidate> {
        // Through the registry, so an id nobody vouched for cannot be resolved even if it is somehow
        // in the table — the same guard rendezvous::resolve applies, not bypassed by having a
        // convenience method here.
        if self.registry.level(id) == crate::trustkit::Trust::Unknown { return None; }
        self.discovery.find(id)
    }
}

fn verify_ed25519(key: &[u8], msg: &[u8], sig: &[u8; 64]) -> bool {
    let k: [u8; 32] = match key.try_into() { Ok(k) => k, Err(_) => return false };
    crypto_kit::verify(&k, msg, sig)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trustkit::{Registry, Trust};

    fn tmp(n: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("logscope-node-{n}-{}", std::process::id()));
        p
    }
    fn node(n: &str) -> ClusterNode {
        let p = tmp(n);
        let _ = std::fs::remove_file(&p);
        let id = crate::machineid::Identity::load_or_create(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        ClusterNode::new(id, Registry::new(), "127.0.0.1:7000".into())
    }

    #[test]
    fn the_identity_and_the_trust_list_load_from_ONE_directory() {
        // They are one setting: an identity without its registry is a machine nobody trusts, and a
        // registry without its identity is a list of machines that cannot recognise you. Backing up
        // the directory has to keep the pair, or a restore produces a half-configured node.
        let dir = std::env::temp_dir().join(format!("logscope-node-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut n, w) = ClusterNode::from_dir(&dir, "127.0.0.1:7000".into()).expect("creates");
        assert!(w.is_empty(), "a fresh directory produces no warnings: {w:?}");
        let id = n.identity.id();

        n.registry.register_keyed("m2", Trust::Handoff, vec![1, 2, 3]);
        n.save_registry(&dir).expect("saves");

        // A second node from the same directory is the SAME machine with the SAME trust list.
        let (n2, _) = ClusterNode::from_dir(&dir, "127.0.0.1:7000".into()).unwrap();
        assert_eq!(n2.identity.id(), id, "the identity persisted");
        assert_eq!(n2.registry.level("m2"), Trust::Handoff, "and so did the trust list");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_socket_failure_is_RECORDED_not_fatal() {
        // A phone with multicast blocked must still show its own id and its trust list — those are
        // exactly what an operator needs in order to fix the situation.
        let n = node("sockfail");
        assert!(!n.identity.id().is_empty(), "the id is available regardless");
        if let Some(e) = &n.wire_error {
            assert!(e.contains("only LAN auto-discovery is off"), "and the message scopes the loss");
        }
    }

    #[test]
    fn locate_refuses_an_UNREGISTERED_id_even_if_it_is_in_the_table() {
        // A convenience accessor must not bypass the guard rendezvous::resolve applies. Discovery
        // populating the table is not the same as an operator vouching for a machine.
        let mut n = node("locate");
        n.discovery.observed("stranger", crate::rendezvous::Candidate {
            addr: "10.0.0.9:7000".into(), route: crate::rendezvous::Route::Lan, age_s: 0 });
        assert!(n.locate("stranger").is_none(), "in the table and still not resolvable");
        n.registry.register_keyed("stranger", Trust::Compute, vec![1]);
        assert!(n.locate("stranger").is_some(), "registered: now it resolves");
    }

    #[test]
    fn polling_ages_the_table_even_with_no_socket() {
        // Ageing is not conditional on the wire. A node whose socket failed must still forget peers,
        // or a stale address survives forever precisely when discovery cannot refresh it.
        let mut n = node("age");
        n.registry.register_keyed("m2", Trust::Compute, vec![1]);
        n.discovery.observed("m2", crate::rendezvous::Candidate {
            addr: "10.0.0.5:7000".into(), route: crate::rendezvous::Route::Lan, age_s: 0 });
        n.poll(crate::rendezvous::MAX_AGE_S as f32 + 1.0, 1000);
        assert!(n.locate("m2").is_none(), "aged out with no socket involved");
    }

    #[test]
    fn the_first_poll_does_not_announce_immediately() {
        // tick() only fires on the interval, so a node that is constructed and polled once should not
        // emit — otherwise a UI that rebuilds the node on a settings change would broadcast per edit.
        let mut n = node("firstpoll");
        n.poll(0.016, 1000);
        assert!(n.discovery.peers().is_empty());
    }
}
